use doc_extractor::{diagrams, extract, model::*};
use std::path::PathBuf;
fn fixture(name: &str, options: Options) -> Document {
    let dir = tempfile::tempdir().unwrap();
    extract::extract(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(name),
        name,
        dir.path(),
        options,
        Limits::default(),
        "test",
    )
    .unwrap()
}
fn text(d: &Document) -> String {
    d.units
        .iter()
        .flat_map(|u| u.blocks.iter())
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}
#[test]
fn csv_preserves_strings_quotes_newlines() {
    let d = fixture("quoted.csv", Options::default());
    assert_eq!(d.status, "complete");
    let t = d.units[0].blocks[0].table.as_ref().unwrap();
    assert_eq!(t.rows, 3);
    assert_eq!(t.cells[5].raw, "0012");
    assert_eq!(t.cells[4].raw, "first, second");
    assert_eq!(t.cells[7].raw, "two\nlines");
    assert!(t.headers.is_empty());
}
#[test]
fn sheet_formula_cached_value_and_merge() {
    let d = fixture("workbook.xlsx", Options::default());
    assert!(d.errors.is_empty(), "{:?}", d.errors);
    let t = d.units[0].blocks[0].table.as_ref().unwrap();
    let formula = t.cells.iter().find(|c| c.formula.is_some()).unwrap();
    assert_eq!(formula.formula.as_deref(), Some("SUM(A2:B2)"));
    assert_eq!(formula.cached_value, Some(serde_json::json!(5.0)));
    assert_eq!(t.cells[0].column_span, 2);
}
#[test]
fn ods_typed_cells() {
    let d = fixture("workbook.ods", Options::default());
    assert!(d.errors.is_empty(), "{:?}", d.errors);
    assert_eq!(
        d.units[0].blocks[0].table.as_ref().unwrap().cells[0].value,
        serde_json::json!(42.0)
    );
}
#[test]
fn legacy_xls_typed_values_and_formula() {
    let d = fixture("workbook.xls", Options::default());
    assert!(d.errors.is_empty(), "{:?}", d.errors);
    let t = d.units[0].blocks[0].table.as_ref().unwrap();
    let cell = |row, column| {
        t.cells
            .iter()
            .find(|c| c.row == row && c.column == column)
            .unwrap()
    };
    assert_eq!(cell(0, 0).raw, "Header");
    assert_eq!(cell(1, 1).value, serde_json::json!(3.0));
    let formula = cell(2, 0);
    assert_eq!(formula.formula.as_deref(), Some("5"));
    assert_eq!(formula.cached_value, Some(serde_json::json!(5.0)));
}
#[test]
fn xlsm_macros_never_executed_or_extracted() {
    let d = fixture("workbook.xlsm", Options::default());
    assert!(d.errors.is_empty(), "{:?}", d.errors);
    assert_eq!(d.status, "complete");
    assert!(
        !serde_json::to_string(&d)
            .unwrap()
            .contains("MACRO_SENTINEL"),
        "VBA source must never be extracted or executed"
    );
    let t = d.units[0].blocks[0].table.as_ref().unwrap();
    assert!(t
        .cells
        .iter()
        .any(|c| c.formula.as_deref() == Some("SUM(A2:B2)")));
}
#[test]
fn word_logical_sections_tables_media() {
    let d = fixture("document.docx", Options::default());
    assert_eq!(d.units.len(), 2);
    assert_eq!(d.units[0].kind, "section");
    assert!(d.units[0].blocks.iter().any(|b| b.kind == "heading"));
    assert_eq!(
        d.units[0]
            .blocks
            .iter()
            .find_map(|b| b.table.as_ref())
            .unwrap()
            .cells[0]
            .column_span,
        2
    );
    assert!(d.assets.iter().any(|a| a.mime == "image/png"));
    assert!(text(&d).contains("Section two"));
}
#[test]
fn slides_use_presentation_order() {
    let d = fixture("presentation.pptx", Options::default());
    assert!(d.units[0].blocks[0].text.contains("First in source order"));
    assert!(d.units[1].blocks[0].text.contains("Second in source order"));
    assert_eq!(d.units[0].kind, "slide");
}
#[test]
fn broad_text_formats() {
    for (name, expected) in [
        ("document.odt", "ODT paragraph"),
        ("document.html", "Offline HTML"),
        ("document.md", "Native text"),
        ("document.json", "नमस्ते"),
        ("document.xml", "Native XML"),
        ("message.eml", "Email text"),
        ("book.epub", "EPUB content"),
        ("document.rtf", "Native RTF"),
    ] {
        let d = fixture(name, Options::default());
        assert!(text(&d).contains(expected), "{name}: {:?}", d.errors);
    }
}
#[test]
fn drawio_compressed_source_equivalent() {
    let a = fixture("diagram.drawio", Options::default());
    let b = fixture("compressed.drawio", Options::default());
    let a = a.units[0].blocks[0].diagram.as_ref().unwrap();
    let b = b.units[0].blocks[0].diagram.as_ref().unwrap();
    assert_eq!(a.nodes.len(), 2);
    assert_eq!(b.nodes.len(), 2);
    assert_eq!(b.edges[0].source.as_deref(), Some("orders"));
    assert_eq!(b.edges[0].target.as_deref(), Some("users"));
    assert!(b.edges[0].cardinality.is_none());
    assert_eq!(b.nodes[0].bounds.as_ref().unwrap().x, 10.);
}
#[test]
fn embedded_graph_svg_png() {
    for name in ["diagram.svg", "diagram.drawio.png"] {
        let d = fixture(name, Options::default());
        assert!(
            d.units
                .iter()
                .flat_map(|u| u.blocks.iter())
                .any(|b| b.diagram.as_ref().is_some_and(|g| g.nodes.len() == 2)),
            "{name}: {:?}",
            d.errors
        );
    }
}
#[test]
fn subset_never_silently_drops_constructs() {
    let d = fixture("flow.mmd", Options::default());
    assert_eq!(d.status, "partial");
    let g = d.units[0].blocks[0].diagram.as_ref().unwrap();
    assert_eq!(g.edges.len(), 1);
    assert!(!g.warnings.is_empty());
}
#[test]
fn dot_subset_warns_instead_of_dropping() {
    let d = fixture("flow.dot", Options::default());
    let g = d.units[0].blocks[0].diagram.as_ref().unwrap();
    assert_eq!(g.nodes.len(), 2);
    assert_eq!(g.edges.len(), 1);
    assert!(
        g.warnings
            .iter()
            .any(|w| w.contains("unsupported dot construct")),
        "{:?}",
        g.warnings
    );
}
#[test]
fn xml_entities_and_archive_traversal_rejected() {
    let a = fixture("unsafe.xml", Options::default());
    assert_eq!(a.status, "failed");
    assert_eq!(a.errors[0].code, "unsafe_xml");
    let b = fixture("unsafe.docx", Options::default());
    assert_eq!(b.status, "failed");
    assert!(!PathBuf::from("outside.txt").exists());
}
#[test]
fn corrupt_and_unsupported_are_explicit() {
    let a = fixture("corrupt.pdf", Options::default());
    assert_eq!(a.status, "failed");
    let b = fixture("unsupported.doc", Options::default());
    assert_eq!(b.status, "unsupported");
}
#[test]
fn resource_limits() {
    let dir = tempfile::tempdir().unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/quoted.csv");
    let limits = Limits {
        upload_bytes: 10,
        ..Default::default()
    };
    assert!(extract::extract(
        &path,
        "quoted.csv",
        dir.path(),
        Options::default(),
        limits,
        "test"
    )
    .is_err());
    assert!(diagrams::drawio(
        &std::fs::read_to_string("fixtures/compressed.drawio").unwrap(),
        10
    )
    .is_err());
}
// Tests that mutate process-wide environment (DOC_XBERG) must hold this lock. Tests whose
// assertions depend on environment-derived identity must also hold it: the PDF page
// checkpoint compares provider versions, which include DOC_XBERG metadata.
static PROCESS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn xberg_env_guard(value: Option<&str>) -> impl Drop {
    let previous = std::env::var("DOC_XBERG").ok();
    match value {
        Some(v) => std::env::set_var("DOC_XBERG", v),
        None => std::env::remove_var("DOC_XBERG"),
    }
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("DOC_XBERG", v),
                None => std::env::remove_var("DOC_XBERG"),
            }
        }
    }
    Restore(previous)
}
fn provisioned_xberg() -> Option<PathBuf> {
    let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/providers/xberg-cli-aarch64-apple-darwin/xberg");
    candidate.is_file().then_some(candidate)
}
#[test]
fn xberg_missing_cli_is_explicit() {
    let _lock = PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = xberg_env_guard(None);
    let d = fixture(
        "quoted.csv",
        Options {
            parser: "xberg".into(),
            ..Default::default()
        },
    );
    // A missing provisioned engine is a dependency failure, not an unsupported document.
    assert_eq!(d.status, "failed", "{:?}", d.errors);
    assert_eq!(d.errors[0].code, "dependency_unavailable");
    assert!(
        d.warnings.iter().any(|w| w.contains("DOC_XBERG")),
        "adapter reason must be surfaced: {:?}",
        d.warnings
    );
}
#[test]
fn xberg_rejects_unexpected_version() {
    let _lock = PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // /bin/echo prints its version argument, which is not 1.3.8; the adapter must refuse
    // rather than run an unevaluated engine. ANSI-colored upstream output is covered by
    // the NO_COLOR env and tolerant parse in adapters.rs.
    let _guard = xberg_env_guard(Some("/bin/echo"));
    let d = fixture(
        "quoted.csv",
        Options {
            parser: "xberg".into(),
            ..Default::default()
        },
    );
    assert_eq!(d.status, "unsupported", "{:?}", d.errors);
    assert!(
        d.errors[0].message.contains("1.3.8"),
        "{:?}",
        d.errors[0].message
    );
}
#[test]
fn xberg_adapter_maps_units_when_provisioned() {
    let Some(executable) = provisioned_xberg() else {
        return;
    };
    let _lock = PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = xberg_env_guard(Some(executable.to_str().unwrap()));
    let d = fixture(
        "digital.pdf",
        Options {
            parser: "xberg".into(),
            ..Default::default()
        },
    );
    assert!(
        d.status == "complete" || d.status == "partial",
        "status {} errors {:?}",
        d.status,
        d.errors
    );
    assert_eq!(d.units.len(), 2);
    assert_eq!(d.units[0].kind, "page");
    assert!(d.units[0].blocks.iter().any(|b| b.text.contains("ONE")));
    assert_eq!(d.versions["adapter"], "xberg/1.3.8");
    assert!(
        d.assets.iter().any(|a| a.mime == "application/json"),
        "full upstream JSON must be retained"
    );
    assert!(
        d.units[0]
            .warnings
            .iter()
            .any(|w| w.contains("canonical mapping is partial")),
        "mapping limits must be stated: {:?}",
        d.units[0].warnings
    );
}
#[test]
fn direct_parser_is_unaffected_by_doc_xberg() {
    let Some(executable) = provisioned_xberg() else {
        return;
    };
    let _lock = PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _guard = xberg_env_guard(Some(executable.to_str().unwrap()));
    let d = fixture("quoted.csv", Options::default());
    assert_eq!(d.status, "complete", "{:?}", d.errors);
    let cells = &d.units[0].blocks[0].table.as_ref().unwrap().cells;
    assert!(
        cells.iter().any(|c| c.raw == "0012"),
        "leading zeros must survive: {:?}",
        cells.iter().map(|c| &c.raw).collect::<Vec<_>>()
    );
    assert!(matches!(cells[5].value, serde_json::Value::String(_)));
}
#[test]
fn pdf_native_pages_geometry_without_ocr() {
    if !doc_extractor::providers::available("pdftotext") {
        return;
    }
    let d = fixture("digital.pdf", Options::default());
    let pages = d
        .units
        .iter()
        .filter(|u| u.kind == "page")
        .collect::<Vec<_>>();
    assert_eq!(pages.len(), 2);
    assert!(text(&d).contains("Native page ONE"));
    assert!(pages[0]
        .blocks
        .iter()
        .any(|b| b.kind == "text" && b.provenance.bounds.is_some()));
    assert!(!pages[0].blocks.iter().any(|b| b.provenance.method == "ocr"));
}
#[test]
fn mixed_pdf_ocr_routes_images_and_suppresses_hidden_layer() {
    if !doc_extractor::providers::available("tesseract") {
        return;
    }
    let options = Options {
        ocr: true,
        ..Default::default()
    };
    let d = fixture("mixed.pdf", options.clone());
    assert!(text(&d).contains("Digital heading"));
    assert!(text(&d).contains("Invoice"));
    let d = fixture("hidden-ocr.pdf", options);
    assert!(
        !d.units[0]
            .blocks
            .iter()
            .any(|b| b.provenance.method == "ocr"),
        "{:?}",
        d.units[0].blocks
    );
}
#[test]
fn image_needs_ocr_and_missing_language_is_partial() {
    let d = fixture("scan.png", Options::default());
    assert_eq!(d.status, "needs_ocr");
    let d = fixture(
        "scan.png",
        Options {
            ocr: true,
            languages: "unprovisioned".into(),
            ..Default::default()
        },
    );
    assert_eq!(d.status, "partial");
    assert_eq!(d.units[0].errors[0].code, "dependency_unavailable");
}
#[test]
fn disabled_vision_never_uses_model() {
    let d = fixture("scan.png", Options::default());
    assert!(d
        .units
        .iter()
        .flat_map(|u| u.blocks.iter())
        .all(|b| b.provenance.method != "inferred"));
}
#[test]
fn schema_has_required_contract() {
    let schema = serde_json::to_value(schemars::schema_for!(Document)).unwrap();
    assert!(schema["properties"]["units"].is_object());
    assert!(schema["properties"]["assets"].is_object());
    let spec = doc_extractor::service::openapi();
    assert_eq!(spec["openapi"], "3.1.0");
    assert!(spec["paths"]["/v1/extractions/{id}/cancel"].is_object());
}

#[test]
fn right_angle_ocr_maps_to_original_coordinates() {
    if !doc_extractor::providers::available("tesseract") {
        return;
    }
    let d = fixture(
        "rotated.png",
        Options {
            ocr: true,
            ..Default::default()
        },
    );
    assert!(text(&d).contains("Invoice"), "{d:?}");
    for b in d.units[0]
        .blocks
        .iter()
        .filter(|b| b.provenance.method == "ocr")
    {
        let bounds = b.provenance.bounds.as_ref().unwrap();
        assert!(bounds.x >= 0. && bounds.y >= 0.);
        assert!(bounds.x + bounds.width <= 360. && bounds.y + bounds.height <= 1200.);
    }
}
#[test]
fn source_er_preserves_only_explicit_keys_and_cardinality() {
    let g=diagrams::source("erDiagram\nusers {\n int id PK\n string name\n}\norders {\n int user_id FK\n}\nusers ||--o{ orders : owns\n","mermaid").unwrap();
    assert_eq!(g.nodes.len(), 2);
    assert_eq!(g.edges[0].cardinality.as_deref(), Some("||--o{"));
    assert_eq!(
        g.nodes[0].attributes["entity_attributes"][0]["primary_key"],
        true
    );
    assert_eq!(
        g.nodes[0].attributes["entity_attributes"][1]["foreign_key"],
        false
    );
    assert_eq!(g.edges[0].method, "native");
}
#[test]
fn inferred_graph_does_not_create_missing_endpoints() {
    let g=diagrams::vision_graph(r#"{"nodes":[{"id":"a","label":"A"}],"edges":[{"source":"invented","target":"a","cardinality":null}]}"#).unwrap();
    assert!(g.edges[0].source.is_none());
    assert!(g.edges[0].cardinality.is_none());
    assert_eq!(g.edges[0].method, "inferred");
}
#[test]
fn geometric_tables_keep_native_text_and_inferred_provenance() {
    use doc_extractor::layout::{pdf_tables, Word};
    let mut u = Unit::new(0, "page", "Page");
    u.text("text", "original text", "native");
    let mut words = vec![];
    for r in 0..3 {
        for c in 0..2 {
            words.push(Word {
                text: format!("R{r}C{c}"),
                x: 20. + 100. * c as f64,
                y: 20. + 20. * r as f64,
                width: 30.,
                height: 12.,
            });
        }
    }
    pdf_tables(&mut u, words);
    assert_eq!(u.blocks[0].text, "original text");
    assert_eq!(u.blocks[1].provenance.method, "inferred");
    assert_eq!(u.blocks[1].table.as_ref().unwrap().cells.len(), 6);
}
#[test]
fn openapi_references_resolve() {
    let spec = doc_extractor::service::openapi();
    fn refs(v: &serde_json::Value, spec: &serde_json::Value) {
        if let Some(reference) = v.get("$ref").and_then(|v| v.as_str()) {
            assert!(
                spec.pointer(reference.trim_start_matches('#')).is_some(),
                "{reference}"
            );
        }
        match v {
            serde_json::Value::Object(m) => {
                for v in m.values() {
                    refs(v, spec);
                }
            }
            serde_json::Value::Array(a) => {
                for v in a {
                    refs(v, spec);
                }
            }
            _ => {}
        }
    }
    refs(&spec, &spec);
}

#[test]
fn retained_pdf_source_graph() {
    if !doc_extractor::providers::available("pdfinfo") {
        return;
    }
    let d = fixture("embedded-diagram.pdf", Options::default());
    assert!(
        d.units
            .iter()
            .flat_map(|u| u.blocks.iter())
            .any(|b| b.diagram.as_ref().is_some_and(|g| g.edges.len() == 1)),
        "{:?}",
        d.errors
    );
}
#[test]
fn pdf_table_fixture_infers_exact_grid() {
    if !doc_extractor::providers::available("pdftotext") {
        return;
    }
    let d = fixture("table.pdf", Options::default());
    let b = d.units[0]
        .blocks
        .iter()
        .find(|b| b.table.is_some())
        .unwrap();
    assert_eq!(b.provenance.method, "inferred");
    let t = b.table.as_ref().unwrap();
    assert_eq!((t.rows, t.columns), (3, 2));
    assert_eq!(t.cells[5].raw, "34");
}

#[test]
fn password_protected_pdf_is_explicit() {
    if !doc_extractor::providers::available("pdfinfo") {
        return;
    }
    use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
    let dir = tempfile::tempdir().unwrap();
    let mut pdf = lopdf::Document::load("fixtures/digital.pdf").unwrap();
    pdf.trailer.set(
        "ID",
        lopdf::Object::Array(vec![
            lopdf::Object::string_literal("fixture-document-id"),
            lopdf::Object::string_literal("fixture-document-id"),
        ]),
    );
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &pdf,
        owner_password: "owner",
        user_password: "secret",
        key_length: 128,
        permissions: Permissions::all(),
    })
    .unwrap();
    pdf.encrypt(&state).unwrap();
    let path = dir.path().join("encrypted.pdf");
    pdf.save(&path).unwrap();
    let d = extract::extract(
        &path,
        "encrypted.pdf",
        &dir.path().join("out"),
        Options::default(),
        Limits::default(),
        "encrypted",
    )
    .unwrap();
    assert_eq!(d.status, "failed");
    assert_eq!(d.errors[0].code, "password_required");
}

#[test]
fn native_chart_data_retains_cache_and_reference() {
    let mut u = Unit::new(0, "sheet", "Sheet");
    let source=br#"<chartSpace><chart><plotArea><barChart><ser><idx val="0"/><cat><strRef><f>Sheet1!A1:A2</f><strCache><pt idx="0"><v>Ada</v></pt></strCache></strRef></cat><val><numRef><f>Sheet1!B1:B2</f><numCache><pt idx="0"><v>12.50</v></pt></numCache></numRef></val></ser></barChart></plotArea></chart></chartSpace>"#;
    doc_extractor::charts::extract(source, "xl/charts/chart1.xml", &mut u).unwrap();
    let b = &u.blocks[0];
    assert_eq!(b.attributes["chart_type"], "barChart");
    assert_eq!(
        b.attributes["series"][0]["cached_data"][1]["points"][0]["raw"],
        "12.50"
    );
    assert_eq!(b.attributes["recalculated"], false);
    assert_eq!(b.provenance.method, "native");
}

#[test]
fn pdf_checkpoint_reuses_complete_pages() {
    if !doc_extractor::providers::available("pdfinfo") {
        return;
    }
    // Provider versions compared during checkpoint reuse include env-derived identity.
    let _lock = PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let input = std::path::Path::new("fixtures/digital.pdf");
    let first = extract::extract(
        input,
        "digital.pdf",
        dir.path(),
        Options::default(),
        Limits::default(),
        "resume",
    )
    .unwrap();
    let unit = dir.path().join("units/u1.json");
    let modified = std::fs::metadata(&unit).unwrap().modified().unwrap();
    std::fs::remove_file(dir.path().join("units/u2.json")).unwrap();
    let second = extract::extract(
        input,
        "digital.pdf",
        dir.path(),
        Options::default(),
        Limits::default(),
        "resume",
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(unit).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(second.units.len(), 2);
    assert_eq!(second.assets.len(), first.assets.len());
    assert_eq!(
        second.units[0].blocks[1].text,
        first.units[0].blocks[1].text
    );
}

#[test]
fn markdown_tables_and_links_are_structured() {
    let mut u = Unit::new(0, "section", "Markdown");
    doc_extractor::markup::markdown(&mut u,"# Heading\n\n[Link](https://invalid.example)\n\n| Name | Value |\n|---|---|\n| Ada | 001 |\n");
    let t = u.blocks.iter().find_map(|b| b.table.as_ref()).unwrap();
    assert_eq!((t.rows, t.columns), (2, 2));
    assert_eq!(t.cells[3].raw, "001");
    assert_eq!(t.headers, vec![0]);
    assert!(u
        .blocks
        .iter()
        .any(|b| b.attributes["links"][0]["href"] == "https://invalid.example"));
}
#[test]
fn pdf_batch_images_map_to_correct_pages() {
    if !doc_extractor::providers::available("pdfinfo") {
        return;
    }
    // Pages 2 and 5 carry distinct images; batching attributes `pdfimages` output via `-list`.
    let d = fixture("pages-images.pdf", Options::default());
    let counts: Vec<usize> = d
        .units
        .iter()
        .map(|u| u.blocks.iter().filter(|b| b.kind == "figure").count())
        .collect();
    assert_eq!(counts, vec![1, 1, 0, 0, 1, 0], "{:?}", d.errors);
    assert_eq!(d.assets.len(), 3, "original + two distinct images");
}
#[test]
fn large_csv_is_chunked_into_bounded_units() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.csv");
    let mut file = std::fs::File::create(&path).unwrap();
    use std::io::Write;
    writeln!(file, "id,name,amount").unwrap();
    for i in 0..60_000 {
        writeln!(file, "{i},user{},{}", i % 1000, i % 97).unwrap();
    }
    drop(file);
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let d = extract::extract(
        &path,
        "large.csv",
        &out,
        Options::default(),
        Limits::default(),
        "test",
    )
    .unwrap();
    assert_eq!(d.status, "complete", "{:?}", d.errors);
    assert!(d.units.len() >= 2, "expected chunks, got {}", d.units.len());
    assert_eq!(d.units[0].name, "Records");
    let mut rows = 0usize;
    for u in &d.units {
        let t = u.blocks[0].table.as_ref().unwrap();
        rows += t.rows;
        assert_eq!(t.cells[0].row, 0, "chunk cell rows restart at zero");
        assert!(
            u.blocks[0]
                .provenance
                .cell_range
                .as_deref()
                .is_some_and(|r| r.starts_with("rows ")),
            "{:?}",
            u.blocks[0].provenance.cell_range
        );
        let on_disk: Unit = serde_json::from_slice(
            &std::fs::read(out.join("units").join(format!("{}.json", u.id))).unwrap(),
        )
        .unwrap();
        assert_eq!(
            on_disk.blocks[0].table.as_ref().unwrap().rows,
            u.blocks[0].table.as_ref().unwrap().rows
        );
    }
    assert_eq!(rows, 60_001);
    let result: Document =
        serde_json::from_slice(&std::fs::read(out.join("result.json")).unwrap()).unwrap();
    assert_eq!(result.units.len(), d.units.len());
    assert_eq!(result.metadata["chunks"], serde_json::json!(d.units.len()));
}
