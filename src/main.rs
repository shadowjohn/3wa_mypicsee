#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod navigation;
mod pipeline;

use eframe::egui::{self, Color32, ColorImage, Key, PointerButton, Rect, RichText, Sense, TextureHandle, TextureOptions, Vec2};
use std::{path::{Path, PathBuf}, sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc::{self, Receiver, Sender}}};

const VERSION: &str = include_str!("../RELEASE_VERSION");
const PREVIEW_EDGE: u32 = 2304;
const DETAIL_EDGE: u32 = 4096;
const SUFFIXES: [&str; 8] = ["jpg", "jpeg", "png", "webp", "gif", "tif", "tiff", "bmp"];
type FolderResult = (u64, Result<Vec<PathBuf>, String>);

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(PathBuf::from);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1050.0, 720.0]).with_min_inner_size([430.0, 315.0]);
    if let Ok(icon) = image::load_from_memory(include_bytes!("../assets/app-icon.png")) {
        let rgba = icon.into_rgba8();
        viewport = viewport.with_icon(egui::IconData {
            width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw(),
        });
    }
    eframe::run_native(
        &format!("看快寶 | 3wa_mypicsee {}", VERSION.trim()),
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(move |cc| Ok(Box::new(Viewer::new(&cc.egui_ctx, path)))),
    )
}

struct Displayed {
    texture: TextureHandle,
    natural: Vec2,
}

struct Viewer {
    pipeline: pipeline::Pipeline,
    mascot: Option<TextureHandle>,
    selected: Option<PathBuf>,
    displayed: Option<Displayed>,
    paths: Vec<PathBuf>,
    generation: u64,
    folder_generation: Arc<AtomicU64>,
    folder_tx: Sender<FolderResult>,
    folder_rx: Receiver<FolderResult>,
    picker: Option<Receiver<Option<PathBuf>>>,
    scale: Option<f32>,
    pan: Vec2,
    lock_pan: bool,
    has_detail: bool,
    loading: bool,
    wheel_points: f32,
    error: Option<String>,
}

impl Viewer {
    fn new(ctx: &egui::Context, initial: Option<PathBuf>) -> Self {
        let (folder_tx, folder_rx) = mpsc::channel();
        let repaint = ctx.clone();
        let pipeline = pipeline::Pipeline::new(move || repaint.request_repaint());

        // Opening a named image must not wait on the artwork.
        let mascot = initial.is_none().then(|| {
            let image = image::load_from_memory(include_bytes!("../assets/mika-3wa.png")).ok()?;
            let image = image.thumbnail(480, 480).to_rgba8();
            Some(ctx.load_texture("mika", ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize], image.as_raw(),
            ), TextureOptions::LINEAR))
        }).flatten();

        let mut viewer = Self {
            pipeline, mascot, selected: None, displayed: None, paths: Vec::new(),
            generation: 0, folder_generation: Arc::new(AtomicU64::new(0)),
            folder_tx, folder_rx, picker: None, scale: None, pan: Vec2::ZERO,
            lock_pan: false, has_detail: false, loading: false, wheel_points: 0.0, error: None,
        };
        if let Some(path) = initial { viewer.open(ctx, path); }
        viewer
    }

    fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        if !supported(&path) {
            self.error = Some("Choose a JPEG, PNG, WebP, GIF, TIFF, or BMP image.".into());
            return;
        }

        let path = if path.is_absolute() { path } else {
            std::env::current_dir().map_or_else(|_| path.clone(), |cwd| cwd.join(&path))
        };
        let folder_id = self.folder_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let counter = self.folder_generation.clone();
        let tx = self.folder_tx.clone();
        let repaint = ctx.clone();
        self.paths = vec![path.clone()];
        self.selected = Some(path.clone());
        self.pan = Vec2::ZERO;
        self.has_detail = false;
        self.request(PREVIEW_EDGE, 1, true);

        let _ = std::thread::Builder::new().name("picsee-folder".into()).spawn(move || {
            let Some(folder) = path.parent() else { return; };
            let result = (|| -> std::io::Result<Vec<PathBuf>> {
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(folder)? {
                    if counter.load(Ordering::Relaxed) != folder_id { return Ok(Vec::new()); }
                    let Ok(entry) = entry else { continue; };
                    let candidate = entry.path();
                    if supported(&candidate) && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        entries.push((name, candidate));
                    }
                }
                entries.sort_unstable_by(|a, b| navigation::natural_cmp(&a.0, &b.0)
                    .then_with(|| a.1.cmp(&b.1)));
                Ok(entries.into_iter().map(|(_, path)| path).collect())
            })().map_err(|e| format!("Folder scan failed: {e}"));
            if counter.load(Ordering::Relaxed) == folder_id {
                let _ = tx.send((folder_id, result));
                repaint.request_repaint();
            }
        });
    }

    fn choose_file(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() { return; }
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let path = rfd::FileDialog::new().add_filter("Images", &SUFFIXES).pick_file();
            let _ = tx.send(path);
            repaint.request_repaint();
        });
        self.picker = Some(rx);
    }

    fn request(&mut self, max_edge: u32, direction: i32, show_loading: bool) {
        let Some(path) = self.selected.clone() else { return; };
        self.generation = self.generation.wrapping_add(1);
        if show_loading {
            self.error = None;
            self.loading = true;
        }
        let neighbors = if max_edge == PREVIEW_EDGE {
            self.neighbor_paths(direction)
        } else { Vec::new() };
        self.pipeline.request(self.generation, path, max_edge, neighbors);
    }

    fn index(&self) -> Option<usize> {
        self.selected.as_ref().and_then(|path| self.paths.iter().position(|p| p == path))
    }

    fn neighbor_paths(&self, direction: i32) -> Vec<PathBuf> {
        self.index().map_or_else(Vec::new, |i|
            navigation::neighbors(i, self.paths.len(), direction)
                .into_iter().map(|i| self.paths[i].clone()).collect())
    }

    fn navigate(&mut self, step: i32) {
        let Some(index) = self.index() else { return; };
        let next = index.saturating_add_signed(step as isize).min(self.paths.len() - 1);
        if next == index { return; }
        self.selected = Some(self.paths[next].clone());
        if !self.lock_pan { self.pan = Vec2::ZERO; }
        self.has_detail = false;
        self.request(PREVIEW_EDGE, step, true);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(picker) = &self.picker {
            match picker.try_recv() {
                Ok(path) => { self.picker = None; if let Some(path) = path { self.open(ctx, path); } }
                Err(mpsc::TryRecvError::Disconnected) => self.picker = None,
                Err(mpsc::TryRecvError::Empty) => (),
            }
        }
        while let Ok((id, result)) = self.folder_rx.try_recv() {
            if id != self.folder_generation.load(Ordering::Relaxed) { continue; }
            if let Ok(paths) = result && self.selected.as_ref().is_some_and(|p| paths.contains(p)) {
                self.paths = paths;
                // Background discovery never invalidates the first foreground image.
                if let Some(path) = &self.selected {
                    self.pipeline.prime_neighbors(self.generation, path, PREVIEW_EDGE, self.neighbor_paths(0));
                }
            }
        }
        while let Some(decoded) = self.pipeline.try_recv() {
            if decoded.generation != self.generation || self.selected.as_ref() != Some(&decoded.path) {
                continue;
            }
            let was_loading = self.loading;
            self.loading = false;
            match decoded.result {
                Ok(pixels) => {
                    let natural = egui::vec2(pixels.original_width as f32, pixels.original_height as f32);
                    let rgba = ColorImage::from_rgba_unmultiplied(
                        [pixels.width as usize, pixels.height as usize], &pixels.rgba,
                    );
                    let texture = ctx.load_texture("selected-photo", rgba, TextureOptions::LINEAR);
                    self.displayed = Some(Displayed { texture, natural });
                    self.error = None;
                }
                Err(error) if was_loading => {
                    self.displayed = None;
                    self.error = Some(error);
                }
                Err(_) => (),
            }
        }
    }

    fn fit(&mut self, viewport: Vec2) {
        if !self.loading && let Some(display) = &self.displayed {
            self.scale = Some(navigation::fit_scale(
                (display.natural.x, display.natural.y), (viewport.x, viewport.y),
            ));
            self.pan = Vec2::ZERO;
        }
    }

    fn zoom(&mut self, factor: f32) {
        if self.loading || self.displayed.is_none() { return; }
        self.scale = Some((self.scale.unwrap_or(1.0) * factor).clamp(0.0001, 16.0));
        if !self.has_detail && let Some(display) = &self.displayed {
            let zoom = self.scale.unwrap_or(1.0);
            let displayed_edge = display.natural.x.max(display.natural.y) * zoom;
            let resident = display.texture.size();
            if displayed_edge > PREVIEW_EDGE as f32 && resident[0].max(resident[1]) < DETAIL_EDGE as usize
                && displayed_edge > resident[0].max(resident[1]) as f32 {
                self.has_detail = true;
                self.request(DETAIL_EDGE, 0, false);
            }
        }
    }

    fn title_line(&self) -> String {
        let Some(path) = &self.selected else { return format!("Build {}", VERSION.trim()); };
        let name = path.file_name().map_or_else(|| path.display().to_string(), |s| s.to_string_lossy().into_owned());
        let dims = self.displayed.as_ref().filter(|_| !self.loading).map_or_else(String::new, |d| {
            format!("   •   {:.0} × {:.0}", d.natural.x, d.natural.y)
        });
        format!("{} / {}   •   {name}{dims}", self.index().map_or(1, |i| i + 1), self.paths.len())
    }
}

impl eframe::App for Viewer {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll(ctx);
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().find_map(|item| item.path.clone()));
        if let Some(path) = dropped { self.open(ctx, path); }

        let (open, previous, next, star, plus, minus, lock, scroll) = ctx.input(|i| {
            let star = i.events.iter().any(|e| matches!(e, egui::Event::Text(s) if s == "*"));
            let wheel = i.events.iter().filter_map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => Some((*unit, delta.y)),
                _ => None,
            }).collect::<Vec<_>>();
            (
                i.modifiers.command && i.key_pressed(Key::O),
                i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowDown),
                star || (i.modifiers.shift && i.key_pressed(Key::Num8)),
                i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals),
                i.key_pressed(Key::Minus),
                i.key_pressed(Key::P) && !i.modifiers.command,
                wheel,
            )
        });
        if open { self.choose_file(ctx); }
        if previous { self.navigate(-1); }
        if next { self.navigate(1); }
        if lock { self.lock_pan = !self.lock_pan; }

        let mut fit = star;
        let mut zoom = if plus { 1.25 } else if minus { 0.8 } else { 1.0 };
        egui::TopBottomPanel::top("tools").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("PICSEE").strong().color(Color32::from_rgb(99, 219, 196)));
                ui.separator();
                if ui.button("Open…").on_hover_text("Ctrl+O").clicked() { self.choose_file(ctx); }
                if ui.button("Prev").on_hover_text("Previous image").clicked() { self.navigate(-1); }
                if ui.button("Next").on_hover_text("Next image").clicked() { self.navigate(1); }
                ui.separator();
                if ui.button("Fit *").on_hover_text("Fit the current image and lock the scale").clicked() { fit = true; }
                if ui.button("−").clicked() { zoom = 0.8; }
                ui.label(self.scale.map_or_else(|| "Fit".into(), |x| format!("{:.0}%", x * 100.0)));
                if ui.button("+").clicked() { zoom = 1.25; }
                ui.toggle_value(&mut self.lock_pan, "Lock pan (P)");
            });
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.title_line());
                if self.loading { ui.spinner(); }
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_rgb(15, 20, 23)))
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
                if response.hovered() {
                    let mut point_steps = 0i32;
                    let mut last_notch = 0i32;
                    for (unit, y) in scroll {
                        if !y.is_finite() || y == 0.0 { continue; }
                        if unit == egui::MouseWheelUnit::Point {
                            point_steps = point_steps.saturating_add(
                                navigation::point_wheel_step(&mut self.wheel_points, y),
                            );
                        } else {
                            self.wheel_points = 0.0;
                            // Coalesce duplicated discrete notifications delivered in
                            // the same frame by device and window-system events.
                            last_notch = if y > 0.0 { -1 } else { 1 };
                        }
                    }
                    let steps = if last_notch != 0 { last_notch } else { point_steps };
                    if steps != 0 { self.navigate(steps); }
                } else {
                    self.wheel_points = 0.0;
                }
                if fit || (self.scale.is_none() && self.displayed.is_some() && !self.loading) {
                    self.fit(rect.size() - egui::vec2(12.0, 12.0));
                }
                if zoom != 1.0 { self.zoom(zoom); }
                if self.displayed.is_some() && response.dragged_by(PointerButton::Primary) {
                    self.pan += ctx.input(|i| i.pointer.delta());
                }

                let painter = ui.painter().with_clip_rect(rect);
                if let Some(display) = &self.displayed {
                    let bounds = Rect::from_center_size(rect.center() + self.pan, display.natural * self.scale.unwrap_or(1.0));
                    painter.image(display.texture.id(), bounds, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                } else if self.selected.is_none() && self.error.is_none() {
                    if let Some(mika) = &self.mascot {
                        let available = (rect.height() - 100.0).clamp(60.0, 350.0);
                        let size = mika.size_vec2() * (available / mika.size_vec2().y);
                        let target = Rect::from_center_size(rect.center() - egui::vec2(0.0, 35.0), size);
                        painter.image(mika.id(), target, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    let button = Rect::from_center_size(rect.center() + egui::vec2(0.0, (rect.height()*0.36).min(160.0)), egui::vec2(180.0, 36.0));
                    if ui.put(button, egui::Button::new("Open photo…")).clicked() { self.choose_file(ctx); }
                    painter.text(button.center_bottom() + egui::vec2(0.0, 10.0), egui::Align2::CENTER_TOP,
                        "Or drop an image here", egui::FontId::proportional(13.0), Color32::LIGHT_GRAY);
                }
                if let Some(error) = &self.error {
                    painter.text(rect.center(), egui::Align2::CENTER_CENTER, error,
                        egui::FontId::proportional(15.0), Color32::from_rgb(255, 178, 143));
                } else if self.loading && self.displayed.is_none() {
                    painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Opening image…",
                        egui::FontId::proportional(15.0), Color32::LIGHT_GRAY);
                }
            });
    }
}

fn supported(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str())
        .is_some_and(|s| SUFFIXES.iter().any(|format| s.eq_ignore_ascii_case(format)))
}
