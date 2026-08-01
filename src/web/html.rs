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

/// Render a millisecond epoch timestamp as local wall-clock time.
///
/// Local rather than UTC because every reader of these pages is sitting at the machine
/// (or on the LAN) where the events happened, and "when did this actually happen" is the
/// question the column exists to answer. The date is omitted when the timestamp falls on
/// today, which is the common case on a dashboard and keeps the column narrow.
///
/// **Milliseconds are shown deliberately.** `created_at` has millisecond resolution and
/// the bus routinely writes several rows inside one second — a send and the ack it
/// provokes, for instance. Truncating to seconds made consecutive rows look simultaneous,
/// so a correctly ordered table read as though its rows were shuffled. The extra three
/// digits are what let a reader confirm the order rather than doubt it.
///
/// Output contains only digits, `-`, `:` and `.`, so it needs no escaping — but callers
/// pass it through `esc` anyway rather than special-casing one column.
pub fn fmt_time(ms: i64) -> String {
    let Some(utc) = chrono::DateTime::from_timestamp_millis(ms) else {
        // Out of range rather than merely odd: render something honest instead of
        // panicking a whole page over one bad row.
        return format!("t={ms}");
    };
    let local = utc.with_timezone(&chrono::Local);
    if local.date_naive() == chrono::Local::now().date_naive() {
        local.format("%H:%M:%S%.3f").to_string()
    } else {
        local.format("%m-%d %H:%M:%S%.3f").to_string()
    }
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

// A light palette. Deliberately plain: this is an interim scheme pending a proper UI
// pass, so it aims to be legible and unobtrusive rather than designed. Monospace
// throughout because almost every value on these pages is an identifier, a timestamp,
// or a hash, and proportional type makes columns of those harder to scan.
const CSS: &str = "\
body{font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;margin:0;background:#f7f7f8;color:#1f2328}\
nav{padding:.6rem 1rem;background:#fff;border-bottom:1px solid #d8dade}\
nav a{color:#0b57d0;margin-right:1rem;text-decoration:none}\
nav a:hover{text-decoration:underline}\
main{padding:1rem;max-width:72rem}\
h1{font-size:1.25rem;margin:.2rem 0 .8rem}\
h2{font-size:1rem;margin:1.4rem 0 .4rem;color:#57606a}\
table{border-collapse:collapse;width:100%;background:#fff;border:1px solid #e3e5e8}\
td,th{text-align:left;padding:.35rem .6rem;border-bottom:1px solid #eceef1;vertical-align:top}\
tr:last-child td{border-bottom:none}\
th{color:#57606a;font-weight:600;background:#fbfbfc;white-space:nowrap}\
a{color:#0b57d0}\
.msg{white-space:pre-wrap}\
.ev{color:#57606a;font-style:italic}\
.off{color:#8c959f}\
.when{color:#6e7781;white-space:nowrap}\
.detail{color:#424a53}\
.note{font-weight:400;font-size:.8rem;color:#8c959f}\
.human{font-size:.8rem;color:#0b57d0;border:1px solid #cfe0fb;border-radius:.6rem;padding:0 .35rem;margin-left:.4rem}\
.stale{font-size:.8rem;color:#8a5a00;border:1px solid #f0d9a8;border-radius:.6rem;padding:0 .35rem;margin-left:.4rem}\
.relayer{font-size:.8rem;color:#1a7f5a;border:1px solid #b8e0cd;border-radius:.6rem;padding:0 .35rem;margin-left:.4rem}\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_shows_milliseconds_so_ordering_is_legible() {
        // The bus writes several rows inside one second (a send and the ack it
        // provokes). At second resolution those render identically and a correctly
        // ordered table reads as shuffled — this precision is what lets a reader
        // confirm the order instead of doubting it.
        let a = fmt_time(1_785_000_000_123);
        let b = fmt_time(1_785_000_000_124);
        assert_ne!(a, b, "1ms apart must not render identically: {a} vs {b}");
        assert!(a.contains('.'), "expected fractional seconds: {a}");
    }

    #[test]
    fn an_out_of_range_timestamp_does_not_panic_the_page() {
        // One bad row must not take down a whole view.
        let s = fmt_time(i64::MAX);
        assert!(s.contains(&i64::MAX.to_string()), "{s}");
    }

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
