-- 0004: AI-categorisation settings.
-- Stored in the existing `settings` key/value table — no new schema.
-- Just seed defaults so the UI knows what model we're using.

INSERT INTO settings (key, value) VALUES
    ('openrouter_model', 'meta-llama/llama-3.1-8b-instruct:free')
ON CONFLICT(key) DO NOTHING;
