// SPDX-License-Identifier: CC0-1.0

//! A small RGBA raster canvas, and its PNG encoder.
//!
//! Every raster asset this service serves — raster tiles, sprite sheets and
//! static maps — is drawn here and encoded with the `png` crate, so the bytes
//! on the wire really are a PNG any client can decode: the `Content-Type` this
//! service reports is never a claim about bytes it did not produce.

use super::Rgba;

/// A row-major RGBA8 image.
#[derive(Debug, Clone)]
pub struct Canvas {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Canvas {
    /// A canvas of `width` x `height`, filled with `fill`.
    ///
    /// Both dimensions are clamped into `1..=MAX_CANVAS_SIDE`: a zero-sized PNG
    /// cannot be encoded, and the upper clamp means no caller can size an
    /// allocation here — the allocation is at most
    /// `MAX_CANVAS_SIDE^2 * 4` bytes whatever it asks for. Every operation
    /// refuses an out-of-range size before it gets here (see the modeled
    /// `Width`/`Height` range on `GetStaticMap`); the clamp is the backstop that
    /// makes that a property of this type rather than of its callers.
    pub fn new(width: u32, height: u32, fill: Rgba) -> Self {
        let width = width.clamp(1, MAX_CANVAS_SIDE);
        let height = height.clamp(1, MAX_CANVAS_SIDE);
        let pixels = width as usize * height as usize;
        // One row built once and repeated: the largest asset this service
        // renders is 2800x2800, so filling pixel by pixel would be ~7.8M
        // appends for the same bytes.
        let row: Vec<u8> = fill.repeat(width as usize);
        let mut data = Vec::with_capacity(pixels * 4);
        for _ in 0..height {
            data.extend_from_slice(&row);
        }
        Self {
            width,
            height,
            data,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Paint one pixel. Coordinates outside the canvas are dropped, so a
    /// drawing routine never has to clip.
    pub fn put(&mut self, x: i64, y: i64, color: Rgba) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        self.data[offset..offset + 4].copy_from_slice(&color);
    }

    /// Read one pixel, or `None` outside the canvas.
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        Some([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    /// Fill an axis-aligned rectangle.
    ///
    /// The loop bounds are the rectangle INTERSECTED with the canvas, so the
    /// work is bounded by the canvas even when the caller asks for a rectangle
    /// far larger than it, or entirely outside it.
    pub fn fill_rect(&mut self, x: i64, y: i64, width: u32, height: u32, color: Rgba) {
        let Some((x0, x1)) = self.clip_x(x, x.saturating_add(i64::from(width)) - 1) else {
            return;
        };
        let Some((y0, y1)) = self.clip_y(y, y.saturating_add(i64::from(height)) - 1) else {
            return;
        };
        for row in y0..=y1 {
            for column in x0..=x1 {
                self.put(column, row, color);
            }
        }
    }

    /// Draw a one-pixel horizontal run, inclusive of both ends, clipped to the
    /// canvas.
    pub fn hline(&mut self, y: i64, x0: i64, x1: i64, color: Rgba) {
        if self.clip_y(y, y).is_none() {
            return;
        }
        let Some((lo, hi)) = self.clip_x(x0.min(x1), x0.max(x1)) else {
            return;
        };
        for x in lo..=hi {
            self.put(x, y, color);
        }
    }

    /// Draw a one-pixel vertical run, inclusive of both ends, clipped to the
    /// canvas.
    pub fn vline(&mut self, x: i64, y0: i64, y1: i64, color: Rgba) {
        if self.clip_x(x, x).is_none() {
            return;
        }
        let Some((lo, hi)) = self.clip_y(y0.min(y1), y0.max(y1)) else {
            return;
        };
        for y in lo..=hi {
            self.put(x, y, color);
        }
    }

    /// Draw a one-pixel line between two points, clipped to the canvas before
    /// it is walked, so the step count is bounded by the canvas rather than by
    /// how far apart the endpoints are.
    pub fn line(&mut self, from: (i64, i64), to: (i64, i64), color: Rgba) {
        let Some(((mut x0, mut y0), (x1, y1))) = self.clip_segment(from, to) else {
            return;
        };
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.put(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let err2 = err * 2;
            if err2 >= dy {
                err += dy;
                x0 += sx;
            }
            if err2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    /// Fill a disc of radius `radius` centred on `(cx, cy)`, iterating only the
    /// part of its bounding box that is on the canvas.
    pub fn disc(&mut self, cx: i64, cy: i64, radius: u32, color: Rgba) {
        // A radius beyond the canvas diagonal covers it entirely; clamping it
        // keeps the squared-distance test inside i64 and the loop inside the
        // canvas.
        let r = i64::from(radius.min(self.width.saturating_add(self.height)));
        let Some((x0, x1)) = self.clip_x(cx.saturating_sub(r), cx.saturating_add(r)) else {
            return;
        };
        let Some((y0, y1)) = self.clip_y(cy.saturating_sub(r), cy.saturating_add(r)) else {
            return;
        };
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x - cx, y - cy);
                if dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)) <= r * r {
                    self.put(x, y, color);
                }
            }
        }
    }

    /// `[lo, hi]` intersected with the canvas's x range, or `None` when the
    /// span misses the canvas entirely.
    fn clip_x(&self, lo: i64, hi: i64) -> Option<(i64, i64)> {
        clip_span(lo, hi, i64::from(self.width) - 1)
    }

    /// `[lo, hi]` intersected with the canvas's y range.
    fn clip_y(&self, lo: i64, hi: i64) -> Option<(i64, i64)> {
        clip_span(lo, hi, i64::from(self.height) - 1)
    }

    /// Clip a segment to the canvas rectangle (Liang-Barsky), so a line whose
    /// endpoints are astronomically far apart is walked only across the pixels
    /// it actually covers.
    fn clip_segment(
        &self,
        from: (i64, i64),
        to: (i64, i64),
    ) -> Option<((i64, i64), (i64, i64))> {
        let (x0, y0) = (from.0 as f64, from.1 as f64);
        let (dx, dy) = (to.0 as f64 - x0, to.1 as f64 - y0);
        let x_max = f64::from(self.width - 1);
        let y_max = f64::from(self.height - 1);
        let mut enter = 0.0_f64;
        let mut leave = 1.0_f64;
        for (edge, distance) in [
            (-dx, x0),
            (dx, x_max - x0),
            (-dy, y0),
            (dy, y_max - y0),
        ] {
            if edge == 0.0 {
                // Parallel to this edge: either wholly inside it, or wholly out.
                if distance < 0.0 {
                    return None;
                }
                continue;
            }
            let crossing = distance / edge;
            if edge < 0.0 {
                if crossing > leave {
                    return None;
                }
                enter = enter.max(crossing);
            } else {
                if crossing < enter {
                    return None;
                }
                leave = leave.min(crossing);
            }
        }
        let point = |t: f64| {
            (
                (x0 + t * dx).round().clamp(0.0, x_max) as i64,
                (y0 + t * dy).round().clamp(0.0, y_max) as i64,
            )
        };
        Some((point(enter), point(leave)))
    }

    /// Encode the canvas as an 8-bit RGBA PNG.
    ///
    /// Infallible by construction: the sink is a `Vec` (which never fails to
    /// write), [`Canvas::new`] clamps both dimensions to at least one pixel, and
    /// `data` is kept at exactly `width * height * 4` bytes — the only
    /// conditions under which the encoder reports an error.
    pub fn to_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder
                .write_header()
                .expect("PNG header for a non-empty RGBA8 canvas");
            writer
                .write_image_data(&self.data)
                .expect("PNG image data for a non-empty RGBA8 canvas");
            writer
                .finish()
                .expect("PNG finalization for a non-empty RGBA8 canvas");
        }
        out
    }

    /// Encode the canvas as a baseline JPEG at [`JPEG_QUALITY`], flattening the
    /// alpha channel away (JPEG has none).
    ///
    /// Infallible for the same reasons as [`Canvas::to_png`]: the dimensions
    /// are non-zero and, because [`Canvas::new`] clamps them to
    /// [`MAX_CANVAS_SIDE`], they always fit the `u16` a JPEG header carries.
    pub fn to_jpeg(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.data.len() / 4 * 3);
        for pixel in self.data.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
        }
        let mut out = Vec::new();
        let encoder = jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY);
        let (Ok(width), Ok(height)) = (u16::try_from(self.width), u16::try_from(self.height))
        else {
            // Unreachable: `MAX_CANVAS_SIDE` is far below `u16::MAX`.
            return out;
        };
        encoder
            .encode(&rgb, width, height, jpeg_encoder::ColorType::Rgb)
            .expect("JPEG encoding of a non-empty RGB canvas");
        out
    }
}

/// The JPEG quality every photographic asset is encoded at. A fixed value, so
/// the same asset always encodes to the same bytes; 85 is the usual quality for
/// map imagery, high enough that the terrain shading survives it.
pub const JPEG_QUALITY: u8 = 85;

/// The widest or tallest canvas this module will build.
///
/// The largest asset the service renders is a `map@2x` static map at the
/// modeled `Width`/`Height` maximum of 1400, i.e. 2800 device pixels, so this
/// is an order of magnitude of headroom over any legitimate render and well
/// inside the `u16` dimension limit a JPEG header can carry.
pub const MAX_CANVAS_SIDE: u32 = 8192;

/// `[lo, hi]` intersected with `[0, max]`, or `None` when they do not overlap.
fn clip_span(lo: i64, hi: i64, max: i64) -> Option<(i64, i64)> {
    let lo = lo.max(0);
    let hi = hi.min(max);
    (lo <= hi).then_some((lo, hi))
}
