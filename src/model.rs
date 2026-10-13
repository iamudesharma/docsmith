use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub parser: String,
    pub ocr: bool,
    pub ocr_auto_rotate: bool,
    pub ocr_provider: String,
    pub languages: String,
    pub vision: bool,
    pub interpret_diagrams: bool,
    pub delimiter: Option<char>,
    pub flexible_rows: bool,
    /// Omit default/redundant cell fields in table output (value when equal to raw, null
    /// formulas, unit spans). Smaller results for large delimited files; consumers treat a
    /// missing `value` as the raw string.
    pub compact_tables: bool,
    /// Default representation of this job's result: `json` (canonical), `markdown` or
    /// `text`. `GET .../result` uses it when no `?format=` query is given, so a job can be
    /// requested in the form that will be shipped downstream (for example to object storage).
    pub result_format: String,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            parser: "direct".into(),
            ocr: false,
            ocr_auto_rotate: true,
            ocr_provider: "tesseract".into(),
            languages: "eng".into(),
            vision: false,
            interpret_diagrams: false,
            delimiter: None,
            flexible_rows: true,
            compact_tables: false,
            result_format: "json".into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub upload_bytes: u64,
    pub archive_bytes: u64,
    pub archive_entries: usize,
    pub units: usize,
    pub page_pixels: u64,
    pub output_bytes: u64,
    pub timeout_seconds: u64,
    pub memory_bytes: u64,
    pub workers: usize,
    pub queued_jobs: usize,
    /// Parallel 100-page batch workers inside a PDF extraction (1 = sequential).
    pub pdf_workers: usize,
    /// Concurrent OCR subprocesses across all page workers (1 = serial OCR).
    pub ocr_threads: usize,
    /// Delimited inputs larger than this are extracted in streamed mode: block payloads are
    /// dropped from memory once each unit file is written, and result/markdown assembly reads
    /// the unit files one at a time.
    pub stream_bytes: u64,
    /// Retained bytes per tenant across completed jobs; 0 disables the quota.
    pub disk_bytes: u64,
    /// Seconds a terminal job is retained before the sweep removes it; 0 disables the sweep.
    pub retention_seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            upload_bytes: 32 * 1024 * 1024,
            archive_bytes: 128 * 1024 * 1024,
            archive_entries: 4096,
            units: 20_000,
            page_pixels: 20_000_000,
            output_bytes: 512 * 1024 * 1024,
            timeout_seconds: 1800,
            memory_bytes: 2 * 1024 * 1024 * 1024,
            workers: 2,
            queued_jobs: 100,
            pdf_workers: 4,
            ocr_threads: 4,
            stream_bytes: 16 * 1024 * 1024,
            disk_bytes: 0,
            retention_seconds: 0,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub coordinate_space: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    pub unit_id: String,
    pub method: String,
    pub source: String,
    pub bounds: Option<Bounds>,
    pub cell_range: Option<String>,
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Cell {
    pub row: u32,
    pub column: u32,
    pub raw: String,
    pub value: Value,
    pub formula: Option<String>,
    pub cached_value: Option<Value>,
    pub row_span: u32,
    pub column_span: u32,
}
#[derive(Debug, Clone, Default, JsonSchema)]
pub struct Table {
    pub cells: Vec<Cell>,
    pub rows: usize,
    pub columns: usize,
    pub headers: Vec<usize>,
    /// Output style only: omit default/redundant cell fields when serializing. Not round-tripped.
    #[schemars(skip)]
    pub compact: bool,
}
impl serde::Serialize for Table {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Table", 4)?;
        state.serialize_field("cells", &TableCells(self))?;
        state.serialize_field("rows", &self.rows)?;
        state.serialize_field("columns", &self.columns)?;
        state.serialize_field("headers", &self.headers)?;
        state.end()
    }
}
struct TableCells<'a>(&'a Table);
impl serde::Serialize for TableCells<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.0.cells.len()))?;
        for cell in &self.0.cells {
            if self.0.compact {
                seq.serialize_element(&CompactCell(cell))?;
            } else {
                seq.serialize_element(cell)?;
            }
        }
        seq.end()
    }
}
/// Compact cell wire form: `value` is omitted when it equals the raw string, null
/// formula/cached values and default spans are omitted entirely.
struct CompactCell<'a>(&'a Cell);
impl serde::Serialize for CompactCell<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let cell = self.0;
        let omit_value =
            matches!(&cell.value, Value::String(v) if v == &cell.raw) && !cell.raw.is_empty();
        let mut state = serializer.serialize_struct("Cell", 8)?;
        state.serialize_field("row", &cell.row)?;
        state.serialize_field("column", &cell.column)?;
        state.serialize_field("raw", &cell.raw)?;
        if !omit_value {
            state.serialize_field("value", &cell.value)?;
        }
        if let Some(formula) = &cell.formula {
            state.serialize_field("formula", formula)?;
        }
        if let Some(cached) = &cell.cached_value {
            state.serialize_field("cached_value", cached)?;
        }
        if cell.row_span != 1 {
            state.serialize_field("row_span", &cell.row_span)?;
        }
        if cell.column_span != 1 {
            state.serialize_field("column_span", &cell.column_span)?;
        }
        state.end()
    }
}
#[derive(serde::Deserialize)]
struct TableWire {
    cells: Vec<CellWire>,
    rows: usize,
    columns: usize,
    headers: Vec<usize>,
}
#[derive(serde::Deserialize)]
struct CellWire {
    row: u32,
    column: u32,
    #[serde(default)]
    raw: String,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    formula: Option<String>,
    #[serde(default)]
    cached_value: Option<Value>,
    #[serde(default = "one")]
    row_span: u32,
    #[serde(default = "one")]
    column_span: u32,
}
fn one() -> u32 {
    1
}
impl<'de> serde::Deserialize<'de> for Table {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let wire = TableWire::deserialize(deserializer)?;
        Ok(Table {
            cells: wire
                .cells
                .into_iter()
                .map(|cell| Cell {
                    row: cell.row,
                    column: cell.column,
                    // A missing `value` is the compact form of "value equals the raw string".
                    value: cell
                        .value
                        .unwrap_or_else(|| Value::String(cell.raw.clone())),
                    raw: cell.raw,
                    formula: cell.formula,
                    cached_value: cell.cached_value,
                    row_span: cell.row_span,
                    column_span: cell.column_span,
                })
                .collect(),
            rows: wire.rows,
            columns: wire.columns,
            headers: wire.headers,
            compact: false,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Block {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub provenance: Provenance,
    pub table: Option<Table>,
    pub asset_id: Option<String>,
    pub diagram: Option<Diagram>,
    pub attributes: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiagramNode {
    pub id: String,
    pub label: String,
    pub parent: Option<String>,
    pub bounds: Option<Bounds>,
    pub attributes: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiagramEdge {
    pub id: String,
    pub source: Option<String>,
    pub target: Option<String>,
    pub label: String,
    pub method: String,
    pub cardinality: Option<String>,
    pub attributes: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct Diagram {
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Unit {
    pub id: String,
    pub kind: String,
    pub index: usize,
    pub name: String,
    pub status: String,
    pub blocks: Vec<Block>,
    pub warnings: Vec<String>,
    pub errors: Vec<Failure>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Asset {
    pub id: String,
    pub mime: String,
    pub hash: String,
    pub bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub provenance: Provenance,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Failure {
    pub code: String,
    pub message: String,
    pub unit_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Document {
    pub schema_version: String,
    pub id: String,
    pub hash: String,
    pub mime: String,
    pub filename: String,
    pub versions: Value,
    pub options: Options,
    pub status: String,
    pub metadata: Value,
    pub units: Vec<Unit>,
    pub assets: Vec<Asset>,
    pub warnings: Vec<String>,
    pub errors: Vec<Failure>,
    pub elapsed_ms: u128,
}
impl Unit {
    pub fn new(index: usize, kind: &str, name: impl Into<String>) -> Self {
        Self {
            id: format!("u{}", index + 1),
            kind: kind.into(),
            index,
            name: name.into(),
            status: "complete".into(),
            blocks: vec![],
            warnings: vec![],
            errors: vec![],
        }
    }
    pub fn text(
        &mut self,
        kind: &str,
        text: impl Into<String>,
        source: impl Into<String>,
    ) -> &mut Block {
        let b = Block {
            id: format!("{}-b{}", self.id, self.blocks.len() + 1),
            kind: kind.into(),
            text: text.into(),
            provenance: Provenance {
                unit_id: self.id.clone(),
                method: "native".into(),
                source: source.into(),
                bounds: None,
                cell_range: None,
                evidence: vec![],
            },
            table: None,
            asset_id: None,
            diagram: None,
            attributes: Value::Null,
        };
        self.blocks.push(b);
        self.blocks.last_mut().unwrap()
    }
}
pub fn markdown(doc: &Document) -> String {
    let mut out = format!("# {}\n\nStatus: {}\n\n", doc.filename, doc.status);
    for u in &doc.units {
        unit_markdown(u, &mut out);
    }
    out
}
/// Append one unit's Markdown section. Split out so large results can be streamed unit by
/// unit without building the whole document string in memory.
pub fn unit_markdown(u: &Unit, out: &mut String) {
    out.push_str(&format!("## {}\n\n", u.name));
    for b in &u.blocks {
        if let Some(t) = &b.table {
            let cells = t
                .cells
                .iter()
                .map(|cell| ((cell.row, cell.column), cell))
                .collect::<std::collections::HashMap<_, _>>();
            if t.headers.is_empty() {
                out.push_str(&format!(
                    "| {} |\n",
                    (0..t.columns)
                        .map(|i| format!("Column {}", i + 1))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
                out.push_str(&format!("| {} |\n", vec!["---"; t.columns].join(" | ")));
            }
            for r in 0..t.rows {
                let row = (0..t.columns)
                    .map(|c| {
                        cells
                            .get(&(r as u32, c as u32))
                            .map(|x| x.raw.replace('|', "\\|").replace('\n', " "))
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>();
                out.push_str(&format!("| {} |\n", row.join(" | ")));
                if r == 0 && !t.headers.is_empty() {
                    out.push_str(&format!("| {} |\n", vec!["---"; t.columns].join(" | ")));
                }
            }
        } else {
            out.push_str(&b.text);
            out.push('\n');
        }
        if let Some(id) = &b.asset_id {
            out.push_str(&format!("[Asset {id}](assets/{id})\n"));
        }
        out.push('\n');
    }
    for w in &u.warnings {
        out.push_str(&format!("> Warning: {w}\n"));
    }
}

pub fn plaintext(doc: &Document) -> String {
    let mut out = String::new();
    for unit in &doc.units {
        unit_plaintext(unit, &mut out);
    }
    out
}
/// Append one unit's tab-separated text. See [`unit_markdown`] for why this is split.
pub fn unit_plaintext(unit: &Unit, out: &mut String) {
    for block in &unit.blocks {
        if let Some(table) = &block.table {
            let cells = table
                .cells
                .iter()
                .map(|cell| ((cell.row, cell.column), cell.raw.as_str()))
                .collect::<std::collections::HashMap<_, _>>();
            for r in 0..table.rows {
                for c in 0..table.columns {
                    if c > 0 {
                        out.push('\t');
                    }
                    out.push_str(cells.get(&(r as u32, c as u32)).copied().unwrap_or(""));
                }
                out.push('\n');
            }
        } else {
            out.push_str(&block.text);
            out.push('\n');
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub units: usize,
    pub result_status: Option<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<Failure>,
}
