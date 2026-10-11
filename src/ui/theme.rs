//! Visual theme: the Purpamail light and dark palettes.

use egui::{Color32, CornerRadius, Margin, Shadow, Stroke, Style, Vec2, Visuals};

use crate::core::config::Theme;
use crate::core::model::FlagColor;

/// Semantic colors used throughout the interface.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub accent_text: Color32,
    pub window_bg: Color32,
    pub sidebar_bg: Color32,
    pub list_bg: Color32,
    pub viewer_bg: Color32,
    pub toolbar_bg: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub selection_bg: Color32,
    pub selection_inactive_bg: Color32,
    pub selection_text: Color32,
    pub hover_bg: Color32,
    pub unread_dot: Color32,
    pub vip: Color32,
    pub danger: Color32,
    pub warning_bg: Color32,
    pub quote_bar: Color32,
    pub field_bg: Color32,
}

impl Palette {
    pub fn light() -> Self {
        let accent = Color32::from_rgb(0x7b, 0x4f, 0xd6);
        Self {
            dark: false,
            accent,
            accent_soft: Color32::from_rgb(0xee, 0xe7, 0xfb),
            accent_text: Color32::WHITE,
            window_bg: Color32::from_rgb(0xff, 0xff, 0xff),
            sidebar_bg: Color32::from_rgb(0xf3, 0xf0, 0xf8),
            list_bg: Color32::from_rgb(0xff, 0xff, 0xff),
            viewer_bg: Color32::from_rgb(0xff, 0xff, 0xff),
            toolbar_bg: Color32::from_rgb(0xf8, 0xf6, 0xfb),
            separator: Color32::from_rgb(0xe2, 0xdd, 0xea),
            text: Color32::from_rgb(0x1d, 0x1b, 0x22),
            text_secondary: Color32::from_rgb(0x6b, 0x67, 0x75),
            text_tertiary: Color32::from_rgb(0x9a, 0x96, 0xa3),
            selection_bg: accent,
            selection_inactive_bg: Color32::from_rgb(0xe3, 0xdc, 0xf2),
            selection_text: Color32::WHITE,
            hover_bg: Color32::from_rgb(0xf1, 0xed, 0xf8),
            unread_dot: accent,
            vip: Color32::from_rgb(0xf0, 0xb4, 0x1c),
            danger: Color32::from_rgb(0xe0, 0x3b, 0x3b),
            warning_bg: Color32::from_rgb(0xff, 0xf6, 0xdb),
            quote_bar: Color32::from_rgb(0x7b, 0x4f, 0xd6),
            field_bg: Color32::from_rgb(0xff, 0xff, 0xff),
        }
    }

    pub fn dark() -> Self {
        let accent = Color32::from_rgb(0x9d, 0x7b, 0xf0);
        Self {
            dark: true,
            accent,
            accent_soft: Color32::from_rgb(0x3a, 0x30, 0x55),
            accent_text: Color32::WHITE,
            window_bg: Color32::from_rgb(0x1e, 0x1c, 0x23),
            sidebar_bg: Color32::from_rgb(0x26, 0x23, 0x2d),
            list_bg: Color32::from_rgb(0x1e, 0x1c, 0x23),
            viewer_bg: Color32::from_rgb(0x1a, 0x18, 0x1f),
            toolbar_bg: Color32::from_rgb(0x2a, 0x27, 0x31),
            separator: Color32::from_rgb(0x3a, 0x36, 0x42),
            text: Color32::from_rgb(0xec, 0xea, 0xf0),
            text_secondary: Color32::from_rgb(0xa8, 0xa3, 0xb3),
            text_tertiary: Color32::from_rgb(0x7a, 0x75, 0x85),
            selection_bg: Color32::from_rgb(0x6c, 0x4b, 0xc4),
            selection_inactive_bg: Color32::from_rgb(0x3a, 0x34, 0x4a),
            selection_text: Color32::WHITE,
            hover_bg: Color32::from_rgb(0x2e, 0x2a, 0x38),
            unread_dot: accent,
            vip: Color32::from_rgb(0xf5, 0xc5, 0x42),
            danger: Color32::from_rgb(0xff, 0x5c, 0x5c),
            warning_bg: Color32::from_rgb(0x3d, 0x35, 0x1c),
            quote_bar: Color32::from_rgb(0x9d, 0x7b, 0xf0),
            field_bg: Color32::from_rgb(0x15, 0x13, 0x19),
        }
    }

    /// Palette for the current egui theme.
    pub fn of(ctx: &egui::Context) -> Self {
        if ctx.global_style().visuals.dark_mode { Self::dark() } else { Self::light() }
    }

    pub fn flag(&self, c: FlagColor) -> Color32 {
        let [r, g, b] = c.rgb();
        Color32::from_rgb(r, g, b)
    }
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.window_bg;
    v.window_fill = p.window_bg;
    v.extreme_bg_color = p.field_bg;
    v.faint_bg_color = p.hover_bg;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = if p.dark { Color32::from_rgb(0x55, 0x40, 0x99) } else { Color32::from_rgb(0xd6, 0xc8, 0xf5) };
    v.selection.stroke = Stroke::new(1.0, p.text);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, p.separator);
    v.window_shadow = Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if p.dark { 100 } else { 36 }) };
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(6);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(6);
    }
    v.widgets.inactive.weak_bg_fill = if p.dark { Color32::from_rgb(0x35, 0x31, 0x3d) } else { Color32::from_rgb(0xee, 0xeb, 0xf3) };
    v.widgets.inactive.bg_fill = v.widgets.inactive.weak_bg_fill;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.weak_bg_fill = if p.dark { Color32::from_rgb(0x42, 0x3c, 0x4d) } else { Color32::from_rgb(0xe4, 0xdf, 0xee) };
    v.widgets.hovered.bg_fill = v.widgets.hovered.weak_bg_fill;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.separator);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.active.weak_bg_fill = p.accent_soft;
    v.widgets.active.bg_fill = p.accent;
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.open.weak_bg_fill = p.accent_soft;
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    v.striped = false;
    v
}

/// Apply the Purpamail style to the context.
pub fn apply(ctx: &egui::Context, theme: Theme, scale: f32) {
    let pref = match theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Light => egui::ThemePreference::Light,
        Theme::Dark => egui::ThemePreference::Dark,
    };
    ctx.set_theme(pref);
    ctx.set_visuals_of(egui::Theme::Light, visuals(&Palette::light()));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&Palette::dark()));
    ctx.all_styles_mut(|s: &mut Style| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(8.0, 4.0);
        s.spacing.menu_margin = Margin::same(6);
        s.spacing.window_margin = Margin::same(12);
        s.spacing.interact_size.y = 24.0;
        s.interaction.tooltip_delay = 0.6;
        use egui::{FontFamily, FontId, TextStyle};
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(20.0, FontFamily::Name(super::fonts::BOLD.into()))),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        ]
        .into();
    });
    if scale > 0.4 && scale < 4.0 {
        ctx.set_zoom_factor(scale);
    }
}

/// Parse "#rrggbb" into a color.
pub fn hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() == 6 {
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    } else if s.len() == 3 {
        let v = u32::from_str_radix(s, 16).ok()?;
        let r = ((v >> 8) & 0xf) as u8;
        let g = ((v >> 4) & 0xf) as u8;
        let b = (v & 0xf) as u8;
        Some(Color32::from_rgb(r * 17, g * 17, b * 17))
    } else {
        None
    }
}

pub fn to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}
