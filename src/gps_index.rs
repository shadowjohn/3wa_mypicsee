use exif::{In, Reader, Tag, Value};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs::{self, File, Metadata},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
}

pub fn is_jpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| x.eq_ignore_ascii_case("jpeg") || x.eq_ignore_ascii_case("jpg"))
}

fn axis(
    exif: &exif::Exif,
    coord: Tag,
    side: Tag,
    pos: u8,
    neg: u8,
    max: f64,
) -> Result<f64, String> {
    let malformed = || "Invalid GPS coordinate in JPEG EXIF".to_string();
    let degrees = exif.get_field(coord, In::PRIMARY).ok_or_else(malformed)?;
    let direction = exif.get_field(side, In::PRIMARY).ok_or_else(malformed)?;
    let [d, m, s] = match &degrees.value {
        Value::Rational(raw) if raw.len() == 3 && raw.iter().all(|r| r.denom != 0) => {
            [raw[0].to_f64(), raw[1].to_f64(), raw[2].to_f64()]
        }
        // Some cameras store D/M/S as SRATIONAL. Magnitude must still be
        // nonnegative; hemisphere is carried by the separate GPS ref tag.
        Value::SRational(raw)
            if raw.len() == 3 && raw.iter().all(|r| r.num >= 0 && r.denom > 0) =>
        {
            [raw[0].to_f64(), raw[1].to_f64(), raw[2].to_f64()]
        }
        _ => return Err(malformed()),
    };
    let Value::Ascii(dir) = &direction.value else {
        return Err(malformed());
    };
    let sign = match dir.as_slice() {
        [dir] if dir.as_slice() == [pos] => 1.0,
        [dir] if dir.as_slice() == [neg] => -1.0,
        _ => return Err(malformed()),
    };
    let result = d + m / 60.0 + s / 3600.0;
    if m >= 60.0 || s >= 60.0 || result > max {
        return Err(malformed());
    }
    Ok(result * sign)
}

fn read_jpeg(reader: impl BufRead) -> Result<Option<Point>, String> {
    let mut reader = reader.take(8 * 1024 * 1024);
    let read_error = |e| format!("JPEG header could not be read: {e}");
    let mut header = [0; 2];
    let mut has_frame_header = false;
    reader.read_exact(&mut header).map_err(read_error)?;
    if header != [0xff, 0xd8] {
        return Err("Not a JPEG".into());
    }
    loop {
        let mut b = [0; 1];
        reader.read_exact(&mut b).map_err(read_error)?;
        if b[0] != 0xff {
            return Err("Invalid JPEG marker".into());
        }
        while b[0] == 0xff {
            reader.read_exact(&mut b).map_err(read_error)?;
        }
        let code = b[0];
        if matches!(code, 0 | 0xd0..=0xd9) {
            return Err("Invalid or incomplete JPEG header".into());
        }
        if code == 1 {
            continue;
        }
        reader.read_exact(&mut header).map_err(read_error)?;
        let len = u16::from_be_bytes(header)
            .checked_sub(2)
            .ok_or("Invalid JPEG segment length")?;
        let mut block = vec![0; usize::from(len)];
        reader.read_exact(&mut block).map_err(read_error)?;
        if code == 0xda {
            // A valid scan header confirms metadata ended, without reading compressed pixels.
            if !has_frame_header
                || !block
                    .first()
                    .is_some_and(|&n| n != 0 && block.len() == 4 + 2 * usize::from(n))
            {
                return Err("Invalid JPEG scan header".into());
            }
            return Ok(None);
        }
        if matches!(code, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) && block.len() >= 6
        {
            has_frame_header = true;
        }
        if code != 0xe1 || !block.starts_with(b"Exif\0\0") {
            continue;
        }
        let exif = Reader::new()
            .read_raw(block.split_off(6))
            .map_err(|e| format!("Invalid JPEG EXIF: {e}"))?;
        let tags = [
            Tag::GPSLatitude,
            Tag::GPSLongitude,
            Tag::GPSLatitudeRef,
            Tag::GPSLongitudeRef,
        ];
        if tags
            .iter()
            .all(|&t| exif.get_field(t, In::PRIMARY).is_none())
        {
            return Ok(None);
        }
        return Ok(Some(Point {
            lat: axis(
                &exif,
                Tag::GPSLatitude,
                Tag::GPSLatitudeRef,
                b'N',
                b'S',
                90.0,
            )?,
            lon: axis(
                &exif,
                Tag::GPSLongitude,
                Tag::GPSLongitudeRef,
                b'E',
                b'W',
                180.0,
            )?,
        }));
    }
}

#[derive(PartialEq)]
struct Stamp {
    size: i64,
    modified: String,
}

fn stamp(meta: &Metadata) -> Result<Stamp, String> {
    if !meta.is_file() {
        return Err("Not an ordinary image file".into());
    }
    let size = i64::try_from(meta.len()).map_err(|_| "Image size exceeds the cache range")?;
    let modified = match meta
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(UNIX_EPOCH)
    {
        Ok(t) => format!("{}:{:09}", t.as_secs(), t.subsec_nanos()),
        Err(e) => format!(
            "-{}:{:09}",
            e.duration().as_secs(),
            e.duration().subsec_nanos()
        ),
    };
    Ok(Stamp { size, modified })
}

fn key(path: &Path) -> Vec<u8> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().as_encoded_bytes().to_vec()
    }
}

fn open_db(path: &Path) -> rusqlite::Result<Connection> {
    let mut db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(2))?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    match version {
        0 => tx.execute_batch(
            "CREATE TABLE photo_gps (
                path BLOB PRIMARY KEY NOT NULL,
                size INTEGER NOT NULL,
                mtime TEXT NOT NULL,
                lat REAL,
                lon REAL,
                CHECK ((lat IS NULL AND lon IS NULL) OR (lat BETWEEN -90 AND 90 AND lon BETWEEN -180 AND 180))
            ) WITHOUT ROWID;
            PRAGMA user_version=1;"
        )?,
        1 => (),
        _ => return Err(rusqlite::Error::InvalidQuery),
    }
    tx.commit()?;
    Ok(db)
}

#[cfg(test)]
fn probe(db: &Path, path: &Path) -> Result<(Option<Point>, bool), String> {
    probe_photo(&open_db(db).map_err(|e| e.to_string())?, path)
}

fn probe_photo(db: &Connection, path: &Path) -> Result<(Option<Point>, bool), String> {
    let stat = || {
        fs::metadata(path)
            .map_err(|e| e.to_string())
            .and_then(|meta| stamp(&meta))
    };
    let before = stat()?;
    let path_key = key(path);
    let cached = db
        .query_row(
            "SELECT lat, lon FROM photo_gps WHERE path=?1 AND size=?2 AND mtime=?3",
            params![&path_key, before.size, &before.modified],
            |row| {
                Ok(row
                    .get::<_, Option<f64>>(0)?
                    .zip(row.get::<_, Option<f64>>(1)?)
                    .map(|(lat, lon)| Point { lat, lon }))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(value) = cached {
        return Ok((value, true));
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    if stamp(&file.metadata().map_err(|e| e.to_string())?)? != before {
        return Err("Image changed before GPS reading".into());
    }
    let result = read_jpeg(BufReader::new(file))?;
    if stat()? != before {
        return Err("Image changed during GPS reading".into());
    }
    let (lat, lon) = result.map_or((None, None), |p| (Some(p.lat), Some(p.lon)));
    db.execute(
        "INSERT INTO photo_gps(path,size,mtime,lat,lon) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(path) DO UPDATE SET size=excluded.size,mtime=excluded.mtime,lat=excluded.lat,lon=excluded.lon",
        params![&path_key, before.size, &before.modified, lat, lon],
    ).map_err(|e| e.to_string())?;
    Ok((result, false))
}

fn clear(db_path: &Path) -> Result<(), String> {
    if !db_path.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    open_db(db_path)
        .and_then(|db| db.execute_batch("PRAGMA secure_delete=ON; DELETE FROM photo_gps;"))
        .map_err(|e| e.to_string())
}

fn cache_path() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    base.map(|dir| dir.join("3wa_mypicsee").join("gps-index.sqlite3"))
        .ok_or_else(|| "The local PicSee cache directory is unavailable".into())
}

pub fn clear_local() -> Result<(), String> {
    clear(&cache_path()?)
}

pub const LIMIT: usize = 10_000;

pub enum Scope {
    Photo(PathBuf),
    Folder(PathBuf),
}

impl Scope {
    pub fn path(&self) -> &Path {
        match self {
            Self::Photo(path) | Self::Folder(path) => path,
        }
    }
}

pub struct Scan {
    pub locations: Vec<(PathBuf, Point)>,
    pub no_gps: usize,
    pub errors: usize,
    pub first_error: Option<String>,
    pub cache_hits: usize,
    pub truncated: bool,
}

pub fn scan(scope: &Scope, cancel: &AtomicBool) -> Result<Scan, String> {
    scan_at(scope, cancel, cache_path)
}

fn scan_at(
    scope: &Scope,
    cancel: &AtomicBool,
    db_location: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<Scan, String> {
    let stopped = || cancel.load(Ordering::Relaxed);
    let mut out = Scan {
        locations: Vec::new(),
        no_gps: 0,
        errors: 0,
        first_error: None,
        cache_hits: 0,
        truncated: false,
    };
    let mut paths = Vec::new();
    match scope {
        Scope::Photo(p) if is_jpeg(p) => paths.push(p.clone()),
        Scope::Photo(_) => return Err("GPS indexing currently supports JPG/JPEG only".into()),
        Scope::Folder(folder) => {
            for entry in fs::read_dir(folder).map_err(|e| format!("Cannot read folder: {e}"))? {
                if stopped() {
                    return Err("GPS scan cancelled".into());
                }
                let entry = entry.map_err(|e| format!("Cannot read folder entry: {e}"))?;
                if !is_jpeg(&entry.path())
                    || !entry.file_type().map_err(|e| e.to_string())?.is_file()
                {
                    continue;
                }
                if paths.len() >= LIMIT {
                    out.truncated = true;
                    break;
                }
                paths.push(entry.path());
            }
            paths.sort_unstable_by(|a, b| {
                crate::navigation::natural_cmp(
                    &a.file_name().unwrap_or_default().to_string_lossy(),
                    &b.file_name().unwrap_or_default().to_string_lossy(),
                )
                .then_with(|| a.cmp(b))
            });
        }
    }
    if stopped() {
        return Err("GPS scan cancelled".into());
    }
    if paths.is_empty() {
        return Ok(out);
    }
    let db_path = db_location()?;
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Cannot create GPS cache: {e}"))?;
    }
    let mut db = open_db(&db_path).map_err(|e| format!("Cannot open GPS cache: {e}"))?;
    for batch in paths.chunks(128) {
        if stopped() {
            return Err("GPS scan cancelled".into());
        }
        let tx = db
            .transaction()
            .map_err(|e| format!("Cannot begin GPS cache batch: {e}"))?;
        for path in batch {
            if stopped() {
                return Err("GPS scan cancelled".into());
            }
            match probe_photo(&tx, path) {
                Ok((point, hit)) => {
                    if hit {
                        out.cache_hits += 1;
                    }
                    if let Some(point) = point {
                        out.locations.push((path.clone(), point));
                    } else {
                        out.no_gps += 1;
                    }
                }
                Err(e) => {
                    out.errors += 1;
                    if out.first_error.is_none() {
                        out.first_error = Some(format!("{}: {e}", path.display()));
                    }
                }
            }
        }
        if stopped() {
            return Err("GPS scan cancelled".into());
        }
        tx.commit()
            .map_err(|e| format!("Cannot save GPS cache batch: {e}"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::Cursor,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    fn gps_jpeg() -> Vec<u8> {
        // EXIF/TIFF fixture, little endian: GPS IFD at byte 26, rationals at 80 and 104.
        let mut tiff = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        tiff.extend_from_slice(&[1, 0, 0x25, 0x88, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0]);
        tiff.extend_from_slice(&[4, 0]);
        tiff.extend_from_slice(&[1, 0, 2, 0, 2, 0, 0, 0, b'S', 0, 0, 0]);
        tiff.extend_from_slice(&[2, 0, 5, 0, 3, 0, 0, 0, 80, 0, 0, 0]);
        tiff.extend_from_slice(&[3, 0, 2, 0, 2, 0, 0, 0, b'W', 0, 0, 0]);
        tiff.extend_from_slice(&[4, 0, 5, 0, 3, 0, 0, 0, 104, 0, 0, 0]);
        tiff.extend_from_slice(&[0; 4]);
        for n in [25u32, 1, 30, 1, 0, 1, 121, 1, 15, 1, 0, 1] {
            tiff.extend_from_slice(&n.to_le_bytes());
        }
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    fn jpeg_without_exif() -> Vec<u8> {
        let mut buf = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut buf)
            .encode(&[0; 3], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        buf
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "picsee-gps-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn extracts_signed_gps_without_decoding_pixels() {
        assert_eq!(
            read_jpeg(Cursor::new(gps_jpeg())).unwrap(),
            Some(Point {
                lat: -25.5,
                lon: -121.25
            })
        );
    }

    #[test]
    fn common_signed_gps_rationals_keep_hemisphere_and_reject_invalid_components() {
        let mut jpeg = gps_jpeg();
        // TIFF type 10 = SRATIONAL. Nonnegative D/M/S still use the ASCII hemisphere.
        for tiff_offset in [42, 66] {
            jpeg[12 + tiff_offset] = 10;
        }
        let point = read_jpeg(Cursor::new(&jpeg)).unwrap().unwrap();
        assert!((point.lat + 25.5).abs() < 1e-9);
        assert!((point.lon + 121.25).abs() < 1e-9);
        jpeg[92..96].copy_from_slice(&(-25i32).to_le_bytes());
        assert!(read_jpeg(Cursor::new(&jpeg)).is_err());
        jpeg[92..96].copy_from_slice(&25i32.to_le_bytes());
        jpeg[96..100].copy_from_slice(&0i32.to_le_bytes());
        assert!(read_jpeg(Cursor::new(&jpeg)).is_err());
    }


    #[test]
    fn distinguishes_absent_gps_from_unreadable_metadata() {
        assert_eq!(read_jpeg(Cursor::new(jpeg_without_exif())).unwrap(), None);
        assert!(read_jpeg(Cursor::new(b"not a JPEG")).is_err());
        let mut broken = gps_jpeg();
        broken[6 + 6 + 80 + 4] = 0; // Zero the latitude degrees denominator.
        assert!(read_jpeg(Cursor::new(broken)).is_err());
    }

    #[test]
    fn cache_reopens_invalidates_by_mtime_and_clears() {
        let dir = Scratch::new();
        let db = dir.0.join("gps.sqlite");
        let img = dir.0.join("photo.jpg");
        let bytes = gps_jpeg();
        fs::write(&img, &bytes).unwrap();
        assert_eq!(
            probe(&db, &img).unwrap(),
            (
                Some(Point {
                    lat: -25.5,
                    lon: -121.25
                }),
                false
            )
        );
        assert!(probe(&db, &img).unwrap().1);
        let file = fs::File::options().write(true).open(&img).unwrap();
        let old = file.metadata().unwrap().modified().unwrap();
        file.set_modified(old - std::time::Duration::from_secs(10))
            .unwrap();
        assert!(!probe(&db, &img).unwrap().1);
        clear(&db).unwrap();
        assert!(!probe(&db, &img).unwrap().1);
    }

    #[test]
    fn caches_genuine_no_gps_but_not_parse_failures() {
        let dir = Scratch::new();
        let db = dir.0.join("gps.sqlite");
        let img = dir.0.join("photo.jpeg");
        fs::write(&img, jpeg_without_exif()).unwrap();
        assert_eq!(probe(&db, &img).unwrap(), (None, false));
        assert_eq!(probe(&db, &img).unwrap(), (None, true));
        fs::write(&img, b"not a JPEG").unwrap();
        assert!(probe(&db, &img).is_err());
        assert!(probe(&db, &img).is_err());
    }

    #[test]
    fn folder_scan_is_non_recursive_and_keeps_same_point_photos_selectable() {
        let scratch = Scratch::new();
        let folder = scratch.0.join("photos");
        fs::create_dir(&folder).unwrap();
        fs::create_dir(folder.join("nested")).unwrap();
        let bytes = gps_jpeg();
        for name in [
            "photo10.jpeg",
            "photo2.JPG",
            "nested/hidden.jpg",
            "ignore.png",
        ] {
            fs::write(folder.join(name), &bytes).unwrap();
        }
        fs::write(folder.join("blank.jpg"), jpeg_without_exif()).unwrap();
        fs::write(folder.join("bad.jpg"), b"not a JPEG").unwrap();
        let db = scratch.0.join("index.sqlite");
        let scope = Scope::Folder(folder.clone());
        let cancel = AtomicBool::new(false);
        let scan = scan_at(&scope, &cancel, || Ok(db.clone())).unwrap();
        assert_eq!(
            scan.locations
                .iter()
                .map(|(p, _)| p.file_name().unwrap().to_str().unwrap())
                .collect::<Vec<_>>(),
            ["photo2.JPG", "photo10.jpeg"]
        );
        assert_eq!(scan.locations[0].1, scan.locations[1].1);
        assert_eq!((scan.no_gps, scan.errors, scan.cache_hits), (1, 1, 0));
        let reread = scan_at(&scope, &cancel, || Ok(db.clone())).unwrap();
        assert_eq!((reread.cache_hits, reread.errors), (3, 1));
        assert_eq!(fs::read(folder.join("photo2.JPG")).unwrap(), bytes);
        cancel.store(true, Ordering::Relaxed);
        assert!(
            scan_at(&scope, &cancel, || panic!(
                "cancelled scan should not open cache"
            ))
            .is_err()
        );
    }
}
