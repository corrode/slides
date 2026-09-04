UPDATE decks
SET theme_background = '#282934',
    theme_text = '#e1e1e1',
    theme_accent = '#fc218a'
WHERE theme_background = '#1e1e2e'
  AND theme_text = '#cdd6f4'
  AND theme_accent = '#f9e2af';

UPDATE deck_versions
SET theme_background = '#282934',
    theme_text = '#e1e1e1',
    theme_accent = '#fc218a'
WHERE theme_background = '#1e1e2e'
  AND theme_text = '#cdd6f4'
  AND theme_accent = '#f9e2af';
