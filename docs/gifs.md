# GIF search: a server-side proxy for Giphy

The model as it stands in the final code on the `roles-docs` branch. Every
fact is re-derived from that code; file and function names come with each
claim.

## Overview

The browser never talks to Giphy's API. [`gif_search`] runs the search on the
server, where the key lives in the environment; with no key configured the
verb answers a typed "not configured" refusal. Each GIF comes back with a
small rendition for the picker grid and a bigger one to send, and the media
URL carries its pixel size in the fragment (`…/giphy.gif#480x270`). An empty
`query` means Giphy's trending GIFs. Results are filtered to Giphy's media
hosts ([`media_host_ok`], the same allow-list a message body already accepts),
so a search's URLs are safe to put in a message: a GIF is sent as an ordinary
message whose body is the media URL — no new message type — and rendering is
the client's job.

The single source of the rules is `crates/chat-api/src/gifs.rs`. The rate
limit rides the existing server-kit limiter through the verb's `limit` tag.

## The model (`chat-api/src/gifs.rs`)

- `Gif` is one GIF: `id`, `title`, a `preview_url` with `preview_width`/
  `preview_height`, and a `url` with `width`/`height`. `GifPage` is one page:
  `gifs: Vec<Gif>` plus `next_offset: Option<u32>`, which is `None` on the last
  page. Both are `#[derive]`d `Serialize`/`Deserialize`/`PartialEq`/`Eq`, so
  they ride the socket as plain JSON.
- `gifs.rs` consts: `PAGE_SIZE` = 30 results per page; `QUERY_MAX_CHARS` = 50,
  past which [`query_ok`] refuses the search; `OFFSET_MAX` = 500, past which
  [`offset_ok`] refuses the page.
- [`query_ok`] measures `query.trim().chars().count()` (code points, not bytes),
  so a 50-codepoint search of any script passes. [`offset_ok`] is just
  `offset <= OFFSET_MAX`.
- [`media_host_ok`] delegates to [`gif_body`]: a URL is a Giphy media host when
  [`gif_body`] parses it. That is the same allow-list a message body already
  accepts, which is why a search's URLs are safe to drop into a message.

## Parsing the upstream answer (`num`, `rendition`, `parse_giphy`)

- [`num`] reads a Giphy value as a `u32` whether it came back as a string
  (`"200"`) or a number, and yields `0` when it is neither. Giphy returns
  dimensions inconsistently, so both shapes are handled.
- [`rendition`] pulls the `(url, width, height)` of one named rendition out of
  an `images` map, or `None` when the rendition is absent, its `url` is missing
  or not `https://`, or either side is `0`.
- [`parse_giphy`] walks the `data` array. A GIF with no `images` map or no
  usable `fixed_width` preview is skipped, and either rendition that fails the
  [`media_host_ok`] allow-list is skipped — so a search's `gifs` are only ever
  Giphy media URLs. The `url` is the `downsized` rendition when present, else
  the `original`, else the preview. `pagination` (`offset`, `count`,
  `total_count`, each through [`num`]) drives `next_offset`: it is `offset +
  count` only while `count > 0` and that sum is under `total_count`, else
  `None`. An empty or malformed `data` decodes to an empty page, never a panic.

## Sending a GIF (`gif_message`, `gif_body`)

- [`gif_message`] turns a `Gif` into the message body a client sends: the `url`
  with the current size re-stamped in the fragment, `…#width x height`. The
  size on the fragment is the truth for the client; it strips any old fragment
  first.
- [`gif_body`] is the inverse: when a message body is *just* a Giphy media link
  — no whitespace, `https://`, host on the allow-list (`i.giphy.com`,
  `media.giphy.com`, or `mediaNNNNN.giphy.com`), and the path a `.gif`/`.webp`
  — it returns the image URL (query text stripped) and the size parsed from its
  fragment. Anything else is `None`, so a client never fetches a URL a message
  body could make up. The returned URL is the clean `…/path.gif` even when the
  input carried `?query`: parsing happens on the query-stripped `file`, and the
  rebuilt URL is built from that `file`, so a search's output round-trips.

## The server side (`gifs.rs::body`, `#[server]`)

- The key lives in the environment only: [`resolve_key`] takes the value of
  `CHAT_GIPHY_KEY` and the path named by `CHAT_GIPHY_KEY_FILE`, trims, and
  prefers the value; an empty value falls through to the file (read with
  `std::fs::read_to_string`, trimmed), and if neither yields a non-blank key the
  answer is `None`. It is kept pure (env/file passed in, not read) so the
  not-configured path is testable without touching the process environment.
- The whole network layer is gated `#[cfg(feature = "server")]` in a `body`
  module, so the parsing above compiles (and unit-tests) without `reqwest`.
- [`base_url`] is `CHAT_GIPHY_BASE` trimmed of a trailing `/` when set (tests
  point it at a local stub), else Giphy's `https://api.giphy.com/v1/gifs`.
- [`fetch_page`] builds one `ClientBuilder` with redirects refused outright
  (`Policy::none()`, since a hop to another host would be an unchecked fetch)
  and a [`TIMEOUT_SECS`] = 10 s timeout. It appends `api_key`, `limit`
  (= `PAGE_SIZE`), `offset`, and `rating` = `pg-13`; it adds `q` only when the
  query is non-empty, so an empty query is the trending endpoint, not a
  blank-keyword search. A non-2xx resolves to a fixed sentence — `429` is the
  "busy; try again later" one, everything else the "refused that search" one —
  never the upstream body and never the request. The answer is read as bytes,
  refused past [`MAX_RESPONSE_BYTES`] = 512 KiB, then parsed with
  `serde_json::from_slice`. Any failure (transport, oversize, bad JSON) is one
  of those fixed sentences, so the key cannot leak into a response, an event,
  or a log line.
- [`page`] is the seam: no key, it returns `ApiError::Unavailable` with
  [`NOT_CONFIGURED`] ("GIF search isn't set up on this server."); otherwise the
  [`fetch_page`] result is handed to [`parse_giphy`]. [`ApiError::Unavailable`]
  is the existing typed error for "this feature can't run right now"; its
  message is shown to the user verbatim, so these refusals are plain
  sentences.

## The verb (`gif_search`)

`gif_search(query, offset, user: Auth<Principal>)` is `#[server(tags(limit =
"10/min"))]` — rate-limited per account, so hammering search returns a `Slow`
with a retry time and a dev-tier key stays inside its 100 calls an hour. It
needs only a signed-in account (the search is not space-scoped), so the
`user` argument is the auth requirement, not data the body reads. It trims the
query, refuses an over-long query and a too-far offset with typed invalid
[`ApiError`]s, picks `trending` for an empty query else `search`, and calls
`body::page`. The `query` handed downstream is already trimmed, and the
allow-list filter and the "empty means trending" choice are the only two places
the query's emptiness matters.

## Client rendering

A GIF is an ordinary message: the picker grid shows the `preview_url` renditions
and sending posts [`gif_message`]'s body. A client reading such a message body
calls [`gif_body`] to decide the image to fetch and its size; because the body
is already allow-listed by [`media_host_ok`] upstream, [`gif_body`] never
returns a URL the server did not put there.

## Rate limit and quota

The limit tag is `10/min` keyed by account (server-kit's limiter, the key by
which a `Slow` carries a retry time). Giphy's free dev-tier key is 100 calls
an hour, so at 10/min per account the aggregate stays inside that for a
handful of simultaneous searchers; the upstream itself answers `429`, which
[`fetch_page`] surfaces as the "busy; try again later" sentence rather than
failing the search outright.

## Tests (`gifs.rs` `mod tests`, 14)

The unit tests pin the pure logic — [`num`] both shapes, [`rendition`] missing/
zero-sided, [`parse_giphy`] on real-shaped fixtures (pagination math for
`next_offset`, a host outside the allow-list skipped, a bad `data` decoding to
an empty page), [`gif_message`]/[`gif_body`] round-tripping (including a
`?query` in the input body stripping to a clean URL), [`resolve_key`] env-then-
file precedence, and [`query_ok`]/[`offset_ok`] at their exact boundaries —
without any network. [`fetch_page`]'s timeout/size/refusal branches and the
key-never-rides-out rule are checked at the seams that keep the key and the
upstream body off the answer (a fixed-sentence `Err`, and the typed
`NOT_CONFIGURED` refusal), not by calling a live Giphy.
