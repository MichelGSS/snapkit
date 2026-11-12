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

use image::RgbaImage;
use serde::{Deserialize, Serialize};

const INDEX_FILE: &str = "index.json";
const THUMB_DIR: &str = ".miniaturas";
/// Maximum thumbnail width, in pixels.
pub const THUMB_WIDTH: u32 = 240;

/// Local date and time, already split (year, month, day, hour, minute, second).
pub type LocalTime = (u16, u16, u16, u16, u16, u16);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Name of the PNG file inside the library folder.
    pub file: String,
    /// Date to show and search, such as "10/11/2026 02:45".
    pub when: String,
    /// Title of the window that was in the foreground at capture time.
    #[serde(default)]
    pub title: String,
    pub width: u32,
    pub height: u32,
}

impl Entry {
    fn matches(&self, query_lower: &str) -> bool {
        self.title.to_lowercase().contains(query_lower)
            || self.when.contains(query_lower)
            || self.file.to_lowercase().contains(query_lower)
    }
}

pub struct Library {
    dir: PathBuf,
    entries: Vec<Entry>,
}

impl Library {
    /// Default folder: the user's `Pictures\SnapKit`.
    pub fn default_dir() -> PathBuf {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        home.join("Pictures").join("SnapKit")
    }

    /// Opens the library, dropping from the index the entries whose file is gone.
    pub fn open(dir: PathBuf) -> Self {
        let mut entries: Vec<Entry> = std::fs::read_to_string(dir.join(INDEX_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        entries.retain(|e| is_plain_file_name(&e.file) && dir.join(&e.file).is_file());
        Self { dir, entries }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    pub fn thumb_path(&self, file: &str) -> PathBuf {
        self.dir.join(THUMB_DIR).join(file)
    }

    /// Entries that contain the search text (case-insensitive), newest first.
    pub fn search(&self, query: &str) -> Vec<&Entry> {
        let q = query.trim().to_lowercase();
        self.entries.iter().filter(|e| q.is_empty() || e.matches(&q)).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Registers a new capture and returns the file name reserved for it. The PNG itself is written
    /// later, by [`Writer`].
    pub fn add(&mut self, now: LocalTime, title: String, width: u32, height: u32) -> String {
        let (y, mo, d, h, mi, s) = now;
        let base = format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}");
        let mut file = format!("{base}.png");
        let mut n = 2;
        // Two captures in the same second get a suffix.
        while self.entries.iter().any(|e| e.file == file) || self.path(&file).exists() {
            file = format!("{base}_{n}.png");
            n += 1;
        }
        let when = format!("{d:02}/{mo:02}/{y:04} {h:02}:{mi:02}");
        self.entries.insert(0, Entry { file: file.clone(), when, title, width, height });
        self.save_index();
        file
    }

    /// Updates the recorded size after an edit (a crop, for example).
    pub fn set_size(&mut self, file: &str, width: u32, height: u32) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.file == file) {
            if (e.width, e.height) != (width, height) {
                (e.width, e.height) = (width, height);
                self.save_index();
            }
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
            let _ = std::fs::write(self.dir.join(INDEX_FILE), text);
        }
    }
}

/// A tampered index must not point outside the library folder.
fn is_plain_file_name(file: &str) -> bool {
    !file.is_empty() && !file.contains(['/', '\\', ':']) && file != "." && file != ".."
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

struct Job {
    path: PathBuf,
    thumb_path: PathBuf,
    image: RgbaImage,
    thumb: RgbaImage,
}

/// Writes PNGs on a single thread, in the order requested, so that an edit saved right after the
/// capture is never overwritten by the earlier write.
pub struct Writer {
    tx: Sender<Job>,
}

impl Writer {
    pub fn spawn() -> Self {
        let (tx, rx) = channel::<Job>();
        std::thread::spawn(move || {
            for job in rx {
                let _ = write_job(&job);
            }
        });
        Self { tx }
    }

    pub fn save(&self, library: &Library, file: &str, image: RgbaImage, thumb: RgbaImage) {
        let _ = self.tx.send(Job {
            path: library.path(file),
            thumb_path: library.thumb_path(file),
            image,
            thumb,
        });
    }
}

fn write_job(job: &Job) -> Result<(), String> {
    for path in [&job.path, &job.thumb_path] {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    job.image.save(&job.path).map_err(|e| e.to_string())?;
    job.thumb.save(&job.thumb_path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let index = r#"[{"file":"..\\fora.png","when":"","width":1,"height":1}]"#;
        std::fs::write(sub.join(INDEX_FILE), index).unwrap();
        assert_eq!(Library::open(sub).len(), 0);
    }

    #[test]
    fn corrupt_index_becomes_empty_library() {
        let dir = temp_dir("corrupted");
        std::fs::write(dir.join(INDEX_FILE), "{{{").unwrap();
        assert_eq!(Library::open(dir).len(), 0);
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
        })
        .unwrap();
        assert_eq!(image::open(lib.path("a.png")).unwrap().width(), 300);
        assert_eq!(image::open(lib.thumb_path("a.png")).unwrap().width(), 240);
    }
}
