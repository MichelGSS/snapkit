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
//! On macOS the system's own engine, **Vision** (`crate::ocr_vision`), comes first for printed
//! text and needs no folder; Tesseract and the neural engine are still used when their folders
//! exist: as fallbacks, and the neural engine for handwriting.
//!
//! None uses the internet. All return fragments with their positions, and the final text is
//! rebuilt here from them: words at the same height become a line, and indentation and wide gaps
//! are reproduced with spaces. This preserves the shape of code and tables, which the engines alone
//! scramble.

use std::path::{Path, PathBuf};

use image::RgbaImage;

/// How far back from the chunk limit to look for a plain line to cut at.
const SPLIT_SEARCH: u32 = 400;
/// Tiny images read poorly; small areas get a margin up to this size.
pub(crate) const MIN_SIDE: u32 = 64;
/// Words with confidence below this (0–100) are usually icons read as letters.
const MIN_CONFIDENCE: f32 = 30.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Engine {
    Tesseract,
    Neural,
    /// Apple's Vision framework, part of macOS.
    #[cfg(target_os = "macos")]
    Vision,
}

/// What the user wants to read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Printed or screen text: Tesseract (Vision on macOS), with the others as fallbacks.
    Printed,
    /// Handwriting or decorative lettering: neural engine (on macOS, Vision without it).
    Handwriting,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Engine::Tesseract => "Tesseract",
            Engine::Neural => t!("motor neural", "neural engine"),
            #[cfg(target_os = "macos")]
            Engine::Vision => "Apple Vision",
        }
    }

    /// The engine that reads printed text when nothing goes wrong.
    #[cfg(not(target_os = "macos"))]
    pub const PRINTED: Engine = Engine::Tesseract;
    #[cfg(target_os = "macos")]
    pub const PRINTED: Engine = Engine::Vision;
}

/// Recognizes the text in the image and says which engine was used. Must be called off the
/// interface thread: it can take a few seconds.
pub fn recognize(img: &RgbaImage, mode: Mode) -> Result<(String, Engine), String> {
    if img.width() == 0 || img.height() == 0 {
        let engine = match mode {
            Mode::Printed => Engine::PRINTED,
            Mode::Handwriting => Engine::Neural,
        };
        return Ok((String::new(), engine));
    }
    // The error of the last engine that failed, reported if no other one can read the image.
    let mut last_err = None;
    #[cfg(target_os = "macos")]
    if mode == Mode::Printed {
        match crate::ocr_vision::recognize(img) {
            Ok(words) => return Ok((layout(&words), Engine::Vision)),
            Err(e) => last_err = Some(e),
        }
    }
    if mode == Mode::Printed
        && let Some(res) = find_tesseract().map(|install| tesseract_words(&install, img))
    {
        match res {
            Ok(words) => return Ok((layout(&words), Engine::Tesseract)),
            // Without Tesseract, or with it failing, the neural engine can still read the image.
            Err(e) => last_err = Some(e),
        }
    }
    let dir = match crate::neural::find_models() {
        Some(dir) => dir,
        None => {
            // Without the neural models, Vision still reads a fair amount of handwriting.
            #[cfg(target_os = "macos")]
            if mode == Mode::Handwriting {
                return crate::ocr_vision::recognize(img).map(|words| (layout(&words), Engine::Vision));
            }
            if let Some(e) = last_err {
                return Err(e);
            }
            return Err(match mode {
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
            });
        }
    };
    let lines = crate::neural::recognize(&dir, img);
    // A neural engine that can't start (no ONNX Runtime, for instance) still leaves Vision.
    #[cfg(target_os = "macos")]
    let lines = match lines {
        Err(e) if mode == Mode::Handwriting => {
            return crate::ocr_vision::recognize(img).map(|words| (layout(&words), Engine::Vision)).map_err(|_| e);
        }
        lines => lines,
    };
    // Each line read goes in as a single fragment; the layout takes care of order and indentation.
    let words: Vec<Word> = lines?
        .into_iter()
        .map(|l| Word { x: l.x, y: l.y, w: l.w, h: l.h, text: l.text })
        .collect();
    Ok((layout(&words), Engine::Neural))
}

/// A recognized word and its rectangle in the image.
#[derive(Clone, Debug, PartialEq)]
pub struct WordBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
}

type Word = WordBox;

/// Recognizes words in `img` and returns their bounding boxes in `img` pixel coordinates.
pub fn recognize_boxes(img: &RgbaImage) -> Result<Vec<WordBox>, String> {
    if img.width() == 0 || img.height() == 0 {
        return Ok(Vec::new());
    }
    // The error of the last engine that failed, reported if no other one can read the image.
    #[cfg(not(target_os = "macos"))]
    let mut last_err = None;
    #[cfg(target_os = "macos")]
    let mut last_err = match crate::ocr_vision::recognize(img) {
        Ok(words) => return Ok(words),
        Err(e) => Some(e),
    };
    if let Some(install) = find_tesseract() {
        match tesseract_words_in_image_coords(&install, img) {
            Ok(words) => return Ok(words),
            Err(e) => last_err = Some(e),
        }
    }
    if let Some(dir) = crate::neural::find_models() {
        let lines = crate::neural::recognize(&dir, img)?;
        return Ok(lines
            .into_iter()
            .map(|l| WordBox { x: l.x, y: l.y, w: l.w, h: l.h, text: l.text })
            .collect());
    }
    if let Some(e) = last_err {
        return Err(e);
    }
    Err(t!(
        "nenhum motor de OCR encontrado (pastas tesseract e ocr-neural)",
        "no OCR engine found (tesseract and ocr-neural folders)"
    )
    .to_owned())
}

/// Scans `img` locally with OCR and returns `(x1, y1, x2, y2)` pixel rectangles covering sensitive
/// data (emails, API keys/tokens, passwords after labels, credit cards, CPF/CNPJ, phones, IPs).
pub fn find_sensitive_regions(img: &RgbaImage) -> Result<Vec<(f32, f32, f32, f32)>, String> {
    let words = recognize_boxes(img)?;
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let raw = detect_sensitive_boxes(&words);
    Ok(raw
        .into_iter()
        .filter_map(|(x1, y1, x2, y2)| {
            let h = (y2 - y1).max(4.0);
            let pad_x = (h * 0.18).clamp(3.0, 8.0);
            let pad_y = (h * 0.14).clamp(2.0, 6.0);
            let ax = (x1 - pad_x).clamp(0.0, iw);
            let ay = (y1 - pad_y).clamp(0.0, ih);
            let bx = (x2 + pad_x).clamp(0.0, iw);
            let by = (y2 + pad_y).clamp(0.0, ih);
            (bx - ax >= 4.0 && by - ay >= 4.0).then_some((ax, ay, bx, by))
        })
        .collect())
}

fn tesseract_words_in_image_coords(install: &TesseractInstall, img: &RgbaImage) -> Result<Vec<Word>, String> {
    let scale = if img.width() * 3 <= 9000 {
        3
    } else if img.width() * 2 <= 9000 {
        2
    } else {
        1
    };
    let max = 30_000u32;
    let scaled_w = img.width() * scale;
    let scaled_h = img.height() * scale;
    let (eff_w, eff_h, eff_scale) = if scaled_w > max {
        let h = (scaled_h as u64 * max as u64 / scaled_w as u64).max(1) as u32;
        (max, h, max as f32 / img.width().max(1) as f32)
    } else {
        (scaled_w, scaled_h, scale as f32)
    };
    let pad_x = if eff_w < MIN_SIDE { (MIN_SIDE - eff_w) / 2 } else { 0 } as f32;
    let pad_y = if eff_h < MIN_SIDE { (MIN_SIDE - eff_h) / 2 } else { 0 } as f32;

    let words = tesseract_words(install, img)?;
    Ok(words
        .into_iter()
        .map(|w| Word {
            x: ((w.x - pad_x) / eff_scale).max(0.0),
            y: ((w.y - pad_y) / eff_scale).max(0.0),
            w: (w.w / eff_scale).max(1.0),
            h: (w.h / eff_scale).max(1.0),
            text: w.text,
        })
        .collect())
}

fn trim_token(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '“' | '”' | '‘' | '’' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
        )
    })
}

fn is_email(s: &str) -> bool {
    let s = trim_token(s).trim_end_matches('.');
    let Some((local, domain)) = s.split_once('@') else { return false };
    if local.is_empty() || domain.len() < 3 || domain.contains('@') {
        return false;
    }
    let Some((host, tld)) = domain.rsplit_once('.') else { return false };
    !host.is_empty()
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && local.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
}

fn is_ipv4(s: &str) -> bool {
    let s = trim_token(s);
    // Strip optional :port
    let ip = match s.rsplit_once(':') {
        Some((left, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => left,
        _ => s,
    };
    let parts: Vec<&str> = ip.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
}

fn is_cpf_or_cnpj(s: &str) -> bool {
    let s = trim_token(s);
    let b = s.as_bytes();
    // CPF: ddd.ddd.ddd-dd (14 chars)
    let is_cpf = b.len() == 14
        && b[3] == b'.'
        && b[7] == b'.'
        && b[11] == b'-'
        && b.iter().enumerate().all(|(i, c)| matches!(i, 3 | 7 | 11) || c.is_ascii_digit());
    // CNPJ: dd.ddd.ddd/dddd-dd (18 chars)
    let is_cnpj = b.len() == 18
        && b[2] == b'.'
        && b[6] == b'.'
        && b[10] == b'/'
        && b[15] == b'-'
        && b.iter().enumerate().all(|(i, c)| matches!(i, 2 | 6 | 10 | 15) || c.is_ascii_digit());
    is_cpf || is_cnpj
}

fn is_api_key_or_token(s: &str) -> bool {
    let s = trim_token(s);
    if s.len() < 12 {
        return false;
    }
    // Check inline key=value or token=... assignments
    let lower = s.to_ascii_lowercase();
    for k in ["api_key=", "apikey=", "secret=", "token=", "password=", "senha=", "access_key=", "client_secret="] {
        if let Some(pos) = lower.find(k)
            && s.len() > pos + k.len() + 3
        {
            return true;
        }
    }
    const PREFIXES: &[&str] = &[
        "sk-",
        "sk_live_",
        "sk_test_",
        "rk_live_",
        "pk_live_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "AIza",
        "AKIA",
        "ASIA",
        "xoxb-",
        "xoxp-",
        "glpat-",
    ];
    if PREFIXES.iter().any(|p| s.starts_with(p)) && s.len() >= 14 {
        return true;
    }
    if s.starts_with("eyJ") && s.len() >= 20 && s.contains('.') {
        return true;
    }
    // Long high-entropy secret token (24+ token chars, mixes upper/lower/digits, not a URL/path)
    if s.len() >= 24
        && !s.contains('/')
        && !s.contains('\\')
        && !lower.starts_with("http")
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
    {
        let has_alpha = s.chars().any(|c| c.is_ascii_alphabetic());
        let has_digit = s.chars().any(|c| c.is_ascii_digit());
        let has_upper = s.chars().any(|c| c.is_ascii_uppercase());
        let has_lower = s.chars().any(|c| c.is_ascii_lowercase());
        let hex_only = s.len() >= 32 && s.chars().all(|c| c.is_ascii_hexdigit());
        if hex_only || (has_alpha && has_digit && (has_upper && has_lower || s.len() >= 28)) {
            return true;
        }
    }
    false
}

fn is_secret_label(s: &str) -> bool {
    let clean = trim_token(s)
        .trim_end_matches([':', '='])
        .to_ascii_lowercase();
    matches!(
        clean.as_str(),
        "password"
            | "passwd"
            | "pwd"
            | "senha"
            | "secret"
            | "segredo"
            | "api_key"
            | "apikey"
            | "api-key"
            | "access_key"
            | "private_key"
            | "client_secret"
            | "auth_token"
            | "token"
            | "bearer"
            | "chave"
    )
}

fn luhn_valid(digits: &[u8]) -> bool {
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    // Reject all-identical digits like 0000000000000
    if digits.iter().all(|&d| d == digits[0]) {
        return false;
    }
    let mut sum = 0u32;
    for (i, &d) in digits.iter().rev().enumerate() {
        let mut v = (d - b'0') as u32;
        if i % 2 == 1 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    sum.is_multiple_of(10)
}

fn looks_like_phone_span(tokens: &[&str]) -> bool {
    let joined = tokens.join(" ");
    let trimmed = trim_token(&joined);
    if !trimmed.chars().all(|c| c.is_ascii_digit() || " +-().–".contains(c)) {
        return false;
    }
    let digits: usize = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
    let has_sep = trimmed.contains('-') || trimmed.contains('(') || trimmed.starts_with('+');
    has_sep && (10..=13).contains(&digits)
}

/// Pure geometric & lexical detector over recognized words: returns `(x1, y1, x2, y2)` boxes.
pub fn detect_sensitive_boxes(words: &[WordBox]) -> Vec<(f32, f32, f32, f32)> {
    let Some(line_h) = median(words.iter().map(|w| w.h).collect()) else {
        return Vec::new();
    };
    let mut sorted: Vec<&WordBox> = words.iter().collect();
    sorted.sort_by(|a, b| (a.y + a.h * 0.5).total_cmp(&(b.y + b.h * 0.5)));
    let mut rows: Vec<(f32, Vec<&WordBox>)> = Vec::new();
    for w in sorted {
        let cy = w.y + w.h * 0.5;
        match rows.last_mut() {
            Some((row_cy, row)) if (cy - *row_cy).abs() < line_h * 0.55 => {
                row.push(w);
                *row_cy += (cy - *row_cy) / row.len() as f32;
            }
            _ => rows.push((cy, vec![w])),
        }
    }

    let mut hits: Vec<(f32, f32, f32, f32)> = Vec::new();
    let span_box = |slice: &[&WordBox]| -> (f32, f32, f32, f32) {
        let x1 = slice.iter().map(|w| w.x).fold(f32::INFINITY, f32::min);
        let y1 = slice.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
        let x2 = slice.iter().map(|w| w.x + w.w).fold(f32::NEG_INFINITY, f32::max);
        let y2 = slice.iter().map(|w| w.y + w.h).fold(f32::NEG_INFINITY, f32::max);
        (x1, y1, x2, y2)
    };

    for (_, mut row) in rows {
        row.sort_by(|a, b| a.x.total_cmp(&b.x));
        let n = row.len();

        for i in 0..n {
            let t = row[i].text.as_str();
            if is_email(t) || is_ipv4(t) || is_cpf_or_cnpj(t) || is_api_key_or_token(t) {
                hits.push(span_box(&row[i..=i]));
            }
            // Split email across adjacent tokens (e.g. "user@" + "domain.com")
            if i + 1 < n && row[i + 1].x - (row[i].x + row[i].w) < line_h * 1.2 {
                let pair = format!("{}{}", row[i].text, row[i + 1].text);
                if is_email(&pair) {
                    hits.push(span_box(&row[i..=i + 1]));
                }
            }
            // Key: Value secret label (e.g. "Password: mySecret123" or "API Key : sk_123")
            let raw_trim = trim_token(t);
            let has_colon = raw_trim.ends_with(':') || raw_trim.ends_with('=');
            if is_secret_label(t) {
                let mut val_idx = if has_colon {
                    Some(i + 1)
                } else if i + 1 < n && matches!(row[i + 1].text.as_str(), ":" | "=") {
                    Some(i + 2)
                } else {
                    None
                };
                if let Some(vi) = val_idx.take().filter(|&vi| vi < n) {
                    let val_tok = trim_token(&row[vi].text);
                    if !val_tok.is_empty() && !is_secret_label(val_tok) {
                        hits.push(span_box(&row[vi..=vi]));
                    }
                }
            } else if i >= 1
                && row[i - 1].text.eq_ignore_ascii_case("api")
                && matches!(
                    raw_trim.trim_end_matches([':', '=']).to_ascii_lowercase().as_str(),
                    "key" | "token" | "secret"
                )
            {
                let next_i = if has_colon {
                    Some(i + 1)
                } else if i + 1 < n && matches!(row[i + 1].text.as_str(), ":" | "=") {
                    Some(i + 2)
                } else {
                    None
                };
                if let Some(vi) = next_i.filter(|&vi| vi < n) {
                    hits.push(span_box(&row[vi..=vi]));
                }
            }

            // Multi-token credit cards (Luhn) & phone numbers (1..=5 tokens)
            for end in i..(i + 5).min(n) {
                if end > i && row[end].x - (row[end - 1].x + row[end - 1].w) > line_h * 2.5 {
                    break;
                }
                let slice = &row[i..=end];
                let texts: Vec<&str> = slice.iter().map(|w| w.text.as_str()).collect();
                if looks_like_phone_span(&texts) {
                    hits.push(span_box(slice));
                }
                let joined = texts.join("");
                let clean = trim_token(&joined);
                if clean.chars().all(|c| c.is_ascii_digit() || c == '-') {
                    let digits: Vec<u8> = clean.bytes().filter(|b| b.is_ascii_digit()).collect();
                    if luhn_valid(&digits) {
                        hits.push(span_box(slice));
                    }
                }
            }
        }
    }

    // Merge overlapping or nearly touching boxes on the same line
    hits.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    let mut merged: Vec<(f32, f32, f32, f32)> = Vec::new();
    for b in hits {
        if let Some(prev) = merged.iter_mut().find(|m| {
            let v_overlap = m.1.max(b.1) < m.3.min(b.3);
            let h_touch = b.0 <= m.2 + 4.0 && b.2 >= m.0 - 4.0;
            v_overlap && h_touch
        }) {
            prev.0 = prev.0.min(b.0);
            prev.1 = prev.1.min(b.1);
            prev.2 = prev.2.max(b.2);
            prev.3 = prev.3.max(b.3);
        } else {
            merged.push(b);
        }
    }
    merged
}

// ---------------------------------------------------------------- Tesseract

struct TesseractInstall {
    bin: PathBuf,
    tessdata: PathBuf,
}

/// Tesseract binary and `tessdata` folder, if available.
fn find_tesseract() -> Option<TesseractInstall> {
    let bin_name = crate::platform::exe_name("tesseract");
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
    if let Some(dir) = candidates.iter().find(|d| d.join(&bin_name).is_file() && d.join("tessdata").is_dir()) {
        return Some(TesseractInstall {
            bin: dir.join(&bin_name),
            tessdata: dir.join("tessdata"),
        });
    }
    #[cfg(target_os = "linux")]
    {
        let bin = std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|p| p.join("tesseract"))
                    .find(|p| p.is_file())
            })
            .or_else(|| {
                let p = PathBuf::from("/usr/bin/tesseract");
                p.is_file().then_some(p)
            })?;
        let tessdata = candidates
            .into_iter()
            .map(|d| d.join("tessdata"))
            .chain(
                [
                    "/usr/share/tessdata",
                    "/usr/share/tesseract-ocr/5/tessdata",
                    "/usr/share/tesseract-ocr/4.00/tessdata",
                ]
                .into_iter()
                .map(PathBuf::from),
            )
            .find(|d| d.is_dir())?;
        return Some(TesseractInstall { bin, tessdata });
    }
    #[cfg(not(target_os = "linux"))]
    None
}

/// Languages installed in `tessdata`, with Portuguese and English first: "por+eng".
#[cfg_attr(not(test), allow(dead_code))]
fn tesseract_languages(dir: &Path) -> String {
    tesseract_languages_in(&dir.join("tessdata"))
}

fn tesseract_languages_in(tessdata: &Path) -> String {
    let mut langs: Vec<String> = std::fs::read_dir(tessdata)
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

fn tesseract_words(install: &TesseractInstall, img: &RgbaImage) -> Result<Vec<Word>, String> {
    // Scaling 3× helps Tesseract the most with screen text; the cap avoids giant images.
    let scale = if img.width() * 3 <= 9000 {
        3
    } else if img.width() * 2 <= 9000 {
        2
    } else {
        1
    };
    let prepared = prepare(img, scale, 30_000);
    let langs = tesseract_languages_in(&install.tessdata);
    if langs.is_empty() {
        return Err(t!("nenhum idioma em tessdata", "no language in tessdata").to_owned());
    }

    static NEXT_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run_id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut words = Vec::new();
    for (index, (top, bottom)) in split_rows(&prepared, 6000).into_iter().enumerate() {
        let tile = image::imageops::crop_imm(&prepared, 0, top, prepared.width(), bottom - top).to_image();
        let path = std::env::temp_dir().join(format!("snapkit-ocr-{}-{run_id}-{index}.png", std::process::id()));
        tile.save(&path).map_err(|e| e.to_string())?;
        let output = crate::platform::background_command(&install.bin)
            .arg(&path)
            .arg("stdout")
            .args(["-l", &langs])
            // Mode 11: sparse text, no column assumptions. The best for screens with several
            // panels.
            .args(["--psm", "11"])
            .args(["-c", "tessedit_create_tsv=1"])
            .env("TESSDATA_PREFIX", &install.tessdata)
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
pub(crate) fn prepare(img: &RgbaImage, scale: u32, max: u32) -> RgbaImage {
    if img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
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
pub(crate) fn split_rows(img: &RgbaImage, tile: u32) -> Vec<(u32, u32)> {
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
    if w == 0 {
        return true;
    }
    let row = &img.as_raw()[y as usize * w * 4..(y as usize + 1) * w * 4];
    let (pixels, _) = row.as_chunks::<4>();
    let first = &pixels[0][..3];
    pixels.iter().all(|px| &px[..3] == first)
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
        assert_eq!(prepare(&white(0, 0), 2, 10_000).dimensions(), (0, 0));
        assert_eq!(recognize(&white(0, 10), Mode::Printed).unwrap().0, "");
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
            let op = Op::Text { pos: (x, y), text: text.into(), color: [230, 230, 230, 255], size: 34.0, boxed: false, angle: 0.0 };
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
        let install = find_tesseract().expect("Tesseract not found: run scripts/fetch-tesseract.ps1");
        let text = layout(&tesseract_words(&install, &sample()).unwrap());
        assert_sample_text(&text);
        assert!(text.contains("espaço"), "the Portuguese model must read the cedilla: {text:?}");
        assert_eq!(recognize(&sample(), Mode::Printed).unwrap().1, Engine::PRINTED, "the default engine has priority");
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

    #[test]
    fn sensitive_detector_finds_emails_secrets_cpf_cards_phones_and_ips() {
        let words = vec![
            word(0, 0, "Email:"),
            word(8, 0, "cliente@empresa.com.br"),
            word(0, 1, "API"),
            word(4, 1, "Key:"),
            word(9, 1, "sk-proj-AbCdEf1234567890XyZ"),
            word(0, 2, "CPF:"),
            word(6, 2, "123.456.789-00"),
            word(0, 3, "Cartão:"),
            word(9, 3, "4532"),
            word(14, 3, "0151"),
            word(19, 3, "1283"),
            word(24, 3, "0366"),
            word(0, 4, "Tel:"),
            word(6, 4, "(11)"),
            word(11, 4, "98765-4321"),
            word(0, 5, "Host:"),
            word(7, 5, "192.168.1.42"),
            word(0, 6, "Senha:"),
            word(8, 6, "minhaSenhaSecreta!"),
            word(0, 7, "Clique"),
            word(8, 7, "em"),
            word(12, 7, "Salvar"),
            word(20, 7, "2026"),
        ];
        let boxes = detect_sensitive_boxes(&words);
        assert_eq!(boxes.len(), 7, "expected 7 sensitive regions, got {boxes:?}");
        // Verify none of the boxes are on row 7 ("Clique em Salvar 2026", y ~ 143)
        assert!(boxes.iter().all(|b| b.1 < 135.0), "{boxes:?}");
        // Verify label "Email:" at x=0..60 is NOT inside row 0's redaction box (starts at col 8 -> x=80)
        assert_eq!(boxes[0].0, 80.0);
    }

    #[test]
    fn find_sensitive_regions_maps_ocr_boxes_back_to_image_pixels() {
        if crate::in_ci() {
            return;
        }
        let mut img = white(640, 160);
        let font = crate::editor::default_font();
        render::apply(
            &mut img,
            &Op::Text {
                pos: (24.0, 30.0),
                text: "Contato: maria@exemplo.com".into(),
                color: [20, 20, 20, 255],
                size: 28.0,
                boxed: false,
                angle: 0.0,
            },
            &font,
        );
        render::apply(
            &mut img,
            &Op::Text {
                pos: (24.0, 90.0),
                text: "CPF: 123.456.789-00".into(),
                color: [20, 20, 20, 255],
                size: 28.0,
                boxed: false,
                angle: 0.0,
            },
            &font,
        );
        let boxes = find_sensitive_regions(&img).unwrap();
        assert!(boxes.len() >= 2, "expected email and CPF boxes: {boxes:?}");
        // Both boxes must lie after the label ("Contato:" / "CPF:") and within image bounds.
        for &(x1, y1, x2, y2) in &boxes {
            assert!(x1 > 70.0 && x2 <= 640.0 && y1 >= 0.0 && y2 <= 160.0, "box=({x1},{y1},{x2},{y2})");
        }
    }
}
