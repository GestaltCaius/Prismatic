//! The page itself and the images it uses, all embedded in the binary.

use axum::http::{header, HeaderValue};
use axum::response::{Html, IntoResponse, Response};

pub const PAGE: &str = include_str!("ui.html");
const LOGO: &[u8] = include_bytes!("assets/logo.png");
const DUCK: &[u8] = include_bytes!("assets/duck.png");
const DOG: &[u8] = include_bytes!("assets/dog.gif");

pub async fn page() -> Response {
    let mut response = Html(PAGE).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'",
    ));
    response
}

fn image(content_type: &'static str, bytes: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "max-age=3600"),
        ],
        bytes,
    )
        .into_response()
}

pub async fn logo() -> Response {
    image("image/png", LOGO)
}

pub async fn duck() -> Response {
    image("image/png", DUCK)
}

pub async fn dog() -> Response {
    image("image/gif", DOG)
}

#[cfg(test)]
mod tests {
    use super::PAGE;

    fn color(name: &str) -> [f64; 3] {
        let key = format!("--{name}: #");
        let start = PAGE.find(&key).unwrap() + key.len();
        let channel =
            |i: usize| f64::from(u8::from_str_radix(&PAGE[start + i..start + i + 2], 16).unwrap());
        [channel(0), channel(2), channel(4)]
    }

    /// WCAG 2.x contrast ratio between two palette colors
    fn contrast(a: &str, b: &str) -> f64 {
        let luminance = |name: &str| {
            let [r, g, b] = color(name).map(|c| {
                let c = c / 255.0;
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            });
            0.0722f64.mul_add(b, 0.7152f64.mul_add(g, 0.2126 * r))
        };
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    fn failing<'a>(pairs: &[(&'a str, &'a str)], minimum: f64) -> Vec<String> {
        pairs
            .iter()
            .map(|&(a, b)| (a, b, contrast(a, b)))
            .filter(|&(_, _, ratio)| ratio < minimum)
            .map(|(a, b, ratio)| format!("{a} on {b} is {ratio:.2}:1, needs {minimum}:1"))
            .collect()
    }

    #[test]
    fn text_colors_meet_wcag_aa() {
        // Pairs mirror where each color is used: panels sit on `surface`, inputs and the error banner on `bg`
        let pairs = [
            ("text", "surface"),
            ("text", "bg"),
            ("muted", "surface"),
            ("muted", "bg"),
            ("accent-contrast", "accent"),
            ("ok", "surface"),
            ("warn", "surface"),
            ("danger", "surface"),
            ("danger", "bg"),
        ];
        let failures = failing(&pairs, 4.5);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn control_boundaries_and_focus_ring_meet_wcag_non_text_contrast() {
        let pairs = [
            ("border", "surface"),
            ("accent", "surface"),
            ("text", "surface"),
        ];
        let failures = failing(&pairs, 3.0);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn contrast_math_matches_the_wcag_reference() {
        assert!((contrast("text", "text") - 1.0).abs() < f64::EPSILON);
        assert!(contrast("text", "surface") > contrast("muted", "surface"));
    }

    #[test]
    fn page_has_the_accessibility_basics() {
        assert!(PAGE.contains("<html lang=\"en\">"));
        assert!(PAGE.contains("name=\"viewport\""));
        assert!(PAGE.contains(":focus-visible"));
        assert!(PAGE.contains("role=\"alert\""));
        assert!(PAGE.contains("id=\"cookieStatus\" aria-live=\"polite\""));
        assert!(!PAGE.contains("outline: none") && !PAGE.contains("outline: 0"));
        for tag in PAGE.split("<img").skip(1) {
            let tag = &tag[..tag.find('>').unwrap()];
            assert!(tag.contains(" alt="), "image without alt text: <img{tag}>");
        }
        for tag in PAGE.split("<input").skip(1) {
            let tag = &tag[..tag.find('>').unwrap()];
            assert!(
                tag.contains("aria-label="),
                "input without a label: <input{tag}>"
            );
        }
    }
}
