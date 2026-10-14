use std::io::Cursor;

use image::{imageops::FilterType, ImageFormat, ImageReader};
use loco_rs::{Error, Result};

pub fn compress_logo(bytes: &[u8]) -> Result<Vec<u8>> {
    let fail = || Error::Message("Use a PNG or JPEG logo up to 5 MB.".into());
    if bytes.is_empty() || bytes.len() > 5 * 1024 * 1024 {
        return Err(fail());
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| fail())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(fail());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let logo = reader
        .decode()
        .map_err(|_| fail())?
        .resize(512, 512, FilterType::Lanczos3);
    for side in [512, 384, 256, 128] {
        let resized = logo.resize(side, side, FilterType::Lanczos3);
        let mut square = image::DynamicImage::new_rgba8(side, side);
        image::imageops::overlay(
            &mut square,
            &resized,
            i64::from((side - resized.width()) / 2),
            i64::from((side - resized.height()) / 2),
        );
        let mut output = Cursor::new(Vec::new());
        square
            .write_to(&mut output, ImageFormat::Png)
            .map_err(|_| fail())?;
        if output.get_ref().len() <= 500 * 1024 {
            return Ok(output.into_inner());
        }
    }
    Err(fail())
}
