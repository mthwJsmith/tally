//! Rule engine: matches transactions against user-defined rules in priority order,
//! returns the set of mutations to apply.
//!
//! Rule matching is conservative: ALL provided match-criteria must match (AND).
//! Effects are applied in rule order; first rule that sets a given field wins.

use crate::models::{Rule, Transaction};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;

/// In-memory compiled regex cache so we don't recompile per transaction during big syncs.
pub struct CompiledRules {
    pub rules: Vec<CompiledRule>,
}

pub struct CompiledRule {
    pub rule: Rule,
    pub desc_re: Option<Regex>,
    pub merchant_re: Option<Regex>,
    pub add_tag_ids: Vec<i64>,
}

impl CompiledRules {
    pub fn compile(rules: Vec<Rule>) -> Self {
        let compiled = rules
            .into_iter()
            .map(|r| {
                let desc_re = r
                    .match_description_regex
                    .as_ref()
                    .and_then(|s| Regex::new(&format!("(?i){s}")).ok());
                let merchant_re = r
                    .match_merchant_regex
                    .as_ref()
                    .and_then(|s| Regex::new(&format!("(?i){s}")).ok());
                let add_tag_ids = r
                    .add_tag_ids
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<Vec<i64>>(s).ok())
                    .unwrap_or_default();
                CompiledRule {
                    rule: r,
                    desc_re,
                    merchant_re,
                    add_tag_ids,
                }
            })
            .collect();
        Self { rules: compiled }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuleEffects {
    pub set_category_id: Option<i64>,
    pub add_tag_ids: Vec<i64>,
    pub set_notes: Option<String>,
    pub rules_fired: Vec<i64>,
}

impl RuleEffects {
    pub fn is_empty(&self) -> bool {
        self.set_category_id.is_none()
            && self.add_tag_ids.is_empty()
            && self.set_notes.is_none()
    }
}

/// Apply rules to a transaction, returning the aggregate of effects.
/// First-rule-wins for category and notes; tags accumulate from all matched rules.
pub fn apply(txn: &Transaction, rules: &CompiledRules) -> RuleEffects {
    let mut eff = RuleEffects::default();
    let mut tag_set: HashMap<i64, ()> = HashMap::new();

    for cr in &rules.rules {
        if cr.rule.enabled == 0 {
            continue;
        }
        if !matches_rule(txn, cr) {
            continue;
        }
        eff.rules_fired.push(cr.rule.id);
        if eff.set_category_id.is_none() {
            if let Some(cid) = cr.rule.set_category_id {
                eff.set_category_id = Some(cid);
            }
        }
        if eff.set_notes.is_none() {
            if let Some(ref n) = cr.rule.set_notes {
                eff.set_notes = Some(n.clone());
            }
        }
        for &tid in &cr.add_tag_ids {
            tag_set.insert(tid, ());
        }
    }
    eff.add_tag_ids = tag_set.into_keys().collect();
    eff
}

fn matches_rule(txn: &Transaction, cr: &CompiledRule) -> bool {
    if let Some(ref re) = cr.desc_re {
        if !re.is_match(&txn.description) {
            return false;
        }
    }
    if let Some(ref re) = cr.merchant_re {
        let m = txn.merchant_name.as_deref().unwrap_or("");
        if !re.is_match(m) {
            return false;
        }
    }
    if let Some(min) = cr.rule.match_min_amount_cents {
        if txn.amount_cents < min {
            return false;
        }
    }
    if let Some(max) = cr.rule.match_max_amount_cents {
        if txn.amount_cents > max {
            return false;
        }
    }
    if let Some(aid) = cr.rule.match_account_id {
        if txn.account_id != aid {
            return false;
        }
    }
    if let Some(ic) = cr.rule.match_is_credit {
        if txn.is_credit != ic {
            return false;
        }
    }
    true
}

/// Run rules against existing transactions in DB (for "rebuild after creating a new rule").
/// Returns counts of (matched, mutated).
pub async fn run_all(
    db: &crate::db::Db,
    rules: &CompiledRules,
    only_uncategorised: bool,
) -> Result<(i64, i64)> {
    let mut matched = 0i64;
    let mut mutated = 0i64;
    // page through transactions to avoid loading 100k+ at once
    let mut offset: i64 = 0;
    let page = 500i64;
    loop {
        let mut txns = db
            .list_transactions(None, None, None, None, None, None, None, None, page, offset)
            .await?;
        if txns.is_empty() {
            break;
        }
        let len = txns.len() as i64;
        if only_uncategorised {
            txns.retain(|t| t.category_id.is_none());
        }
        for t in &txns {
            let eff = apply(t, rules);
            if eff.is_empty() {
                continue;
            }
            matched += 1;
            let mut did = false;
            if let Some(cid) = eff.set_category_id {
                if t.category_id != Some(cid) {
                    db.update_transaction_category(t.id, Some(cid)).await?;
                    did = true;
                }
            }
            for tid in &eff.add_tag_ids {
                db.tag_transaction(t.id, *tid).await?;
                did = true;
            }
            if let Some(notes) = &eff.set_notes {
                db.update_transaction_notes(t.id, Some(notes)).await?;
                did = true;
            }
            for rid in &eff.rules_fired {
                let _ = db.bump_rule_applied(*rid).await;
            }
            if did {
                mutated += 1;
            }
        }
        offset += len;
        if len < page {
            break;
        }
    }
    Ok((matched, mutated))
}
