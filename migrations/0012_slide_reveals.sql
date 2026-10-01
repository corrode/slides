ALTER TABLE sessions ADD COLUMN reveal_step INTEGER NOT NULL DEFAULT 0 CHECK (reveal_step >= 0);
