//! Calendar invitations (iCalendar, RFC 5545) — parse events and build RSVP replies.
//!
//! The parser unfolds content lines, understands property parameters (including
//! quoted values), decodes escaped text and resolves date-times given in UTC
//! (`Z` suffix), as floating local time, as all-day `DATE` values, or with a
//! `TZID`. Time zones are resolved from the calendar's own `VTIMEZONE`
//! definitions when present, otherwise from a built-in table of common zones
//! (IANA and Windows names); unknown zones fall back to local time.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub organizer: String,
    pub attendees: Vec<String>,
    /// unix seconds
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub all_day: bool,
    pub method: String,
    pub sequence: i64,
    pub raw: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rsvp {
    Accept,
    Tentative,
    Decline,
}

impl Rsvp {
    /// The `PARTSTAT` parameter value for this response.
    pub fn partstat(&self) -> &'static str {
        match self {
            Rsvp::Accept => "ACCEPTED",
            Rsvp::Tentative => "TENTATIVE",
            Rsvp::Decline => "DECLINED",
        }
    }
    /// Human readable verb, e.g. for the subject of the reply message ("Accepted: …").
    pub fn label(&self) -> &'static str {
        match self {
            Rsvp::Accept => "Accepted",
            Rsvp::Tentative => "Tentative",
            Rsvp::Decline => "Declined",
        }
    }
}

/// A participant (organizer or attendee) with its parameters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Participant {
    pub email: String,
    pub name: String,
    /// `PARTSTAT` (e.g. NEEDS-ACTION, ACCEPTED), empty if absent.
    pub partstat: String,
    /// `ROLE` (e.g. REQ-PARTICIPANT), empty if absent.
    pub role: String,
    pub rsvp: bool,
}

/// One content line: `NAME;PARAM=VALUE:value`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Property {
    /// Upper-case property name.
    pub name: String,
    /// Parameters with upper-case names and unquoted values.
    pub params: Vec<(String, String)>,
    /// Raw (still escaped) value.
    pub value: String,
}

impl Property {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Undo RFC 5545 line folding (CRLF or LF followed by a space or tab).
pub fn unfold(s: &str) -> String {
    let s = s.replace("\r\n", "\n");
    let mut out = String::with_capacity(s.len());
    for line in s.split('\n') {
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            out.push_str(rest);
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    out
}

/// Parse a single unfolded content line.
pub fn parse_line(line: &str) -> Option<Property> {
    let line = line.trim_end_matches('\r');
    if line.trim().is_empty() {
        return None;
    }
    let mut name_end = None;
    for (i, c) in line.char_indices() {
        if c == ';' || c == ':' {
            name_end = Some(i);
            break;
        }
    }
    let name_end = name_end?;
    let name = line[..name_end].trim().to_ascii_uppercase();
    let mut params = Vec::new();
    let mut rest = &line[name_end..];
    while let Some(r) = rest.strip_prefix(';') {
        // Parameter name
        let eq = r.find(['=', ':', ';']).unwrap_or(r.len());
        let pname = r[..eq].trim().to_ascii_uppercase();
        let mut r2 = &r[eq..];
        let mut values: Vec<String> = Vec::new();
        if let Some(v) = r2.strip_prefix('=') {
            r2 = v;
            loop {
                if let Some(q) = r2.strip_prefix('"') {
                    let close = q.find('"').unwrap_or(q.len());
                    values.push(q[..close].to_string());
                    r2 = q.get(close + 1..).unwrap_or("");
                } else {
                    let end = r2.find([',', ';', ':']).unwrap_or(r2.len());
                    values.push(r2[..end].to_string());
                    r2 = &r2[end..];
                }
                match r2.strip_prefix(',') {
                    Some(n) => r2 = n,
                    None => break,
                }
            }
        }
        params.push((pname, values.join(",")));
        rest = r2;
    }
    let value = rest.strip_prefix(':').unwrap_or(rest).to_string();
    Some(Property { name, params, value })
}

/// Decode an escaped TEXT value.
pub fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o)
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Escape a value for use as TEXT.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Fold a content line at 75 octets (continuations start with a space), CRLF terminated.
pub fn fold_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + line.len() / 70 * 3 + 2);
    let mut count = 0usize;
    let mut first = true;
    for c in line.chars() {
        let limit = if first { 75 } else { 74 };
        let len = c.len_utf8();
        if count + len > limit {
            out.push_str("\r\n ");
            count = 0;
            first = false;
        }
        out.push(c);
        count += len;
    }
    out.push_str("\r\n");
    out
}

fn strip_mailto(v: &str) -> String {
    let v = v.trim();
    let lower = v.to_ascii_lowercase();
    if lower.starts_with("mailto:") { v[7..].trim().to_string() } else { v.to_string() }
}

fn participant(p: &Property) -> Participant {
    Participant {
        email: strip_mailto(&p.value),
        name: p.param("CN").unwrap_or("").to_string(),
        partstat: p.param("PARTSTAT").unwrap_or("").to_ascii_uppercase(),
        role: p.param("ROLE").unwrap_or("").to_ascii_uppercase(),
        rsvp: p.param("RSVP").is_some_and(|v| v.eq_ignore_ascii_case("TRUE")),
    }
}

// ------------------------------------------------------------------ time zones

/// A recurring DST transition ("last Sunday of March at 02:00 local").
#[derive(Clone, Copy, Debug, PartialEq)]
struct Transition {
    month: u32,
    /// 1..=5 for the nth weekday, negative for counting from the end (-1 = last).
    week: i32,
    weekday: Weekday,
    /// Seconds after local midnight (wall clock before the transition).
    at: i64,
}

impl Transition {
    fn date(&self, year: i32) -> Option<NaiveDate> {
        nth_weekday(year, self.month, self.week, self.weekday)
    }
    fn local(&self, year: i32) -> Option<NaiveDateTime> {
        Some(self.date(year)?.and_time(NaiveTime::MIN) + Duration::seconds(self.at))
    }
}

fn nth_weekday(year: i32, month: u32, week: i32, wd: Weekday) -> Option<NaiveDate> {
    if week > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let delta = (wd.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64).rem_euclid(7);
        let d = first + Duration::days(delta + 7 * (week as i64 - 1));
        // "5th Sunday" that does not exist means the last one.
        if d.month() != month { Some(d - Duration::days(7)) } else { Some(d) }
    } else {
        let next_month = if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(year, month + 1, 1)? };
        let last = next_month - Duration::days(1);
        let delta = (last.weekday().num_days_from_monday() as i64 - wd.num_days_from_monday() as i64).rem_euclid(7);
        Some(last - Duration::days(delta + 7 * ((-week) as i64 - 1)))
    }
}

/// Offset rules of a time zone: standard offset plus optional daylight saving time.
#[derive(Clone, Debug, PartialEq)]
struct TzRule {
    std: i64,
    dst: Option<(i64, Transition, Transition)>,
}

impl TzRule {
    fn fixed(std: i64) -> Self {
        TzRule { std, dst: None }
    }
    /// Offset (seconds east of UTC) in effect at a local wall-clock time.
    fn offset_at(&self, local: NaiveDateTime) -> i64 {
        let Some((dst, start, end)) = &self.dst else { return self.std };
        let y = local.year();
        let (Some(s), Some(e)) = (start.local(y), end.local(y)) else { return self.std };
        let in_dst = if s <= e { local >= s && local < e } else { local >= s || local < e };
        if in_dst { *dst } else { self.std }
    }
}

const H: i64 = 3600;

fn eu(std_hours: i64) -> TzRule {
    // EU: last Sunday of March / October at 01:00 UTC.
    let std = std_hours * H;
    TzRule {
        std,
        dst: Some((std + H, Transition { month: 3, week: -1, weekday: Weekday::Sun, at: H + std }, Transition { month: 10, week: -1, weekday: Weekday::Sun, at: H + std + H })),
    }
}

fn us(std_hours: i64) -> TzRule {
    // US / Canada: second Sunday of March 02:00 to first Sunday of November 02:00 (local).
    let std = std_hours * H;
    TzRule {
        std,
        dst: Some((std + H, Transition { month: 3, week: 2, weekday: Weekday::Sun, at: 2 * H }, Transition { month: 11, week: 1, weekday: Weekday::Sun, at: 2 * H })),
    }
}

fn builtin_zone(tzid: &str) -> Option<TzRule> {
    let id = tzid.trim().trim_matches('"');
    let lower = id.to_ascii_lowercase();
    let eu1 = [
        "europe/berlin", "europe/paris", "europe/amsterdam", "europe/brussels", "europe/vienna", "europe/zurich", "europe/rome", "europe/madrid",
        "europe/stockholm", "europe/oslo", "europe/copenhagen", "europe/prague", "europe/warsaw", "europe/budapest", "europe/luxembourg",
        "europe/belgrade", "europe/zagreb", "europe/ljubljana", "europe/bratislava", "europe/monaco", "europe/malta", "cet", "met",
        "w. europe standard time", "central europe standard time", "central european standard time", "romance standard time",
    ];
    let eu0 = ["europe/london", "europe/dublin", "europe/lisbon", "atlantic/canary", "wet", "gmt standard time"];
    let eu2 = [
        "europe/athens", "europe/helsinki", "europe/kiev", "europe/kyiv", "europe/bucharest", "europe/sofia", "europe/riga", "europe/tallinn",
        "europe/vilnius", "eet", "e. europe standard time", "fle standard time", "gtb standard time",
    ];
    let matches = |list: &[&str]| list.iter().any(|z| lower == *z || lower.ends_with(&format!("/{z}")));
    if matches(&eu1) {
        return Some(eu(1));
    }
    if matches(&eu0) {
        return Some(eu(0));
    }
    if matches(&eu2) {
        return Some(eu(2));
    }
    let us_zones: [(&[&str], i64); 5] = [
        (&["america/new_york", "america/toronto", "america/detroit", "us/eastern", "est5edt", "eastern standard time"], -5),
        (&["america/chicago", "america/winnipeg", "us/central", "cst6cdt", "central standard time"], -6),
        (&["america/denver", "america/edmonton", "us/mountain", "mst7mdt", "mountain standard time"], -7),
        (&["america/los_angeles", "america/vancouver", "us/pacific", "pst8pdt", "pacific standard time"], -8),
        (&["america/anchorage", "us/alaska", "alaskan standard time"], -9),
    ];
    for (list, off) in us_zones {
        if matches(list) {
            return Some(us(off));
        }
    }
    let fixed: [(&[&str], i64); 14] = [
        (&["utc", "gmt", "etc/utc", "etc/gmt", "z", "zulu", "universal", "coordinated universal time"], 0),
        (&["america/phoenix", "us/arizona", "us mountain standard time"], -7 * H),
        (&["pacific/honolulu", "us/hawaii", "hawaiian standard time"], -10 * H),
        (&["america/sao_paulo", "e. south america standard time"], -3 * H),
        (&["europe/moscow", "europe/istanbul", "europe/minsk", "russian standard time", "turkey standard time"], 3 * H),
        (&["asia/dubai", "arabian standard time"], 4 * H),
        (&["asia/kolkata", "asia/calcutta", "india standard time"], 5 * H + 30 * 60),
        (&["asia/bangkok", "asia/jakarta", "asia/ho_chi_minh", "se asia standard time"], 7 * H),
        (&["asia/shanghai", "asia/hong_kong", "asia/singapore", "asia/taipei", "australia/perth", "china standard time", "singapore standard time"], 8 * H),
        (&["asia/tokyo", "asia/seoul", "tokyo standard time", "korea standard time"], 9 * H),
        (&["australia/brisbane", "e. australia standard time"], 10 * H),
        (&["asia/karachi", "pakistan standard time"], 5 * H),
        (&["africa/johannesburg", "south africa standard time"], 2 * H),
        (&["africa/lagos", "w. central africa standard time"], H),
    ];
    for (list, off) in fixed {
        if matches(list) {
            return Some(TzRule::fixed(off));
        }
    }
    if matches(&["australia/sydney", "australia/melbourne", "australia/canberra", "australia/hobart", "aus eastern standard time"]) {
        return Some(TzRule {
            std: 10 * H,
            dst: Some((11 * H, Transition { month: 10, week: 1, weekday: Weekday::Sun, at: 2 * H }, Transition { month: 4, week: 1, weekday: Weekday::Sun, at: 3 * H })),
        });
    }
    if matches(&["pacific/auckland", "new zealand standard time"]) {
        return Some(TzRule {
            std: 12 * H,
            dst: Some((13 * H, Transition { month: 9, week: -1, weekday: Weekday::Sun, at: 2 * H }, Transition { month: 4, week: 1, weekday: Weekday::Sun, at: 3 * H })),
        });
    }
    // "UTC+01:00" / "GMT-5" style identifiers
    for prefix in ["utc", "gmt"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if let Some(off) = parse_offset(rest.trim()) {
                return Some(TzRule::fixed(off));
            }
        }
    }
    None
}

/// Parse "+0100", "-05:30", "+1" into seconds.
fn parse_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    let (sign, rest) = match s.chars().next()? {
        '+' => (1, &s[1..]),
        '-' => (-1, &s[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    let (h, m, sec) = match digits.len() {
        1 | 2 => (digits.parse::<i64>().ok()?, 0, 0),
        3 => (digits[..1].parse::<i64>().ok()?, digits[1..].parse::<i64>().ok()?, 0),
        4 => (digits[..2].parse::<i64>().ok()?, digits[2..].parse::<i64>().ok()?, 0),
        6 => (digits[..2].parse::<i64>().ok()?, digits[2..4].parse::<i64>().ok()?, digits[4..].parse::<i64>().ok()?),
        _ => return None,
    };
    if h > 14 || m > 59 {
        return None;
    }
    Some(sign * (h * H + m * 60 + sec))
}

/// Observance (STANDARD / DAYLIGHT) collected from a VTIMEZONE.
#[derive(Default, Clone, Debug)]
struct Observance {
    daylight: bool,
    offset_to: Option<i64>,
    start: Option<NaiveDateTime>,
    rrule: String,
}

fn transition_from(obs: &Observance) -> Option<Transition> {
    let start = obs.start?;
    let mut month = start.month();
    let mut week: Option<i32> = None;
    let mut weekday = start.weekday();
    for part in obs.rrule.split(';') {
        let (k, v) = part.split_once('=')?;
        match k.to_ascii_uppercase().as_str() {
            "BYMONTH" => month = v.split(',').next()?.parse().ok()?,
            "BYDAY" => {
                let v = v.split(',').next()?.trim();
                let split = v.len().checked_sub(2)?;
                let (n, d) = v.split_at(split);
                weekday = match d.to_ascii_uppercase().as_str() {
                    "MO" => Weekday::Mon,
                    "TU" => Weekday::Tue,
                    "WE" => Weekday::Wed,
                    "TH" => Weekday::Thu,
                    "FR" => Weekday::Fri,
                    "SA" => Weekday::Sat,
                    "SU" => Weekday::Sun,
                    _ => return None,
                };
                week = Some(if n.is_empty() { 1 } else { n.trim_start_matches('+').parse().ok()? });
            }
            _ => {}
        }
    }
    let week = week.unwrap_or_else(|| ((start.day() as i32 - 1) / 7) + 1);
    let at = start.time().signed_duration_since(NaiveTime::MIN).num_seconds();
    Some(Transition { month, week, weekday, at })
}

fn rule_from_observances(obs: &[Observance]) -> Option<TzRule> {
    let std = obs.iter().filter(|o| !o.daylight).max_by_key(|o| o.start);
    let dst = obs.iter().filter(|o| o.daylight).max_by_key(|o| o.start);
    match (std, dst) {
        (Some(s), Some(d)) => {
            let std_off = s.offset_to?;
            let dst_off = d.offset_to?;
            match (transition_from(d), transition_from(s)) {
                (Some(start), Some(end)) if !d.rrule.is_empty() && !s.rrule.is_empty() => Some(TzRule { std: std_off, dst: Some((dst_off, start, end)) }),
                _ => Some(TzRule::fixed(std_off)),
            }
        }
        (Some(s), None) => Some(TzRule::fixed(s.offset_to?)),
        (None, Some(d)) => Some(TzRule::fixed(d.offset_to?)),
        (None, None) => None,
    }
}

fn parse_naive(v: &str) -> Option<(NaiveDateTime, bool, bool)> {
    // Returns (datetime, is_date_only, is_utc)
    let v = v.trim();
    if v.len() == 8 && v.chars().all(|c| c.is_ascii_digit()) {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((d.and_time(NaiveTime::MIN), true, false));
    }
    let utc = v.ends_with('Z') || v.ends_with('z');
    let core = v.trim_end_matches(['Z', 'z']);
    let dt = NaiveDateTime::parse_from_str(core, "%Y%m%dT%H%M%S").or_else(|_| NaiveDateTime::parse_from_str(core, "%Y%m%dT%H%M")).ok()?;
    Some((dt, false, utc))
}

fn local_to_ts(dt: NaiveDateTime) -> Option<i64> {
    Local.from_local_datetime(&dt).earliest().or_else(|| Local.from_local_datetime(&(dt + Duration::hours(1))).earliest()).map(|d| d.timestamp())
}

/// Parse an RFC 5545 DURATION such as "PT1H30M", "P1D", "-P1W".
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (sign, rest) = if let Some(r) = s.strip_prefix('-') { (-1, r) } else { (1, s.strip_prefix('+').unwrap_or(s)) };
    let rest = rest.strip_prefix(['P', 'p'])?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c.to_ascii_uppercase() {
            'T' => in_time = true,
            d if d.is_ascii_digit() => num.push(d),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (unit, in_time) {
                    ('W', _) => n * 7 * 86400,
                    ('D', _) => n * 86400,
                    ('H', true) => n * 3600,
                    ('M', true) => n * 60,
                    ('S', true) => n,
                    _ => return None,
                };
            }
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(sign * total)
}

// ------------------------------------------------------------------ parsing

struct Parsed {
    event: CalendarEvent,
    organizer: Option<Participant>,
    attendees: Vec<Participant>,
}

fn parse_full(ics: &str) -> Option<Parsed> {
    let unfolded = unfold(ics);
    let props: Vec<Property> = unfolded.lines().filter_map(parse_line).collect();
    if !props.iter().any(|p| p.name == "BEGIN" && p.value.trim().eq_ignore_ascii_case("VEVENT")) {
        return None;
    }

    // First pass: time zone definitions.
    let mut zones: Vec<(String, TzRule)> = Vec::new();
    {
        let mut stack: Vec<String> = Vec::new();
        let mut tzid = String::new();
        let mut obs: Vec<Observance> = Vec::new();
        let mut cur: Option<Observance> = None;
        for p in &props {
            match p.name.as_str() {
                "BEGIN" => {
                    let comp = p.value.trim().to_ascii_uppercase();
                    if comp == "VTIMEZONE" {
                        tzid.clear();
                        obs.clear();
                    } else if (comp == "STANDARD" || comp == "DAYLIGHT") && stack.last().map(|s| s == "VTIMEZONE").unwrap_or(false) {
                        cur = Some(Observance { daylight: comp == "DAYLIGHT", ..Default::default() });
                    }
                    stack.push(comp);
                }
                "END" => {
                    let comp = p.value.trim().to_ascii_uppercase();
                    if comp == "STANDARD" || comp == "DAYLIGHT" {
                        if let Some(o) = cur.take() {
                            obs.push(o);
                        }
                    } else if comp == "VTIMEZONE" && !tzid.is_empty() {
                        if let Some(rule) = rule_from_observances(&obs) {
                            zones.push((tzid.clone(), rule));
                        }
                    }
                    stack.pop();
                }
                "TZID" if stack.last().map(|s| s == "VTIMEZONE").unwrap_or(false) => tzid = p.value.trim().to_string(),
                "TZOFFSETTO" => {
                    if let Some(o) = cur.as_mut() {
                        o.offset_to = parse_offset(&p.value);
                    }
                }
                "DTSTART" => {
                    if let Some(o) = cur.as_mut() {
                        o.start = parse_naive(&p.value).map(|x| x.0);
                    }
                }
                "RRULE" => {
                    if let Some(o) = cur.as_mut() {
                        o.rrule = p.value.trim().to_string();
                    }
                }
                _ => {}
            }
        }
    }

    let resolve = |p: &Property| -> Option<(i64, bool)> {
        let (dt, date_only, utc) = parse_naive(&p.value)?;
        let date_only = date_only || p.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
        if date_only {
            return Some((local_to_ts(dt.date().and_time(NaiveTime::MIN))?, true));
        }
        if utc {
            return Some((Utc.from_utc_datetime(&dt).timestamp(), false));
        }
        if let Some(tzid) = p.param("TZID") {
            let rule = zones.iter().find(|(id, _)| id.eq_ignore_ascii_case(tzid.trim())).map(|(_, r)| r.clone()).or_else(|| builtin_zone(tzid));
            if let Some(rule) = rule {
                let off = rule.offset_at(dt);
                return Some((dt.and_utc().timestamp() - off, false));
            }
        }
        Some((local_to_ts(dt)?, false))
    };

    let mut ev = CalendarEvent { raw: ics.to_string(), ..Default::default() };
    let mut organizer = None;
    let mut attendees = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut seen_event = false;
    let mut in_first_event = false;
    let mut duration: Option<i64> = None;
    for p in &props {
        match p.name.as_str() {
            "BEGIN" => {
                let comp = p.value.trim().to_ascii_uppercase();
                if comp == "VEVENT" && !seen_event {
                    seen_event = true;
                    in_first_event = true;
                }
                stack.push(comp);
                continue;
            }
            "END" => {
                let comp = p.value.trim().to_ascii_uppercase();
                if comp == "VEVENT" {
                    in_first_event = false;
                }
                stack.pop();
                continue;
            }
            _ => {}
        }
        let top = stack.last().map(|s| s.as_str()).unwrap_or("");
        if top == "VCALENDAR" && p.name == "METHOD" {
            ev.method = p.value.trim().to_ascii_uppercase();
            continue;
        }
        if !in_first_event || top != "VEVENT" {
            continue;
        }
        match p.name.as_str() {
            "UID" => ev.uid = p.value.trim().to_string(),
            "SUMMARY" => ev.summary = unescape_text(&p.value),
            "DESCRIPTION" => ev.description = unescape_text(&p.value),
            "LOCATION" => ev.location = unescape_text(&p.value),
            "SEQUENCE" => ev.sequence = p.value.trim().parse().unwrap_or(0),
            "ORGANIZER" => {
                let part = participant(p);
                ev.organizer = part.email.clone();
                organizer = Some(part);
            }
            "ATTENDEE" => {
                let part = participant(p);
                ev.attendees.push(part.email.clone());
                attendees.push(part);
            }
            "DTSTART" => {
                if let Some((ts, all_day)) = resolve(p) {
                    ev.start = Some(ts);
                    ev.all_day = all_day;
                }
            }
            "DTEND" => ev.end = resolve(p).map(|x| x.0),
            "DURATION" => duration = parse_duration(&p.value),
            _ => {}
        }
    }
    if ev.end.is_none() {
        if let Some(s) = ev.start {
            ev.end = match duration {
                Some(d) => Some(s + d),
                None if ev.all_day => Some(s + 86400),
                None => None,
            };
        }
    }
    Some(Parsed { event: ev, organizer, attendees })
}

pub fn parse(ics: &str) -> Option<CalendarEvent> {
    parse_full(ics).map(|p| p.event)
}

/// Organizer and attendees of the (first) event with names and participation status.
pub fn participants(ics: &str) -> (Option<Participant>, Vec<Participant>) {
    match parse_full(ics) {
        Some(p) => (p.organizer, p.attendees),
        None => (None, Vec::new()),
    }
}

/// Participation status of `email` in the event, if it is an attendee.
pub fn partstat_for(ev: &CalendarEvent, email: &str) -> Option<String> {
    participants(&ev.raw).1.into_iter().find(|a| a.email.eq_ignore_ascii_case(email)).map(|a| a.partstat)
}

fn fmt_utc(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0).map(|d| d.format("%Y%m%dT%H%M%SZ").to_string()).unwrap_or_default()
}

fn fmt_date(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&Local).format("%Y%m%d").to_string()).unwrap_or_default()
}

fn quote_param(v: &str) -> String {
    let clean: String = v.chars().filter(|c| *c != '"' && !c.is_control()).collect();
    if clean.contains([':', ';', ',']) { format!("\"{clean}\"") } else { clean }
}

/// Build a METHOD:REPLY calendar object answering `ev` as `attendee_email`.
pub fn build_reply(ev: &CalendarEvent, attendee_email: &str, rsvp: Rsvp) -> String {
    let (organizer, attendees) = participants(&ev.raw);
    let me = attendees.iter().find(|a| a.email.eq_ignore_ascii_case(attendee_email));
    let mut lines: Vec<String> = vec![
        "BEGIN:VCALENDAR".into(),
        "VERSION:2.0".into(),
        "PRODID:-//Purpamail//Purpamail Calendar//EN".into(),
        "METHOD:REPLY".into(),
        "BEGIN:VEVENT".into(),
        format!("UID:{}", ev.uid),
        format!("DTSTAMP:{}", fmt_utc(Utc::now().timestamp())),
    ];
    if ev.sequence != 0 {
        lines.push(format!("SEQUENCE:{}", ev.sequence));
    }
    // Copy RECURRENCE-ID verbatim if the invitation targets a single occurrence.
    let unfolded = unfold(&ev.raw);
    if let Some(rid) = unfolded.lines().find(|l| l.to_ascii_uppercase().starts_with("RECURRENCE-ID")) {
        lines.push(rid.trim_end().to_string());
    }
    if let Some(s) = ev.start {
        if ev.all_day {
            lines.push(format!("DTSTART;VALUE=DATE:{}", fmt_date(s)));
            if let Some(e) = ev.end {
                lines.push(format!("DTEND;VALUE=DATE:{}", fmt_date(e)));
            }
        } else {
            lines.push(format!("DTSTART:{}", fmt_utc(s)));
            if let Some(e) = ev.end {
                lines.push(format!("DTEND:{}", fmt_utc(e)));
            }
        }
    }
    if !ev.summary.is_empty() {
        lines.push(format!("SUMMARY:{}", escape_text(&ev.summary)));
    }
    if !ev.location.is_empty() {
        lines.push(format!("LOCATION:{}", escape_text(&ev.location)));
    }
    match &organizer {
        Some(o) if !o.name.is_empty() => lines.push(format!("ORGANIZER;CN={}:mailto:{}", quote_param(&o.name), o.email)),
        Some(o) => lines.push(format!("ORGANIZER:mailto:{}", o.email)),
        None if !ev.organizer.is_empty() => lines.push(format!("ORGANIZER:mailto:{}", ev.organizer)),
        None => {}
    }
    let mut att = format!("ATTENDEE;PARTSTAT={}", rsvp.partstat());
    if let Some(m) = me {
        if !m.role.is_empty() {
            att.push_str(&format!(";ROLE={}", m.role));
        }
        if !m.name.is_empty() {
            att.push_str(&format!(";CN={}", quote_param(&m.name)));
        }
    }
    att.push_str(&format!(":mailto:{}", me.map(|m| m.email.as_str()).unwrap_or(attendee_email)));
    lines.push(att);
    lines.push("END:VEVENT".into());
    lines.push("END:VCALENDAR".into());
    lines.iter().map(|l| fold_line(l)).collect()
}

/// Human readable time range of an event in local time.
///
/// Examples: "Monday, 12 October 2026, 14:00 – 15:30", "Monday, 12 October 2026 (all day)",
/// "12 October 2026 – 14 October 2026 (all day)", "12 Oct 2026, 22:00 – 13 Oct 2026, 01:00".
pub fn format_event_time(ev: &CalendarEvent) -> String {
    let Some(start) = ev.start.and_then(|s| DateTime::from_timestamp(s, 0)).map(|d| d.with_timezone(&Local)) else {
        return String::new();
    };
    let end = ev.end.and_then(|e| DateTime::from_timestamp(e, 0)).map(|d| d.with_timezone(&Local));
    if ev.all_day {
        // DTEND of an all-day event is exclusive.
        let last = end.map(|e| e - Duration::days(1)).filter(|e| e.date_naive() > start.date_naive());
        return match last {
            Some(l) => format!("{} – {} (all day)", start.format("%-d %B %Y"), l.format("%-d %B %Y")),
            None => format!("{} (all day)", start.format("%A, %-d %B %Y")),
        };
    }
    match end {
        Some(e) if e.date_naive() == start.date_naive() => {
            format!("{}, {} – {}", start.format("%A, %-d %B %Y"), start.format("%H:%M"), e.format("%H:%M"))
        }
        Some(e) => format!("{} – {}", start.format("%-d %b %Y, %H:%M"), e.format("%-d %b %Y, %H:%M")),
        None => format!("{}, {}", start.format("%A, %-d %B %Y"), start.format("%H:%M")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nPRODID:-//Example Corp//Calendar//EN\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\n\
BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin\r\nBEGIN:DAYLIGHT\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200\r\nDTSTART:19700329T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nEND:DAYLIGHT\r\n\
BEGIN:STANDARD\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nDTSTART:19701025T030000\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:abc-123@example.com\r\nSEQUENCE:2\r\nDTSTAMP:20261001T080000Z\r\nDTSTART;TZID=Europe/Berlin:20261012T140000\r\nDTEND;TZID=Europe/Berlin:20261012T153000\r\n\
SUMMARY:Design review\\, round 2\r\nDESCRIPTION:Agenda:\\n1. Mockups\\n2. Next steps\r\nLOCATION:Room 4\\; 2nd floor\r\n\
ORGANIZER;CN=\"Doe: Jane\":mailto:jane@example.com\r\nATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Max Muster\r\n mann:mailto:max@example.org\r\n\
ATTENDEE;CN=Other;PARTSTAT=ACCEPTED:MAILTO:other@example.net\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_invitation() {
        let ev = parse(INVITE).unwrap();
        assert_eq!(ev.uid, "abc-123@example.com");
        assert_eq!(ev.method, "REQUEST");
        assert_eq!(ev.sequence, 2);
        assert_eq!(ev.summary, "Design review, round 2");
        assert_eq!(ev.description, "Agenda:\n1. Mockups\n2. Next steps");
        assert_eq!(ev.location, "Room 4; 2nd floor");
        assert_eq!(ev.organizer, "jane@example.com");
        assert_eq!(ev.attendees, vec!["max@example.org".to_string(), "other@example.net".to_string()]);
        // 14:00 CEST = 12:00 UTC
        assert_eq!(ev.start, Some(Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap().timestamp()));
        assert_eq!(ev.end, Some(Utc.with_ymd_and_hms(2026, 10, 12, 13, 30, 0).unwrap().timestamp()));
        assert!(!ev.all_day);
        let (org, att) = participants(INVITE);
        assert_eq!(org.unwrap().name, "Doe: Jane");
        assert_eq!(att[0].name, "Max Mustermann");
        assert!(att[0].rsvp);
        assert_eq!(partstat_for(&ev, "other@example.net").as_deref(), Some("ACCEPTED"));
    }

    #[test]
    fn builtin_zones_and_dst() {
        let winter = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:1\nDTSTART;TZID=W. Europe Standard Time:20261212T100000\nDTEND;TZID=America/New_York:20260712T100000\nEND:VEVENT\nEND:VCALENDAR";
        let ev = parse(winter).unwrap();
        assert_eq!(ev.start, Some(Utc.with_ymd_and_hms(2026, 12, 12, 9, 0, 0).unwrap().timestamp()));
        assert_eq!(ev.end, Some(Utc.with_ymd_and_hms(2026, 7, 12, 14, 0, 0).unwrap().timestamp()));
        let ny = TzRule::fixed(0);
        assert_eq!(ny.offset_at(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap().and_time(NaiveTime::MIN)), 0);
        assert_eq!(nth_weekday(2026, 3, -1, Weekday::Sun), NaiveDate::from_ymd_opt(2026, 3, 29));
        assert_eq!(nth_weekday(2026, 11, 1, Weekday::Sun), NaiveDate::from_ymd_opt(2026, 11, 1));
        assert_eq!(nth_weekday(2026, 3, 2, Weekday::Sun), NaiveDate::from_ymd_opt(2026, 3, 8));
    }

    #[test]
    fn all_day_utc_and_duration() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:2\nDTSTART;VALUE=DATE:20261224\nSUMMARY:Holiday\nEND:VEVENT\nEND:VCALENDAR";
        let ev = parse(ics).unwrap();
        assert!(ev.all_day);
        assert_eq!(ev.end.unwrap() - ev.start.unwrap(), 86400);
        assert!(format_event_time(&ev).contains("(all day)"));
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:3\nDTSTART:20261012T080000Z\nDURATION:PT1H30M\nEND:VEVENT\nEND:VCALENDAR";
        let ev = parse(ics).unwrap();
        assert_eq!(ev.start, Some(Utc.with_ymd_and_hms(2026, 10, 12, 8, 0, 0).unwrap().timestamp()));
        assert_eq!(ev.end.unwrap() - ev.start.unwrap(), 5400);
        assert!(format_event_time(&ev).contains(" – "));
        assert_eq!(parse_duration("P1W"), Some(604800));
        assert_eq!(parse_duration("-P1DT2H"), Some(-93600));
        assert_eq!(parse_duration("garbage"), None);
    }

    #[test]
    fn reply_is_well_formed() {
        let ev = parse(INVITE).unwrap();
        let r = build_reply(&ev, "MAX@example.org", Rsvp::Accept);
        assert!(r.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(r.ends_with("END:VCALENDAR\r\n"));
        assert!(r.contains("METHOD:REPLY\r\n"));
        assert!(r.contains("UID:abc-123@example.com\r\n"));
        assert!(r.contains("SEQUENCE:2\r\n"));
        for line in r.split("\r\n") {
            assert!(line.len() <= 75, "line too long: {line}");
        }
        let back = parse(&r).unwrap();
        assert_eq!(back.method, "REPLY");
        assert_eq!(back.start, ev.start);
        assert_eq!(back.summary, ev.summary);
        let (org, att) = participants(&r);
        assert_eq!(org.unwrap().name, "Doe: Jane");
        assert_eq!(att.len(), 1);
        assert_eq!(att[0].email, "max@example.org");
        assert_eq!(att[0].partstat, "ACCEPTED");
        assert_eq!(att[0].name, "Max Mustermann");
        let d = build_reply(&ev, "max@example.org", Rsvp::Decline);
        assert!(d.contains("PARTSTAT=DECLINED"));
    }

    #[test]
    fn folding_respects_utf8_and_length() {
        let long = format!("DESCRIPTION:{}", "ä".repeat(100));
        let folded = fold_line(&long);
        for l in folded.split("\r\n") {
            assert!(l.len() <= 75);
        }
        assert_eq!(unfold(&folded).trim_end(), long);
    }

    #[test]
    fn malformed_input() {
        assert!(parse("").is_none());
        assert!(parse("BEGIN:VCALENDAR\nEND:VCALENDAR").is_none());
        let ev = parse("BEGIN:VEVENT\nDTSTART:garbage\nATTENDEE;CN=\"unterminated:mailto:x@y\nEND:VEVENT").unwrap();
        assert_eq!(ev.start, None);
        assert_eq!(format_event_time(&ev), "");
    }
}
