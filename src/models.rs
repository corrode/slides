pub const DEFAULT_THEME_BACKGROUND: &str = "#1e1e2e";
pub const DEFAULT_THEME_TEXT: &str = "#cdd6f4";
pub const DEFAULT_THEME_ACCENT: &str = "#f9e2af";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Deck {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub draft_source: String,
    pub theme_font: String,
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
pub struct DeckVersion {
    pub id: i64,
    pub title: String,
    pub source: String,
    pub theme_font: String,
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
    pub font: String,
    pub background: String,
    pub text: String,
    pub accent: String,
}

impl Theme {
    pub fn style(&self) -> String {
        let font = match self.font.as_str() {
            "serif" => "Georgia, 'Times New Roman', serif",
            "mono" => "'SFMono-Regular', Consolas, monospace",
            _ => "Inter, ui-sans-serif, system-ui, sans-serif",
        };
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
            "--font-display:{font};--font-sans:{font};--bg:{};--bg-deep:{background_deep};--surface:{surface};--text:{};--text-soft:{text_soft};--highlight:{}",
            self.background, self.text, self.accent,
        )
    }
}

impl From<&Deck> for Theme {
    fn from(deck: &Deck) -> Self {
        Self {
            font: deck.theme_font.clone(),
            background: deck.theme_background.clone(),
            text: deck.theme_text.clone(),
            accent: deck.theme_accent.clone(),
        }
    }
}

impl From<&DeckVersion> for Theme {
    fn from(version: &DeckVersion) -> Self {
        Self {
            font: version.theme_font.clone(),
            background: version.theme_background.clone(),
            text: version.theme_text.clone(),
            accent: version.theme_accent.clone(),
        }
    }
}
