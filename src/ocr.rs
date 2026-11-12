//! Text extraction from images (OCR).
//!
//! Two engines, both bundled with the program:
//!
//! 1. **Tesseract** (`tesseract` folder next to the executable): the default. It's the most
//!    accurate on screen text, code and configuration.
//! 2. **Neural** ([`crate::neural`], `ocr-neural` folder): reads handwriting and decorative fonts,
//!    which Tesseract can't, but loses capitals and indentation in code. It's used on request, and
//!    also as a fallback if Tesseract is missing or fails.
//!
//! Neither uses the internet. Both return fragments with their positions, and the final text is
//! rebuilt here from them: words at the same height become a line, and indentation and wide gaps
//! are reproduced with spaces. This preserves the shape of code and tables, which the engines alone
//! scramble.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::RgbaImage;

/// How far back from the chunk limit to look for a plain line to cut at.
const SPLIT_SEARCH: u32 = 400;
/// Tiny images read poorly; small areas get a margin up to this size.
const MIN_SIDE: u32 = 64;
/// Words with confidence below this (0–100) are usually icons read as letters.
const MIN_CONFIDENCE: f32 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Engine {
    Tesseract,
    Neural,
}

/// What the user wants to read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Printed or screen text: Tesseract, with the neural engine as a fallback.
    Printed,
    /// Handwriting or decorative lettering: neural engine.
    Handwriting,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Tesseract => "Tesseract",
            Engine::Neural => t!("motor neural", "neural engine"),
        }
    }
}

/// Recognizes the text in the image and says which engine was used. Must be called off the
/// interface thread: it can take a few seconds.
pub fn recognize(img: &RgbaImage, mode: Mode) -> Result<(String, Engine), String> {
    if mode == Mode::Printed {
        match find_tesseract().map(|dir| tesseract_words(&dir, img)) {
            Some(Ok(words)) => return Ok((layout(&words), Engine::Tesseract)),
            // Without Tesseract, or with it failing, the neural engine can still read the image.
            Some(Err(_)) | None => {}
        }
    }
    let dir = crate::neural::find_models().ok_or_else(|| match mode {
        Mode::Printed => t!(
            "nenhum motor de OCR encontrado (pastas tesseract e ocr-neural)",
            "no OCR engine found (tesseract and ocr-neural folders)"
        )
        .to_owned(),
        Mode::Handwriting => t!(
            "os modelos do motor neural não foram encontrados (pasta ocr-neural)",
            "the neural engine models were not found (ocr-neural folder)"
        )
        .to_owned(),
    })?;
    // Each line read goes in as a single fragment; the layout takes care of order and indentation.
    let words: Vec<Word> = crate::neural::recognize(&dir, img)?
        .into_iter()
        .map(|l| Word { x: l.x, y: l.y, w: l.w, h: l.h, text: l.text })
        .collect();
    Ok((layout(&words), Engine::Neural))
}

/// A recognized word and its rectangle in the image.
#[derive(Clone, Debug, PartialEq)]
struct Word {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    text: String,
}

// ---------------------------------------------------------------- Tesseract

/// Folder with `tesseract.exe` and `tessdata`, if it exists.
fn find_tesseract() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("SNAPKIT_TESSERACT_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(exe_dir.join("tesseract"));
    }
    // During development the binaries are in `vendor/tesseract` (see scripts/fetch-tesseract.ps1).
    #[cfg(debug_assertions)]
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor").join("tesseract"));
    candidates.into_iter().find(|d| d.join("tesseract.exe").is_file() && d.join("tessdata").is_dir())
}

/// Languages installed in `tessdata`, with Portuguese and English first: "por+eng".
fn tesseract_languages(dir: &Path) -> String {
    let mut langs: Vec<String> = std::fs::read_dir(dir.join("tessdata"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let lang = name.strip_suffix(".traineddata")?;
            // "osd" detects orientation; it isn't a language.
            (lang != "osd").then(|| lang.to_owned())
        })
        .collect();
    langs.sort_by_key(|l| match l.as_str() {
        "por" => 0,
        "eng" => 1,
        _ => 2,
    });
    langs.join("+")
}

fn tesseract_words(dir: &Path, img: &RgbaImage) -> Result<Vec<Word>, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Scaling 3× helps Tesseract the most with screen text; the cap avoids giant images.
    let scale = if img.width() * 3 <= 9000 {
        3
    } else if img.width() * 2 <= 9000 {
        2
    } else {
        1
    };
    let prepared = prepare(img, scale, 30_000);
    let langs = tesseract_languages(dir);
    if langs.is_empty() {
        return Err(t!("nenhum idioma em tessdata", "no language in tessdata").to_owned());
    }

    let mut words = Vec::new();
    for (index, (top, bottom)) in split_rows(&prepared, 6000).into_iter().enumerate() {
        let tile = image::imageops::crop_imm(&prepared, 0, top, prepared.width(), bottom - top).to_image();
        let path = std::env::temp_dir().join(format!("snapkit-ocr-{}-{index}.png", std::process::id()));
        tile.save(&path).map_err(|e| e.to_string())?;
        let output = Command::new(dir.join("tesseract.exe"))
            .arg(&path)
            .arg("stdout")
            .args(["-l", &langs])
            // Mode 11: sparse text, no column assumptions. The best for screens with several
            // panels.
            .args(["--psm", "11"])
            .args(["-c", "tessedit_create_tsv=1"])
            .env("TESSDATA_PREFIX", dir.join("tessdata"))
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let _ = std::fs::remove_file(&path);
        let output = output.map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        words.extend(parse_tsv(&String::from_utf8_lossy(&output.stdout)).into_iter().map(|mut w| {
            w.y += top as f32;
            w
        }));
    }
    Ok(words)
}

/// Reads Tesseract's TSV output. Only level 5 rows are words: `level page block paragraph line word
/// left top width height confidence text`.
fn parse_tsv(tsv: &str) -> Vec<Word> {
    tsv.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 12 || cols[0] != "5" {
                return None;
            }
            let num = |i: usize| cols[i].trim().parse::<f32>().ok();
            let text = cols[11].trim();
            if text.is_empty() || num(10)? < MIN_CONFIDENCE {
                return None;
            }
            Some(Word { x: num(6)?, y: num(7)?, w: num(8)?, h: num(9)?, text: text.to_owned() })
        })
        .collect()
}

// ---------------------------------------------------------------- Text layout

fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    Some(values[values.len() / 2])
}

/// Rebuilds the text from the positions of the words.
fn layout(words: &[Word]) -> String {
    let Some(line_h) = median(words.iter().map(|w| w.h).collect()) else { return String::new() };
    // Typical width of a character, to convert distances into spaces.
    let char_w = median(
        words
            .iter()
            .filter(|w| w.text.chars().count() >= 2)
            .map(|w| w.w / w.text.chars().count() as f32)
            .collect(),
    )
    .unwrap_or(line_h * 0.5)
    .max(1.0);

    // Groups into lines: words whose vertical center falls near the line's center.
    let mut sorted: Vec<&Word> = words.iter().collect();
    sorted.sort_by(|a, b| (a.y + a.h / 2.0).total_cmp(&(b.y + b.h / 2.0)));
    let mut rows: Vec<(f32, Vec<&Word>)> = Vec::new(); // (vertical center, words)
    for word in sorted {
        let cy = word.y + word.h / 2.0;
        match rows.last_mut() {
            Some((row_cy, row)) if (cy - *row_cy).abs() < line_h * 0.5 => {
                row.push(word);
                *row_cy += (cy - *row_cy) / row.len() as f32;
            }
            _ => rows.push((cy, vec![word])),
        }
    }

    // Normal distance between two consecutive lines: the lower quartile of the distances, so that
    // blank lines (larger distances) don't count.
    let mut pitches: Vec<f32> = rows.windows(2).map(|p| p[1].0 - p[0].0).collect();
    pitches.sort_by(|a, b| a.total_cmp(b));
    let pitch = pitches.get(pitches.len() / 4).copied().unwrap_or(line_h * 1.4);

    let left = words.iter().map(|w| w.x).fold(f32::INFINITY, f32::min);
    let mut out = String::new();
    let mut prev_cy: Option<f32> = None;
    for (cy, mut row) in rows {
        row.sort_by(|a, b| a.x.total_cmp(&b.x));
        if let Some(prev) = prev_cy {
            out.push('\n');
            // A jump much larger than normal was a blank line in the original.
            if cy - prev > pitch * 1.6 {
                out.push('\n');
            }
        }
        prev_cy = Some(cy);
        let indent = ((row[0].x - left) / char_w).round() as usize;
        out.extend(std::iter::repeat_n(' ', indent));
        let mut end: Option<f32> = None;
        for word in row {
            if let Some(end) = end {
                let gap = word.x - end;
                // Attached punctuation ("host:", "a.b") comes as words almost touching: no space.
                // The same goes for a lone sign that the OCR split from the word ("enabled :").
                let closing = matches!(word.text.as_str(), ":" | "," | "." | ";" | ")" | "]" | "}");
                if gap > char_w * 0.3 && !(closing && gap < char_w * 1.5) {
                    let spaces = ((gap / char_w).round() as usize).clamp(1, 12);
                    out.extend(std::iter::repeat_n(' ', spaces));
                }
            }
            // Typographic quotes become the keyboard's straight quotes: in code, only those are
            // valid.
            out.extend(word.text.chars().map(|c| match c {
                '“' | '”' | '„' => '"',
                '‘' | '’' => '\'',
                c => c,
            }));
            end = Some(word.x + word.w);
        }
    }
    out
}

// ---------------------------------------------------------------- Image preparation

/// Prepares the image for OCR: light background, text enlarged `scale` times, a guaranteed minimum
/// size and a width within `max`.
fn prepare(img: &RgbaImage, scale: u32, max: u32) -> RgbaImage {
    let mut out = img.clone();
    // The engines read dark text on a light background better: dark images are inverted.
    if is_dark(&out) {
        for px in out.pixels_mut() {
            for c in 0..3 {
                px.0[c] = 255 - px.0[c];
            }
        }
    }
    if scale > 1 {
        out = image::imageops::resize(
            &out,
            out.width() * scale,
            out.height() * scale,
            image::imageops::FilterType::CatmullRom,
        );
    }
    if out.width() > max {
        let h = (out.height() as u64 * max as u64 / out.width() as u64).max(1) as u32;
        out = image::imageops::resize(&out, max, h, image::imageops::FilterType::Triangle);
    }
    if out.width() < MIN_SIDE || out.height() < MIN_SIDE {
        // Margin in the corner's color, so as not to create a border the OCR reads as a stroke.
        let bg = *out.get_pixel(0, 0);
        let (w, h) = (out.width().max(MIN_SIDE), out.height().max(MIN_SIDE));
        let mut padded = RgbaImage::from_pixel(w, h, bg);
        image::imageops::replace(&mut padded, &out, ((w - out.width()) / 2) as i64, ((h - out.height()) / 2) as i64);
        out = padded;
    }
    out
}

/// Is most of the image dark? (samples one pixel in 7)
fn is_dark(img: &RgbaImage) -> bool {
    let (mut dark, mut total) = (0u32, 0u32);
    for px in img.pixels().step_by(7) {
        let luma = (px.0[0] as u32 * 3 + px.0[1] as u32 * 6 + px.0[2] as u32) / 10;
        dark += (luma < 110) as u32;
        total += 1;
    }
    dark * 2 > total
}

/// Splits the height into chunks of up to `tile` rows, cutting preferably at a row of uniform color
/// (between two lines of text) so letters aren't split in half.
fn split_rows(img: &RgbaImage, tile: u32) -> Vec<(u32, u32)> {
    let h = img.height();
    let mut parts = Vec::new();
    let mut top = 0;
    while h - top > tile {
        let limit = top + tile;
        let lowest = limit.saturating_sub(SPLIT_SEARCH).max(top + 1);
        let cut = (lowest..=limit).rev().find(|&y| row_is_uniform(img, y)).unwrap_or(limit);
        parts.push((top, cut));
        top = cut;
    }
    parts.push((top, h));
    parts
}

fn row_is_uniform(img: &RgbaImage, y: u32) -> bool {
    let w = img.width() as usize;
    let row = &img.as_raw()[y as usize * w * 4..(y as usize + 1) * w * 4];
    let first = &row[..3];
    row.chunks_exact(4).all(|px| &px[..3] == first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::{self, Op};

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]))
    }

    #[test]
    fn short_image_is_one_part() {
        assert_eq!(split_rows(&white(100, 500), 3000), vec![(0, 500)]);
    }

    #[test]
    fn tall_image_is_cut_on_uniform_rows() {
        let mut img = white(50, 7000);
        // "Text" (non-uniform rows) from 2900 to 3100: the cut has to avoid it.
        for y in 2900..3100 {
            img.put_pixel(10, y, image::Rgba([0, 0, 0, 255]));
        }
        let parts = split_rows(&img, 3000);
        assert_eq!(parts[0], (0, 2899), "cuts at the last plain row before the text");
        assert_eq!(parts.last().unwrap().1, 7000);
        // The chunks are contiguous and none goes past the limit.
        for pair in parts.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
        assert!(parts.iter().all(|(a, b)| b - a <= 3000 && b > a));
    }

    #[test]
    fn busy_image_still_gets_cut_at_the_limit() {
        let img = RgbaImage::from_fn(20, 6500, |x, y| image::Rgba([(x * 7 + y) as u8, 0, 0, 255]));
        assert_eq!(split_rows(&img, 3000), vec![(0, 3000), (3000, 6000), (6000, 6500)]);
    }

    #[test]
    fn prepare_scales_caps_and_pads() {
        assert_eq!(prepare(&white(300, 100), 2, 10_000).dimensions(), (600, 200));
        assert_eq!(prepare(&white(300, 100), 3, 10_000).dimensions(), (900, 300));
        assert_eq!(prepare(&white(300, 100), 1, 10_000).dimensions(), (300, 100));
        assert_eq!(prepare(&white(20, 10), 1, 10_000).dimensions(), (64, 64));
        assert_eq!(prepare(&white(12_000, 3000), 1, 10_000).dimensions(), (10_000, 2500));
    }

    #[test]
    fn dark_images_are_inverted() {
        let dark = RgbaImage::from_pixel(100, 100, image::Rgba([20, 22, 30, 255]));
        assert_eq!(prepare(&dark, 1, 10_000).get_pixel(5, 5).0, [235, 233, 225, 255]);
        assert_eq!(prepare(&white(100, 100), 1, 10_000).get_pixel(5, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn tsv_keeps_confident_words_only() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t900\t300\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t30\t40\t120\t22\t96.5\thost:\n\
                   5\t1\t1\t1\t1\t2\t170\t41\t90\t21\t12.0\t|||\n\
                   5\t1\t1\t1\t1\t3\t170\t41\t90\t21\t88\t   \n\
                   5\t1\t1\t1\t1\t4\t270\t40\t60\t22\t91\tação\n\
                   linha quebrada\n";
        let words = parse_tsv(tsv);
        assert_eq!(words.len(), 2, "{words:?}");
        assert_eq!(words[0], Word { x: 30.0, y: 40.0, w: 120.0, h: 22.0, text: "host:".into() });
        assert_eq!(words[1].text, "ação");
    }

    #[test]
    fn languages_put_portuguese_and_english_first() {
        let dir = std::env::temp_dir().join(format!("snapkit-test-tess-{}", std::process::id()));
        let data = dir.join("tessdata");
        std::fs::create_dir_all(&data).unwrap();
        for f in ["spa.traineddata", "eng.traineddata", "osd.traineddata", "por.traineddata", "leia-me.txt"] {
            std::fs::write(data.join(f), b"").unwrap();
        }
        assert_eq!(tesseract_languages(&dir), "por+eng+spa");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A word on a grid of characters 10 px wide and rows 20 px tall.
    fn word(col: u32, row: u32, text: &str) -> Word {
        Word {
            x: col as f32 * 10.0,
            y: row as f32 * 20.0 + 3.0,
            w: text.chars().count() as f32 * 10.0,
            h: 14.0,
            text: text.to_owned(),
        }
    }

    #[test]
    fn layout_rebuilds_rows_indentation_and_blank_lines() {
        // Out of order, as the OCR returns it: the values come after all the keys.
        let words = [
            word(0, 0, "cluster:"),
            word(2, 1, "enabled:"),
            word(0, 3, "premium:"),
            word(2, 4, "enabled:"),
            word(11, 1, "true"),
            word(11, 4, "true"),
        ];
        assert_eq!(layout(&words), "cluster:\n  enabled: true\n\npremium:\n  enabled: true");
    }

    #[test]
    fn layout_joins_touching_punctuation_and_keeps_wide_gaps() {
        // "postgres." "example." "internal" touching; a distant column on the right.
        let mut dot1 = word(0, 0, "postgres.");
        let mut dot2 = word(9, 0, "example.");
        dot1.w -= 1.0;
        dot2.w -= 1.0;
        let words = [dot1, dot2, word(17, 0, "internal"), word(30, 0, "ok")];
        assert_eq!(layout(&words), "postgres.example.internal     ok");
    }

    #[test]
    fn layout_normalizes_typographic_quotes() {
        assert_eq!(layout(&[word(0, 0, "cpu:"), word(5, 0, "“2”")]), "cpu: \"2\"");
    }

    #[test]
    fn evenly_double_spaced_text_gets_no_blank_lines() {
        let words = [word(0, 0, "one"), word(0, 2, "two"), word(0, 4, "three")];
        assert_eq!(layout(&words), "one
two
three");
    }

    #[test]
    fn layout_reattaches_stray_punctuation() {
        // "enabled" + ":" separated by almost one character; the value comes after.
        let words = [word(0, 0, "enabled"), word(8, 0, ":"), word(10, 0, "true")];
        assert_eq!(layout(&words), "enabled: true");
    }

    #[test]
    fn layout_tolerates_slightly_uneven_baselines() {
        let mut high = word(0, 0, "same");
        let mut low = word(5, 0, "line");
        high.y -= 2.0;
        low.y += 3.0;
        assert_eq!(layout(&[low, high]), "same line");
        assert_eq!(layout(&[]), "");
    }

    /// Reads an image file and prints the text, to judge the quality by eye:
    /// `SNAPKIT_OCR_FILE=image.png cargo test ocr_file -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ocr_file() {
        let path = std::env::var("SNAPKIT_OCR_FILE").expect("defina SNAPKIT_OCR_FILE");
        let img = image::open(path).unwrap().to_rgba8();
        // SNAPKIT_OCR_MODE=handwriting uses the neural engine.
        let mode = match std::env::var("SNAPKIT_OCR_MODE").as_deref() {
            Ok("handwriting") => Mode::Handwriting,
            _ => Mode::Printed,
        };
        let (text, engine) = recognize(&img, mode).unwrap();
        println!("----- {}\n{text}\n-----", engine.name());
    }

    /// Light text on a dark background, with indentation and a blank line, like a code snippet.
    fn sample() -> RgbaImage {
        let mut img = RgbaImage::from_pixel(700, 260, image::Rgba([24, 26, 31, 255]));
        let font = crate::editor::default_font();
        let lines = [(20.0, 20.0, "Recorte de tela 2026"), (80.0, 70.0, "segunda linha"), (20.0, 190.0, "depois do espaço")];
        for (x, y, text) in lines {
            let op = Op::Text { pos: (x, y), text: text.into(), color: [230, 230, 230, 255], size: 34.0, boxed: false };
            render::apply(&mut img, &op, &font);
        }
        img
    }

    fn assert_sample_text(text: &str) {
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows.len(), 4, "text read: {text:?}");
        assert!(rows[0].to_lowercase().starts_with("recorte de tela 2026"), "text read: {text:?}");
        assert!(rows[1].starts_with("  ") && rows[1].trim() == "segunda linha", "text read: {text:?}");
        assert_eq!(rows[2], "", "text read: {text:?}");
        assert!(rows[3].to_lowercase().contains("depois do espa"), "text read: {text:?}");
    }

    /// Uses the Tesseract in `vendor/tesseract`; run scripts/fetch-tesseract.ps1 if it's missing.
    #[test]
    fn tesseract_engine_reads_rendered_text_with_accents() {
        if crate::in_ci() {
            return;
        }
        let dir = find_tesseract().expect("Tesseract not found: run scripts/fetch-tesseract.ps1");
        let text = layout(&tesseract_words(&dir, &sample()).unwrap());
        assert_sample_text(&text);
        assert!(text.contains("espaço"), "the Portuguese model must read the cedilla: {text:?}");
        assert_eq!(recognize(&sample(), Mode::Printed).unwrap().1, Engine::Tesseract, "Tesseract has priority");
    }

    /// Uses the models in `vendor/ocr-neural`.
    #[test]
    fn handwriting_mode_uses_the_neural_engine() {
        if crate::in_ci() {
            return;
        }
        let (text, engine) = recognize(&sample(), Mode::Handwriting).unwrap();
        assert_eq!(engine, Engine::Neural);
        assert!(text.to_lowercase().contains("segunda linha"), "text read: {text:?}");
        assert!(text.lines().count() >= 3, "text read: {text:?}");
    }
}
