//! Resolving the client address for rate limiting (roadmap 2.8).

use actix_web::HttpRequest;

use crate::config::RateLimitConfig;

/// The client address to count against the rate limits: when
/// `rate_limit.client_ip_header` is set, the first comma-separated value of
/// that header, trimmed — proxies append, so the leftmost entry is the
/// original client — falling back to the TCP peer address when the header is
/// missing or its first value empty; the peer address when no header is
/// configured.
// Built ahead of its callers: the login and token-link endpoints use it once
// rate limiting is attached to them (roadmap 2.8, step 5).
#[allow(dead_code)]
pub fn client_ip(req: &HttpRequest, config: &RateLimitConfig) -> String {
    let header = config.client_ip_header.trim();
    if !header.is_empty()
        && let Some(value) = req.headers().get(header)
        && let Ok(text) = value.to_str()
        && let Some(first) = text.split(',').next()
    {
        let first = first.trim();
        if !first.is_empty() {
            return first.to_owned();
        }
    }
    req.peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::test::TestRequest;

    fn config(header: &str) -> RateLimitConfig {
        RateLimitConfig {
            enabled: true,
            client_ip_header: header.to_owned(),
        }
    }

    fn request(headers: &[(&str, &str)]) -> HttpRequest {
        let mut req = TestRequest::get().peer_addr("203.0.113.7:5678".parse().unwrap());
        for (name, value) in headers {
            req = req.insert_header((*name, *value));
        }
        req.to_http_request()
    }

    #[test]
    fn without_a_configured_header_the_peer_address_is_used() {
        let req = request(&[("X-Forwarded-For", "198.51.100.8")]);
        assert_eq!(client_ip(&req, &config("")), "203.0.113.7:5678");
    }

    #[test]
    fn the_first_comma_separated_value_wins_trimmed() {
        let req = request(&[("X-Forwarded-For", "  198.51.100.8 , 203.0.113.7")]);
        assert_eq!(client_ip(&req, &config("X-Forwarded-For")), "198.51.100.8");
    }

    #[test]
    fn a_missing_or_empty_header_falls_back_to_the_peer_address() {
        let req = request(&[]);
        assert_eq!(
            client_ip(&req, &config("X-Forwarded-For")),
            "203.0.113.7:5678"
        );

        // A proxy that recorded no client leaves the first entry empty.
        let req = request(&[("X-Forwarded-For", "  , 203.0.113.7")]);
        assert_eq!(
            client_ip(&req, &config("X-Forwarded-For")),
            "203.0.113.7:5678"
        );
    }
}
