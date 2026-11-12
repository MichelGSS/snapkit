//! Neural OCR engine (PP-OCR models, run with ONNX Runtime).
//!
//! It's slower than Tesseract, but it reads well what Tesseract can't: handwriting, decorative
//! fonts, text over photos. It works in two stages:
//!
//! 1. **Detection**: a model marks, pixel by pixel, where there's text. The marked blobs become
//!    rectangles, one per line of text.
//! 2. **Recognition**: each rectangle is cropped, scaled to a height of 48 px and read by a second
//!    model, which returns a sequence of characters.
//!
//! The models are in the `ocr-neural` folder next to the executable (`det.onnx`, `rec.onnx`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use image::RgbaImage;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

/// Minimum probability for a pixel to count as text.
const PIXEL_THRESHOLD: f32 = 0.3;
/// Minimum average probability inside a rectangle for it to be accepted.
const BOX_THRESHOLD: f32 = 0.5;
/// The model marks only the core of the letters; the rectangle is widened by this ratio.
const UNCLIP_RATIO: f32 = 1.6;
/// Minimum shorter side and maximum longer side of the image given to detection.
const DET_MIN_SIDE: f32 = 736.0;
const DET_MAX_SIDE: f32 = 1920.0;
/// Height at which each line is recognized, and maximum width after scaling.
const REC_HEIGHT: u32 = 48;
const REC_MAX_WIDTH: u32 = 2400;
/// Minimum average confidence of the characters in a line.
const TEXT_THRESHOLD: f32 = 0.5;

/// A recognized line of text and its rectangle in the original image.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
}

/// Folder with `det.onnx` and `rec.onnx`, if it exists.
pub fn find_models() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("SNAPKIT_OCR_NEURAL_DIR") {
        candidates.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(exe_dir.join("ocr-neural"));
    }
    #[cfg(debug_assertions)]
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor").join("ocr-neural"));
    candidates.into_iter().find(|d| d.join("det.onnx").is_file() && d.join("rec.onnx").is_file())
}

/// Reads every line of text in the image.
pub fn recognize(dir: &Path, img: &RgbaImage) -> Result<Vec<Line>, String> {
    let err = |e: ort::Error| tf!("falha no motor neural: {e}", "neural engine failure: {e}");
    // ONNX Runtime allows only one environment per process; it's created on the first read.
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    let env = ENV.get_or_init(|| ort::init().build().map_err(|e| e.to_string())).as_ref().map_err(String::clone)?;
    let mut det = Session::builder(env).map_err(err)?.commit_from_file(dir.join("det.onnx")).map_err(err)?;
    let mut rec = Session::builder(env).map_err(err)?.commit_from_file(dir.join("rec.onnx")).map_err(err)?;
    let alphabet = alphabet(&rec)?;

    let boxes = detect(&mut det, img)?;
    let mut lines = Vec::new();
    for b in boxes {
        let crop = image::imageops::crop_imm(img, b.0, b.1, b.2 - b.0, b.3 - b.1).to_image();
        if let Some(text) = read_line(&mut rec, &crop, &alphabet)? {
            lines.push(Line {
                x: b.0 as f32,
                y: b.1 as f32,
                w: (b.2 - b.0) as f32,
                h: (b.3 - b.1) as f32,
                text,
            });
        }
    }
    Ok(lines)
}

/// The character list is stored inside the recognition model itself. Output index 0 is the CTC
/// "blank"; the last one is the space.
fn alphabet(rec: &Session) -> Result<Vec<String>, String> {
    let meta = rec.metadata().map_err(|e| e.to_string())?;
    let chars = meta
        .custom("character")
        .ok_or(t!("o modelo de reconhecimento não traz a lista de caracteres", "the recognition model has no character list"))?;
    let mut list: Vec<String> = vec![String::new()];
    list.extend(chars.lines().map(str::to_owned));
    list.push(" ".to_owned());
    Ok(list)
}

/// Converts to the tensor the models expect: separate BGR channels, values in [-1, 1].
fn to_tensor(img: &image::RgbImage) -> Result<Tensor<f32>, String> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut data = vec![0f32; 3 * w * h];
    for (i, px) in img.pixels().enumerate() {
        for c in 0..3 {
            // Channel 0 of the tensor is blue: the models were trained on BGR images.
            data[c * w * h + i] = (px.0[2 - c] as f32 / 255.0 - 0.5) / 0.5;
        }
    }
    Tensor::from_array(([1usize, 3, h, w], data)).map_err(|e| e.to_string())
}

/// Rectangles (left, top, right, bottom) of the lines of text, top to bottom.
fn detect(det: &mut Session, img: &RgbaImage) -> Result<Vec<(u32, u32, u32, u32)>, String> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    // Small images are enlarged (tiny text disappears in detection); huge ones are reduced.
    let mut scale = if w.min(h) < DET_MIN_SIDE { DET_MIN_SIDE / w.min(h) } else { 1.0 };
    if w.max(h) * scale > DET_MAX_SIDE {
        scale = DET_MAX_SIDE / w.max(h);
    }
    // The model requires sides that are multiples of 32.
    let round32 = |v: f32| (((v / 32.0).round() as u32).max(1)) * 32;
    let (dw, dh) = (round32(w * scale), round32(h * scale));
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let resized = image::imageops::resize(&rgb, dw, dh, image::imageops::FilterType::Triangle);

    let input = to_tensor(&resized)?;
    let outputs = det.run(ort::inputs![input]).map_err(|e| e.to_string())?;
    let (_, prob) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    if prob.len() != (dw * dh) as usize {
        return Err(t!("saída inesperada do modelo de detecção", "unexpected output from the detection model").to_owned());
    }

    let (sx, sy) = (w / dw as f32, h / dh as f32);
    let mut boxes: Vec<(u32, u32, u32, u32)> = components(prob, dw as usize, dh as usize)
        .into_iter()
        .filter_map(|c| {
            let (bw, bh) = ((c.x1 - c.x0 + 1) as f32, (c.y1 - c.y0 + 1) as f32);
            if bw.min(bh) < 3.0 || c.score < BOX_THRESHOLD {
                return None;
            }
            // Widens the rectangle: distance = area × ratio ÷ perimeter.
            let grow = bw * bh * UNCLIP_RATIO / (2.0 * (bw + bh));
            let x0 = ((c.x0 as f32 - grow) * sx).max(0.0);
            let y0 = ((c.y0 as f32 - grow) * sy).max(0.0);
            let x1 = ((c.x1 as f32 + 1.0 + grow) * sx).min(w);
            let y1 = ((c.y1 as f32 + 1.0 + grow) * sy).min(h);
            (x1 - x0 >= 4.0 && y1 - y0 >= 4.0).then_some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
        })
        .collect();
    boxes.sort_by_key(|b| (b.1, b.0));
    Ok(boxes)
}

/// A blob of pixels marked as text.
#[derive(Debug, PartialEq)]
struct Component {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    /// Average probability of the blob's pixels.
    score: f32,
}

/// Groups the pixels above the threshold into connected blobs (8-neighborhood).
fn components(prob: &[f32], w: usize, h: usize) -> Vec<Component> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || prob[start] <= PIXEL_THRESHOLD {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        let (mut sum, mut count) = (0f32, 0u32);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            sum += prob[i];
            count += 1;
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = ny * w + nx;
                    if !seen[j] && prob[j] > PIXEL_THRESHOLD {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(Component { x0, y0, x1, y1, score: sum / count as f32 });
    }
    out
}

/// Reads one cropped line. Returns `None` if the model isn't confident in the result.
fn read_line(rec: &mut Session, crop: &RgbaImage, alphabet: &[String]) -> Result<Option<String>, String> {
    let ratio = crop.width() as f32 / crop.height().max(1) as f32;
    let width = ((REC_HEIGHT as f32 * ratio).ceil() as u32).clamp(REC_HEIGHT / 2, REC_MAX_WIDTH);
    let rgb = image::DynamicImage::ImageRgba8(crop.clone()).to_rgb8();
    let resized = image::imageops::resize(&rgb, width, REC_HEIGHT, image::imageops::FilterType::Triangle);

    let input = to_tensor(&resized)?;
    let outputs = rec.run(ort::inputs![input]).map_err(|e| e.to_string())?;
    let (shape, probs) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
    let classes = *shape.last().ok_or(t!("saída inesperada do modelo de reconhecimento", "unexpected output from the recognition model"))? as usize;
    if classes == 0 || probs.len() % classes != 0 {
        return Err(t!("saída inesperada do modelo de reconhecimento", "unexpected output from the recognition model").to_owned());
    }
    Ok(decode_ctc(probs, classes, alphabet))
}

/// Greedy CTC decoding: at each step the most likely class wins; consecutive repeats collapse into
/// one, and class 0 ("blank") is dropped.
fn decode_ctc(probs: &[f32], classes: usize, alphabet: &[String]) -> Option<String> {
    let mut text = String::new();
    let (mut confidence, mut chars) = (0f32, 0u32);
    let mut previous = 0usize;
    for step in probs.chunks_exact(classes) {
        let (best, p) = step
            .iter()
            .enumerate()
            .fold((0, f32::MIN), |acc, (i, &p)| if p > acc.1 { (i, p) } else { acc });
        if best != 0 && best != previous {
            if let Some(ch) = alphabet.get(best) {
                text.push_str(ch);
                confidence += p;
                chars += 1;
            }
        }
        previous = best;
    }
    let text = text.trim().to_owned();
    (chars > 0 && confidence / chars as f32 >= TEXT_THRESHOLD && !text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_finds_separate_blobs_with_scores() {
        // 8×4 map with two blobs: a strong one on the left, a weak one on the right.
        let mut prob = vec![0.0f32; 32];
        for (x, y) in [(0, 0), (1, 0), (1, 1)] {
            prob[y * 8 + x] = 0.9;
        }
        for (x, y) in [(5, 2), (6, 3)] {
            prob[y * 8 + x] = 0.4; // connected diagonally
        }
        prob[3] = 0.2; // below the threshold: ignored
        let found = components(&prob, 8, 4);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].x0, found[0].y0, found[0].x1, found[0].y1), (0, 0, 1, 1));
        assert!((found[0].score - 0.9).abs() < 1e-6);
        assert_eq!((found[1].x0, found[1].y0, found[1].x1, found[1].y1), (5, 2, 6, 3));
    }

    #[test]
    fn ctc_collapses_repeats_and_drops_blanks() {
        let alphabet: Vec<String> = ["", "o", "l", "á", " "].iter().map(|s| s.to_string()).collect();
        // Steps: o o _ l _ á á  →  "olá". A "blank" between equal letters would keep them apart.
        let steps = [1, 1, 0, 2, 0, 3, 3];
        let mut probs = Vec::new();
        for &s in &steps {
            let mut row = vec![0.02f32; 5];
            row[s] = 0.92;
            probs.extend(row);
        }
        assert_eq!(decode_ctc(&probs, 5, &alphabet), Some("olá".to_owned()));

        // "l _ l" is two letters; "l l" is just one.
        let mut probs = Vec::new();
        for &s in &[2, 0, 2, 2] {
            let mut row = vec![0.0f32; 5];
            row[s] = 0.9;
            probs.extend(row);
        }
        assert_eq!(decode_ctc(&probs, 5, &alphabet), Some("ll".to_owned()));
    }

    #[test]
    fn ctc_rejects_low_confidence_and_empty_lines() {
        let alphabet: Vec<String> = ["", "a", " "].iter().map(|s| s.to_string()).collect();
        assert_eq!(decode_ctc(&[0.4, 0.45, 0.15], 3, &alphabet), None, "low confidence");
        assert_eq!(decode_ctc(&[0.9, 0.05, 0.05], 3, &alphabet), None, "only blank");
        assert_eq!(decode_ctc(&[0.0, 0.1, 0.9], 3, &alphabet), None, "only space");
    }

    /// Uses the models in `vendor/ocr-neural`; run scripts/fetch-ocr-neural.ps1 if they're missing.
    #[test]
    fn reads_rendered_text() {
        if crate::in_ci() {
            return;
        }
        use crate::render::{self, Op};
        let dir = find_models().expect("models not found: run scripts/fetch-ocr-neural.ps1");
        let mut img = RgbaImage::from_pixel(700, 200, image::Rgba([255, 255, 255, 255]));
        let font = crate::editor::default_font();
        for (y, text) in [(20.0, "Dear Santa 2026"), (110.0, "second line here")] {
            let op = Op::Text { pos: (30.0, y), text: text.into(), color: [0, 0, 0, 255], size: 44.0, boxed: false };
            render::apply(&mut img, &op, &font);
        }
        let lines = recognize(&dir, &img).unwrap();
        let texts: Vec<String> = lines.iter().map(|l| l.text.to_lowercase()).collect();
        assert_eq!(texts.len(), 2, "linhas lidas: {texts:?}");
        assert!(texts[0].contains("dear santa") && texts[0].contains("2026"), "linhas lidas: {texts:?}");
        assert!(texts[1].contains("second line"), "linhas lidas: {texts:?}");
        assert!(lines[0].y < lines[1].y);
    }
}
