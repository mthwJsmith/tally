-- Retirement forecast settings: one row of JSON. Kept as JSON because the plan is a
-- small bag of user assumptions (target age, growth %, salary…) that will evolve;
-- schema churn for each new knob isn't worth it.
CREATE TABLE retirement_plan (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    plan_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
