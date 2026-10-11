//! Spell checking using system word lists (hunspell dictionaries or /usr/share/dict).
//!
//! Hunspell `.dic` files are expanded with the prefix and suffix rules of their
//! `.aff` file (single-character, long and numeric flag formats, rule conditions,
//! cross products and one level of continuation classes). Compound words are
//! accepted when the affix file defines compound flags (`COMPOUNDFLAG`,
//! `COMPOUNDBEGIN` / `COMPOUNDMIDDLE` / `COMPOUNDEND`), which covers German.
//! Words the user teaches are kept in `config_dir()/dictionary.txt`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Directories searched for hunspell / myspell dictionaries.
const DICT_DIRS: [&str; 4] = ["/usr/share/hunspell", "/usr/share/myspell", "/usr/share/myspell/dicts", "/usr/local/share/hunspell"];
/// Plain word lists used when no hunspell dictionary is installed.
const WORD_LISTS: [&str; 4] = ["/usr/share/dict/words", "/usr/share/dict/american-english", "/usr/share/dict/british-english", "/usr/share/dict/ngerman"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FlagMode {
    Char,
    Long,
    Num,
}

fn parse_flags(s: &str, mode: FlagMode) -> Vec<u32> {
    match mode {
        FlagMode::Char => s.chars().map(|c| c as u32).collect(),
        FlagMode::Long => {
            let chars: Vec<char> = s.chars().collect();
            chars.chunks(2).map(|c| ((c[0] as u32) << 16) | c.get(1).map(|x| *x as u32).unwrap_or(0)).collect()
        }
        FlagMode::Num => s.split(',').filter_map(|n| n.trim().parse().ok()).collect(),
    }
}

/// One element of an affix condition such as `[^aeiou]y`.
#[derive(Clone, Debug, PartialEq)]
enum CondChar {
    Any,
    Set { chars: Vec<char>, negated: bool },
}

impl CondChar {
    fn matches(&self, c: char) -> bool {
        match self {
            CondChar::Any => true,
            CondChar::Set { chars, negated } => chars.contains(&c) != *negated,
        }
    }
}

fn parse_condition(s: &str) -> Vec<CondChar> {
    if s == "." || s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => out.push(CondChar::Any),
            '[' => {
                let negated = chars.peek() == Some(&'^');
                if negated {
                    chars.next();
                }
                let mut set = Vec::new();
                for x in chars.by_ref() {
                    if x == ']' {
                        break;
                    }
                    set.push(x);
                }
                out.push(CondChar::Set { chars: set, negated });
            }
            c => out.push(CondChar::Set { chars: vec![c], negated: false }),
        }
    }
    out
}

#[derive(Clone, Debug)]
struct AffixEntry {
    strip: String,
    add: String,
    cont: Vec<u32>,
    cond: Vec<CondChar>,
}

impl AffixEntry {
    fn suffix_applies(&self, word: &[char]) -> bool {
        let strip: Vec<char> = self.strip.chars().collect();
        if word.len() < self.cond.len() || word.len() < strip.len() || !word.ends_with(&strip) {
            return false;
        }
        let tail = &word[word.len() - self.cond.len()..];
        self.cond.iter().zip(tail).all(|(c, ch)| c.matches(*ch)) && word.len() > strip.len()
    }
    fn prefix_applies(&self, word: &[char]) -> bool {
        let strip: Vec<char> = self.strip.chars().collect();
        if word.len() < self.cond.len() || word.len() < strip.len() || !word.starts_with(&strip) {
            return false;
        }
        self.cond.iter().zip(word).all(|(c, ch)| c.matches(*ch)) && word.len() > strip.len()
    }
    fn apply_suffix(&self, word: &[char]) -> String {
        let n = word.len() - self.strip.chars().count();
        let mut s: String = word[..n].iter().collect();
        s.push_str(&self.add);
        s
    }
    fn apply_prefix(&self, word: &[char]) -> String {
        let n = self.strip.chars().count();
        let mut s = self.add.clone();
        s.extend(&word[n..]);
        s
    }
}

#[derive(Clone, Debug, Default)]
struct AffixGroup {
    cross: bool,
    entries: Vec<AffixEntry>,
}

/// Parsed subset of a hunspell `.aff` file.
#[derive(Debug)]
struct AffixFile {
    mode: FlagMode,
    latin1: bool,
    try_chars: String,
    replacements: Vec<(String, String)>,
    prefixes: HashMap<u32, AffixGroup>,
    suffixes: HashMap<u32, AffixGroup>,
    need_affix: Option<u32>,
    forbidden: Option<u32>,
    only_in_compound: Option<u32>,
    compound_flag: Option<u32>,
    compound_begin: Option<u32>,
    compound_middle: Option<u32>,
    compound_end: Option<u32>,
    compound_min: usize,
}

impl Default for AffixFile {
    fn default() -> Self {
        AffixFile {
            mode: FlagMode::Char,
            latin1: false,
            try_chars: String::new(),
            replacements: Vec::new(),
            prefixes: HashMap::new(),
            suffixes: HashMap::new(),
            need_affix: None,
            forbidden: None,
            only_in_compound: None,
            compound_flag: None,
            compound_begin: None,
            compound_middle: None,
            compound_end: None,
            compound_min: 3,
        }
    }
}

impl AffixFile {
    fn parse(text: &str) -> AffixFile {
        let mut a = AffixFile::default();
        // Settings first (FLAG must be known before parsing rule flags).
        for line in text.lines() {
            let mut it = line.split_whitespace();
            match (it.next(), it.next()) {
                (Some("FLAG"), Some(v)) => {
                    a.mode = match v {
                        "long" => FlagMode::Long,
                        "num" => FlagMode::Num,
                        _ => FlagMode::Char,
                    }
                }
                (Some("SET"), Some(v)) => a.latin1 = v.to_ascii_uppercase().starts_with("ISO8859") || v.eq_ignore_ascii_case("cp1252"),
                (Some("TRY"), Some(v)) => a.try_chars = v.to_string(),
                _ => {}
            }
        }
        let flag = |v: &str, mode: FlagMode| parse_flags(v, mode).into_iter().next();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() || parts[0].starts_with('#') {
                continue;
            }
            match parts[0] {
                "NEEDAFFIX" | "PSEUDOROOT" => a.need_affix = parts.get(1).and_then(|v| flag(v, a.mode)),
                "FORBIDDENWORD" => a.forbidden = parts.get(1).and_then(|v| flag(v, a.mode)),
                "ONLYINCOMPOUND" => a.only_in_compound = parts.get(1).and_then(|v| flag(v, a.mode)),
                "COMPOUNDFLAG" => a.compound_flag = parts.get(1).and_then(|v| flag(v, a.mode)),
                "COMPOUNDBEGIN" => a.compound_begin = parts.get(1).and_then(|v| flag(v, a.mode)),
                "COMPOUNDMIDDLE" => a.compound_middle = parts.get(1).and_then(|v| flag(v, a.mode)),
                "COMPOUNDEND" | "COMPOUNDLAST" => a.compound_end = parts.get(1).and_then(|v| flag(v, a.mode)),
                "COMPOUNDMIN" => a.compound_min = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(3).max(1),
                "REP" if parts.len() >= 3 => a.replacements.push((parts[1].replace('_', " ").to_lowercase(), parts[2].replace('_', " ").to_lowercase())),
                kind @ ("PFX" | "SFX") if parts.len() >= 4 => {
                    let Some(f) = flag(parts[1], a.mode) else { continue };
                    let cross = parts[2] == "Y";
                    let count: usize = parts[3].parse().unwrap_or(0);
                    let mut group = AffixGroup { cross, entries: Vec::with_capacity(count) };
                    for _ in 0..count {
                        let Some(l) = lines.next() else { break };
                        let p: Vec<&str> = l.split_whitespace().collect();
                        if p.len() < 4 || p[0] != kind {
                            continue;
                        }
                        let strip = if p[2] == "0" { String::new() } else { p[2].to_string() };
                        let (add, cont) = match p[3].split_once('/') {
                            Some((add, fl)) => (add, parse_flags(fl, a.mode)),
                            None => (p[3], Vec::new()),
                        };
                        let add = if add == "0" { String::new() } else { add.to_string() };
                        let cond = parse_condition(p.get(4).copied().unwrap_or("."));
                        group.entries.push(AffixEntry { strip, add, cont, cond });
                    }
                    let map = if kind == "PFX" { &mut a.prefixes } else { &mut a.suffixes };
                    map.entry(f).or_default().entries.extend(group.entries);
                    map.get_mut(&f).unwrap().cross = cross;
                }
                _ => {}
            }
        }
        a
    }
}

fn decode(bytes: &[u8], latin1: bool) -> String {
    if !latin1 && let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    bytes.iter().map(|&b| b as char).collect()
}

/// Bounded Damerau-Levenshtein (optimal string alignment) distance; returns `None` when above `max`.
pub fn edit_distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > max {
        return None;
    }
    let mut prev2: Vec<usize> = vec![0; m + 1];
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur: Vec<usize> = vec![0; m + 1];
    for i in 1..=n {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v;
            row_min = row_min.min(v);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    let d = prev[m];
    (d <= max).then_some(d)
}

pub struct SpellChecker {
    /// Word forms bucketed by character count (case preserved).
    words: HashMap<usize, HashSet<Box<str>>>,
    /// Lowercase forms usable as compound parts.
    compound_begin: HashSet<Box<str>>,
    compound_middle: HashSet<Box<str>>,
    compound_end: HashSet<Box<str>>,
    compound_min: usize,
    /// Lowercase personal and session-ignored words.
    personal: HashSet<String>,
    ignored: HashSet<String>,
    personal_path: Option<PathBuf>,
    alphabet: Vec<char>,
    /// REP pairs (lowercase) from the affix file.
    replacements: Vec<(String, String)>,
    count: usize,
}

fn language_candidates(lang: &str) -> Vec<String> {
    let mut l = lang.trim().to_string();
    if l.is_empty() {
        l = std::env::var("LC_MESSAGES").or_else(|_| std::env::var("LANG")).unwrap_or_else(|_| "en_US".into());
    }
    let l = l.split(['.', '@']).next().unwrap_or("en_US").replace('-', "_");
    let mut v = vec![l.clone()];
    let base = l.split('_').next().unwrap_or("en").to_lowercase();
    match base.as_str() {
        "en" => v.extend(["en_US".to_string(), "en_GB".to_string()]),
        "de" => v.extend(["de_DE".to_string(), "de_AT".to_string(), "de_CH".to_string()]),
        _ => {}
    }
    v.push(base);
    v.dedup();
    v
}

fn find_hunspell(lang: &str) -> Option<(PathBuf, PathBuf)> {
    let cands = language_candidates(lang);
    for c in &cands {
        for dir in DICT_DIRS {
            let dic = Path::new(dir).join(format!("{c}.dic"));
            let aff = Path::new(dir).join(format!("{c}.aff"));
            if dic.is_file() && aff.is_file() {
                return Some((dic, aff));
            }
        }
    }
    // Any dictionary for the base language (e.g. "en_AU" for "en").
    let base = cands.last().cloned().unwrap_or_default();
    for dir in DICT_DIRS {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for dic in files {
            let name = dic.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".dic") && (name.starts_with(&format!("{base}_")) || name == format!("{base}.dic")) {
                let aff = dic.with_extension("aff");
                if aff.is_file() {
                    return Some((dic, aff));
                }
            }
        }
    }
    None
}

impl SpellChecker {
    fn empty() -> SpellChecker {
        SpellChecker {
            words: HashMap::new(),
            compound_begin: HashSet::new(),
            compound_middle: HashSet::new(),
            compound_end: HashSet::new(),
            compound_min: 3,
            personal: HashSet::new(),
            ignored: HashSet::new(),
            personal_path: None,
            alphabet: "abcdefghijklmnopqrstuvwxyz".chars().collect(),
            replacements: Vec::new(),
            count: 0,
        }
    }

    /// Load the best available dictionary for the given language (e.g. "en_US").
    pub fn load(lang: &str) -> SpellChecker {
        let mut sc = match find_hunspell(lang) {
            Some((dic, aff)) => match (std::fs::read(&dic), std::fs::read(&aff)) {
                (Ok(d), Ok(a)) => Self::from_hunspell_bytes(&d, &a),
                _ => Self::empty(),
            },
            None => Self::empty(),
        };
        if !sc.is_available()
            && let Some(list) = WORD_LISTS.iter().map(Path::new).find(|p| p.is_file())
            && let Ok(bytes) = std::fs::read(list)
        {
            sc = Self::from_word_list(&decode(&bytes, false));
        }
        let path = super::config::config_dir().join("dictionary.txt");
        if let Ok(text) = std::fs::read_to_string(&path) {
            sc.personal.extend(text.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()));
        }
        sc.personal_path = Some(path);
        sc
    }

    /// Build a checker from hunspell `.dic` and `.aff` contents (no personal dictionary).
    pub fn from_dictionary(dic: &str, aff: &str) -> SpellChecker {
        Self::from_hunspell_bytes(dic.as_bytes(), aff.as_bytes())
    }

    fn from_hunspell_bytes(dic: &[u8], aff: &[u8]) -> SpellChecker {
        let aff_text = decode(aff, false);
        let affix = AffixFile::parse(&aff_text);
        let aff_text = if affix.latin1 { decode(aff, true) } else { aff_text };
        let affix = if affix.latin1 { AffixFile::parse(&aff_text) } else { affix };
        let dic_text = decode(dic, affix.latin1);
        let mut sc = Self::empty();
        sc.compound_min = affix.compound_min;
        sc.replacements = affix.replacements.clone();
        let try_letters: Vec<char> = affix.try_chars.chars().filter(|c| c.is_alphabetic() && c.is_lowercase()).collect();
        if !try_letters.is_empty() {
            let mut a = try_letters;
            a.extend("abcdefghijklmnopqrstuvwxyz".chars());
            let mut seen = HashSet::new();
            a.retain(|c| seen.insert(*c));
            sc.alphabet = a;
        }
        for (i, line) in dic_text.lines().enumerate() {
            if i == 0 && line.trim().chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let entry = line.split('\t').next().unwrap_or("").trim();
            if entry.is_empty() || entry.starts_with('#') {
                continue;
            }
            // "word/FLAGS morph..." — a slash escaped as "\/" belongs to the word.
            let entry = entry.split(' ').next().unwrap_or(entry);
            let (word, flags) = split_entry(entry);
            if word.is_empty() {
                continue;
            }
            let flags = parse_flags(&flags, affix.mode);
            sc.expand(&affix, &word, &flags);
        }
        sc
    }

    /// Build a checker from a plain word list (one word per line).
    pub fn from_word_list(text: &str) -> SpellChecker {
        let mut sc = Self::empty();
        for w in text.lines().map(str::trim).filter(|w| !w.is_empty() && !w.starts_with('#')) {
            sc.insert(w);
        }
        sc
    }

    fn insert(&mut self, w: &str) {
        let n = w.chars().count();
        if self.words.entry(n).or_default().insert(w.into()) {
            self.count += 1;
        }
    }

    fn add_form(&mut self, affix: &AffixFile, form: &str, flags: &[u32], cont: &[u32]) {
        let has = |f: Option<u32>| f.is_some_and(|f| flags.contains(&f) || cont.contains(&f));
        if has(affix.forbidden) {
            return;
        }
        let lower: Box<str> = form.to_lowercase().into();
        if has(affix.compound_flag) {
            self.compound_begin.insert(lower.clone());
            self.compound_middle.insert(lower.clone());
            self.compound_end.insert(lower.clone());
        }
        if has(affix.compound_begin) {
            self.compound_begin.insert(lower.clone());
        }
        if has(affix.compound_middle) {
            self.compound_middle.insert(lower.clone());
        }
        if has(affix.compound_end) {
            self.compound_end.insert(lower);
        }
        if !has(affix.only_in_compound) && !cont.iter().any(|c| Some(*c) == affix.need_affix) {
            self.insert(form);
        }
    }

    fn expand(&mut self, affix: &AffixFile, word: &str, flags: &[u32]) {
        let chars: Vec<char> = word.chars().collect();
        let needs_affix = affix.need_affix.is_some_and(|f| flags.contains(&f));
        if !needs_affix {
            self.add_form(affix, word, flags, &[]);
        } else {
            // Only record compound membership for the bare stem.
            let only: Vec<u32> = flags.iter().copied().filter(|f| Some(*f) != affix.need_affix).collect();
            let lower: Box<str> = word.to_lowercase().into();
            if affix.compound_begin.is_some_and(|f| only.contains(&f)) {
                self.compound_begin.insert(lower.clone());
            }
            if affix.compound_middle.is_some_and(|f| only.contains(&f)) {
                self.compound_middle.insert(lower.clone());
            }
            if affix.compound_end.is_some_and(|f| only.contains(&f)) {
                self.compound_end.insert(lower);
            }
        }
        let prefix_entries: Vec<(&AffixGroup, &AffixEntry)> = flags
            .iter()
            .filter_map(|f| affix.prefixes.get(f))
            .flat_map(|g| g.entries.iter().map(move |e| (g, e)))
            .filter(|(_, e)| e.prefix_applies(&chars))
            .collect();
        for f in flags {
            let Some(group) = affix.suffixes.get(f) else { continue };
            for e in &group.entries {
                if !e.suffix_applies(&chars) {
                    continue;
                }
                let form = e.apply_suffix(&chars);
                let form_chars: Vec<char> = form.chars().collect();
                self.add_form(affix, &form, flags, &e.cont);
                // One level of continuation classes (twofold suffixes).
                for cf in &e.cont {
                    if let Some(g2) = affix.suffixes.get(cf) {
                        for e2 in &g2.entries {
                            if e2.suffix_applies(&form_chars) {
                                let f2 = e2.apply_suffix(&form_chars);
                                self.add_form(affix, &f2, flags, &e2.cont);
                            }
                        }
                    }
                }
                if group.cross {
                    for (pg, pe) in &prefix_entries {
                        if pg.cross && pe.prefix_applies(&form_chars) {
                            let pf = pe.apply_prefix(&form_chars);
                            self.add_form(affix, &pf, flags, &pe.cont);
                        }
                    }
                }
            }
        }
        for (_, pe) in &prefix_entries {
            let form = pe.apply_prefix(&chars);
            self.add_form(affix, &form, flags, &pe.cont);
        }
    }

    pub fn is_available(&self) -> bool {
        self.count > 0
    }

    /// Number of distinct word forms known.
    pub fn word_count(&self) -> usize {
        self.count
    }

    fn contains(&self, w: &str) -> bool {
        self.words.get(&w.chars().count()).is_some_and(|s| s.contains(w))
    }

    fn is_compound(&self, lower: &str, depth: usize) -> bool {
        if depth > 3 || self.compound_end.is_empty() {
            return false;
        }
        let idx: Vec<usize> = lower.char_indices().map(|(i, _)| i).chain(std::iter::once(lower.len())).collect();
        let n = idx.len() - 1;
        if n < self.compound_min * 2 {
            return false;
        }
        for &split in &idx[self.compound_min..=n - self.compound_min] {
            let (left, right) = lower.split_at(split);
            let left_ok = if depth == 0 { self.compound_begin.contains(left) } else { self.compound_middle.contains(left) };
            if !left_ok {
                continue;
            }
            if self.compound_end.contains(right) || self.is_compound(right, depth + 1) {
                return true;
            }
        }
        false
    }

    fn check_form(&self, w: &str) -> bool {
        if self.contains(w) {
            return true;
        }
        let mut chars = w.chars();
        let first = chars.next();
        let rest: String = chars.collect();
        let lower = w.to_lowercase();
        // "Hello" at the start of a sentence matches "hello"; "HELLO" matches "Hello"/"hello".
        if first.is_some_and(|c| c.is_uppercase()) && (rest.chars().all(|c| !c.is_uppercase()) || rest.chars().all(|c| !c.is_lowercase())) {
            if self.contains(&lower) {
                return true;
            }
            let cap: String = first.into_iter().chain(rest.to_lowercase().chars()).collect();
            if cap != w && self.contains(&cap) {
                return true;
            }
        }
        self.is_compound(&lower, 0)
    }

    pub fn check(&self, word: &str) -> bool {
        let w = word.trim().replace('’', "'");
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        if w.is_empty() || !self.is_available() {
            return true;
        }
        let lower = w.to_lowercase();
        if self.personal.contains(&lower) || self.ignored.contains(&lower) {
            return true;
        }
        if self.check_form(w) {
            return true;
        }
        // Possessives ("Anna's") and hyphenated words ("well-known").
        if let Some(stem) = w.strip_suffix("'s").or_else(|| w.strip_suffix("'S"))
            && (self.check_form(stem) || self.personal.contains(&stem.to_lowercase()))
        {
            return true;
        }
        if w.contains('-') {
            return w.split('-').filter(|p| !p.is_empty()).all(|p| self.check(p));
        }
        false
    }

    /// Byte ranges of misspelled words in text.
    pub fn misspelled(&self, text: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if !self.is_available() {
            return out;
        }
        let mut line_start = 0;
        for line in text.split_inclusive('\n') {
            let offset = line_start;
            line_start += line.len();
            if line.trim_start().starts_with('>') {
                continue;
            }
            // Whitespace-separated chunks first, so URLs and addresses are skipped whole.
            let mut pos = 0;
            for chunk in line.split(char::is_whitespace) {
                let chunk_start = pos;
                pos += chunk.len() + 1;
                if chunk.is_empty() {
                    continue;
                }
                let lower = chunk.to_ascii_lowercase();
                if lower.contains("://") || lower.starts_with("www.") || chunk.contains('@') || lower.starts_with("mailto:") {
                    continue;
                }
                for (ws, we) in words_in(chunk) {
                    let word = &chunk[ws..we];
                    if should_skip(word, chunk, ws, we) {
                        continue;
                    }
                    if !self.check(word) {
                        out.push((offset + chunk_start + ws, offset + chunk_start + we));
                    }
                }
            }
        }
        out
    }

    pub fn suggest(&self, word: &str, max: usize) -> Vec<String> {
        if !self.is_available() || max == 0 {
            return Vec::new();
        }
        let w = word.trim().replace('’', "'");
        let lower = w.to_lowercase();
        let lchars: Vec<char> = lower.chars().collect();
        if lchars.is_empty() || lchars.len() > 40 {
            return Vec::new();
        }
        let mut found: HashMap<String, usize> = HashMap::new();
        // Wrong capitalization only ("monday" -> "Monday").
        if let Some(form) = self.lookup_form(&lower).filter(|f| *f != w) {
            found.insert(form, 0);
        }
        // Distance 1: generate all edits and look them up.
        for cand in edits1(&lchars, &self.alphabet) {
            if cand != lower
                && let Some(form) = self.lookup_form(&cand)
            {
                found.entry(form).or_insert(1);
            }
        }
        // Typical misspellings from the REP table ("ss" -> "ß", "f" -> "ph").
        for (from, to) in &self.replacements {
            if from.is_empty() {
                continue;
            }
            for (i, _) in lower.match_indices(from.as_str()) {
                let cand = format!("{}{}{}", &lower[..i], to, &lower[i + from.len()..]);
                if let Some(form) = self.lookup_form(&cand).filter(|f| *f != w) {
                    found.insert(form, 0);
                }
            }
        }
        // Distance 2 (and capitalized forms): scan words of similar length.
        if found.len() < max {
            let n = lchars.len();
            for len in n.saturating_sub(2)..=n + 2 {
                let Some(bucket) = self.words.get(&len) else { continue };
                for cand in bucket {
                    let cl: Vec<char> = cand.to_lowercase().chars().collect();
                    // Cheap pre-filter: share the first or second letter.
                    if cl.first() != lchars.first() && cl.get(1) != lchars.get(1) && cl.first() != lchars.get(1) {
                        continue;
                    }
                    if let Some(d) = edit_distance(&lchars, &cl, 2)
                        && (d > 0 || **cand != *w)
                    {
                        let e = found.entry(cand.to_string()).or_insert(d);
                        *e = (*e).min(d);
                    }
                }
            }
        }
        let first = lchars[0];
        let mut list: Vec<(String, usize)> = found.into_iter().filter(|(s, _)| *s != w).collect();
        list.sort_by(|a, b| {
            let fa = a.0.to_lowercase().starts_with(first);
            let fb = b.0.to_lowercase().starts_with(first);
            a.1.cmp(&b.1)
                .then(fb.cmp(&fa))
                .then(a.0.chars().count().abs_diff(lchars.len()).cmp(&b.0.chars().count().abs_diff(lchars.len())))
                .then(a.0.cmp(&b.0))
        });
        let capitalized = w.chars().next().is_some_and(|c| c.is_uppercase());
        let all_caps = w.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()) && w.chars().count() > 1;
        let mut out: Vec<String> = Vec::new();
        for (s, _) in list {
            let s = if all_caps {
                s.to_uppercase()
            } else if capitalized {
                let mut c = s.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or(s)
            } else {
                s
            };
            if !out.contains(&s) {
                out.push(s);
            }
            if out.len() >= max {
                break;
            }
        }
        out
    }

    /// Find the stored form of a lowercase candidate ("monday" -> "Monday").
    fn lookup_form(&self, lower: &str) -> Option<String> {
        if self.contains(lower) {
            return Some(lower.to_string());
        }
        let mut c = lower.chars();
        let cap: String = c.next().map(|f| f.to_uppercase().chain(c).collect())?;
        self.contains(&cap).then_some(cap)
    }

    /// Accept a word for the rest of the session without saving it.
    pub fn ignore(&mut self, word: &str) {
        let w = word.trim().to_lowercase();
        if !w.is_empty() {
            self.ignored.insert(w);
        }
    }

    /// Add a word to the user's personal dictionary.
    pub fn learn(&mut self, word: &str) {
        let w = word.trim().to_lowercase();
        if w.is_empty() || !self.personal.insert(w.clone()) {
            return;
        }
        if let Some(path) = &self.personal_path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            let mut words: Vec<&String> = self.personal.iter().collect();
            words.sort();
            let text: String = words.into_iter().map(|w| format!("{w}\n")).collect();
            if let Err(e) = std::fs::write(path, text) {
                log::warn!("could not save personal dictionary {}: {e}", path.display());
            }
        }
    }

    /// Remove a word from the personal dictionary.
    pub fn unlearn(&mut self, word: &str) {
        let w = word.trim().to_lowercase();
        if self.personal.remove(&w)
            && let Some(path) = &self.personal_path
        {
            let mut words: Vec<&String> = self.personal.iter().collect();
            words.sort();
            std::fs::write(path, words.into_iter().map(|w| format!("{w}\n")).collect::<String>()).ok();
        }
    }

    /// Use a different personal dictionary file (loading the words it contains).
    pub fn with_personal_path(mut self, path: PathBuf) -> Self {
        if let Ok(text) = std::fs::read_to_string(&path) {
            self.personal.extend(text.lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()));
        }
        self.personal_path = Some(path);
        self
    }
}

fn split_entry(entry: &str) -> (String, String) {
    let mut word = String::new();
    let mut chars = entry.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'/') => {
                word.push('/');
                chars.next();
            }
            '/' => return (word, chars.collect()),
            c => word.push(c),
        }
    }
    (word, String::new())
}

/// Byte ranges of word-like runs (letters with internal apostrophes / hyphens) in a chunk.
fn words_in(chunk: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let chars: Vec<(usize, char)> = chunk.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].1.is_alphabetic() {
            i += 1;
            continue;
        }
        let start = chars[i].0;
        let mut j = i;
        while j < chars.len() {
            let c = chars[j].1;
            let joiner = matches!(c, '\'' | '’' | '-') && j + 1 < chars.len() && chars[j + 1].1.is_alphabetic();
            if c.is_alphabetic() || c.is_ascii_digit() || joiner {
                j += 1;
            } else {
                break;
            }
        }
        let end = if j < chars.len() { chars[j].0 } else { chunk.len() };
        out.push((start, end));
        i = j;
    }
    out
}

fn should_skip(word: &str, chunk: &str, start: usize, end: usize) -> bool {
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 2 || word.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    // Glued to digits or path / identifier characters ("3pm", "foo_bar", "a/b").
    let prev = chunk[..start].chars().last();
    let next = chunk[end..].chars().next();
    if prev.is_some_and(|c| c.is_ascii_digit() || matches!(c, '_' | '/' | '\\' | '#' | '$' | '%' | '&' | '=' | '<'))
        || next.is_some_and(|c| c.is_ascii_digit() || matches!(c, '_' | '/' | '\\' | '=' | '(' | '>'))
    {
        return true;
    }
    // ALLCAPS and camelCase / mixed-case identifiers.
    if word.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase()) {
        return true;
    }
    word.chars().skip(1).any(|c| c.is_uppercase()) && !word.contains('-')
}

/// All strings at Damerau-Levenshtein distance 1.
fn edits1(word: &[char], alphabet: &[char]) -> HashSet<String> {
    let mut out = HashSet::new();
    let n = word.len();
    for i in 0..n {
        // deletion
        out.insert(word[..i].iter().chain(&word[i + 1..]).collect());
        // transposition
        if i + 1 < n {
            let mut w = word.to_vec();
            w.swap(i, i + 1);
            out.insert(w.into_iter().collect());
        }
        // replacement
        for &c in alphabet {
            if c != word[i] {
                let mut w = word.to_vec();
                w[i] = c;
                out.insert(w.into_iter().collect());
            }
        }
    }
    // insertion
    for i in 0..=n {
        for &c in alphabet {
            let mut w = word[..i].to_vec();
            w.push(c);
            w.extend_from_slice(&word[i..]);
            out.insert(w.into_iter().collect());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFF: &str = "SET UTF-8\nTRY esianrtolcdugmphbyfvkw\n\nPFX U Y 1\nPFX U   0     un         .\n\n\
SFX S Y 4\nSFX S   y     ies        [^aeiou]y\nSFX S   0     s          [aeiou]y\nSFX S   0     es         [sxzh]\nSFX S   0     s          [^sxzhy]\n\n\
SFX D Y 2\nSFX D   0     d          e\nSFX D   0     ed         [^ey]\n\nSFX G Y 1\nSFX G   e     ing        e\n";
    const DIC: &str = "8\nhello/S\nworld/S\nlike/DGU\nhappy\nfly/S\nbox/S\nMonday/S\nreceive/DG\ndefinitely\n";

    fn checker() -> SpellChecker {
        SpellChecker::from_dictionary(DIC, AFF)
    }

    #[test]
    fn affix_expansion() {
        let sc = checker();
        assert!(sc.is_available());
        for w in ["hello", "hellos", "worlds", "liked", "liking", "unlike", "unliked", "flies", "boxes", "Mondays", "received", "receiving"] {
            assert!(sc.check(w), "{w} should be correct");
        }
        for w in ["flys", "boxs", "likeing", "recieve", "monday"] {
            assert!(!sc.check(w), "{w} should be wrong");
        }
        // Capitalization at sentence start and ALLCAPS handling
        assert!(sc.check("Hello"));
        assert!(sc.check("HELLO"));
        assert!(sc.check("hello's"));
    }

    #[test]
    fn misspelled_ranges_skip_noise() {
        let sc = checker();
        let text = "Helo world, recieve https://exampel.com/wrold and mail me at wrold@exmaple.com\n> quoted wrold here\nNASA 3pm abc123 iPhone world-happy definately";
        let bad: Vec<&str> = sc.misspelled(text).into_iter().map(|(s, e)| &text[s..e]).collect();
        assert_eq!(bad, vec!["Helo", "recieve", "and", "mail", "me", "at", "definately"]);
    }

    #[test]
    fn suggestions() {
        let sc = checker();
        assert_eq!(sc.suggest("recieve", 3)[0], "receive");
        assert_eq!(sc.suggest("helo", 3)[0], "hello");
        assert_eq!(sc.suggest("Wrold", 3)[0], "World");
        assert_eq!(sc.suggest("definately", 3)[0], "definitely");
        assert_eq!(sc.suggest("monday", 3)[0], "Monday");
        assert!(sc.suggest("zzzzzzzzzz", 3).is_empty());
        assert_eq!(edit_distance(&['a', 'b'], &['b', 'a'], 2), Some(1));
        assert_eq!(edit_distance(&"kitten".chars().collect::<Vec<_>>(), &"sitting".chars().collect::<Vec<_>>(), 2), None);
    }

    #[test]
    fn learn_and_personal_dictionary() {
        let dir = std::env::temp_dir().join(format!("purpamail-spell-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join("dictionary.txt");
        let mut sc = checker().with_personal_path(path.clone());
        assert!(!sc.check("Purpamail"));
        sc.learn("Purpamail");
        assert!(sc.check("purpamail"));
        assert!(std::fs::read_to_string(&path).unwrap().contains("purpamail"));
        let sc2 = checker().with_personal_path(path.clone());
        assert!(sc2.check("Purpamail"));
        let mut sc3 = sc2;
        sc3.unlearn("purpamail");
        assert!(!sc3.check("Purpamail"));
        sc3.ignore("Grml");
        assert!(sc3.check("grml"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compounds_and_flag_formats() {
        let aff = "SET UTF-8\nFLAG long\nCOMPOUNDBEGIN xx\nCOMPOUNDEND zz\nCOMPOUNDMIN 3\nSFX Ss Y 1\nSFX Ss 0 s/xx .\n";
        let dic = "3\nHaus/xxzz\nTür/zz\nArbeit/Ss\n";
        let sc = SpellChecker::from_dictionary(dic, aff);
        assert!(sc.check("Haus"));
        assert!(sc.check("Haustür"));
        assert!(sc.check("Arbeitshaus"));
        assert!(!sc.check("Türhaus"));
        let num = SpellChecker::from_dictionary("1\nfoo/1,2\n", "FLAG num\nSFX 2 Y 1\nSFX 2 0 bar .\n");
        assert!(num.check("foobar"));
        let list = SpellChecker::from_word_list("cherry\nbanana\n");
        assert!(list.check("Cherry") && !list.check("chery"));
        assert_eq!(list.suggest("chery", 1), vec!["cherry".to_string()]);
    }

    #[test]
    fn unavailable_checker_accepts_everything() {
        let sc = SpellChecker::empty();
        assert!(!sc.is_available());
        assert!(sc.check("asdfgh"));
        assert!(sc.misspelled("asdfgh qwerty").is_empty());
        assert!(sc.suggest("asdf", 3).is_empty());
        // Garbage affix / dic files do not panic
        let g = SpellChecker::from_dictionary("x\n/\n\\/a/\n", "SFX\nSFX A Y 9\nSFX A\nPFX B N x\nFLAG long\n");
        let _ = g.check("a");
    }

    #[test]
    fn replacement_table_suggestions() {
        let sc = SpellChecker::from_dictionary("3\nGrüße\nGüsse\nTelefon\n", "SET UTF-8\nREP 2\nREP ss ß\nREP ph f\n");
        assert_eq!(sc.suggest("Grüsse", 2)[0], "Grüße");
        assert_eq!(sc.suggest("Telephon", 2)[0], "Telefon");
    }

    /// Exercise installed system dictionaries when present (not required).
    #[test]
    fn system_dictionaries_if_installed() {
        if let Some((dic, aff)) = find_hunspell("en_US") {
            let sc = SpellChecker::from_hunspell_bytes(&std::fs::read(dic).unwrap(), &std::fs::read(aff).unwrap());
            assert!(sc.check("receiving") && sc.check("unhappiness") && sc.check("Mondays"));
            assert!(!sc.check("recieve"));
            assert_eq!(sc.suggest("recieve", 3)[0], "receive");
        }
        if let Some((dic, aff)) = find_hunspell("de_DE") {
            let sc = SpellChecker::from_hunspell_bytes(&std::fs::read(dic).unwrap(), &std::fs::read(aff).unwrap());
            assert!(sc.check("Haustür") && sc.check("Grüße") && sc.check("gegangen"));
            assert!(!sc.check("Hausstürr"));
        }
    }
}
