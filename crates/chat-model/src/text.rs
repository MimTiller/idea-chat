//! Message text: how it is written down, what it may contain, and how it is
//! read back into pieces to draw.
//!
//! # Stored form
//!
//! A body is plain text with three kinds of token:
//!
//! ```text
//! <@0192f0c4-…>   a person, by id
//! <#0192f0c4-…>   a channel, by id
//! @everyone       everyone who can read the channel
//! ```
//!
//! People type `@handle`; the server resolves handles to ids when the message
//! is posted ([`resolve_mentions`]), so a mention survives a rename and points
//! at exactly one person. Everything else is a small, forgiving subset of
//! Markdown: `**bold**`, `*italic*` / `_italic_`, `~~struck~~`, `` `code` ``,
//! fenced code blocks, `> quotes`, and bare `http(s)://` links. A delimiter
//! with no partner is just a character.

use crate::Id;

/// The longest message body, in characters.
pub const MESSAGE_MAX: usize = 4000;
/// The longest display name, in characters.
pub const DISPLAY_NAME_MAX: usize = 48;
/// Handle length bounds, in characters.
pub const HANDLE_MIN: usize = 2;
pub const HANDLE_MAX: usize = 32;
/// The longest space, channel or category name.
pub const NAME_MAX: usize = 64;
/// The longest channel topic or bio.
pub const ABOUT_MAX: usize = 300;
/// The longest custom status line.
pub const STATUS_MAX: usize = 120;
/// The shortest password accepted.
pub const PASSWORD_MIN: usize = 10;

/// Why a piece of text was refused, in words a person can act on.
pub type Invalid = String;

/// A handle: 2–32 of `a-z`, `0-9`, `_` and `.`, not starting or ending with
/// a dot. Returned lowercased; callers store what this returns.
pub fn handle(raw: &str) -> Result<String, Invalid> {
    let h = raw.trim().trim_start_matches('@').to_lowercase();
    let n = h.chars().count();
    if n < HANDLE_MIN || n > HANDLE_MAX {
        return Err(format!("A username is {HANDLE_MIN} to {HANDLE_MAX} characters."));
    }
    if !h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.') {
        return Err("A username may use letters, numbers, underscores and dots.".into());
    }
    if h.starts_with('.') || h.ends_with('.') || h.contains("..") {
        return Err("A username can't start or end with a dot, or have two in a row.".into());
    }
    if h == "everyone" || h == "here" {
        return Err("That username is reserved.".into());
    }
    Ok(h)
}

/// A display name: trimmed, 1–48 characters, no control characters.
pub fn display_name(raw: &str) -> Result<String, Invalid> {
    bounded(raw, 1, DISPLAY_NAME_MAX, "A display name")
}

/// A space or category name.
pub fn name(raw: &str) -> Result<String, Invalid> {
    bounded(raw, 1, NAME_MAX, "A name")
}

/// A text or voice channel name: lowercased, runs of spaces become one dash,
/// so `General Chat` is `general-chat`.
pub fn channel_name(raw: &str) -> Result<String, Invalid> {
    let lowered = raw.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    for word in lowered.split_whitespace() {
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(word);
    }
    bounded(&out, 1, NAME_MAX, "A channel name")
}

/// A topic, bio or other short free text; may be empty.
pub fn about(raw: &str) -> Result<String, Invalid> {
    let t = raw.trim();
    if t.chars().count() > ABOUT_MAX {
        return Err(format!("Keep it under {ABOUT_MAX} characters."));
    }
    Ok(t.to_string())
}

/// A status line; empty means none.
pub fn status_text(raw: &str) -> Result<Option<String>, Invalid> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().count() > STATUS_MAX {
        return Err(format!("A status is at most {STATUS_MAX} characters."));
    }
    Ok(Some(t.to_string()))
}

/// A message body: trimmed, not empty (unless it carries attachments, which
/// the caller checks), within [`MESSAGE_MAX`].
pub fn message(raw: &str) -> Result<String, Invalid> {
    let t = raw.trim_matches(|c: char| c == '\n' || c == '\r' || c == ' ');
    if t.chars().count() > MESSAGE_MAX {
        return Err(format!("A message is at most {MESSAGE_MAX} characters."));
    }
    Ok(t.to_string())
}

/// An email address, loosely: something@something.tld, lowercased. The
/// verification email is the real check.
pub fn email(raw: &str) -> Result<String, Invalid> {
    let e = raw.trim().to_lowercase();
    let ok = e.len() <= 254
        && e.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        })
        && !e.chars().any(|c| c.is_whitespace());
    if ok {
        Ok(e)
    } else {
        Err("That doesn't look like an email address.".into())
    }
}

/// A new password: long enough to matter. Composition rules are left out on
/// purpose; length is what resists guessing.
pub fn password(raw: &str) -> Result<(), Invalid> {
    if raw.chars().count() < PASSWORD_MIN {
        return Err(format!("Use at least {PASSWORD_MIN} characters."));
    }
    if raw.len() > 1024 {
        return Err("That password is too long.".into());
    }
    Ok(())
}

fn bounded(raw: &str, min: usize, max: usize, what: &str) -> Result<String, Invalid> {
    let t = raw.trim();
    let n = t.chars().count();
    if n < min {
        return Err(format!("{what} can't be empty."));
    }
    if n > max {
        return Err(format!("{what} is at most {max} characters."));
    }
    if t.chars().any(char::is_control) {
        return Err(format!("{what} can't contain control characters."));
    }
    Ok(t.to_string())
}

/// A body with `@handle` mentions resolved to tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub body: String,
    pub mentions: Vec<Id>,
    pub everyone: bool,
}

/// Turn every `@handle` that `lookup` knows into a `<@id>` token, and note
/// `@everyone`. Handles inside code are left alone, as are ones `lookup`
/// does not know (they stay plain text). Mention tokens already in the body
/// are kept and counted.
pub fn resolve_mentions(body: &str, lookup: impl Fn(&str) -> Option<Id>) -> Resolved {
    let mut out = String::with_capacity(body.len());
    let mut mentions = Vec::new();
    let mut everyone = false;
    let mut in_block = false;
    for (n, line) in body.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") {
            in_block = !in_block;
            out.push_str(line);
            continue;
        }
        if in_block {
            out.push_str(line);
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        let mut in_code = false;
        while i < chars.len() {
            let c = chars[i];
            if c == '`' {
                in_code = !in_code;
                out.push(c);
                i += 1;
                continue;
            }
            if !in_code && c == '<' {
                if let Some((token, len)) = token_at(&chars[i..]) {
                    if let Token::User(id) = token {
                        if !mentions.contains(&id) {
                            mentions.push(id);
                        }
                    }
                    out.extend(&chars[i..i + len]);
                    i += len;
                    continue;
                }
            }
            let boundary = i == 0 || !is_handle_char(chars[i - 1]);
            if !in_code && c == '@' && boundary {
                let end = (i + 1..chars.len()).find(|&j| !is_handle_char(chars[j])).unwrap_or(chars.len());
                // A handle can't end with a dot: "@ana." is Ana, then a full stop.
                let mut end = end;
                while end > i + 1 && chars[end - 1] == '.' {
                    end -= 1;
                }
                let word: String = chars[i + 1..end].iter().collect::<String>().to_lowercase();
                if word == "everyone" {
                    everyone = true;
                    out.push_str("@everyone");
                    i = end;
                    continue;
                }
                if let Some(id) = (!word.is_empty()).then(|| lookup(&word)).flatten() {
                    if !mentions.contains(&id) {
                        mentions.push(id);
                    }
                    out.push_str(&format!("<@{id}>"));
                    i = end;
                    continue;
                }
            }
            out.push(c);
            i += 1;
        }
    }
    Resolved { body: out, mentions, everyone }
}

fn is_handle_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

enum Token {
    User(Id),
    Channel(Id),
}

/// A `<@uuid>` or `<#uuid>` token at the start of `chars`, and its length.
fn token_at(chars: &[char]) -> Option<(Token, usize)> {
    // "<@" + 36 + ">"
    const LEN: usize = 39;
    if chars.len() < LEN || chars[0] != '<' || chars[LEN - 1] != '>' {
        return None;
    }
    let id: String = chars[2..LEN - 1].iter().collect();
    let id = Id::parse_str(&id).ok()?;
    match chars[1] {
        '@' => Some((Token::User(id), LEN)),
        '#' => Some((Token::Channel(id), LEN)),
        _ => None,
    }
}

/// One run of a body, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text { text: String, style: Style },
    Link { url: String },
    User(Id),
    Channel(Id),
    Everyone,
}

/// How a text run is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

/// One block of a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Quote(Vec<Inline>),
    /// A fenced code block, verbatim.
    Code(String),
}

/// Whether a body carries a link a reader would expand: the EmbedLink
/// permission's subject. Code stays code, as in the drawing.
pub fn embeds(body: &str) -> bool {
    parse(body).iter().any(|block| match block {
        Block::Code(_) => false,
        Block::Paragraph(runs) | Block::Quote(runs) =>
            runs.iter().any(|run| matches!(run, Inline::Link { .. })),
    })
}

/// Read a stored body into blocks of styled runs.
pub fn parse(body: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut code: Option<Vec<&str>> = None;
    let mut para: Vec<&str> = Vec::new();
    let mut quote: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, quote: &mut Vec<&str>, blocks: &mut Vec<Block>| {
        if !para.is_empty() {
            blocks.push(Block::Paragraph(inlines(&para.join("\n"))));
            para.clear();
        }
        if !quote.is_empty() {
            blocks.push(Block::Quote(inlines(&quote.join("\n"))));
            quote.clear();
        }
    };
    for line in body.split('\n') {
        if let Some(lines) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                blocks.push(Block::Code(lines.join("\n")));
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        if line.trim_start().starts_with("```") {
            flush(&mut para, &mut quote, &mut blocks);
            code = Some(Vec::new());
            continue;
        }
        if let Some(rest) = line.strip_prefix("> ").or_else(|| (line == ">").then_some("")) {
            if !para.is_empty() {
                blocks.push(Block::Paragraph(inlines(&para.join("\n"))));
                para.clear();
            }
            quote.push(rest);
        } else {
            if !quote.is_empty() {
                blocks.push(Block::Quote(inlines(&quote.join("\n"))));
                quote.clear();
            }
            para.push(line);
        }
    }
    // An unclosed fence: what was typed after it is still code.
    if let Some(lines) = code {
        flush(&mut para, &mut quote, &mut blocks);
        blocks.push(Block::Code(lines.join("\n")));
    }
    flush(&mut para, &mut quote, &mut blocks);
    blocks
}

/// Read one block's text into styled runs.
pub fn inlines(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut style = Style::default();
    let mut buf = String::new();
    let push_text = |buf: &mut String, out: &mut Vec<Inline>, style: Style| {
        if !buf.is_empty() {
            match out.last_mut() {
                Some(Inline::Text { text, style: s }) if *s == style => text.push_str(buf),
                _ => out.push(Inline::Text { text: std::mem::take(buf), style }),
            }
            buf.clear();
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Inline code: everything to the next backtick, verbatim.
        if c == '`' {
            if let Some(close) = (i + 1..chars.len()).find(|&j| chars[j] == '`') {
                push_text(&mut buf, &mut out, style);
                let code: String = chars[i + 1..close].iter().collect();
                out.push(Inline::Text { text: code, style: Style { code: true, ..style } });
                i = close + 1;
                continue;
            }
        }
        if c == '<' {
            if let Some((token, len)) = token_at(&chars[i..]) {
                push_text(&mut buf, &mut out, style);
                out.push(match token {
                    Token::User(id) => Inline::User(id),
                    Token::Channel(id) => Inline::Channel(id),
                });
                i += len;
                continue;
            }
        }
        if c == '@' && chars[i..].starts_with(&['@', 'e', 'v', 'e', 'r', 'y', 'o', 'n', 'e']) {
            let after = chars.get(i + 9);
            if after.is_none_or(|c| !is_handle_char(*c)) {
                push_text(&mut buf, &mut out, style);
                out.push(Inline::Everyone);
                i += 9;
                continue;
            }
        }
        if (c == 'h') && (starts_with(&chars[i..], "https://") || starts_with(&chars[i..], "http://")) {
            let boundary = i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(';
            if boundary {
                let mut end = (i..chars.len()).find(|&j| chars[j].is_whitespace()).unwrap_or(chars.len());
                // Trailing punctuation belongs to the sentence, not the link.
                while end > i && matches!(chars[end - 1], '.' | ',' | ')' | '!' | '?' | ';' | ':' | '"' | '\'') {
                    end -= 1;
                }
                let url: String = chars[i..end].iter().collect();
                if url.len() > 8 {
                    push_text(&mut buf, &mut out, style);
                    out.push(Inline::Link { url });
                    i = end;
                    continue;
                }
            }
        }
        // Delimiters toggle a style only when they have a partner ahead.
        let toggle = |delim: &str, i: usize| -> bool {
            starts_with(&chars[i..], delim) && find_from(&chars, i + delim.len(), delim).is_some()
        };
        if starts_with(&chars[i..], "**") && (style.bold || toggle("**", i)) {
            push_text(&mut buf, &mut out, style);
            style.bold = !style.bold;
            i += 2;
            continue;
        }
        if starts_with(&chars[i..], "~~") && (style.strike || toggle("~~", i)) {
            push_text(&mut buf, &mut out, style);
            style.strike = !style.strike;
            i += 2;
            continue;
        }
        if (c == '*' || c == '_') && (style.italic || toggle(&c.to_string(), i)) {
            // `snake_case` is not italics: an underscore inside a word is a character.
            let inside_word = c == '_'
                && i > 0
                && chars[i - 1].is_alphanumeric()
                && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric());
            if !inside_word {
                push_text(&mut buf, &mut out, style);
                style.italic = !style.italic;
                i += 1;
                continue;
            }
        }
        buf.push(c);
        i += 1;
    }
    push_text(&mut buf, &mut out, style);
    out
}

fn starts_with(chars: &[char], s: &str) -> bool {
    let mut it = chars.iter();
    s.chars().all(|c| it.next() == Some(&c))
}

fn find_from(chars: &[char], from: usize, s: &str) -> Option<usize> {
    (from..chars.len()).find(|&j| starts_with(&chars[j..], s))
}

/// A body as one line of plain text — for a reply's quote, a notification or
/// a search result — with tokens written as `@name` / `#name` through the
/// lookups, and formatting dropped. Cut to `max` characters with an ellipsis.
pub fn plain(body: &str, user: impl Fn(Id) -> String, channel: impl Fn(Id) -> String, max: usize) -> String {
    let mut out = String::new();
    for block in parse(body) {
        if !out.is_empty() {
            out.push(' ');
        }
        match block {
            Block::Code(code) => out.push_str(code.lines().next().unwrap_or("")),
            Block::Paragraph(runs) | Block::Quote(runs) => {
                for run in runs {
                    match run {
                        Inline::Text { text, .. } => out.push_str(&text.replace('\n', " ")),
                        Inline::Link { url } => out.push_str(&url),
                        Inline::User(id) => {
                            out.push('@');
                            out.push_str(&user(id));
                        }
                        Inline::Channel(id) => {
                            out.push('#');
                            out.push_str(&channel(id));
                        }
                        Inline::Everyone => out.push_str("@everyone"),
                    }
                }
            }
        }
    }
    let out = out.trim().to_string();
    if out.chars().count() > max {
        let cut: String = out.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    } else {
        out
    }
}

/// A stored body as its author would retype it: person tokens back to
/// `@handle`, so an edit round-trips through [`resolve_mentions`]. Channel
/// tokens and everything else stay as stored.
pub fn plain_editable(body: &str, handle: impl Fn(Id) -> String) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some((Token::User(id), len)) = token_at(&chars[i..]) {
                let h = handle(id);
                if !h.is_empty() {
                    out.push('@');
                    out.push_str(&h);
                    i += len;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The `@word` being typed at the end of `draft`, if the caret sits in one —
/// what the composer's mention picker filters by.
pub fn mention_query(draft: &str) -> Option<&str> {
    let at = draft.rfind('@')?;
    let before_ok = draft[..at].chars().last().is_none_or(|c| c.is_whitespace());
    let word = &draft[at + 1..];
    (before_ok && word.chars().all(is_handle_char) && word.len() <= HANDLE_MAX).then_some(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    #[test]
    fn handles_are_normalised_and_bounded() {
        assert_eq!(handle("  @Ana.B ").unwrap(), "ana.b");
        assert!(handle("a").is_err());
        assert!(handle("has space").is_err());
        assert!(handle(".dot").is_err());
        assert!(handle("two..dots").is_err());
        assert!(handle("everyone").is_err());
    }

    #[test]
    fn channel_names_become_lowercase_dashed() {
        assert_eq!(channel_name("  General   Chat ").unwrap(), "general-chat");
        assert!(channel_name("   ").is_err());
    }

    #[test]
    fn emails_are_checked_loosely_and_lowercased() {
        assert_eq!(email(" Ana@Example.COM ").unwrap(), "ana@example.com");
        assert!(email("ana@localhost").is_err());
        assert!(email("not an email").is_err());
    }

    #[test]
    fn mentions_resolve_to_tokens_and_are_collected_once() {
        let r = resolve_mentions("hi @Ana and @bob, @ana again. @nobody", |h| match h {
            "ana" => Some(id(1)),
            "bob" => Some(id(2)),
            _ => None,
        });
        assert_eq!(r.mentions, vec![id(1), id(2)]);
        assert!(r.body.starts_with(&format!("hi <@{}> and <@{}>, <@{}> again.", id(1), id(2), id(1))));
        assert!(r.body.ends_with("@nobody"), "unknown handles stay text");
        assert!(!r.everyone);
    }

    #[test]
    fn a_mention_in_code_is_not_a_mention() {
        let r = resolve_mentions("`@ana` and\n```\n@ana\n```\n", |_| Some(id(1)));
        assert!(r.mentions.is_empty());
        assert!(r.body.contains("`@ana`"));
    }

    #[test]
    fn an_email_address_is_not_a_mention() {
        let r = resolve_mentions("mail ana@example.com", |_| Some(id(1)));
        assert!(r.mentions.is_empty());
    }

    #[test]
    fn everyone_is_noted() {
        let r = resolve_mentions("@everyone look", |_| None);
        assert!(r.everyone);
        assert_eq!(r.body, "@everyone look");
    }

    #[test]
    fn existing_tokens_count_as_mentions() {
        let body = format!("re <@{}>", id(7));
        assert_eq!(resolve_mentions(&body, |_| None).mentions, vec![id(7)]);
    }

    #[test]
    fn formatting_parses_with_partners_only() {
        let runs = inlines("a **b** *c* 2*3 snake_case ~~d~~");
        let styled: Vec<(String, Style)> = runs
            .into_iter()
            .map(|r| match r {
                Inline::Text { text, style } => (text, style),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert!(styled.iter().any(|(t, s)| t == "b" && s.bold));
        assert!(styled.iter().any(|(t, s)| t == "c" && s.italic));
        assert!(styled.iter().any(|(t, s)| t.contains("2*3 snake_case") && !s.italic));
        assert!(styled.iter().any(|(t, s)| t == "d" && s.strike));
    }

    #[test]
    fn links_stop_before_trailing_punctuation() {
        let runs = inlines("see https://example.com/a.");
        assert_eq!(runs[1], Inline::Link { url: "https://example.com/a".into() });
        assert_eq!(runs[2], Inline::Text { text: ".".into(), style: Style::default() });
    }

    #[test]
    fn tokens_parse_back_to_people_and_channels() {
        let body = format!("<@{}> in <#{}> @everyone", id(1), id(2));
        let runs = inlines(&body);
        assert_eq!(runs[0], Inline::User(id(1)));
        assert_eq!(runs[2], Inline::Channel(id(2)));
        assert_eq!(runs[4], Inline::Everyone);
    }

    #[test]
    fn blocks_split_code_and_quotes() {
        let blocks = parse("hello\n> quoted\n```\nlet x = 1;\n```\nbye");
        assert_eq!(blocks.len(), 4);
        assert!(matches!(&blocks[1], Block::Quote(_)));
        assert_eq!(blocks[2], Block::Code("let x = 1;".into()));
    }

    #[test]
    fn an_unclosed_fence_is_still_code() {
        assert_eq!(parse("```\nopen"), vec![Block::Code("open".into())]);
    }

    #[test]
    fn plain_text_names_tokens_and_truncates() {
        let body = format!("**hi** <@{}>, a very long tail here", id(1));
        assert_eq!(plain(&body, |_| "ana".into(), |_| "c".into(), 100), "hi @ana, a very long tail here");
        assert_eq!(plain(&body, |_| "ana".into(), |_| "c".into(), 8), "hi @ana…");
    }

    #[test]
    fn an_edit_round_trips_mentions_through_handles() {
        let body = format!("hi <@{}>!", id(1));
        let shown = plain_editable(&body, |_| "ana".into());
        assert_eq!(shown, "hi @ana!");
        assert_eq!(resolve_mentions(&shown, |h| (h == "ana").then(|| id(1))).body, body);
    }

    #[test]
    fn the_mention_query_is_the_word_after_a_trailing_at() {
        assert_eq!(mention_query("hey @an"), Some("an"));
        assert_eq!(mention_query("hey @"), Some(""));
        assert_eq!(mention_query("mail a@b"), None);
        assert_eq!(mention_query("@ana done"), None);
    }

    #[test]
    fn a_custom_emoji_token_is_not_a_mention_and_survives_resolve() {
        // <:name:id> is a space's custom emoji, not a mention: token_at
        // recognises only <@ and <#, so the body passes through untouched
        // even when a handle inside the token resolves.
        let colon_token: Vec<char> = "<:wave:123>".chars().collect();
        assert!(token_at(&colon_token).is_none(),
            "a colon token is not a mention token");
        let r = resolve_mentions("ping @ana :wave:", |h| (h == "ana").then(|| Id::nil()));
        assert!(r.body.ends_with(":wave:"), "a :name: is left as plain text");
        assert_eq!(r.body, format!("ping <@{}> :wave:", Id::nil()), "only the handle resolves; the emoji stays");
        let with_colons = resolve_mentions("<:party:cat>", |h| (h == "party").then(|| Id::nil()));
        assert_eq!(with_colons.body, "<:party:cat>", "the emoji token survives whole");
        assert!(with_colons.mentions.is_empty(), "it is not a mention");
        assert!(!with_colons.everyone, "and not @everyone");
    }
}
