-- Custom recurring reminders / checklists (Help to Save, credit-card due dates, etc.).
-- A reminder has a recurrence and a deadline for the current period; the scheduler notifies
-- when it's due-and-unticked, then rolls the deadline to the next period.
CREATE TABLE reminders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    notes TEXT,
    freq TEXT NOT NULL,                          -- 'hours' | 'days' | 'weeks' | 'months'
    every_n INTEGER NOT NULL DEFAULT 1,
    anchor_day INTEGER,                          -- day-of-month for monthly (e.g. 28)
    due_at INTEGER NOT NULL,                     -- deadline for the CURRENT period (unix secs)
    notify_before INTEGER NOT NULL DEFAULT 0,    -- secs before due_at to ping (e.g. 86400 = 1 day)
    notify_enabled INTEGER NOT NULL DEFAULT 1,
    completed_at INTEGER,                        -- null = not ticked this period
    notified_at INTEGER,                         -- dedup: pinged once per period
    archived INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_reminders_active ON reminders(archived, due_at);
