use image::{DynamicImage, GenericImageView, ImageFormat, imageops};
use std::io::{Cursor};

#[derive(Debug, Clone)]
pub struct ImageProcessingOptions {
    pub size: Option<u32>,
    pub background: bool,
    pub background_blur: f32,
    pub corner_radius: Option<f32>,
}

impl Default for ImageProcessingOptions {
    fn default() -> Self {
        Self {
            size: None,
            background: true,
            background_blur: 3.0,
            corner_radius: Some(4.0),
        }
    }
}

pub fn make_square_with_blur(input_bytes: &[u8], options: &ImageProcessingOptions) -> Result<Vec<u8>, image::ImageError> {
    let img = image::load_from_memory(input_bytes)?;
    let (width, height) = img.dimensions();
    let size = options.size.unwrap_or_else(|| width.max(height));

    let fg_buf = if width > height {
        let new_height = (size as f32 * (height as f32 / width as f32)) as u32;
        imageops::resize(&img, size, new_height, imageops::FilterType::Lanczos3)
    } else {
        let new_width = (size as f32 * (width as f32 / height as f32)) as u32;
        imageops::resize(&img, new_width, size, imageops::FilterType::Lanczos3)
    };

    let fg_dyn = DynamicImage::ImageRgba8(fg_buf);
    let (fg_w, fg_h) = fg_dyn.dimensions();
    let mut canvas = DynamicImage::new_rgba8(size, size);

    if options.background {
        let bg_buf = imageops::resize(&img, size, size, imageops::FilterType::Gaussian);
        let mut bg_dyn = DynamicImage::ImageRgba8(bg_buf);
        if options.background_blur > 0.0 {
            let blur_radius = (size as f32) * (options.background_blur / 100.0);
            bg_dyn = DynamicImage::ImageRgba8(imageops::blur(&bg_dyn, blur_radius));
        }
        imageops::overlay(&mut canvas, &bg_dyn, 0, 0);
        imageops::overlay(&mut canvas, &fg_dyn, ((size - fg_w) / 2) as i64, ((size - fg_h) / 2) as i64);
    } else {
        let mut fg_rounded = fg_dyn;
        
        if let Some(radius_percent) = options.corner_radius {
            let radius = ((size as f32) * (radius_percent / 100.0)) as u32;
            if radius > 0 {
                apply_rounded_corners(&mut fg_rounded, radius);
            }
        }

        imageops::overlay(&mut canvas, &fg_rounded, ((size - fg_w) / 2) as i64, ((size - fg_h) / 2) as i64);
    }

    let mut buf = Vec::new();
    canvas.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)?;
    Ok(buf)
}

fn apply_rounded_corners(img: &mut DynamicImage, radius: u32) {
    let (width, height) = img.dimensions();
    let rgba = img.as_mut_rgba8().unwrap();
    let radius_f = radius as f32;

    for y in 0..height {
        for x in 0..width {
            let corner_center = if x < radius && y < radius {
                Some((radius - 1, radius - 1))
            } else if x >= width - radius && y < radius {
                Some((width - radius, radius - 1))
            } else if x < radius && y >= height - radius {
                Some((radius - 1, height - radius))
            } else if x >= width - radius && y >= height - radius {
                Some((width - radius, height - radius))
            } else {
                None
            };

            if let Some((cx, cy)) = corner_center {
                let dx = x as f32 - cx as f32;
                let dy = y as f32 - cy as f32;
                let distance = (dx * dx + dy * dy).sqrt();
                
                if distance > radius_f {
                    let pixel = rgba.get_pixel_mut(x, y);
                    pixel[3] = 0;
                } else if distance > radius_f - 1.0 {
                    let alpha_factor = radius_f - distance;
                    let pixel = rgba.get_pixel_mut(x, y);
                    pixel[3] = (pixel[3] as f32 * alpha_factor).round() as u8;
                }
            }
        }
    }
}
