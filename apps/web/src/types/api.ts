// Match the Rust DTOs in apps/api/src/models.rs and route responses.

export interface Account {
  id: number;
  consent_id: number;
  truelayer_id: string;
  kind: "account" | "card";
  display_name: string;
  iban: string | null;
  sort_code: string | null;
  account_number: string | null;
  card_last4: string | null;
  currency: string;
  enabled: number;
  // 0006
  current_balance_cents: number | null;
  available_balance_cents: number | null;
  overdraft_cents: number | null;
  credit_limit_cents: number | null;
  last_statement_balance_cents: number | null;
  last_statement_date: string | null;
  payment_due_cents: number | null;
  payment_due_date: string | null;
  account_type: string | null;
  card_network: string | null;
  name_on_card: string | null;
  custom_display_name: string | null;
  balance_updated_at: number | null;
  consent_nickname: string | null;
  pending_net_cents: number | null;
}

export interface Transaction {
  id: number;
  account_id: number;
  provider_txn_id: string;
  timestamp: number;
  description: string;
  amount_cents: number;
  currency: string;
  is_credit: number;
  is_pending: number;
  merchant_name: string | null;
  counterparty_iban: string | null;
  counterparty_name: string | null;
  category_id: number | null;
  notes: string | null;
  created_at: number;
  updated_at: number;
}

export interface Category {
  id: number;
  name: string;
  parent_id: number | null;
  icon: string | null;
  colour: string | null;
}

export interface Tag {
  id: number;
  name: string;
}

export interface Rule {
  id: number;
  name: string;
  enabled: number;
  priority: number;
  match_description_regex: string | null;
  match_merchant_regex: string | null;
  match_min_amount_cents: number | null;
  match_max_amount_cents: number | null;
  match_account_id: number | null;
  match_is_credit: number | null;
  set_category_id: number | null;
  add_tag_ids: string | null;
  set_notes: string | null;
  times_applied: number;
  last_applied_at: number | null;
}

export interface Budget {
  id: number;
  name: string;
  category_id: number | null;
  amount_cents: number;
  period: "weekly" | "monthly" | "yearly";
  currency: string;
  rollover: number;
  enabled: number;
}

export interface BudgetStatus {
  budget: Budget;
  spent_cents: number;
  percent: number;
  remaining_cents: number;
  over_budget: boolean;
}

export interface Bill {
  id: number;
  name: string;
  expected_amount_min_cents: number;
  expected_amount_max_cents: number;
  currency: string;
  repeat_freq: string;
  next_expected_date: number | null;
  last_paid_date: number | null;
  match_description_regex: string | null;
  enabled: number;
}

export interface Consent {
  id: number;
  nickname: string;
  enabled: number;
  last_sync_at: number | null;
  last_sync_status: string | null;
  last_sync_error: string | null;
  consent_expires_at: number | null;
}

export interface MeResponse {
  authenticated: boolean;
  awaiting_2fa?: boolean;
  totp_enrolled?: boolean;
  username?: string;
  setup_required: boolean;
}

export interface TransactionsListResponse {
  transactions: Transaction[];
  total: number;
  limit: number;
  offset: number;
}
