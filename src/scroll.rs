//! Scrolling capture, in two modes:
//!
//! * **Automatic** ([`run`]): the program scrolls the area by itself (vertically, horizontally or
//!   both), takes a shot at each step and stitches the shots together.
//! * **Panoramic** ([`start_panoramic`]): the user scrolls freely and the program builds the image
//!   from whatever passes through the area.
//!
//! Stitching needs to find out how far the content moved between two shots. There are two methods:
//!
//! * **Row signature** ([`find_motion`]): fast and exact, but only works when the whole row moves
//!   together. It also recognizes a fixed header and footer.
//! * **Pixel voting** ([`estimate_shift`]): compares pixels sampled along several thin strips and
//!   picks the shift at which the most "informative" pixels match. It works even with parts of the
//!   area standing still (a fixed sidebar, a fixed wallpaper behind the content) and tolerates
//!   small color differences.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use image::RgbaImage;

use crate::capture;
use crate::win::{self, PxRect};

/// Safety limits for endless pages.
const MAX_FRAMES: usize = 80;
const MAX_LENGTH: u32 = 32_000;
/// Maximum difference per color channel for two pixels to count as equal in the tolerant comparison
/// (antialiasing and compression change 1 or 2 levels).
const COLOR_TOLERANCE: i16 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    Vertical,
    Horizontal,
    Both,
}

#[derive(Debug, PartialEq)]
enum Motion {
    /// Both shots are the same: the scrolling reached the end.
    None,
    /// The content moved up `offset` rows; `top`/`bottom` rows at the edges didn't move.
    Moved { top: usize, bottom: usize, offset: usize },
    /// Couldn't find the overlap between the shots.
    Lost,
}

/// One signature per row. The right-hand strip is ignored because the scroll bar changes position
/// in every shot.
fn row_signatures(img: &RgbaImage) -> Vec<u64> {
    let w = img.width() as usize;
    let usable = usable_width(w);
    let raw = img.as_raw();
    (0..img.height() as usize)
        .map(|y| {
            let row = &raw[y * w * 4..(y * w + usable) * 4];
            // FNV-1a over RGB, every other pixel.
            let mut h: u64 = 0xcbf29ce484222325;
            for px in row.chunks_exact(4).step_by(2) {
                for &c in &px[..3] {
                    h = (h ^ c as u64).wrapping_mul(0x100000001b3);
                }
            }
            h
        })
        .collect()
}

fn usable_width(w: usize) -> usize {
    if w >= 120 { w - (w / 20).max(24) } else { w }
}

fn find_motion(prev: &[u64], next: &[u64]) -> Motion {
    let h = prev.len();
    if h != next.len() {
        return Motion::Lost;
    }
    let top = prev.iter().zip(next).take_while(|(a, b)| a == b).count();
    if top == h {
        return Motion::None;
    }
    let bottom = prev[top..].iter().rev().zip(next[top..].iter().rev()).take_while(|(a, b)| a == b).count();
    let end = h - bottom;
    let band = end - top;
    if band < 16 {
        return Motion::Lost;
    }
    // Rows equal to their neighbor (plain background) match anywhere and prove nothing.
    let informative = |y: usize| y > 0 && prev[y] != prev[y - 1];

    let min_compared = (band / 8).max(12);
    let mut best: Option<(usize, usize)> = None; // (informative rows that matched, shift)
    for offset in 1..band {
        let compared = band - offset;
        if compared < min_compared {
            break;
        }
        let (mut matches, mut info) = (0, 0);
        for y in top..end - offset {
            if next[y] == prev[y + offset] {
                matches += 1;
                if informative(y + offset) {
                    info += 1;
                }
            }
        }
        if matches * 10 >= compared * 9 && info >= 3 && best.is_none_or(|(b, _)| info > b) {
            best = Some((info, offset));
        }
    }
    match best {
        Some((_, offset)) => Motion::Moved { top, bottom, offset },
        None => Motion::Lost,
    }
}

type Rgb = [u8; 3];

fn similar(a: Rgb, b: Rgb) -> bool {
    (0..3).all(|c| (a[c] as i16 - b[c] as i16).abs() <= COLOR_TOLERANCE)
}

/// Strips one pixel wide along the axis of motion: columns for vertical motion, rows for
/// horizontal. Only the indices `from..to` of that axis are read.
fn sample_lines(img: &RgbaImage, vertical: bool, from: usize, to: usize) -> Vec<Vec<Rgb>> {
    const LINES: usize = 28;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let across = if vertical { w } else { h };
    let raw = img.as_raw();
    (0..LINES)
        .map(|k| {
            // Spread across the area, away from the edges.
            let pos = (across * (2 * k + 1)) / (2 * LINES);
            (from..to)
                .map(|i| {
                    let (x, y) = if vertical { (pos, i) } else { (i, pos) };
                    let o = (y * w + x) * 4;
                    [raw[o], raw[o + 1], raw[o + 2]]
                })
                .collect()
        })
        .collect()
}

/// Shift `d` such that `next[i] ≈ prev[i + d]` for most informative pixels. Positive: the content
/// moved toward the start of the axis (the page was scrolled forward).
///
/// Strips that stand completely still don't vote (a fixed part of the screen). A pixel only counts
/// as evidence if it differs from its neighbor in the previous shot: a plain background matches
/// anywhere.
fn estimate_shift(prev: &[Vec<Rgb>], next: &[Vec<Rgb>], allow_backward: bool) -> Option<i64> {
    let len = prev.first()?.len();
    let moving: Vec<usize> = (0..prev.len()).filter(|&k| prev[k] != next[k]).collect();
    if moving.is_empty() || len < 24 {
        return None;
    }
    let informative: Vec<Vec<bool>> = moving
        .iter()
        .map(|&k| (0..len).map(|i| i > 0 && !similar(prev[k][i], prev[k][i - 1])).collect())
        .collect();

    let min_overlap = (len / 8).max(12) as i64;
    let max_shift = len as i64 - min_overlap;
    let mut best: Option<(u32, i64)> = None; // (informative pixels that matched, shift)
    // In order of increasing |d|: on a tie, the smallest motion wins.
    for magnitude in 1..=max_shift {
        for d in [magnitude, -magnitude] {
            if d < 0 && !allow_backward {
                continue;
            }
            let (lo, hi) = ((-d).max(0) as usize, (len as i64 - d.max(0)) as usize);
            let (mut score, mut possible) = (0u32, 0u32);
            for (m, &k) in moving.iter().enumerate() {
                let (p, n, info) = (&prev[k], &next[k], &informative[m]);
                for i in lo..hi {
                    let j = (i as i64 + d) as usize;
                    if info[j] {
                        possible += 1;
                        if similar(n[i], p[j]) {
                            score += 1;
                        }
                    }
                }
            }
            // At least a third of the available evidence has to agree.
            if score >= 24 && score * 3 >= possible && best.is_none_or(|(b, _)| score > b) {
                best = Some((score, d));
            }
        }
    }
    best.map(|(_, d)| d)
}

#[derive(Debug, PartialEq)]
pub enum Push {
    Added,
    NoMovement,
    Lost,
}

/// Accumulates shots from a top-to-bottom scroll into a single image.
pub struct Stitcher {
    width: u32,
    frame_height: u32,
    /// RGBA rows already stitched, top to bottom.
    rows: Vec<u8>,
    last: RgbaImage,
    last_sigs: Vec<u64>,
}

impl Stitcher {
    pub fn new(first: &RgbaImage) -> Self {
        Self {
            width: first.width(),
            frame_height: first.height(),
            rows: first.as_raw().clone(),
            last_sigs: row_signatures(first),
            last: first.clone(),
        }
    }

    pub fn height(&self) -> u32 {
        (self.rows.len() / (self.width as usize * 4)) as u32
    }

    pub fn push(&mut self, next: &RgbaImage) -> Push {
        if next.width() != self.width || next.height() != self.frame_height {
            return Push::Lost;
        }
        let sigs = row_signatures(next);
        let (bottom, offset) = match find_motion(&self.last_sigs, &sigs) {
            Motion::None => return Push::NoMovement,
            Motion::Moved { bottom, offset, .. } => (bottom, offset),
            Motion::Lost => {
                // The fixed edges (identical rows at the top and bottom) are left out of the vote.
                let h = self.frame_height as usize;
                let top = self.last_sigs.iter().zip(&sigs).take_while(|(a, b)| a == b).count();
                let bottom = self.last_sigs[top..].iter().rev().zip(sigs[top..].iter().rev()).take_while(|(a, b)| a == b).count();
                let (prev, cur) = (
                    sample_lines(&self.last, true, top, h - bottom),
                    sample_lines(next, true, top, h - bottom),
                );
                match estimate_shift(&prev, &cur, false) {
                    Some(offset) => (bottom, offset as usize),
                    None => return Push::Lost,
                }
            }
        };
        let stride = self.width as usize * 4;
        let h = self.frame_height as usize;
        // Removes the fixed footer from the previous shot and adds the new content plus the current
        // footer.
        self.rows.truncate(self.rows.len() - bottom * stride);
        let from = (h - bottom - offset) * stride;
        self.rows.extend_from_slice(&next.as_raw()[from..]);
        self.last_sigs = sigs;
        self.last = next.clone();
        Push::Added
    }

    pub fn finish(self) -> RgbaImage {
        let height = self.height();
        RgbaImage::from_raw(self.width, height, self.rows).expect("whole rows by construction")
    }
}

/// Swaps rows and columns. Lets the vertical stitching be reused horizontally.
fn transpose(img: &RgbaImage) -> RgbaImage {
    RgbaImage::from_fn(img.height(), img.width(), |x, y| *img.get_pixel(y, x))
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Vertical,
    Horizontal,
}

impl Axis {
    /// Turns the wheel to move forward (`forward`) or back along the axis.
    fn step(self, notches: i32, forward: bool) {
        match (self, forward) {
            (Axis::Vertical, true) => win::wheel(-notches),
            (Axis::Vertical, false) => win::wheel(notches),
            (Axis::Horizontal, true) => win::hwheel(notches),
            (Axis::Horizontal, false) => win::hwheel(-notches),
        }
    }

    fn grab(self, rect: PxRect) -> Result<RgbaImage, String> {
        let img = grab_stable(rect)?;
        Ok(if self == Axis::Horizontal { transpose(&img) } else { img })
    }
}

/// Takes shots until two in a row come out the same (end of the scroll animation).
fn grab_stable(rect: PxRect) -> Result<RgbaImage, String> {
    let mut last = capture::grab_rect(rect)?;
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(70));
        let now = capture::grab_rect(rect)?;
        if now == last {
            return Ok(now);
        }
        last = now;
    }
    Ok(last)
}

/// Takes the area back to the start (top or left). Returns `false` if Esc was pressed.
fn scroll_to_start(rect: PxRect, axis: Axis) -> Result<bool, String> {
    let mut before = capture::grab_rect(rect)?;
    for _ in 0..60 {
        if win::escape_pressed() {
            return Ok(false);
        }
        axis.step(10, false);
        std::thread::sleep(Duration::from_millis(140));
        let after = capture::grab_rect(rect)?;
        if after == before {
            break;
        }
        before = after;
    }
    grab_stable(rect)?;
    Ok(true)
}

/// Scrolls the area along one axis to the end, stitching. The image comes back in the normal
/// orientation.
fn strip(rect: PxRect, axis: Axis) -> Result<(RgbaImage, String), String> {
    let first = axis.grab(rect)?;
    let mut stitcher = Stitcher::new(&first);
    // Short areas need small steps for the shots to overlap.
    let mut notches = if first.height() < 500 { 1 } else { 3 };
    let mut reason = t!("limite de fotos atingido", "shot limit reached").to_owned();
    let mut still = 0;

    for _ in 0..MAX_FRAMES {
        if win::escape_pressed() {
            reason = t!("interrompida com Esc", "stopped with Esc").to_owned();
            break;
        }
        axis.step(notches, true);
        std::thread::sleep(Duration::from_millis(120));
        let next = axis.grab(rect)?;
        match stitcher.push(&next) {
            Push::Added => still = 0,
            Push::NoMovement => {
                // Slow programs sometimes ignore a step: try again before concluding it's over.
                still += 1;
                if still >= 3 {
                    reason = t!("chegou ao fim da rolagem", "reached the end of the scroll").to_owned();
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Push::Lost if notches > 1 => {
                // Step too large: go back and try again with one-notch steps.
                axis.step(notches, false);
                std::thread::sleep(Duration::from_millis(120));
                axis.grab(rect)?;
                notches = 1;
            }
            Push::Lost => {
                reason = t!(
                    "parou: o conteúdo mudou de um jeito que não deu para costurar",
                    "stopped: the content changed in a way that could not be stitched"
                )
                .to_owned();
                break;
            }
        }
        if stitcher.height() >= MAX_LENGTH {
            reason = tf!("parou no limite de {MAX_LENGTH} px", "stopped at the {MAX_LENGTH} px limit");
            break;
        }
    }
    let out = stitcher.finish();
    Ok((if axis == Axis::Horizontal { transpose(&out) } else { out }, reason))
}

/// Scrolls on both axes: a full vertical strip for each horizontal position, then joins the strips
/// side by side.
fn both_axes(rect: PxRect) -> Result<(RgbaImage, String), String> {
    let mut columns: Vec<RgbaImage> = Vec::new();
    let mut reason;
    loop {
        let (column, why) = strip(rect, Axis::Vertical)?;
        columns.push(column);
        reason = why;
        if reason.contains("Esc") || columns.len() >= 12 {
            break;
        }
        if !scroll_to_start(rect, Axis::Vertical)? {
            reason = t!("interrompida com Esc", "stopped with Esc").to_owned();
            break;
        }
        let before = grab_stable(rect)?;
        Axis::Horizontal.step(3, true);
        std::thread::sleep(Duration::from_millis(150));
        if grab_stable(rect)? == before {
            break; // there's nothing more to the right
        }
    }
    // The strips can differ in height by a few pixels; trims them to the shortest.
    let height = columns.iter().map(|c| c.height()).min().unwrap_or(0);
    let mut iter = columns.iter().map(|c| transpose(&image::imageops::crop_imm(c, 0, 0, c.width(), height).to_image()));
    let first = iter.next().ok_or(t!("nenhuma faixa capturada", "no strip captured"))?;
    let mut stitcher = Stitcher::new(&first);
    for column in iter {
        if stitcher.push(&column) == Push::Lost {
            reason = t!("parou: não deu para juntar as faixas na horizontal", "stopped: the strips could not be joined horizontally").to_owned();
            break;
        }
    }
    Ok((transpose(&stitcher.finish()), reason))
}

/// Automatic capture: scrolls the area `rect` (screen coordinates) in the requested direction, from
/// the start, and returns the stitched image and why the capture stopped.
pub fn run(rect: PxRect, direction: Direction) -> Result<(RgbaImage, String), String> {
    let original_cursor = win::cursor_pos();
    let center = ((rect.0 + rect.2) / 2, (rect.1 + rect.3) / 2);
    // The mouse wheel goes to the window under the cursor; bringing it to the front keeps another
    // window from covering the area in the middle of the capture.
    win::focus_window_at(center);
    win::set_cursor_pos(center);
    std::thread::sleep(Duration::from_millis(200));

    let result = (|| {
        let axes: &[Axis] = match direction {
            Direction::Vertical => &[Axis::Vertical],
            Direction::Horizontal => &[Axis::Horizontal],
            Direction::Both => &[Axis::Vertical, Axis::Horizontal],
        };
        for &axis in axes {
            if !scroll_to_start(rect, axis)? {
                return Ok((capture::grab_rect(rect)?, t!("interrompida com Esc", "stopped with Esc").to_owned()));
            }
        }
        match direction {
            Direction::Vertical => strip(rect, Axis::Vertical),
            Direction::Horizontal => strip(rect, Axis::Horizontal),
            Direction::Both => both_axes(rect),
        }
    })();
    win::set_cursor_pos(original_cursor);
    result
}

/// Panoramic capture mosaic: each shot is pasted at the position the content moved to, in any of
/// the four directions.
pub struct Panorama {
    canvas: RgbaImage,
    /// Position of the last shot inside the mosaic.
    pos: (u32, u32),
    last: RgbaImage,
    last_rows: Vec<u64>,
}

impl Panorama {
    pub fn new(first: &RgbaImage) -> Self {
        Self { canvas: first.clone(), pos: (0, 0), last: first.clone(), last_rows: row_signatures(first) }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.canvas.width(), self.canvas.height())
    }

    /// How far the content moved from `last` to `next`: (dx, dy) in mosaic pixels.
    fn displacement(&self, next: &RgbaImage, next_rows: &[u64]) -> Option<(i64, i64)> {
        // Fast path: the whole area scrolled vertically.
        if let Motion::Moved { offset, top: 0, bottom: 0 } = find_motion(&self.last_rows, next_rows) {
            return Some((0, offset as i64));
        }
        if let Motion::Moved { offset, top: 0, bottom: 0 } = find_motion(next_rows, &self.last_rows) {
            return Some((0, -(offset as i64)));
        }
        let (w, h) = (next.width() as usize, next.height() as usize);
        let vertical = estimate_shift(
            &sample_lines(&self.last, true, 0, h),
            &sample_lines(next, true, 0, h),
            true,
        );
        if let Some(dy) = vertical {
            return Some((0, dy));
        }
        let horizontal = estimate_shift(
            &sample_lines(&self.last, false, 0, w),
            &sample_lines(next, false, 0, w),
            true,
        );
        horizontal.map(|dx| (dx, 0))
    }

    /// Adds a shot. `Lost` means it was ignored (scrolling too fast).
    pub fn push(&mut self, next: &RgbaImage) -> Push {
        if next.dimensions() != self.last.dimensions() {
            return Push::Lost;
        }
        if *next == self.last {
            return Push::NoMovement;
        }
        let next_rows = row_signatures(next);
        let Some((dx, dy)) = self.displacement(next, &next_rows) else { return Push::Lost };
        let (x, y) = (self.pos.0 as i64 + dx, self.pos.1 as i64 + dy);
        // Grows the mosaic on the side where the shot goes past the edge.
        let (grow_left, grow_top) = ((-x).max(0) as u32, (-y).max(0) as u32);
        let new_w = (x.max(0) as u32 + next.width()).max(self.canvas.width()) + grow_left;
        let new_h = (y.max(0) as u32 + next.height()).max(self.canvas.height()) + grow_top;
        if new_w > MAX_LENGTH || new_h > MAX_LENGTH {
            return Push::Lost;
        }
        if (new_w, new_h) != self.canvas.dimensions() {
            let mut bigger = RgbaImage::from_pixel(new_w, new_h, image::Rgba([255, 255, 255, 255]));
            image::imageops::replace(&mut bigger, &self.canvas, grow_left as i64, grow_top as i64);
            self.canvas = bigger;
        }
        self.pos = ((x + grow_left as i64) as u32, (y + grow_top as i64) as u32);
        image::imageops::replace(&mut self.canvas, next, self.pos.0 as i64, self.pos.1 as i64);
        self.last = next.clone();
        self.last_rows = next_rows;
        Push::Added
    }

    pub fn finish(self) -> RgbaImage {
        self.canvas
    }
}

/// A panoramic capture in progress.
pub struct PanoramicSession {
    stop: Arc<AtomicBool>,
    size: Arc<Mutex<(u32, u32)>>,
    lost: Arc<AtomicU32>,
    worker: JoinHandle<Result<RgbaImage, String>>,
}

/// Starts watching the area `rect`; the user scrolls and the shots get pasted in.
pub fn start_panoramic(rect: PxRect) -> PanoramicSession {
    let stop = Arc::new(AtomicBool::new(false));
    let size = Arc::new(Mutex::new((0, 0)));
    let lost = Arc::new(AtomicU32::new(0));
    let (stop2, size2, lost2) = (stop.clone(), size.clone(), lost.clone());
    let worker = std::thread::spawn(move || {
        let mut pano = Panorama::new(&capture::grab_rect(rect)?);
        *size2.lock().unwrap() = pano.size();
        while !stop2.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(40));
            let frame = capture::grab_rect(rect)?;
            match pano.push(&frame) {
                Push::Added => *size2.lock().unwrap() = pano.size(),
                Push::Lost => {
                    lost2.fetch_add(1, Ordering::Relaxed);
                }
                Push::NoMovement => {}
            }
        }
        Ok(pano.finish())
    });
    PanoramicSession { stop, size, lost, worker }
}

impl PanoramicSession {
    /// Current size of the mosaic, to show progress.
    pub fn size(&self) -> (u32, u32) {
        *self.size.lock().unwrap()
    }

    /// Shots that couldn't be fitted so far.
    pub fn lost(&self) -> u32 {
        self.lost.load(Ordering::Relaxed)
    }

    /// Stops watching and returns the mosaic and a note for the user.
    pub fn finish(self) -> Result<(RgbaImage, String), String> {
        self.stop.store(true, Ordering::Relaxed);
        let lost = self.lost.load(Ordering::Relaxed);
        let image = self.worker.join().map_err(|_| t!("a captura panorâmica falhou", "the panoramic capture failed").to_owned())??;
        let note = if lost == 0 {
            t!("panorâmica concluída", "panoramic finished").to_owned()
        } else {
            tf!(
                "panorâmica concluída; {lost} fotos não encaixaram (role mais devagar)",
                "panoramic finished; {lost} shots did not fit (scroll more slowly)"
            )
        };
        Ok((image, note))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 200;

    /// Deterministic "random" color for the point (x, y) of a made-up page.
    fn page_pixel(x: u32, y: u32) -> [u8; 4] {
        let v = (x / 8).wrapping_mul(2654435761).wrapping_add(y.wrapping_mul(40503)) >> 7;
        [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]
    }

    /// A shot of `h` rows of the page starting at `scroll`, with a fixed header and footer and a
    /// "scroll bar" on the right that changes in every shot.
    fn frame(scroll: u32, h: u32, header: u32, footer: u32) -> RgbaImage {
        RgbaImage::from_fn(W, h, |x, y| {
            if x >= W - 10 {
                let thumb = y.abs_diff(scroll / 4) < 20;
                return image::Rgba(if thumb { [90, 90, 90, 255] } else { [230, 230, 230, 255] });
            }
            if y < header {
                image::Rgba([10, 20, (y * 7) as u8, 255])
            } else if y >= h - footer {
                image::Rgba([200, (y * 3) as u8, 30, 255])
            } else {
                image::Rgba(page_pixel(x, scroll + y - header))
            }
        })
    }

    /// Window onto a 2D page with no fixed edges, for the panoramic tests.
    fn view(x0: u32, y0: u32) -> RgbaImage {
        RgbaImage::from_fn(160, 120, |x, y| {
            let (px, py) = (x0 + x, y0 + y);
            let v = px.wrapping_mul(2654435761).wrapping_add(py.wrapping_mul(40503)) >> 5;
            image::Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, 255])
        })
    }

    fn stitch(scrolls: &[u32], h: u32, header: u32, footer: u32) -> RgbaImage {
        let mut s = Stitcher::new(&frame(scrolls[0], h, header, footer));
        for &sc in &scrolls[1..] {
            assert_eq!(s.push(&frame(sc, h, header, footer)), Push::Added, "scroll {sc}");
        }
        s.finish()
    }

    #[test]
    fn stitches_plain_page() {
        let out = stitch(&[0, 120, 240, 300], 300, 0, 0);
        assert_eq!(out.height(), 600);
        for y in [0, 150, 299, 300, 450, 599] {
            assert_eq!(out.get_pixel(40, y).0, page_pixel(40, y), "row {y}");
        }
    }

    #[test]
    fn keeps_sticky_header_and_footer_once() {
        let (h, header, footer) = (300, 40, 30);
        let out = stitch(&[0, 100, 200], h, header, footer);
        assert_eq!(out.height(), h + 200);
        assert_eq!(out.get_pixel(40, 10).0, [10, 20, 70, 255], "header at the top");
        // The body is continuous: row y of the image is row (y - header) of the page.
        for y in [header, 200, 350, h + 200 - footer - 1] {
            assert_eq!(out.get_pixel(40, y).0, page_pixel(40, y - header), "row {y}");
        }
        let last = out.height() - 1;
        assert_eq!(out.get_pixel(40, last).0[0], 200, "footer only at the end");
        assert_ne!(out.get_pixel(40, h - 1).0[0], 200, "no footer in the middle");
    }

    #[test]
    fn identical_frame_means_end_of_scroll() {
        let mut s = Stitcher::new(&frame(500, 300, 0, 0));
        assert_eq!(s.push(&frame(500, 300, 0, 0)), Push::NoMovement);
        assert_eq!(s.finish().height(), 300);
    }

    #[test]
    fn unrelated_frame_is_reported_as_lost() {
        let mut s = Stitcher::new(&frame(0, 300, 0, 0));
        // A jump larger than the shot's height: no rows in common.
        assert_eq!(s.push(&frame(5000, 300, 0, 0)), Push::Lost);
        assert_eq!(s.push(&RgbaImage::new(W, 100)), Push::Lost, "different size");
        assert_eq!(s.finish().height(), 300, "the accumulated image stays intact");
    }

    #[test]
    fn blank_page_does_not_invent_a_match() {
        let blank = RgbaImage::from_pixel(W, 300, image::Rgba([255, 255, 255, 255]));
        let mut other = blank.clone();
        for x in 0..100 {
            other.put_pixel(x, 150, image::Rgba([0, 0, 0, 255]));
        }
        let mut s = Stitcher::new(&blank);
        assert_eq!(s.push(&other), Push::Lost);
    }

    #[test]
    fn tolerates_small_color_noise() {
        // The second shot has ±2 noise in every pixel: no exact signature matches.
        let noisy = |scroll: u32| {
            let mut f = frame(scroll, 300, 0, 0);
            for (i, p) in f.pixels_mut().enumerate() {
                let d = (i % 5) as i16 - 2;
                for c in 0..3 {
                    p.0[c] = (p.0[c] as i16 + d).clamp(0, 255) as u8;
                }
            }
            f
        };
        let mut s = Stitcher::new(&frame(0, 300, 0, 0));
        assert_eq!(s.push(&noisy(90)), Push::Added);
        assert_eq!(s.finish().height(), 390);
    }

    #[test]
    fn large_color_change_is_not_matched() {
        let mut different = frame(90, 300, 0, 0);
        for p in different.pixels_mut() {
            p.0[0] = p.0[0].wrapping_add(40);
        }
        let mut s = Stitcher::new(&frame(0, 300, 0, 0));
        assert_eq!(s.push(&different), Push::Lost);
    }

    /// Like `frame`, but with a fixed 60 px sidebar on the left (it doesn't scroll).
    fn frame_with_sidebar(scroll: u32, h: u32) -> RgbaImage {
        let mut f = frame(scroll, h, 0, 0);
        for y in 0..h {
            for x in 0..60 {
                f.put_pixel(x, y, image::Rgba([(x * 3) as u8, (y * 5) as u8, 77, 255]));
            }
        }
        f
    }

    #[test]
    fn fixed_sidebar_does_not_block_stitching() {
        let mut s = Stitcher::new(&frame_with_sidebar(0, 300));
        for sc in [80, 170, 260] {
            assert_eq!(s.push(&frame_with_sidebar(sc, 300)), Push::Added, "scroll {sc}");
        }
        let out = s.finish();
        assert_eq!(out.height(), 560);
        assert_eq!(out.get_pixel(120, 500).0, page_pixel(120, 500));
    }

    /// Content that scrolls over a fixed patterned background (like a chat wallpaper): only the
    /// "bubbles" move; the background between them stays still.
    fn chat_view(scroll: u32) -> RgbaImage {
        RgbaImage::from_fn(240, 200, |x, y| {
            let page_y = scroll + y;
            let in_bubble = (page_y / 40) % 2 == 0 && x > 30 && x < 200;
            if in_bubble {
                let v = x.wrapping_mul(2654435761).wrapping_add(page_y.wrapping_mul(40503)) >> 5;
                image::Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, 255])
            } else {
                // Fixed background: depends only on the position on the screen, not on the
                // scrolling.
                let v = (x / 3).wrapping_mul(97).wrapping_add((y / 3).wrapping_mul(193));
                image::Rgba([40 + (v % 20) as u8, 50 + (v % 13) as u8, 60, 255])
            }
        })
    }

    #[test]
    fn panorama_follows_content_over_fixed_background() {
        let mut p = Panorama::new(&chat_view(0));
        for sc in [30, 75, 130, 90] {
            assert_eq!(p.push(&chat_view(sc)), Push::Added, "scroll {sc}");
        }
        assert_eq!(p.size(), (240, 330));
    }

    #[test]
    fn panorama_follows_content_beside_fixed_sidebar() {
        let side = |x0: u32, y0: u32| {
            let mut v = view(x0, y0);
            for y in 0..v.height() {
                for x in 0..40 {
                    v.put_pixel(x, y, image::Rgba([9, (y * 2) as u8, (x * 6) as u8, 255]));
                }
            }
            v
        };
        let mut p = Panorama::new(&side(0, 0));
        assert_eq!(p.push(&side(0, 50)), Push::Added);
        assert_eq!(p.push(&side(0, 20)), Push::Added);
        assert_eq!(p.size(), (160, 170));
    }

    #[test]
    fn estimate_shift_reports_direction_and_rejects_noise() {
        let lines = |sc: u32| sample_lines(&frame(sc, 300, 0, 0), true, 0, 300);
        assert_eq!(estimate_shift(&lines(100), &lines(160), true), Some(60));
        assert_eq!(estimate_shift(&lines(160), &lines(100), true), Some(-60));
        assert_eq!(estimate_shift(&lines(160), &lines(100), false), None, "no permission to go back");
        assert_eq!(estimate_shift(&lines(0), &lines(9000), true), None, "unrelated shots");
    }

    /// Measures the cost per shot on a large area: `cargo test bench_pano -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn bench_pano() {
        let big = |y0: u32, fixed: bool| {
            RgbaImage::from_fn(1400, 1000, |x, y| {
                let py = if fixed && x < 300 { y } else { y0 + y };
                let v = x.wrapping_mul(2654435761).wrapping_add(py.wrapping_mul(40503)) >> 5;
                image::Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, 255])
            })
        };
        for (label, fixed) in [("whole row scrolls", false), ("with fixed sidebar", true)] {
            let frames: Vec<RgbaImage> = (0..6).map(|i| big(i * 130, fixed)).collect();
            let mut p = Panorama::new(&frames[0]);
            let t = std::time::Instant::now();
            for f in &frames[1..] {
                assert_eq!(p.push(f), Push::Added);
            }
            println!("{label}: {:.0} ms por foto", t.elapsed().as_secs_f64() * 1000.0 / 5.0);
        }
        let mut p = Panorama::new(&big(0, false));
        let t = std::time::Instant::now();
        assert_eq!(p.push(&big(50_000, false)), Push::Lost);
        println!("unrelated shot (worst case): {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    }

    #[test]
    fn transpose_swaps_axes_and_round_trips() {
        let img = view(3, 7);
        let t = transpose(&img);
        assert_eq!(t.dimensions(), (120, 160));
        assert_eq!(t.get_pixel(5, 9), img.get_pixel(9, 5));
        assert_eq!(transpose(&t), img);
    }

    #[test]
    fn horizontal_strip_via_transpose() {
        let mut s = Stitcher::new(&transpose(&view(0, 0)));
        for x in [50, 100, 130] {
            assert_eq!(s.push(&transpose(&view(x, 0))), Push::Added, "x {x}");
        }
        let out = transpose(&s.finish());
        assert_eq!(out.dimensions(), (160 + 130, 120));
        assert_eq!(out.get_pixel(250, 60), view(130, 0).get_pixel(120, 60));
    }

    #[test]
    fn panorama_follows_all_four_directions() {
        let mut p = Panorama::new(&view(100, 100));
        // goes down, up past the start, to the right and back past the start
        for (x, y) in [(100, 150), (100, 190), (100, 110), (100, 60), (140, 60), (60, 60)] {
            assert_eq!(p.push(&view(x, y)), Push::Added, "({x}, {y})");
        }
        assert_eq!(p.push(&view(60, 60)), Push::NoMovement);
        // x covered 60..300 and y covered 60..310
        assert_eq!(p.size(), (240, 250));
        let out = p.finish();
        // Point (0,0) of the mosaic is point (60,60) of the page.
        assert_eq!(out.get_pixel(0, 0), view(60, 60).get_pixel(0, 0));
        assert_eq!(out.get_pixel(50, 200), view(100, 190).get_pixel(10, 70));
    }

    #[test]
    fn panorama_ignores_frames_it_cannot_place() {
        let mut p = Panorama::new(&view(0, 0));
        assert_eq!(p.push(&view(4000, 9000)), Push::Lost);
        assert_eq!(p.size(), (160, 120));
        assert_eq!(p.push(&view(0, 40)), Push::Added, "continues from the last good shot");
    }
}
