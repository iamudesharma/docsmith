//! Second-pass OCR refinement for hard scans.
//!
//! The first Tesseract pass is tuned for pages of text. It can miss isolated labels inside
//! graphics (seals, chart labels) and it cannot resolve printed math glyphs. This module is the
//! raster half of a bounded second pass: it finds unclaimed ink regions, segments glyph columns,
//! checks superscript and pi shapes, and renders upscaled crops. The OCR calls themselves stay in
//! the provider so this module has no process dependencies.
//!
//! Everything produced here is a proposal. Callers mark refined text as `ocr` with explicit
//! evidence, never as native content.

use crate::layout::Word;
use anyhow::{Context, Result};
use image::{GrayImage, ImageReader};
use std::path::Path;

const INK_THRESHOLD: u8 = 190;
const MAX_COMPONENT_PIXELS: usize = 300_000;

/// A pixel rectangle in the OCR image's coordinate space.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// Pixel rect for an OCR word, clipped to the image.
    pub fn from_word(word: &Word, width: u32, height: u32) -> Option<Self> {
        let x0 = word.x.floor().max(0.0);
        let y0 = word.y.floor().max(0.0);
        let x1 = (word.x + word.width).ceil().min(width as f64);
        let y1 = (word.y + word.height).ceil().min(height as f64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Self {
            x: x0 as u32,
            y: y0 as u32,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        })
    }

    pub fn right(self) -> u32 {
        self.x + self.width
    }

    pub fn bottom(self) -> u32 {
        self.y + self.height
    }

    pub fn area(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// Grow the rect by `pad` pixels, clipped to the image.
    pub fn expand(self, pad: u32, width: u32, height: u32) -> Self {
        let x0 = self.x.saturating_sub(pad);
        let y0 = self.y.saturating_sub(pad);
        let x1 = self.right().saturating_add(pad).min(width);
        let y1 = self.bottom().saturating_add(pad).min(height);
        Self {
            x: x0,
            y: y0,
            width: x1.saturating_sub(x0),
            height: y1.saturating_sub(y0),
        }
    }

    pub fn union(self, other: Self) -> Self {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = self.right().max(other.right());
        let y1 = self.bottom().max(other.bottom());
        Self {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }

    fn vertical_overlap(self, other: Self) -> f64 {
        let top = self.y.max(other.y) as f64;
        let bottom = self.bottom().min(other.bottom()) as f64;
        (bottom - top).max(0.0) / self.height.max(1) as f64
    }
}

/// A loaded OCR raster used for refinement.
pub struct Page {
    image: GrayImage,
}

/// A rendered crop and the mapping back to page pixels.
pub struct CropImage {
    pub png: Vec<u8>,
    pub x: u32,
    pub y: u32,
    pub scale_x: f64,
    pub scale_y: f64,
}

impl Page {
    pub fn load(path: &Path) -> Result<Self> {
        let image = ImageReader::open(path)?
            .with_guessed_format()?
            .decode()
            .context("corrupt or unsupported image")?
            .to_luma8();
        Ok(Self { image })
    }

    pub fn width(&self) -> u32 {
        self.image.width()
    }

    pub fn height(&self) -> u32 {
        self.image.height()
    }

    fn ink(&self, x: u32, y: u32) -> bool {
        self.image.get_pixel(x, y).0[0] < INK_THRESHOLD
    }

    /// Column runs of ink inside `rect`, in absolute image x coordinates.
    pub fn column_runs(&self, rect: Rect) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        let left = rect.x;
        let right = rect.right().min(self.width());
        let top = rect.y;
        let bottom = rect.bottom().min(self.height());
        if right <= left || bottom <= top {
            return runs;
        }
        let mut x = left;
        while x < right {
            let ink = (top..bottom).any(|y| self.ink(x, y));
            if ink {
                let start = x;
                while x + 1 < right && (top..bottom).any(|y| self.ink(x + 1, y)) {
                    x += 1;
                }
                runs.push((start, x));
            }
            x += 1;
        }
        runs
    }

    /// Render an upscaled PNG of `rect` for a crop OCR pass.
    ///
    /// The crop is padded by 3 px and may be capped to keep the render bounded, so the returned
    /// origin and scale factors are needed to map crop coordinates back to the page.
    pub fn crop(&self, rect: Rect, scale: f64) -> Result<CropImage> {
        self.crop_pad(rect, scale, 3)
    }

    /// Binarized crop: thin symbols (`1..*`, arrows) survive thresholding when the grayscale
    /// crop is too noisy for the model.
    pub fn crop_binarized(&self, rect: Rect, scale: f64, pad: u32) -> Result<CropImage> {
        let rect = rect.expand(pad, self.width(), self.height());
        if rect.width == 0 || rect.height == 0 {
            anyhow::bail!("empty crop");
        }
        let crop = image::imageops::crop_imm(&self.image, rect.x, rect.y, rect.width, rect.height)
            .to_image();
        let binary = GrayImage::from_fn(crop.width(), crop.height(), |x, y| {
            if crop.get_pixel(x, y).0[0] < 170 {
                image::Luma([0u8])
            } else {
                image::Luma([255u8])
            }
        });
        let scale = scale.clamp(1.0, 6.0);
        let width = ((rect.width as f64 * scale).round() as u32).clamp(1, 1800);
        let height = ((rect.height as f64 * scale).round() as u32).clamp(1, 1200);
        let scale_x = width as f64 / rect.width as f64;
        let scale_y = height as f64 / rect.height as f64;
        let scaled = image::imageops::resize(
            &binary,
            width,
            height,
            image::imageops::FilterType::Lanczos3,
        );
        let mut bytes = Vec::new();
        image::DynamicImage::ImageLuma8(scaled)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .context("encode crop")?;
        Ok(CropImage {
            png: bytes,
            x: rect.x,
            y: rect.y,
            scale_x,
            scale_y,
        })
    }

    /// Render a crop with an explicit padding (glyph crops need less context than lines).
    pub fn crop_pad(&self, rect: Rect, scale: f64, pad: u32) -> Result<CropImage> {
        let rect = rect.expand(pad, self.width(), self.height());
        if rect.width == 0 || rect.height == 0 {
            anyhow::bail!("empty crop");
        }
        let crop = image::imageops::crop_imm(&self.image, rect.x, rect.y, rect.width, rect.height)
            .to_image();
        let scale = scale.clamp(1.0, 6.0);
        let width = ((rect.width as f64 * scale).round() as u32).clamp(1, 1800);
        let height = ((rect.height as f64 * scale).round() as u32).clamp(1, 1200);
        let scale_x = width as f64 / rect.width as f64;
        let scale_y = height as f64 / rect.height as f64;
        let scaled =
            image::imageops::resize(&crop, width, height, image::imageops::FilterType::Lanczos3);
        let mut bytes = Vec::new();
        image::DynamicImage::ImageLuma8(scaled)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .context("encode crop")?;
        Ok(CropImage {
            png: bytes,
            x: rect.x,
            y: rect.y,
            scale_x,
            scale_y,
        })
    }

    /// Tight ink bounds inside `rect`: `(left, top, right, bottom)`.
    pub fn ink_bounds(&self, rect: Rect) -> Option<(u32, u32, u32, u32)> {
        let mut left = self.width();
        let mut top = self.height();
        let mut right = 0;
        let mut bottom = 0;
        let mut found = false;
        for y in rect.y..rect.bottom().min(self.height()) {
            for x in rect.x..rect.right().min(self.width()) {
                if self.ink(x, y) {
                    found = true;
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x);
                    bottom = bottom.max(y);
                }
            }
        }
        found.then_some((left, top, right, bottom))
    }

    /// Large hollow, near-square components: seals and stamps.
    ///
    /// Stamp text often touches the ring, so it is not a separate component; the ring box is the
    /// handle used to crop the text bands inside it.
    pub fn rings(&self, limit: usize) -> Vec<Rect> {
        let width = self.width() as usize;
        let height = self.height() as usize;
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut ink = vec![false; width * height];
        for y in 0..height {
            for x in 0..width {
                ink[y * width + x] = self.ink(x as u32, y as u32);
            }
        }
        let mut rings: Vec<Rect> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for start in 0..width * height {
            if !ink[start] {
                continue;
            }
            stack.push(start);
            ink[start] = false;
            let mut count = 0usize;
            let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0usize, 0usize);
            let mut too_big = false;
            while let Some(index) = stack.pop() {
                count += 1;
                if count > MAX_COMPONENT_PIXELS {
                    too_big = true;
                    break;
                }
                let x = index % width;
                let y = index / width;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as i64 + dx;
                        let ny = y as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                            continue;
                        }
                        let next = ny as usize * width + nx as usize;
                        if ink[next] {
                            ink[next] = false;
                            stack.push(next);
                        }
                    }
                }
            }
            if too_big {
                continue;
            }
            let rect = Rect {
                x: x0 as u32,
                y: y0 as u32,
                width: (x1 - x0 + 1) as u32,
                height: (y1 - y0 + 1) as u32,
            };
            if !(60..=420).contains(&rect.width) || !(60..=420).contains(&rect.height) {
                continue;
            }
            let aspect = rect.width as f64 / rect.height as f64;
            if !(0.7..=1.45).contains(&aspect) {
                continue;
            }
            let fill = count as f64 / rect.area().max(1) as f64;
            if !(0.02..=0.6).contains(&fill) {
                continue;
            }
            if self.center_is_light(rect) {
                rings.push(rect);
            }
        }
        rings.sort_by_key(|rect| std::cmp::Reverse(rect.area()));
        rings.truncate(limit);
        rings
    }

    fn center_is_light(&self, rect: Rect) -> bool {
        let cx = rect.x + rect.width / 2;
        let cy = rect.y + rect.height / 2;
        let mut dark = 0;
        let mut samples = 0;
        for y in cy.saturating_sub(7)..(cy + 7).min(self.height()) {
            for x in cx.saturating_sub(7)..(cx + 7).min(self.width()) {
                samples += 1;
                if self.ink(x, y) {
                    dark += 1;
                }
            }
        }
        samples > 0 && (dark as f64) / (samples as f64) < 0.25
    }

    /// Text bands inside a seal: rows whose ink density stands well above the ring's sides.
    pub fn text_bands(&self, rect: Rect) -> Vec<Rect> {
        // The ring stroke is a few pixels wide; inset far enough to exclude it but keep the
        // stamp text (which starts just inside the ring).
        let left = rect.x + 8;
        let right = rect.right().saturating_sub(8).min(self.width());
        if right <= left {
            return Vec::new();
        }
        let mut density = Vec::new();
        for y in rect.y..rect.bottom().min(self.height()) {
            let mut count = 0usize;
            for x in left..right {
                if self.ink(x, y) {
                    count += 1;
                }
            }
            density.push(count);
        }
        if density.is_empty() {
            return Vec::new();
        }
        let mut sorted = density.clone();
        sorted.sort_unstable();
        let baseline = sorted[sorted.len() / 2].max(1);
        let threshold = ((baseline as f64 * 2.2).max((baseline + 8) as f64)) as usize;
        let mut bands = Vec::new();
        let mut start = None;
        for (index, value) in density.iter().enumerate() {
            if *value >= threshold {
                start.get_or_insert(index);
            } else if let Some(begin) = start.take() {
                if index - begin >= 4 {
                    bands.push(Rect {
                        x: left,
                        y: rect.y + begin as u32,
                        width: right - left,
                        height: (index - begin) as u32,
                    });
                }
            }
        }
        if let Some(begin) = start {
            let index = density.len();
            if index - begin >= 4 {
                bands.push(Rect {
                    x: left,
                    y: rect.y + begin as u32,
                    width: right - left,
                    height: (index - begin) as u32,
                });
            }
        }
        bands
    }

    /// A pi glyph has a full-width top bar and two stems below it, with no bottom bar.
    pub fn looks_like_pi(&self, rect: Rect) -> bool {
        if rect.width < 8 || rect.height < 10 {
            return false;
        }
        let runs = self.column_runs(rect);
        if runs.len() != 1 {
            return false;
        }
        // The bar is one or two scanlines at low resolution. It must be one solid run: an `m`
        // has a full-width ink span too, but its arches leave gaps in the top scanline.
        let top_rows = (rect.height * 2 / 5).max(3);
        let mut solid_bar = false;
        for y in rect.y..(rect.y + top_rows).min(self.height()) {
            if self
                .row_runs(y, rect)
                .iter()
                .any(|(left, right)| (right - left + 1) as f64 >= 0.8 * rect.width as f64)
            {
                solid_bar = true;
                break;
            }
        }
        if !solid_bar {
            return false;
        }
        // Below the bar there must be two stems separated by a gap.
        let sample = (rect.y + rect.height / 2).min(self.height().saturating_sub(1));
        let mut columns = Vec::new();
        for x in rect.x..rect.right().min(self.width()) {
            if self.ink(x, sample) {
                columns.push(x);
            }
        }
        if columns.is_empty() {
            return false;
        }
        let mut groups = 0;
        let mut previous = columns[0];
        groups += 1;
        for x in &columns[1..] {
            if *x > previous + 1 {
                groups += 1;
            }
            previous = *x;
        }
        let covered = columns.len() as f64;
        groups == 2 && covered <= 0.8 * rect.width as f64
    }

    /// Split a raised superscript glyph off the right side of a text run.
    ///
    /// `mc²` is read as one word `mc?`; the superscript touches the preceding letter at low
    /// resolution, so column gaps cannot separate it. Its ink stops above the baseline, which
    /// gives the split point.
    pub fn superscript_rect(&self, rect: Rect) -> Option<Rect> {
        let word_bottom = rect.bottom();
        let mut columns = Vec::new();
        for x in rect.x..rect.right().min(self.width()) {
            let mut bottom = None;
            for y in rect.y..rect.bottom().min(self.height()) {
                if self.ink(x, y) {
                    bottom = Some(y);
                }
            }
            if let Some(bottom) = bottom {
                columns.push((x, bottom));
            }
        }
        if columns.len() < 6 {
            return None;
        }
        let mut start = None;
        for (x, bottom) in columns.iter().rev() {
            if *bottom + 3 <= word_bottom {
                start = Some(*x);
            } else {
                break;
            }
        }
        let start = start?;
        let end = columns.last()?.0;
        if end <= start + 2 {
            return None;
        }
        let width = end - start + 1;
        // The raised part must be narrower than the baseline glyph next to it, but a two-glyph
        // run (`r²` read as one word) still leaves a valid split.
        if width * 4 > rect.width * 3 {
            return None;
        }
        Some(Rect {
            x: start,
            y: rect.y,
            width,
            height: rect.height,
        })
    }

    /// Contiguous ink runs on one scanline inside `rect`.
    fn row_runs(&self, y: u32, rect: Rect) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        let mut x = rect.x;
        let right = rect.right().min(self.width());
        while x < right {
            if self.ink(x, y) {
                let start = x;
                while x + 1 < right && self.ink(x + 1, y) {
                    x += 1;
                }
                runs.push((start, x));
            }
            x += 1;
        }
        runs
    }

    /// Ink components not covered by an OCR word box, grouped into line-like regions.
    ///
    /// The primary OCR claims most of a text page, so this is cheap there. On graphic-heavy
    /// pages it recovers isolated labels (seals, chart annotations) that full-page segmentation
    /// skipped. Components that look like graphics (rings, QR blocks, solid bars) are rejected
    /// before any OCR call happens.
    pub fn unclaimed(&self, words: &[Word], limit: usize) -> Vec<Rect> {
        let width = self.width() as usize;
        let height = self.height() as usize;
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut ink = vec![false; width * height];
        for y in 0..height {
            for x in 0..width {
                ink[y * width + x] = self.ink(x as u32, y as u32);
            }
        }
        let mut claimed = vec![false; width * height];
        for word in words {
            let Some(rect) = Rect::from_word(word, width as u32, height as u32) else {
                continue;
            };
            let rect = rect.expand(2, width as u32, height as u32);
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    claimed[y as usize * width + x as usize] = true;
                }
            }
        }

        let mut components: Vec<Rect> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        for start in 0..width * height {
            if !ink[start] || claimed[start] {
                continue;
            }
            stack.push(start);
            ink[start] = false;
            let mut count = 0usize;
            let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0usize, 0usize);
            let mut too_big = false;
            while let Some(index) = stack.pop() {
                count += 1;
                if count > MAX_COMPONENT_PIXELS {
                    too_big = true;
                    break;
                }
                let x = index % width;
                let y = index / width;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as i64 + dx;
                        let ny = y as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= width as i64 || ny >= height as i64 {
                            continue;
                        }
                        let next = ny as usize * width + nx as usize;
                        if ink[next] && !claimed[next] {
                            ink[next] = false;
                            stack.push(next);
                        }
                    }
                }
            }
            if too_big {
                continue;
            }
            let rect = Rect {
                x: x0 as u32,
                y: y0 as u32,
                width: (x1 - x0 + 1) as u32,
                height: (y1 - y0 + 1) as u32,
            };
            if keep_component(rect, count) {
                components.push(rect);
            }
        }
        if components.is_empty() {
            return Vec::new();
        }

        // Group nearby components into one line-like region.
        components.sort_by_key(|rect| (rect.y, rect.x));
        let mut groups: Vec<Rect> = Vec::new();
        for rect in components {
            let mut merged = false;
            for group in &mut groups {
                let overlap = rect.vertical_overlap(*group);
                let gap = if rect.x > group.right() {
                    rect.x - group.right()
                } else if group.x > rect.right() {
                    group.x - rect.right()
                } else {
                    0
                };
                let tolerance = (2.5 * rect.height.max(group.height) as f64) as u32;
                if overlap >= 0.35 && gap <= tolerance {
                    let candidate = group.union(rect);
                    if candidate.width as f64 <= 0.9 * width as f64 {
                        *group = candidate;
                        merged = true;
                        break;
                    }
                }
            }
            if !merged {
                groups.push(rect);
            }
        }
        groups.retain(|rect| {
            rect.width >= 8
                && (6..=220).contains(&rect.height)
                && rect.width <= 1000
                && (rect.area() as f64) < 0.4 * (width * height) as f64
        });
        groups.sort_by_key(|rect| std::cmp::Reverse(rect.area()));
        groups.truncate(limit);
        groups
    }
}

fn keep_component(rect: Rect, pixels: usize) -> bool {
    if pixels < 12 || rect.width < 3 || rect.height < 6 || rect.width > 420 || rect.height > 220 {
        return false;
    }
    let fill = pixels as f64 / rect.area().max(1) as f64;
    if !(0.04..=0.86).contains(&fill) {
        return false;
    }
    // Rings, logos and QR finder patterns are nearly square and large; without this they cost a
    // crop OCR call on every page with a seal or a code.
    let aspect = rect.width as f64 / rect.height as f64;
    if rect.width > 80 && rect.height > 80 && (0.6..=1.6).contains(&aspect) {
        return false;
    }
    true
}

/// Unicode superscript for an ASCII digit.
pub fn superscript(digit: char) -> Option<char> {
    Some(match digit {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        _ => return None,
    })
}

/// Lines worth a second, line-level OCR pass: equations, fractions and formulas.
pub fn is_formula_line(text: &str) -> bool {
    let text = text.trim();
    if text.len() < 3 || text.len() > 90 {
        return false;
    }
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let digits = text.chars().filter(|c| c.is_ascii_digit()).count();
    let equals = text.contains('=') || text.contains('≈') || text.contains('≠');
    let fraction = text.contains('/') && digits >= 4;
    let operator = text.contains('+') || text.contains('−') || text.contains('-');
    (equals && letters + digits >= 3 && text.chars().any(|c| c.is_ascii_alphanumeric()))
        || (fraction && (equals || operator))
}

/// Slightly wider net for the line OCR pass: short lines that carry a formula signal even when
/// the first pass merged the equals sign into a word (`E वाल` for `E = mc²`).
pub fn is_formula_candidate(text: &str) -> bool {
    let text = text.trim();
    if text.chars().count() < 2 || text.chars().count() > 60 {
        return false;
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 8 {
        return false;
    }
    let has_symbol = "=+−?^".chars().any(|c| text.contains(c));
    let has_digit = text.chars().any(|c| c.is_ascii_digit());
    let short_latin = words
        .iter()
        .any(|word| word.len() == 1 && word.chars().all(|c| c.is_ascii_alphabetic()));
    let fraction = digit_slash_digit(text);
    is_formula_line(text)
        || (has_symbol && (has_digit || short_latin))
        || (fraction && has_digit)
        || (short_latin && words.len() <= 2)
}

/// `1/2` and `8 / 10` are fractions; a slash used as a separator (`Hindi / English`) is not.
fn digit_slash_digit(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.iter().enumerate().any(|(index, ch)| {
        if *ch != '/' {
            return false;
        }
        let before = chars[..index].iter().rev().find(|c| !c.is_whitespace());
        let after = chars[index + 1..].iter().find(|c| !c.is_whitespace());
        before.is_some_and(|c| c.is_ascii_digit()) && after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// Relationship cardinalities such as `1..*` and `0..1` are mostly punctuation; a normal
/// alphanumeric density check would reject them.
pub fn is_cardinality(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && text.len() <= 8
        && text.contains("..")
        && text.chars().any(|c| c == '*')
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '*')
}

/// Tokens that look like a chemical formula (letters and digits only).
pub fn is_chemical_token(text: &str) -> bool {
    if text.len() < 3 || text.len() > 12 {
        return false;
    }
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    let digits = text.chars().filter(|c| c.is_ascii_digit()).count();
    let letters = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
    digits >= 1 && letters >= 1 && text.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superscripts_cover_digits() {
        assert_eq!(superscript('2'), Some('²'));
        assert_eq!(superscript('0'), Some('⁰'));
        assert_eq!(superscript('x'), None);
    }

    #[test]
    fn formula_detection_is_narrow() {
        assert!(is_formula_line("E = mc?"));
        assert!(is_formula_line("1/2 + 1/3 = 5/6"));
        assert!(is_formula_line("क्षेत्रफल = rr?"));
        assert!(!is_formula_line("The total is due on Friday."));
        assert!(!is_formula_line("8 / 10"));
        assert!(!is_formula_candidate("8. Language mix / भाषा मिश्रण"));
        assert!(!is_formula_candidate("50"));
        assert!(is_formula_candidate("E वाल"));
        assert!(is_formula_candidate("H20 +CO2"));
    }

    #[test]
    fn chemical_detection_accepts_common_formulas() {
        assert!(is_chemical_token("H20"));
        assert!(is_chemical_token("CO2"));
        assert!(!is_chemical_token("Q1"));
        assert!(!is_chemical_token("Total"));
    }
}
