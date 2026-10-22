//! Pure-Rust parser for SmolDocling-256M DocTags (`<doctag>`, `<loc_0..500>`, and `<otsl>` tables).
//!
//! SmolDocling-256M emits a compact token stream where every element can carry four normalized
//! `0..=500` location tags (`<loc_x1><loc_y1><loc_x2><loc_y2>`) and tables use OTSL
//! (`<ched>`, `<rhed>`, `<fcel>`, `<ecel>`, `<lcel>`, `<ucel>`, `<xcel>`, `<nl>`).
//! This module compiles to both native targets and `wasm32-unknown-unknown` with zero native C/Python
//! dependencies.

use crate::model::*;
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Check whether a model response or file contains SmolDocling DocTags markup.
pub fn contains_doctags(source: &str) -> bool {
    source.contains("<doctag>")
        || source.contains("<otsl>")
        || source.contains("<fcel>")
        || source.contains("<ched>")
        || source.contains("<loc_")
}

/// Parse a SmolDocling DocTags string and append the resulting blocks (`text`, `heading`, `list`,
/// `table`, `figure`, `footnote`) and bounding boxes to `unit`.
pub fn extract(source: &str, unit: &mut Unit, width: u32, height: u32, method: &str) -> Result<()> {
    let (blocks, warnings) = parse_blocks(source, width, height, method)?;
    unit.warnings.extend(warnings);
    for mut block in blocks {
        block.provenance.unit_id = unit.id.clone();
        block.id = format!("{}-b{}", unit.id, unit.blocks.len() + 1);
        unit.blocks.push(block);
    }
    Ok(())
}

/// Parse SmolDocling DocTags into standalone [`Block`]s (with temporary IDs) and warnings.
pub fn parse_blocks(
    source: &str,
    width: u32,
    height: u32,
    method: &str,
) -> Result<(Vec<Block>, Vec<String>)> {
    if source.len() > 16 * 1024 * 1024 {
        bail!("limit_exceeded: DocTags input exceeds 16 MiB");
    }
    let page_w = if width > 0 { width as f64 } else { 500.0 };
    let page_h = if height > 0 { height as f64 } else { 500.0 };
    let coord_space = if width > 0 && height > 0 {
        "image_pixels"
    } else {
        "doctags_500"
    };

    let open_re = regex::Regex::new(
        r"<(section_header_level_\d+|title|page_header|text|paragraph|list_item|code|formula|equation|caption|footnote|page_footer|otsl|table|picture|chart|figure)>",
    )?;

    let mut blocks = Vec::new();
    let mut warnings = Vec::new();
    let mut cursor = 0usize;

    while let Some(cap) = open_re.captures(&source[cursor..]) {
        let Some(open_match) = cap.get(0) else {
            break;
        };
        let tag = &cap[1];
        let body_start = cursor + open_match.end();
        let close_tag = format!("</{tag}>");
        let (inner, next_cursor) = match source[body_start..].find(&close_tag) {
            Some(rel_end) => (
                &source[body_start..body_start + rel_end],
                body_start + rel_end + close_tag.len(),
            ),
            None => (&source[body_start..], source.len()),
        };
        cursor = next_cursor;
        let (bounds, remainder) = extract_leading_bounds(inner, page_w, page_h, coord_space);

        match tag {
            "otsl" | "table" => {
                let table = parse_otsl_table(remainder, &mut warnings);
                if table.rows == 0 && table.columns == 0 {
                    continue;
                }
                blocks.push(Block {
                    id: format!("dt-b{}", blocks.len() + 1),
                    kind: "table".into(),
                    text: String::new(),
                    provenance: Provenance {
                        unit_id: String::new(),
                        method: method.into(),
                        source: "SmolDocling-256M DocTags OTSL".into(),
                        bounds,
                        cell_range: None,
                        evidence: vec!["OTSL structural table tags".into()],
                    },
                    table: Some(table),
                    asset_id: None,
                    diagram: None,
                    attributes: json!({
                        "doctags_tag": tag,
                        "model": "SmolDocling-256M",
                    }),
                });
            }
            "picture" | "chart" | "figure" => {
                let cleaned = strip_all_tags(remainder).trim().to_owned();
                blocks.push(Block {
                    id: format!("dt-b{}", blocks.len() + 1),
                    kind: "figure".into(),
                    text: cleaned,
                    provenance: Provenance {
                        unit_id: String::new(),
                        method: method.into(),
                        source: "SmolDocling-256M DocTags".into(),
                        bounds,
                        cell_range: None,
                        evidence: vec![],
                    },
                    table: None,
                    asset_id: None,
                    diagram: None,
                    attributes: json!({
                        "doctags_tag": tag,
                        "model": "SmolDocling-256M",
                    }),
                });
            }
            _ => {
                let kind = match tag {
                    "title" | "page_header" => "heading",
                    s if s.starts_with("section_header") => "heading",
                    "list_item" => "list",
                    "caption" | "footnote" | "page_footer" => "footnote",
                    _ => "text",
                };
                let cleaned = strip_all_tags(remainder).trim().to_owned();
                if cleaned.is_empty() {
                    continue;
                }
                let mut attrs = serde_json::Map::new();
                attrs.insert("doctags_tag".into(), json!(tag));
                attrs.insert("model".into(), json!("SmolDocling-256M"));
                if tag == "code" {
                    attrs.insert("code".into(), json!(true));
                } else if tag == "formula" || tag == "equation" {
                    attrs.insert("formula".into(), json!(true));
                }
                blocks.push(Block {
                    id: format!("dt-b{}", blocks.len() + 1),
                    kind: kind.into(),
                    text: cleaned,
                    provenance: Provenance {
                        unit_id: String::new(),
                        method: method.into(),
                        source: "SmolDocling-256M DocTags".into(),
                        bounds,
                        cell_range: None,
                        evidence: vec![],
                    },
                    table: None,
                    asset_id: None,
                    diagram: None,
                    attributes: Value::Object(attrs),
                });
            }
        }
    }

    // Fallback: if the input uses bare `<ched>` / `<fcel>` OTSL tokens without an outer `<otsl>` wrapper
    if blocks.is_empty() && (source.contains("<fcel>") || source.contains("<ched>")) {
        let (bounds, remainder) = extract_leading_bounds(source, page_w, page_h, coord_space);
        let table = parse_otsl_table(remainder, &mut warnings);
        if table.rows > 0 && table.columns > 0 {
            blocks.push(Block {
                id: "dt-b1".into(),
                kind: "table".into(),
                text: String::new(),
                provenance: Provenance {
                    unit_id: String::new(),
                    method: method.into(),
                    source: "SmolDocling-256M DocTags OTSL".into(),
                    bounds,
                    cell_range: None,
                    evidence: vec!["OTSL structural table tags".into()],
                },
                table: Some(table),
                asset_id: None,
                diagram: None,
                attributes: json!({
                    "doctags_tag": "otsl",
                    "model": "SmolDocling-256M",
                }),
            });
        }
    }

    if blocks.is_empty() {
        let plain = strip_all_tags(source).trim().to_owned();
        if !plain.is_empty() {
            let (bounds, _) = extract_leading_bounds(source, page_w, page_h, coord_space);
            blocks.push(Block {
                id: "dt-b1".into(),
                kind: "text".into(),
                text: plain,
                provenance: Provenance {
                    unit_id: String::new(),
                    method: method.into(),
                    source: "SmolDocling-256M DocTags".into(),
                    bounds,
                    cell_range: None,
                    evidence: vec![],
                },
                table: None,
                asset_id: None,
                diagram: None,
                attributes: json!({ "model": "SmolDocling-256M" }),
            });
        } else {
            warnings.push("SmolDocling DocTags stream contained no extractable blocks".into());
        }
    }

    Ok((blocks, warnings))
}

/// Extract up to four leading `<loc_N>` tags (`0..=500`) and map them to `Bounds`.
fn extract_leading_bounds<'a>(
    input: &'a str,
    page_w: f64,
    page_h: f64,
    coord_space: &str,
) -> (Option<Bounds>, &'a str) {
    let loc_re = regex::Regex::new(r"^\s*<loc_(\d+)>\s*<loc_(\d+)>\s*<loc_(\d+)>\s*<loc_(\d+)>\s*")
        .expect("static regex");
    if let Some(cap) = loc_re.captures(input) {
        let full_len = cap.get(0).map(|m| m.end()).unwrap_or(0);
        let n = |idx: usize| -> f64 {
            cap.get(idx)
                .and_then(|m| m.as_str().parse::<f64>().ok())
                .unwrap_or(0.0)
                .clamp(0.0, 500.0)
        };
        let x1 = (n(1) / 500.0) * page_w;
        let y1 = (n(2) / 500.0) * page_h;
        let x2 = (n(3) / 500.0) * page_w;
        let y2 = (n(4) / 500.0) * page_h;
        let bounds = Bounds {
            x: x1.min(x2),
            y: y1.min(y2),
            width: (x2 - x1).abs().max(2.0),
            height: (y2 - y1).abs().max(2.0),
            coordinate_space: coord_space.into(),
        };
        return (Some(bounds), &input[full_len..]);
    }
    (None, input)
}

/// Parse SmolDocling's OTSL table representation (`<ched>`, `<rhed>`, `<fcel>`, `<ecel>`,
/// `<lcel>`, `<ucel>`, `<xcel>`, `<nl>`) into a [`Table`].
pub fn parse_otsl_table(input: &str, warnings: &mut Vec<String>) -> Table {
    let token_re =
        regex::Regex::new(r"<(ched|rhed|fcel|ecel|lcel|ucel|xcel|nl)>").expect("static regex");
    let matches: Vec<(usize, usize, &str)> = token_re
        .captures_iter(input)
        .filter_map(|cap| {
            let m = cap.get(0)?;
            let name = cap.get(1)?.as_str();
            Some((m.start(), m.end(), name))
        })
        .collect();

    if matches.is_empty() {
        return Table::default();
    }

    let mut cells: Vec<Cell> = Vec::new();
    let mut headers: Vec<usize> = Vec::new();
    let mut row: u32 = 0;
    let mut col: u32 = 0;
    let mut max_cols: usize = 0;

    for (idx, &(_, token_end, kind)) in matches.iter().enumerate() {
        let next_start = matches
            .get(idx + 1)
            .map(|&(s, _, _)| s)
            .unwrap_or(input.len());
        let raw_payload = &input[token_end..next_start];
        let text = strip_all_tags(raw_payload).trim().to_owned();

        match kind {
            "nl" => {
                max_cols = max_cols.max(col as usize);
                if col > 0 {
                    row += 1;
                    col = 0;
                }
            }
            "ched" | "rhed" | "fcel" | "ecel" => {
                if kind == "ched" && !headers.contains(&(row as usize)) {
                    headers.push(row as usize);
                }
                let raw = if kind == "ecel" { String::new() } else { text };
                let value = infer_cell_value(&raw);
                cells.push(Cell {
                    row,
                    column: col,
                    raw,
                    value,
                    formula: None,
                    cached_value: None,
                    row_span: 1,
                    column_span: 1,
                });
                col += 1;
                max_cols = max_cols.max(col as usize);
            }
            "lcel" => {
                // Horizontal merge continuation of the most recent cell in this row
                if let Some(prev) = cells.iter_mut().rev().find(|c| c.row == row) {
                    prev.column_span += 1;
                } else {
                    warnings.push(format!(
                        "OTSL <lcel> at row {row} col {col} had no left cell"
                    ));
                }
                col += 1;
                max_cols = max_cols.max(col as usize);
            }
            "ucel" | "xcel" => {
                // Vertical merge continuation of the cell above in the same column
                if row > 0 {
                    if let Some(above) = cells
                        .iter_mut()
                        .rev()
                        .find(|c| c.column == col && c.row < row)
                    {
                        if above.row + above.row_span == row {
                            above.row_span += 1;
                        }
                    }
                }
                col += 1;
                max_cols = max_cols.max(col as usize);
            }
            _ => {}
        }
    }

    let total_rows = cells
        .iter()
        .map(|c| (c.row + c.row_span) as usize)
        .max()
        .unwrap_or(0);

    Table {
        cells,
        rows: total_rows,
        columns: max_cols,
        headers,
        compact: false,
    }
}

fn infer_cell_value(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::String(String::new());
    }
    if raw.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    // Preserve leading-zero strings like "0012" as strings
    if !(raw.starts_with('0') && raw.len() > 1 && !raw.starts_with("0.")) {
        if let Ok(int_val) = raw.parse::<i64>() {
            return json!(int_val);
        }
        if let Ok(float_val) = raw.parse::<f64>() {
            if float_val.is_finite() {
                return json!(float_val);
            }
        }
    }
    Value::String(raw.to_owned())
}

fn strip_all_tags(input: &str) -> String {
    let re = regex::Regex::new(r"<[^>]+>").expect("static regex");
    re.replace_all(input, "").into_owned()
}
