//! Library: every capture is kept in a folder, with a thumbnail and an index that allows searching
//! by date or by the title of the window that was in use.
//!
//! On-disk layout:
//! ```text
//! <folder>/index.json            list of entries, newest first
//! <folder>/2026-11-10_02-45-12.png
//! <folder>/.miniaturas/2026-11-10_02-45-12.png
//! ```

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::RgbaImage;
use serde::{Deserialize, Serialize};

const INDEX_FILE: &str = "index.json";
const THUMB_DIR: &str = ".miniaturas";
const LAYERS_VERSION: u32 = 1;
/// Maximum thumbnail width, in pixels.
pub const THUMB_WIDTH: u32 = 240;

#[derive(Serialize)]
struct LayersFileRef<'a> {
    version: u32,
    ops: &'a [crate::render::Op],
}

#[derive(Deserialize)]
struct LayersFile {
    version: u32,
    ops: Vec<crate::render::Op>,
}

/// Local date and time, already split (year, month, day, hour, minute, second).
pub type LocalTime = (u16, u16, u16, u16, u16, u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaFilter {
    #[default]
    All,
    Images,
    Videos,
    Favorites,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Name of the media file (`.png`, `.mp4`, or `.gif`) inside the library folder.
    pub file: String,
    /// Date to show and search, such as "10/11/2026 02:45".
    pub when: String,
    /// Title of the window that was in the foreground at capture time.
    #[serde(default)]
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// Duration in seconds (`0.0` for static images, `> 0.0` for videos/GIFs).
    #[serde(default)]
    pub duration_secs: f32,
    /// User-starred capture for quick filtering.
    #[serde(default)]
    pub favorite: bool,
    /// Custom user tags (without leading `#`), searchable from the library filter bar.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Entry {
    pub fn is_video(&self) -> bool {
        Path::new(&self.file).extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
    }

    pub fn is_gif(&self) -> bool {
        Path::new(&self.file).extension().is_some_and(|e| e.eq_ignore_ascii_case("gif"))
    }

    pub fn is_image(&self) -> bool {
        !self.is_video()
    }

    fn matches(&self, query_lower: &str) -> bool {
        let q_tag = query_lower.trim_start_matches('#');
        self.title.to_lowercase().contains(query_lower)
            || self.when.contains(query_lower)
            || self.file.to_lowercase().contains(query_lower)
            || (!q_tag.is_empty() && self.tags.iter().any(|t| t.to_lowercase().contains(q_tag)))
    }
}

pub struct Library {
    dir: PathBuf,
    entries: Vec<Entry>,
}

impl Library {
    /// Default folder: the user's `Pictures\SnapKit`.
    pub fn default_dir() -> PathBuf {
        crate::platform::pictures_dir().join("SnapKit")
    }

    /// Opens the library, dropping from the index the entries whose file is gone and adding the
    /// captures in the folder that the index lost (a damaged index, or one saved over by another
    /// copy of SnapKit). An index that can't be read is kept as `index.json.bad`.
    pub fn open(dir: PathBuf) -> Self {
        let index = dir.join(INDEX_FILE);
        let mut entries: Vec<Entry> = match std::fs::read(&index) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                let _ = std::fs::rename(&index, dir.join("index.json.bad"));
                crate::log_error(&format!("index.json could not be read ({e}); it was kept as index.json.bad"));
                Vec::new()
            }
            Ok(bytes) => match std::str::from_utf8(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string()))
            {
                Ok(entries) => entries,
                Err(e) => {
                    let _ = std::fs::rename(&index, dir.join("index.json.bad"));
                    crate::log_error(&format!("index.json could not be read ({e}); it was kept as index.json.bad"));
                    Vec::new()
                }
            },
        };
        entries.retain(|e| is_plain_file_name(&e.file) && dir.join(&e.file).is_file());
        let mut library = Self { dir, entries };
        if library.recover_lost_files() {
            library.save_index();
        }
        library
    }

    /// Adds the captures in the folder that aren't in the index. Returns `true` if any was added.
    fn recover_lost_files(&mut self) -> bool {
        let Ok(files) = std::fs::read_dir(&self.dir) else { return false };
        let mut lost: Vec<Entry> = files
            .flatten()
            .filter_map(|f| f.file_name().into_string().ok())
            .filter(|name| !is_sidecar_name(name) && capture_time(name).is_some() && !self.entries.iter().any(|e| &e.file == name))
            .filter_map(|file| {
                let (y, mo, d, h, mi) = capture_time(&file)?;
                let full_path = self.dir.join(&file);
                if !full_path.is_file() {
                    return None;
                }
                let is_mp4 = Path::new(&file).extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
                let (width, height) = if is_mp4 {
                    (0, 0)
                } else {
                    image::image_dimensions(&full_path).ok()?
                };
                let when = format!("{d}/{mo}/{y} {h}:{mi}");
                Some(Entry {
                    file,
                    when,
                    title: String::new(),
                    width,
                    height,
                    duration_secs: 0.0,
                    favorite: false,
                    tags: Vec::new(),
                })
            })
            .collect();
        if lost.is_empty() {
            return false;
        }
        self.entries.append(&mut lost);
        // The names are the capture time, so this is newest first, like the index.
        self.entries.sort_by(|a, b| capture_sort_key(&b.file).cmp(&capture_sort_key(&a.file)));
        true
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    pub fn thumb_path(&self, file: &str) -> PathBuf {
        let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file);
        self.dir.join(THUMB_DIR).join(format!("{stem}.png"))
    }

    pub fn save_thumb(&self, file: &str, thumb: &RgbaImage) {
        let _ = save_png_atomic(&make_thumb(thumb), &self.thumb_path(file));
    }

    pub fn layers_path(&self, file: &str) -> PathBuf {
        let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file);
        self.dir.join(format!("{stem}.snap.json"))
    }

    pub fn base_path(&self, file: &str) -> PathBuf {
        let stem = Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or(file);
        self.dir.join(format!("{stem}.base.png"))
    }

    pub fn save_layers_json(ops: &[crate::render::Op]) -> Result<String, String> {
        serde_json::to_string(&LayersFileRef { version: LAYERS_VERSION, ops }).map_err(|e| e.to_string())
    }

    pub fn load_layers(&self, file: &str) -> Option<(RgbaImage, Vec<crate::render::Op>)> {
        let bytes = std::fs::read(self.layers_path(file)).ok()?;
        let parsed: LayersFile = serde_json::from_slice(&bytes).ok()?;
        if parsed.version != LAYERS_VERSION {
            return None;
        }
        let base = image::open(self.base_path(file)).ok()?.to_rgba8();
        Some((base, parsed.ops))
    }

    pub fn remove_layers(&self, file: &str) {
        let _ = std::fs::remove_file(self.layers_path(file));
        let _ = std::fs::remove_file(self.base_path(file));
    }

    /// Entries that contain the search text (case-insensitive), newest first.
    pub fn search(&self, query: &str) -> Vec<&Entry> {
        self.search_filtered(query, MediaFilter::All)
    }

    /// Entries matching the search text and media filter, newest first.
    pub fn search_filtered(&self, query: &str, filter: MediaFilter) -> Vec<&Entry> {
        let q = query.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| match filter {
                MediaFilter::All => true,
                MediaFilter::Images => e.is_image(),
                MediaFilter::Videos => e.is_video(),
                MediaFilter::Favorites => e.favorite,
            })
            .filter(|e| q.is_empty() || e.matches(&q))
            .collect()
    }

    /// Returns `(total_count, images_count, videos_count)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let total = self.entries.len();
        let videos = self.entries.iter().filter(|e| e.is_video()).count();
        let images = total.saturating_sub(videos);
        (total, images, videos)
    }

    /// Number of starred captures in the library.
    pub fn favorite_count(&self) -> usize {
        self.entries.iter().filter(|e| e.favorite).count()
    }

    /// Toggles the favorite flag of `file`, saves the index, and returns the new state.
    pub fn toggle_favorite(&mut self, file: &str) -> Option<bool> {
        let e = self.entries.iter_mut().find(|e| e.file == file)?;
        e.favorite = !e.favorite;
        let state = e.favorite;
        self.save_index();
        Some(state)
    }

    /// Updates the user-editable title and normalized tags for `file`.
    pub fn set_metadata(&mut self, file: &str, title: String, tags: Vec<String>) -> bool {
        let Some(e) = self.entries.iter_mut().find(|e| e.file == file) else {
            return false;
        };
        let clean_title = title.trim().to_owned();
        let mut clean_tags: Vec<String> = Vec::new();
        for raw in tags {
            let trimmed = raw.trim().trim_start_matches('#').trim();
            if !trimmed.is_empty() && !clean_tags.iter().any(|t| t.eq_ignore_ascii_case(trimmed)) {
                clean_tags.push(trimmed.to_owned());
            }
        }
        if e.title != clean_title || e.tags != clean_tags {
            e.title = clean_title;
            e.tags = clean_tags;
            self.save_index();
        }
        true
    }

    pub fn get(&self, file: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.file == file)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reserves a unique `<YYYY-MM-DD_HH-MM-SS>.<ext>` name without modifying the index.
    pub fn reserve_name(&self, now: LocalTime, ext: &str) -> String {
        let ext = ext.trim_start_matches('.');
        let (y, mo, d, h, mi, s) = now;
        let base = format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}");
        let mut file = format!("{base}.{ext}");
        let mut n = 2;
        while self.entries.iter().any(|e| e.file == file) || self.path(&file).exists() {
            file = format!("{base}_{n}.{ext}");
            n += 1;
        }
        file
    }

    /// Registers a new capture and returns the file name reserved for it. The PNG itself is written
    /// later, by [`Writer`].
    pub fn add(&mut self, now: LocalTime, title: String, width: u32, height: u32) -> String {
        self.add_media(now, "png", title, width, height, 0.0)
    }

    /// Registers a new capture or video/GIF with extension `ext` and returns the reserved file name.
    pub fn add_media(
        &mut self,
        now: LocalTime,
        ext: &str,
        title: String,
        width: u32,
        height: u32,
        duration_secs: f32,
    ) -> String {
        let file = self.reserve_name(now, ext);
        let (_, mo, d, h, mi, _) = now;
        let y = now.0;
        let when = format!("{d:02}/{mo:02}/{y:04} {h:02}:{mi:02}");
        self.entries.insert(
            0,
            Entry {
                file: file.clone(),
                when,
                title,
                width,
                height,
                duration_secs,
                favorite: false,
                tags: Vec::new(),
            },
        );
        self.save_index();
        file
    }

    /// Inserts or updates an entry for a file that has already been written inside the library folder.
    pub fn register_existing_file(
        &mut self,
        file: String,
        now: LocalTime,
        title: String,
        width: u32,
        height: u32,
        duration_secs: f32,
    ) {
        let (y, mo, d, h, mi, _) = now;
        let when = format!("{d:02}/{mo:02}/{y:04} {h:02}:{mi:02}");
        if let Some(e) = self.entries.iter_mut().find(|e| e.file == file) {
            e.when = when;
            e.title = title;
            e.width = width;
            e.height = height;
            e.duration_secs = duration_secs;
        } else {
            self.entries.insert(
                0,
                Entry {
                    file,
                    when,
                    title,
                    width,
                    height,
                    duration_secs,
                    favorite: false,
                    tags: Vec::new(),
                },
            );
        }
        self.save_index();
    }

    /// Updates the recorded size after an edit (a crop, for example).
    pub fn set_size(&mut self, file: &str, width: u32, height: u32) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.file == file)
            && (e.width, e.height) != (width, height)
        {
            (e.width, e.height) = (width, height);
            self.save_index();
        }
    }

    /// Updates dimensions and duration after a video or GIF is edited in-place.
    pub fn update_media(&mut self, file: &str, width: u32, height: u32, duration_secs: f32) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.file == file)
            && ((e.width, e.height) != (width, height) || (e.duration_secs - duration_secs).abs() > f32::EPSILON)
        {
            e.width = width;
            e.height = height;
            e.duration_secs = duration_secs;
            self.save_index();
        }
    }

    /// Removes the entry from the index. Deleting the files is up to the caller.
    pub fn forget(&mut self, file: &str) {
        self.entries.retain(|e| e.file != file);
        self.save_index();
    }

    fn save_index(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(text) = serde_json::to_string_pretty(&self.entries) {
            let _ = crate::config::write_atomic(&self.dir.join(INDEX_FILE), text.as_bytes());
        }
    }
}

fn strip_media_ext(name: &str) -> Option<&str> {
    for ext in [".png", ".mp4", ".gif", ".PNG", ".MP4", ".GIF"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return Some(stem);
        }
    }
    None
}

/// Date and time in the name of a capture file (`2026-11-10_02-45-12.png`/`.mp4`/`.gif`, or with `_2`
/// for a second one in the same second), as text: year, month, day, hour, minute.
fn capture_time(name: &str) -> Option<(&str, &str, &str, &str, &str)> {
    let stem = strip_media_ext(name)?;
    let (stamp, suffix) = stem.split_at_checked(19)?;
    let b = stamp.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let shape = digits(0..4) && b[4] == b'-' && digits(5..7) && b[7] == b'-' && digits(8..10) && b[10] == b'_'
        && digits(11..13) && b[13] == b'-' && digits(14..16) && b[16] == b'-' && digits(17..19);
    let suffix_ok = suffix.is_empty() || suffix.strip_prefix('_').is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()));
    (shape && suffix_ok).then(|| (&stamp[0..4], &stamp[5..7], &stamp[8..10], &stamp[11..13], &stamp[14..16]))
}

/// Chronological key `(timestamp, sequence_within_second)` so `..._10.png` sorts after `..._2.png`.
fn capture_sort_key(name: &str) -> (&str, u64) {
    if let Some(stem) = strip_media_ext(name)
        && let Some((stamp, suffix)) = stem.split_at_checked(19)
    {
        let seq = if suffix.is_empty() {
            1
        } else {
            suffix.strip_prefix('_').and_then(|n| n.parse::<u64>().ok()).unwrap_or(0)
        };
        return (stamp, seq);
    }
    (name, 0)
}

fn is_sidecar_name(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    if lower.ends_with(".base.png")
        || lower.ends_with(".tmp.png")
        || lower.ends_with(".snap.json")
        || lower.ends_with(".video.mp4")
        || lower.ends_with(".tmp.mp4")
        || lower.ends_with(".tmp.gif")
        || lower.ends_with(".audio.wav")
        || lower.ends_with(".parts.txt")
    {
        return true;
    }
    if let Some(stem) = lower.strip_suffix(".mp4")
        && let Some((_, after_part)) = stem.rsplit_once(".part")
        && !after_part.is_empty()
        && after_part.bytes().all(|b| b.is_ascii_digit())
    {
        return true;
    }
    false
}

/// A tampered index must not point outside the library folder or at non-media files (like `index.json`).
fn is_plain_file_name(file: &str) -> bool {
    !file.is_empty()
        && !file.contains(['/', '\\', ':'])
        && file != "."
        && file != ".."
        && !is_sidecar_name(file)
        && Path::new(file).extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("png")
                || ext.eq_ignore_ascii_case("mp4")
                || ext.eq_ignore_ascii_case("gif")
        })
}

/// What gets selected after `file` leaves the list: the next item or, at the end, the previous one.
pub fn neighbor<'a>(files: &[&'a str], file: &str) -> Option<&'a str> {
    let at = files.iter().position(|f| *f == file)?;
    files.get(at + 1).or_else(|| at.checked_sub(1).and_then(|i| files.get(i))).copied()
}

/// Shrinks the image to fit [`THUMB_WIDTH`], without enlarging small images.
pub fn make_thumb(image: &RgbaImage) -> RgbaImage {
    if image.width() <= THUMB_WIDTH {
        return image.clone();
    }
    let h = (image.height() as u64 * THUMB_WIDTH as u64 / image.width() as u64).max(1) as u32;
    image::imageops::thumbnail(image, THUMB_WIDTH, h)
}

/// Stitches two or more images together vertically (top-to-bottom) or horizontally (left-to-right),
/// separated by `gap` pixels and centered along the cross axis.
pub fn combine_images(paths: &[PathBuf], vertical: bool, gap: u32) -> Result<RgbaImage, String> {
    if paths.len() < 2 {
        return Err("Select at least 2 images to combine".into());
    }
    let mut images = Vec::with_capacity(paths.len());
    for p in paths {
        let img = image::open(p)
            .map_err(|e| format!("{}: {e}", p.display()))?
            .to_rgba8();
        if img.width() == 0 || img.height() == 0 {
            return Err(format!("{}: empty image", p.display()));
        }
        images.push(img);
    }

    let total_gap = gap.saturating_mul((images.len() - 1) as u32);
    let (out_w, out_h) = if vertical {
        let w = images.iter().map(|i| i.width()).max().unwrap_or(1);
        let h = images.iter().map(|i| i.height()).fold(0u32, u32::saturating_add).saturating_add(total_gap);
        (w, h)
    } else {
        let w = images.iter().map(|i| i.width()).fold(0u32, u32::saturating_add).saturating_add(total_gap);
        let h = images.iter().map(|i| i.height()).max().unwrap_or(1);
        (w, h)
    };

    let mut canvas = RgbaImage::from_pixel(out_w, out_h, image::Rgba([30, 30, 34, 255]));
    let mut offset = 0u32;
    for img in &images {
        let (dx, dy) = if vertical {
            ((out_w.saturating_sub(img.width())) / 2, offset)
        } else {
            (offset, (out_h.saturating_sub(img.height())) / 2)
        };
        image::imageops::replace(&mut canvas, img, dx as i64, dy as i64);
        offset = offset
            .saturating_add(if vertical { img.height() } else { img.width() })
            .saturating_add(gap);
    }

    Ok(canvas)
}

pub struct LayersJob {
    pub base_path: PathBuf,
    pub base_image: Option<RgbaImage>,
    pub layers_path: PathBuf,
    pub ops_json: String,
}

struct Job {
    path: PathBuf,
    thumb_path: PathBuf,
    image: RgbaImage,
    thumb: RgbaImage,
    layers: Option<LayersJob>,
}

struct PendingGuard<'a>(&'a AtomicUsize);

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Writes PNGs on a single thread, in the order requested, so that an edit saved right after the
/// capture is never overwritten by the earlier write.
pub struct Writer {
    tx: Sender<Job>,
    /// Last failure (a full disk, a folder without permission), for the interface to show.
    error: Arc<Mutex<Option<String>>>,
    /// Captures handed over and not written yet.
    pending: Arc<AtomicUsize>,
}

impl Writer {
    pub fn spawn() -> Self {
        let (tx, rx) = channel::<Job>();
        let error = Arc::new(Mutex::new(None));
        let pending = Arc::new(AtomicUsize::new(0));
        let (report, done) = (error.clone(), pending.clone());
        std::thread::spawn(move || {
            for job in rx {
                let _guard = PendingGuard(&done);
                let path = job.path.clone();
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| write_job(&job)));
                match res {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        *report.lock().unwrap() = Some(format!("{}: {e}", path.display()));
                    }
                    Err(_) => {
                        *report.lock().unwrap() = Some(format!("{}: unexpected error while encoding PNG", path.display()));
                    }
                }
            }
        });
        Self { tx, error, pending }
    }

    /// Waits (up to `limit`) for the captures still being written, before SnapKit closes.
    pub fn wait_idle(&self, limit: std::time::Duration) {
        let end = std::time::Instant::now() + limit;
        while self.pending.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The last failure to save, if there was one since the previous call.
    pub fn take_error(&self) -> Option<String> {
        self.error.lock().unwrap().take()
    }

    pub fn save(&self, library: &Library, file: &str, image: RgbaImage, _thumb: RgbaImage) {
        self.save_with_layers(library.path(file), library.thumb_path(file), image, None);
    }

    pub fn save_with_layers(&self, path: PathBuf, thumb_path: PathBuf, image: RgbaImage, layers: Option<LayersJob>) {
        let thumb = make_thumb(&image);
        self.pending.fetch_add(1, Ordering::SeqCst);
        if self
            .tx
            .send(Job {
                path: path.clone(),
                thumb_path,
                image,
                thumb,
                layers,
            })
            .is_err()
        {
            self.pending.fetch_sub(1, Ordering::SeqCst);
            *self.error.lock().unwrap() = Some(format!("{}: background writer unavailable", path.display()));
        }
    }
}

fn save_png_atomic(img: &RgbaImage, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut tmp_name = path.file_stem().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp.png");
    let tmp = path.with_file_name(tmp_name);
    if let Err(e) = img.save(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}

fn write_job(job: &Job) -> Result<(), String> {
    save_png_atomic(&job.image, &job.path)?;
    save_png_atomic(&job.thumb, &job.thumb_path)?;
    if let Some(l) = &job.layers {
        if let Some(base) = &l.base_image {
            save_png_atomic(base, &l.base_path)?;
        }
        if let Some(dir) = l.layers_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        crate::config::write_atomic(&l.layers_path, l.ops_json.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_names_are_recognized() {
        assert_eq!(capture_time("2026-11-10_02-45-12.png"), Some(("2026", "11", "10", "02", "45")));
        assert!(capture_time("2026-11-10_02-45-12_2.png").is_some());
        for other in ["holiday.png", "2026-11-10_02-45-12.jpg", "2026-11-10_02-45-12_.png", "2026-11-10_02-45-12_x.png", "index.json"] {
            assert!(capture_time(other).is_none(), "{other}");
        }
    }

    #[test]
    fn captures_missing_from_the_index_come_back() {
        let dir = std::env::temp_dir().join(format!("snapkit-recover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        RgbaImage::new(30, 20).save(dir.join("2026-11-10_02-45-12.png")).unwrap();
        RgbaImage::new(40, 10).save(dir.join("2026-11-11_08-00-00.png")).unwrap();
        RgbaImage::new(5, 5).save(dir.join("holiday.png")).unwrap();
        // A damaged index: kept aside, and the captures are found again.
        std::fs::write(dir.join(INDEX_FILE), b"[{\"file\": \"2026-11").unwrap();
        let lib = Library::open(dir.clone());
        let files: Vec<&str> = lib.search("").iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, ["2026-11-11_08-00-00.png", "2026-11-10_02-45-12.png"], "newest first, no foreign files");
        assert_eq!((lib.search("")[1].width, lib.search("")[1].height), (30, 20));
        assert_eq!(lib.search("")[1].when, "10/11/2026 02:45");
        assert!(dir.join("index.json.bad").is_file());
        // The rebuilt index was saved and reads back the same.
        assert_eq!(Library::open(dir.clone()).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    const T1: LocalTime = (2026, 11, 10, 2, 45, 12);

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("snapkit-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(lib: &Library, file: &str) {
        std::fs::write(lib.path(file), b"x").unwrap();
    }

    #[test]
    fn neighbor_is_the_next_item_or_the_previous_at_the_end() {
        let files = ["a.png", "b.png", "c.png"];
        assert_eq!(neighbor(&files, "a.png"), Some("b.png"));
        assert_eq!(neighbor(&files, "c.png"), Some("b.png"), "at the end, goes back one");
        assert_eq!(neighbor(&["a.png"], "a.png"), None, "it was the only one");
        assert_eq!(neighbor(&files, "x.png"), None, "not in the list (filtered by the search)");
    }

    #[test]
    fn add_names_by_time_and_avoids_collisions() {
        let mut lib = Library::open(temp_dir("names"));
        let a = lib.add(T1, "Notepad".into(), 10, 10);
        touch(&lib, &a);
        let b = lib.add(T1, "Other".into(), 10, 10);
        assert_eq!(a, "2026-11-10_02-45-12.png");
        assert_eq!(b, "2026-11-10_02-45-12_2.png");
        assert_eq!(lib.search("")[0].file, b, "the newest comes first");
        assert_eq!(lib.search("")[0].when, "10/11/2026 02:45");
    }

    #[test]
    fn search_matches_title_and_date_ignoring_case() {
        let mut lib = Library::open(temp_dir("search"));
        lib.add(T1, "Report - Firefox".into(), 10, 10);
        lib.add((2026, 12, 1, 9, 0, 0), "Planilha.xlsx - Excel".into(), 10, 10);
        assert_eq!(lib.search("FIREFOX").len(), 1);
        assert_eq!(lib.search("  excel ").len(), 1);
        assert_eq!(lib.search("10/11").len(), 1);
        assert_eq!(lib.search("2026").len(), 2);
        assert!(lib.search("nonexistent").is_empty());
    }

    #[test]
    fn reopen_keeps_entries_and_prunes_missing_files() {
        let dir = temp_dir("reopen");
        let mut lib = Library::open(dir.clone());
        let kept = lib.add(T1, "kept".into(), 10, 10);
        touch(&lib, &kept);
        lib.add((2026, 11, 11, 0, 0, 0), "file deleted".into(), 10, 10);
        let lib = Library::open(dir);
        assert_eq!(lib.len(), 1);
        assert_eq!(lib.search("")[0].title, "kept");
    }

    #[test]
    fn tampered_index_cannot_escape_the_folder() {
        let dir = temp_dir("index");
        std::fs::write(dir.join("fora.png"), b"x").unwrap();
        let sub = dir.join("lib");
        std::fs::create_dir_all(&sub).unwrap();
        let index = r#"[{"file":"..\\fora.png","when":"","width":1,"height":1},{"file":"index.json","when":"","width":1,"height":1}]"#;
        std::fs::write(sub.join(INDEX_FILE), index).unwrap();
        assert_eq!(Library::open(sub).len(), 0);
    }

    #[test]
    fn corrupt_index_becomes_empty_library() {
        let dir = temp_dir("corrupted");
        std::fs::write(dir.join(INDEX_FILE), "{{{").unwrap();
        assert_eq!(Library::open(dir.clone()).len(), 0);
        assert!(dir.join("index.json.bad").is_file());
    }

    #[test]
    fn same_second_suffixes_sort_numerically() {
        assert!(capture_sort_key("2026-11-10_02-45-12_2.png") > capture_sort_key("2026-11-10_02-45-12.png"));
        assert!(capture_sort_key("2026-11-10_02-45-12_10.png") > capture_sort_key("2026-11-10_02-45-12_2.png"));
    }

    #[test]
    fn forget_and_set_size_update_the_index() {
        let dir = temp_dir("edit");
        let mut lib = Library::open(dir.clone());
        let f = lib.add(T1, "t".into(), 10, 10);
        touch(&lib, &f);
        lib.set_size(&f, 4, 3);
        assert_eq!(Library::open(dir.clone()).search("")[0].width, 4);
        lib.forget(&f);
        assert_eq!(Library::open(dir).len(), 0);
    }

    #[test]
    fn thumb_shrinks_wide_images_and_keeps_small_ones() {
        let t = make_thumb(&RgbaImage::new(2560, 1440));
        assert_eq!((t.width(), t.height()), (240, 135));
        let small = make_thumb(&RgbaImage::new(100, 300));
        assert_eq!((small.width(), small.height()), (100, 300));
    }

    #[test]
    fn writer_saves_image_and_thumb() {
        let lib = Library::open(temp_dir("write"));
        let img = RgbaImage::from_pixel(300, 200, image::Rgba([1, 2, 3, 255]));
        write_job(&Job {
            path: lib.path("a.png"),
            thumb_path: lib.thumb_path("a.png"),
            thumb: make_thumb(&img),
            image: img,
            layers: None,
        })
        .unwrap();
        assert_eq!(image::open(lib.path("a.png")).unwrap().width(), 300);
        assert_eq!(image::open(lib.thumb_path("a.png")).unwrap().width(), 240);
    }

    #[test]
    fn layers_and_base_paths_use_file_stem() {
        let lib = Library::open(temp_dir("layer-paths"));
        assert_eq!(lib.layers_path("2026-11-10_02-45-12.png"), lib.dir().join("2026-11-10_02-45-12.snap.json"));
        assert_eq!(lib.base_path("2026-11-10_02-45-12.png"), lib.dir().join("2026-11-10_02-45-12.base.png"));
    }

    #[test]
    fn save_load_and_remove_layers_roundtrip() {
        let dir = temp_dir("layers-roundtrip");
        let lib = Library::open(dir.clone());
        let file = "2026-11-10_02-45-12.png";
        let base = RgbaImage::from_pixel(20, 20, image::Rgba([10, 20, 30, 255]));
        let rendered = RgbaImage::from_pixel(20, 20, image::Rgba([255, 0, 0, 255]));
        let ops = vec![crate::render::Op::Line {
            from: (0.0, 10.0),
            to: (20.0, 10.0),
            color: [255, 0, 0, 255],
            width: 4.0,
            dashed: false,
        }];
        let ops_json = Library::save_layers_json(&ops).unwrap();

        write_job(&Job {
            path: lib.path(file),
            thumb_path: lib.thumb_path(file),
            thumb: make_thumb(&rendered),
            image: rendered,
            layers: Some(LayersJob {
                base_path: lib.base_path(file),
                base_image: Some(base.clone()),
                layers_path: lib.layers_path(file),
                ops_json,
            }),
        })
        .unwrap();

        let (loaded_base, loaded_ops) = lib.load_layers(file).expect("layers should load");
        assert_eq!(loaded_base, base);
        assert_eq!(loaded_ops, ops);

        // Wrong version is rejected.
        std::fs::write(lib.layers_path(file), r#"{"version":999,"ops":[]}"#).unwrap();
        assert!(lib.load_layers(file).is_none());

        lib.remove_layers(file);
        assert!(!lib.layers_path(file).exists());
        assert!(!lib.base_path(file).exists());
        assert!(lib.load_layers(file).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_lost_files_ignores_sidecars() {
        let dir = temp_dir("ignore-sidecars");
        RgbaImage::new(30, 20).save(dir.join("2026-11-10_02-45-12.png")).unwrap();
        RgbaImage::new(30, 20).save(dir.join("2026-11-10_02-45-12.base.png")).unwrap();
        RgbaImage::new(30, 20).save(dir.join("2026-11-10_02-45-12.tmp.png")).unwrap();
        std::fs::write(dir.join("2026-11-10_02-45-12.snap.json"), b"{\"version\":1,\"ops\":[]}").unwrap();

        assert!(!is_plain_file_name("2026-11-10_02-45-12.base.png"));
        assert!(!is_plain_file_name("2026-11-10_02-45-12.tmp.png"));
        assert!(!is_plain_file_name("2026-11-10_02-45-12.snap.json"));
        assert!(is_plain_file_name("2026-11-10_02-45-12.png"));

        let lib = Library::open(dir.clone());
        let files: Vec<&str> = lib.search("").iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, ["2026-11-10_02-45-12.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn media_entries_filter_counts_and_thumb_paths() {
        let dir = temp_dir("media-filter");
        let mut lib = Library::open(dir.clone());
        let img = lib.add(T1, "Screenshot".into(), 1920, 1080);
        touch(&lib, &img);
        let vid = lib.add_media((2026, 11, 10, 2, 46, 0), "mp4", "Demo recording".into(), 1280, 720, 12.5);
        touch(&lib, &vid);
        let gif = lib.add_media((2026, 11, 10, 2, 47, 0), "gif", "Clip animation".into(), 640, 360, 4.0);
        touch(&lib, &gif);

        assert_eq!(vid, "2026-11-10_02-46-00.mp4");
        assert_eq!(gif, "2026-11-10_02-47-00.gif");
        assert_eq!(
            lib.thumb_path(&vid),
            lib.dir().join(THUMB_DIR).join("2026-11-10_02-46-00.png")
        );
        assert_eq!(
            lib.thumb_path(&gif),
            lib.dir().join(THUMB_DIR).join("2026-11-10_02-47-00.png")
        );

        assert_eq!(lib.counts(), (3, 2, 1));
        assert_eq!(lib.search_filtered("", MediaFilter::All).len(), 3);
        assert_eq!(lib.search_filtered("", MediaFilter::Videos).len(), 1);
        assert_eq!(lib.search_filtered("", MediaFilter::Videos)[0].file, vid);
        assert_eq!(lib.search_filtered("", MediaFilter::Images).len(), 2);

        lib.update_media(&vid, 800, 600, 8.25);
        let updated = lib.get(&vid).unwrap();
        assert_eq!((updated.width, updated.height, updated.duration_secs), (800, 600, 8.25));
        assert!(updated.is_video());
        assert!(!updated.is_image());
        assert!(lib.get(&gif).unwrap().is_gif());
        assert!(lib.get(&gif).unwrap().is_image());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_lost_files_recovers_mp4_and_ignores_temp_recording_files() {
        let dir = temp_dir("recover-mp4");
        RgbaImage::new(30, 20).save(dir.join("2026-11-10_02-45-12.png")).unwrap();
        std::fs::write(dir.join("2026-11-10_02-46-00.mp4"), b"fake-mp4").unwrap();
        std::fs::write(dir.join("2026-11-10_02-46-00.video.mp4"), b"temp-video").unwrap();
        std::fs::write(dir.join("2026-11-10_02-46-00.tmp.mp4"), b"temp-mp4").unwrap();
        std::fs::write(dir.join("2026-11-10_02-46-00.part1.mp4"), b"part1").unwrap();
        std::fs::write(dir.join("2026-11-10_02-46-00.audio.wav"), b"wav").unwrap();
        std::fs::write(dir.join("2026-11-10_02-46-00.parts.txt"), b"parts").unwrap();

        assert!(is_plain_file_name("2026-11-10_02-46-00.mp4"));
        assert!(is_plain_file_name("2026-11-10_02-46-00.gif"));
        assert!(!is_plain_file_name("2026-11-10_02-46-00.video.mp4"));
        assert!(!is_plain_file_name("2026-11-10_02-46-00.tmp.mp4"));
        assert!(!is_plain_file_name("2026-11-10_02-46-00.part1.mp4"));
        assert!(!is_plain_file_name("2026-11-10_02-46-00.audio.wav"));
        assert!(!is_plain_file_name("2026-11-10_02-46-00.parts.txt"));

        let lib = Library::open(dir.clone());
        let files: Vec<&str> = lib.search("").iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, ["2026-11-10_02-46-00.mp4", "2026-11-10_02-45-12.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn combine_images_stitches_vertically_and_horizontally() {
        let dir = temp_dir("combine");
        let p1 = dir.join("one.png");
        let p2 = dir.join("two.png");
        RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255])).save(&p1).unwrap();
        RgbaImage::from_pixel(20, 30, image::Rgba([0, 255, 0, 255])).save(&p2).unwrap();

        assert!(combine_images(std::slice::from_ref(&p1), true, 4).is_err());

        let vert = combine_images(&[p1.clone(), p2.clone()], true, 6).unwrap();
        assert_eq!((vert.width(), vert.height()), (40, 20 + 30 + 6));
        assert_eq!(vert.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(vert.get_pixel(20, 22).0, [30, 30, 34, 255]); // gap
        assert_eq!(vert.get_pixel(5, 30).0, [30, 30, 34, 255]); // left margin of centered narrower image
        assert_eq!(vert.get_pixel(15, 30).0, [0, 255, 0, 255]); // centered second image

        let horiz = combine_images(&[p1, p2], false, 4).unwrap();
        assert_eq!((horiz.width(), horiz.height()), (40 + 20 + 4, 30));
        assert_eq!(horiz.get_pixel(0, 2).0, [30, 30, 34, 255]); // top margin of centered shorter image
        assert_eq!(horiz.get_pixel(10, 15).0, [255, 0, 0, 255]); // centered first image
        assert_eq!(horiz.get_pixel(42, 15).0, [30, 30, 34, 255]); // gap
        assert_eq!(horiz.get_pixel(50, 15).0, [0, 255, 0, 255]); // second image

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn favorites_tags_metadata_and_legacy_index_compat() {
        let dir = temp_dir("favorites-tags");
        // Write a legacy index.json without `favorite` or `tags`.
        RgbaImage::new(16, 16).save(dir.join("2026-11-10_02-45-12.png")).unwrap();
        RgbaImage::new(16, 16).save(dir.join("2026-11-11_09-00-00.png")).unwrap();
        let legacy_json = r#"[
            {"file":"2026-11-11_09-00-00.png","when":"11/11/2026 09:00","title":"Old Two","width":16,"height":16},
            {"file":"2026-11-10_02-45-12.png","when":"10/11/2026 02:45","title":"Old One","width":16,"height":16}
        ]"#;
        std::fs::write(dir.join(INDEX_FILE), legacy_json).unwrap();

        let mut lib = Library::open(dir.clone());
        assert_eq!(lib.len(), 2);
        assert_eq!(lib.favorite_count(), 0);
        assert!(lib.search_filtered("", MediaFilter::Favorites).is_empty());

        assert_eq!(lib.toggle_favorite("2026-11-10_02-45-12.png"), Some(true));
        assert_eq!(lib.favorite_count(), 1);
        assert_eq!(lib.search_filtered("", MediaFilter::Favorites).len(), 1);
        assert_eq!(lib.toggle_favorite("missing.png"), None);

        assert!(lib.set_metadata(
            "2026-11-10_02-45-12.png",
            "  Login Bug  ".into(),
            vec!["#bug".into(), "UI".into(), "bug".into(), "  ".into(), "#sprint-4".into()],
        ));
        assert!(!lib.set_metadata("missing.png", "X".into(), vec![]));

        let reopened = Library::open(dir.clone());
        assert_eq!(reopened.favorite_count(), 1);
        let e = reopened.get("2026-11-10_02-45-12.png").unwrap();
        assert!(e.favorite);
        assert_eq!(e.title, "Login Bug");
        assert_eq!(e.tags, vec!["bug", "UI", "sprint-4"]);

        assert_eq!(reopened.search("#bug").len(), 1);
        assert_eq!(reopened.search("sprint-4").len(), 1);
        assert_eq!(reopened.search_filtered("#ui", MediaFilter::Favorites).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
