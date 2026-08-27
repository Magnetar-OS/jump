// Copyright 2026 entro314-labs
// SPDX-License-Identifier: MPL-2.0

//! Web links: quicklinks and fallback searches.
//!
//! Both are the same mechanism — a named URL template with a `{query}` hole —
//! used two ways. A **quicklink** carries a keyword and claims the query the
//! way a plugin keyword does: `yt cats` opens a YouTube search and nothing
//! else answers. A **fallback** has no keyword; the frontend appends fallbacks
//! below the results of an ordinary search, so a query that matched little
//! still ends somewhere useful.
//!
//! Templates come from the user's own configuration, so any scheme `xdg-open`
//! understands is allowed — `https:` mostly, but a `zotero:` or `obsidian:`
//! template is deliberate, not an accident to filter out.

use std::process::Stdio;

use serde::{Deserialize, Serialize};

use tokio::process::Command;

/// A named URL template. `{query}` is replaced with the percent-encoded query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// Display name — "YouTube", "Wikipedia".
    pub name: String,
    /// Word that claims the query. Empty for fallbacks, which are not
    /// addressed but appended.
    #[serde(default)]
    pub keyword: String,
    /// URL with an optional `{query}` placeholder.
    pub template: String,
}

impl Link {
    /// Whether this link's keyword claims `query`, and the text after it.
    ///
    /// Same word-boundary rule as plugin keywords: the keyword must be
    /// followed by a space or end the query, so `youtube` is not read as a
    /// `yt` quicklink plus `outube`.
    #[must_use]
    pub fn match_query<'a>(&self, query: &'a str) -> Option<&'a str> {
        if self.keyword.is_empty() {
            return None;
        }
        let rest = query.strip_prefix(self.keyword.as_str())?;
        match rest.strip_prefix(' ') {
            Some(rest) => Some(rest.trim_start()),
            None if rest.is_empty() => Some(""),
            None => None,
        }
    }

    /// The template with `{query}` filled in.
    #[must_use]
    pub fn url_for(&self, query: &str) -> String {
        self.template.replace("{query}", &encode_component(query))
    }
}

/// Percent-encode `text` for use inside one URL component.
///
/// Everything outside RFC 3986's unreserved set is encoded, spaces included —
/// query strings are where this lands, and `+` for space is a convention some
/// engines do not share, while `%20` is understood by all of them.
#[must_use]
pub fn encode_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                // Uppercase hex is the RFC 3986 normal form.
                encoded.push('%');
                encoded.push(
                    char::from_digit(u32::from(byte >> 4), 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
                encoded.push(
                    char::from_digit(u32::from(byte & 0xf), 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    encoded
}

/// Open `url` with the user's default handler, detached from the caller.
pub fn open(url: &str) {
    let result = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    match result {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(error) => tracing::error!(%error, url, "could not open URL"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(keyword: &str, template: &str) -> Link {
        Link {
            name: "Test".to_owned(),
            keyword: keyword.to_owned(),
            template: template.to_owned(),
        }
    }

    #[test]
    fn keyword_claims_with_word_boundary() {
        let yt = link("yt", "https://youtube.com/results?search_query={query}");
        assert_eq!(yt.match_query("yt cats"), Some("cats"));
        assert_eq!(yt.match_query("yt"), Some(""));
        assert_eq!(yt.match_query("youtube"), None);
        assert_eq!(yt.match_query("firefox"), None);
    }

    #[test]
    fn fallbacks_have_no_keyword_and_claim_nothing() {
        let ddg = link("", "https://duckduckgo.com/?q={query}");
        assert_eq!(ddg.match_query("anything"), None);
    }

    #[test]
    fn query_is_encoded_into_the_template() {
        let ddg = link("", "https://duckduckgo.com/?q={query}");
        assert_eq!(
            ddg.url_for("caffè & crème"),
            "https://duckduckgo.com/?q=caff%C3%A8%20%26%20cr%C3%A8me"
        );
    }

    #[test]
    fn template_without_placeholder_is_left_alone() {
        let home = link("home", "https://example.com/dashboard");
        assert_eq!(home.url_for("ignored"), "https://example.com/dashboard");
    }

    #[test]
    fn unreserved_characters_pass_through() {
        assert_eq!(encode_component("abc-XYZ_0.9~"), "abc-XYZ_0.9~");
        assert_eq!(encode_component("a/b?c=d"), "a%2Fb%3Fc%3Dd");
    }
}
