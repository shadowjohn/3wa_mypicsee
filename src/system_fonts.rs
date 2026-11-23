use eframe::egui;
use std::path::Path;

pub(crate) struct CjkFallback {
    requested: bool,
    #[cfg(windows)]
    ctx: egui::Context,
}

impl CjkFallback {
    pub(crate) fn new(_ctx: &egui::Context) -> Self {
        Self {
            requested: false,
            #[cfg(windows)]
            ctx: _ctx.clone(),
        }
    }

    pub(crate) fn request_for_path(&mut self, path: &Path) {
        if !self.requested && contains_cjk(path) {
            self.request();
        }
    }

    pub(crate) fn request(&mut self) {
        if self.requested {
            return;
        }
        self.requested = true;

        #[cfg(windows)]
        {
            // A system TTC can be tens of MiB; never read it on the UI thread
            // or during an ordinary ASCII-only photo open.
            let ctx = self.ctx.clone();
            let _ = std::thread::Builder::new()
                .name("picsee-system-font".into())
                .spawn(move || {
                    if let Some(font) = load_system_font() {
                        install_font(&ctx, font);
                        ctx.request_repaint();
                    }
                });
        }
    }
}

fn contains_cjk(path: &Path) -> bool {
    path.as_os_str().to_string_lossy().chars().any(|ch| {
        matches!(
            ch,
            '\u{1100}'..='\u{11FF}' // Hangul Jamo
                | '\u{2E80}'..='\u{9FFF}' // CJK, kana and punctuation
                | '\u{AC00}'..='\u{D7FF}' // Hangul syllables
                | '\u{F900}'..='\u{FAFF}' // CJK compatibility ideographs
                | '\u{FF00}'..='\u{FFEF}' // Full/half-width forms
                | '\u{20000}'..='\u{323AF}' // CJK extensions
        )
    })
}

#[cfg(windows)]
fn load_system_font() -> Option<Vec<u8>> {
    use std::{fs::File, io::Read, path::PathBuf};
    const MAX_FONT: u64 = 48 * 1024 * 1024;
    let fonts = PathBuf::from(std::env::var_os("SystemRoot")?).join("Fonts");
    for name in ["msjh.ttc", "mingliu.ttc", "msyh.ttc"] {
        let Ok(file) = File::open(fonts.join(name)) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(MAX_FONT + 1).read_to_end(&mut bytes).is_ok()
            && bytes.len() as u64 <= MAX_FONT
            && supported_header(&bytes)
        {
            return Some(bytes);
        }
    }
    None
}

#[cfg(windows)]
fn supported_header(bytes: &[u8]) -> bool {
    let base = if bytes.starts_with(b"ttcf") {
        let Some(offset) = bytes.get(12..16).and_then(|raw| raw.try_into().ok()) else {
            return false;
        };
        u32::from_be_bytes(offset) as usize
    } else {
        0
    };
    bytes.get(base..).is_some_and(|font| {
        font.len() >= 12 && (font.starts_with(b"\x00\x01\x00\x00") || font.starts_with(b"OTTO"))
    })
}

#[cfg(windows)]
fn install_font(ctx: &egui::Context, font: Vec<u8>) {
    ctx.add_font(egui::epaint::text::FontInsert {
        name: "PicSee CJK fallback".into(),
        data: egui::FontData::from_owned(font),
        families: [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
            .into_iter()
            .map(|family| egui::epaint::text::InsertFontFamily {
                family,
                priority: egui::epaint::text::FontPriority::Lowest,
            })
            .collect(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cjk_in_any_path_component_without_loading_for_ascii() {
        assert!(!contains_cjk(Path::new("C:/photos/A016.jpg")));
        assert!(!contains_cjk(Path::new("C:/photos/café.jpg")));
        assert!(contains_cjk(Path::new("C:/測試照片/A016.jpg")));
        assert!(contains_cjk(Path::new("C:/photos/車站.jpg")));
        assert!(contains_cjk(Path::new("C:/photos/花・色.jpg")));
        let mut fallback = CjkFallback::new(&egui::Context::default());
        fallback.request_for_path(Path::new("C:/photos/A016.jpg"));
        assert!(!fallback.requested);
    }

    #[cfg(windows)]
    #[test]
    fn installed_fallback_adds_traditional_chinese_without_changing_latin_advance() {
        let Some(font) = load_system_font() else {
            eprintln!("No supported CJK system font on this Windows installation");
            return;
        };
        let ctx = egui::Context::default();
        let inspect = |ctx: &egui::Context| {
            let mut info = None;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                info = Some(ctx.fonts_mut(|fonts| {
                    [
                        egui::FontId::proportional(14.0),
                        egui::FontId::monospace(14.0),
                    ]
                    .map(|id| {
                        (
                            fonts.has_glyphs(&id, "照片火車快看寶開啟圖片縮放地圖關於適合視窗鎖定位置"),
                            fonts.glyph_width(&id, 'A'),
                        )
                    })
                }));
            });
            info.unwrap()
        };
        let before = inspect(&ctx);
        install_font(&ctx, font);
        let after = inspect(&ctx);
        assert!(after.iter().all(|(has_cjk, _)| *has_cjk));
        for (first, second) in before.into_iter().zip(after) {
            assert_eq!(first.1, second.1, "Latin fallback precedence changed");
        }
    }
}
