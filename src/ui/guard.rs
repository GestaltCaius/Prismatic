//! Keeps other web pages and other machines away from the local API.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use super::Shared;

pub const TOKEN_HEADER: &str = "x-prismatic-token";

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Only the loopback names with our own port, anything else is a DNS rebinding attempt
fn allowed_host(host: &str, port: u16) -> bool {
    let host = host.to_ascii_lowercase();
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}

fn token_from_query(query: &str) -> Option<&str> {
    query.split('&').find_map(|pair| pair.strip_prefix("token="))
}

/// Requires a loopback `Host` and the launch token (in the query for the page, in a header for the API)
pub async fn guard(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let host_ok = request.headers().get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| allowed_host(host, state.port));
    let supplied = if request.uri().path() == "/" {
        request.uri().query().and_then(token_from_query)
    } else {
        request.headers().get(TOKEN_HEADER).and_then(|token| token.to_str().ok())
    };
    // Images can't send the token header, and they hold nothing private
    let public = request.uri().path().starts_with("/assets/");
    if !host_ok || !(public || supplied.is_some_and(|token| constant_time_eq(token, &state.token))) {
        return (StatusCode::FORBIDDEN, "Forbidden").into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::{allowed_host, constant_time_eq, token_from_query};

    #[test]
    fn only_loopback_hosts_with_our_port_are_allowed() {
        assert!(allowed_host("127.0.0.1:47821", 47821));
        assert!(allowed_host("LocalHost:47821", 47821));
        assert!(!allowed_host("127.0.0.1:80", 47821));
        assert!(!allowed_host("evil.example:47821", 47821));
        assert!(!allowed_host("127.0.0.1.evil.example:47821", 47821));
    }

    #[test]
    fn finds_the_token_in_a_query() {
        assert_eq!(token_from_query("token=abc"), Some("abc"));
        assert_eq!(token_from_query("x=1&token=abc"), Some("abc"));
        assert_eq!(token_from_query("x=1"), None);
    }

    #[test]
    fn compares_tokens_exactly() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
