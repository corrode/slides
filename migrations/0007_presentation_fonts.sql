ALTER TABLE decks ADD COLUMN theme_headline_font TEXT NOT NULL DEFAULT 'inter';
ALTER TABLE decks ADD COLUMN theme_text_font TEXT NOT NULL DEFAULT 'inter';
ALTER TABLE decks ADD COLUMN theme_code_font TEXT NOT NULL DEFAULT 'jetbrains-mono';

UPDATE decks
SET theme_headline_font = CASE theme_font
        WHEN 'serif' THEN 'georgia'
        WHEN 'mono' THEN 'system-mono'
        ELSE 'inter'
    END,
    theme_text_font = CASE theme_font
        WHEN 'serif' THEN 'georgia'
        WHEN 'mono' THEN 'system-mono'
        ELSE 'inter'
    END,
    theme_code_font = 'system-mono';

ALTER TABLE deck_versions ADD COLUMN theme_headline_font TEXT NOT NULL DEFAULT 'inter';
ALTER TABLE deck_versions ADD COLUMN theme_text_font TEXT NOT NULL DEFAULT 'inter';
ALTER TABLE deck_versions ADD COLUMN theme_code_font TEXT NOT NULL DEFAULT 'jetbrains-mono';

UPDATE deck_versions
SET theme_headline_font = CASE theme_font
        WHEN 'serif' THEN 'georgia'
        WHEN 'mono' THEN 'system-mono'
        ELSE 'inter'
    END,
    theme_text_font = CASE theme_font
        WHEN 'serif' THEN 'georgia'
        WHEN 'mono' THEN 'system-mono'
        ELSE 'inter'
    END,
    theme_code_font = 'system-mono';
