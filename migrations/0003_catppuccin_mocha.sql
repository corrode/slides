UPDATE decks
SET theme_background = '#1e1e2e',
    theme_text = '#cdd6f4',
    theme_accent = '#f9e2af'
WHERE (theme_background = '#151616' AND theme_text = '#f2f2ef' AND theme_accent = '#fab71c')
   OR (theme_background = '#10141b' AND theme_text = '#f4f1e8' AND theme_accent = '#e0ad52');

UPDATE deck_versions
SET theme_background = '#1e1e2e',
    theme_text = '#cdd6f4',
    theme_accent = '#f9e2af'
WHERE (theme_background = '#151616' AND theme_text = '#f2f2ef' AND theme_accent = '#fab71c')
   OR (theme_background = '#10141b' AND theme_text = '#f4f1e8' AND theme_accent = '#e0ad52');
