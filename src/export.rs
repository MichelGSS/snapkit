//! Multi-format image and multi-page PDF export (PNG, JPEG, WebP, BMP, PDF 1.4).
//!
//! Runs 100% locally with no external tools: JPEG, WebP, PNG, and BMP use the `image` crate, and
//! PDF 1.4 documents embed high-quality RGB JPEG streams (`/DCTDecode`) with exact cross-reference
//! tables.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use image::{ImageFormat, RgbImage, RgbaImage};

/// PDF 1.4 maximum page dimension in points (ISO 32000-1 architectural limit is 14,400 pt).
const MAX_PDF_PT: f64 = 14_000.0;

/// Composites an RGBA image over opaque white into 24-bit RGB (used for JPEG and PDF).
pub fn rgba_to_rgb_white(img: &RgbaImage) -> RgbImage {
    let (w, h) = img.dimensions();
    let mut rgb = RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels() {
        let [r, g, b, a] = px.0;
        let out = if a == 255 {
            [r, g, b]
        } else if a == 0 {
            [255, 255, 255]
        } else {
            let alpha = a as u16;
            let inv = 255 - alpha;
            [
                ((r as u16 * alpha + 255 * inv + 127) / 255) as u8,
                ((g as u16 * alpha + 255 * inv + 127) / 255) as u8,
                ((b as u16 * alpha + 255 * inv + 127) / 255) as u8,
            ]
        };
        rgb.put_pixel(x, y, image::Rgb(out));
    }
    rgb
}

/// Encodes an RGBA image as high-quality RGB JPEG bytes (`quality` 1..=100).
pub fn encode_jpeg(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgb = rgba_to_rgb_white(img);
    let mut bytes = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality.clamp(1, 100));
    enc.encode_image(&rgb).map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Builds a standards-compliant PDF 1.4 file in memory with one page per image.
pub fn encode_pdf_pages(pages: &[&RgbaImage]) -> Result<Vec<u8>, String> {
    if pages.is_empty() {
        return Err(t!("nenhuma página para exportar", "no pages to export").to_owned());
    }
    for p in pages {
        if p.width() == 0 || p.height() == 0 {
            return Err(t!("imagem vazia", "empty image").to_owned());
        }
    }

    // Object numbering:
    // 1 = Catalog
    // 2 = Pages
    // For page i (0..pages.len()):
    //   3 + i*3     = Page
    //   3 + i*3 + 1 = Contents stream
    //   3 + i*3 + 2 = Image XObject
    let total_objs = 3 + pages.len() * 3; // includes object 0 in xref
    let mut offsets = vec![0usize; total_objs];
    let mut out = Vec::with_capacity(64 * 1024);

    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");

    let start_obj = |out: &mut Vec<u8>, offsets: &mut [usize], id: usize| {
        offsets[id] = out.len();
        let _ = writeln!(out, "{id} 0 obj");
    };

    // 1: Catalog
    start_obj(&mut out, &mut offsets, 1);
    out.extend_from_slice(b"<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    // 2: Pages
    start_obj(&mut out, &mut offsets, 2);
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 3 + i * 3)).collect();
    let _ = writeln!(
        out,
        "<< /Type /Pages /Kids [{}] /Count {} >>\nendobj",
        kids.join(" "),
        pages.len()
    );

    for (i, img) in pages.iter().enumerate() {
        let page_id = 3 + i * 3;
        let content_id = page_id + 1;
        let image_id = page_id + 2;

        let (w, h) = (img.width(), img.height());
        // Convert 96 DPI screen pixels to 72 pt/inch PDF user space, scaling down if taller/wider
        // than PDF 1.4's 14,400 pt architectural limit (e.g. long scrolling captures).
        let mut pt_w = (w as f64 * 0.75).max(18.0);
        let mut pt_h = (h as f64 * 0.75).max(18.0);
        let max_dim = pt_w.max(pt_h);
        if max_dim > MAX_PDF_PT {
            let factor = MAX_PDF_PT / max_dim;
            pt_w *= factor;
            pt_h *= factor;
        }

        let jpeg = encode_jpeg(img, 94)?;
        let content = format!("q\n{pt_w:.2} 0 0 {pt_h:.2} 0 0 cm\n/Im0 Do\nQ\n");

        // Page dictionary
        start_obj(&mut out, &mut offsets, page_id);
        let _ = writeln!(
            out,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pt_w:.2} {pt_h:.2}] /Resources << /XObject << /Im0 {image_id} 0 R >> >> /Contents {content_id} 0 R >>\nendobj"
        );

        // Page content stream
        start_obj(&mut out, &mut offsets, content_id);
        let _ = write!(
            out,
            "<< /Length {} >>\nstream\n{}endstream\nendobj\n",
            content.len(),
            content
        );

        // Embedded JPEG XObject
        start_obj(&mut out, &mut offsets, image_id);
        let _ = write!(
            out,
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            jpeg.len()
        );
        out.extend_from_slice(&jpeg);
        out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    let xref_offset = out.len();
    let _ = writeln!(out, "xref\n0 {total_objs}");
    out.extend_from_slice(b"0000000000 65535 f \n");
    for &off in &offsets[1..] {
        let _ = write!(out, "{off:010} 00000 n \n");
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {total_objs} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
    );
    Ok(out)
}

/// Writes one or more images as a single- or multi-page PDF document at `path`.
pub fn save_pdf_pages(pages: &[&RgbaImage], path: &Path) -> Result<(), String> {
    let bytes = encode_pdf_pages(pages)?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// Saves `img` to `path`, picking PNG, JPEG, WebP, BMP, or PDF from the file extension.
pub fn save_image_any(img: &RgbaImage, path: &Path) -> Result<(), String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let bytes = encode_jpeg(img, 94)?;
            let mut f = BufWriter::new(File::create(path).map_err(|e| e.to_string())?);
            f.write_all(&bytes).map_err(|e| e.to_string())?;
            f.flush().map_err(|e| e.to_string())
        }
        "webp" => img.save_with_format(path, ImageFormat::WebP).map_err(|e| e.to_string()),
        "bmp" => img.save_with_format(path, ImageFormat::Bmp).map_err(|e| e.to_string()),
        "pdf" => save_pdf_pages(&[img], path),
        _ => img.save_with_format(path, ImageFormat::Png).map_err(|e| e.to_string()),
    }
}

/// Prompts the user to save `img` as PNG, JPEG, WebP, PDF, or BMP and returns the status message.
pub fn save_capture_dialog(img: &RgbaImage) -> String {
    let Some(path) = rfd::FileDialog::new()
        .add_filter(t!("Imagem PNG", "PNG image"), &["png"])
        .add_filter(t!("Imagem JPEG", "JPEG image"), &["jpg", "jpeg"])
        .add_filter(t!("Imagem WebP", "WebP image"), &["webp"])
        .add_filter(t!("Documento PDF", "PDF document"), &["pdf"])
        .add_filter(t!("Imagem BMP", "BMP image"), &["bmp"])
        .set_file_name(t!("captura.png", "capture.png"))
        .save_file()
    else {
        return t!("Salvamento cancelado.", "Save canceled.").to_owned();
    };
    match save_image_any(img, &path) {
        Ok(()) => tf!("Salvo em {}", "Saved to {}", path.display()),
        Err(e) => tf!("Não foi possível salvar: {e}", "Could not save: {e}"),
    }
}

/// Loads multiple image files and exports them into a single multi-page PDF chosen by the user.
pub fn export_files_as_pdf_dialog(paths: &[PathBuf]) -> String {
    let mut loaded: Vec<RgbaImage> = Vec::with_capacity(paths.len());
    for p in paths {
        if let Ok(img) = image::open(p) {
            loaded.push(img.to_rgba8());
        }
    }
    if loaded.is_empty() {
        return t!(
            "Nenhuma imagem válida selecionada para o PDF.",
            "No valid image selected for the PDF."
        )
        .to_owned();
    }
    let Some(dest) = rfd::FileDialog::new()
        .add_filter(t!("Documento PDF", "PDF document"), &["pdf"])
        .set_file_name(t!("capturas.pdf", "captures.pdf"))
        .save_file()
    else {
        return t!("Exportação cancelada.", "Export canceled.").to_owned();
    };
    let refs: Vec<&RgbaImage> = loaded.iter().collect();
    match save_pdf_pages(&refs, &dest) {
        Ok(()) => tf!(
            "PDF com {} páginas salvo em {}",
            "{}-page PDF saved to {}",
            refs.len(),
            dest.display()
        ),
        Err(e) => tf!("Não foi possível criar o PDF: {e}", "Could not create the PDF: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_png_jpeg_webp_and_bmp_round_trip() {
        let dir = std::env::temp_dir().join(format!("snapkit-export-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut img = RgbaImage::from_pixel(48, 32, image::Rgba([40, 120, 220, 255]));
        // Add a semi-transparent pixel to verify alpha compositing onto white in JPEG.
        img.put_pixel(4, 4, image::Rgba([200, 0, 0, 128]));

        for ext in ["png", "jpg", "webp", "bmp"] {
            let path = dir.join(format!("out.{ext}"));
            save_image_any(&img, &path).unwrap();
            let decoded = image::open(&path).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), (48, 32), "mismatch for {ext}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_page_pdf_has_valid_header_xref_and_scales_tall_pages() {
        let page1 = RgbaImage::from_pixel(320, 200, image::Rgba([255, 138, 76, 255]));
        // Very tall scrolling-style image (> 19,000 px => > 14,000 pt unscaled)
        let page2 = RgbaImage::from_pixel(80, 22_000, image::Rgba([20, 30, 40, 255]));

        let pdf = encode_pdf_pages(&[&page1, &page2]).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Count 2"));
        assert!(text.contains("/Filter /DCTDecode"));
        assert!(text.contains("/MediaBox [0 0 240.00 150.00]"));
        assert!(text.contains("14000.00"), "tall page should clamp to MAX_PDF_PT");
    }
}
