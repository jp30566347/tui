//! Response reading shared by both API clients.

use color_eyre::eyre::{eyre, Context, Result};

/// Reads a response body with a size cap enforced as it streams.
///
/// The declared length is only a hint, so the loop is what actually enforces
/// the cap; the early check just fails fast when the header is honest. The cap
/// exists so a hostile or malfunctioning endpoint, or a captive portal, cannot
/// exhaust memory.
pub async fn read_body(resp: reqwest::Response, what: &str, max_bytes: usize) -> Result<String> {
    if resp.content_length().is_some_and(|n| n > max_bytes as u64) {
        return Err(eyre!("{what}: response too large"));
    }
    let mut body = Vec::new();
    let mut stream = resp;
    while let Some(chunk) = stream
        .chunk()
        .await
        .with_context(|| format!("{what}: could not read body"))?
    {
        if body.len() + chunk.len() > max_bytes {
            return Err(eyre!("{what}: response too large"));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).with_context(|| format!("{what}: response was not valid UTF-8"))
}

/// First 200 characters of a response body, for error messages.
///
/// Truncates on a character boundary; slicing by byte index would panic on
/// multi-byte UTF-8, which both feeds routinely contain.
pub fn preview(body: &str) -> String {
    let mut s: String = body.chars().take(200).collect();
    if s.len() < body.len() {
        s.push('\u{2026}');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_truncates_on_a_character_boundary() {
        let body = "\u{2019}".repeat(300);
        let p = preview(&body);
        assert!(p.ends_with('\u{2026}'));
        assert_eq!(p.chars().count(), 201);
    }

    #[test]
    fn preview_leaves_a_short_body_whole() {
        assert_eq!(preview("oops"), "oops");
    }
}
