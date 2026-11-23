//! Native raster tile cache. This never receives photo paths or precise EXIF data.
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    borrow::Cow,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use wry::{
    RequestAsyncResponder,
    http::{HeaderMap, Response},
};

pub(crate) const MAX_TILE: usize = 512 * 1024;
const DEFAULT_TTL: i64 = 7 * 86_400;
const MAX_CACHE: i64 = 96 * 1024 * 1024;
const MAX_ROWS: i64 = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tile {
    pub(crate) epoch: u32,
    z: u32,
    x: u32,
    y: u32,
}

impl Tile {
    pub(crate) fn parse(path: &str) -> Option<Self> {
        let mut parts = path.strip_prefix("osm/")?.strip_suffix(".png")?.split('/');
        let mut num = || {
            let s = parts.next()?;
            (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse::<u32>().ok())
                .flatten()
        };
        let (epoch, z, x, y) = (num()?, num()?, num()?, num()?);
        if epoch == 0 || z > 19 || x >= 1 << z || y >= 1 << z || parts.next().is_some() {
            return None;
        }
        Some(Self { epoch, z, x, y })
    }

    fn url(self) -> String {
        format!(
            "https://tile.openstreetmap.org/{}/{}/{}.png",
            self.z, self.x, self.y
        )
    }

    fn key(self) -> String {
        format!("{}/{}/{}", self.z, self.x, self.y)
    }
}

#[derive(Default)]
pub(crate) struct Gate {
    pub(crate) online: AtomicBool,
    pub(crate) visible: AtomicBool,
    pub(crate) epoch: AtomicU32,
}

impl Gate {
    fn allows(&self, tile: Tile) -> bool {
        self.online.load(Ordering::Acquire)
            && self.visible.load(Ordering::Acquire)
            && self.epoch.load(Ordering::Acquire) == tile.epoch
    }
}

struct Job(Tile, RequestAsyncResponder);

pub(crate) struct TileService {
    pub(crate) gate: Arc<Gate>,
    jobs: Mutex<Option<SyncSender<Job>>>,
}

fn reply(responder: RequestAsyncResponder, bytes: Option<Vec<u8>>) {
    let (status, content, mime) = match bytes {
        Some(png) => (200, Cow::Owned(png), "image/png"),
        None => (
            404,
            Cow::Borrowed(b"Tile unavailable".as_slice()),
            "text/plain",
        ),
    };
    responder.respond(
        Response::builder()
            .status(status)
            .header("Content-Type", mime)
            .header("X-Content-Type-Options", "nosniff")
            .header("Cache-Control", "no-store")
            .body(content)
            .expect("fixed response"),
    );
}

impl TileService {
    pub(crate) fn new() -> Self {
        Self {
            gate: Arc::new(Gate::default()),
            jobs: Mutex::new(None),
        }
    }

    pub(crate) fn submit(&self, tile: Tile, responder: RequestAsyncResponder) {
        if !self.gate.allows(tile) {
            reply(responder, None);
            return;
        }
        let Ok(mut jobs) = self.jobs.lock() else {
            reply(responder, None);
            return;
        };
        if jobs.is_none() {
            let (sender, receiver) = mpsc::sync_channel::<Job>(24);
            let gate = self.gate.clone();
            if std::thread::Builder::new()
                .name("picsee-osm-tiles".into())
                .spawn(move || {
                    let mut store = Store::open();
                    while let Ok(Job(tile, responder)) = receiver.recv() {
                        let bytes = if gate.allows(tile) {
                            store.as_mut().and_then(|store| store.tile(tile, &gate))
                        } else {
                            None
                        };
                        reply(responder, if gate.allows(tile) { bytes } else { None });
                    }
                })
                .is_ok()
            {
                *jobs = Some(sender);
            }
        }
        if let Some(queue) = jobs.as_ref() {
            if let Err(
                TrySendError::Full(Job(_, responder))
                | TrySendError::Disconnected(Job(_, responder)),
            ) = queue.try_send(Job(tile, responder))
            {
                // No backlog growth or fallback requests outside the visible viewport.
                reply(responder, None);
            }
        } else {
            reply(responder, None);
        }
    }
}

#[derive(Default)]
struct CacheMeta {
    expiry: i64,
    ttl: i64,
    etag: Option<String>,
    modified: Option<String>,
    no_store: bool,
}

fn unix(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn cache_meta(headers: &HeaderMap, old: &CacheMeta, now: i64) -> CacheMeta {
    let head = |key| headers.get(key).and_then(|v| v.to_str().ok());
    let ttl = if old.expiry > 0 { old.ttl } else { DEFAULT_TTL };
    let mut meta = CacheMeta {
        expiry: now.saturating_add(ttl),
        ttl,
        etag: head("etag")
            .filter(|s| s.len() <= 512)
            .map(str::to_owned)
            .or_else(|| old.etag.clone()),
        modified: head("last-modified")
            .filter(|s| s.len() <= 100)
            .map(str::to_owned)
            .or_else(|| old.modified.clone()),
        no_store: false,
    };
    let age: i64 = head("age")
        .and_then(|age| age.parse().ok())
        .filter(|age| *age >= 0)
        .unwrap_or(0);
    let mut max_age = None;
    let mut no_cache = false;
    for value in headers.get_all("cache-control") {
        if let Ok(value) = value.to_str() {
            for directive in value.split(',').map(str::trim) {
                let (key, value) = directive.split_once('=').unwrap_or((directive, ""));
                match key.trim().to_ascii_lowercase().as_str() {
                    "max-age" => {
                        max_age = value
                            .trim()
                            .trim_matches('"')
                            .parse::<i64>()
                            .ok()
                            .filter(|n| *n >= 0)
                    }
                    "no-cache" => no_cache = true,
                    "no-store" => meta.no_store = true,
                    _ => {}
                }
            }
        }
    }
    let response_age = head("date")
        .and_then(|s| httpdate::parse_http_date(s).ok())
        .map_or(age, |time| age.max(now.saturating_sub(unix(time))));
    if no_cache {
        meta.ttl = 0;
        meta.expiry = now;
    } else if let Some(seconds) = max_age {
        meta.ttl = seconds;
        meta.expiry = now.saturating_add(seconds.saturating_sub(response_age).max(0));
    } else if let Some(expires) = head("expires").and_then(|s| httpdate::parse_http_date(s).ok()) {
        meta.expiry = unix(expires);
        meta.ttl = meta.expiry.saturating_sub(now).max(0);
    }
    meta
}

fn valid_tile_png(data: &[u8]) -> bool {
    data.len() <= MAX_TILE
        && data.len() >= 33
        && data.starts_with(b"\x89PNG\r\n\x1A\n\0\0\0\x0DIHDR")
        && data[16..24] == [0, 0, 1, 0, 0, 0, 1, 0]
}

struct Store {
    db: Connection,
    client: ureq::Agent,
    pause_until: i64,
}

impl Store {
    fn open() -> Option<Self> {
        let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("3wa_mypicsee");
        std::fs::create_dir_all(&dir).ok()?;
        let db = Connection::open(dir.join("osm-tiles.sqlite3")).ok()?;
        db.busy_timeout(Duration::from_secs(2)).ok()?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS tile (
               id TEXT PRIMARY KEY, expiry INTEGER NOT NULL, accessed INTEGER NOT NULL,
               ttl INTEGER NOT NULL, etag TEXT, modified TEXT, png BLOB NOT NULL
             ); CREATE INDEX IF NOT EXISTS tile_accessed ON tile(accessed);",
        )
        .ok()?;
        let cfg = ureq::Agent::config_builder()
            .https_only(true)
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(7)))
            .user_agent(format!(
                "3wa_mypicsee/{} (+https://github.com/shadowjohn/3wa_mypicsee)",
                crate::VERSION.trim()
            ))
            .build();
        Some(Self {
            db,
            client: cfg.into(),
            pause_until: 0,
        })
    }

    fn tile(&mut self, tile: Tile, gate: &Gate) -> Option<Vec<u8>> {
        let now = unix(SystemTime::now());
        let key = tile.key();
        let cached: Option<(CacheMeta, Vec<u8>)> = self
            .db
            .query_row(
                "SELECT expiry,ttl,etag,modified,png FROM tile WHERE id=?1",
                [&key],
                |row| {
                    Ok((
                        CacheMeta {
                            expiry: row.get(0)?,
                            ttl: row.get(1)?,
                            etag: row.get(2)?,
                            modified: row.get(3)?,
                            no_store: false,
                        },
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .optional()
            .ok()?
            .filter(|(_, png)| valid_tile_png(png));
        if let Some((meta, png)) = &cached
            && meta.expiry > now
        {
            let _ = self
                .db
                .execute("UPDATE tile SET accessed=?2 WHERE id=?1", params![key, now]);
            return Some(png.clone());
        }
        if now < self.pause_until || !gate.allows(tile) {
            return None;
        }
        let mut req = self.client.get(tile.url()).header("Accept", "image/png");
        if let Some((meta, _)) = &cached {
            if let Some(etag) = &meta.etag {
                req = req.header("If-None-Match", etag);
            }
            if let Some(modified) = &meta.modified {
                req = req.header("If-Modified-Since", modified);
            }
        }
        let mut res = match req.call() {
            Ok(res) => res,
            Err(_) => {
                self.pause_until = now.saturating_add(30);
                return None;
            }
        };
        let status = res.status().as_u16();
        if matches!(status, 403 | 429 | 500..=599) {
            let default_delay = if status == 403 { 300 } else { 60 };
            let retry = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok());
            self.pause_until = retry_deadline(retry, now, default_delay);
            return None;
        }
        let (meta, png) = if status == 304 {
            let (old, png) = cached?;
            (cache_meta(res.headers(), &old, now), png)
        } else if status == 200
            && res
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|s| {
                    s.split(';')
                        .next()
                        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("image/png"))
                })
        {
            let meta = cache_meta(res.headers(), &CacheMeta::default(), now);
            let png = res
                .body_mut()
                .with_config()
                .limit(MAX_TILE as u64)
                .read_to_vec()
                .ok()?;
            if !valid_tile_png(&png) {
                return None;
            }
            (meta, png)
        } else {
            return None;
        };
        if meta.no_store {
            let _ = self.db.execute("DELETE FROM tile WHERE id=?1", [&key]);
        } else {
            self.save(&key, &meta, &png, now).ok()?;
        }
        Some(png)
    }

    fn save(&mut self, key: &str, meta: &CacheMeta, data: &[u8], now: i64) -> rusqlite::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("INSERT OR REPLACE INTO tile(id,expiry,accessed,ttl,etag,modified,png) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![key,meta.expiry,now,meta.ttl,meta.etag,meta.modified,data])?;
        let (rows, bytes): (i64, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(png)),0) FROM tile",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let (mut rows, mut bytes) = (rows, bytes);
        while rows > MAX_ROWS || bytes > MAX_CACHE {
            let (old, length): (String, i64) = tx.query_row(
                "SELECT id,length(png) FROM tile ORDER BY accessed,id LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            tx.execute("DELETE FROM tile WHERE id=?1", [old])?;
            rows -= 1;
            bytes -= length;
        }
        tx.commit()
    }
}

fn retry_deadline(value: Option<&str>, now: i64, default_delay: i64) -> i64 {
    value
        .and_then(|s| {
            s.parse::<i64>()
                .ok()
                .filter(|n| *n >= 0)
                .map(|n| now.saturating_add(n))
                .or_else(|| httpdate::parse_http_date(s).ok().map(unix))
        })
        .unwrap_or_else(|| now.saturating_add(default_delay))
        .max(now.saturating_add(30))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_host_and_scoped_xyz() {
        let tile = Tile::parse("osm/7/3/2/5.png").unwrap();
        assert_eq!(tile.url(), "https://tile.openstreetmap.org/3/2/5.png");
        let gate = Gate::default();
        assert!(!gate.allows(tile));
        gate.online.store(true, Ordering::Release);
        gate.visible.store(true, Ordering::Release);
        gate.epoch.store(7, Ordering::Release);
        assert!(gate.allows(tile));
        gate.epoch.store(8, Ordering::Release);
        assert!(!gate.allows(tile));
        for value in [
            "osm/0/3/2/5.png",
            "osm/7/20/1/1.png",
            "osm/8/3/8/0.png",
            "osm/1/3/0/8.png",
            "osm/7/3/-1/5.png",
            "osm/7/3/2/5.png?redirect=google",
            "osm/7/3/2/5/4.png",
            "osm/7/3/2/../photo.png",
            "https://evil.test/osm/7/3/2/5.png",
        ] {
            assert_eq!(Tile::parse(value), None, "{value}");
        }
    }

    #[test]
    fn honors_expiry_store_and_revalidation() {
        let mut h = HeaderMap::new();
        h.insert("cache-control", "public, max-age=600".parse().unwrap());
        h.insert("age", "10".parse().unwrap());
        h.insert("etag", "\"fixture\"".parse().unwrap());
        let first = cache_meta(&h, &CacheMeta::default(), 100);
        assert_eq!(first.expiry, 690);
        let second = cache_meta(&HeaderMap::new(), &first, 700);
        assert_eq!(second.expiry, 1300);
        assert_eq!(second.etag.as_deref(), Some("\"fixture\""));
        h.insert("cache-control", "no-store, no-cache".parse().unwrap());
        let not_stored = cache_meta(&h, &first, 800);
        assert_eq!(not_stored.expiry, 800);
        assert!(not_stored.no_store);
        assert_eq!(
            cache_meta(&HeaderMap::new(), &CacheMeta::default(), 42).expiry,
            42 + DEFAULT_TTL
        );
        assert_eq!(retry_deadline(Some("155"), 6, 60), 161);
        assert_eq!(retry_deadline(None, 6, 60), 66);
    }
}
