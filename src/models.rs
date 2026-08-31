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
    pub show_join_code: bool,
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
    pub show_join_code: bool,
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
        format!(
            "--font-display:{font};--font-sans:{font};--bg:{};--bg-deep:{};--surface:{};--text:{};--text-soft:{};--accent:{};--accent-bright:{}",
            self.background,
            self.background,
            self.background,
            self.text,
            self.text,
            self.accent,
            self.accent,
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
