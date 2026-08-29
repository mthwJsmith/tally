pub mod api;
pub mod auth;
pub mod consents;
pub mod dashboard;
pub mod healthz;
pub mod login;
pub mod mcp;
pub mod mcp_render;
pub mod oauth;

use crate::AppState;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post};
use axum::Router;
use std::sync::Arc;
use tower_http::services::ServeDir;

pub fn router(state: Arc<AppState>) -> Router {
    // CSV import accepts a larger body than axum's 2 MB default; cap it explicitly to bound memory.
    let csv_routes = Router::new()
        .route("/csv/preview", post(api::csv_import::preview))
        .route("/csv/commit", post(api::csv_import::commit))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024));

    // Authenticated JSON API.
    let api_routes = Router::new()
        .route("/accounts", get(api::accounts::list))
        .route("/accounts/{id}", patch(api::accounts::patch))
        .route("/consents/{id}/rename", patch(consents::rename))
        .route("/transactions", get(api::transactions::list))
        .route("/transactions/summary", get(api::transactions::spending_summary))
        .route(
            "/transactions/{id}",
            get(api::transactions::get_one).patch(api::transactions::patch),
        )
        .route(
            "/categories",
            get(api::categories::list).post(api::categories::create),
        )
        .route(
            "/categories/{id}",
            patch(api::categories::update).delete(api::categories::delete),
        )
        .route("/tags", get(api::tags::list).post(api::tags::create))
        .route("/tags/attach", post(api::tags::attach))
        .route("/tags/{txn_id}/{tag_id}", delete(api::tags::detach))
        .route("/rules", get(api::rules::list).post(api::rules::create))
        .route("/rules/{id}", delete(api::rules::delete))
        .route("/rules/{id}/toggle", post(api::rules::toggle))
        .route("/rules/{id}/test", post(api::rules::test_one))
        .route("/rules/run-all", post(api::rules::run_all))
        .route("/budgets", get(api::budgets::list).post(api::budgets::create))
        .route("/budgets/{id}", delete(api::budgets::delete))
        .route("/budgets/{id}/status", get(api::budgets::status))
        .route("/safe-to-spend", get(api::safe_to_spend::get_status))
        .route(
            "/safe-to-spend/config",
            get(api::safe_to_spend::get_config).put(api::safe_to_spend::put_config),
        )
        // Planning / "Ahead" forecast layer
        .route("/ahead", get(api::ahead::get_ahead))
        .route(
            "/plan/accounts",
            get(api::plan::list_accounts).post(api::plan::create_account),
        )
        .route("/plan/accounts/sync", post(api::plan::sync_accounts))
        .route(
            "/plan/actions",
            get(api::plan::get_actions).put(api::plan::put_actions),
        )
        .route(
            "/plan/accounts/{id}",
            patch(api::plan::patch_account).delete(api::plan::delete_account),
        )
        .route(
            "/plan/events",
            get(api::plan::list_events).post(api::plan::create_event),
        )
        .route(
            "/plan/events/{id}",
            patch(api::plan::patch_event).delete(api::plan::delete_event),
        )
        .route("/goals", get(api::goals::list).post(api::goals::create))
        .route(
            "/goals/{id}",
            patch(api::goals::patch).delete(api::goals::delete),
        )
        .route("/bills", get(api::bills::list).post(api::bills::create))
        .route("/bills/upcoming", get(api::bills::upcoming))
        .route(
            "/bills/{id}",
            patch(api::bills::update).delete(api::bills::delete),
        )
        .route(
            "/reminders",
            get(api::reminders::list).post(api::reminders::create),
        )
        .route(
            "/reminders/{id}",
            patch(api::reminders::update).delete(api::reminders::delete),
        )
        .route("/reminders/{id}/tick", post(api::reminders::tick))
        .route("/reminders/{id}/untick", post(api::reminders::untick))
        .route(
            "/telegram",
            get(api::telegram::get).put(api::telegram::put),
        )
        .route("/telegram/test", post(api::telegram::test))
        .route(
            "/routine",
            get(api::routine::get).put(api::routine::put),
        )
        .route("/routine/fire", post(api::routine::fire))
        .route("/sync/status", get(consents::api_recent_transactions))
        .route("/sync", post(consents::sync_all))
        .merge(csv_routes)
        .route(
            "/ai/settings",
            get(api::ai::get_settings)
                .put(api::ai::update_settings)
                .post(api::ai::update_settings)
                .patch(api::ai::update_settings),
        )
        .route("/ai/transactions/{id}/suggest", post(api::ai::suggest_one))
        .route("/ai/transactions/bulk", post(api::ai::bulk))
        // Investments
        .route(
            "/brokers",
            get(api::holdings::list_brokers).post(api::holdings::create_broker),
        )
        .route("/brokers/{id}", delete(api::holdings::delete_broker))
        .route(
            "/holdings",
            get(api::holdings::list_holdings).post(api::holdings::create_holding),
        )
        .route("/holdings/{id}", delete(api::holdings::delete_holding))
        .route("/holdings/sync-quotes", post(api::holdings::sync_quotes))
        .route("/holdings/symbol-search", get(api::holdings::symbol_search))
        .route("/holdings/history", get(api::holdings::symbol_history))
        .route(
            "/holdings/portfolio-history",
            get(api::holdings::portfolio_history),
        )
        .route("/holdings/net-worth", get(api::holdings::net_worth))
        .route(
            "/retirement",
            get(api::retirement::get).put(api::retirement::put),
        )
        .route(
            "/retirement/notes",
            get(api::retirement::get_notes).put(api::retirement::put_notes),
        )
        .route(
            "/net-worth/history",
            get(api::retirement::net_worth_history),
        )
        .route(
            "/holdings/{id}/activities",
            get(api::holdings::list_activities),
        )
        .route("/activities", post(api::holdings::create_activity))
        .route(
            "/activities/{id}",
            patch(api::holdings::update_activity).delete(api::holdings::delete_activity),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_auth,
        ));

    // Public auth endpoints (no established session required).
    let public_auth = Router::new()
        .route("/me", get(login::me))
        .route("/setup", post(login::setup))
        .route("/login", post(login::login))
        .route("/verify-2fa", post(login::verify_2fa))
        .route("/recovery", post(login::recovery));
    // Auth endpoints that require a full (post-2FA) session: enrolling/replacing the
    // authenticator and logout. Gating enrol/confirm here is what prevents a password-only
    // (awaiting-2fa) caller from overwriting the victim's TOTP secret.
    let guarded_auth = Router::new()
        .route("/logout", post(login::logout))
        .route("/2fa/enrol", post(login::enrol_2fa))
        .route("/2fa/confirm", post(login::confirm_2fa))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_auth,
        ));
    let auth_routes = public_auth.merge(guarded_auth);

    // The React SPA lives at /app/web. Serve it as fallback so deep links work
    // (/transactions, /budgets etc all serve index.html → SPA routes resolve client-side).
    let spa_serve = ServeDir::new("/app/web")
        .append_index_html_on_directories(true)
        .fallback(tower_http::services::ServeFile::new("/app/web/index.html"));

    // Embedded admin / OAuth-linking pages. These expose linked-bank info and allow
    // destructive consent mutations, so they MUST sit behind the same auth gate as /api
    // (a valid non-awaiting-2fa session) — never publicly reachable.
    let admin_routes = Router::new()
        .route("/consents", get(consents::list).post(consents::create))
        .route("/consents/{id}/sync", post(consents::sync_one))
        .route("/consents/{id}/disable", post(consents::disable))
        .route("/consents/{id}/enable", post(consents::enable))
        .route("/consents/{id}", delete(consents::delete))
        .route("/admin", get(dashboard::index))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_auth,
        ));

    Router::new()
        .merge(admin_routes)
        // TrueLayer redirects the browser here after consent — must stay public.
        // The auth binding is carried in the single-use `state` token, not the session.
        .route("/auth/callback", get(auth::callback))
        // MCP server (Streamable HTTP) — addable as a Claude/ChatGPT connector.
        .route("/mcp", get(mcp::get_handler).post(mcp::post_handler))
        // RFC 9728 Protected Resource Metadata — points MCP clients at the external IdP
        // that issues tokens. tally is a resource server, not an authorization server.
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::protected_resource_metadata),
        )
        .route("/healthz", get(healthz::healthz))
        .nest("/auth", auth_routes)
        .nest("/api", api_routes)
        .nest_service("/static", ServeDir::new("static"))
        .fallback_service(spa_serve)
        .with_state(state)
}
