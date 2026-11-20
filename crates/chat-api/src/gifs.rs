//! GIF search: a server-side proxy for Giphy, so no client ever holds the
//! API key.
//!
//! ## Design
//!
//! The browser never talks to Giphy's API: [`gif_search`] runs the search on
//! the server, where the key lives in the environment (`CHAT_GIPHY_KEY`, or
//! the file named by `CHAT_GIPHY_KEY_FILE`); with neither, the verb answers
//! a typed "not configured" refusal. The upstream base is `CHAT_GIPHY_BASE`
//! (default Giphy's API; tests point it at a local stub).
//!
//! The key never rides out: it rides in the request's query string, and
//! every upstream trouble resolves to a fixed sentence — never an upstream
//! body, never the request — so the key cannot leak into a response, an
//! event or a log line.
//!
//! Each GIF comes back with a small rendition for the picker grid and a
//! bigger one to send; the media URL carries its pixel size in the
//! fragment (`…/giphy.gif#480x270`). An empty `query` means Giphy's
//! trending GIFs. Results are filtered to Giphy's media hosts
//! ([`media_host_ok`], the same allow-list RWND accepts in a message body),
//! so a search's URLs are safe to put in a message: a GIF is sent as an
//! ordinary message whose body is the media URL — no new message type —
//! and rendering is the client's job.
//!
//! Rate limiting rides the existing server-kit limiter, keyed by account,
//! through the verb's `limit` tag: 10/min, so an account hammering search
//! gets a `Slow` with a retry time and the dev-tier key stays inside its
//! 100 calls an hour.

use serde::{Deserialize, Serialize};
use server::server;

use crate::{Answer, Principal};
use server_kit::Auth;

/// Results per page.
pub const PAGE_SIZE: u32 = 30u32;

/// A search's query is refused past this many characters.
pub const QUERY_MAX_CHARS: usize = 50usize;

/// The furthest page a search may ask for.
pub const OFFSET_MAX: u32 = 500u32;

/// One GIF: a small rendition for the picker grid and a bigger one to send.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gif {
    pub id: String,
    pub title: String,
    pub preview_url: String,
    pub preview_width: u32,
    pub preview_height: u32,
    /// The media URL carries its pixel size in the fragment.
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// One page of results; `next_offset` is `None` on the last page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GifPage {
    pub gifs: Vec<Gif>,
    pub next_offset: Option<u32>,
}

/// Whether [`gif_search`] accepts an offset.
pub fn offset_ok(offset: u32) -> bool {
    offset <= OFFSET_MAX
}

/// Whether [`gif_search`] accepts a query (after trimming).
pub fn query_ok(query: String) -> bool {
    query.trim().chars().count() <= QUERY_MAX_CHARS
}

/// Whether a URL is on Giphy's media hosts — the same allow-list RWND accepts
/// in a message body, so a search's URLs are safe to put in a message.
pub fn media_host_ok(url: String) -> bool {
    gif_body(url).is_some()
}

/// A Giphy value parsed as a u32, whether it came back as a string ("200") or
/// a number; `0` when neither.
pub fn num(v: &serde_json::Value) -> u32 {
    match v.as_str() {
        Some(s) => s.trim().parse().ok().unwrap_or(0u32),
        None => v.as_u64().unwrap_or(0u64) as u32,
    }
}

/// The `(url, width, height)` of one named rendition inside an image map, or
/// `None` when the rendition is missing, has no URL, or reports a zero side.
pub fn rendition(images: serde_json::Value, name: String) -> Option<(String, u32, u32)> {
    let Some(r) = images.get(&name) else { return None };
    let Some(url) = r.get("url").and_then(|u| u.as_str()) else { return None };
    let (w, h) = (num(r.get("width").unwrap_or(&serde_json::Value::Null)), num(r.get("height").unwrap_or(&serde_json::Value::Null)));
    (url.starts_with("https://") && w > 0 && h > 0).then_some((url.to_string(), w, h))
}

/// Parse a Giphy search/trending response. GIFs missing a usable preview, or on
/// a host outside [`media_host_ok`], are skipped.
pub fn parse_giphy(json: serde_json::Value) -> GifPage {
    let Some(items) = json.get("data").and_then(|d| d.as_array()) else {
        return GifPage { gifs: [].to_vec(), next_offset: None }
    };
    let gifs = items.iter().filter_map(
        move |g|
        {
            let Some(images) = g.get("images") else { return None };
            let Some((preview_url, preview_width, preview_height)) = rendition(images.clone(), "fixed_width".into()) else { return None };
            if !media_host_ok(preview_url.clone()) {
                return None
            }
            let (url, width, height) = match rendition(images.clone(), "downsized".into()) {
                Some(uwh) => uwh,
                None => rendition(images.clone(), "original".into())
                    .unwrap_or((preview_url.clone(), preview_width, preview_height)),
            };
            if !media_host_ok(url.clone()) {
                return None
            }
            Some(
                Gif {
                    id: g.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    title: g.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    preview_url,
                    preview_width,
                    preview_height,
                    url,
                    width,
                    height,
                }
            )
        }
    )
    .collect();
    let null = serde_json::Value::Null;
    let p = json.get("pagination").unwrap_or(&null);
    let (offset, count, total) = (
        num(p.get("offset").unwrap_or(&null)),
        num(p.get("count").unwrap_or(&null)),
        num(p.get("total_count").unwrap_or(&null)),
    );
    let next = offset + count;
    GifPage { gifs, next_offset: (count > 0 && next < total).then_some(next) }
}

/// The message body that sends `gif`: its URL with the size in the fragment. A
/// client sends this as an ordinary message body.
pub fn gif_message(gif: Gif) -> String {
    let base = gif.url.split('#').next().unwrap_or_default();
    format!("{}#{}x{}", base, gif.width, gif.height)
}

/// When a message body is just a Giphy media link, the image URL and the size
/// from its fragment (if any). Anything else is `None`, so a client never
/// fetches a URL a message could make up.
pub fn gif_body(body: String) -> Option<(String, Option<(u32, u32)>)> {
    let body = body.trim();
    if body.contains(char::is_whitespace) {
        return None
    }
    let Some(rest) = body.strip_prefix("https://") else { return None };
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    let giphy = host == "i.giphy.com"
        || host == "media.giphy.com"
        || (
            host.starts_with("media")
                && host.ends_with(".giphy.com")
                && host.len() >= 16
                && host[5..host.len() - 10].chars().all(|c| c.is_ascii_digit())
        );
    if !giphy {
        return None
    }
    let (path, frag) = match path.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (path, None),
    };
    let file = path.split('?').next().unwrap_or_default();
    if !(file.ends_with(".gif") || file.ends_with(".webp")) {
        return None
    }
    let size = frag.and_then(
        move |f|
        {
            let Some((w, h)) = f.split_once('x') else { return None };
            Some((w.parse().ok()?, h.parse().ok()?))
        }
    );
    Some((format!("https://{}/{}", host, file), size))
}

/// The key from an environment value or a file path, resolved. Kept pure so
/// the not-configured path is testable without touching the process
/// environment.
pub fn resolve_key(from_env: Option<String>, from_file: Option<String>) -> Option<String> {
    match from_env {
        Some(k) if !k.trim().is_empty() => Some(k.trim().into()),
        _ => match from_file {
            Some(p) => match std::fs::read_to_string(p).ok() {
                Some(k) if !k.trim().is_empty() => Some(k.trim().into()),
                _ => None,
            },
            None => None,
        },
    }
}

#[cfg(feature = "server")]
mod body {
    use reqwest::{Client, ClientBuilder, Response};
    use serde_json::Value;
    use std::time::Duration;
    use std::env::var;

    use crate::{Answer, ApiError};
    use crate::srv::fail;
    use super::{GifPage, parse_giphy, resolve_key, PAGE_SIZE};

    /// Cap on the upstream answer we will even look at.
    pub const MAX_RESPONSE_BYTES: usize = 512 * 1024;

    /// How long the upstream call may take.
    pub const TIMEOUT_SECS: u64 = 10u64;

    /// The refusal when no upstream is configured: a fixed sentence that
    /// never carries the key.
    pub const NOT_CONFIGURED: &str = "GIF search isn't set up on this server.";

    /// Giphy's API, unless `CHAT_GIPHY_BASE` says otherwise (tests point it at
    /// a local stub).
    pub fn base_url() -> String {
        match var("CHAT_GIPHY_BASE").ok().map(move |b| b.trim().to_string()).filter(move |b| !b.is_empty()) {
            Some(b) => b.trim_end_matches('/').to_string(),
            None => "https://api.giphy.com/v1/gifs".to_string(),
        }
    }

    /// The Giphy key, from `CHAT_GIPHY_KEY` or the file `CHAT_GIPHY_KEY_FILE`.
    pub fn api_key() -> Option<String> {
        resolve_key(var("CHAT_GIPHY_KEY").ok(), var("CHAT_GIPHY_KEY_FILE").ok())
    }

    /// Ask the upstream for one page. Bounded by timeout and size cap, and
    /// redirects are refused outright: a hop to another host would be an
    /// unchecked fetch. Every failure resolves to a fixed sentence — never an
    /// upstream body, never the request — so the key cannot leak.
    pub async fn fetch_page(endpoint: String, query: String, offset: u32, key: String) -> Result<Value, String> {
        let url = format!("{}/{}", base_url(), endpoint);
        let http: Client = match ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .build() {
            Ok(c) => c,
            Err(_) => return Err("Couldn't reach the GIF search service.".to_string()),
        };
        let mut params: Vec<(String, String)> = [
            ("api_key".into(), key),
            ("limit".into(), PAGE_SIZE.to_string()),
            ("offset".into(), offset.to_string()),
            ("rating".into(), "pg-13".into()),
        ].to_vec();
        if !query.is_empty() {
            params.push(("q".into(), query))
        }
        let r: Response = match http.get(url).query(&params).send().await {
            Ok(r) => r,
            Err(_) => return Err("Couldn't reach the GIF search service.".to_string()),
        };
        if !r.status().is_success() {
            let busy = r.status() == 429;
            let why = if busy { "The GIF search service is busy; try again later." } else { "The GIF search service refused that search." };
            return Err(why.to_string());
        }
        match r.bytes().await {
            Ok(bytes) if bytes.len() > MAX_RESPONSE_BYTES => Err("The GIF search service sent too much.".to_string()),
            Ok(bytes) => match serde_json::from_slice::<Value>(bytes.as_ref()) {
                Ok(v) => Ok(v),
                Err(_) => Err("The GIF search service sent a bad answer.".to_string()),
            },
            Err(_) => Err("The GIF search service sent a bad answer.".to_string()),
        }
    }

    /// One page of search or trending GIFs: the upstream call, then the
    /// allow-list filter inside [`parse_giphy`].
    pub async fn page(endpoint: String, query: String, offset: u32) -> Answer<GifPage> {
        let Some(key) = api_key() else {
            return Err(fail(ApiError::Unavailable { message: NOT_CONFIGURED.into() }));
        };
        match fetch_page(endpoint, query, offset, key).await {
            Ok(json) => Ok(parse_giphy(json)),
            Err(why) => Err(fail(ApiError::Unavailable { message: why.into() })),
        }
    }
}

/// Ask for one page of GIFs, on the server so the browser never sees the key.
/// An empty `query` means trending. Needs only a signed-in account: the search
/// is not space-scoped. Rate limit 10/min, keyed by account.
#[server(tags(limit = "10/min"))]
pub async fn gif_search(query: String, offset: u32, user: Auth<Principal>) -> Answer<GifPage> {
    use crate::srv::fail;
    use crate::ApiError;

    let _ = user;
    let query = query.trim().to_string();
    if !query_ok(query.clone()) {
        return Err(fail(ApiError::invalid("query", "That search is too long.")));
    }
    if !offset_ok(offset) {
        return Err(fail(ApiError::invalid("offset", "That page is too far along.")));
    }
    let endpoint = if query.is_empty() { "trending".to_string() } else { "search".to_string() };
    body::page(endpoint, query, offset).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JSON fixture as a Value, from source.
    fn v(source: String) -> serde_json::Value {
        match serde_json::from_str(&source) {
            Ok(x) => x,
            Err(e) => panic!("a fixture that does not parse: {}", e.to_string()),
        }
    }

    #[test]
    fn the_query_and_offset_limits_are_checked() {
        assert!(query_ok("a".repeat(QUERY_MAX_CHARS).into()), "exactly 50 characters are allowed");
        assert!(!query_ok("a".repeat(QUERY_MAX_CHARS + 1).into()), "51 characters are refused");
        assert!(query_ok(" ".repeat(QUERY_MAX_CHARS + 1).into()), "trimming first is what counts");
        assert!(offset_ok(0u32), "the first page is allowed");
        assert!(offset_ok(OFFSET_MAX), "the furthest page is allowed");
        assert!(!offset_ok(OFFSET_MAX + 1), "one past it is refused");
    }

    #[test]
    fn a_giphy_number_parses_whether_it_is_a_string_or_a_number() {
        assert_eq!(num(&v("null".into())), 0u32, "null is 0");
        assert_eq!(num(&v("200".into())), 200u32, "a number parses");
        assert_eq!(num(&v("\"42\"".into())), 42u32, "a numeric string parses");
        assert_eq!(num(&v("\" 7 \"".into())), 7u32, "surrounding blanks are trimmed");
        assert_eq!(num(&v("\"cat\"".into())), 0u32, "a word is 0");
        assert_eq!(num(&v("true".into())), 0u32, "a bool is 0");
    }

    #[test]
    fn a_rendition_needs_an_https_url_and_real_sides() {
        let images = v("{\"fixed_width\":{\"url\":\"https://i.giphy.com/p.gif\",\"width\":200,\"height\":0},\"downsized\":{\"url\":\"http://i.giphy.com/b.gif\",\"width\":480,\"height\":270},\"nameless_size\":{\"url\":\"https://i.giphy.com/c.gif\"}}".into());
        assert_eq!(rendition(images.clone(), "fixed_width".into()), None, "a zero side is refused");
        assert_eq!(rendition(images.clone(), "downsized".into()), None, "http is not https");
        assert_eq!(rendition(images.clone(), "nameless_size".into()), None, "no size is refused");
        assert_eq!(rendition(images.clone(), "missing".into()), None, "a missing rendition is none");
    }

    #[test]
    fn parse_giphy_skips_off_host_and_preview_less_results() {
        let page = parse_giphy(v("{\"data\":[{\"id\":\"ok\",\"title\":\"good\",\"images\":{\"fixed_width\":{\"url\":\"https://i.giphy.com/g-preview.gif\",\"width\":200,\"height\":200},\"downsized\":{\"url\":\"https://i.giphy.com/g-big.gif\",\"width\":480,\"height\":270}}},{\"id\":\"off\",\"images\":{\"fixed_width\":{\"url\":\"https://evil.example.com/trap.gif\",\"width\":200,\"height\":200}}},{\"id\":\"no-preview\",\"images\":{\"original\":{\"url\":\"https://media.giphy.com/only-o.gif\",\"width\":480,\"height\":270}}},{\"id\":\"small\",\"images\":{\"fixed_width\":{\"url\":\"https://media3.giphy.com/media/abc/s.gif\",\"width\":100,\"height\":100},\"original\":{\"url\":\"https://media3.giphy.com/media/abc/b.gif\",\"width\":500,\"height\":500}}}],\"pagination\":{\"offset\":0,\"count\":4,\"total_count\":10}}".into()));
        assert_eq!(page.gifs.len(), 2usize, "the off-host and preview-less results are skipped");
        assert_eq!(page.gifs[0].url, "https://i.giphy.com/g-big.gif", "downsized is the send rendition");
        assert_eq!(page.gifs[1].url, "https://media3.giphy.com/media/abc/b.gif", "original is the fallback");
        assert_eq!(page.gifs[1].preview_url, "https://media3.giphy.com/media/abc/s.gif", "the preview survives");
        assert_eq!(page.next_offset, Some(4u32), "offset+count is the next page while short of total");
    }

    #[test]
    fn a_shapeless_or_empty_answer_is_an_empty_page() {
        assert_eq!(parse_giphy(v("{\"data\":[]}".into())).gifs.len(), 0usize, "nothing came back");
        assert_eq!(parse_giphy(v("{\"data\":[]}".into())).next_offset, None, "and no next page");
        assert_eq!(parse_giphy(v("{}".into())).gifs.len(), 0usize, "no data array is empty too");
        assert_eq!(parse_giphy(v("{\"data\":\"not-a-list\"}".into())).gifs.len(), 0usize, "a wrong shape is empty, not a crash");
        assert_eq!(parse_giphy(v("{\"data\":[{\"id\":\"bare\"}],\"pagination\":{\"count\":9,\"total_count\":99}}".into())).gifs.len(), 0usize, "a result with no images is skipped");
    }

    #[test]
    fn the_last_page_stops_the_pager() {
        let page = parse_giphy(v("{\"data\":[{\"id\":\"last\",\"images\":{\"fixed_width\":{\"url\":\"https://i.giphy.com/l.gif\",\"width\":200,\"height\":200}}}],\"pagination\":{\"offset\":30,\"count\":1,\"total_count\":31}}".into()));
        assert_eq!(page.next_offset, None, "offset+count is total, so there is no next page");
    }

    #[test]
    fn a_gif_is_sent_as_a_plain_message_body_with_the_size_in_the_fragment() {
        let gif = Gif {
            id: "1".into(),
            title: "".into(),
            preview_url: "https://i.giphy.com/small.gif".into(),
            preview_width: 200u32,
            preview_height: 200u32,
            url: "https://media.giphy.com/big.gif#480x270".into(),
            width: 480u32,
            height: 270u32,
        };
        let body = gif_message(gif);
        assert_eq!(body, "https://media.giphy.com/big.gif#480x270", "the fragment carries the size");
        assert!(media_host_ok(body), "so the body passes the message allow-list");
    }

    #[test]
    fn gif_body_reads_only_bare_giphy_media_links() {
        let ok = gif_body("https://i.giphy.com/a.gif#480x270".into());
        assert!(ok.is_some(), "the plain media host is read");
        let (url, size) = ok.unwrap();
        assert_eq!(url, "https://i.giphy.com/a.gif", "the fragment is stripped from the url");
        assert_eq!(size, Some((480u32, 270u32)), "the size comes from the fragment");
        let (preview_url, preview_size) = gif_body("  https://media3.giphy.com/media/abc123/b.gif  ".into()).unwrap();
        assert_eq!(preview_url, "https://media3.giphy.com/media/abc123/b.gif", "blanks are trimmed");
        assert!(preview_size.is_none(), "no fragment means no size");
        let (query_url, query_size) = gif_body("https://i.giphy.com/q.gif?x=1#100x100".into()).unwrap();
        assert_eq!(query_url, "https://i.giphy.com/q.gif", "query text is cut before the fragment");
        assert_eq!(query_size, Some((100u32, 100u32)), "the size survives the cut");
        assert!(gif_body("https://media.giphy.com/a.webp".into()).is_some(), "webp is a fine media type");
        assert!(gif_body("https://i.giphy.com/a.gif look here".into()).is_none(), "a body with more than a link is not a gif");
        assert!(gif_body("https://evil.example.com/a.gif".into()).is_none(), "other hosts are refused");
        assert!(gif_body("http://i.giphy.com/a.gif".into()).is_none(), "http is refused");
        assert!(gif_body("https://mediaX.giphy.com/media/abc/a.gif".into()).is_none(), "the numbered host must be all digits");
        assert!(gif_body("https://media.giphy.com/notanimage".into()).is_none(), "not a gif or webp file");
        assert!(gif_body("https://media0123456789.giphy.com/media/x/a.gif".into()).is_some(), "a long numeric media host is allowed");
        assert!(gif_body("https://media.giphy.com/a#480x270".into()).is_none(), "a fragment but no real file name is refused");
    }

    #[test]
    fn the_key_resolves_from_env_or_file_with_blanks_as_none() {
        assert_eq!(resolve_key(Some(" dcrt ".into()), None).unwrap(), "dcrt", "the env value is trimmed");
        assert!(resolve_key(Some(" ".into()), None).is_none(), "a blank env with no file is not configured");
        assert!(resolve_key(Some("".into()), None).is_none(), "an empty env with no file is not configured");
        assert!(resolve_key(None, None).is_none(), "nothing configured is none");
        let path = "/tmp/roles-docs-giphy-key-test";
        match std::fs::write(&path, " file_key \n") {
            Ok(_) => {
                let from_file = resolve_key(Some(" ".into()), Some(path.into())).unwrap();
                assert_eq!(from_file, "file_key", "a blank env falls through to the file contents, trimmed");
                let from_env = resolve_key(Some(" env_key ".into()), Some(path.into())).unwrap();
                assert_eq!(from_env, "env_key", "the env value wins over the file");
            }
            Err(e) => panic!("could not set up the file fixture: {}", e.to_string()),
        }
    }
}
