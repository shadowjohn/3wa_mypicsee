use crate::gps_index::{self, Scan, Scope};
#[cfg(all(windows, feature = "photo-webgl"))]
use crate::photo_webview::WebPhotoMap;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, RichText, Sense, Stroke};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
};

const TEAL: Color32 = Color32::from_rgb(122, 210, 191);
const WARN: Color32 = Color32::from_rgb(234, 184, 117);

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
    message_detail: Option<String>,
    scan: Option<Scan>,
    #[cfg(all(windows, feature = "photo-webgl"))]
    web: Option<WebPhotoMap>,
    #[cfg(all(windows, feature = "photo-webgl"))]
    map_revision: u32,
    #[cfg(all(windows, feature = "photo-webgl"))]
    cluster: Vec<usize>,
    #[cfg(all(windows, feature = "photo-webgl"))]
    map_error: Option<String>,
}

impl PhotoMap {
    fn invalidate_map(&mut self) {
        #[cfg(all(windows, feature = "photo-webgl"))]
        {
            self.map_revision = self.map_revision.wrapping_add(1).max(1);
            self.cluster.clear();
        }
    }

    #[cfg(all(windows, feature = "photo-webgl"))]
    fn show_web(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        frame: Option<&eframe::Frame>,
        selected: Option<&Path>,
    ) -> Option<PathBuf> {
        if ui
            .button(if self.web.is_some() {
                "收起互動地圖"
            } else {
                "開啟互動地圖"
            })
            .clicked()
        {
            self.map_error = None;
            if self.web.is_some() {
                self.web = None;
                self.cluster.clear();
            } else if let Some(frame) = frame {
                match WebPhotoMap::new(frame, ctx) {
                    Ok(web) => self.web = Some(web),
                    Err(error) => self.map_error = Some(error),
                }
            }
        }
        if let Some(message) = &self.map_error {
            ui.label(RichText::new(message).color(WARN));
        }

        let web = self.web.as_mut()?;
        ui.add_space(3.0);
        let mut online = web.osm_enabled();
        if ui
            .add_enabled(
                web.is_ready() && frame.is_some(),
                egui::Checkbox::new(&mut online, "使用 OpenStreetMap（連線）"),
            )
            .changed()
            && let Err(error) = web.set_osm(online)
        {
            ui.label(format!("無法切換底圖：{error}"));
        }
        ui.label(
            RichText::new("開啟連線底圖後，OpenStreetMap 會收到目前瀏覽的區域與你的 IP。")
                .small()
                .weak(),
        );
        if web.osm_enabled() {
            ui.label(
                RichText::new("底圖無法載入時，仍會顯示離線格線。")
                    .small()
                    .weak(),
            );
            ui.hyperlink_to(
                "© OpenStreetMap contributors",
                "https://www.openstreetmap.org/copyright",
            );
        }
        let (space, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 230.0), Sense::hover());
        if !web.is_ready() {
            let painter = ui.painter_at(space);
            painter.rect_filled(space, 6.0, Color32::from_rgb(19, 32, 44));
            painter.text(
                space.center(),
                Align2::CENTER_CENTER,
                "正在啟動互動地圖…",
                FontId::proportional(14.0),
                TEAL,
            );
        }
        let bounds = if frame.is_some() {
            space
                .intersect(ui.clip_rect())
                .intersect(ctx.viewport_rect())
        } else {
            egui::Rect::ZERO
        };
        let points = self
            .scan
            .as_ref()
            .map_or(&[][..], |scan| scan.locations.as_slice());
        let active = selected.and_then(|s| points.iter().position(|(path, _)| path == s));
        let result = web
            .place(bounds, ctx.pixels_per_point())
            .and_then(|_| web.tick(ctx, self.map_revision.max(1), points, active));
        match result {
            Ok(Some(ids)) if ids.len() == 1 => {
                self.cluster.clear();
                return points.get(ids[0]).map(|(path, _)| path.clone());
            }
            Ok(Some(ids)) => self.cluster = ids,
            Ok(None) => {}
            Err(error) => {
                self.web = None;
                self.cluster.clear();
                self.map_error = Some(format!("{error} 已切回內建離線點圖。"));
                ctx.request_repaint();
            }
        }
        None
    }

    pub fn clear_view(&mut self) {
        #[cfg(all(windows, feature = "photo-webgl"))]
        if let Some(web) = &mut self.web {
            let _ = web.set_osm(false);
        }
        if let Some(run) = &self.running {
            run.cancel.store(true, Ordering::Relaxed);
        }
        self.scan = None;
        self.invalidate_map();
        self.message = None;
        self.message_detail = None;
        self.label.clear();
    }

    fn start(&mut self, ctx: &egui::Context, action: Action) {
        if self.running.is_some() {
            return;
        }
        self.message = None;
        self.message_detail = None;
        self.scan = None;
        self.invalidate_map();
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
            Err(e) => {
                self.message = Some("無法啟動 GPS 掃描。".into());
                self.message_detail = Some(e.to_string());
            }
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
                self.message = Some("已停止掃描。已建立的索引仍保存在本機。".into())
            }
            Some(Outcome::Scan(Ok(scan))) => {
                self.invalidate_map();
                self.scan = Some(scan);
            }
            Some(Outcome::Scan(Err(e))) => {
                self.message = Some("無法取得掃描結果。".into());
                self.message_detail = Some(e);
            }
            Some(Outcome::Clear(Err(e))) => {
                self.message = Some("無法清除 GPS 索引。".into());
                self.message_detail = Some(e);
            }
            Some(Outcome::Clear(Ok(()))) => self.message = Some("已清除本機 GPS 索引。".into()),
            None => self.message = Some("GPS 掃描未傳回結果。".into()),
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        selected: Option<&Path>,
        _frame: Option<&eframe::Frame>,
    ) -> Option<PathBuf> {
        self.poll();
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut jump = None;
        #[cfg(all(windows, feature = "photo-webgl"))]
        let mut clear_cluster = false;
        egui::Window::new("照片地圖")
            .open(&mut open)
            .default_width(430.0)
            .max_width(700.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("拍攝位置").strong().size(15.0));
                ui.label(
                    RichText::new("從 JPG／JPEG 照片讀取拍攝位置，預設離線。")
                        .small()
                        .weak(),
                );
                ui.add_space(5.0);
                let busy = self.running.is_some();
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !busy && selected.is_some_and(gps_index::is_jpeg),
                            egui::Button::new("讀取這張"),
                        )
                        .clicked()
                        && let Some(path) = selected
                    {
                        self.start(ctx, Action::Scan(Scope::Photo(path.into())));
                    }
                    if ui
                        .add_enabled(
                            !busy && selected.and_then(Path::parent).is_some(),
                            egui::Button::new(RichText::new("掃描此資料夾").strong()),
                        )
                        .clicked()
                        && let Some(folder) = selected.and_then(Path::parent)
                    {
                        self.start(ctx, Action::Scan(Scope::Folder(folder.into())));
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("清除索引"))
                        .clicked()
                    {
                        self.start(ctx, Action::Clear);
                    }
                    if let Some(run) = &self.running {
                        let stopping = run.cancel.load(Ordering::Relaxed);
                        ui.spinner();
                        ui.label(
                            RichText::new(if run.cancellable {
                                "正在掃描…"
                            } else {
                                "正在清除…"
                            })
                            .small()
                            .weak(),
                        );
                        if ui
                            .add_enabled(
                                run.cancellable && !stopping,
                                egui::Button::new(if stopping {
                                    "正在停止…"
                                } else {
                                    "取消"
                                }),
                            )
                            .clicked()
                        {
                            run.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                });
                if !self.label.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&self.label).small().weak()).truncate())
                        .on_hover_text(&self.label);
                }
                if let Some(message) = &self.message {
                    let notice = ui.label(RichText::new(message).color(
                        if self.message_detail.is_some() {
                            WARN
                        } else {
                            TEAL
                        },
                    ));
                    if let Some(detail) = &self.message_detail {
                        notice.on_hover_text(detail);
                    }
                }
                #[cfg(all(windows, feature = "photo-webgl"))]
                {
                    jump = self.show_web(ui, ctx, _frame, selected);
                }
                if let Some(scan) = &self.scan {
                    ui.add_space(5.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("{} 張有定位", scan.locations.len()))
                                .small()
                                .strong()
                                .color(TEAL),
                        );
                        ui.label(
                            RichText::new(format!("{} 張無定位", scan.no_gps))
                                .small()
                                .weak(),
                        );
                        if scan.errors != 0 {
                            ui.label(
                                RichText::new(format!("{} 張無法讀取", scan.errors))
                                    .small()
                                    .color(WARN),
                            );
                        }
                        if scan.cache_hits != 0 {
                            ui.label(
                                RichText::new(format!("{} 張來自快取", scan.cache_hits))
                                    .small()
                                    .weak(),
                            );
                        }
                    });
                    if let Some(e) = &scan.first_error {
                        ui.label(RichText::new("查看讀取失敗的原因").small().color(WARN))
                            .on_hover_text(e);
                    }
                    if scan.truncated {
                        ui.label(format!(
                            "已達 {} 張 JPEG 的掃描上限，以下為部分結果。",
                            gps_index::LIMIT
                        ));
                    }
                    if scan.locations.is_empty() {
                        ui.label(RichText::new("此範圍沒有可顯示的拍攝位置。").small().weak());
                    } else {
                        #[cfg(all(windows, feature = "photo-webgl"))]
                        let cluster = self.cluster.as_slice();
                        #[cfg(not(all(windows, feature = "photo-webgl")))]
                        let cluster: &[usize] = &[];
                        #[cfg(all(windows, feature = "photo-webgl"))]
                        let native = self.web.is_none();
                        #[cfg(not(all(windows, feature = "photo-webgl")))]
                        let native = true;
                        if native {
                            jump = plot(ui, &scan.locations, selected);
                        }
                        let row_count = if cluster.is_empty() {
                            scan.locations.len()
                        } else {
                            cluster.len()
                        };
                        if cluster.is_empty() {
                            ui.add_space(4.0);
                            ui.label(RichText::new("照片清單").small().strong());
                            ui.label(
                                RichText::new("點位重疊時，可從清單挑選照片。")
                                    .small()
                                    .weak(),
                            );
                        }
                        #[cfg(all(windows, feature = "photo-webgl"))]
                        if !cluster.is_empty() {
                            let show_all = ui
                                .horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("此處有 {} 張照片", cluster.len()))
                                            .small()
                                            .strong(),
                                    );
                                    ui.button("顯示全部").clicked()
                                })
                                .inner;
                            if show_all {
                                ctx.request_repaint();
                                clear_cluster = true;
                            }
                        }
                        let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                        egui::ScrollArea::vertical().max_height(140.0).show_rows(
                            ui,
                            row,
                            row_count,
                            |ui, rows| {
                                for i in rows {
                                    let index = cluster.get(i).copied().unwrap_or(i);
                                    let (path, point) = &scan.locations[index];
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
        #[cfg(all(windows, feature = "photo-webgl"))]
        {
            if clear_cluster {
                self.cluster.clear();
            }
            if !open {
                self.web = None;
                self.cluster.clear();
                self.map_error = None;
            }
        }
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
    paint.rect_filled(frame, 6.0, Color32::from_rgb(18, 31, 42));
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let x = egui::lerp(inner.x_range(), t);
        let y = egui::lerp(inner.y_range(), t);
        let grid = Stroke::new(1.0, Color32::from_rgb(43, 63, 75));
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
        "↑ 北",
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
