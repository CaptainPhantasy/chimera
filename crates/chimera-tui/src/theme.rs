//! chimera-tui::theme — Design token system for a competitive terminal UI.
//!
//! A curated color palette and reusable style system inspired by modern
//! developer tools (Claude Code, Linear, Ghostty). All views draw from these
//! tokens so the look is consistent and themeable.

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Color palette
// ---------------------------------------------------------------------------

/// The full color palette. Each field maps to a semantic role.
///
/// Colors are chosen for high contrast on dark backgrounds. The palette leans
/// on a warm ember/cool teal duality: ember accents for the CHIMERA brand and
/// agent activity, teal for information and structure.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    // Brand
    pub ember: Color,
    pub ember_dim: Color,
    pub ember_bright: Color,

    // Structure / information
    pub teal: Color,
    pub teal_dim: Color,

    // Foreground hierarchy
    pub fg: Color,
    pub fg_muted: Color,
    pub fg_subtle: Color,
    pub fg_faint: Color,

    // Background hierarchy
    pub bg: Color,
    pub bg_elevated: Color,
    pub bg_panel: Color,
    pub bg_hover: Color,

    // Semantic
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,

    // Syntax highlight (for code blocks)
    pub syntax_keyword: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_comment: Color,
    pub syntax_fn: Color,
    pub syntax_type: Color,

    // Borders and lines
    pub border: Color,
    pub border_focus: Color,
}

impl Palette {
    /// The default dark theme — CHIMERA's primary look.
    pub fn dark() -> Self {
        Self {
            // Brand: warm ember/orange
            ember: Color::Rgb(235, 134, 74),
            ember_dim: Color::Rgb(160, 82, 45),
            ember_bright: Color::Rgb(255, 170, 100),

            // Structure: cool teal
            teal: Color::Rgb(79, 196, 188),
            teal_dim: Color::Rgb(45, 120, 115),

            // Foreground: warm white hierarchy
            fg: Color::Rgb(224, 222, 221),
            fg_muted: Color::Rgb(168, 165, 163),
            fg_subtle: Color::Rgb(120, 117, 116),
            fg_faint: Color::Rgb(72, 70, 69),

            // Background: deep warm charcoal
            bg: Color::Rgb(24, 23, 27),
            bg_elevated: Color::Rgb(32, 31, 36),
            bg_panel: Color::Rgb(38, 37, 43),
            bg_hover: Color::Rgb(48, 46, 54),

            // Semantic
            success: Color::Rgb(134, 200, 120),
            warning: Color::Rgb(230, 185, 90),
            error: Color::Rgb(220, 95, 95),
            info: Color::Rgb(110, 175, 230),

            // Syntax
            syntax_keyword: Color::Rgb(200, 130, 220),
            syntax_string: Color::Rgb(150, 200, 120),
            syntax_number: Color::Rgb(220, 170, 100),
            syntax_comment: Color::Rgb(110, 108, 108),
            syntax_fn: Color::Rgb(120, 180, 230),
            syntax_type: Color::Rgb(230, 195, 120),

            // Borders
            border: Color::Rgb(55, 54, 62),
            border_focus: Color::Rgb(235, 134, 74),
        }
    }

    /// A light theme variant.
    pub fn light() -> Self {
        Self {
            ember: Color::Rgb(190, 80, 30),
            ember_dim: Color::Rgb(140, 60, 20),
            ember_bright: Color::Rgb(210, 100, 40),
            teal: Color::Rgb(20, 110, 105),
            teal_dim: Color::Rgb(40, 90, 85),
            fg: Color::Rgb(40, 39, 43),
            fg_muted: Color::Rgb(90, 88, 92),
            fg_subtle: Color::Rgb(130, 128, 132),
            fg_faint: Color::Rgb(180, 178, 182),
            bg: Color::Rgb(245, 244, 242),
            bg_elevated: Color::Rgb(238, 237, 234),
            bg_panel: Color::Rgb(232, 230, 227),
            bg_hover: Color::Rgb(222, 220, 216),
            success: Color::Rgb(60, 140, 70),
            warning: Color::Rgb(170, 120, 30),
            error: Color::Rgb(190, 50, 50),
            info: Color::Rgb(50, 110, 180),
            syntax_keyword: Color::Rgb(150, 50, 170),
            syntax_string: Color::Rgb(60, 120, 50),
            syntax_number: Color::Rgb(170, 110, 30),
            syntax_comment: Color::Rgb(150, 148, 145),
            syntax_fn: Color::Rgb(50, 100, 170),
            syntax_type: Color::Rgb(160, 100, 30),
            border: Color::Rgb(205, 203, 200),
            border_focus: Color::Rgb(190, 80, 30),
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::dark()
    }
}

// ---------------------------------------------------------------------------
// Styles — reusable semantic styles built from the palette
// ---------------------------------------------------------------------------

/// A set of pre-built [`Style`]s for common UI roles.
#[derive(Debug, Clone, Copy)]
pub struct Styles {
    pub palette: Palette,
}

impl Styles {
    pub fn new(palette: Palette) -> Self {
        Self { palette }
    }

    pub fn dark() -> Self {
        Self::new(Palette::dark())
    }

    // --- Brand ---
    pub fn brand(&self) -> Style {
        Style::default()
            .fg(self.palette.ember)
            .add_modifier(Modifier::BOLD)
    }

    pub fn brand_dim(&self) -> Style {
        Style::default().fg(self.palette.ember_dim)
    }

    // --- Text hierarchy ---
    pub fn text(&self) -> Style {
        Style::default().fg(self.palette.fg)
    }

    pub fn text_muted(&self) -> Style {
        Style::default().fg(self.palette.fg_muted)
    }

    pub fn text_subtle(&self) -> Style {
        Style::default().fg(self.palette.fg_subtle)
    }

    pub fn text_faint(&self) -> Style {
        Style::default().fg(self.palette.fg_faint)
    }

    // --- Semantic ---
    pub fn success(&self) -> Style {
        Style::default().fg(self.palette.success)
    }

    pub fn warning(&self) -> Style {
        Style::default()
            .fg(self.palette.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn error(&self) -> Style {
        Style::default()
            .fg(self.palette.error)
            .add_modifier(Modifier::BOLD)
    }

    pub fn info(&self) -> Style {
        Style::default().fg(self.palette.info)
    }

    // --- Roles ---
    pub fn role_user(&self) -> Style {
        Style::default()
            .fg(self.palette.teal)
            .add_modifier(Modifier::BOLD)
    }

    pub fn role_assistant(&self) -> Style {
        Style::default()
            .fg(self.palette.ember)
            .add_modifier(Modifier::BOLD)
    }

    pub fn role_system(&self) -> Style {
        Style::default().fg(self.palette.fg_subtle)
    }

    pub fn role_tool(&self) -> Style {
        Style::default().fg(self.palette.fg_muted)
    }

    // --- Syntax ---
    pub fn syntax_keyword(&self) -> Style {
        Style::default().fg(self.palette.syntax_keyword)
    }

    pub fn syntax_string(&self) -> Style {
        Style::default().fg(self.palette.syntax_string)
    }

    pub fn syntax_number(&self) -> Style {
        Style::default().fg(self.palette.syntax_number)
    }

    pub fn syntax_comment(&self) -> Style {
        Style::default().fg(self.palette.syntax_comment)
    }

    pub fn syntax_fn(&self) -> Style {
        Style::default().fg(self.palette.syntax_fn)
    }

    pub fn syntax_type(&self) -> Style {
        Style::default().fg(self.palette.syntax_type)
    }

    // --- UI chrome ---
    pub fn border(&self) -> Style {
        Style::default().fg(self.palette.border)
    }

    pub fn border_focus(&self) -> Style {
        Style::default().fg(self.palette.border_focus)
    }

    pub fn title(&self) -> Style {
        Style::default()
            .fg(self.palette.fg_muted)
            .add_modifier(Modifier::BOLD)
    }

    pub fn highlight(&self) -> Style {
        Style::default()
            .fg(self.palette.fg)
            .add_modifier(Modifier::REVERSED)
    }

    pub fn status_pill_ok(&self) -> Style {
        Style::default()
            .fg(self.palette.bg)
            .bg(self.palette.success)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_pill_warn(&self) -> Style {
        Style::default()
            .fg(self.palette.bg)
            .bg(self.palette.warning)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_pill_err(&self) -> Style {
        Style::default()
            .fg(self.palette.bg)
            .bg(self.palette.error)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_pill_info(&self) -> Style {
        Style::default()
            .fg(self.palette.bg)
            .bg(self.palette.info)
            .add_modifier(Modifier::BOLD)
    }
}

impl Default for Styles {
    fn default() -> Self {
        Self::dark()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_palette_has_distinct_brand_colors() {
        let p = Palette::dark();
        assert_ne!(p.ember, p.teal);
        assert_ne!(p.ember, p.fg);
    }

    #[test]
    fn light_palette_has_distinct_brand_colors() {
        let p = Palette::light();
        assert_ne!(p.ember, p.teal);
        assert_ne!(p.ember, p.fg);
    }

    #[test]
    fn styles_build_from_palette() {
        let s = Styles::dark();
        let brand = s.brand();
        assert!(brand.fg.is_some());
        let err = s.error();
        assert!(err.fg.is_some());
    }

    #[test]
    fn status_pills_have_bg() {
        let s = Styles::dark();
        assert!(s.status_pill_ok().bg.is_some());
        assert!(s.status_pill_warn().bg.is_some());
        assert!(s.status_pill_err().bg.is_some());
    }
}
