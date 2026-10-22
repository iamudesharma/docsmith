//! Pure-Rust WebAssembly client engine for the browser workbench.
//!
//! Compiles to both native targets (for `cargo test` and `cargo clippy`) and
//! `wasm32-unknown-unknown` (`cdylib`), exposing deterministic client-side format derivation,
//! live RAG chunking, spatial bounding-box SVG overlays, hit-testing, diagram layout,
//! in-browser parser playground execution, and downstream LLM vision prompt packaging.

use crate::{charts, diagrams, doctags, layout, markup, model::*};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

thread_local! {
    static OUTPUT_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Allocate `len` bytes in WASM linear memory for the host to write UTF-8 strings into.
#[no_mangle]
pub extern "C" fn wasm_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let buf = vec![0u8; len].into_boxed_slice();
    Box::into_raw(buf) as *mut u8
}

/// Free a buffer previously allocated by [`wasm_alloc`].
///
/// # Safety
/// `ptr` and `len` must match a live allocation returned by [`wasm_alloc`].
#[no_mangle]
pub unsafe extern "C" fn wasm_dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() && len > 0 {
        let slice = std::ptr::slice_from_raw_parts_mut(ptr, len);
        drop(Box::from_raw(slice));
    }
}

/// Pointer to the most recent UTF-8 JSON response written by [`wasm_invoke`].
#[no_mangle]
pub extern "C" fn wasm_output_ptr() -> *const u8 {
    OUTPUT_BUF.with(|buf| buf.borrow().as_ptr())
}

/// Byte length of the most recent UTF-8 JSON response written by [`wasm_invoke`].
#[no_mangle]
pub extern "C" fn wasm_output_len() -> usize {
    OUTPUT_BUF.with(|buf| buf.borrow().len())
}

/// Invoke a named WASM operation with a UTF-8 JSON payload. Writes the UTF-8 JSON response
/// into the thread-local output buffer and returns `1` on success or `0` on error.
///
/// # Safety
/// `op_ptr..op_ptr+op_len` and `arg_ptr..arg_ptr+arg_len` must be valid readable byte slices.
#[no_mangle]
pub unsafe extern "C" fn wasm_invoke(
    op_ptr: *const u8,
    op_len: usize,
    arg_ptr: *const u8,
    arg_len: usize,
) -> u32 {
    let op_bytes = if op_ptr.is_null() || op_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(op_ptr, op_len)
    };
    let arg_bytes = if arg_ptr.is_null() || arg_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(arg_ptr, arg_len)
    };
    let op = std::str::from_utf8(op_bytes).unwrap_or("");
    let arg = std::str::from_utf8(arg_bytes).unwrap_or("{}");
    let (ok, body) = match invoke_op(op, arg) {
        Ok(val) => (1u32, serde_json::to_vec(&val).unwrap_or_default()),
        Err(err) => (
            0u32,
            serde_json::to_vec(&json!({
                "error": {
                    "code": "wasm_error",
                    "message": err.to_string(),
                }
            }))
            .unwrap_or_default(),
        ),
    };
    OUTPUT_BUF.with(|buf| {
        *buf.borrow_mut() = body;
    });
    ok
}

/// Safe Rust entry point used by both [`wasm_invoke`] and native integration tests.
pub fn invoke(op: &str, payload: &str) -> String {
    match invoke_op(op, payload) {
        Ok(val) => serde_json::to_string(&val).unwrap_or_else(|_| "{}".into()),
        Err(err) => serde_json::to_string(&json!({
            "error": {
                "code": "wasm_error",
                "message": err.to_string(),
            }
        }))
        .unwrap_or_else(|_| "{}".into()),
    }
}

pub fn invoke_op(op: &str, payload: &str) -> Result<Value> {
    let input: Value = if payload.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(payload).context("invalid JSON payload")?
    };
    match op {
        "ping" => Ok(json!({
            "ok": true,
            "engine": "doc-extractor-wasm",
            "version": env!("CARGO_PKG_VERSION"),
            "arch": std::env::consts::ARCH,
            "operations": [
                "ping",
                "validate_options",
                "inspect_document",
                "derive_unit",
                "render_page_svg",
                "hit_test_page",
                "render_diagram_svg",
                "playground_extract",
                "build_llm_payload"
            ]
        })),
        "validate_options" => validate_options_op(&input),
        "inspect_document" => inspect_document_op(&input),
        "derive_unit" => derive_unit_op(&input),
        "render_page_svg" => render_page_svg_op(&input),
        "hit_test_page" => hit_test_page_op(&input),
        "render_diagram_svg" => render_diagram_svg_op(&input),
        "playground_extract" => playground_extract_op(&input),
        "build_llm_payload" => build_llm_payload_op(&input),
        other => bail!("unknown wasm operation '{other}'"),
    }
}

fn validate_options_op(input: &Value) -> Result<Value> {
    let opts_val = input.get("options").unwrap_or(input);
    let mut errors = Vec::new();
    let opts: Options = match serde_json::from_value(opts_val.clone()) {
        Ok(o) => o,
        Err(err) => {
            errors.push(format!("options schema: {err}"));
            Options::default()
        }
    };
    if !["direct", "xberg"].contains(&opts.parser.as_str()) {
        errors.push(format!("unsupported parser '{}'", opts.parser));
    }
    if !["tesseract", "apple_vision", "smoldocling"].contains(&opts.ocr_provider.as_str()) {
        errors.push(format!("unsupported ocr_provider '{}'", opts.ocr_provider));
    }
    if !["json", "markdown", "text"].contains(&opts.result_format.as_str()) {
        errors.push(format!(
            "unsupported result_format '{}'",
            opts.result_format
        ));
    }
    if !["none", "failed", "all"].contains(&opts.page_images.as_str()) {
        errors.push(format!(
            "unsupported page_images '{}' (expected none, failed or all)",
            opts.page_images
        ));
    }
    if opts.chunk_chars == Some(0) {
        errors.push("chunk_chars must be at least 1".into());
    }
    let mut page_count = None;
    let mut page_preview = Vec::new();
    let mut open_ended = false;
    if let Some(spec) = opts
        .pages
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        open_ended = spec.split(',').any(|p| p.trim().ends_with('-'));
        match parse_page_range(spec) {
            Ok(pages) => {
                page_count = Some(pages.len());
                page_preview = pages.into_iter().take(24).collect();
            }
            Err(err) => errors.push(format!("invalid page range: {err}")),
        }
    }
    Ok(json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "normalized": opts,
        "selected_pages_count": page_count,
        "selected_pages_preview": page_preview,
        "open_ended": open_ended,
    }))
}

fn inspect_document_op(input: &Value) -> Result<Value> {
    let doc_val = input.get("document").unwrap_or(input);
    let doc: Document = serde_json::from_value(doc_val.clone()).context("invalid Document JSON")?;
    let manifest = page_image_manifest(&doc);
    let mut native_blocks = 0usize;
    let mut ocr_blocks = 0usize;
    let mut inferred_blocks = 0usize;
    let mut native_chars = 0usize;
    let mut ocr_chars = 0usize;
    let mut inferred_chars = 0usize;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut tables_count = 0usize;
    let mut total_rows = 0usize;
    let mut total_cells = 0usize;
    let mut formulas_count = 0usize;
    let mut diagrams_count = 0usize;
    let mut diagram_nodes = 0usize;
    let mut diagram_edges = 0usize;
    let mut units_summary = Vec::with_capacity(doc.units.len());

    for u in &doc.units {
        let mut u_native = 0usize;
        let mut u_ocr = 0usize;
        let mut u_inferred = 0usize;
        let mut u_tables = 0usize;
        let mut u_diagrams = 0usize;
        let mut u_figures = 0usize;
        let mut u_bounds = 0usize;
        let mut u_page_image: Option<Value> = None;

        for b in &u.blocks {
            *kinds.entry(b.kind.clone()).or_insert(0) += 1;
            let chars = b.text.chars().count();
            match b.provenance.method.as_str() {
                "ocr" => {
                    ocr_blocks += 1;
                    ocr_chars += chars;
                    u_ocr += 1;
                }
                "inferred" => {
                    inferred_blocks += 1;
                    inferred_chars += chars;
                    u_inferred += 1;
                }
                _ => {
                    native_blocks += 1;
                    native_chars += chars;
                    u_native += 1;
                }
            }
            if b.provenance.bounds.is_some() {
                u_bounds += 1;
            }
            if let Some(t) = &b.table {
                tables_count += 1;
                u_tables += 1;
                total_rows += t.rows;
                total_cells += t.cells.len();
                formulas_count += t.cells.iter().filter(|c| c.formula.is_some()).count();
            }
            if let Some(d) = &b.diagram {
                diagrams_count += 1;
                u_diagrams += 1;
                diagram_nodes += d.nodes.len();
                diagram_edges += d.edges.len();
            }
            if b.kind == "figure" {
                u_figures += 1;
            }
            if let Some(meta) = b.attributes.get("page_image") {
                u_page_image = Some(meta.clone());
            }
        }
        units_summary.push(json!({
            "id": u.id,
            "name": u.name,
            "kind": u.kind,
            "index": u.index,
            "status": u.status,
            "blocks": u.blocks.len(),
            "native": u_native,
            "ocr": u_ocr,
            "inferred": u_inferred,
            "tables": u_tables,
            "diagrams": u_diagrams,
            "figures": u_figures,
            "bounds_blocks": u_bounds,
            "page_image": u_page_image,
            "warnings": u.warnings.len(),
            "errors": u.errors.len(),
        }));
    }

    Ok(json!({
        "id": doc.id,
        "filename": doc.filename,
        "mime": doc.mime,
        "status": doc.status,
        "elapsed_ms": doc.elapsed_ms,
        "units_count": doc.units.len(),
        "assets_count": doc.assets.len(),
        "warnings_count": doc.warnings.len(),
        "errors_count": doc.errors.len(),
        "page_images": manifest,
        "provenance": {
            "native_blocks": native_blocks,
            "ocr_blocks": ocr_blocks,
            "inferred_blocks": inferred_blocks,
            "total_blocks": native_blocks + ocr_blocks + inferred_blocks,
            "native_chars": native_chars,
            "ocr_chars": ocr_chars,
            "inferred_chars": inferred_chars,
            "total_chars": native_chars + ocr_chars + inferred_chars,
        },
        "kinds": kinds,
        "tables": {
            "count": tables_count,
            "rows": total_rows,
            "cells": total_cells,
            "formulas": formulas_count,
        },
        "diagrams": {
            "count": diagrams_count,
            "nodes": diagram_nodes,
            "edges": diagram_edges,
        },
        "units": units_summary,
    }))
}

fn derive_unit_op(input: &Value) -> Result<Value> {
    let unit_val = input.get("unit").unwrap_or(input);
    let mut unit: Unit = serde_json::from_value(unit_val.clone()).context("invalid Unit JSON")?;
    let chunk_chars = input
        .get("chunk_chars")
        .and_then(Value::as_u64)
        .unwrap_or(320) as usize;
    let chunk_overlap = input
        .get("chunk_overlap")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or_else(|| (chunk_chars / 8).max(16));
    let compact_tables = input
        .get("compact_tables")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let image_width = input.get("image_width").and_then(Value::as_f64);
    let image_height = input.get("image_height").and_then(Value::as_f64);
    let dpi = input.get("dpi").and_then(Value::as_f64).unwrap_or(150.0);
    let filter = input.get("filter").and_then(Value::as_str).unwrap_or("all");
    let active_block_id = input
        .get("active_block_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let overlay_only = input
        .get("overlay_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    for b in &mut unit.blocks {
        if let Some(t) = &mut b.table {
            t.compact = compact_tables;
        }
    }

    let mut md = String::new();
    unit_markdown(&unit, &mut md);

    let mut txt = String::new();
    unit_plaintext(&unit, &mut txt);

    let mut html = String::new();
    unit_html(&unit, &mut html);

    let mut jsonl = String::new();
    unit_jsonl(&unit, &mut jsonl)?;

    let mut chunks_jsonl = String::new();
    unit_chunks(&unit, chunk_chars, chunk_overlap, &mut chunks_jsonl)?;
    let chunks: Vec<Value> = chunks_jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let unit_json = serde_json::to_string_pretty(&unit)?;
    let page_svg = build_page_svg(
        &unit,
        image_width,
        image_height,
        dpi,
        filter,
        active_block_id,
        overlay_only,
    );

    let mut diagrams_out = Vec::new();
    for b in &unit.blocks {
        if let Some(d) = &b.diagram {
            diagrams_out.push(json!({
                "block_id": b.id,
                "source": b.provenance.source,
                "method": b.provenance.method,
                "nodes": d.nodes.len(),
                "edges": d.edges.len(),
                "warnings": d.warnings,
                "svg": build_diagram_svg(d, ""),
            }));
        }
    }

    let native = unit
        .blocks
        .iter()
        .filter(|b| b.provenance.method == "native")
        .count();
    let ocr = unit
        .blocks
        .iter()
        .filter(|b| b.provenance.method == "ocr")
        .count();
    let inferred = unit
        .blocks
        .iter()
        .filter(|b| b.provenance.method == "inferred")
        .count();
    let tables = unit.blocks.iter().filter(|b| b.table.is_some()).count();
    let diagrams_cnt = unit.blocks.iter().filter(|b| b.diagram.is_some()).count();
    let figures = unit.blocks.iter().filter(|b| b.kind == "figure").count();
    let bounds_blocks = unit
        .blocks
        .iter()
        .filter(|b| b.provenance.bounds.is_some())
        .count();

    Ok(json!({
        "unit_id": unit.id,
        "unit_name": unit.name,
        "markdown": md,
        "plaintext": txt,
        "html": html,
        "jsonl": jsonl,
        "chunks_jsonl": chunks_jsonl,
        "chunks": chunks,
        "unit_json": unit_json,
        "page_svg": page_svg,
        "diagrams": diagrams_out,
        "stats": {
            "blocks": unit.blocks.len(),
            "native": native,
            "ocr": ocr,
            "inferred": inferred,
            "tables": tables,
            "diagrams": diagrams_cnt,
            "figures": figures,
            "bounds_blocks": bounds_blocks,
            "chars": txt.chars().count(),
            "words": txt.split_whitespace().count(),
            "chunks_count": chunks.len(),
        }
    }))
}

fn render_page_svg_op(input: &Value) -> Result<Value> {
    let unit_val = input.get("unit").unwrap_or(input);
    let unit: Unit = serde_json::from_value(unit_val.clone()).context("invalid Unit JSON")?;
    let image_width = input.get("image_width").and_then(Value::as_f64);
    let image_height = input.get("image_height").and_then(Value::as_f64);
    let dpi = input.get("dpi").and_then(Value::as_f64).unwrap_or(150.0);
    let filter = input.get("filter").and_then(Value::as_str).unwrap_or("all");
    let active_block_id = input
        .get("active_block_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let overlay_only = input
        .get("overlay_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let svg = build_page_svg(
        &unit,
        image_width,
        image_height,
        dpi,
        filter,
        active_block_id,
        overlay_only,
    );
    Ok(json!({ "svg": svg }))
}

fn resolve_canvas_dimensions(
    unit: &Unit,
    image_width: Option<f64>,
    image_height: Option<f64>,
    dpi: f64,
) -> (f64, f64, f64) {
    let pdf_scale = if image_width.is_some() && dpi > 0.0 {
        dpi / 72.0
    } else {
        1.0
    };
    let mut max_x = 612.0 * pdf_scale;
    let mut max_y = 792.0 * pdf_scale;
    for b in &unit.blocks {
        if let Some(bounds) = &b.provenance.bounds {
            let s = if bounds.coordinate_space == "pdf_points_top_left" {
                pdf_scale
            } else {
                1.0
            };
            max_x = max_x.max((bounds.x + bounds.width) * s + 24.0);
            max_y = max_y.max((bounds.y + bounds.height) * s + 24.0);
        }
    }
    let width = image_width.filter(|&w| w > 1.0).unwrap_or(max_x);
    let height = image_height.filter(|&h| h > 1.0).unwrap_or(max_y);
    (width, height, pdf_scale)
}

fn scaled_block_bounds(bounds: &Bounds, pdf_scale: f64) -> (f64, f64, f64, f64) {
    let scale = if bounds.coordinate_space == "pdf_points_top_left" {
        pdf_scale
    } else {
        1.0
    };
    (
        bounds.x * scale,
        bounds.y * scale,
        (bounds.width * scale).max(6.0),
        (bounds.height * scale).max(6.0),
    )
}

fn build_page_svg(
    unit: &Unit,
    image_width: Option<f64>,
    image_height: Option<f64>,
    dpi: f64,
    filter: &str,
    active_block_id: &str,
    overlay_only: bool,
) -> String {
    let (width, height, pdf_scale) =
        resolve_canvas_dimensions(unit, image_width, image_height, dpi);
    let mut svg = String::with_capacity(4096);
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {width:.1} {height:.1}\" data-canvas-width=\"{width:.1}\" data-canvas-height=\"{height:.1}\" class=\"wasm-page-svg\">"
    ));
    if !overlay_only {
        svg.push_str(&format!(
            "<rect x=\"0\" y=\"0\" width=\"{width:.1}\" height=\"{height:.1}\" rx=\"6\" fill=\"#ffffff\" stroke=\"#cbd5e1\" stroke-width=\"1.5\"/>"
        ));
        // Subtle page grid lines in synthetic vector mode
        let step = (width / 12.0).max(40.0);
        let mut gx = step;
        while gx < width {
            svg.push_str(&format!(
                "<line x1=\"{gx:.1}\" y1=\"0\" x2=\"{gx:.1}\" y2=\"{height:.1}\" stroke=\"#f1f5f9\" stroke-width=\"0.8\"/>"
            ));
            gx += step;
        }
        let mut gy = step;
        while gy < height {
            svg.push_str(&format!(
                "<line x1=\"0\" y1=\"{gy:.1}\" x2=\"{width:.1}\" y2=\"{gy:.1}\" stroke=\"#f1f5f9\" stroke-width=\"0.8\"/>"
            ));
            gy += step;
        }
    }

    for b in &unit.blocks {
        if filter != "all" && b.provenance.method != filter {
            continue;
        }
        let Some(bounds) = &b.provenance.bounds else {
            continue;
        };
        let (x, y, w, h) = scaled_block_bounds(bounds, pdf_scale);
        let is_active = !active_block_id.is_empty() && b.id == active_block_id;
        let (stroke, fill, badge_bg) = match b.provenance.method.as_str() {
            "ocr" => (
                "#f59e0b",
                if is_active {
                    "rgba(245,158,11,0.32)"
                } else {
                    "rgba(245,158,11,0.13)"
                },
                "#b45309",
            ),
            "inferred" => (
                "#a855f7",
                if is_active {
                    "rgba(168,85,247,0.32)"
                } else {
                    "rgba(168,85,247,0.12)"
                },
                "#7e22ce",
            ),
            _ => (
                "#38bdf8",
                if is_active {
                    "rgba(56,189,248,0.32)"
                } else {
                    "rgba(56,189,248,0.12)"
                },
                "#0369a1",
            ),
        };
        let stroke_width = if is_active { 2.8 } else { 1.4 };
        let dash = if b.provenance.method == "inferred" {
            " stroke-dasharray=\"5,3\""
        } else {
            ""
        };
        svg.push_str(&format!(
            "<g class=\"bbox-group{}\" data-block-id=\"{}\" data-method=\"{}\" data-kind=\"{}\" style=\"cursor:pointer\">",
            if is_active { " is-active" } else { "" },
            html_escape(&b.id),
            html_escape(&b.provenance.method),
            html_escape(&b.kind),
        ));
        svg.push_str(&format!(
            "<rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" rx=\"3\" fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"{stroke_width:.1}\"{dash}/>"
        ));
        if !overlay_only && !b.text.trim().is_empty() {
            let preview: String = b.text.chars().take(42).collect();
            let font_size = (h * 0.58).clamp(9.0, 15.0);
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"#0f172a\" font-size=\"{:.1}\" font-family=\"-apple-system, BlinkMacSystemFont, sans-serif\">{}</text>",
                x + 4.0,
                y + (h * 0.72).min(h - 2.0).max(10.0),
                font_size,
                html_escape(&preview)
            ));
        }
        let badge_label = format!("{} · {}", b.id, b.provenance.method);
        let badge_w = (badge_label.len() as f64 * 5.8 + 10.0).min(w.max(68.0));
        let badge_y = (y - 15.0).max(2.0);
        svg.push_str(&format!(
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"14\" rx=\"3\" fill=\"{}\" opacity=\"{}\"/>",
            x,
            badge_y,
            badge_w,
            badge_bg,
            if is_active { "0.98" } else { "0.82" }
        ));
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"#f8fafc\" font-size=\"9.5\" font-family=\"system-ui, -apple-system, sans-serif\" font-weight=\"600\">{}</text>",
            x + 4.0,
            badge_y + 10.2,
            html_escape(&badge_label)
        ));
        svg.push_str("</g>");
    }
    svg.push_str("</svg>");
    svg
}

fn hit_test_page_op(input: &Value) -> Result<Value> {
    let unit_val = input.get("unit").unwrap_or(input);
    let unit: Unit = serde_json::from_value(unit_val.clone()).context("invalid Unit JSON")?;
    let x = input.get("x").and_then(Value::as_f64).unwrap_or(0.0);
    let y = input.get("y").and_then(Value::as_f64).unwrap_or(0.0);
    let image_width = input.get("image_width").and_then(Value::as_f64);
    let image_height = input.get("image_height").and_then(Value::as_f64);
    let dpi = input.get("dpi").and_then(Value::as_f64).unwrap_or(150.0);
    let filter = input.get("filter").and_then(Value::as_str).unwrap_or("all");

    let (_, _, pdf_scale) = resolve_canvas_dimensions(&unit, image_width, image_height, dpi);
    let mut matches: Vec<(f64, &Block)> = Vec::new();
    for b in &unit.blocks {
        if filter != "all" && b.provenance.method != filter {
            continue;
        }
        let Some(bounds) = &b.provenance.bounds else {
            continue;
        };
        let (bx, by, bw, bh) = scaled_block_bounds(bounds, pdf_scale);
        if x >= bx && x <= bx + bw && y >= by && y <= by + bh {
            matches.push((bw * bh, b));
        }
    }
    matches.sort_by(|a, b| a.0.total_cmp(&b.0));
    let hit = matches.first().map(|(_, b)| (*b).clone());
    let enclosing: Vec<String> = matches.iter().map(|(_, b)| b.id.clone()).collect();
    Ok(json!({
        "hit": hit.as_ref().map(|b| b.id.clone()),
        "enclosing": enclosing,
        "block": hit,
    }))
}

fn render_diagram_svg_op(input: &Value) -> Result<Value> {
    let diag_val = input.get("diagram").unwrap_or(input);
    let diagram: Diagram =
        serde_json::from_value(diag_val.clone()).context("invalid Diagram JSON")?;
    let active_node = input
        .get("active_node_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let svg = build_diagram_svg(&diagram, active_node);
    Ok(json!({
        "svg": svg,
        "nodes": diagram.nodes.len(),
        "edges": diagram.edges.len(),
        "warnings": diagram.warnings,
    }))
}

fn normalize_endpoint(endpoint: &str, node_ids: &[String]) -> String {
    if node_ids.iter().any(|id| id == endpoint) {
        return endpoint.to_owned();
    }
    if let Some((prefix, _)) = endpoint.split_once('.') {
        if node_ids.iter().any(|id| id == prefix) {
            return prefix.to_owned();
        }
    }
    endpoint.to_owned()
}

fn build_diagram_svg(diagram: &Diagram, active_node_id: &str) -> String {
    if diagram.nodes.is_empty() {
        return "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 420 120\" class=\"wasm-diagram-svg\"><rect width=\"420\" height=\"120\" rx=\"8\" fill=\"#0f172a\" stroke=\"#334155\"/><text x=\"210\" y=\"64\" text-anchor=\"middle\" fill=\"#94a3b8\" font-size=\"13\" font-family=\"system-ui, sans-serif\">Empty diagram graph (0 nodes)</text></svg>".into();
    }

    let node_ids: Vec<String> = diagram.nodes.iter().map(|n| n.id.clone()).collect();
    let has_explicit_geometry = diagram.nodes.iter().any(|n| {
        n.bounds
            .as_ref()
            .is_some_and(|b| b.width > 1.0 && b.height > 1.0)
    });

    let mut positions: HashMap<String, (f64, f64, f64, f64)> = HashMap::new();
    if has_explicit_geometry {
        for (idx, n) in diagram.nodes.iter().enumerate() {
            if let Some(b) = &n.bounds {
                positions.insert(
                    n.id.clone(),
                    (
                        b.x.max(20.0),
                        b.y.max(20.0),
                        b.width.max(130.0),
                        b.height.max(54.0),
                    ),
                );
            } else {
                positions.insert(
                    n.id.clone(),
                    (
                        40.0 + (idx % 3) as f64 * 220.0,
                        40.0 + (idx / 3) as f64 * 110.0,
                        160.0,
                        60.0,
                    ),
                );
            }
        }
    } else {
        // Compute topological layer for each node
        let mut indegree: HashMap<String, usize> =
            node_ids.iter().map(|id| (id.clone(), 0usize)).collect();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for e in &diagram.edges {
            if let (Some(raw_s), Some(raw_t)) = (&e.source, &e.target) {
                let s = normalize_endpoint(raw_s, &node_ids);
                let t = normalize_endpoint(raw_t, &node_ids);
                if s != t && indegree.contains_key(&s) && indegree.contains_key(&t) {
                    adj.entry(s).or_default().push(t.clone());
                    *indegree.entry(t).or_insert(0) += 1;
                }
            }
        }
        let mut layer: HashMap<String, usize> = HashMap::new();
        let mut queue: Vec<String> = node_ids
            .iter()
            .filter(|id| indegree.get(*id).copied().unwrap_or(0) == 0)
            .cloned()
            .collect();
        if queue.is_empty() {
            queue.push(node_ids[0].clone());
        }
        for id in &queue {
            layer.insert(id.clone(), 0);
        }
        let mut head = 0usize;
        while head < queue.len() {
            let curr = queue[head].clone();
            head += 1;
            let next_layer = layer.get(&curr).copied().unwrap_or(0) + 1;
            if let Some(neighbors) = adj.get(&curr) {
                for nb in neighbors {
                    let entry = layer.entry(nb.clone()).or_insert(0);
                    if next_layer > *entry && next_layer <= diagram.nodes.len() {
                        *entry = next_layer;
                    }
                    if let Some(deg) = indegree.get_mut(nb) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 && !queue.contains(nb) {
                            queue.push(nb.clone());
                        }
                    }
                }
            }
        }
        for id in &node_ids {
            layer.entry(id.clone()).or_insert(0);
        }

        let mut layers: BTreeMap<usize, Vec<&DiagramNode>> = BTreeMap::new();
        for n in &diagram.nodes {
            let l = layer.get(&n.id).copied().unwrap_or(0);
            layers.entry(l).or_default().push(n);
        }

        for (col_idx, (_, nodes_in_layer)) in layers.iter().enumerate() {
            let x = 36.0 + col_idx as f64 * 255.0;
            let mut y = 36.0;
            for n in nodes_in_layer {
                let attr_count = n
                    .attributes
                    .get("entity_attributes")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let w = 195.0;
                let h = if attr_count > 0 {
                    44.0 + attr_count as f64 * 20.0
                } else {
                    54.0
                };
                positions.insert(n.id.clone(), (x, y, w, h));
                y += h + 32.0;
            }
        }
    }

    let mut max_x = 460.0f64;
    let mut max_y = 220.0f64;
    for &(x, y, w, h) in positions.values() {
        max_x = max_x.max(x + w + 44.0);
        max_y = max_y.max(y + h + 44.0);
    }

    let mut svg = String::with_capacity(4096);
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {max_x:.1} {max_y:.1}\" class=\"wasm-diagram-svg\">"
    ));
    svg.push_str(
        "<defs>\
        <marker id=\"arrow-native\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"6.5\" markerHeight=\"6.5\" orient=\"auto-start-reverse\">\
          <path d=\"M 0 1 L 10 5 L 0 9 z\" fill=\"#38bdf8\"/>\
        </marker>\
        <marker id=\"arrow-inferred\" viewBox=\"0 0 10 10\" refX=\"9\" refY=\"5\" markerWidth=\"6.5\" markerHeight=\"6.5\" orient=\"auto-start-reverse\">\
          <path d=\"M 0 1 L 10 5 L 0 9 z\" fill=\"#a855f7\"/>\
        </marker>\
        </defs>",
    );
    svg.push_str(&format!(
        "<rect width=\"{max_x:.1}\" height=\"{max_y:.1}\" rx=\"8\" fill=\"#0f172a\" stroke=\"#1e293b\"/>"
    ));

    // Render edges
    for e in &diagram.edges {
        let (Some(raw_s), Some(raw_t)) = (&e.source, &e.target) else {
            continue;
        };
        let s_id = normalize_endpoint(raw_s, &node_ids);
        let t_id = normalize_endpoint(raw_t, &node_ids);
        let (Some(&(sx, sy, sw, sh)), Some(&(tx, ty, tw, th))) =
            (positions.get(&s_id), positions.get(&t_id))
        else {
            continue;
        };
        let (x1, y1, x2, y2) = if sx + sw <= tx {
            (sx + sw, sy + sh / 2.0, tx, ty + th / 2.0)
        } else if tx + tw <= sx {
            (sx, sy + sh / 2.0, tx + tw, ty + th / 2.0)
        } else {
            (sx + sw / 2.0, sy + sh, tx + tw / 2.0, ty)
        };
        let dx = ((x2 - x1).abs() * 0.45).max(32.0);
        let cx1 = if x2 >= x1 { x1 + dx } else { x1 - dx };
        let cx2 = if x2 >= x1 { x2 - dx } else { x2 + dx };
        let inferred = e.method == "inferred";
        let stroke = if inferred { "#a855f7" } else { "#38bdf8" };
        let marker = if inferred {
            "url(#arrow-inferred)"
        } else {
            "url(#arrow-native)"
        };
        let dash = if inferred {
            " stroke-dasharray=\"5,4\""
        } else {
            ""
        };
        svg.push_str(&format!(
            "<path d=\"M {x1:.1} {y1:.1} C {cx1:.1} {y1:.1}, {cx2:.1} {y2:.1}, {x2:.1} {y2:.1}\" fill=\"none\" stroke=\"{stroke}\" stroke-width=\"1.8\" marker-end=\"{marker}\"{dash}/>"
        ));
        let mut edge_tag = e.label.clone();
        if let Some(card) = &e.cardinality {
            if edge_tag.is_empty() {
                edge_tag = card.clone();
            } else {
                edge_tag = format!("{edge_tag} ({card})");
            }
        }
        if !edge_tag.is_empty() {
            let mx = (x1 + x2) / 2.0;
            let my = (y1 + y2) / 2.0 - 6.0;
            let pill_w = (edge_tag.len() as f64 * 6.0 + 14.0).max(36.0);
            svg.push_str(&format!(
                "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"16\" rx=\"8\" fill=\"#1e293b\" stroke=\"{}\" stroke-width=\"1\"/>",
                mx - pill_w / 2.0,
                my - 11.0,
                pill_w,
                stroke
            ));
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"#e2e8f0\" font-size=\"10\" font-family=\"ui-monospace, SFMono-Regular, monospace\">{}</text>",
                mx,
                my,
                html_escape(&edge_tag)
            ));
        }
    }

    // Render nodes
    for n in &diagram.nodes {
        let Some(&(x, y, w, h)) = positions.get(&n.id) else {
            continue;
        };
        let is_active = !active_node_id.is_empty() && n.id == active_node_id;
        let inferred = n
            .attributes
            .get("inferred")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || n.attributes
                .get("candidate_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let border = if is_active {
            "#f59e0b"
        } else if inferred {
            "#a855f7"
        } else {
            "#38bdf8"
        };
        let header_bg = if inferred { "#3b0764" } else { "#0c4a6e" };
        let attrs = n
            .attributes
            .get("entity_attributes")
            .and_then(Value::as_array);

        svg.push_str(&format!(
            "<g class=\"diagram-node\" data-node-id=\"{}\" style=\"cursor:pointer\">",
            html_escape(&n.id)
        ));
        svg.push_str(&format!(
            "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" rx=\"7\" fill=\"#1e293b\" stroke=\"{}\" stroke-width=\"{}\"/>",
            x,
            y,
            w,
            h,
            border,
            if is_active { "2.6" } else { "1.6" }
        ));
        if let Some(fields) = attrs.filter(|a| !a.is_empty()) {
            svg.push_str(&format!(
                "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"30\" rx=\"7\" fill=\"{}\"/>",
                x + 1.0,
                y + 1.0,
                w - 2.0,
                header_bg
            ));
            let title = if n.label.is_empty() { &n.id } else { &n.label };
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"#f8fafc\" font-size=\"12\" font-weight=\"700\" font-family=\"system-ui, sans-serif\">{}</text>",
                x + 10.0,
                y + 20.0,
                html_escape(title)
            ));
            for (idx, attr) in fields.iter().enumerate() {
                let fy = y + 46.0 + idx as f64 * 20.0;
                let fname = attr["name"].as_str().unwrap_or("");
                let ftype = attr["type"].as_str().unwrap_or("");
                let pk = attr["primary_key"].as_bool().unwrap_or(false);
                let fk = attr["foreign_key"].as_bool().unwrap_or(false);
                let badge = if pk {
                    "PK "
                } else if fk {
                    "FK "
                } else {
                    ""
                };
                let badge_color = if pk { "#fbbf24" } else { "#38bdf8" };
                if !badge.is_empty() {
                    svg.push_str(&format!(
                        "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{}\" font-size=\"10\" font-weight=\"700\" font-family=\"ui-monospace, monospace\">{}</text>",
                        x + 10.0,
                        fy,
                        badge_color,
                        badge
                    ));
                }
                svg.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"#e2e8f0\" font-size=\"11\" font-family=\"ui-monospace, monospace\">{}</text>",
                    x + if badge.is_empty() { 10.0 } else { 32.0 },
                    fy,
                    html_escape(fname)
                ));
                svg.push_str(&format!(
                    "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\" fill=\"#94a3b8\" font-size=\"10\" font-family=\"ui-monospace, monospace\">{}</text>",
                    x + w - 10.0,
                    fy,
                    html_escape(ftype)
                ));
            }
        } else {
            let title = if n.label.is_empty() { &n.id } else { &n.label };
            let display: String = title.chars().take(28).collect();
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"#f8fafc\" font-size=\"12.5\" font-weight=\"600\" font-family=\"system-ui, sans-serif\">{}</text>",
                x + w / 2.0,
                y + h / 2.0 + 1.0,
                html_escape(&display)
            ));
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" fill=\"#64748b\" font-size=\"9.5\" font-family=\"ui-monospace, monospace\">id: {} · {}</text>",
                x + w / 2.0,
                y + h - 8.0,
                html_escape(&n.id),
                if inferred { "inferred" } else { "native" }
            ));
        }
        svg.push_str("</g>");
    }

    svg.push_str("</svg>");
    svg
}

fn playground_extract_op(input: &Value) -> Result<Value> {
    let source = input
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let format = input
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("mermaid");
    let chunk_chars = input
        .get("chunk_chars")
        .and_then(Value::as_u64)
        .unwrap_or(320) as usize;
    let chunk_overlap = input
        .get("chunk_overlap")
        .and_then(Value::as_u64)
        .unwrap_or(40) as usize;
    let compact_tables = input
        .get("compact_tables")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut unit = Unit::new(0, "section", format!("WASM Playground ({format})"));
    match format {
        "mermaid" | "plantuml" | "dot" | "dbml" => {
            let diagram = diagrams::source(source, format)?;
            unit.warnings.extend(diagram.warnings.clone());
            let b = unit.text("diagram", source, format!("WASM {format} parser"));
            b.diagram = Some(diagram);
        }
        "drawio" => {
            let pages = diagrams::drawio(source, 8 * 1024 * 1024)?;
            for (name, diagram) in pages {
                unit.warnings.extend(diagram.warnings.clone());
                let b = unit.text("diagram", name, "WASM draw.io XML parser");
                b.diagram = Some(diagram);
            }
        }
        "vision_json" => {
            let diagram = diagrams::vision_graph(source)?;
            unit.warnings.extend(diagram.warnings.clone());
            let b = unit.text("diagram", "Vision JSON graph", "WASM vision_graph parser");
            b.provenance.method = "inferred".into();
            b.diagram = Some(diagram);
        }
        "markdown" => {
            markup::markdown(&mut unit, source);
        }
        "chart_xml" => {
            charts::extract(source.as_bytes(), "xl/charts/chart1.xml", &mut unit)?;
        }
        "doctags" => {
            doctags::extract(source, &mut unit, 612, 792, "ocr")?;
        }
        "pdf_layout" => {
            let words = parse_layout_words(source)?;
            for w in &words {
                let b = unit.text("text", &w.text, "PDF word bbox");
                b.provenance.bounds = Some(Bounds {
                    x: w.x,
                    y: w.y,
                    width: w.width,
                    height: w.height,
                    coordinate_space: "pdf_points_top_left".into(),
                });
            }
            layout::pdf_tables(&mut unit, words);
            layout::figure_graph(&mut unit);
        }
        other => bail!("unsupported playground format '{other}'"),
    }

    if !unit.warnings.is_empty() {
        unit.status = "partial".into();
    }

    derive_unit_op(&json!({
        "unit": unit,
        "chunk_chars": chunk_chars,
        "chunk_overlap": chunk_overlap,
        "compact_tables": compact_tables,
    }))
}

fn parse_layout_words(source: &str) -> Result<Vec<layout::Word>> {
    let trimmed = source.trim();
    if trimmed.starts_with('[') {
        let items: Vec<Value> = serde_json::from_str(trimmed).context("invalid word JSON array")?;
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            out.push(layout::Word {
                text: item["text"].as_str().unwrap_or("").to_owned(),
                x: item["x"].as_f64().unwrap_or(0.0),
                y: item["y"].as_f64().unwrap_or(0.0),
                width: item["width"].as_f64().unwrap_or(32.0),
                height: item["height"].as_f64().unwrap_or(12.0),
            });
        }
        return Ok(out);
    }
    let mut out = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        if parts.len() < 3 {
            bail!("expected 'text, x, y[, width, height]' line, got '{line}'");
        }
        out.push(layout::Word {
            text: parts[0].to_owned(),
            x: parts[1].parse().context("invalid x coordinate")?,
            y: parts[2].parse().context("invalid y coordinate")?,
            width: parts
                .get(3)
                .and_then(|s| s.parse().ok())
                .unwrap_or((parts[0].len() as f64 * 6.5).max(18.0)),
            height: parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(12.0),
        });
    }
    Ok(out)
}

fn build_llm_payload_op(input: &Value) -> Result<Value> {
    let doc_val = input.get("document").unwrap_or(input);
    let doc: Document = serde_json::from_value(doc_val.clone()).context("invalid Document JSON")?;
    let target_unit = input.get("unit_id").and_then(Value::as_str);
    let base_url = input
        .get("base_url")
        .and_then(Value::as_str)
        .unwrap_or("http://127.0.0.1:8787")
        .trim_end_matches('/');

    let manifest = page_image_manifest(&doc);
    let selected: Vec<&PageImage> = manifest
        .pages
        .iter()
        .filter(|p| target_unit.is_none_or(|id| p.unit_id == id))
        .collect();

    let mut items = Vec::with_capacity(selected.len());
    for page in selected {
        let asset_url = format!(
            "{base_url}/v1/extractions/{}/assets/{}",
            doc.id, page.asset_id
        );
        let unit = doc.units.iter().find(|u| u.id == page.unit_id);
        let existing_text = unit
            .map(|u| {
                let mut t = String::new();
                unit_plaintext(u, &mut t);
                t.trim().to_owned()
            })
            .unwrap_or_default();
        let prompt = format!(
            "Extract all text, tables, key-value fields and diagram relationships from {} ({}, page {}, capture reason: {}). Preserve exact cell strings and mark uncertain visual relationships as inferred.{}",
            doc.filename,
            page.unit_name,
            page.page,
            page.reason,
            if existing_text.is_empty() {
                String::new()
            } else {
                format!("\n\nExisting partial text recovered by doc-extractor:\n{existing_text}")
            }
        );
        items.push(json!({
            "unit_id": page.unit_id,
            "unit_name": page.unit_name,
            "page": page.page,
            "reason": page.reason,
            "path": page.path,
            "asset_id": page.asset_id,
            "asset_url": asset_url,
            "mime": page.mime,
            "width": page.width,
            "height": page.height,
            "dpi": page.dpi,
            "bytes": page.bytes,
            "hash": page.hash,
            "existing_text": existing_text,
            "prompt": prompt,
            "smoldocling_mlx_request": {
                "model": "ds4sd/SmolDocling-256M-preview-mlx-bf16",
                "temperature": 0,
                "max_tokens": 2048,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "image_url", "image_url": { "url": asset_url } },
                        { "type": "text", "text": "Convert this page to docling." }
                    ]
                }]
            },
            "openai_vision_request": {
                "model": "gpt-4.1-mini",
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "text", "text": prompt },
                        { "type": "image_url", "image_url": { "url": asset_url, "detail": "high" } }
                    ]
                }]
            },
            "anthropic_vision_request": {
                "model": "claude-sonnet-4-20250514",
                "max_tokens": 4096,
                "messages": [{
                    "role": "user",
                    "content": [
                        { "type": "image", "source": { "type": "url", "url": asset_url } },
                        { "type": "text", "text": prompt }
                    ]
                }]
            }
        }));
    }

    Ok(json!({
        "document_id": doc.id,
        "filename": doc.filename,
        "mode": manifest.mode,
        "count": items.len(),
        "items": items,
    }))
}
