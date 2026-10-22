//! Rule-based table detection for scanned pages.
//!
//! Tesseract returns words, not tables. When a scanned page contains printed grid rules, the
//! rules themselves define the table: horizontal rules give the row bands, vertical rules give
//! the column boundaries, and OCR words are assigned to the resulting cells. Cells are proposed
//! geometry, so the resulting table is always marked `inferred`.
//!
//! Native-only: it decodes the page raster with the `image` crate.

use crate::layout::Word;
use crate::model::{Bounds, Cell, Table};
use serde_json::json;

/// One detected table region and its structured cells.
pub struct DetectedTable {
    pub table: Table,
    pub bounds: Bounds,
    /// Number of OCR words placed into cells.
    pub words_placed: usize,
}

#[derive(Debug)]
struct HLine {
    y0: f64,
    y1: f64,
    x0: f64,
    x1: f64,
}
const INK_THRESHOLD: u8 = 180;
const MIN_BAND: f64 = 12.0;
const MAX_BAND: f64 = 1400.0;
const BORDER_COVERAGE: f64 = 0.72;
const MERGED_COVERAGE: f64 = 0.45;

/// Detect printed tables on a scanned page and assign OCR words to cells.
pub fn detect(path: &std::path::Path, words: &[Word]) -> Vec<DetectedTable> {
    let Some(image) = image::ImageReader::open(path)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.decode().ok())
    else {
        return Vec::new();
    };
    let gray = image.to_luma8();
    let (width, height) = gray.dimensions();
    if width < 200 || height < 200 {
        return Vec::new();
    }
    let ink = |x: u32, y: u32| -> bool { gray.get_pixel(x, y).0[0] < INK_THRESHOLD };

    let horizontals = horizontal_lines(width, height, &ink);
    if std::env::var("DOC_TABLE_DEBUG").is_ok() {
        eprintln!("rules: {horizontals:?}");
    }
    if horizontals.len() < 3 {
        return Vec::new();
    }
    let mut tables = Vec::new();
    for group in group_rules(horizontals) {
        if group.len() < 3 {
            continue;
        }
        if let Some(table) = build_table(&group, words, &ink, width, height) {
            tables.push(table);
        }
    }
    tables
}

/// Group horizontal rules that share an extent and are close together: one table's row rules.
fn group_rules(lines: Vec<HLine>) -> Vec<Vec<HLine>> {
    let mut groups: Vec<Vec<HLine>> = Vec::new();
    for line in lines {
        // Rules of one table share the same extent; a page divider or a boxed total has a
        // different extent and must not join the table's row rules.
        let existing = groups.iter_mut().find(|group| {
            let reference = &group[0];
            (line.x0 - reference.x0).abs() <= 8.0 && (line.x1 - reference.x1).abs() <= 8.0
        });
        match existing {
            Some(group) => group.push(line),
            None => groups.push(vec![line]),
        }
    }
    for group in &mut groups {
        group.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    }
    // Two tables can share the same rule extent; a large vertical gap starts a new table.
    let mut split: Vec<Vec<HLine>> = Vec::new();
    for group in groups {
        let mut current: Vec<HLine> = Vec::new();
        for line in group {
            if let Some(previous) = current.last() {
                let gap = line.y0 - previous.y1;
                if !(MIN_BAND - 1.0..=MAX_BAND).contains(&gap) {
                    split.push(std::mem::take(&mut current));
                }
            }
            current.push(line);
        }
        if !current.is_empty() {
            split.push(current);
        }
    }
    split
}

fn horizontal_lines(width: u32, height: u32, ink: &impl Fn(u32, u32) -> bool) -> Vec<HLine> {
    let min_run = 30u32;
    let mut raw: Vec<(u32, u32, u32)> = Vec::new();
    for y in 0..height {
        let mut x = 0u32;
        while x < width {
            if ink(x, y) {
                let start = x;
                while x + 1 < width && ink(x + 1, y) {
                    x += 1;
                }
                if x - start + 1 >= min_run {
                    raw.push((y, start, x));
                }
                x += 1;
            } else {
                x += 1;
            }
        }
    }
    let mut lines: Vec<HLine> = Vec::new();
    for (y, start, end) in raw {
        // Rules are often broken by anti-aliasing or by text crossing them, so fragments on
        // neighbouring scanlines belong to the same printed rule.
        let merged = match lines.last_mut() {
            Some(line) if y as f64 <= line.y1 + 2.0 => {
                line.y1 = y as f64;
                line.x0 = line.x0.min(start as f64);
                line.x1 = line.x1.max(end as f64);
                true
            }
            _ => false,
        };
        if !merged {
            lines.push(HLine {
                y0: y as f64,
                y1: y as f64,
                x0: start as f64,
                x1: end as f64,
            });
        }
    }
    lines
        .into_iter()
        .filter(|line| {
            line.x1 - line.x0 >= 0.15 * width as f64
                && line_coverage(line, &ink, width, height) >= 0.80
        })
        .collect()
}

/// Ink coverage along a candidate rule: a printed rule is continuous, a line of text is not.
fn line_coverage(line: &HLine, ink: &impl Fn(u32, u32) -> bool, width: u32, height: u32) -> f64 {
    let y = ((line.y0 + line.y1) / 2.0).round().max(0.0) as u32;
    if y >= height {
        return 0.0;
    }
    let start = line.x0.max(0.0) as u32;
    let end = (line.x1.max(0.0) as u32).min(width.saturating_sub(1));
    if end <= start {
        return 0.0;
    }
    let mut hits = 0usize;
    let mut samples = 0usize;
    let mut x = start;
    while x <= end {
        samples += 1;
        if ink(x, y) {
            hits += 1;
        }
        x += 2;
    }
    if samples == 0 {
        0.0
    } else {
        hits as f64 / samples as f64
    }
}

/// Vertical rule positions inside one table region.
fn local_verticals(
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
    ink: &impl Fn(u32, u32) -> bool,
    width: u32,
    height: u32,
) -> Vec<f64> {
    let mut found: Vec<f64> = Vec::new();
    let start_x = x0.max(0.0) as u32;
    let end_x = (x1.max(0.0) as u32).min(width.saturating_sub(1));
    let start_y = y0.max(0.0) as u32;
    let end_y = (y1.max(0.0) as u32).min(height.saturating_sub(1));
    if end_x <= start_x || end_y <= start_y {
        return found;
    }
    for x in start_x..=end_x {
        let mut hits = 0usize;
        let mut samples = 0usize;
        let mut y = start_y;
        while y <= end_y {
            samples += 1;
            if ink(x, y) {
                hits += 1;
            }
            y += 2;
        }
        if samples > 0 && hits as f64 / samples as f64 >= BORDER_COVERAGE {
            let position = x as f64;
            if found
                .last()
                .is_none_or(|last| (position - last).abs() >= 3.0)
            {
                found.push(position);
            }
        }
    }
    found
}

/// Ink coverage of one column inside a row band: a present rule reads as a continuous line.
fn band_coverage(
    x: f64,
    band_top: f64,
    band_bottom: f64,
    ink: &impl Fn(u32, u32) -> bool,
    width: u32,
    height: u32,
) -> f64 {
    let px = x.round() as i64;
    if px < 0 || px >= width as i64 {
        return 0.0;
    }
    let start = band_top.round().max(0.0) as u32;
    let end = band_bottom.round().max(0.0) as u32;
    if end <= start || end > height {
        return 0.0;
    }
    let mut hits = 0usize;
    let mut samples = 0usize;
    let mut y = start;
    while y < end {
        samples += 1;
        if ink(px as u32, y) {
            hits += 1;
        }
        y += 2;
    }
    if samples == 0 {
        0.0
    } else {
        hits as f64 / samples as f64
    }
}

fn build_table(
    group: &[HLine],
    words: &[Word],
    ink: &impl Fn(u32, u32) -> bool,
    width: u32,
    height: u32,
) -> Option<DetectedTable> {
    let top = group.first()?.y0;
    let bottom = group.last()?.y1;
    let left = group
        .iter()
        .map(|line| line.x0)
        .fold(f64::INFINITY, f64::min);
    let right = group.iter().map(|line| line.x1).fold(0.0, f64::max);
    let columns = local_verticals(left, right, top, bottom, ink, width, height);
    if columns.len() < 3 {
        return None;
    }

    let mut cells: Vec<Cell> = Vec::new();
    let mut words_placed = 0usize;
    for (row_index, pair) in group.windows(2).enumerate() {
        let band_top = pair[0].y0;
        let band_bottom = pair[1].y1;
        let mut column_index = 0usize;
        while column_index + 1 < columns.len() {
            // Extend the cell while the next column rule is missing inside this row band:
            // that is a merged cell.
            let mut span = 1usize;
            while column_index + span + 1 < columns.len() {
                let boundary = columns[column_index + span];
                let coverage = band_coverage(boundary, band_top, band_bottom, ink, width, height);
                if coverage >= MERGED_COVERAGE {
                    break;
                }
                span += 1;
            }
            let left_edge = columns[column_index];
            let right_edge = columns[column_index + span];
            let mut cell_words: Vec<&Word> = words
                .iter()
                .filter(|word| {
                    let cx = word.x + word.width / 2.0;
                    let cy = word.y + word.height / 2.0;
                    cx >= left_edge - 2.0 && cx < right_edge && cy >= band_top && cy <= band_bottom
                })
                .collect();
            cell_words.sort_by(|a, b| a.x.total_cmp(&b.x));
            let raw = cell_words
                .iter()
                .map(|word| word.text.trim())
                .filter(|text| !text.is_empty() && !is_rule_text(text))
                .collect::<Vec<_>>()
                .join(" ");
            words_placed += cell_words.len();
            if !raw.is_empty() {
                cells.push(Cell {
                    row: row_index as u32,
                    column: column_index as u32,
                    value: cell_value(&raw),
                    raw,
                    formula: None,
                    cached_value: None,
                    row_span: 1,
                    column_span: span as u32,
                });
            }
            column_index += span;
        }
    }
    if cells.is_empty() {
        return None;
    }
    let rows_count = cells.iter().map(|cell| cell.row + 1).max().unwrap_or(0) as usize;
    let columns_count = cells
        .iter()
        .map(|cell| cell.column + cell.column_span)
        .max()
        .unwrap_or(0) as usize;
    let table = Table {
        cells,
        rows: rows_count,
        columns: columns_count,
        headers: vec![],
        compact: false,
    };
    Some(DetectedTable {
        table,
        bounds: Bounds {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
            coordinate_space: "image_pixels".into(),
        },
        words_placed,
    })
}

/// Tokens that are printed rules rather than content.
fn is_rule_text(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= 3
        && trimmed
            .chars()
            .all(|c| matches!(c, '|' | '¦' | '।' | '_' | 'I'))
}

fn cell_value(raw: &str) -> serde_json::Value {
    if raw.is_empty() {
        return serde_json::Value::String(String::new());
    }
    if raw.eq_ignore_ascii_case("true") {
        return json!(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return json!(false);
    }
    if !(raw.starts_with('0') && raw.len() > 1 && !raw.starts_with("0.")) {
        if let Ok(value) = raw.parse::<i64>() {
            return json!(value);
        }
        if let Ok(value) = raw.parse::<f64>() {
            if value.is_finite() {
                return json!(value);
            }
        }
    }
    json!(raw)
}
