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
fn pdftoppm_available() -> bool {
    // Poppler's pdftoppm exits 1 on `--version` but 0 on `-v`.
    doc_extractor::providers::command("pdftoppm", &["-v"]).is_ok()
}

#[test]
fn page_images_all_render_and_index_every_pdf_page() {
    if !doc_extractor::providers::available("pdftotext") || !pdftoppm_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = extract::extract(
        std::path::Path::new("fixtures/digital.pdf"),
        "digital.pdf",
        dir.path(),
        Options {
            page_images: "all".into(),
            ..Options::default()
        },
        Limits::default(),
        "page-images-all",
    )
    .unwrap();
    assert_eq!(d.units.len(), 2);
    for (index, unit) in d.units.iter().enumerate() {
        let name = format!("pages/u{}.png", index + 1);
        assert!(dir.path().join(&name).is_file(), "{name} written");
        let block = unit.blocks.iter().find(|b| b.kind == "page_image").unwrap();
        assert_eq!(block.attributes["page_image"]["path"], name);
        assert_eq!(block.attributes["page_image"]["reason"], "requested");
        assert!(d
            .assets
            .iter()
            .any(|a| Some(&a.id) == block.asset_id.as_ref() && a.mime == "image/png"));
    }
    let manifest: PageImageManifest =
        serde_json::from_slice(&std::fs::read(dir.path().join("pages/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest.mode, "all");
    assert_eq!(manifest.count, 2);
    assert_eq!(manifest.pages.len(), 2);
    assert_eq!(manifest.pages[0].page, 1);
    assert_eq!(manifest.pages[1].page, 2);
    assert!(manifest
        .pages
        .iter()
        .all(|p| p.dpi == Some(150) && p.bytes > 0 && p.width.unwrap_or(0) > 0));
}

#[test]
fn page_images_clear_stale_files_when_options_change() {
    if !doc_extractor::providers::available("pdftotext") || !pdftoppm_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let first = Options {
        page_images: "all".into(),
        ..Options::default()
    };
    extract::extract(
        std::path::Path::new("fixtures/digital.pdf"),
        "digital.pdf",
        dir.path(),
        first,
        Limits::default(),
        "page-images-stale",
    )
    .unwrap();
    assert!(dir.path().join("pages/u1.png").is_file());
    assert!(dir.path().join("pages/u2.png").is_file());
    // A different page range changes identity, so units are re-extracted and page 1's stale
    // raster must not survive.
    let second = Options {
        page_images: "all".into(),
        pages: Some("2".into()),
        ..Options::default()
    };
    extract::extract(
        std::path::Path::new("fixtures/digital.pdf"),
        "digital.pdf",
        dir.path(),
        second,
        Limits::default(),
        "page-images-stale",
    )
    .unwrap();
    assert!(!dir.path().join("pages/u1.png").exists(), "stale page 1");
    assert!(dir.path().join("pages/u2.png").is_file());
    let manifest: PageImageManifest =
        serde_json::from_slice(&std::fs::read(dir.path().join("pages/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest.count, 1);
    assert_eq!(manifest.pages[0].page, 2);
    assert_eq!(manifest.pages[0].path, "pages/u2.png");
}

#[test]
fn page_images_all_reuse_the_ocr_render() {
    if !doc_extractor::providers::available("pdftotext")
        || !doc_extractor::providers::available("tesseract")
        || !pdftoppm_available()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = extract::extract(
        std::path::Path::new("fixtures/digital.pdf"),
        "digital.pdf",
        dir.path(),
        Options {
            ocr: true,
            page_images: "all".into(),
            // Page 2 is sparse and has no embedded image, so it exercises the OCR render path.
            pages: Some("2".into()),
            ..Options::default()
        },
        Limits {
            ocr_threads: 1,
            ..Limits::default()
        },
        "page-images-ocr",
    )
    .unwrap();
    assert_eq!(d.units.len(), 1);
    let annotated = d.units[0]
        .blocks
        .iter()
        .filter(|b| b.attributes.get("page_image").is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        annotated.len(),
        1,
        "one raster reference, no duplicate block"
    );
    // The OCR render block doubles as the page image.
    assert_eq!(annotated[0].kind, "figure");
    assert!(annotated[0].asset_id.is_some());
    let manifest: PageImageManifest =
        serde_json::from_slice(&std::fs::read(dir.path().join("pages/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest.count, 1);
    assert!(
        d.units.iter().all(|u| u.errors.is_empty()),
        "{:?}",
        d.units
            .iter()
            .flat_map(|u| u.errors.iter())
            .collect::<Vec<_>>()
    );
}

#[test]
fn page_images_survive_pdf_resume() {
    if !doc_extractor::providers::available("pdftotext") || !pdftoppm_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let options = Options {
        page_images: "all".into(),
        ..Options::default()
    };
    extract::extract(
        std::path::Path::new("fixtures/digital.pdf"),
        "digital.pdf",
        dir.path(),
        options.clone(),
        Limits::default(),
        "page-images-resume",
    )
    .unwrap();
    let reused = dir.path().join("units/u1.json");
    let modified = std::fs::metadata(&reused).unwrap().modified().unwrap();
    std::fs::remove_file(dir.path().join("units/u2.json")).unwrap();
    extract::extract(
        std::path::Path::new("fixtures/digital.pdf"),
        "digital.pdf",
        dir.path(),
        options,
        Limits::default(),
        "page-images-resume",
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(&reused).unwrap().modified().unwrap(),
        modified,
        "page 1 unit was reused"
    );
    assert!(dir.path().join("pages/u1.png").is_file());
    assert!(dir.path().join("pages/u2.png").is_file());
    let manifest: PageImageManifest =
        serde_json::from_slice(&std::fs::read(dir.path().join("pages/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest.count, 2, "manifest rebuilds from reused units");
}

#[test]
fn page_images_failed_keeps_unreadable_pdf_pages_only() {
    if !doc_extractor::providers::available("pdftotext") || !pdftoppm_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = extract::extract(
        std::path::Path::new("fixtures/scanned.pdf"),
        "scanned.pdf",
        dir.path(),
        Options {
            page_images: "failed".into(),
            ..Options::default()
        },
        Limits::default(),
        "page-images-failed",
    )
    .unwrap();
    assert_eq!(d.units.len(), 1);
    let manifest: PageImageManifest =
        serde_json::from_slice(&std::fs::read(dir.path().join("pages/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest.mode, "failed");
    assert_eq!(manifest.count, 1, "{manifest:?}");
    assert_eq!(manifest.pages[0].reason, "no_text");
    assert_eq!(manifest.pages[0].path, "pages/u1.png");
    assert_eq!(manifest.pages[0].dpi, Some(150));
    assert!(dir.path().join("pages/u1.png").is_file());
    // The raster is also a regular downloadable asset.
    assert!(d
        .assets
        .iter()
        .any(|a| a.id == manifest.pages[0].asset_id && a.mime == "image/png"));
}

#[test]
fn page_images_failed_reuses_original_image_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let d = extract::extract(
        std::path::Path::new("fixtures/scan.png"),
        "scan.png",
        dir.path(),
        Options {
            page_images: "failed".into(),
            ..Options::default()
        },
        Limits::default(),
        "page-images-image",
    )
    .unwrap();
    let manifest: PageImageManifest =
        serde_json::from_slice(&std::fs::read(dir.path().join("pages/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest.count, 1);
    assert_eq!(manifest.pages[0].reason, "no_text");
    assert_eq!(manifest.pages[0].path, "pages/u1.png");
    assert_eq!(manifest.pages[0].dpi, None);
    assert_eq!(
        d.assets.len(),
        1,
        "page image is a link, not a second asset"
    );
    assert_eq!(
        std::fs::read(dir.path().join("pages/u1.png")).unwrap(),
        std::fs::read("fixtures/scan.png").unwrap()
    );
    // Default mode leaves no pages directory behind.
    let plain = tempfile::tempdir().unwrap();
    extract::extract(
        std::path::Path::new("fixtures/scan.png"),
        "scan.png",
        plain.path(),
        Options::default(),
        Limits::default(),
        "page-images-none",
    )
    .unwrap();
    assert!(!plain.path().join("pages").exists());
    // Formats that cannot produce page rasters ignore the option instead of writing an
    // empty artifact directory.
    let csv = tempfile::tempdir().unwrap();
    extract::extract(
        std::path::Path::new("fixtures/quoted.csv"),
        "quoted.csv",
        csv.path(),
        Options {
            page_images: "all".into(),
            ..Options::default()
        },
        Limits::default(),
        "page-images-csv",
    )
    .unwrap();
    assert!(!csv.path().join("pages").exists());
}

#[test]
fn page_images_option_is_validated() {
    use doc_extractor::service::validate_options;
    for mode in ["none", "failed", "all"] {
        let options = Options {
            page_images: mode.into(),
            ..Options::default()
        };
        assert!(validate_options(&options).is_ok(), "{mode} accepted");
    }
    let bad = Options {
        page_images: "sometimes".into(),
        ..Options::default()
    };
    assert!(validate_options(&bad).is_err(), "unknown mode rejected");
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
#[test]
fn compact_tables_omit_redundant_cell_fields() {
    let dir = tempfile::tempdir().unwrap();
    let options = Options {
        compact_tables: true,
        ..Default::default()
    };
    let d = extract::extract(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/quoted.csv"),
        "quoted.csv",
        dir.path(),
        options,
        Limits::default(),
        "compact",
    )
    .unwrap();
    assert_eq!(d.status, "complete");
    let raw = std::fs::read_to_string(dir.path().join("result.json")).unwrap();
    assert!(
        !raw.contains("\"value\""),
        "compact cells must not repeat values equal to raw"
    );
    assert!(!raw.contains("\"formula\""));
    assert!(!raw.contains("\"cached_value\""));
    assert!(!raw.contains("\"row_span\""));
    // Round-trips to the full in-memory shape: a missing value means "the raw string".
    let round: Document = serde_json::from_str(&raw).unwrap();
    let cells = &round.units[0].blocks[0].table.as_ref().unwrap().cells;
    let cell = cells.iter().find(|c| c.raw == "0012").unwrap();
    assert_eq!(cell.value, serde_json::json!("0012"));
}
#[test]
fn pdf_parallel_and_sequential_batches_match() {
    if !doc_extractor::providers::available("pdfinfo") {
        return;
    }
    let _lock = PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let input = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/long.pdf");
    let run = |workers: usize, name: &str| -> (serde_json::Value, Document) {
        let dir = tempfile::tempdir().unwrap();
        let limits = Limits {
            pdf_workers: workers,
            ..Default::default()
        };
        let d = extract::extract(
            &input,
            "long.pdf",
            dir.path(),
            Options::default(),
            limits,
            name,
        )
        .unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("result.json")).unwrap())
                .unwrap();
        value["elapsed_ms"] = serde_json::json!(0);
        value["id"] = serde_json::json!("test");
        // Wall and per-page timings depend on scheduling; compare the deterministic result.
        value["metadata"]["timing"] = serde_json::Value::Null;
        if let Some(units) = value["units"].as_array_mut() {
            for unit in units {
                if let Some(object) = unit.as_object_mut() {
                    object.remove("timing");
                }
            }
        }
        (value, d)
    };
    let (sequential, doc_a) = run(1, "seq");
    let (parallel, doc_b) = run(4, "par");
    assert!(
        doc_a.units.len() > 100,
        "fixture must span multiple batches"
    );
    assert_eq!(doc_b.units.len(), doc_a.units.len());
    assert_eq!(
        sequential,
        parallel,
        "parallel batches differ: {:?}",
        first_json_diff(&sequential, &parallel, "$".to_string())
    );
}
fn first_json_diff(a: &serde_json::Value, b: &serde_json::Value, path: String) -> Option<String> {
    match (a, b) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            for (key, value) in left {
                let child = format!("{path}.{key}");
                match right.get(key) {
                    None => return Some(format!("{child}: missing on right")),
                    Some(other) => {
                        if let Some(diff) = first_json_diff(value, other, child) {
                            return Some(diff);
                        }
                    }
                }
            }
            None
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!("{path}: array {} vs {}", left.len(), right.len()));
            }
            for (index, (value, other)) in left.iter().zip(right).enumerate() {
                if let Some(diff) = first_json_diff(value, other, format!("{path}[{index}]")) {
                    return Some(diff);
                }
            }
            None
        }
        _ => (a != b).then(|| {
            let left = a.to_string();
            let right = b.to_string();
            format!(
                "{path}: {} vs {}",
                &left[..left.len().min(200)],
                &right[..right.len().min(200)]
            )
        }),
    }
}

#[test]
fn checkpoints_are_versioned_ordered_and_stream_result_matches() {
    if !doc_extractor::providers::available("pdfinfo") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/long.pdf");
    let doc = extract::extract(
        &input,
        "long.pdf",
        dir.path(),
        Options::default(),
        Limits::default(),
        "checkpoint",
    )
    .unwrap();
    let identity: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("identity.json")).unwrap()).unwrap();
    assert_eq!(identity["checkpoint_format"], serde_json::json!(2));
    // checkpoint.json is a complete, page-ordered snapshot of unit state (stubs, no blocks).
    let checkpoint: Document =
        serde_json::from_slice(&std::fs::read(dir.path().join("checkpoint.json")).unwrap())
            .unwrap();
    assert_eq!(checkpoint.units.len(), doc.units.len());
    let indices: Vec<usize> = checkpoint.units.iter().map(|u| u.index).collect();
    assert!(indices.windows(2).all(|w| w[0] < w[1]), "{indices:?}");
    // result.json was assembled by streaming unit files and must equal the in-memory units.
    let result: Document =
        serde_json::from_slice(&std::fs::read(dir.path().join("result.json")).unwrap()).unwrap();
    assert_eq!(result.units.len(), doc.units.len());
    assert_eq!(
        serde_json::to_value(&result.units[7].blocks).unwrap(),
        serde_json::to_value(&doc.units[7].blocks).unwrap()
    );
    // Stage timings artifact exists and records the Poppler phases.
    let timings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("timings.json")).unwrap()).unwrap();
    assert!(timings["poppler_text_ms"].is_u64());
    assert!(timings["unit_write_ms"].is_u64());
    // No temporary files survive a clean run.
    let leftovers = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn streamed_tables_release_blocks_but_keep_full_output() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        stream_bytes: 1,
        ..Default::default()
    };
    let d = extract::extract(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/quoted.csv"),
        "quoted.csv",
        dir.path(),
        Options::default(),
        limits,
        "stream",
    )
    .unwrap();
    // The returned document keeps stubs; payload lives in unit files and derived outputs.
    assert!(
        d.units.iter().all(|u| u.blocks.is_empty()),
        "blocks released"
    );
    let unit: Unit =
        serde_json::from_slice(&std::fs::read(dir.path().join("units/u1.json")).unwrap()).unwrap();
    assert!(!unit.blocks.is_empty(), "unit file keeps payload");
    let result: Document =
        serde_json::from_slice(&std::fs::read(dir.path().join("result.json")).unwrap()).unwrap();
    assert_eq!(
        result.units[0].blocks[0].table.as_ref().unwrap().cells[5].raw,
        "0012"
    );
    let markdown = std::fs::read_to_string(dir.path().join("result.md")).unwrap();
    let plaintext = std::fs::read_to_string(dir.path().join("result.txt")).unwrap();
    assert!(markdown.contains("0012"), "streamed markdown keeps cells");
    assert!(plaintext.contains("0012"), "streamed plaintext keeps cells");
}

#[test]
fn sparse_text_pages_render_for_ocr_without_duplicating_native_text() {
    if !doc_extractor::providers::available("tesseract")
        || !doc_extractor::providers::available("pdftoppm")
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let options = Options {
        ocr: true,
        ..Default::default()
    };
    let limits = Limits {
        ocr_threads: 1,
        ..Default::default()
    };
    let d = extract::extract(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/digital.pdf"),
        "digital.pdf",
        dir.path(),
        options,
        limits,
        "sparse",
    )
    .unwrap();
    assert_eq!(d.units.len(), 2);
    // Pages with almost no native text (15 chars, no images) are rendered and OCR'd; the native
    // lines must survive exactly once and the render/OCR must not corrupt or fail the page.
    for (unit, want) in d.units.iter().zip(["Native page ONE", "Native page TWO"]) {
        let exact = unit
            .blocks
            .iter()
            .filter(|b| b.kind == "text" && b.text.trim() == want)
            .count();
        assert_eq!(exact, 1, "page {} keeps its native line once", unit.name);
        assert!(unit.errors.is_empty(), "{:?}", unit.errors);
        assert!(
            unit.blocks.iter().any(|b| b.kind == "figure"),
            "render asset attached on {}",
            unit.name
        );
    }
    // Every page-render temp file was cleaned up.
    let leftovers = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("render-"))
        .count();
    assert_eq!(leftovers, 0);
}
#[test]
fn result_format_option_is_validated() {
    use doc_extractor::service::validate_options;
    for format in ["json", "markdown", "text"] {
        let options = Options {
            result_format: format.into(),
            ..Options::default()
        };
        assert!(validate_options(&options).is_ok(), "{format} accepted");
    }
    let bad = Options {
        result_format: "pdf".into(),
        ..Options::default()
    };
    assert!(validate_options(&bad).is_err(), "unknown format rejected");
}
#[test]
fn page_range_specs_parse() {
    use doc_extractor::model::parse_page_range;
    assert_eq!(parse_page_range("1-3,5").unwrap(), vec![1, 2, 3, 5]);
    assert_eq!(parse_page_range(" 7 ").unwrap(), vec![7]);
    assert_eq!(parse_page_range("3,1-2,2").unwrap(), vec![1, 2, 3]);
    assert_eq!(parse_page_range("10-").unwrap()[0], 10);
    for bad in ["", "0", "5-2", "a", "1,,2", "-3"] {
        assert!(parse_page_range(bad).is_err(), "{bad:?} rejected");
    }
}
#[test]
fn pdf_page_range_selects_only_requested_pages() {
    if !doc_extractor::providers::available("pdftotext") {
        return;
    }
    let d = fixture(
        "digital.pdf",
        Options {
            pages: Some("2".into()),
            ..Options::default()
        },
    );
    assert_eq!(
        d.units.len(),
        1,
        "{:?}",
        d.units.iter().map(|u| &u.name).collect::<Vec<_>>()
    );
    assert_eq!(d.units[0].name, "Page 2");
    assert!(text(&d).contains("Native page TWO"));
    assert!(!text(&d).contains("Native page ONE"));
    assert_eq!(d.metadata["pages_total"], 2);
    assert_eq!(d.metadata["page_range"], "2");
}
#[test]
fn chunk_artifact_windows_plain_text() {
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("document.md");
    let dir = tempfile::tempdir().unwrap();
    let doc = extract::extract(
        &input,
        "document.md",
        dir.path(),
        Options {
            chunk_chars: Some(40),
            chunk_overlap: Some(10),
            ..Options::default()
        },
        Limits::default(),
        "test",
    )
    .unwrap();
    let raw = std::fs::read_to_string(dir.path().join("result.chunks.jsonl")).unwrap();
    let chunks = raw
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(chunks.len() >= 2, "{chunks:?}");
    assert!(chunks.iter().all(|c| c["chars"].as_u64().unwrap() <= 40
        && c["unit"] == doc.units[0].id
        && !c["text"].as_str().unwrap().is_empty()));
    // Chunking is opt-in: without chunk_chars no artifact is written.
    let plain = tempfile::tempdir().unwrap();
    extract::extract(
        &input,
        "document.md",
        plain.path(),
        Options::default(),
        Limits::default(),
        "test",
    )
    .unwrap();
    assert!(!plain.path().join("result.chunks.jsonl").exists());
}

#[test]
fn chunks_are_utf8_boundary_safe_for_hindi_text() {
    let mut unit = Unit::new(0, "text", "Hindi OCR");
    let hindi = "प्रमाणपत्र-सह-अंकपत्र यह प्रमाणित किया जाता है कि परिषद के अभिलेखानुसार ".repeat(6);
    unit.text("text", &hindi, "test");
    let mut out = String::new();
    unit_chunks(&unit, 41, 9, &mut out).unwrap();
    let chunks = out
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(chunks.len() >= 2, "{chunks:?}");
    assert!(chunks.iter().all(|chunk| {
        chunk["text"].as_str().is_some_and(|text| !text.is_empty())
            && chunk["chars"].as_u64().unwrap_or(0) > 0
    }));
}

#[test]
fn wasm_engine_derives_formats_chunks_and_hit_tests_bounds() {
    use doc_extractor::wasm;
    let ping = wasm::invoke_op("ping", "{}").unwrap();
    assert_eq!(ping["ok"], true);
    assert_eq!(ping["engine"], "doc-extractor-wasm");

    let valid = wasm::invoke_op(
        "validate_options",
        r#"{"options":{"page_images":"failed","pages":"1-3, 7"}}"#,
    )
    .unwrap();
    assert_eq!(valid["valid"], true);
    assert_eq!(valid["selected_pages_count"], 4);

    let invalid = wasm::invoke_op(
        "validate_options",
        r#"{"options":{"page_images":"bogus","pages":"8-2"}}"#,
    )
    .unwrap();
    assert_eq!(invalid["valid"], false);
    assert_eq!(invalid["errors"].as_array().unwrap().len(), 2);

    // Build a unit with positioned blocks and test spatial SVG + hit-testing + chunking.
    let mut u = Unit::new(0, "page", "Page 1");
    {
        let b1 = u.text(
            "text",
            "Header block on page one with enough text for RAG chunking",
            "pdftotext",
        );
        b1.provenance.bounds = Some(Bounds {
            x: 40.0,
            y: 50.0,
            width: 300.0,
            height: 24.0,
            coordinate_space: "pdf_points_top_left".into(),
        });
    }
    {
        let b2 = u.text("text", "Invoice TOTAL 12345", "tesseract");
        b2.provenance.method = "ocr".into();
        b2.provenance.bounds = Some(Bounds {
            x: 60.0,
            y: 120.0,
            width: 140.0,
            height: 18.0,
            coordinate_space: "pdf_points_top_left".into(),
        });
    }

    let derived = wasm::invoke_op(
        "derive_unit",
        &serde_json::to_string(&serde_json::json!({
            "unit": u,
            "chunk_chars": 28,
            "chunk_overlap": 6,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(derived["markdown"]
        .as_str()
        .unwrap()
        .contains("Invoice TOTAL 12345"));
    assert!(derived["page_svg"].as_str().unwrap().contains("<svg"));
    assert!(derived["chunks"].as_array().unwrap().len() >= 2);
    assert_eq!(derived["stats"]["native"], 1);
    assert_eq!(derived["stats"]["ocr"], 1);

    let hit = wasm::invoke_op(
        "hit_test_page",
        &serde_json::to_string(&serde_json::json!({
            "unit": u,
            "x": 80.0,
            "y": 128.0,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(hit["hit"], "u1-b2");
}

#[test]
fn wasm_playground_parses_diagrams_layout_and_packages_llm_payloads() {
    use doc_extractor::wasm;
    let dbml = wasm::invoke_op(
        "playground_extract",
        &serde_json::to_string(&serde_json::json!({
            "format": "dbml",
            "source": "Table users {\n  id uuid [pk]\n}\nTable orders {\n  id uuid [pk]\n  user_id uuid [ref: > users.id]\n}"
        }))
        .unwrap(),
    )
    .unwrap();
    let diagrams = dbml["diagrams"].as_array().unwrap();
    assert_eq!(diagrams.len(), 1);
    assert_eq!(diagrams[0]["nodes"], 2);
    assert_eq!(diagrams[0]["edges"], 1);
    assert!(diagrams[0]["svg"].as_str().unwrap().contains("<svg"));

    let layout_res = wasm::invoke_op(
        "playground_extract",
        &serde_json::to_string(&serde_json::json!({
            "format": "pdf_layout",
            "source": "Name, 40, 50, 40, 12\nAmount, 180, 50, 50, 12\nAda, 40, 72, 35, 12\n12, 180, 72, 24, 12\nLin, 40, 94, 35, 12\n34, 180, 94, 24, 12"
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(layout_res["stats"]["tables"], 1);
    assert!(layout_res["markdown"].as_str().unwrap().contains("Ada"));

    let d = fixture(
        "scan.png",
        Options {
            page_images: "failed".into(),
            ..Options::default()
        },
    );
    let inspected = wasm::invoke_op(
        "inspect_document",
        &serde_json::to_string(&serde_json::json!({ "document": d })).unwrap(),
    )
    .unwrap();
    assert_eq!(inspected["page_images"]["count"], 1);

    let llm = wasm::invoke_op(
        "build_llm_payload",
        &serde_json::to_string(&serde_json::json!({
            "document": d,
            "base_url": "http://127.0.0.1:8787"
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(llm["count"], 1);
    assert_eq!(llm["items"][0]["reason"], "no_text");
    assert!(llm["items"][0]["openai_vision_request"]["messages"].is_array());
}

#[test]
fn oversized_ocr_images_are_downscaled_with_box_mapping() {
    if !doc_extractor::providers::available("tesseract") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scan.png");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/scan.png"),
        &path,
    )
    .unwrap();
    let options = Options {
        ocr: true,
        ..Default::default()
    };
    // scan.png is 1200x360 (432k pixels); 300k forces a downscale for OCR.
    let limits = Limits {
        page_pixels: 300_000,
        ocr_threads: 1,
        ..Default::default()
    };
    let out = doc_extractor::providers::ocr_blocks(&path, &options, &limits).unwrap();
    assert!(
        out.warnings.iter().any(|w| w.contains("downscaled")),
        "expected a downscale warning, got {:?}",
        out.warnings
    );
    assert!(!out.blocks.is_empty());
    let joined = out
        .blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.to_lowercase().contains("invoice") || joined.contains("12345"),
        "OCR text after downscale: {joined:?}"
    );
    for block in &out.blocks {
        if let Some(bounds) = &block.provenance.bounds {
            assert!(
                bounds.x <= 1200.0 && bounds.y <= 360.0,
                "bounds must be mapped back to the original image: {bounds:?}"
            );
        }
    }
}

#[test]
fn tesseract_words_are_grouped_into_readable_lines() {
    if !doc_extractor::providers::available("tesseract") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scan.png");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/scan.png"),
        &path,
    )
    .unwrap();
    let out = doc_extractor::providers::ocr_blocks(
        &path,
        &Options {
            ocr: true,
            ..Default::default()
        },
        &Limits {
            ocr_threads: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        out.blocks
            .iter()
            .any(|block| block.text.split_whitespace().count() >= 2),
        "OCR output should contain grouped lines, got {:?}",
        out.blocks.iter().map(|b| &b.text).collect::<Vec<_>>()
    );
    assert!(
        out.blocks.iter().all(|block| {
            block
                .attributes
                .get("words")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                >= 1
        }),
        "each OCR block should record its word count"
    );
}

#[test]
fn scanned_table_pages_promote_printed_grids_to_structured_tables() {
    if !doc_extractor::providers::available("tesseract") || !pdftoppm_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = extract::extract(
        std::path::Path::new("fixtures/final-comprehensive.pdf"),
        "final-comprehensive.pdf",
        dir.path(),
        Options {
            ocr: true,
            languages: "eng+hin".into(),
            pages: Some("1".into()),
            ..Options::default()
        },
        Limits {
            ocr_threads: 1,
            ..Default::default()
        },
        "ocr-table",
    )
    .unwrap();
    let unit = &d.units[0];
    let timing = unit.timing.as_ref().expect("page timing");
    assert_eq!(timing.method, "ocr");
    assert!(timing.ocr_ms > 0);
    let table = unit
        .blocks
        .iter()
        .find_map(|block| block.table.as_ref())
        .expect("printed grid must be promoted to a table");
    assert!(
        table.rows >= 8 && table.columns >= 5,
        "marks table grid: {table:?}"
    );
    let cells = table
        .cells
        .iter()
        .map(|cell| cell.raw.as_str())
        .collect::<Vec<_>>();
    assert!(
        cells.iter().any(|raw| raw.contains("100")),
        "expected maximum marks column: {cells:?}"
    );
    assert!(
        cells.iter().any(|raw| raw.contains("B2")),
        "expected grade column: {cells:?}"
    );
    // Merged header and merged total row keep their span.
    assert!(
        table.cells.iter().any(|cell| cell.column_span > 1),
        "merged cells keep their span: {cells:?}"
    );
    assert!(
        unit.blocks
            .iter()
            .any(|block| block.provenance.method == "inferred" && block.table.is_some()),
        "table proposals stay inferred"
    );
}

#[test]
fn pdf_units_report_per_page_timing() {
    let d = fixture("digital.pdf", Options::default());
    assert!(!d.units.is_empty());
    for unit in &d.units {
        let timing = unit.timing.as_ref().expect("page timing");
        assert!(timing.total_ms >= timing.native_ms);
        assert!(timing.chars > 0);
        assert!(matches!(
            timing.method.as_str(),
            "native" | "ocr" | "hybrid" | "none"
        ));
    }
    let summary = &d.metadata["timing"];
    assert_eq!(summary["pages"], d.units.len());
    assert!(summary["native_pages"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(
        summary["per_page"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        d.units.len()
    );
}

#[test]
fn refinement_recovers_formulas_stamps_and_cardinality() {
    if !doc_extractor::providers::available("tesseract") || !pdftoppm_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = extract::extract(
        std::path::Path::new("fixtures/final-comprehensive.pdf"),
        "final-comprehensive.pdf",
        dir.path(),
        Options {
            ocr: true,
            languages: "eng+hin".into(),
            ocr_auto_rotate: true,
            pages: Some("3,5,6".into()),
            ..Options::default()
        },
        Limits {
            ocr_threads: 1,
            ..Default::default()
        },
        "ocr-refine",
    )
    .unwrap();
    let text = d
        .units
        .iter()
        .flat_map(|unit| unit.blocks.iter())
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    // Printed math glyphs are recovered from shapes, not from a symbol-capable text model.
    assert!(text.contains("E = mc²"), "superscript formula: {text}");
    assert!(
        text.contains("क्षेत्रफल = πr²"),
        "pi glyph and superscript: {text}"
    );
    assert!(text.contains("H2O + CO2"), "chemical glyphs: {text}");
    assert!(
        text.contains("1/2 + 1/3 = 5/6"),
        "line-level formula re-read: {text}"
    );
    assert!(
        text.contains("1..*"),
        "diagram cardinality from a binarized crop: {text}"
    );
    // Seal text is recovered from text bands inside the ring (or by the primary pass on a
    // lucky segmentation); either way it must be OCR evidence, not native content.
    let stamp = d
        .units
        .iter()
        .flat_map(|unit| unit.blocks.iter())
        .find(|block| block.text.to_uppercase().contains("INSPECTED"))
        .expect("seal text recovered");
    assert_eq!(stamp.provenance.method, "ocr");
    // Refined text stays OCR evidence, never native content.
    assert!(d
        .units
        .iter()
        .flat_map(|unit| unit.blocks.iter())
        .filter(|block| block.attributes.get("ocr_refined").is_some())
        .all(|block| block.provenance.method == "ocr"));
}

#[test]
fn absent_smoldocling_server_falls_back_to_tesseract() {
    if !doc_extractor::providers::available("tesseract") {
        return;
    }
    let _lock = PROCESS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    struct RestoreEnv {
        smoldocling: Option<String>,
        vlm: Option<String>,
    }
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            for (key, value) in [
                ("DOC_SMOLDOCLING_URL", &self.smoldocling),
                ("DOC_VLM_URL", &self.vlm),
            ] {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
    let _restore = RestoreEnv {
        smoldocling: std::env::var("DOC_SMOLDOCLING_URL").ok(),
        vlm: std::env::var("DOC_VLM_URL").ok(),
    };
    std::env::remove_var("DOC_SMOLDOCLING_URL");
    std::env::remove_var("DOC_VLM_URL");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scan.png");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/scan.png"),
        &path,
    )
    .unwrap();
    let options = Options {
        ocr: true,
        ocr_provider: "smoldocling".into(),
        ..Default::default()
    };
    let out = doc_extractor::providers::ocr_blocks(&path, &options, &Limits::default()).unwrap();
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("SmolDocling-256M unavailable")),
        "expected SmolDocling fallback warning, got {:?}",
        out.warnings
    );
    assert!(
        !out.blocks.is_empty(),
        "Tesseract fallback returned no text"
    );
}

#[test]
fn wasm_ffi_memory_abi_roundtrips() {
    use doc_extractor::wasm::*;
    let op = b"ping";
    let arg = b"{}";
    unsafe {
        let op_ptr = wasm_alloc(op.len());
        let arg_ptr = wasm_alloc(arg.len());
        std::ptr::copy_nonoverlapping(op.as_ptr(), op_ptr, op.len());
        std::ptr::copy_nonoverlapping(arg.as_ptr(), arg_ptr, arg.len());
        let ok = wasm_invoke(op_ptr, op.len(), arg_ptr, arg.len());
        assert_eq!(ok, 1);
        let out_slice = std::slice::from_raw_parts(wasm_output_ptr(), wasm_output_len());
        let val: serde_json::Value = serde_json::from_slice(out_slice).unwrap();
        assert_eq!(val["ok"], true);
        wasm_dealloc(op_ptr, op.len());
        wasm_dealloc(arg_ptr, arg.len());
    }
}

#[test]
fn smoldocling_doctags_parses_bounds_tables_and_merges_in_pure_rust() {
    let sample = r#"<doctag>
        <section_header_level_1><loc_50><loc_25><loc_350><loc_60>Invoice #INV-2026</section_header_level_1>
        <text><loc_50><loc_80><loc_450><loc_110>Customer: Ada Lovelace</text>
        <otsl><loc_50><loc_130><loc_450><loc_280>
            <ched>Item<ched>Qty<ched>Amount<nl>
            <fcel>Rust OCR<fcel>2<fcel>1200.50<nl>
            <fcel>Total<lcel><fcel>1200.50<nl>
        </otsl>
    </doctag>"#;

    let mut unit = Unit::new(0, "page", "Page 1");
    doc_extractor::doctags::extract(sample, &mut unit, 1000, 1500, "ocr").unwrap();
    assert_eq!(unit.blocks.len(), 3);
    assert_eq!(unit.blocks[0].kind, "heading");
    assert_eq!(unit.blocks[0].text, "Invoice #INV-2026");
    let b0_bounds = unit.blocks[0].provenance.bounds.as_ref().unwrap();
    assert_eq!(b0_bounds.coordinate_space, "image_pixels");
    assert!((b0_bounds.x - 100.0).abs() < 0.01);
    assert!((b0_bounds.y - 75.0).abs() < 0.01);

    let table = unit.blocks[2].table.as_ref().unwrap();
    assert_eq!(table.rows, 3);
    assert_eq!(table.columns, 3);
    assert_eq!(table.headers, vec![0]);
    let total_label = table
        .cells
        .iter()
        .find(|c| c.row == 2 && c.column == 0)
        .unwrap();
    assert_eq!(total_label.raw, "Total");
    assert_eq!(total_label.column_span, 2);
    let total_val = table
        .cells
        .iter()
        .find(|c| c.row == 2 && c.column == 2)
        .unwrap();
    assert_eq!(total_val.value, serde_json::json!(1200.5));

    // Verify WASM playground_extract also handles doctags end-to-end.
    let wasm_out = doc_extractor::wasm::invoke_op(
        "playground_extract",
        &serde_json::to_string(&serde_json::json!({
            "format": "doctags",
            "source": sample,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(wasm_out["stats"]["tables"], 1);
    assert_eq!(wasm_out["stats"]["bounds_blocks"], 3);
    assert!(wasm_out["markdown"]
        .as_str()
        .unwrap()
        .contains("Invoice #INV-2026"));
}
