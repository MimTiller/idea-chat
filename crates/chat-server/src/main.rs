//! Idea Chat's server.
//!
//! One process serves everything a client talks to:
//!
//! - the API (`/_srv/*`) and the live connections (`/_srv/_ws/live`);
//! - uploaded files (`/f/<key>`, a redirect to a signed storage URL);
//! - the web app (`/`, from the built `dist/web`);
//! - the call relay (QUIC, on its own UDP port) — calls are end-to-end
//!   encrypted, so the relay forwards media it cannot read.
//!
//! Everything is configured from the environment; see [`Config`]. With no
//! environment at all it runs against `deploy/chat/docker-compose.yml`'s
//! services on localhost and prints emails instead of sending them. GIF
//! search reads its own `CHAT_GIPHY_KEY` / `CHAT_GIPHY_KEY_FILE` (see
//! `chat_api`'s `gifs` module); with neither set, search is not configured.

mod mail;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Path, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use chat_api::srv::{App, Bus, Calls, Files};
use chat_api::Principal;
use tower_http::services::{ServeDir, ServeFile};

// Referencing the API crate keeps the linker from dropping its route
// registrations, which are only reached through `inventory`.
use chat_api::auth::whoami as _force_link;

/// Configuration, from the environment.
struct Config {
    /// `CHAT_BIND`: where to listen for HTTP.
    bind: SocketAddr,
    /// `CHAT_PUBLIC_URL`: where people reach the app.
    public_url: String,
    /// `CHAT_WEB_DIST` (or `WEB_DIST`): the built web app.
    web_dist: PathBuf,
    /// `DATABASE_URL`.
    database_url: String,
    /// `REDIS_URL`: sessions, rate limits and cross-instance fan-out. Unset:
    /// all three in memory, for a single instance.
    redis_url: Option<String>,
    /// `CHAT_MAILER`: `log` (default) or `ses`.
    mailer: mail::Delivery,
    /// `CHAT_MAIL_FROM`.
    mail_from: String,
    /// `S3_ENDPOINT` (unset: AWS), `S3_PUBLIC_ENDPOINT` (how clients reach
    /// it, if not the same), `S3_BUCKET`, `S3_REGION`.
    s3_endpoint: Option<String>,
    s3_public_endpoint: Option<String>,
    s3_bucket: String,
    s3_region: String,
    /// `CALL_RELAY_BIND`, `CALL_RELAY_PUBLIC_URL`, `CALL_RELAY_HOSTS` (names
    /// for a self-signed certificate), `CALL_RELAY_CERT` / `CALL_RELAY_KEY`
    /// (PEM files for a CA-signed one).
    relay_bind: SocketAddr,
    relay_public_url: String,
    relay_hosts: Vec<String>,
    relay_cert: Option<(PathBuf, PathBuf)>,
    /// `CALL_SECRET`: shared with the relay; signs call grants.
    call_secret: Vec<u8>,
    /// `LIVE_SECRET`: signs live-connection tickets.
    live_secret: Vec<u8>,
    /// `CHAT_TRUST_PROXY=1`: a reverse proxy in front sets X-Forwarded-For.
    /// Otherwise the server sets it from the peer address, overwriting what
    /// a client sent, so rate limits key on the real client.
    trust_proxy: bool,
    /// `CHAT_INSTANCE`: this instance's name.
    instance: String,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// A secret from the environment, or — in development only — a random one
/// for this run, said out loud.
fn secret(name: &str) -> Vec<u8> {
    match env(name) {
        Some(s) if s.len() >= 32 => s.into_bytes(),
        Some(_) => panic!("{name} must be at least 32 characters"),
        None => {
            eprintln!("[chat] {name} is not set; using a random one for this run (sessions and calls won't survive a restart)");
            let bytes: [u8; 32] = rand::random();
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, bytes).into_bytes()
        }
    }
}

impl Config {
    fn from_env() -> Config {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Config {
            bind: env("CHAT_BIND").unwrap_or_else(|| "127.0.0.1:8090".into()).parse().expect("CHAT_BIND is host:port"),
            public_url: env("CHAT_PUBLIC_URL").unwrap_or_else(|| "http://localhost:8090".into()).trim_end_matches('/').to_string(),
            web_dist: env("CHAT_WEB_DIST")
                .or_else(|| env("WEB_DIST"))
                .map(PathBuf::from)
                .unwrap_or_else(|| crate_dir.join("../chat/dist/web")),
            database_url: env("DATABASE_URL").unwrap_or_else(|| "postgres://postgres:dev@localhost:55433/chat".into()),
            redis_url: env("REDIS_URL"),
            mailer: match env("CHAT_MAILER").as_deref() {
                Some("ses") => mail::Delivery::Send,
                Some("log") | None => mail::Delivery::Log,
                Some(other) => panic!("CHAT_MAILER is `log` or `ses`, not `{other}`"),
            },
            mail_from: env("CHAT_MAIL_FROM").unwrap_or_else(|| "Idea Chat <no-reply@localhost>".into()),
            s3_endpoint: env("S3_ENDPOINT").or_else(|| Some("http://localhost:59000".into())).filter(|e| e != "aws"),
            s3_public_endpoint: env("S3_PUBLIC_ENDPOINT"),
            s3_bucket: env("S3_BUCKET").unwrap_or_else(|| "idea-chat".into()),
            s3_region: env("S3_REGION").unwrap_or_else(|| "us-east-1".into()),
            relay_bind: env("CALL_RELAY_BIND").unwrap_or_else(|| "0.0.0.0:8798".into()).parse().expect("CALL_RELAY_BIND is host:port"),
            relay_public_url: env("CALL_RELAY_PUBLIC_URL").unwrap_or_else(|| "https://localhost:8798/".into()),
            relay_hosts: env("CALL_RELAY_HOSTS")
                .unwrap_or_else(|| "localhost,127.0.0.1".into())
                .split(',')
                .map(|h| h.trim().to_string())
                .filter(|h| !h.is_empty())
                .collect(),
            relay_cert: env("CALL_RELAY_CERT").zip(env("CALL_RELAY_KEY")).map(|(c, k)| (c.into(), k.into())),
            call_secret: secret("CALL_SECRET"),
            live_secret: secret("LIVE_SECRET"),
            trust_proxy: env("CHAT_TRUST_PROXY").as_deref() == Some("1"),
            instance: env("CHAT_INSTANCE").unwrap_or_else(|| format!("{}-{}", hostname(), std::process::id())),
        }
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "chat".into())
}

/// Parse `Name <addr>` or a bare address.
fn mailbox(raw: &str) -> email::Mailbox {
    match raw.rsplit_once('<') {
        Some((name, addr)) => email::Mailbox::named(name.trim(), addr.trim_end_matches('>').trim()),
        None => email::Mailbox::new(raw.trim()),
    }
}

async fn s3_client(region: &str, endpoint: Option<&str>) -> aws_sdk_s3::Client {
    let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .load()
        .await;
    let mut builder = aws_sdk_s3::config::Builder::from(&shared);
    if let Some(endpoint) = endpoint {
        // MinIO and most S3-compatible stores want path-style addressing.
        builder = builder.endpoint_url(endpoint).force_path_style(true);
    }
    aws_sdk_s3::Client::from_conf(builder.build())
}

/// Start the call relay in this process. Returns its certificate hash
/// (empty for a CA certificate) and a way to revoke grants at it.
fn start_relay(cfg: &Config) -> (String, Option<Arc<dyn Fn(String) + Send + Sync>>) {
    let identity = match &cfg.relay_cert {
        Some((cert, key)) => {
            let chain = std::fs::read(cert).expect("read CALL_RELAY_CERT");
            let key = std::fs::read(key).expect("read CALL_RELAY_KEY");
            datagram::Identity::from_pem(&chain, &key).expect("CALL_RELAY_CERT / CALL_RELAY_KEY are a PEM chain and key")
        }
        None => {
            let names: Vec<&str> = cfg.relay_hosts.iter().map(String::as_str).collect();
            datagram::Identity::self_signed(&names).expect("make a self-signed relay certificate")
        }
    };
    let hash = if cfg.relay_cert.is_some() { String::new() } else { identity.hash_hex() };
    match datagram::Listener::bind(cfg.relay_bind, &identity) {
        Ok(listener) => {
            let handle = call_relay::spawn(
                listener,
                call_relay::Config {
                    auth: Some(call_relay::Auth { secret: cfg.call_secret.clone(), max_ttl: Duration::from_secs(15 * 60) }),
                    throttle: true,
                    ..Default::default()
                },
            );
            println!("  relay  → {} (udp {})", cfg.relay_public_url, cfg.relay_bind);
            let handle = Arc::new(handle);
            let revoke: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |token: String| {
                let handle = handle.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle.revoke(&token).await {
                        eprintln!("[chat] relay revocation refused: {e:?}");
                    }
                });
            });
            (hash, Some(revoke))
        }
        Err(e) => {
            eprintln!("[chat] the call relay could not bind {}: {e:?} — calls will not connect", cfg.relay_bind);
            (hash, None)
        }
    }
}

/// `GET /f/<key>`: a file, by redirect to a signed URL. Images open inline;
/// everything else downloads.
async fn file(Path(key): Path<String>) -> Response {
    let a = chat_api::srv::app();
    match a.db.object_exists(&key).await {
        Ok(Some((name, content_type))) => {
            let inline = content_type.starts_with("image/") || content_type.starts_with("video/") || content_type.starts_with("audio/");
            match a.files.get_url(&key, &name, inline).await {
                Ok(url) => {
                    let mut r = Redirect::temporary(&url).into_response();
                    // The signed URL lives an hour; let clients reuse the redirect for most of it.
                    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3000"));
                    r
                }
                Err(e) => {
                    eprintln!("[chat] sign {key}: {e}");
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
            }
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

/// Set X-Forwarded-For to the peer, replacing whatever the client claimed.
async fn real_client(ConnectInfo(peer): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    if let Ok(v) = HeaderValue::from_str(&peer.ip().to_string()) {
        req.headers_mut().insert("x-forwarded-for", v);
    }
    next.run(req).await
}

/// Background work every instance does: keep its connections' presence
/// alive, sweep connections (and calls) whose instance died, and sweep
/// uploads nobody attached.
fn spawn_upkeep(instance: String) {
    tokio::spawn(async move {
        let mut beat = tokio::time::interval(Duration::from_secs(20));
        let mut ticks: u64 = 0;
        loop {
            beat.tick().await;
            ticks += 1;
            let a = chat_api::srv::app();
            if let Err(e) = a.db.heartbeat(&instance).await {
                eprintln!("[chat] heartbeat: {e}");
            }
            if let Ok((users, left)) = a.db.sweep_connections().await {
                for l in left {
                    chat_api::calls::announce_left(l).await;
                }
                for u in users {
                    chat_api::srv::announce_presence(u).await;
                }
            }
            // Hourly.
            if ticks % 180 == 0 {
                if let Ok(keys) = a.db.sweep_uploads().await {
                    for k in keys {
                        a.files.delete(&k).await;
                    }
                }
            }
        }
    });
}

#[tokio::main]
async fn main() {
    let _ = _force_link;
    let cfg = Config::from_env();
    println!("Idea Chat");

    let db = chat_db::Db::connect(&cfg.database_url).await.expect("connect to Postgres and migrate");

    // Sessions, rate limits and fan-out: Redis when there is one. The
    // session guard runs first, so the rate limiter can key on the account.
    let sessions = match &cfg.redis_url {
        Some(url) => {
            let client = redis::Client::open(url.as_str()).expect("REDIS_URL");
            server::install_state(client.clone());
            pubsub::configure(pubsub::RedisBackend::from_client(client.clone()).await.expect("connect to Redis for pub/sub"));
            let sessions = sessions_of(cache::RedisCache::new(client.clone()), &cfg);
            server_kit::install_middleware(sessions.guard::<Principal>());
            server_kit::install_middleware(
                server_kit::rate_limit().key_by(principal_key).store(server_kit::RedisLimitStore::new(client)),
            );
            sessions
        }
        None => {
            eprintln!("[chat] REDIS_URL is not set; sessions, rate limits and fan-out are in memory (one instance only)");
            pubsub::configure(pubsub::MemoryBackend::new());
            let sessions = sessions_of(cache::MemoryCache::new(), &cfg);
            server_kit::install_middleware(sessions.guard::<Principal>());
            server_kit::install_middleware(server_kit::rate_limit().key_by(principal_key).store(server_kit::MemoryLimitStore::default()));
            sessions
        }
    };
    // Browsers send Origin on every cross-site write; native clients send none.
    server_kit::install_middleware(server_kit::csrf_guard([cfg.public_url.clone()]));

    if cfg.mailer == mail::Delivery::Send {
        email::configure(email::SesProvider::from_env().await);
    }
    let from = mailbox(&cfg.mail_from);

    let files = Files {
        client: s3_client(&cfg.s3_region, cfg.s3_endpoint.as_deref()).await,
        public_client: s3_client(&cfg.s3_region, cfg.s3_public_endpoint.as_deref().or(cfg.s3_endpoint.as_deref())).await,
        bucket: cfg.s3_bucket.clone(),
    };
    if let Err(e) = files.ensure_bucket().await {
        eprintln!("[chat] storage: {e} — uploads will fail until it is reachable");
    }

    let (cert_hash, revoke) = start_relay(&cfg);

    // A clean start: connections this instance had before are gone.
    let stale = db.clear_instance(&cfg.instance).await.unwrap_or_default();

    chat_api::srv::install(App {
        db,
        sessions,
        mail: Arc::new(mail::Mailer { delivery: cfg.mailer, from }),
        files,
        calls: Calls { secret: cfg.call_secret.clone(), relay_url: cfg.relay_public_url.clone(), cert_hash, revoke },
        bus: Bus::start(),
        live_secret: cfg.live_secret.clone(),
        public_url: cfg.public_url.clone(),
        instance: cfg.instance.clone(),
    });
    for l in stale {
        chat_api::calls::announce_left(l).await;
    }
    spawn_upkeep(cfg.instance.clone());

    if !cfg.web_dist.join("index.html").exists() {
        eprintln!(
            "[chat] no web app at {} — build it with `idealyst build --web crates/chat`",
            cfg.web_dist.display()
        );
    }
    let index = cfg.web_dist.join("index.html");
    let mut app: Router = server::router()
        .route("/f/*key", get(file))
        .fallback_service(ServeDir::new(&cfg.web_dist).fallback(ServeFile::new(index)));
    if !cfg.trust_proxy {
        app = app.layer(axum::middleware::from_fn(real_client));
    }

    println!("  app    → {}", cfg.public_url);
    println!("  mail   → {}", if cfg.mailer == mail::Delivery::Log { "printed here" } else { "SES" });
    let listener = tokio::net::TcpListener::bind(cfg.bind).await.expect("bind CHAT_BIND");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("serve");
}

/// Rate-limit by account when signed in, else by client address.
fn principal_key(ctx: &server::Context) -> Option<String> {
    ctx.get::<Principal>().map(|p| format!("user:{}", p.user))
}

fn sessions_of(store: impl cache::Cache + 'static, cfg: &Config) -> server_kit::Sessions {
    let secure = cfg.public_url.starts_with("https://");
    server_kit::Sessions::new(store)
        .cookie_name("idea_chat_session")
        .cookie(move |c| c.secure(secure))
        .ttl(Duration::from_secs(60 * 60 * 24 * 30))
        .sliding()
}
