//! Conservative geometric proposals. Their provenance always says inferred.
use crate::model::*;
use serde_json::json;
#[derive(Clone)]
pub struct Word {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
pub fn pdf_tables(u: &mut Unit, mut words: Vec<Word>) {
    words.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut lines: Vec<Vec<Word>> = vec![];
    for w in words {
        if lines.last().is_some_and(|row| (row[0].y - w.y).abs() < 3.) {
            lines.last_mut().unwrap().push(w);
        } else {
            lines.push(vec![w]);
        }
    }
    let mut groups: Vec<Vec<Vec<Word>>> = vec![];
    for mut line in lines {
        line.sort_by(|a, b| a.x.total_cmp(&b.x));
        let mut cols: Vec<Vec<Word>> = vec![];
        for w in line {
            if cols
                .last()
                .and_then(|c| c.last())
                .is_some_and(|last| w.x - (last.x + last.width) < 18.)
            {
                cols.last_mut().unwrap().push(w);
            } else {
                cols.push(vec![w]);
            }
        }
        if cols.len() < 2 {
            if groups.len() >= 3 {
                add_table(u, &groups);
            }
            groups.clear();
            continue;
        }
        let aligns = groups.last().is_some_and(|previous| {
            previous.len() == cols.len()
                && previous
                    .iter()
                    .zip(&cols)
                    .all(|(a, b)| (a[0].x - b[0].x).abs() < 6.)
                && (cols[0][0].y - previous[0][0].y) < 50.
        });
        if !aligns && !groups.is_empty() {
            if groups.len() >= 3 {
                add_table(u, &groups);
            }
            groups.clear();
        }
        groups.push(cols);
    }
    if groups.len() >= 3 {
        add_table(u, &groups);
    }
}
fn add_table(u: &mut Unit, rows: &[Vec<Vec<Word>>]) {
    let mut cells = vec![];
    for (r, row) in rows.iter().enumerate() {
        for (c, words) in row.iter().enumerate() {
            let raw = words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            cells.push(Cell {
                row: r as u32,
                column: c as u32,
                raw: raw.clone(),
                value: json!(raw),
                formula: None,
                cached_value: None,
                row_span: 1,
                column_span: 1,
            });
        }
    }
    let xmin = rows
        .iter()
        .flatten()
        .flatten()
        .map(|w| w.x)
        .fold(f64::INFINITY, f64::min);
    let ymin = rows
        .iter()
        .flatten()
        .flatten()
        .map(|w| w.y)
        .fold(f64::INFINITY, f64::min);
    let xmax = rows
        .iter()
        .flatten()
        .flatten()
        .map(|w| w.x + w.width)
        .fold(0., f64::max);
    let ymax = rows
        .iter()
        .flatten()
        .flatten()
        .map(|w| w.y + w.height)
        .fold(0., f64::max);
    let b = u.text("table", "", "PDF aligned-word table proposal");
    b.provenance.method = "inferred".into();
    b.provenance.bounds = Some(Bounds {
        x: xmin,
        y: ymin,
        width: xmax - xmin,
        height: ymax - ymin,
        coordinate_space: "pdf_points_top_left".into(),
    });
    b.provenance.evidence =
        vec!["At least three rows, two columns and consistent column starts".into()];
    b.table = Some(Table {
        cells,
        rows: rows.len(),
        columns: rows[0].len(),
        headers: vec![],
        compact: false,
    });
    u.warnings.push("Geometric table proposal may also represent aligned prose. Native text retained; no header or merged-cell inference".into());
}
pub fn figure_graph(u: &mut Unit) {
    let nodes = u
        .blocks
        .iter()
        .filter(|b| b.kind == "text" && !b.text.trim().is_empty())
        .filter_map(|b| {
            b.provenance.bounds.as_ref().map(|bounds| DiagramNode {
                id: b.id.clone(),
                label: b.text.clone(),
                parent: None,
                bounds: Some(bounds.clone()),
                attributes: json!({"candidate_only":true,"native_label_block":b.id}),
            })
        })
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        return;
    }
    let b = u.text(
        "diagram",
        "Candidate labels from flattened vector source",
        "PDF label geometry",
    );
    b.provenance.method = "inferred".into();
    b.diagram=Some(Diagram{nodes,edges:vec![],warnings:vec!["Label boxes are candidates, not verified entities; vector connector directions/cardinalities remain unknown".into()]});
}
