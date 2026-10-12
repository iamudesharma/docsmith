# Tested capability matrix

Status is about actual extraction coverage, not advertised file extensions. All tests here use small synthetic fixtures unless explicitly stated. Baseline runs with OCR and VLM disabled.

| Format | Implemented extraction | Evidence / limitations |
|---|---|---|
| PDF | Actual pages, native lines/bounds, embedded images, original vector source, metadata/attachments, optional page/image OCR | Poppler runs once per 100-page range for text and images; pages are checkpointed individually and reused on restart. Digital, scanned, mixed, hidden OCR, columns, table, corrupt/encrypted/source-embedded fixtures. Table grids and diagram label groups are inferred; complex reading order, exact image placement and connector topology remain partial. |
| CSV / TSV | Streaming records, quoting, multiline cells, configurable delimiter, flexible rows, original strings, bounded chunk units for large files | Quoted/leading-zero/multiline/flexible fixtures plus a generated 60k-row chunking test; a 700k-row local run completes in seconds. Chunk units carry `rows N-M` provenance. UTF-8 only, no guessed header or silent type conversion. Result JSON is roughly 12x a tiny-cell input; multi-million-row files need raised limits and a compact mode is future work. |
| XLSX | Sheets, cell origins, typed values, formulas separately from cached values, source merge spans, package assets/chart cache | Fixture compares A3 formula/cached result and A1:B1 merge. No recalculation, formatting equivalence or chart rendering. |
| ODS | Calamine sheets, values, formulas where exposed | Single typed-cell fixture; complex merged/repeated cells are not fully validated. |
| XLS | BIFF8 sheet, typed values, formulas with cached results | Hand-built minimal BIFF8 fixture (A1 string, B2 number, A3 formula + cached 5.0). No formatting/date-format coverage or large workbook stress. |
| XLSM | Same workbook plus macro-enabled package | Macro fixture asserts VBA source is never extracted or executed. No macro-workbook corpus validation. |
| XLSB | Calamine adapter compiled | No representative binary fixture acceptance. Macros are never executed. |
| DOCX | Logical sections, paragraphs, headings, lists, tables/spans, hyperlink IDs, notes/auxiliary text, media, relationship/SmartArt/chart XML | Section/table/media fixture. Physical pages/rendered drawing placement, floating-object order and exhaustive nested tables remain partial. |
| PPTX | Presentation relationship order, slides, paragraphs, table source, notes, media/source geometry XML | Reordered-slide fixture. Complex animations/groups/rendered layout remain partial. |
| ODT / ODP | Logical document/slide units, text and source tables/media | ODT text fixture; ODP compiled but not fixture validated. Repeated cells/rendered layout partial. |
| RTF | macOS textutil text; portable explicit text subset; original retained | Text fixture; formatting/tables/assets remain partial. Linux path not locally validated. |
| HTML | Headings, paragraphs, lists, links/image references, table cells/spans | Table/link fixture. No remote fetching, JS/CSS execution or rendered geometry. |
| Markdown | Headings/text/lists/tables/links/image references/footnotes/code events; original source retained | Text/list/table/link fixtures. Unsupported rich constructs stay recoverable in original source; exhaustive structural parity is not claimed. |
| JSON / XML | Validated structured values/raw XML, logical text | Unicode fixture, DTD/entity rejection; domain-specific XML semantics not inferred. |
| EML | Headers, MIME plain/HTML text and attachment bytes | Multipart fixture; nesting bounded to 16; MSG requires optional broad adapter/fixtures. |
| EPUB | Container/manifest/spine source order, HTML chapters, media | Chapter fixture; no rendered pagination/DRM support. |
| Images | Original bytes/dimensions; optional OCR/vision | Clean/noisy/rotated English and Hindi synthetic fixtures with zero Tesseract CER in the 12 October rerun. Apple Vision currently fails every rerun with `e5rt` errors on this host; handwriting/real scanner corpus unrun. |
| draw.io | Compressed/uncompressed pages, nodes/edges, explicit IDs, labels/groups/geometry/style/source | Two-node/one-edge source accuracy; PNG/SVG/PDF embedded-source checks. No invented keys/cardinalities. |
| Mermaid | Flowchart identifier/nodes/simple edges; ER entities/attributes/PK/FK/UK/crow's-foot subset | Source graph/ER/unsupported-construct tests. Styling, directives, aliases and broader grammars return partial warnings. |
| DOT | Basic identifier nodes, quoted label attribute, `->` / `--` edges | Simple source fixture. Ports, HTML labels, subgraphs and complex attributes unsupported with warnings. |
| PlantUML | Basic edges and entity/typed attribute/crow's-foot subset | Source fixture. Includes/macros/theme directives are never executed; broad syntax unsupported. |
| DBML | Table fields/explicit keys, inline references and `Ref: field > field` subset | Source fixture. Complex projects/indexes/enums/quoting unsupported with warnings. |
| SVG | Original source, texts, paths/shape attributes, transforms, embedded draw.io | Geometry/source fixture. Arbitrary paths do not establish graph semantics. |
| VSDX / SmartArt | Original/source XML/relationship/assets retained | VSDX source fixture; semantic graph relationships and legacy VSD deferred. |
| OOXML charts | Native cached points, series/axis/formula references, original XML | Cached-point/reference unit test. No recalculation/rendering or exact raster-chart numeric recovery. |
| Xberg 1.3.8 | Optional native-only process adapter and full upstream JSON asset | Adapter verified end-to-end on the evaluated CLI in docs/xberg-adapter-check.json (the earlier version gate failed on ANSI-colored output). Canonical mapping is partial; native baseline does not require Xberg. |
| Local vision | Literal-loopback llama.cpp descriptions / constrained inferred graph JSON | Actual small-model requests measured; graph fidelity remains a benchmark gate, not guaranteed semantics. |
| Legacy DOC / PPT / VSD | Explicit unsupported in direct adapter | No claim of complete binary-format support; optional Xberg requires format-specific corpus validation. |

`GET /v1/capabilities` reports installed native/OCR languages and configured optional engines. A provisioned language/model is distinct from tested accuracy. Document statuses are `complete`, `partial`, `needs_ocr`, `unsupported` and `failed`; successful units and structured failures survive incomplete extraction.

Unrun gates: real customer scanned/handwritten documents, long-soak and adversarial corpus, complex PDF topology, full legacy/Visio/SmartArt syntax, PaddleOCR/ONNX pipeline, stronger VLM comparison, M1 Pro 16 GB benchmark, container runtime and backup restoration. Linux execution is provided by the GitHub Actions workflow (`.github/workflows/benchmark.yml`, manual or weekly); local macOS runs cannot substitute for its results.
