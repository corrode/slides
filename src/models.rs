pub const DEFAULT_HEADLINE_FONT: &str = "inter";
pub const DEFAULT_TEXT_FONT: &str = "inter";
pub const DEFAULT_CODE_FONT: &str = "jetbrains-mono";
pub const HEADLINE_FONT_IDS: &[&str] = &[
    "inter",
    "bebas-neue",
    "happy",
    "merriweather",
    "system",
    "georgia",
    "jetbrains-mono",
    "system-mono",
];
pub const TEXT_FONT_IDS: &[&str] = &[
    "inter",
    "happy",
    "merriweather",
    "system",
    "georgia",
    "jetbrains-mono",
    "system-mono",
];
pub const CODE_FONT_IDS: &[&str] = &["jetbrains-mono", "system-mono"];
pub const DEFAULT_THEME_BACKGROUND: &str = "#1e1e2e";
pub const DEFAULT_THEME_TEXT: &str = "#cdd6f4";
pub const DEFAULT_THEME_ACCENT: &str = "#f9e2af";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Deck {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub draft_source: String,
    pub theme_headline_font: String,
    pub theme_text_font: String,
    pub theme_code_font: String,
    pub theme_background: String,
    pub theme_text: String,
    pub theme_accent: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeckSummary {
    pub slug: String,
    pub title: String,
    pub published_versions: i64,
    pub active_code: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiTokenSummary {
    pub prefix: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EndedSessionSummary {
    pub title: String,
    pub code: String,
    pub share_token: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeckVersion {
    pub title: String,
    pub source: String,
    pub theme_headline_font: String,
    pub theme_text_font: String,
    pub theme_code_font: String,
    pub theme_background: String,
    pub theme_text: String,
    pub theme_accent: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LiveSession {
    pub id: i64,
    pub deck_version_id: i64,
    pub code: String,
    pub current_slide: i64,
    pub locked: bool,
    pub interaction_open: bool,
    pub results_revealed: bool,
    pub follow_revision: i64,
    pub ended_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub headline_font: String,
    pub text_font: String,
    pub code_font: String,
    pub background: String,
    pub text: String,
    pub accent: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            headline_font: DEFAULT_HEADLINE_FONT.into(),
            text_font: DEFAULT_TEXT_FONT.into(),
            code_font: DEFAULT_CODE_FONT.into(),
            background: DEFAULT_THEME_BACKGROUND.into(),
            text: DEFAULT_THEME_TEXT.into(),
            accent: DEFAULT_THEME_ACCENT.into(),
        }
    }
}

impl Theme {
    pub fn style(&self) -> String {
        let headline_font = headline_font_stack(&self.headline_font);
        let text_font = font_stack(&self.text_font);
        let code_font = font_stack(&self.code_font);
        let is_default = self.background == DEFAULT_THEME_BACKGROUND
            && self.text == DEFAULT_THEME_TEXT
            && self.accent == DEFAULT_THEME_ACCENT;
        let background_deep = if is_default {
            "#11111b"
        } else {
            &self.background
        };
        let surface = if is_default {
            "#181825"
        } else {
            &self.background
        };
        let text_soft = if is_default { "#bac2de" } else { &self.text };

        format!(
            "--font-display:{headline_font};--font-sans:{text_font};--font-mono:{code_font};--bg:{};--bg-deep:{background_deep};--surface:{surface};--text:{};--text-soft:{text_soft};--highlight:{}",
            self.background, self.text, self.accent,
        )
    }
}

pub fn valid_headline_font(font: &str) -> bool {
    HEADLINE_FONT_IDS.contains(&font)
}

pub fn valid_text_font(font: &str) -> bool {
    TEXT_FONT_IDS.contains(&font)
}

pub fn valid_code_font(font: &str) -> bool {
    CODE_FONT_IDS.contains(&font)
}

pub fn legacy_font_id(font: &str) -> &'static str {
    match font {
        "merriweather" | "georgia" => "serif",
        "jetbrains-mono" | "system-mono" => "mono",
        _ => "system",
    }
}

fn headline_font_stack(font: &str) -> &'static str {
    match font {
        "happy" => "'Happy-Headline', 'Inter Variable', sans-serif",
        font => font_stack(font),
    }
}

fn font_stack(font: &str) -> &'static str {
    match font {
        "bebas-neue" => "'Bebas Neue', Impact, sans-serif",
        "happy" => "'Happy', 'Inter Variable', sans-serif",
        "merriweather" => "Merriweather, Georgia, 'Times New Roman', serif",
        "georgia" => "Georgia, 'Times New Roman', serif",
        "jetbrains-mono" => "'JetBrains Mono', 'SFMono-Regular', Consolas, monospace",
        "system-mono" => "'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace",
        "system" => "ui-sans-serif, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
        _ => "'Inter Variable', Inter, ui-sans-serif, system-ui, sans-serif",
    }
}

impl From<&Deck> for Theme {
    fn from(deck: &Deck) -> Self {
        Self {
            headline_font: deck.theme_headline_font.clone(),
            text_font: deck.theme_text_font.clone(),
            code_font: deck.theme_code_font.clone(),
            background: deck.theme_background.clone(),
            text: deck.theme_text.clone(),
            accent: deck.theme_accent.clone(),
        }
    }
}

impl From<&DeckVersion> for Theme {
    fn from(version: &DeckVersion) -> Self {
        Self {
            headline_font: version.theme_headline_font.clone(),
            text_font: version.theme_text_font.clone(),
            code_font: version.theme_code_font.clone(),
            background: version.theme_background.clone(),
            text: version.theme_text.clone(),
            accent: version.theme_accent.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Theme, legacy_font_id};

    #[test]
    fn theme_style_sets_each_font_role_independently() {
        let theme = Theme {
            headline_font: "happy".into(),
            text_font: "happy".into(),
            code_font: "jetbrains-mono".into(),
            ..Theme::default()
        };

        let style = theme.style();

        assert!(style.contains("--font-display:'Happy-Headline', 'Inter Variable', sans-serif"));
        assert!(style.contains("--font-sans:'Happy', 'Inter Variable', sans-serif"));
        assert!(
            style.contains("--font-mono:'JetBrains Mono', 'SFMono-Regular', Consolas, monospace")
        );
        assert_eq!(legacy_font_id("happy"), "system");
        assert_eq!(legacy_font_id("georgia"), "serif");
        assert_eq!(legacy_font_id("system-mono"), "mono");
    }
}
