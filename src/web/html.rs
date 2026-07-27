//! HTML building. There is no template engine here, so escaping is manual and
//! `esc` must wrap every interpolated value.
//!
//! This matters more than it looks: message bodies are model output, and agent names,
//! room names, and file keys are all attacker-influencable in the sense that an agent
//! can choose them. Rendering any of them raw is self-inflicted XSS in your own browser.

/// Escape text for interpolation into HTML element content or a quoted attribute.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            // `&` first: escaping `<` before `&` would produce `&amp;lt;`.
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-encode a string for safe use as a single URL path segment (e.g. a room or
/// agent name spliced into `/rooms/{name}`). Escapes every byte outside the unreserved
/// set `A-Z a-z 0-9 - . _ ~` (RFC 3986 §2.3) — that includes `/`, so this encodes
/// *one* segment and does not preserve internal slashes as separators. It operates on
/// UTF-8 bytes, so multi-byte characters come out as one `%XX` per byte, which is the
/// standard percent-encoding of UTF-8 text.
///
/// This is a distinct job from `esc`: `esc` makes a value safe as HTML, this makes a
/// value safe as a URL path segment. A name that is escaped for the anchor text but
/// spliced raw into `href` is still an injection — the query string / fragment
/// characters `esc` doesn't touch (`?`, `#`, `&`) can still change what the link
/// points at even though the HTML around it is well-formed.
pub fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Wrap a pre-rendered body in the shared page chrome. `body` is passed through
/// verbatim; it is the caller's job to have escaped its parts.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{t} · claude-bus</title><style>{CSS}</style></head>\
         <body><nav><a href=\"/\">overview</a> <a href=\"/rooms\">rooms</a> \
         <a href=\"/agents\">agents</a> <a href=\"/events\">events</a></nav>\
         <main>{body}</main></body></html>",
        t = esc(title),
    )
}

const CSS: &str = "\
body{font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;background:#111;color:#ddd}\
nav{padding:.6rem 1rem;background:#000;border-bottom:1px solid #333}\
nav a{color:#8ab4f8;margin-right:1rem;text-decoration:none}\
main{padding:1rem;max-width:60rem}\
table{border-collapse:collapse;width:100%}\
td,th{text-align:left;padding:.3rem .6rem;border-bottom:1px solid #222;vertical-align:top}\
th{color:#888;font-weight:normal}\
a{color:#8ab4f8}\
.msg{white-space:pre-wrap}\
.ev{color:#888;font-style:italic}\
.off{color:#666}\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_every_character_that_can_break_out_of_html() {
        assert_eq!(esc("<script>"), "&lt;script&gt;");
        assert_eq!(esc("a & b"), "a &amp; b");
        assert_eq!(esc("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(esc("it's"), "it&#39;s");
    }

    #[test]
    fn a_message_body_with_a_script_tag_is_inert() {
        // The realistic attack: an agent writes this into a message and a human opens
        // the transcript.
        let body = "<script>alert('pwned')</script>";
        let out = esc(body);
        assert!(!out.contains("<script"), "must not survive as a tag: {out}");
        assert!(out.contains("&lt;script&gt;"));
    }

    #[test]
    fn ampersand_is_escaped_first_so_entities_are_not_doubled() {
        // Escaping < before & would turn "<" into "&lt;" and then into "&amp;lt;".
        assert_eq!(esc("<"), "&lt;");
        assert_eq!(esc("&lt;"), "&amp;lt;");
    }

    #[test]
    fn encode_path_segment_leaves_unreserved_characters_alone() {
        assert_eq!(encode_path_segment("abcXYZ019-._~"), "abcXYZ019-._~");
    }

    #[test]
    fn encode_path_segment_escapes_a_literal_slash() {
        // A `/` inside a name must not be allowed to introduce a path separator.
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
    }

    #[test]
    fn encode_path_segment_escapes_space_and_url_metacharacters() {
        assert_eq!(encode_path_segment("a b"), "a%20b");
        assert_eq!(encode_path_segment("a?b"), "a%3Fb");
        assert_eq!(encode_path_segment("a#b"), "a%23b");
        assert_eq!(encode_path_segment("a&b"), "a%26b");
    }

    #[test]
    fn encode_path_segment_escapes_non_ascii_as_utf8_bytes() {
        // "é" is 2 UTF-8 bytes (0xC3 0xA9); each byte is percent-encoded separately.
        assert_eq!(encode_path_segment("café"), "caf%C3%A9");
    }

    #[test]
    fn page_wraps_body_and_escapes_the_title() {
        let out = page("a <b> title", "<p>hello</p>");
        assert!(out.starts_with("<!doctype html>"));
        assert!(out.contains("a &lt;b&gt; title"), "title must be escaped");
        assert!(
            out.contains("<p>hello</p>"),
            "body is pre-rendered and passed through"
        );
    }
}
