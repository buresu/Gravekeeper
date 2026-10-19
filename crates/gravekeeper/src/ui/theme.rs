//! Fonts, colors and spacing for both light and dark mode.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke,
    TextStyle, Theme, Visuals,
};

/// Semibold text (headings, emphasis).
pub const SEMIBOLD: &str = "semibold";
/// Larger page titles.
pub const TITLE: &str = "title";
/// Section headings inside a page.
pub const SECTION: &str = "section";

/// Colors that differ between light and dark mode, beyond egui's own visuals.
#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Color32,
    pub card: Color32,
    pub border: Color32,
    pub sidebar: Color32,
    pub text_weak: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub neutral: Color32,
    pub progress: Color32,
}

const DARK: Palette = Palette {
    accent: Color32::from_rgb(0x6E, 0x9B, 0xFF),
    card: Color32::from_rgb(0x22, 0x25, 0x2C),
    border: Color32::from_rgb(0x32, 0x37, 0x41),
    sidebar: Color32::from_rgb(0x17, 0x19, 0x1E),
    text_weak: Color32::from_rgb(0x8E, 0x96, 0xA4),
    success: Color32::from_rgb(0x4C, 0xC9, 0x8A),
    warning: Color32::from_rgb(0xF2, 0xB2, 0x4C),
    danger: Color32::from_rgb(0xF2, 0x6D, 0x6D),
    neutral: Color32::from_rgb(0x8E, 0x96, 0xA4),
    progress: Color32::from_rgb(0x5B, 0x86, 0xE5),
};

const LIGHT: Palette = Palette {
    accent: Color32::from_rgb(0x2F, 0x6F, 0xEB),
    card: Color32::WHITE,
    border: Color32::from_rgb(0xE1, 0xE4, 0xEA),
    sidebar: Color32::from_rgb(0xF0, 0xF2, 0xF5),
    text_weak: Color32::from_rgb(0x67, 0x6F, 0x7D),
    success: Color32::from_rgb(0x1E, 0x9E, 0x5E),
    warning: Color32::from_rgb(0xC2, 0x7A, 0x00),
    danger: Color32::from_rgb(0xD6, 0x3A, 0x3A),
    neutral: Color32::from_rgb(0x80, 0x87, 0x94),
    progress: Color32::from_rgb(0x2F, 0x6F, 0xEB),
};

pub fn palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

/// Installs fonts and both themes. Call once per egui context.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            let (palette, base) = match theme {
                Theme::Dark => (DARK, Visuals::dark()),
                Theme::Light => (LIGHT, Visuals::light()),
            };
            style.visuals = visuals(base, palette);
            spacing(style);
            text_styles(style);
        });
    }
}

fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        &mut fonts,
        "noto-sans",
        include_bytes!("../../assets/fonts/NotoSans-Regular.ttf"),
    );
    add(
        &mut fonts,
        "noto-sans-semibold",
        include_bytes!("../../assets/fonts/NotoSans-SemiBold.ttf"),
    );
    add(
        &mut fonts,
        "phosphor",
        include_bytes!("../../assets/fonts/Phosphor.ttf"),
    );

    // Body text: Noto Sans, then icons, then egui's defaults (emoji, symbols). Other scripts
    // (CJK, ...) come from the system font fallback.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "noto-sans".into());
    proportional.insert(1, "phosphor".into());
    let fallback = proportional.clone();

    let mut semibold = vec!["noto-sans-semibold".to_owned(), "phosphor".to_owned()];
    semibold.extend(fallback.iter().skip(2).cloned());
    fonts
        .families
        .insert(FontFamily::Name(SEMIBOLD.into()), semibold);

    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.push("phosphor".into());
    fonts
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

fn text_styles(style: &mut egui::Style) {
    use FontFamily::{Monospace, Proportional};
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.5, Proportional)),
        (TextStyle::Body, FontId::new(14.0, Proportional)),
        (TextStyle::Button, FontId::new(14.0, Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
        (TextStyle::Heading, semibold(18.0)),
        (TextStyle::Name(TITLE.into()), semibold(22.0)),
        (TextStyle::Name(SECTION.into()), semibold(15.0)),
    ]
    .into();
}

fn spacing(style: &mut egui::Style) {
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 8.0);
    s.button_padding = egui::vec2(10.0, 5.0);
    s.interact_size.y = 28.0;
    s.window_margin = Margin::same(16);
    s.menu_margin = Margin::same(6);
    s.combo_height = 300.0;
}

fn visuals(mut v: Visuals, p: Palette) -> Visuals {
    let radius = CornerRadius::same(6);
    v.selection.bg_fill = p
        .accent
        .gamma_multiply(if v.dark_mode { 0.45 } else { 0.25 });
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_fill = p.card;
    // Only tables opt in to stripes (`Grid::striped`).
    v.striped = false;
    if v.dark_mode {
        v.panel_fill = Color32::from_rgb(0x1B, 0x1D, 0x22);
        v.extreme_bg_color = Color32::from_rgb(0x13, 0x15, 0x19);
        v.faint_bg_color = Color32::from_rgb(0x26, 0x29, 0x31);
        v.code_bg_color = Color32::from_rgb(0x13, 0x15, 0x19);
    } else {
        v.panel_fill = Color32::from_rgb(0xF7, 0xF8, 0xFA);
        v.extreme_bg_color = Color32::from_rgb(0xFB, 0xFC, 0xFD);
        v.faint_bg_color = Color32::from_rgb(0xF3, 0xF5, 0xF8);
        v.code_bg_color = Color32::from_rgb(0xF3, 0xF5, 0xF8);
    }
    let w = &mut v.widgets;
    for widget in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        widget.corner_radius = radius;
    }
    // egui's default text colors are low-contrast greys; use clearer ones.
    let (text, strong) = if v.dark_mode {
        (
            Color32::from_rgb(0xD5, 0xD9, 0xE0),
            Color32::from_rgb(0xF3, 0xF5, 0xF8),
        )
    } else {
        (
            Color32::from_rgb(0x2A, 0x2F, 0x38),
            Color32::from_rgb(0x10, 0x13, 0x18),
        )
    };
    w.noninteractive.fg_stroke = Stroke::new(1.0, text);
    w.inactive.fg_stroke = Stroke::new(1.0, text);
    w.hovered.fg_stroke = Stroke::new(1.5, strong);
    w.active.fg_stroke = Stroke::new(1.5, strong);
    w.open.fg_stroke = Stroke::new(1.0, strong);
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    if v.dark_mode {
        w.inactive.weak_bg_fill = Color32::from_rgb(0x2A, 0x2E, 0x37);
        w.inactive.bg_fill = Color32::from_rgb(0x2A, 0x2E, 0x37);
        w.hovered.weak_bg_fill = Color32::from_rgb(0x33, 0x38, 0x43);
        w.hovered.bg_fill = Color32::from_rgb(0x33, 0x38, 0x43);
    } else {
        w.inactive.weak_bg_fill = Color32::WHITE;
        w.inactive.bg_fill = Color32::WHITE;
        w.hovered.weak_bg_fill = Color32::from_rgb(0xF0, 0xF3, 0xF8);
        w.hovered.bg_fill = Color32::from_rgb(0xF0, 0xF3, 0xF8);
    }
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent.gamma_multiply(0.6));
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.hovered.expansion = 0.0;
    w.active.expansion = 0.0;
    v.text_edit_bg_color = Some(if v.dark_mode {
        Color32::from_rgb(0x15, 0x17, 0x1B)
    } else {
        Color32::WHITE
    });
    v
}
