//! Frozen, blurred wallpaper for the lock screen.
//!
//! GTK4's layer-shell backend has no equivalent of `ext-session-lock-v1`, so
//! the lock surface is an ordinary overlay layer. Leaving it translucent
//! would expose whatever was on screen, so instead the screen is captured
//! the instant before the lock appears and that still frame -- blurred and
//! dimmed -- becomes the background. The effect matches hyprlock's
//! `background { blur_passes }` while keeping the readable UI confined to a
//! card in the middle of the output.
//!
//! Ordering matters: the capture must complete *before* the lock window is
//! mapped, or grim photographs the lock screen itself. Capture is therefore
//! synchronous (it is only a pipe read), while the expensive blur runs on a
//! worker thread and fades in when it lands.

use std::{
    io::Read,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use wait_timeout::ChildExt;

/// grim writes uncompressed PPM far faster than it encodes PNG, and parsing
/// P6 needs no image decoder at all. At 4K this is ~25 MiB through a pipe,
/// which costs a few milliseconds.
const CAPTURE_FORMAT: &str = "ppm";
/// A screenshot that has not arrived by now is never going to; fall back to
/// a flat backdrop rather than delaying the lock.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
/// Refuse absurd headers rather than trying to allocate from them.
const MAX_DIMENSION: usize = 16_384;

/// An RGB8 image with no row padding.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// `width * height * 3` bytes.
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn stride(&self) -> usize {
        self.width * 3
    }
}

/// How the captured frame is turned into a backdrop.
#[derive(Debug, Clone, Copy)]
pub struct BlurSettings {
    /// Box-blur radius, in downscaled pixels.
    pub radius: usize,
    /// Integer shrink factor applied before blurring.
    pub downscale: usize,
    /// Brightness multiplier in `0.0..=1.0`, applied after the blur.
    pub dim: f64,
}

/// Captures one output with grim.
///
/// Blocking, but only for as long as grim takes to read the framebuffer.
/// Must be called before the lock surface is mapped.
pub fn capture(connector: &str) -> Result<Image> {
    let mut child = Command::new("grim")
        .arg("-o")
        .arg(connector)
        .arg("-t")
        .arg(CAPTURE_FORMAT)
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to run grim; install it to get a blurred lock backdrop")?;

    let mut buffer = Vec::new();
    // Drain the pipe first: grim blocks writing a full-screen PPM long
    // before it exits, so waiting on the child before reading would
    // deadlock on the pipe buffer.
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_end(&mut buffer)
            .context("failed to read the screenshot from grim")?;
    }
    match child.wait_timeout(CAPTURE_TIMEOUT) {
        Ok(Some(status)) if status.success() => {}
        Ok(Some(status)) => bail!("grim exited with {status}"),
        Ok(None) => {
            let _ = child.kill();
            bail!("grim did not finish within {CAPTURE_TIMEOUT:?}");
        }
        Err(error) => bail!("failed to wait for grim: {error}"),
    }
    parse_ppm(&buffer)
}

/// Parses the binary `P6` netpbm variant grim emits.
fn parse_ppm(data: &[u8]) -> Result<Image> {
    let mut cursor = 0usize;
    let magic = next_token(data, &mut cursor).context("screenshot is not a netpbm image")?;
    if magic != b"P6" {
        bail!("expected a binary P6 screenshot from grim");
    }
    let width = next_number(data, &mut cursor).context("screenshot has no width")?;
    let height = next_number(data, &mut cursor).context("screenshot has no height")?;
    let max_value = next_number(data, &mut cursor).context("screenshot has no max value")?;
    if max_value != 255 {
        bail!("expected 8-bit samples, got a max value of {max_value}");
    }
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        bail!("implausible screenshot dimensions {width}x{height}");
    }
    // Exactly one whitespace byte separates the header from the raster.
    cursor += 1;

    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .context("screenshot dimensions overflow")?;
    let raster = data
        .get(cursor..cursor + expected)
        .context("screenshot is truncated")?;
    Ok(Image {
        width,
        height,
        pixels: raster.to_vec(),
    })
}

/// Reads the next whitespace-delimited token, skipping `#` comments.
fn next_token<'a>(data: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    loop {
        while data.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
            *cursor += 1;
        }
        if data.get(*cursor) != Some(&b'#') {
            break;
        }
        while data.get(*cursor).is_some_and(|byte| *byte != b'\n') {
            *cursor += 1;
        }
    }
    let start = *cursor;
    while data
        .get(*cursor)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        *cursor += 1;
    }
    (start != *cursor).then(|| &data[start..*cursor])
}

fn next_number(data: &[u8], cursor: &mut usize) -> Option<usize> {
    let token = next_token(data, cursor)?;
    std::str::from_utf8(token).ok()?.parse().ok()
}

/// Downscales, blurs, and dims a captured frame.
///
/// Expensive (hundreds of milliseconds at 4K); run it on a worker thread.
pub fn process(image: &Image, settings: BlurSettings) -> Image {
    // Shrinking first is what makes this cheap: a box blur costs O(pixels)
    // per pass regardless of radius, so a 6x shrink is a 36x saving, and
    // the radius shrinks with it. Scaling back up in the compositor is free
    // and hides the box blur's characteristic banding.
    let mut small = downscale(image, settings.downscale.max(1));
    // Three box passes converge on a Gaussian closely enough that the
    // difference is invisible at this scale (central limit theorem).
    for _ in 0..3 {
        box_blur(&mut small, settings.radius);
    }
    dim(&mut small, settings.dim);
    small
}

/// Averages each `factor`x`factor` block into one pixel.
fn downscale(image: &Image, factor: usize) -> Image {
    if factor <= 1 {
        return image.clone();
    }
    let width = image.width.div_ceil(factor);
    let height = image.height.div_ceil(factor);
    let mut pixels = vec![0u8; width * height * 3];

    for out_y in 0..height {
        let y_start = out_y * factor;
        let y_end = (y_start + factor).min(image.height);
        for out_x in 0..width {
            let x_start = out_x * factor;
            let x_end = (x_start + factor).min(image.width);
            let mut sums = [0u32; 3];
            let mut count = 0u32;
            for y in y_start..y_end {
                let row = y * image.stride();
                for x in x_start..x_end {
                    let offset = row + x * 3;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += u32::from(image.pixels[offset + channel]);
                    }
                    count += 1;
                }
            }
            let offset = (out_y * width + out_x) * 3;
            for (channel, sum) in sums.iter().enumerate() {
                pixels[offset + channel] = (sum / count.max(1)) as u8;
            }
        }
    }
    Image {
        width,
        height,
        pixels,
    }
}

/// Separable box blur with a sliding window, so cost is independent of radius.
fn box_blur(image: &mut Image, radius: usize) {
    if radius == 0 {
        return;
    }
    blur_horizontal(image, radius);
    transpose(image);
    blur_horizontal(image, radius);
    transpose(image);
}

fn blur_horizontal(image: &mut Image, radius: usize) {
    let width = image.width;
    if width == 0 {
        return;
    }
    let radius = radius.min(width - 1);
    let window = (radius * 2 + 1) as u32;
    let mut row_out = vec![0u8; width * 3];

    for y in 0..image.height {
        let row = y * width * 3;
        let sample = |x: usize, channel: usize| -> u32 {
            // Clamp-to-edge, so the blur does not darken the borders the
            // way a zero-padded kernel would.
            u32::from(image.pixels[row + x.min(width - 1) * 3 + channel])
        };
        for channel in 0..3 {
            // Prime the window over [-radius, radius], clamped at the edge.
            let mut sum: u32 = sample(0, channel) * radius as u32;
            for x in 0..=radius {
                sum += sample(x, channel);
            }
            for x in 0..width {
                row_out[x * 3 + channel] = (sum / window) as u8;
                let leaving = sample(x.saturating_sub(radius), channel);
                let entering = sample(x + radius + 1, channel);
                sum = sum - leaving + entering;
            }
        }
        image.pixels[row..row + width * 3].copy_from_slice(&row_out);
    }
}

/// Swaps rows and columns so the horizontal pass can do the vertical one.
fn transpose(image: &mut Image) {
    let mut pixels = vec![0u8; image.pixels.len()];
    for y in 0..image.height {
        for x in 0..image.width {
            let from = (y * image.width + x) * 3;
            let to = (x * image.height + y) * 3;
            pixels[to..to + 3].copy_from_slice(&image.pixels[from..from + 3]);
        }
    }
    image.pixels = pixels;
    std::mem::swap(&mut image.width, &mut image.height);
}

/// Scales every channel toward black so the card's text stays legible over
/// a bright wallpaper.
fn dim(image: &mut Image, factor: f64) {
    let factor = factor.clamp(0.0, 1.0);
    if (factor - 1.0).abs() < f64::EPSILON {
        return;
    }
    // Fixed point: one multiply and shift per channel instead of a float
    // round trip over several million samples.
    let scale = (factor * 256.0).round() as u32;
    for byte in &mut image.pixels {
        *byte = ((u32::from(*byte) * scale) >> 8) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, color: [u8; 3]) -> Image {
        Image {
            width,
            height,
            pixels: color
                .iter()
                .copied()
                .cycle()
                .take(width * height * 3)
                .collect(),
        }
    }

    #[test]
    fn parses_a_binary_ppm() {
        let mut data = b"P6\n2 1\n255\n".to_vec();
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let image = parse_ppm(&data).unwrap();
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn parses_headers_with_comments() {
        let mut data = b"P6\n# made by grim\n2 1\n255\n".to_vec();
        data.extend_from_slice(&[9; 6]);
        assert_eq!(parse_ppm(&data).unwrap().width, 2);
    }

    #[test]
    fn rejects_truncated_rasters() {
        let data = b"P6\n4 4\n255\n\x00\x01".to_vec();
        assert!(parse_ppm(&data).is_err());
    }

    #[test]
    fn rejects_non_p6_images() {
        assert!(parse_ppm(b"P3\n1 1\n255\n0 0 0").is_err());
    }

    #[test]
    fn downscaling_averages_blocks() {
        let mut image = solid(2, 2, [0, 0, 0]);
        image.pixels[0..3].copy_from_slice(&[255, 255, 255]);
        let small = downscale(&image, 2);
        assert_eq!((small.width, small.height), (1, 1));
        // One white pixel out of four averages to 255/4.
        assert_eq!(small.pixels, [63, 63, 63]);
    }

    #[test]
    fn downscaling_handles_non_multiple_dimensions() {
        let small = downscale(&solid(5, 3, [10, 20, 30]), 2);
        assert_eq!((small.width, small.height), (3, 2));
        assert_eq!(small.pixels, [10, 20, 30].repeat(6));
    }

    #[test]
    fn blurring_a_flat_image_is_a_no_op() {
        // Clamp-to-edge sampling means uniform input must survive exactly,
        // with no darkened border.
        let mut image = solid(9, 7, [120, 60, 200]);
        box_blur(&mut image, 3);
        assert_eq!(image.pixels, [120, 60, 200].repeat(63));
    }

    #[test]
    fn blurring_preserves_dimensions() {
        let mut image = solid(11, 5, [1, 2, 3]);
        box_blur(&mut image, 4);
        assert_eq!((image.width, image.height), (11, 5));
        assert_eq!(image.pixels.len(), 11 * 5 * 3);
    }

    #[test]
    fn blurring_spreads_a_single_bright_pixel() {
        let mut image = solid(9, 9, [0, 0, 0]);
        let center = (4 * 9 + 4) * 3;
        image.pixels[center..center + 3].copy_from_slice(&[255, 255, 255]);
        box_blur(&mut image, 2);
        let neighbour = (4 * 9 + 5) * 3;
        assert!(
            image.pixels[neighbour] > 0,
            "energy should reach neighbours"
        );
        assert!(image.pixels[center] < 255, "the peak should be flattened");
    }

    #[test]
    fn transposing_twice_is_the_identity() {
        let original = {
            let mut image = solid(4, 3, [0, 0, 0]);
            for (index, byte) in image.pixels.iter_mut().enumerate() {
                *byte = index as u8;
            }
            image
        };
        let mut image = original.clone();
        transpose(&mut image);
        assert_eq!((image.width, image.height), (3, 4));
        transpose(&mut image);
        assert_eq!(image.pixels, original.pixels);
        assert_eq!((image.width, image.height), (4, 3));
    }

    #[test]
    fn dimming_scales_toward_black() {
        let mut image = solid(1, 1, [200, 100, 50]);
        dim(&mut image, 0.5);
        assert_eq!(image.pixels, [100, 50, 25]);
    }

    #[test]
    fn dimming_by_one_changes_nothing() {
        let mut image = solid(2, 2, [200, 100, 50]);
        dim(&mut image, 1.0);
        assert_eq!(image.pixels, [200, 100, 50].repeat(4));
    }

    #[test]
    fn processing_shrinks_and_darkens() {
        let processed = process(
            &solid(64, 32, [200, 200, 200]),
            BlurSettings {
                radius: 4,
                downscale: 4,
                dim: 0.5,
            },
        );
        assert_eq!((processed.width, processed.height), (16, 8));
        assert!(processed.pixels.iter().all(|byte| *byte < 200));
    }
}
