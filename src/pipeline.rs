//! One foreground lane, one prefetch lane, and a byte-limited decoded LRU.
//! No disk cache or network access. Files are opened read-only.
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

const MAX_INPUT: u64 = 128 * 1024 * 1024;
const MAX_EDGE: u32 = 32_768;
const MAX_PIXELS: u64 = 40_000_000;
const MAX_DECODE_ALLOC: u64 = 192 * 1024 * 1024;
const MAX_PREVIEW_EDGE: u32 = 4096;
const CACHE_BYTES: usize = 96 * 1024 * 1024;
const PREFETCH_COUNT: usize = 5;

#[derive(Clone, Debug)]
pub struct Pixels {
    /// Dimensions after applying the source orientation, before preview resizing.
    pub original_width: u32,
    pub original_height: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Decoded {
    pub generation: u64,
    pub path: PathBuf,
    pub result: Result<Pixels, String>,
}

#[derive(Clone, Debug)]
struct Task {
    ticket: u64,
    generation: u64,
    path: PathBuf,
    max_edge: u32,
}

#[derive(PartialEq, Eq)]
struct Key {
    path: PathBuf,
    max_edge: u32,
    len: u64,
    modified: SystemTime,
}

struct Cache {
    items: VecDeque<(Key, Arc<Pixels>)>,
    bytes: usize,
}

impl Cache {
    fn new() -> Self {
        Self {
            items: VecDeque::new(),
            bytes: 0,
        }
    }

    fn get(&mut self, key: &Key) -> Option<Arc<Pixels>> {
        let index = self.items.iter().position(|(cached, _)| cached == key)?;
        let item = self.items.remove(index)?;
        let pixels = Arc::clone(&item.1);
        self.items.push_back(item);
        Some(pixels)
    }

    fn put(&mut self, key: Key, pixels: Arc<Pixels>) {
        let len = pixels.rgba.len();
        if len > CACHE_BYTES {
            return;
        }
        if let Some(index) = self.items.iter().position(|(cached, _)| *cached == key) {
            if let Some((_, old)) = self.items.remove(index) {
                self.bytes -= old.rgba.len();
            }
        }
        while self.bytes + len > CACHE_BYTES || self.items.len() >= 32 {
            if let Some((_, old)) = self.items.pop_front() {
                self.bytes -= old.rgba.len();
            } else {
                break;
            }
        }
        self.bytes += len;
        self.items.push_back((key, pixels));
    }
}

struct State {
    ticket: u64,
    stopped: bool,
    foreground: Option<Task>,
    prefetch: VecDeque<Task>,
    answer: Option<Decoded>,
    cache: Cache,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    repaint: Box<dyn Fn() + Send + Sync>,
}

pub struct Pipeline {
    shared: Arc<Shared>,
}

impl Pipeline {
    pub fn new(repaint: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                ticket: 0,
                stopped: false,
                foreground: None,
                prefetch: VecDeque::new(),
                answer: None,
                cache: Cache::new(),
            }),
            ready: Condvar::new(),
            repaint: Box::new(repaint),
        });
        for foreground in [true, false] {
            let worker = Arc::clone(&shared);
            std::thread::spawn(move || run(worker, foreground));
        }
        Self { shared }
    }

    pub fn request(&self, generation: u64, path: PathBuf, max_edge: u32, neighbors: Vec<PathBuf>) {
        let max_edge = max_edge.clamp(1, MAX_PREVIEW_EDGE);
        let mut state = self.shared.state.lock().unwrap();
        state.ticket = state.ticket.wrapping_add(1);
        let ticket = state.ticket;
        state.answer = None;
        state.foreground = Some(Task {
            ticket,
            generation,
            path: path.clone(),
            max_edge,
        });
        Self::set_neighbors(&mut state, generation, &path, max_edge, neighbors);
        drop(state);
        self.shared.ready.notify_all();
    }

    pub fn prime_neighbors(&self, generation: u64, path: &Path, max_edge: u32, neighbors: Vec<PathBuf>) {
        let mut state = self.shared.state.lock().unwrap();
        Self::set_neighbors(&mut state, generation, path, max_edge.clamp(1, MAX_PREVIEW_EDGE), neighbors);
        drop(state);
        self.shared.ready.notify_all();
    }

    fn set_neighbors(state: &mut State, generation: u64, path: &Path, max_edge: u32, neighbors: Vec<PathBuf>) {
        state.prefetch.clear();
        for neighbor in neighbors {
            if neighbor != path && !state.prefetch.iter().any(|task| task.path == neighbor) {
                state.prefetch.push_back(Task {
                    ticket: state.ticket,
                    generation,
                    path: neighbor,
                    max_edge,
                });
                if state.prefetch.len() == PREFETCH_COUNT {
                    break;
                }
            }
        }
    }

    pub fn try_recv(&self) -> Option<Decoded> {
        self.shared.state.lock().unwrap().answer.take()
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.stopped = true;
        state.foreground = None;
        state.prefetch.clear();
        state.answer = None;
        // Do not join a blocking decode in the window-close path.
        drop(state);
        self.shared.ready.notify_all();
    }
}

fn run(shared: Arc<Shared>, foreground: bool) {
    loop {
        let task = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.stopped {
                    return;
                }
                let next = if foreground {
                    state.foreground.take()
                } else {
                    state.prefetch.pop_front()
                };
                if let Some(task) = next {
                    break task;
                }
                state = shared.ready.wait(state).unwrap();
            }
        };
        let result = load(&shared, &task);
        if foreground {
            {
                let state = shared.state.lock().unwrap();
                if state.stopped || state.ticket != task.ticket {
                    continue;
                }
            }
            // A result can grow to 64 MiB. Clone outside the queue mutex so
            // advancing a selection never waits behind this memory copy.
            let decoded = Decoded {
                generation: task.generation,
                path: task.path,
                result: result.map(|pixels| Arc::try_unwrap(pixels).unwrap_or_else(|p| (*p).clone())),
            };
            let mut state = shared.state.lock().unwrap();
            if !state.stopped && state.ticket == task.ticket {
                state.answer = Some(decoded);
                drop(state);
                (shared.repaint)();
            }
        }
    }
}

fn load(shared: &Shared, task: &Task) -> Result<Arc<Pixels>, String> {
    if !task.path.is_file() {
        return Err("Image file does not exist or is not a regular file".into());
    }
    let source = File::open(&task.path).map_err(|err| err.to_string())?;
    let meta = source.metadata().map_err(|err| err.to_string())?;
    if !meta.is_file() || meta.len() > MAX_INPUT {
        return Err("Not a regular file or input exceeds 128 MiB".into());
    }
    let key = meta.modified().ok().map(|modified| Key {
        path: task.path.clone(),
        max_edge: task.max_edge,
        len: meta.len(),
        modified,
    });
    if let Some(key) = &key {
        if let Some(pixels) = shared.state.lock().unwrap().cache.get(key) {
            return Ok(pixels);
        }
    }

    let pixels = Arc::new(decode(source, meta.len(), &task.path, task.max_edge)?);
    if let Some(key) = key {
        let mut state = shared.state.lock().unwrap();
        // Keep the current neighborhood hot; an old, slow decode must not
        // evict useful frames after the viewer moves to a new neighborhood.
        if !state.stopped && state.ticket == task.ticket {
            state.cache.put(key, Arc::clone(&pixels));
        }
    }
    Ok(pixels)
}

/// Limit inputs to the initial file length, even if a concurrent writer
/// appends bytes. The seekable adapter also lets TIFF move between IFDs.
struct BoundedFile {
    file: File,
    len: u64,
    pos: u64,
}

impl Read for BoundedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = self.len.saturating_sub(self.pos);
        let n = buf.len().min(available as usize);
        let read = self.file.read(&mut buf[..n])?;
        self.pos += read as u64;
        Ok(read)
    }
}

impl Seek for BoundedFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.pos) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.len) + i128::from(n),
        };
        let pos = u64::try_from(pos)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "Invalid image seek"))?;
        self.pos = self.file.seek(SeekFrom::Start(pos))?;
        Ok(self.pos)
    }
}

fn decode(source: File, len: u64, path: &Path, max_edge: u32) -> Result<Pixels, String> {
    let mut header_reader = BufReader::new(BoundedFile {
        file: source,
        len,
        pos: 0,
    });
    let mut prefix = [0u8; 16];
    let mut size = 0;
    while size < prefix.len() {
        match header_reader.read(&mut prefix[size..]) {
            Ok(0) => break,
            Ok(n) => size += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.to_string()),
        }
    }
    let format = image::guess_format(&prefix[..size]).map_err(|_| {
        format!(
            "Unsupported or unreadable image: {}",
            path.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        )
    })?;
    if !matches!(
        format,
        ImageFormat::Jpeg
            | ImageFormat::Png
            | ImageFormat::WebP
            | ImageFormat::Gif
            | ImageFormat::Tiff
            | ImageFormat::Bmp
    ) {
        return Err("Unsupported image format".into());
    }

    header_reader.rewind().map_err(|err| err.to_string())?;
    let mut reader = ImageReader::with_format(header_reader, format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|err| err.to_string())?;
    let (w, h) = decoder.dimensions();
    if w == 0
        || h == 0
        || u64::from(w) * u64::from(h) > MAX_PIXELS
        || decoder.total_bytes() > MAX_DECODE_ALLOC
    {
        return Err("Image exceeds the preview size limit".into());
    }
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).map_err(|err| err.to_string())?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    let (original_width, original_height) = (image.width(), image.height());
    if original_width.max(original_height) > max_edge {
        image = image.resize(
            max_edge,
            max_edge,
            image::imageops::FilterType::Triangle,
        );
    }
    let pixels = image.into_rgba8();
    Ok(Pixels {
        original_width,
        original_height,
        width: pixels.width(),
        height: pixels.height(),
        rgba: pixels.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(color: [u8; 4]) -> Self {
            static COUNT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "mypicsee-{}-{}-{}.png",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                COUNT.fetch_add(1, Ordering::Relaxed),
            ));
            image::RgbaImage::from_pixel(32, 16, image::Rgba(color))
                .save(&path)
                .unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn preserves_aspect_and_original_dimensions() {
        let input = Fixture::new([0, 130, 120, 255]);
        let source = File::open(&input.0).unwrap();
        let len = source.metadata().unwrap().len();
        let img = decode(source, len, &input.0, 8).unwrap();
        assert_eq!((img.original_width, img.original_height), (32, 16));
        assert_eq!((img.width, img.height), (8, 4));
        assert_eq!(&img.rgba[..4], &[0, 130, 120, 255]);
    }

    #[test]
    fn common_file_formats_and_corrupt_data() {
        let path = Fixture::new([255, 90, 15, 255]);
        let sample = image::RgbImage::from_pixel(16, 9, image::Rgb([255, 90, 15]));
        for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::WebP,
            ImageFormat::Gif, ImageFormat::Tiff, ImageFormat::Bmp] {
            sample.save_with_format(&path.0, format).unwrap();
            let source = File::open(&path.0).unwrap();
            let len = source.metadata().unwrap().len();
            let img = decode(source, len, &path.0, 64).unwrap_or_else(|e| panic!("{format:?}: {e}"));
            assert_eq!((img.original_width, img.original_height), (16, 9), "{format:?}");
        }
        std::fs::write(&path.0, b"This is not a PNG.").unwrap();
        let source = File::open(&path.0).unwrap();
        let len = source.metadata().unwrap().len();
        assert!(decode(source, len, &path.0, 64).is_err());
    }

    #[test]
    fn reader_does_not_expose_bytes_appended_mid_load() {
        use std::io::Write;
        let input = Fixture::new([0, 130, 120, 255]);
        let file = File::open(&input.0).unwrap();
        let initial_len = file.metadata().unwrap().len();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&input.0)
            .unwrap()
            .write_all(b"appended after load started")
            .unwrap();
        let mut bounded = BoundedFile {
            file,
            len: initial_len,
            pos: 0,
        };
        assert_eq!(bounded.seek(SeekFrom::End(0)).unwrap(), initial_len);
        bounded.rewind().unwrap();
        let mut bytes = Vec::new();
        bounded.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, initial_len);
    }

    #[test]
    fn latest_selection_wins_across_rapid_requests() {
        let old = Fixture::new([255, 0, 0, 255]);
        let current = Fixture::new([0, 0, 255, 255]);
        let pipe = Pipeline::new(|| {});
        pipe.request(91, old.0.clone(), 32, vec![]);
        pipe.request(17, current.0.clone(), 16, vec![old.0.clone()]);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(answer) = pipe.try_recv() {
                assert_eq!(answer.generation, 17);
                assert_eq!(answer.path, current.0);
                assert_eq!(&answer.result.unwrap().rgba[..4], &[0, 0, 255, 255]);
                break;
            }
            assert!(Instant::now() < deadline, "preview worker did not reply");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
