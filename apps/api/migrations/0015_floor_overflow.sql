-- 0015: floor-overflow link (a generic, opt-in "this account can't go below its floor" rule).
--
-- The floor was only ever a warning line — it coloured a cell red but never stopped the balance
-- dropping below it. This lets a planning account spill its shortfall into another account: when
-- the projected balance would fall under the floor, the deficit is drawn from `overflow_account_id`
-- instead, so the account sits exactly at its floor and the linked account absorbs the strain.
--
-- e.g. a current account with no overdraft floors at £0 and pushes any shortfall onto a linked
-- overdraft account. NULL (the default) = no overflow; the floor stays a pure warning, as before.
ALTER TABLE plan_accounts ADD COLUMN overflow_account_id INTEGER REFERENCES plan_accounts(id) ON DELETE SET NULL;
