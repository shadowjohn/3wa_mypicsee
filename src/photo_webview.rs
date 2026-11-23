use crate::{
    gps_index,
    osm_tiles::{Tile, TileService},
    photo_map_bridge::{self as bridge, Event, Resource},
};
use eframe::egui;
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    fmt::Write as _,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};
use wry::{NewWindowResponse, Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows, dpi};

const PAGE: &[u8] = include_bytes!("photo_map/map.html");
const CSS: &[u8] = include_bytes!("photo_map/map.css");
const ADAPTER: &[u8] = include_bytes!("photo_map/adapter.js");
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src data: 'self'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";
const PROTOCOL: &str = "picsee-map";
const HOME: &str = bridge::SCHEME_HOME;
const JS_LIMIT: u64 = 512 * 1024;

pub(crate) struct WebPhotoMap {
    webview: WebView,
    events: Receiver<Event>,
    ready: bool,
    started: Instant,
    key: Option<(u32, Option<usize>)>,
    geometry: Option<[i32; 4]>,
    tiles: Arc<TileService>,
    osm: bool,
}

fn asset(dir: &Path, name: &str, hash: &str) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    File::open(dir.join(name))
        .and_then(|file| file.take(JS_LIMIT + 1).read_to_end(&mut data))
        .map_err(|_| format!("缺少地圖元件 {name}，仍可使用內建離線點圖。"))?;
    if data.len() as u64 > JS_LIMIT || format!("{:x}", Sha256::digest(&data)) != hash {
        return Err(format!("地圖元件 {name} 與此版本不符。"));
    }
    Ok(data)
}

fn navigation_allowed(uri: &str) -> bool {
    uri == bridge::ORIGIN || uri == HOME
}

impl WebPhotoMap {
    pub(crate) fn new(frame: &eframe::Frame, ctx: &egui::Context) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| format!("找不到快看寶程式：{e}"))?;
        let folder = exe
            .parent()
            .ok_or("找不到快看寶的程式資料夾。")?
            .join("picsee-map-assets");
        let engine = asset(
            &folder,
            "easymap.js",
            "44e5fc2833d3f750802eee36916eda915c0f018ce8f810af8cb83add780603ae",
        )?;
        let points = asset(
            &folder,
            "dg_webgl_point.js",
            "1125dbd5e9ffb40df6b03c4da57cab6bc35c30629a04ae0b6d766523e5aa101f",
        )?;
        let repaint = ctx.clone();
        let (tx, events) = mpsc::sync_channel(8);
        let tiles = Arc::new(TileService::new());
        let route_tiles = tiles.clone();
        let ipc_gate = tiles.gate.clone();
        let webview = WebViewBuilder::new()
            .with_incognito(true)
            .with_focused(false)
            .with_visible(false)
            .with_devtools(false)
            .with_back_forward_navigation_gestures(false)
            .with_browser_accelerator_keys(false)
            .with_default_context_menus(false)
            .with_additional_browser_args("--disable-background-networking --disable-component-update --disable-sync --no-first-run")
            .with_navigation_handler(|uri| navigation_allowed(&uri))
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .with_ipc_handler(move |req| {
                if req.uri().to_string() != bridge::ORIGIN { return; }
                if req.body() == "osm-pause" {
                    ipc_gate.epoch.store(0, Ordering::Release);
                    return;
                }
                if let Some(value) = req.body().strip_prefix("osm-view:") {
                    if value.len() <= 10 && value.bytes().all(|b| b.is_ascii_digit())
                        && let Ok(epoch) = value.parse::<u32>()
                        && ipc_gate.online.load(Ordering::Acquire)
                    {
                        ipc_gate.epoch.store(epoch, Ordering::Release);
                    }
                    return;
                }
                if let Some(event) = bridge::parse(req.body()) && tx.try_send(event).is_ok() {
                    repaint.request_repaint();
                }
            })
            .with_asynchronous_custom_protocol(PROTOCOL.into(), move |_, request, responder| {
                use wry::http::Response;
                let uri = request.uri().to_string();
                if request.method().as_str() == "GET"
                    && let Some(path) = uri.strip_prefix(HOME).or_else(|| uri.strip_prefix(bridge::ORIGIN))
                    && let Some(tile) = Tile::parse(path)
                {
                    route_tiles.submit(tile, responder);
                    return;
                }
                let resource = bridge::resource(request.method().as_str(), &uri);
                let found: Option<(&str, Cow<'static, [u8]>)> = match resource {
                    Some(Resource::Page) => Some(("text/html; charset=utf-8", Cow::Borrowed(PAGE))),
                    Some(Resource::Css) => Some(("text/css; charset=utf-8", Cow::Borrowed(CSS))),
                    Some(Resource::Adapter) => Some(("text/javascript; charset=utf-8", Cow::Borrowed(ADAPTER))),
                    Some(Resource::Engine) => Some(("text/javascript; charset=utf-8", Cow::Owned(engine.clone()))),
                    Some(Resource::Points) => Some(("text/javascript; charset=utf-8", Cow::Owned(points.clone()))),
                    None => None,
                };
                let (status, mime, bytes) = match found {
                    Some((mime, bytes)) => (200, mime, bytes),
                    None => (404, "text/plain", Cow::Borrowed(b"Not found".as_slice())),
                };
                responder.respond(Response::builder()
                    .status(status)
                    .header("Content-Type", mime)
                    .header("Content-Security-Policy", CSP)
                    .header("Cache-Control", "no-store")
                    .header("X-Content-Type-Options", "nosniff")
                    .body(bytes)
                    .expect("constant local response headers are valid"));
            })
            .with_url(HOME)
            .build_as_child(frame)
            .map_err(|e| format!("無法啟動 WebView2：{e}"))?;
        Ok(Self {
            webview,
            events,
            ready: false,
            started: Instant::now(),
            key: None,
            geometry: None,
            tiles,
            osm: false,
        })
    }

    pub(crate) fn osm_enabled(&self) -> bool {
        self.osm
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.ready
    }

    pub(crate) fn set_osm(&mut self, enabled: bool) -> Result<(), String> {
        if !self.ready || self.osm == enabled {
            return Ok(());
        }
        self.tiles.gate.epoch.store(0, Ordering::Release);
        self.tiles.gate.online.store(enabled, Ordering::Release);
        let call = if enabled {
            "window.picseeMapOsm(true)"
        } else {
            "window.picseeMapOsm(false)"
        };
        if let Err(error) = self.webview.evaluate_script(call) {
            self.tiles.gate.online.store(false, Ordering::Release);
            return Err(error.to_string());
        }
        self.osm = enabled;
        Ok(())
    }

    pub(crate) fn place(&mut self, bounds: egui::Rect, ppp: f32) -> Result<(), String> {
        let next = [bounds.min.x, bounds.min.y, bounds.width(), bounds.height()]
            .map(|n| (n * ppp).round() as i32);
        if self.geometry == Some(next) {
            return Ok(());
        }
        let show = next[2] >= 2 && next[3] >= 2;
        self.tiles.gate.visible.store(show, Ordering::Release);
        if show {
            self.webview
                .set_bounds(Rect {
                    position: dpi::PhysicalPosition::new(next[0], next[1]).into(),
                    size: dpi::PhysicalSize::new(next[2], next[3]).into(),
                })
                .map_err(|e| e.to_string())?;
        }
        self.webview.set_visible(show).map_err(|e| e.to_string())?;
        self.geometry = Some(next);
        Ok(())
    }

    pub(crate) fn tick(
        &mut self,
        ctx: &egui::Context,
        generation: u32,
        locations: &[(PathBuf, gps_index::Point)],
        current: Option<usize>,
    ) -> Result<Option<Vec<usize>>, String> {
        let mut clicked = None;
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Ready => {
                    self.ready = true;
                    self.key = None;
                }
                Event::Failed => return Err("此 WebView2 環境不支援 WebGL。".into()),
                event if self.ready => {
                    if let Some(ids) = bridge::route(event, generation, locations.len()) {
                        clicked = Some(ids);
                    }
                }
                _ => {}
            }
        }
        if !self.ready {
            if self.started.elapsed() > Duration::from_secs(8) {
                return Err("互動地圖未能完成啟動。".into());
            }
            ctx.request_repaint_after(Duration::from_millis(200));
            return Ok(None);
        }
        let key = (generation, current);
        if self.key == Some(key) {
            return Ok(clicked);
        }
        let active = current
            .map(|id| id.to_string())
            .unwrap_or_else(|| "null".into());
        let script = if self.key.is_some_and(|(old, _)| old == generation) {
            format!("window.picseeMapSelect({generation},{active})")
        } else {
            // Only bounded integers and finite GPS numbers go into this fixed JS call.
            // No filename, path, or command supplied by WebView or EXIF goes to the engine.
            let mut json = String::from("[");
            for (i, (_, point)) in locations.iter().enumerate() {
                if i > 0 {
                    json.push(',');
                }
                let (lon, lat) = (point.lon, point.lat);
                if !lon.is_finite() || !lat.is_finite() {
                    return Err("GPS 座標不正確。".into());
                }
                let _ = write!(json, "[{lon},{lat}]");
            }
            json.push(']');
            format!("window.picseeMapSet({generation},{json},{active})")
        };
        self.webview
            .evaluate_script(&script)
            .map_err(|e| e.to_string())?;
        self.key = Some(key);
        Ok(clicked)
    }
}

impl Drop for WebPhotoMap {
    fn drop(&mut self) {
        self.tiles.gate.online.store(false, Ordering::Release);
        self.tiles.gate.visible.store(false, Ordering::Release);
    }
}
