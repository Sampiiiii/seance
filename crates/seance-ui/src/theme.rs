use gpui::Hsla;

fn solid(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

fn alpha(hex: u32, a: f32) -> Hsla {
    let mut c: Hsla = gpui::rgb(hex).into();
    c.a = a;
    c
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeId {
    ObsidianSmoke,
    MidnightFrost,
    Bone,
    Phosphor,
}

impl ThemeId {
    pub const ALL: &[ThemeId] = &[
        ThemeId::ObsidianSmoke,
        ThemeId::MidnightFrost,
        ThemeId::Bone,
        ThemeId::Phosphor,
    ];

    pub fn theme(&self) -> Theme {
        match self {
            ThemeId::ObsidianSmoke => obsidian_smoke(),
            ThemeId::MidnightFrost => midnight_frost(),
            ThemeId::Bone => bone(),
            ThemeId::Phosphor => phosphor(),
        }
    }
}

pub struct Theme {
    pub name: &'static str,

    pub bg_void: Hsla,
    pub bg_deep: Hsla,

    pub glass_tint: Hsla,
    pub glass_strong: Hsla,
    pub glass_border: Hsla,
    pub glass_border_bright: Hsla,
    pub glass_hover: Hsla,
    pub glass_active: Hsla,

    pub sidebar_bg_elevated: Hsla,
    pub sidebar_edge: Hsla,
    pub sidebar_edge_bright: Hsla,
    pub sidebar_row_hover: Hsla,
    pub sidebar_row_active: Hsla,
    pub sidebar_section_label: Hsla,
    pub sidebar_meta: Hsla,
    pub sidebar_separator: Hsla,
    pub sidebar_indicator: Hsla,
    pub shell_divider_glow: Hsla,

    pub accent: Hsla,
    pub accent_glow: Hsla,
    pub selection_soft: Hsla,

    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub text_ghost: Hsla,

    pub warning: Hsla,
    pub scrim: Hsla,

    pub palette_group_label: Hsla,
    pub palette_group_separator: Hsla,
}

fn obsidian_smoke() -> Theme {
    Theme {
        name: "Obsidian Smoke",
        bg_void: solid(0x08070a),
        bg_deep: solid(0x0e0d13),
        glass_tint: alpha(0x1f1e2a, 0.60),
        glass_strong: alpha(0x1a1926, 0.87),
        glass_border: alpha(0xffffff, 0.07),
        glass_border_bright: alpha(0xffffff, 0.13),
        glass_hover: alpha(0xffffff, 0.04),
        glass_active: alpha(0xffffff, 0.09),
        sidebar_bg_elevated: alpha(0x16171d, 0.94),
        sidebar_edge: alpha(0xffffff, 0.06),
        sidebar_edge_bright: alpha(0xffffff, 0.13),
        sidebar_row_hover: alpha(0xffffff, 0.035),
        sidebar_row_active: alpha(0x23242b, 0.96),
        sidebar_section_label: solid(0x605c56),
        sidebar_meta: solid(0x5b5751),
        sidebar_separator: alpha(0xffffff, 0.05),
        sidebar_indicator: alpha(0xc9885f, 0.80),
        shell_divider_glow: alpha(0xc9885f, 0.08),
        accent: solid(0xc9885f),
        accent_glow: alpha(0xc9885f, 0.14),
        selection_soft: alpha(0xffffff, 0.06),
        text_primary: solid(0xe8e4dd),
        text_secondary: solid(0xa8a29c),
        text_muted: solid(0x706b63),
        text_ghost: solid(0x46423c),
        warning: solid(0xebcb8b),
        scrim: alpha(0x08070a, 0.85),
        palette_group_label: solid(0x58534c),
        palette_group_separator: alpha(0xffffff, 0.05),
    }
}

fn midnight_frost() -> Theme {
    Theme {
        name: "Midnight Frost",
        bg_void: solid(0x060a10),
        bg_deep: solid(0x0a1018),
        glass_tint: alpha(0x0f1a2a, 0.55),
        glass_strong: alpha(0x0d1624, 0.85),
        glass_border: alpha(0x80b0e0, 0.08),
        glass_border_bright: alpha(0x80b0e0, 0.15),
        glass_hover: alpha(0x80b0e0, 0.04),
        glass_active: alpha(0x80b0e0, 0.10),
        sidebar_bg_elevated: alpha(0x0f1520, 0.94),
        sidebar_edge: alpha(0x80b0e0, 0.08),
        sidebar_edge_bright: alpha(0xb7d2ee, 0.16),
        sidebar_row_hover: alpha(0x80b0e0, 0.035),
        sidebar_row_active: alpha(0x141d2a, 0.96),
        sidebar_section_label: solid(0x64788a),
        sidebar_meta: solid(0x5f7487),
        sidebar_separator: alpha(0x80b0e0, 0.06),
        sidebar_indicator: alpha(0x5b9bd5, 0.80),
        shell_divider_glow: alpha(0x5b9bd5, 0.08),
        accent: solid(0x5b9bd5),
        accent_glow: alpha(0x5b9bd5, 0.14),
        selection_soft: alpha(0x80b0e0, 0.07),
        text_primary: solid(0xe4eaf0),
        text_secondary: solid(0x8ea4b8),
        text_muted: solid(0x5a7088),
        text_ghost: solid(0x354555),
        warning: solid(0xe5c07b),
        scrim: alpha(0x060a10, 0.85),
        palette_group_label: solid(0x4a6078),
        palette_group_separator: alpha(0x80b0e0, 0.06),
    }
}

fn bone() -> Theme {
    Theme {
        name: "Bone",
        bg_void: solid(0xf0ece4),
        bg_deep: solid(0xe8e2d8),
        glass_tint: alpha(0xf5f0e8, 0.70),
        glass_strong: alpha(0xede8df, 0.85),
        glass_border: alpha(0x000000, 0.08),
        glass_border_bright: alpha(0x000000, 0.14),
        glass_hover: alpha(0x000000, 0.03),
        glass_active: alpha(0x000000, 0.07),
        sidebar_bg_elevated: alpha(0xf7f1e8, 0.96),
        sidebar_edge: alpha(0x000000, 0.08),
        sidebar_edge_bright: alpha(0x000000, 0.14),
        sidebar_row_hover: alpha(0x000000, 0.028),
        sidebar_row_active: alpha(0xffffff, 0.72),
        sidebar_section_label: solid(0x857b70),
        sidebar_meta: solid(0x8a8278),
        sidebar_separator: alpha(0x000000, 0.06),
        sidebar_indicator: alpha(0x8b5e3c, 0.80),
        shell_divider_glow: alpha(0x8b5e3c, 0.07),
        accent: solid(0x8b5e3c),
        accent_glow: alpha(0x8b5e3c, 0.10),
        selection_soft: alpha(0x000000, 0.045),
        text_primary: solid(0x1a1816),
        text_secondary: solid(0x5c564e),
        text_muted: solid(0x8a8278),
        text_ghost: solid(0xb8b0a6),
        warning: solid(0xb8860b),
        scrim: alpha(0x1a1816, 0.50),
        palette_group_label: solid(0x9a9088),
        palette_group_separator: alpha(0x000000, 0.06),
    }
}

fn phosphor() -> Theme {
    Theme {
        name: "Phosphor",
        bg_void: solid(0x040604),
        bg_deep: solid(0x080a08),
        glass_tint: alpha(0x0a100a, 0.90),
        glass_strong: alpha(0x080e08, 0.93),
        glass_border: alpha(0x33ff66, 0.06),
        glass_border_bright: alpha(0x33ff66, 0.12),
        glass_hover: alpha(0x33ff66, 0.03),
        glass_active: alpha(0x33ff66, 0.08),
        sidebar_bg_elevated: alpha(0x081008, 0.96),
        sidebar_edge: alpha(0x33ff66, 0.07),
        sidebar_edge_bright: alpha(0x33ff66, 0.13),
        sidebar_row_hover: alpha(0x33ff66, 0.03),
        sidebar_row_active: alpha(0x0b160b, 0.97),
        sidebar_section_label: solid(0x188c32),
        sidebar_meta: solid(0x168830),
        sidebar_separator: alpha(0x33ff66, 0.05),
        sidebar_indicator: alpha(0x33ff66, 0.80),
        shell_divider_glow: alpha(0x33ff66, 0.07),
        accent: solid(0x33ff66),
        accent_glow: alpha(0x33ff66, 0.12),
        selection_soft: alpha(0x33ff66, 0.055),
        text_primary: solid(0x33ff66),
        text_secondary: solid(0x22cc44),
        text_muted: solid(0x168830),
        text_ghost: solid(0x0a4418),
        warning: solid(0xffcc33),
        scrim: alpha(0x040604, 0.90),
        palette_group_label: solid(0x0e6624),
        palette_group_separator: alpha(0x33ff66, 0.04),
    }
}
