//! A small CSS subset: declaration lists, style sheets with simple selectors,
//! colors and lengths — enough for the inline and embedded styles found in email.

use egui::Color32;

/// One `name: value` declaration.
#[derive(Clone, Debug, PartialEq)]
pub struct Decl {
    /// Lower-case property name.
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// A style rule from a `<style>` block, one per selector of a selector list.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub selector: String,
    pub specificity: u32,
    pub decls: Vec<Decl>,
}

/// A CSS length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Len {
    Px(f32),
    Percent(f32),
    Auto,
}

impl Len {
    /// Resolve against the size of the containing block.
    pub fn resolve(self, base: f32) -> Option<f32> {
        match self {
            Len::Px(v) => Some(v),
            Len::Percent(p) => Some(base * p / 100.0),
            Len::Auto => None,
        }
    }
}

/// Remove `/* ... */` comments.
pub fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Parse a declaration list (the contents of a `style` attribute or a rule body).
pub fn parse_declarations(s: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    // Split on ';' outside parentheses and quotes (url(data:...;base64,...) contains ';').
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0;
    let bytes: Vec<(usize, char)> = s.char_indices().collect();
    let push = |part: &str, out: &mut Vec<Decl>| {
        let Some((name, value)) = part.split_once(':') else { return };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return;
        }
        let mut value = value.trim().to_string();
        let mut important = false;
        if let Some(i) = value.to_ascii_lowercase().rfind("!important") {
            value.truncate(i);
            value = value.trim().to_string();
            important = true;
        }
        if !value.is_empty() {
            out.push(Decl { name, value, important });
        }
    };
    for &(i, c) in &bytes {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth = (depth - 1).max(0),
            (None, ';') if depth == 0 => {
                push(&s[start..i], &mut out);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        push(&s[start..], &mut out);
    }
    out
}

/// Parse a style sheet. At-rules (`@media`, `@font-face`, `@import`, ...) are skipped,
/// as their contents usually target other devices (e.g. narrow screens).
pub fn parse_stylesheet(css: &str) -> Vec<Rule> {
    let css = strip_comments(css).replace("<!--", " ").replace("-->", " ");
    let mut rules = Vec::new();
    let b = css.as_bytes();
    let mut i = 0;
    while i < b.len() {
        // skip whitespace
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        if b[i] == b'@' {
            // At-rule: either ends with ';' or contains a (nested) block.
            let mut j = i;
            while j < b.len() && b[j] != b';' && b[j] != b'{' {
                j += 1;
            }
            if j < b.len() && b[j] == b'{' {
                let mut depth = 0;
                while j < b.len() {
                    match b[j] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
            }
            i = j + 1;
            continue;
        }
        let Some(open) = css[i..].find('{').map(|o| o + i) else { break };
        let Some(close) = css[open..].find('}').map(|c| c + open) else { break };
        let selectors = &css[i..open];
        let decls = parse_declarations(&css[open + 1..close]);
        if !decls.is_empty() {
            for sel in selectors.split(',') {
                let sel = sel.trim();
                if sel.is_empty() || sel.contains("::") || sel.contains(":hover") || sel.contains(":active") || sel.contains(":focus") || sel.contains(":visited") {
                    continue;
                }
                rules.push(Rule { selector: sel.to_string(), specificity: specificity(sel), decls: decls.clone() });
            }
        }
        i = close + 1;
    }
    rules
}

/// Approximate selector specificity packed as `ids << 16 | classes << 8 | tags`.
pub fn specificity(sel: &str) -> u32 {
    let (mut ids, mut classes, mut tags) = (0u32, 0u32, 0u32);
    for compound in sel.split(|c: char| c.is_whitespace() || c == '>' || c == '+' || c == '~') {
        if compound.is_empty() {
            continue;
        }
        let mut first = true;
        for c in compound.chars() {
            match c {
                '#' => ids += 1,
                '.' | '[' => classes += 1,
                ':' => classes += 1,
                c if first && (c.is_ascii_alphabetic()) => tags += 1,
                _ => {}
            }
            first = false;
        }
    }
    (ids.min(255) << 16) | (classes.min(255) << 8) | tags.min(255)
}

/// Parse a CSS color. `transparent` yields [`Color32::TRANSPARENT`].
pub fn parse_color(s: &str) -> Option<Color32> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.as_str();
    if let Some(hex) = s.strip_prefix('#') {
        let hex: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        let v = u32::from_str_radix(&hex, 16).ok()?;
        return match hex.len() {
            3 => Some(Color32::from_rgb((((v >> 8) & 0xf) * 17) as u8, (((v >> 4) & 0xf) * 17) as u8, ((v & 0xf) * 17) as u8)),
            4 => Some(Color32::from_rgba_unmultiplied((((v >> 12) & 0xf) * 17) as u8, (((v >> 8) & 0xf) * 17) as u8, (((v >> 4) & 0xf) * 17) as u8, ((v & 0xf) * 17) as u8)),
            6 => Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)),
            8 => Some(Color32::from_rgba_unmultiplied((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)),
            _ => None,
        };
    }
    if s.starts_with("rgb") {
        let inner = s.split_once('(')?.1.trim_end_matches(')');
        let parts: Vec<&str> = inner.split([',', ' ', '/']).map(|p| p.trim()).filter(|p| !p.is_empty()).collect();
        if parts.len() < 3 {
            return None;
        }
        let comp = |p: &str| -> Option<u8> {
            if let Some(pc) = p.strip_suffix('%') {
                Some((pc.parse::<f32>().ok()? * 2.55).clamp(0.0, 255.0) as u8)
            } else {
                Some(p.parse::<f32>().ok()?.clamp(0.0, 255.0) as u8)
            }
        };
        let a = match parts.get(3) {
            Some(p) if p.ends_with('%') => (p.trim_end_matches('%').parse::<f32>().ok()? * 2.55).clamp(0.0, 255.0) as u8,
            Some(p) => (p.parse::<f32>().ok()? * 255.0).clamp(0.0, 255.0) as u8,
            None => 255,
        };
        return Some(Color32::from_rgba_unmultiplied(comp(parts[0])?, comp(parts[1])?, comp(parts[2])?, a));
    }
    if s == "transparent" {
        return Some(Color32::TRANSPARENT);
    }
    named_color(s)
}

fn named_color(s: &str) -> Option<Color32> {
    let v: u32 = match s {
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "orange" => 0xffa500,
        "purple" => 0x800080,
        "gray" | "grey" => 0x808080,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "navy" => 0x000080,
        "teal" => 0x008080,
        "olive" => 0x808000,
        "lime" => 0x00ff00,
        "aqua" | "cyan" => 0x00ffff,
        "fuchsia" | "magenta" => 0xff00ff,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "dimgray" | "dimgrey" => 0x696969,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "gainsboro" => 0xdcdcdc,
        "whitesmoke" => 0xf5f5f5,
        "darkblue" => 0x00008b,
        "darkred" => 0x8b0000,
        "darkgreen" => 0x006400,
        "darkorange" => 0xff8c00,
        "lightblue" => 0xadd8e6,
        "lightgreen" => 0x90ee90,
        "lightyellow" => 0xffffe0,
        "royalblue" => 0x4169e1,
        "steelblue" => 0x4682b4,
        "dodgerblue" => 0x1e90ff,
        "skyblue" => 0x87ceeb,
        "midnightblue" => 0x191970,
        "slategray" | "slategrey" => 0x708090,
        "crimson" => 0xdc143c,
        "firebrick" => 0xb22222,
        "tomato" => 0xff6347,
        "coral" => 0xff7f50,
        "gold" => 0xffd700,
        "goldenrod" => 0xdaa520,
        "pink" => 0xffc0cb,
        "hotpink" => 0xff69b4,
        "indigo" => 0x4b0082,
        "violet" => 0xee82ee,
        "orchid" => 0xda70d6,
        "brown" => 0xa52a2a,
        "chocolate" => 0xd2691e,
        "tan" => 0xd2b48c,
        "beige" => 0xf5f5dc,
        "ivory" => 0xfffff0,
        "linen" => 0xfaf0e6,
        "lavender" => 0xe6e6fa,
        "aliceblue" => 0xf0f8ff,
        "honeydew" => 0xf0fff0,
        "mintcream" => 0xf5fffa,
        "seagreen" => 0x2e8b57,
        "forestgreen" => 0x228b22,
        "limegreen" => 0x32cd32,
        "turquoise" => 0x40e0d0,
        "salmon" => 0xfa8072,
        "khaki" => 0xf0e68c,
        _ => return None,
    };
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Parse a length. `em_base` resolves `em` (the element's font size); `rem` uses 16px.
pub fn parse_len(s: &str, em_base: f32) -> Option<Len> {
    let s = s.trim().to_ascii_lowercase();
    if s == "auto" {
        return Some(Len::Auto);
    }
    if s == "0" {
        return Some(Len::Px(0.0));
    }
    let num_end = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(s.len());
    let (num, unit) = s.split_at(num_end);
    let v: f32 = num.parse().ok()?;
    if !v.is_finite() {
        return None;
    }
    Some(match unit.trim() {
        "" | "px" => Len::Px(v),
        "pt" => Len::Px(v * 4.0 / 3.0),
        "pc" => Len::Px(v * 16.0),
        "in" => Len::Px(v * 96.0),
        "cm" => Len::Px(v * 96.0 / 2.54),
        "mm" => Len::Px(v * 96.0 / 25.4),
        "em" => Len::Px(v * em_base),
        "rem" => Len::Px(v * 16.0),
        "ex" | "ch" => Len::Px(v * em_base * 0.5),
        "%" => Len::Percent(v),
        "vw" | "vh" => Len::Percent(v),
        _ => return None,
    })
}

/// Resolve a `font-size` value given the parent's size.
pub fn parse_font_size(s: &str, parent: f32, base: f32) -> Option<f32> {
    let k = s.trim().to_ascii_lowercase();
    let scale = base / 16.0;
    let v = match k.as_str() {
        "xx-small" => 9.0 * scale,
        "x-small" => 10.0 * scale,
        "small" => 13.0 * scale,
        "medium" => 16.0 * scale,
        "large" => 18.0 * scale,
        "x-large" => 24.0 * scale,
        "xx-large" => 32.0 * scale,
        "xxx-large" => 48.0 * scale,
        "smaller" => parent * 0.83,
        "larger" => parent * 1.2,
        _ => match parse_len(&k, parent)? {
            Len::Px(v) => v,
            Len::Percent(p) => parent * p / 100.0,
            Len::Auto => return None,
        },
    };
    Some(v.clamp(4.0, 120.0))
}

/// Size for the legacy `<font size=N>` attribute (1..7, or relative +N / -N).
pub fn font_tag_size(attr: &str, parent: f32, base: f32) -> Option<f32> {
    let a = attr.trim();
    let n: i32 = if let Some(r) = a.strip_prefix('+') {
        3 + r.parse::<i32>().ok()?
    } else if let Some(r) = a.strip_prefix('-') {
        3 - r.parse::<i32>().ok()?
    } else {
        a.parse().ok()?
    };
    let _ = parent;
    let px = match n.clamp(1, 7) {
        1 => 10.0,
        2 => 13.0,
        3 => 16.0,
        4 => 18.0,
        5 => 24.0,
        6 => 32.0,
        _ => 48.0,
    };
    Some(px * base / 16.0)
}

/// Expand a 1–4 value shorthand (margin, padding) into top, right, bottom, left.
pub fn parse_sides(s: &str, em_base: f32) -> [Option<Len>; 4] {
    let v: Vec<Option<Len>> = s.split_whitespace().map(|p| parse_len(p, em_base)).collect();
    match v.len() {
        1 => [v[0], v[0], v[0], v[0]],
        2 => [v[0], v[1], v[0], v[1]],
        3 => [v[0], v[1], v[2], v[1]],
        n if n >= 4 => [v[0], v[1], v[2], v[3]],
        _ => [None; 4],
    }
}

/// Parse a `border` shorthand (or one side) into width and color.
pub fn parse_border(s: &str, em_base: f32) -> Option<(f32, Option<Color32>)> {
    let s = s.trim().to_ascii_lowercase();
    if s == "none" || s == "0" || s == "hidden" {
        return Some((0.0, None));
    }
    let mut width = None;
    let mut color = None;
    let mut style_none = false;
    for part in split_value(&s) {
        match part.as_str() {
            "none" | "hidden" => style_none = true,
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {}
            "thin" => width = Some(1.0),
            "medium" => width = Some(3.0),
            "thick" => width = Some(5.0),
            p => {
                if let Some(Len::Px(w)) = parse_len(p, em_base) {
                    width = Some(w);
                } else if let Some(c) = parse_color(p) {
                    color = Some(c);
                }
            }
        }
    }
    if style_none {
        return Some((0.0, None));
    }
    Some((width.unwrap_or(if color.is_some() { 1.0 } else { 0.0 }).clamp(0.0, 20.0), color))
}

/// Split a value on whitespace, keeping parenthesized groups such as `rgb(1, 2, 3)` together.
pub fn split_value(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c)
            }
            ')' => {
                depth -= 1;
                cur.push(c)
            }
            c if c.is_whitespace() && depth <= 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The first color found in a `background` shorthand.
pub fn background_color(s: &str) -> Option<Color32> {
    split_value(s).iter().find_map(|p| if p.starts_with("url(") { None } else { parse_color(p) })
}

/// Does the value reference a remote resource (`url(http...)`)?
pub fn has_remote_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("url(") && (l.contains("http:") || l.contains("https:") || l.contains("url(//"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declarations() {
        let d = parse_declarations("color: #f00; FONT-WEIGHT:bold ; background:url(data:image/png;base64,AAA) red; margin: 0 auto !important;");
        assert_eq!(d.len(), 4);
        assert_eq!(d[0], Decl { name: "color".into(), value: "#f00".into(), important: false });
        assert_eq!(d[1].name, "font-weight");
        assert!(d[2].value.contains("base64,AAA"));
        assert!(d[3].important);
        assert_eq!(d[3].value, "0 auto");
        assert!(parse_declarations("garbage").is_empty());
    }

    #[test]
    fn stylesheet() {
        let css = "/* hi */ body { color: red } @media (max-width: 600px) { .x { display:none } } .a, #b p { font-size: 12px; } a:hover { color: blue } @import url(x.css); td.c{padding:4px}";
        let r = parse_stylesheet(css);
        let sels: Vec<&str> = r.iter().map(|r| r.selector.as_str()).collect();
        assert_eq!(sels, vec!["body", ".a", "#b p", "td.c"]);
        assert!(r[2].specificity > r[1].specificity);
        assert!(r[1].specificity > r[0].specificity);
        assert_eq!(r[3].decls[0].value, "4px");
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#fff"), Some(Color32::WHITE));
        assert_eq!(parse_color("#336699"), Some(Color32::from_rgb(0x33, 0x66, 0x99)));
        assert_eq!(parse_color("rgb(10, 20, 30)"), Some(Color32::from_rgb(10, 20, 30)));
        assert_eq!(parse_color("RGBA(0,0,0,0)"), Some(Color32::from_rgba_unmultiplied(0, 0, 0, 0)));
        assert_eq!(parse_color("Navy"), Some(Color32::from_rgb(0, 0, 0x80)));
        assert_eq!(parse_color("transparent"), Some(Color32::TRANSPARENT));
        assert_eq!(parse_color("nonsense"), None);
        assert_eq!(background_color("url(http://x/y.png) no-repeat #eeeeee"), Some(Color32::from_rgb(0xee, 0xee, 0xee)));
    }

    #[test]
    fn lengths() {
        assert_eq!(parse_len("12px", 16.0), Some(Len::Px(12.0)));
        assert_eq!(parse_len("12pt", 16.0), Some(Len::Px(16.0)));
        assert_eq!(parse_len("1.5em", 10.0), Some(Len::Px(15.0)));
        assert_eq!(parse_len("50%", 10.0), Some(Len::Percent(50.0)));
        assert_eq!(parse_len("auto", 10.0), Some(Len::Auto));
        assert_eq!(parse_len("abc", 10.0), None);
        assert_eq!(parse_font_size("150%", 10.0, 16.0), Some(15.0));
        assert_eq!(parse_font_size("medium", 10.0, 16.0), Some(16.0));
        assert_eq!(font_tag_size("+1", 14.0, 16.0), Some(18.0));
        let s = parse_sides("1px 2px", 16.0);
        assert_eq!(s, [Some(Len::Px(1.0)), Some(Len::Px(2.0)), Some(Len::Px(1.0)), Some(Len::Px(2.0))]);
        assert_eq!(parse_border("1px solid #ccc", 16.0), Some((1.0, Some(Color32::from_rgb(0xcc, 0xcc, 0xcc)))));
        assert_eq!(parse_border("none", 16.0), Some((0.0, None)));
        assert!(has_remote_url("url('https://x/y.png')"));
    }
}
