//! Dark theme tokens (spec 4.1) and the WCAG contrast check (spec 3.12).

use eframe::egui::{self, Color32, CornerRadius, Stroke};

/// Panel surface.
pub const SURFACE: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Darker chrome behind panels (menu bar, status bar).
pub const SURFACE_DEEP: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
/// Raised controls (buttons, rollout headers).
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
/// Viewport background.
pub const VIEWPORT_BG: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);
pub const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xE6);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xB8, 0xB8, 0xB8);
pub const DISABLED: Color32 = Color32::from_rgb(0x8A, 0x8A, 0x8A);
pub const ACCENT: Color32 = Color32::from_rgb(0xF2, 0xC1, 0x4E);
pub const PASS: Color32 = Color32::from_rgb(0x57, 0xC2, 0x6B);
pub const WARN: Color32 = Color32::from_rgb(0xF2, 0xC1, 0x4E);
/// Fail red for strokes, fills and glyphs only (4.09:1 on [`SURFACE`]).
pub const FAIL: Color32 = Color32::from_rgb(0xE5, 0x53, 0x4B);
/// Fail red for text: a lighter tint that reaches 4.5:1 on [`SURFACE`] and
/// on [`SURFACE_DEEP`]. Use it for every label, never [`FAIL`].
pub const FAIL_TEXT: Color32 = Color32::from_rgb(0xFF, 0x7B, 0x72);
pub const AXIS_X: Color32 = Color32::from_rgb(0xE0, 0x5A, 0x50);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x6C, 0xC0, 0x5A);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x5A, 0x8C, 0xE6);

/// Push the Vernon dark theme into egui. Call once at start-up.
pub fn apply(ctx: &egui::Context) {
    // The bundled monospace font covers arrows and geometric shapes the
    // proportional fonts lack; use it as the last fallback.
    let mut fonts = egui::FontDefinitions::default();
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = SURFACE;
        v.window_fill = SURFACE;
        v.faint_bg_color = Color32::from_rgb(0x2C, 0x2C, 0x2C);
        v.extreme_bg_color = Color32::from_rgb(0x1A, 0x1A, 0x1A);
        v.code_bg_color = SURFACE_DEEP;
        v.hyperlink_color = ACCENT;
        v.warn_fg_color = WARN;
        v.error_fg_color = FAIL_TEXT;
        v.selection.bg_fill = Color32::from_rgb(0x7A, 0x5E, 0x1C);
        v.selection.stroke = Stroke::new(1.0_f32, TEXT);
        v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x44, 0x44, 0x44));
        v.window_corner_radius = CornerRadius::same(3);
        v.menu_corner_radius = CornerRadius::same(3);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x3C, 0x3C, 0x3C));
        w.inactive.bg_fill = SURFACE_RAISED;
        w.inactive.weak_bg_fill = SURFACE_RAISED;
        w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.hovered.bg_fill = Color32::from_rgb(0x45, 0x45, 0x45);
        w.hovered.weak_bg_fill = Color32::from_rgb(0x45, 0x45, 0x45);
        w.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT);
        w.active.bg_fill = Color32::from_rgb(0x55, 0x55, 0x55);
        w.active.weak_bg_fill = Color32::from_rgb(0x55, 0x55, 0x55);
        w.open.weak_bg_fill = Color32::from_rgb(0x40, 0x40, 0x40);
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = CornerRadius::same(2);
        }
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
        style.spacing.interact_size.y = 20.0;
    });
}

/// Relative luminance of an sRGB colour (WCAG 2.1).
#[cfg_attr(not(test), allow(dead_code))]
pub fn luminance(c: Color32) -> f64 {
    let lin = |v: u8| {
        let c = v as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio, always >= 1.
#[cfg_attr(not(test), allow(dead_code))]
pub fn contrast_ratio(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Lerp between two colours, `t` in 0..1.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_contrast_ratios_on_surface() {
        let check = |c: Color32, want: f64| {
            let r = contrast_ratio(c, SURFACE);
            assert!((r - want).abs() < 0.01, "{c:?}: {r} vs {want}");
        };
        check(TEXT, 12.13);
        check(TEXT_DIM, 7.63);
        check(ACCENT, 9.02);
        check(PASS, 6.73);
        check(FAIL, 4.09);
        check(FAIL_TEXT, 6.00);
        check(DISABLED, 4.38);
    }

    #[test]
    fn text_tokens_meet_aa() {
        for c in [TEXT, TEXT_DIM, ACCENT, PASS, FAIL_TEXT] {
            assert!(contrast_ratio(c, SURFACE) >= 4.5);
            assert!(contrast_ratio(c, SURFACE_DEEP) >= 4.5);
        }
        // Fail red is allowed as a non-text indicator only (>= 3).
        assert!(contrast_ratio(FAIL, SURFACE) >= 3.0);
        assert!(contrast_ratio(FAIL, SURFACE) < 4.5, "FAIL is the non-text red; text uses FAIL_TEXT");
    }

    #[test]
    fn mix_endpoints() {
        assert_eq!(mix(TEXT, SURFACE, 0.0), TEXT);
        assert_eq!(mix(TEXT, SURFACE, 1.0), SURFACE);
    }
}
