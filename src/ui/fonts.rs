//! Font discovery and installation.
//!
//! Purpamail uses the system's fonts (via fontconfig when available) and falls
//! back to the fonts bundled with egui. Extra families are registered for bold,
//! italic, monospace and the user-selectable list / message fonts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

use crate::core::config::FontsColorsPrefs;

pub const BOLD: &str = "pm-bold";
pub const ITALIC: &str = "pm-italic";
pub const BOLD_ITALIC: &str = "pm-bold-italic";
pub const LIST: &str = "pm-list";
pub const LIST_BOLD: &str = "pm-list-bold";
pub const MESSAGE: &str = "pm-message";
pub const MESSAGE_BOLD: &str = "pm-message-bold";
pub const MESSAGE_ITALIC: &str = "pm-message-italic";
pub const MESSAGE_BOLD_ITALIC: &str = "pm-message-bold-italic";
pub const FIXED: &str = "pm-fixed";
pub const SERIF: &str = "pm-serif";

pub fn bold() -> FontFamily {
    FontFamily::Name(BOLD.into())
}
pub fn italic() -> FontFamily {
    FontFamily::Name(ITALIC.into())
}
pub fn bold_italic() -> FontFamily {
    FontFamily::Name(BOLD_ITALIC.into())
}
pub fn list() -> FontFamily {
    FontFamily::Name(LIST.into())
}
pub fn list_bold() -> FontFamily {
    FontFamily::Name(LIST_BOLD.into())
}
pub fn message() -> FontFamily {
    FontFamily::Name(MESSAGE.into())
}
/// Message font variant for the given style.
pub fn message_styled(bold: bool, italic: bool) -> FontFamily {
    FontFamily::Name(
        match (bold, italic) {
            (false, false) => MESSAGE,
            (true, false) => MESSAGE_BOLD,
            (false, true) => MESSAGE_ITALIC,
            (true, true) => MESSAGE_BOLD_ITALIC,
        }
        .into(),
    )
}
pub fn fixed() -> FontFamily {
    FontFamily::Name(FIXED.into())
}
pub fn serif() -> FontFamily {
    FontFamily::Name(SERIF.into())
}

/// Resolve a font pattern (e.g. "Inter:weight=bold") to a file using fontconfig.
fn fc_match(pattern: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("fc-match").arg("-f").arg("%{file}").arg(pattern).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if p.exists() && (ext == "ttf" || ext == "otf" || ext == "ttc") { Some(p) } else { None }
}

const FONT_DIRS: [&str; 4] = ["/usr/share/fonts", "/usr/local/share/fonts", "~/.local/share/fonts", "~/.fonts"];

fn expand(dir: &str) -> PathBuf {
    if let Some(rest) = dir.strip_prefix("~/") {
        if let Some(h) = directories::BaseDirs::new() {
            return h.home_dir().join(rest);
        }
    }
    PathBuf::from(dir)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out, depth + 1);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if ext == "ttf" || ext == "otf" {
                out.push(p);
            }
        }
    }
}

fn all_font_files() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for d in FONT_DIRS {
        walk(&expand(d), &mut v, 0);
    }
    v
}

/// Find a font file without fontconfig, by file name.
fn find_by_name(files: &[PathBuf], names: &[&str]) -> Option<PathBuf> {
    for n in names {
        if let Some(p) = files.iter().find(|p| p.file_name().and_then(|f| f.to_str()).map(|f| f.eq_ignore_ascii_case(n)).unwrap_or(false)) {
            return Some(p.clone());
        }
    }
    None
}

#[derive(Default, Clone)]
struct Face {
    regular: Option<PathBuf>,
    bold: Option<PathBuf>,
    italic: Option<PathBuf>,
    bold_italic: Option<PathBuf>,
}

fn resolve_family(family: &str) -> Face {
    let check = |pat: &str| -> Option<PathBuf> {
        let p = fc_match(pat)?;
        // fc-match always returns *something*; make sure it is the requested family.
        let fname = p.file_name()?.to_str()?.to_lowercase().replace([' ', '-', '_'], "");
        let want = family.to_lowercase().replace([' ', '-', '_'], "");
        if fname.contains(&want) { Some(p) } else { None }
    };
    Face {
        regular: check(&format!("{family}:weight=regular:slant=roman")),
        bold: check(&format!("{family}:weight=semibold:slant=roman")).or_else(|| check(&format!("{family}:weight=bold:slant=roman"))),
        italic: check(&format!("{family}:weight=regular:slant=italic")),
        bold_italic: check(&format!("{family}:weight=semibold:slant=italic")).or_else(|| check(&format!("{family}:weight=bold:slant=italic"))),
    }
}

fn default_ui_face() -> Face {
    for fam in ["Inter", "Cantarell", "Noto Sans", "Ubuntu", "Open Sans", "DejaVu Sans", "Liberation Sans"] {
        let f = resolve_family(fam);
        if f.regular.is_some() {
            return f;
        }
    }
    // No fontconfig: look for well known files.
    let files = all_font_files();
    Face {
        regular: find_by_name(&files, &["Inter-Regular.otf", "Inter-Regular.ttf", "NotoSans-Regular.ttf", "DejaVuSans.ttf", "LiberationSans-Regular.ttf"]),
        bold: find_by_name(&files, &["Inter-SemiBold.otf", "Inter-SemiBold.ttf", "NotoSans-SemiBold.ttf", "NotoSans-Bold.ttf", "DejaVuSans-Bold.ttf", "LiberationSans-Bold.ttf"]),
        italic: find_by_name(&files, &["Inter-Italic.otf", "Inter-Italic.ttf", "NotoSans-Italic.ttf", "DejaVuSans-Oblique.ttf", "LiberationSans-Italic.ttf"]),
        bold_italic: find_by_name(&files, &["Inter-SemiBoldItalic.otf", "NotoSans-BoldItalic.ttf", "DejaVuSans-BoldOblique.ttf", "LiberationSans-BoldItalic.ttf"]),
    }
}

/// Rewrite a font's character map without private-use-area code points.
///
/// Some text fonts (Inter, for instance) ship glyphs in the private use area,
/// which would shadow the icon font that lives there. The font file is left as is,
/// except that a new `cmap` table (one format 12 subtable) is appended and the
/// table directory is pointed at it.
pub fn strip_private_use(bytes: Vec<u8>) -> Vec<u8> {
    use skrifa::MetadataProvider;
    let is_pua = |c: u32| (0xE000..=0xF8FF).contains(&c) || c >= 0xF0000;
    let mappings: Vec<(u32, u32)> = match skrifa::FontRef::new(&bytes) {
        Ok(f) => {
            let cm = f.charmap();
            if !cm.mappings().any(|(c, _)| is_pua(c)) {
                return bytes;
            }
            cm.mappings().filter(|(c, _)| !is_pua(*c)).map(|(c, g)| (c, g.to_u32())).collect()
        }
        Err(_) => return bytes,
    };
    // Group consecutive code points with consecutive glyph ids.
    let mut groups: Vec<(u32, u32, u32)> = Vec::new();
    for (c, g) in mappings {
        match groups.last_mut() {
            Some((start, end, sg)) if c == *end + 1 && g == *sg + (c - *start) => *end = c,
            _ => groups.push((c, c, g)),
        }
    }
    let be16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_be_bytes());
    let be32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_be_bytes());
    let mut cmap = Vec::new();
    be16(&mut cmap, 0); // version
    be16(&mut cmap, 1); // number of subtables
    be16(&mut cmap, 3); // platform: Windows
    be16(&mut cmap, 10); // encoding: Unicode full repertoire
    be32(&mut cmap, 12); // offset of the subtable
    be16(&mut cmap, 12); // format
    be16(&mut cmap, 0);
    be32(&mut cmap, 16 + 12 * groups.len() as u32);
    be32(&mut cmap, 0); // language
    be32(&mut cmap, groups.len() as u32);
    for (s, e, g) in groups {
        be32(&mut cmap, s);
        be32(&mut cmap, e);
        be32(&mut cmap, g);
    }
    let mut out = bytes;
    if out.len() < 12 {
        return out;
    }
    let num_tables = u16::from_be_bytes([out[4], out[5]]) as usize;
    while out.len() % 4 != 0 {
        out.push(0);
    }
    let new_offset = out.len() as u32;
    for i in 0..num_tables {
        let rec = 12 + i * 16;
        if rec + 16 > out.len() {
            break;
        }
        if &out[rec..rec + 4] == b"cmap" {
            out[rec + 8..rec + 12].copy_from_slice(&new_offset.to_be_bytes());
            out[rec + 12..rec + 16].copy_from_slice(&(cmap.len() as u32).to_be_bytes());
            out.extend_from_slice(&cmap);
            while out.len() % 4 != 0 {
                out.push(0);
            }
            break;
        }
    }
    out
}

fn load(defs: &mut FontDefinitions, key: &str, path: &Option<PathBuf>) -> Option<String> {
    let p = path.as_ref()?;
    let bytes = strip_private_use(std::fs::read(p).ok()?);
    let name = format!("{key}:{}", p.display());
    if !defs.font_data.contains_key(&name) {
        defs.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
    }
    Some(name)
}

/// Fallback fonts appended to every family (icons, emoji, CJK).
fn fallbacks(defs: &FontDefinitions) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for k in ["phosphor", "NotoEmoji-Regular", "emoji-icon-font", "Ubuntu-Light"] {
        if defs.font_data.contains_key(k) {
            v.push(k.to_string());
        }
    }
    v
}

fn set_family(defs: &mut FontDefinitions, family: &str, primary: Vec<String>, extra: &[String]) {
    let mut list = primary;
    for e in extra {
        if !list.contains(e) {
            list.push(e.clone());
        }
    }
    defs.families.insert(FontFamily::Name(family.into()), list);
}

/// Build and install font definitions according to preferences.
pub fn install(ctx: &egui::Context, prefs: &FontsColorsPrefs) {
    let mut defs = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut defs, egui_phosphor::Variant::Regular);

    let ui = default_ui_face();
    let reg = load(&mut defs, "ui", &ui.regular);
    let bold = load(&mut defs, "ui-bold", &ui.bold).or(reg.clone());
    let ital = load(&mut defs, "ui-italic", &ui.italic).or(reg.clone());
    let bi = load(&mut defs, "ui-bold-italic", &ui.bold_italic).or(bold.clone());

    // CJK coverage
    let cjk = fc_match(":lang=zh").or_else(|| fc_match(":lang=ja"));
    let cjk = load(&mut defs, "cjk", &cjk);

    let mut extra = fallbacks(&defs);
    if let Some(c) = &cjk {
        extra.push(c.clone());
    }

    if let Some(r) = &reg {
        let prop = defs.families.entry(FontFamily::Proportional).or_default();
        prop.insert(0, r.clone());
    }
    if let Some(c) = &cjk {
        defs.families.entry(FontFamily::Proportional).or_default().push(c.clone());
    }
    let base: Vec<String> = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    set_family(&mut defs, BOLD, bold.iter().cloned().collect(), &base);
    set_family(&mut defs, ITALIC, ital.iter().cloned().collect(), &base);
    set_family(&mut defs, BOLD_ITALIC, bi.iter().cloned().collect(), &base);

    // Monospace
    let mono_face = if prefs.fixed_font.is_empty() { resolve_family("DejaVu Sans Mono") } else { resolve_family(&prefs.fixed_font) };
    let mono = load(&mut defs, "fixed", &mono_face.regular);
    let mut mono_list: Vec<String> = mono.iter().cloned().collect();
    mono_list.extend(defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default());
    set_family(&mut defs, FIXED, mono_list, &extra);

    // Serif (used by the HTML renderer for serif font-family)
    let serif_face = resolve_family("DejaVu Serif");
    let serif_face = if serif_face.regular.is_some() { serif_face } else { resolve_family("Liberation Serif") };
    let serif = load(&mut defs, "serif", &serif_face.regular);
    set_family(&mut defs, SERIF, serif.iter().cloned().collect(), &base);

    // User selectable list / message fonts
    let pick = |defs: &mut FontDefinitions, key: &str, fam: &str| -> (Option<String>, Option<String>, Option<String>, Option<String>) {
        if fam.is_empty() {
            return (reg.clone(), bold.clone(), ital.clone(), bi.clone());
        }
        let f = resolve_family(fam);
        let r = load(defs, &format!("{key}"), &f.regular).or(reg.clone());
        let b = load(defs, &format!("{key}-b"), &f.bold).or(r.clone());
        let i = load(defs, &format!("{key}-i"), &f.italic).or(r.clone());
        let bi2 = load(defs, &format!("{key}-bi"), &f.bold_italic).or(b.clone());
        (r, b, i, bi2)
    };
    let (lr, lb, _, _) = pick(&mut defs, "list", &prefs.list_font);
    set_family(&mut defs, LIST, lr.into_iter().collect(), &base);
    set_family(&mut defs, LIST_BOLD, lb.into_iter().collect(), &base);
    let (mr, mb, mi, mbi) = pick(&mut defs, "msg", &prefs.message_font);
    set_family(&mut defs, MESSAGE, mr.into_iter().collect(), &base);
    set_family(&mut defs, MESSAGE_BOLD, mb.into_iter().collect(), &base);
    set_family(&mut defs, MESSAGE_ITALIC, mi.into_iter().collect(), &base);
    set_family(&mut defs, MESSAGE_BOLD_ITALIC, mbi.into_iter().collect(), &base);

    ctx.set_fonts(defs);
}

/// Font family names installed on the system (for the Fonts & Colors preferences).
pub fn system_families() -> Vec<String> {
    if let Ok(out) = std::process::Command::new("fc-list").arg(":").arg("family").output() {
        if out.status.success() {
            let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.split(',').next())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            v.sort_by_key(|s| s.to_lowercase());
            v.dedup();
            return v;
        }
    }
    let mut v: Vec<String> = all_font_files()
        .iter()
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.split('-').next().unwrap_or(s).to_string()))
        .collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_private_use_glyphs() {
        let Some(path) = fc_match("Inter") else { return };
        let bytes = std::fs::read(path).unwrap();
        let out = strip_private_use(bytes.clone());
        use skrifa::MetadataProvider;
        let f = skrifa::FontRef::new(&out).unwrap();
        let cm = f.charmap();
        assert!(cm.map('a').is_some());
        assert!(cm.mappings().all(|(c, _)| !(0xE000..=0xF8FF).contains(&c)));
        let orig = skrifa::FontRef::new(&bytes).unwrap();
        assert_eq!(orig.charmap().map('g'), cm.map('g'));
    }
}
