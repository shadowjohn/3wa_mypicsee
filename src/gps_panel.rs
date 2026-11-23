use crate::gps_index::{self, Scan, Scope};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Sense, Stroke};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
};

enum Action {
    Scan(Scope),
    Clear,
}

enum Outcome {
    Scan(Result<Scan, String>),
    Clear(Result<(), String>),
}

struct Running {
    cancellable: bool,
    rx: Receiver<Outcome>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct PhotoMap {
    pub open: bool,
    running: Option<Running>,
    label: String,
    message: Option<String>,
    scan: Option<Scan>,
}

impl PhotoMap {
    pub fn clear_view(&mut self) {
        if let Some(run) = &self.running {
            run.cancel.store(true, Ordering::Relaxed);
        }
        self.scan = None;
        self.message = None;
        self.label.clear();
    }

    fn start(&mut self, ctx: &egui::Context, action: Action) {
        if self.running.is_some() {
            return;
        }
        self.message = None;
        self.scan = None;
        self.label = match &action {
            Action::Scan(scope) => scope.path().display().to_string(),
            Action::Clear => String::new(),
        };
        let cancellable = matches!(&action, Action::Scan(_));
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let repaint = ctx.clone();
        match std::thread::Builder::new()
            .name("picsee-gps".into())
            .spawn(move || {
                let result = match action {
                    Action::Scan(scope) => Outcome::Scan(gps_index::scan(&scope, &flag)),
                    Action::Clear => Outcome::Clear(gps_index::clear_local()),
                };
                let _ = tx.send(result);
                repaint.request_repaint();
            }) {
            Ok(_) => {
                self.running = Some(Running {
                    rx,
                    cancel,
                    cancellable,
                })
            }
            Err(e) => self.message = Some(format!("Cannot start GPS worker: {e}")),
        }
    }

    fn poll(&mut self) {
        let Some(run) = &self.running else {
            return;
        };
        let result = match run.rx.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let cancelled = run.cancel.load(Ordering::Relaxed);
        self.running = None;
        match result {
            Some(Outcome::Scan(_)) if cancelled => {
                self.message = Some("Scan stopped. Previously indexed rows remain local.".into())
            }
            Some(Outcome::Scan(Ok(scan))) => self.scan = Some(scan),
            Some(Outcome::Scan(Err(e)) | Outcome::Clear(Err(e))) => self.message = Some(e),
            Some(Outcome::Clear(Ok(()))) => self.message = Some("Local GPS index cleared.".into()),
            None => self.message = Some("GPS worker ended without a result.".into()),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, selected: Option<&Path>) -> Option<PathBuf> {
        self.poll();
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut jump = None;
        egui::Window::new("Photo locations")
            .open(&mut open)
            .default_width(430.0)
            .max_width(700.0)
            .show(ctx, |ui| {
                ui.label("Offline GPS coordinate plot (no basemap). JPG/JPEG only.");
                let busy = self.running.is_some();
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !busy && selected.is_some_and(gps_index::is_jpeg),
                            egui::Button::new("This photo"),
                        )
                        .clicked()
                        && let Some(path) = selected
                    {
                        self.start(ctx, Action::Scan(Scope::Photo(path.into())));
                    }
                    if ui
                        .add_enabled(
                            !busy && selected.and_then(Path::parent).is_some(),
                            egui::Button::new("Scan current folder"),
                        )
                        .clicked()
                        && let Some(folder) = selected.and_then(Path::parent)
                    {
                        self.start(ctx, Action::Scan(Scope::Folder(folder.into())));
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Clear index"))
                        .clicked()
                    {
                        self.start(ctx, Action::Clear);
                    }
                    if let Some(run) = &self.running {
                        let stopping = run.cancel.load(Ordering::Relaxed);
                        ui.spinner();
                        if ui
                            .add_enabled(
                                run.cancellable && !stopping,
                                egui::Button::new(if stopping { "Stopping…" } else { "Cancel" }),
                            )
                            .clicked()
                        {
                            run.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                });
                if !self.label.is_empty() {
                    ui.add(egui::Label::new(&self.label).truncate())
                        .on_hover_text(&self.label);
                }
                if let Some(message) = &self.message {
                    ui.label(message);
                }
                if let Some(scan) = &self.scan {
                    ui.label(format!(
                        "{} located · {} without GPS · {} unreadable · {} cache hits",
                        scan.locations.len(),
                        scan.no_gps,
                        scan.errors,
                        scan.cache_hits
                    ));
                    if let Some(e) = &scan.first_error {
                        ui.add(egui::Label::new(e).truncate()).on_hover_text(e);
                    }
                    if scan.truncated {
                        ui.label(format!(
                            "Stopped at {} JPEG files; results are partial.",
                            gps_index::LIMIT
                        ));
                    }
                    if !scan.locations.is_empty() {
                        jump = plot(ui, &scan.locations, selected);
                        ui.label("Choose a photo below when pins overlap.");
                        let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                        egui::ScrollArea::vertical().max_height(140.0).show_rows(
                            ui,
                            row,
                            scan.locations.len(),
                            |ui, rows| {
                                for i in rows {
                                    let (path, point) = &scan.locations[i];
                                    let file =
                                        path.file_name().unwrap_or_default().to_string_lossy();
                                    if ui
                                        .selectable_label(
                                            selected == Some(path.as_path()),
                                            format!("{file}   {:.6}, {:.6}", point.lat, point.lon),
                                        )
                                        .clicked()
                                    {
                                        jump = Some(path.clone());
                                    }
                                }
                            },
                        );
                    }
                }
            });
        self.open = open;
        jump
    }
}

impl Drop for PhotoMap {
    fn drop(&mut self) {
        if let Some(run) = &self.running {
            run.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn plot(
    ui: &mut egui::Ui,
    locations: &[(PathBuf, gps_index::Point)],
    selected: Option<&Path>,
) -> Option<PathBuf> {
    let (frame, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 170.0), Sense::click());
    let inner = frame.shrink(18.0);
    let paint = ui.painter_at(frame);
    paint.rect_filled(frame, 6.0, Color32::from_rgb(18, 30, 38));
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let x = egui::lerp(inner.x_range(), t);
        let y = egui::lerp(inner.y_range(), t);
        let grid = Stroke::new(1.0, Color32::from_rgb(40, 60, 66));
        paint.line_segment(
            [Pos2::new(x, inner.top()), Pos2::new(x, inner.bottom())],
            grid,
        );
        paint.line_segment(
            [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
            grid,
        );
    }
    let first = locations[0].1;
    let (mut min_lat, mut max_lat, mut min_lon, mut max_lon) =
        (first.lat, first.lat, first.lon, first.lon);
    for (_, p) in locations {
        min_lat = min_lat.min(p.lat);
        max_lat = max_lat.max(p.lat);
        min_lon = min_lon.min(p.lon);
        max_lon = max_lon.max(p.lon);
    }
    let fraction = |n: f64, min: f64, max: f64| -> f32 {
        if (max - min).abs() < 1e-10 {
            0.5
        } else {
            ((n - min) / (max - min)) as f32
        }
    };
    let mut nearest = None;
    let mut distance = 64.0;
    for (path, point) in locations {
        let pos = Pos2::new(
            egui::lerp(inner.x_range(), fraction(point.lon, min_lon, max_lon)),
            egui::lerp(inner.y_range(), 1.0 - fraction(point.lat, min_lat, max_lat)),
        );
        let active = selected == Some(path.as_path());
        paint.circle_filled(
            pos,
            if active { 5.5 } else { 3.5 },
            if active {
                Color32::from_rgb(255, 198, 103)
            } else {
                Color32::from_rgb(99, 219, 196)
            },
        );
        if let Some(mouse) = response.hover_pos() {
            let d = mouse.distance_sq(pos);
            if d < distance {
                nearest = Some((path, point));
                distance = d;
            }
        }
    }
    paint.text(
        frame.right_top() + egui::vec2(-5.0, 4.0),
        Align2::RIGHT_TOP,
        "N ↑",
        FontId::proportional(12.0),
        Color32::LIGHT_GRAY,
    );
    if let Some((path, point)) = nearest {
        let clicked = response.clicked();
        response.on_hover_text(format!(
            "{}  {:.6}, {:.6}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            point.lat,
            point.lon
        ));
        if clicked {
            return Some(path.clone());
        }
    }
    None
}
