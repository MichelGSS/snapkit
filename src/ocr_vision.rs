//! Text recognition with Apple's Vision framework: the built-in OCR engine on macOS.
//!
//! Vision ships with the system, so OCR works on a Mac without the `tesseract` or `ocr-neural`
//! folders. It reads screen text, photos and a fair amount of handwriting, entirely on the device.
//!
//! Vision returns one observation per line of text, with a box in normalized coordinates (0 to 1,
//! origin at the bottom-left corner). Each line is split here into words, each with its own box in
//! image pixels (origin at the top-left corner), the same fragments the other engines produce, so
//! the layout in [`crate::ocr`] and the sensitive-data detector work unchanged.

use std::ops::Range;

use image::RgbaImage;
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CFData, CGRect};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{NSArray, NSDictionary, NSRange, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText, VNRequest,
    VNRequestTextRecognitionLevel,
};

use crate::ocr::WordBox;

/// Languages to recognize, in order of preference.
const LANGUAGES: [&str; 3] = ["pt-BR", "en-US", "es-ES"];
/// Images whose longer side is at most this are read at twice the size: small text reads better.
const UPSCALE_BELOW: u32 = 1500;
/// Widest image given to Vision; wider ones are reduced.
const MAX_WIDTH: u32 = 8192;
/// Tall images are read in chunks of up to this many rows, because Vision's minimum text height
/// is relative to the height of the image.
const TILE_HEIGHT: u32 = 2048;
/// Smallest text to look for, in pixels of the prepared image.
const MIN_TEXT_PX: f32 = 8.0;
/// Vision's own default minimum text height (relative to the image height).
const DEFAULT_MIN_TEXT: f32 = 1.0 / 32.0;
/// A measured word box at least this fraction of the line's width is the whole line, not the word.
const WHOLE_LINE: f64 = 0.95;

/// Recognizes the words in `img`, with their boxes in `img` pixels.
pub fn recognize(img: &RgbaImage) -> Result<Vec<WordBox>, String> {
    recognize_with_options(img, true)
}

/// Recognizes the words in `img`, with their boxes in `img` pixels, configuring Vision's
/// language correction.
pub fn recognize_with_options(img: &RgbaImage, language_correction: bool) -> Result<Vec<WordBox>, String> {
    if img.width() == 0 || img.height() == 0 {
        return Ok(Vec::new());
    }
    let scale = if img.width().max(img.height()) <= UPSCALE_BELOW { 2 } else { 1 };
    let prepared = crate::ocr::prepare(img, scale, MAX_WIDTH);
    let (eff_scale, pad_x, pad_y) = placement(img.width(), img.height(), scale, MAX_WIDTH);
    let mut words = Vec::new();
    for (top, bottom) in crate::ocr::split_rows(&prepared, TILE_HEIGHT) {
        let tile = image::imageops::crop_imm(&prepared, 0, top, prepared.width(), bottom - top).to_image();
        // Vision's objects are autoreleased; without a pool they would pile up on this thread.
        let lines = autoreleasepool(|_| recognize_lines(&tile, language_correction))?;
        let (tw, th) = (tile.width() as f64, tile.height() as f64);
        for (text, line_box, measure) in lines {
            for (word, norm) in line_words(&text, line_box, |r| measure.get(&r).copied().flatten()) {
                let (x, y, w, h) = to_pixels(norm, tw, th);
                words.push(WordBox {
                    x: ((x - pad_x) / eff_scale).max(0.0),
                    y: ((y + top as f32 - pad_y) / eff_scale).max(0.0),
                    w: (w / eff_scale).max(1.0),
                    h: (h / eff_scale).max(1.0),
                    text: word,
                });
            }
        }
    }
    Ok(words)
}

/// Word boxes Vision measured for a line, keyed by the word's range in UTF-16 units.
struct Measured(Vec<(Range<usize>, Option<Norm>)>);

impl Measured {
    fn get(&self, range: &Range<usize>) -> Option<&Option<Norm>> {
        self.0.iter().find(|(r, _)| r == range).map(|(_, b)| b)
    }
}

/// Runs Vision on one image: for each line, its text, its box and the boxes of its words.
fn recognize_lines(img: &RgbaImage, language_correction: bool) -> Result<Vec<(String, Norm, Measured)>, String> {
    let cg = cg_image(img).ok_or_else(|| t!("não foi possível preparar a imagem", "could not prepare the image").to_owned())?;
    let min_height = (MIN_TEXT_PX / img.height() as f32).min(DEFAULT_MIN_TEXT);
    let request = match perform(&cg, min_height, true, language_correction) {
        Ok(request) => request,
        // Older systems don't know every language in the list; their default still reads English.
        Err(_) => perform(&cg, min_height, false, language_correction)?,
    };
    let Some(results) = request.results() else { return Ok(Vec::new()) };
    let mut lines = Vec::new();
    for obs in results.iter() {
        let Some(candidate) = obs.topCandidates(1).firstObject() else { continue };
        let text = candidate.string().to_string();
        if text.trim().is_empty() {
            continue;
        }
        // SAFETY: a plain property read on an observation Vision just returned.
        let line_box = Norm::from(unsafe { obs.boundingBox() });
        let measured = tokens(&text)
            .into_iter()
            .map(|t| {
                let b = word_box(&candidate, t.utf16.clone());
                (t.utf16, b)
            })
            .collect();
        lines.push((text, line_box, Measured(measured)));
    }
    Ok(lines)
}

/// Builds and runs the text request on `cg`.
fn perform(
    cg: &CGImage,
    min_height: f32,
    languages: bool,
    language_correction: bool,
) -> Result<Retained<VNRecognizeTextRequest>, String> {
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(language_correction);
    request.setMinimumTextHeight(min_height);
    if languages {
        let list: Vec<Retained<NSString>> = LANGUAGES.iter().map(|l| NSString::from_str(l)).collect();
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&list));
    }
    let options = NSDictionary::<VNImageOption, AnyObject>::new();
    // SAFETY: `cg` is a valid image that isn't modified while the handler lives; the options
    // dictionary is empty.
    let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), cg, &options) };
    let as_request: &VNRequest = &request;
    handler
        .performRequests_error(&NSArray::from_slice(&[as_request]))
        .map_err(|e| tf!("falha no Vision: {}", "Vision failure: {}", e.localizedDescription()))?;
    Ok(request)
}

/// The box Vision gives for a range of the candidate's text, if it gives one.
fn word_box(candidate: &VNRecognizedText, utf16: Range<usize>) -> Option<Norm> {
    let range = NSRange::new(utf16.start, utf16.end - utf16.start);
    // SAFETY: the range lies within the candidate's string (it was computed from it).
    let rect = unsafe { candidate.boundingBoxForRange_error(range) }.ok()?;
    // SAFETY: a plain property read.
    Some(Norm::from(unsafe { rect.boundingBox() }))
}

/// Copies the image into a CGImage, over a white background (transparent areas read as paper).
fn cg_image(img: &RgbaImage) -> Option<objc2_core_foundation::CFRetained<CGImage>> {
    let mut rgbx = Vec::with_capacity(img.as_raw().len());
    for px in img.pixels() {
        let [r, g, b, a] = px.0;
        let over_white = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32) + 127) / 255) as u8;
        rgbx.extend_from_slice(&[over_white(r), over_white(g), over_white(b), 255]);
    }
    let data = CFData::from_bytes(&rgbx);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let space = CGColorSpace::new_device_rgb()?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    // SAFETY: the provider holds `h` rows of `w * 4` bytes, in the layout described here; no
    // decode array.
    unsafe {
        CGImage::new(
            w,
            h,
            8,
            32,
            w * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

// ---------------------------------------------------------------- Pure geometry

/// A rectangle in Vision's normalized coordinates: 0 to 1 across the image, origin at the
/// bottom-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Norm {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl From<CGRect> for Norm {
    fn from(r: CGRect) -> Self {
        Norm { x: r.origin.x as f64, y: r.origin.y as f64, w: r.size.width as f64, h: r.size.height as f64 }
    }
}

/// Converts a normalized rectangle into `(x, y, w, h)` pixels of a `width`×`height` image with the
/// origin at the top-left corner, clamped to the image.
fn to_pixels(r: Norm, width: f64, height: f64) -> (f32, f32, f32, f32) {
    let left = (r.x * width).clamp(0.0, width);
    let right = ((r.x + r.w) * width).clamp(0.0, width);
    // Vision's y grows upwards: the top edge of the box is its highest y.
    let top = ((1.0 - (r.y + r.h)) * height).clamp(0.0, height);
    let bottom = ((1.0 - r.y) * height).clamp(0.0, height);
    (left as f32, top as f32, (right - left) as f32, (bottom - top) as f32)
}

/// How [`crate::ocr::prepare`] places a `w`×`h` image: the effective scale and the margin added on
/// the left and on top.
fn placement(w: u32, h: u32, scale: u32, max: u32) -> (f32, f32, f32) {
    let (scaled_w, scaled_h) = (w * scale, h * scale);
    let (eff_w, eff_h, eff_scale) = if scaled_w > max {
        let h = (scaled_h as u64 * max as u64 / scaled_w as u64).max(1) as u32;
        (max, h, max as f32 / w.max(1) as f32)
    } else {
        (scaled_w, scaled_h, scale as f32)
    };
    let min = crate::ocr::MIN_SIDE;
    let pad_x = if eff_w < min { (min - eff_w) / 2 } else { 0 };
    let pad_y = if eff_h < min { (min - eff_h) / 2 } else { 0 };
    (eff_scale, pad_x as f32, pad_y as f32)
}

/// A word of a line of text and where it sits in the line, counted in characters and in UTF-16
/// units (what `NSRange` counts).
#[derive(Debug, PartialEq)]
struct Token {
    text: String,
    chars: Range<usize>,
    utf16: Range<usize>,
}

/// Splits a line into its words (runs of non-blank characters).
fn tokens(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut current: Option<Token> = None;
    let (mut chars, mut utf16) = (0, 0);
    for c in line.chars() {
        if c.is_whitespace() {
            out.extend(current.take());
        } else {
            let token = current.get_or_insert_with(|| Token { text: String::new(), chars: chars..chars, utf16: utf16..utf16 });
            token.text.push(c);
            token.chars.end = chars + 1;
            token.utf16.end = utf16 + c.len_utf16();
        }
        chars += 1;
        utf16 += c.len_utf16();
    }
    out.extend(current);
    out
}

/// Splits a recognized line into words with their boxes. `measure` gives the box Vision measured
/// for a UTF-16 range, if any; words without a usable one get a slice of the line's box in
/// proportion to their position among the characters.
fn line_words(line: &str, line_box: Norm, measure: impl Fn(Range<usize>) -> Option<Norm>) -> Vec<(String, Norm)> {
    let tokens = tokens(line);
    let total = line.chars().count().max(1) as f64;
    let several = tokens.len() > 1;
    tokens
        .into_iter()
        .map(|t| {
            let usable = |b: &Norm| b.w > 0.0 && b.h > 0.0 && !(several && b.w >= line_box.w * WHOLE_LINE);
            let b = measure(t.utf16.clone()).filter(usable).unwrap_or_else(|| Norm {
                x: line_box.x + line_box.w * t.chars.start as f64 / total,
                y: line_box.y,
                w: line_box.w * (t.chars.end - t.chars.start) as f64 / total,
                h: line_box.h,
            });
            (t.text, b)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn normalized_boxes_become_top_left_pixels() {
        // A box in the top-left quarter of a 200×100 image: y = 0.5..1.0 from the bottom.
        let (x, y, w, h) = to_pixels(Norm { x: 0.0, y: 0.5, w: 0.5, h: 0.5 }, 200.0, 100.0);
        assert!(close(x, 0.0) && close(y, 0.0) && close(w, 100.0) && close(h, 50.0), "{x} {y} {w} {h}");
        // A thin line near the bottom.
        let (x, y, w, h) = to_pixels(Norm { x: 0.25, y: 0.1, w: 0.5, h: 0.2 }, 200.0, 100.0);
        assert!(close(x, 50.0) && close(y, 70.0) && close(w, 100.0) && close(h, 20.0), "{x} {y} {w} {h}");
        // Boxes that spill past the edges are clamped to the image.
        let (x, y, w, h) = to_pixels(Norm { x: -0.1, y: 0.9, w: 0.3, h: 0.2 }, 100.0, 100.0);
        assert!(close(x, 0.0) && close(y, 0.0) && close(w, 20.0) && close(h, 10.0), "{x} {y} {w} {h}");
    }

    #[test]
    fn placement_matches_prepare() {
        let white = |w, h| RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        assert_eq!(placement(300, 100, 2, MAX_WIDTH), (2.0, 0.0, 0.0));
        // Small images are padded to the minimum side, centered.
        let (s, px, py) = placement(20, 10, 1, MAX_WIDTH);
        let prepared = crate::ocr::prepare(&white(20, 10), 1, MAX_WIDTH);
        assert_eq!((s, px, py), (1.0, 22.0, 27.0));
        assert_eq!(prepared.dimensions(), (20 + 2 * 22, 10 + 2 * 27));
        // Wide images are reduced to the maximum width.
        let (s, px, py) = placement(10_000, 500, 1, MAX_WIDTH);
        assert!(close(s, MAX_WIDTH as f32 / 10_000.0) && px == 0.0 && py == 0.0);
    }

    #[test]
    fn tokens_count_characters_and_utf16_units() {
        let found = tokens("  ação 😀x  fim ");
        let texts: Vec<&str> = found.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["ação", "😀x", "fim"]);
        assert_eq!(found[0].chars, 2..6);
        assert_eq!(found[0].utf16, 2..6);
        // The emoji is one character but two UTF-16 units.
        assert_eq!(found[1].chars, 7..9);
        assert_eq!(found[1].utf16, 7..10);
        assert_eq!(found[2].chars, 11..14);
        assert_eq!(found[2].utf16, 12..15);
        assert!(tokens("   ").is_empty());
    }

    const LINE: Norm = Norm { x: 0.1, y: 0.4, w: 0.8, h: 0.1 };

    #[test]
    fn words_use_the_boxes_vision_measured() {
        // "host: db" with a box for each word.
        let words = line_words("host: db", LINE, |r| match r.start {
            0 => Some(Norm { x: 0.1, y: 0.41, w: 0.45, h: 0.08 }),
            _ => Some(Norm { x: 0.7, y: 0.4, w: 0.2, h: 0.1 }),
        });
        assert_eq!(words.len(), 2);
        assert_eq!(words[0], ("host:".to_owned(), Norm { x: 0.1, y: 0.41, w: 0.45, h: 0.08 }));
        assert_eq!(words[1].1.x, 0.7);
    }

    #[test]
    fn words_without_a_usable_box_are_split_in_proportion() {
        // "ab cdef" = 7 characters: "ab" takes 2/7 of the line, "cdef" starts at 3/7.
        let check = |words: Vec<(String, Norm)>| {
            assert_eq!(words.len(), 2);
            let (a, b) = (words[0].1, words[1].1);
            assert!((a.x - 0.1).abs() < 1e-9 && (a.w - 0.8 * 2.0 / 7.0).abs() < 1e-9, "{a:?}");
            assert!((b.x - (0.1 + 0.8 * 3.0 / 7.0)).abs() < 1e-9 && (b.w - 0.8 * 4.0 / 7.0).abs() < 1e-9, "{b:?}");
            assert!(a.y == LINE.y && a.h == LINE.h);
        };
        check(line_words("ab cdef", LINE, |_| None));
        // A "word" box as wide as the whole line is the line's box: not trusted.
        check(line_words("ab cdef", LINE, |_| Some(LINE)));
        // Degenerate boxes aren't either.
        check(line_words("ab cdef", LINE, |_| Some(Norm { x: 0.3, y: 0.4, w: 0.0, h: 0.1 })));
    }

    #[test]
    fn a_single_word_line_may_use_the_whole_box() {
        let words = line_words("sozinho", LINE, |_| Some(LINE));
        assert_eq!(words, vec![("sozinho".to_owned(), LINE)]);
        assert!(line_words("", LINE, |_| None).is_empty());
    }

    /// Reads text drawn on a dark background with the system's Vision engine.
    #[test]
    fn vision_reads_rendered_text() {
        use crate::render::{self, Op};
        let mut img = RgbaImage::from_pixel(700, 200, image::Rgba([24, 26, 31, 255]));
        let font = crate::editor::default_font();
        for (y, text) in [(20.0, "Recorte de tela 2026"), (110.0, "segunda linha")] {
            let op = Op::Text { pos: (30.0, y), text: text.into(), color: [230, 230, 230, 255], size: 40.0, boxed: false, angle: 0.0 };
            render::apply(&mut img, &op, &font);
        }
        let words = recognize(&img).unwrap();
        let text: Vec<String> = words.iter().map(|w| w.text.to_lowercase()).collect();
        assert!(text.iter().any(|t| t == "segunda") && text.iter().any(|t| t == "linha"), "words read: {words:?}");
        for w in &words {
            assert!(w.x >= 0.0 && w.y >= 0.0 && w.x + w.w <= 701.0 && w.y + w.h <= 201.0, "{w:?}");
        }
        // The second line is below the first.
        let y_of = |t: &str| words.iter().find(|w| w.text.to_lowercase() == t).map(|w| w.y).unwrap();
        assert!(y_of("segunda") > y_of("recorte") + 40.0, "words read: {words:?}");
    }

    #[test]
    fn vision_reads_portuguese_diacritics() {
        use crate::render::{self, Op};
        let mut img = RgbaImage::from_pixel(700, 160, image::Rgba([24, 26, 31, 255]));
        let font = crate::editor::default_font();
        let op = Op::Text {
            pos: (30.0, 40.0),
            text: "ação configuração".into(),
            color: [230, 230, 230, 255],
            size: 44.0,
            boxed: false,
            angle: 0.0,
        };
        render::apply(&mut img, &op, &font);
        let words = recognize_with_options(&img, true).unwrap();
        let text: Vec<String> = words.iter().map(|w| w.text.to_lowercase()).collect();
        assert!(
            text.iter().any(|t| t == "ação") && text.iter().any(|t| t == "configuração"),
            "words read: {words:?}"
        );
    }

    #[test]
    fn vision_reads_multi_tile_tall_image() {
        use crate::render::{self, Op};
        let mut img = RgbaImage::from_pixel(700, 2600, image::Rgba([24, 26, 31, 255]));
        let font = crate::editor::default_font();
        for (y, text) in [(100.0, "topo"), (2300.0, "fundo")] {
            let op = Op::Text {
                pos: (40.0, y),
                text: text.into(),
                color: [230, 230, 230, 255],
                size: 48.0,
                boxed: false,
                angle: 0.0,
            };
            render::apply(&mut img, &op, &font);
        }
        let words = recognize(&img).unwrap();
        let y_of = |needle: &str| {
            words
                .iter()
                .find(|w| w.text.eq_ignore_ascii_case(needle))
                .map(|w| w.y)
        };
        let top_y = y_of("topo").unwrap_or_else(|| panic!("missing 'topo' in {words:?}"));
        let bottom_y = y_of("fundo").unwrap_or_else(|| panic!("missing 'fundo' in {words:?}"));
        assert!(top_y < 300.0, "top_y={top_y}");
        assert!(bottom_y > 2200.0, "bottom_y={bottom_y}");
    }
}
