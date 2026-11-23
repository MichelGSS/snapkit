//! Pin to Screen on macOS: not available yet. Same API as `pin.rs`.

use image::RgbaImage;

use crate::platform::PxRect;

pub fn spawn(_image: RgbaImage, _accent_rgb: [u8; 3], _at_rect: Option<PxRect>) -> Result<(), String> {
    Err(t!("fixar na tela ainda não está disponível no macOS", "pin to screen isn't available on macOS yet").to_owned())
}

pub fn close_all() {}

#[allow(dead_code)]
pub fn active_count() -> usize {
    0
}
