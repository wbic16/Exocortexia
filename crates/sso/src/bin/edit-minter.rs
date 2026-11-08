//! edit-minter — a MOCK stand-in for the edit.phext.io SSO endpoint,
//! for local demos only.
//!
//! It plays the identity side of the exo-sso protocol: any GET (other
//! than `/health`) mints a fresh 120-second assertion for a fixed mock
//! citizen and 302-redirects the browser to the app's callback with it.
//! Anyone who can reach this port can log in as the mock citizen —
//! never expose it beyond a local demo.
//!
//! Environment:
//! * `MOCK_SSO_SECRET` (or `EXO_SSO_SECRET`) — assertion signing secret,
//!   must match the app's `EXO_SSO_SECRET`. Required.
//! * `APP_CALLBACK` — full app callback URL, e.g.
//!   `http://127.0.0.1:8801/auth/callback`. Required.
//! * `MOCK_PORT` — listen port, default 8799.
//! * `EXO_SSO_AUD` — assertion audience, default `exocortexia`.
//!
//! Routes:
//! * `GET /health` → `ok`
//! * `GET <any>?return=<path>` → 302 `{APP_CALLBACK}?assertion=<token>`
//!   where the assertion's `ret` claim is the `return` value
//!   (default `/`).

use exo_core::http::{serve, Request, Response};
use std::sync::Arc;

fn main() {
    let secret = std::env::var("MOCK_SSO_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("EXO_SSO_SECRET").ok().filter(|s| !s.is_empty()));
    let secret = match secret {
        Some(s) => s,
        None => {
            eprintln!(
                "edit-minter: MOCK_SSO_SECRET (or EXO_SSO_SECRET) must be set — \
                 it must match the app's EXO_SSO_SECRET"
            );
            std::process::exit(1);
        }
    };
    let callback = match std::env::var("APP_CALLBACK").ok().filter(|s| !s.is_empty()) {
        Some(c) => c,
        None => {
            eprintln!(
                "edit-minter: APP_CALLBACK must be set, e.g. http://127.0.0.1:8801/auth/callback"
            );
            std::process::exit(1);
        }
    };
    let port: u16 = std::env::var("MOCK_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8799);
    let aud = std::env::var("EXO_SSO_AUD")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "exocortexia".to_string());

    let state = Arc::new((secret, callback, aud));
    println!(
        "edit-minter (MOCK identity) listening on port {} — callback {}",
        port, state.1
    );
    let result = serve(port, move |req: &Request| {
        if req.path == "/health" {
            return Response::text("ok");
        }
        let return_to = req
            .query
            .get("return")
            .cloned()
            .unwrap_or_else(|| "/".to_string());
        let token = exo_sso::mint_assertion(
            &state.0,
            "mock:citizen-1",
            "mock-citizen",
            &state.2,
            &return_to,
            120,
        );
        // Assertion tokens are base64url + '.', already URL-safe.
        let sep = if state.1.contains('?') { '&' } else { '?' };
        Response::redirect(format!("{}{}assertion={}", state.1, sep, token))
    });
    if let Err(e) = result {
        eprintln!("edit-minter: serve failed: {}", e);
        std::process::exit(1);
    }
}
