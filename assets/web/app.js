(function () {
  "use strict";

  // ==================== STATE ====================
  var state = {
    wasm: null,
    wasmReady: false,
    token: localStorage.getItem("docsmith-token") || "",
    activeTab: "studio",
    activeSubtab: "blocks",
    activeFormat: "markdown",
    activePgOut: "visual",
    provFilter: "all",
    activeBlockId: "",
    selectedFiles: [],
    jobs: [],
    currentJobId: "",
    currentJobMeta: null,
    currentDoc: null,
    currentDocSummary: null,
    currentUnitId: "u1",
    currentUnit: null,
    currentDerived: null,
    currentPageRasterAssetId: null,
    currentRasterDims: { width: null, height: null, dpi: 150 },
    pgResult: null,
  };

  var TAB_HEADERS = {
    studio: {
      title: "Documents",
      subtitle:
        "Upload PDF, Office, spreadsheet, or image files to extract clean text, tables, and diagrams.",
    },
    inspector: {
      title: "Document Viewer",
      subtitle:
        "Read extracted text and tables side-by-side with the page preview. Click any box on the page to find its text.",
    },
    pages: {
      title: "Saved Page Images (For AI)",
      subtitle:
        "Pages that were blank, scanned, or diagram-heavy are saved here as clear images ready for AI reading.",
    },
    playground: {
      title: "Instant Try-Out",
      subtitle:
        "Test how tables, diagrams, and Markdown are read right inside your browser — no file upload needed.",
    },
    system: {
      title: "Speed & System Status",
      subtitle:
        "Verified accuracy across 20 concurrent PDFs (316 pages) and a live check of installed document tools.",
    },
  };

  var FRIENDLY_METHOD = {
    native: "Direct Text (from file)",
    ocr: "Scanned Text (OCR)",
    inferred: "Auto-Detected Layout",
  };

  var FRIENDLY_REASON = {
    no_text: "No selectable text on page",
    sparse_text: "Very little text on page",
    ocr_empty: "OCR found no text",
    extraction_error: "Page had a read error",
    requested: "Saved by your setting",
  };

  var encoder = new TextEncoder();
  var decoder = new TextDecoder("utf-8");

  // ==================== PLAYGROUND PRESETS ====================
  var PRESETS = {
    doctags: {
      format: "doctags",
      source: [
        "<doctag>",
        "  <section_header_level_1><loc_40><loc_35><loc_360><loc_68>Invoice #INV-2026-04</section_header_level_1>",
        "  <text><loc_40><loc_82><loc_450><loc_112>Customer: Ada Lovelace · Payment Terms: Net 30</text>",
        "  <otsl><loc_40><loc_135><loc_460><loc_285>",
        "    <ched>Description<ched>Qty<ched>Amount<nl>",
        "    <fcel>Rust Document Extraction Engine<fcel>1<fcel>1200.00<nl>",
        "    <fcel>SmolDocling-256M Pure-Rust DocTags<fcel>1<fcel>450.00<nl>",
        "    <fcel>Total Due (USD)<lcel><fcel>1650.00<nl>",
        "  </otsl>",
        "  <footnote><loc_40><loc_310><loc_420><loc_335>Thank you for your business. Paid in full.</footnote>",
        "</doctag>"
      ].join("\n"),
    },
    mermaid_er: {
      format: "mermaid",
      source: [
        "erDiagram",
        "    USERS ||--o{ ORDERS : places",
        "    ORDERS ||--|{ LINE_ITEMS : contains",
        "    PRODUCTS ||--o{ LINE_ITEMS : fulfills",
        "    USERS {",
        "        uuid id PK \"Primary user ID\"",
        "        varchar email UK \"Verified email\"",
        "        timestamp created_at",
        "    }",
        "    ORDERS {",
        "        uuid id PK",
        "        uuid user_id FK",
        "        numeric total_cents",
        "        varchar status",
        "    }",
        "    LINE_ITEMS {",
        "        uuid id PK",
        "        uuid order_id FK",
        "        uuid product_id FK",
        "        int quantity",
        "    }"
      ].join("\n"),
    },
    dbml: {
      format: "dbml",
      source: [
        "Table customers {",
        "  id uuid [pk]",
        "  name varchar(120)",
        "  plan varchar(32)",
        "}",
        "",
        "Table invoices {",
        "  id uuid [pk]",
        "  customer_id uuid [ref: > customers.id]",
        "  amount_usd numeric",
        "  status varchar(32)",
        "}"
      ].join("\n"),
    },
    mermaid_flow: {
      format: "mermaid",
      source: [
        "flowchart TD",
        "    Upload[Upload File] --> Check[Check File Type]",
        "    Check --> Digital[Read Direct Text & Tables]",
        "    Check --> Scan[Run OCR on Scanned Pages]",
        "    Check --> SaveImg[Save Unreadable Pages as Images]",
        "    Digital --> Result[Clean Output]",
        "    Scan --> Result",
        "    SaveImg --> Result"
      ].join("\n"),
    },
    plantuml: {
      format: "plantuml",
      source: [
        "@startuml",
        "entity documents {",
        "  *id : uuid <<PK>>",
        "  *sha256 : char(64) <<UK>>",
        "  mime : varchar",
        "}",
        "entity pages {",
        "  *id : varchar <<PK>>",
        "  *doc_id : uuid <<FK>>",
        "  kind : varchar",
        "}",
        "documents ||--o{ pages : contains",
        "@enduml"
      ].join("\n"),
    },
    dot: {
      format: "dot",
      source: [
        "digraph Workflow {",
        "  upload [label=\"1. Upload Document\"];",
        "  extract [label=\"2. Extract Text & Tables\"];",
        "  review [label=\"3. Review & Download\"];",
        "  upload -> extract;",
        "  extract -> review;",
        "}"
      ].join("\n"),
    },
    drawio: {
      format: "drawio",
      source: [
        "<mxfile>",
        "  <diagram name=\"Overview\">",
        "    <mxGraphModel>",
        "      <root>",
        "        <mxCell id=\"0\"/>",
        "        <mxCell id=\"1\" parent=\"0\"/>",
        "        <mxCell id=\"n1\" value=\"PDF / Word / Excel\" vertex=\"1\" parent=\"1\">",
        "          <mxGeometry x=\"40\" y=\"60\" width=\"160\" height=\"60\" as=\"geometry\"/>",
        "        </mxCell>",
        "        <mxCell id=\"n2\" value=\"Doc Extractor\" vertex=\"1\" parent=\"1\">",
        "          <mxGeometry x=\"270\" y=\"60\" width=\"170\" height=\"60\" as=\"geometry\"/>",
        "        </mxCell>",
        "        <mxCell id=\"n3\" value=\"Clean Text & Tables\" vertex=\"1\" parent=\"1\">",
        "          <mxGeometry x=\"510\" y=\"60\" width=\"170\" height=\"60\" as=\"geometry\"/>",
        "        </mxCell>",
        "        <mxCell id=\"e1\" value=\"reads\" edge=\"1\" source=\"n1\" target=\"n2\" parent=\"1\"/>",
        "        <mxCell id=\"e2\" value=\"exports\" edge=\"1\" source=\"n2\" target=\"n3\" parent=\"1\"/>",
        "      </root>",
        "    </mxGraphModel>",
        "  </diagram>",
        "</mxfile>"
      ].join("\n"),
    },
    vision_json: {
      format: "vision_json",
      source: JSON.stringify(
        {
          nodes: [
            { id: "pdf_page", label: "Unreadable PDF Page" },
            { id: "saved_png", label: "Saved 150 DPI Image (pages/)" },
            { id: "ai_vision", label: "AI Vision Reader" }
          ],
          edges: [
            {
              source: "pdf_page",
              target: "saved_png",
              label: "saved automatically",
              cardinality: "1..1"
            },
            {
              source: "saved_png",
              target: "ai_vision",
              label: "manifest.json",
              cardinality: "1..*"
            }
          ]
        },
        null,
        2
      ),
    },
    pdf_layout: {
      format: "pdf_layout",
      source: [
        "# Word, X position, Y position — automatically grouped into a table",
        "Quarter, 48, 60, 52, 12",
        "Revenue, 180, 60, 56, 12",
        "Growth, 310, 60, 48, 12",
        "Q1-2026, 48, 84, 52, 12",
        "$142000, 180, 84, 56, 12",
        "+14.2%, 310, 84, 42, 12",
        "Q2-2026, 48, 108, 52, 12",
        "$168500, 180, 108, 56, 12",
        "+18.6%, 310, 108, 42, 12",
        "Q3-2026, 48, 132, 52, 12",
        "$194200, 180, 132, 56, 12",
        "+21.5%, 310, 132, 42, 12"
      ].join("\n"),
    },
    markdown: {
      format: "markdown",
      source: [
        "# Monthly Operations Summary",
        "",
        "Direct text, scanned text, and auto-detected tables are kept clearly labeled.",
        "",
        "| Department | Documents Processed | Accuracy |",
        "| :--- | ---: | ---: |",
        "| Finance | 140 | 100% |",
        "| Legal | 85 | 100% |",
        "| Operations | 91 | 100% |"
      ].join("\n"),
    },
    chart_xml: {
      format: "chart_xml",
      source: [
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">",
        "  <c:chart>",
        "    <c:plotArea>",
        "      <c:barChart>",
        "        <c:ser>",
        "          <c:idx val=\"0\"/>",
        "          <c:cat><c:strRef><c:f>Sheet1!$A$2:$A$4</c:f>",
        "            <c:strCache>",
        "              <c:pt idx=\"0\"><c:v>Digital PDF</c:v></c:pt>",
        "              <c:pt idx=\"1\"><c:v>Scanned OCR</c:v></c:pt>",
        "              <c:pt idx=\"2\"><c:v>Tables</c:v></c:pt>",
        "            </c:strCache>",
        "          </c:strRef></c:cat>",
        "          <c:val><c:numRef><c:f>Sheet1!$B$2:$B$4</c:f>",
        "            <c:numCache>",
        "              <c:pt idx=\"0\"><c:v>100.0</c:v></c:pt>",
        "              <c:pt idx=\"1\"><c:v>100.0</c:v></c:pt>",
        "              <c:pt idx=\"2\"><c:v>100.0</c:v></c:pt>",
        "            </c:numCache>",
        "          </c:numRef></c:val>",
        "        </c:ser>",
        "      </c:barChart>",
        "    </c:plotArea>",
        "  </c:chart>",
        "</c:chartSpace>"
      ].join("\n"),
    },
  };

  // ==================== RUST WASM BRIDGE ====================
  function initWasm() {
    var statusEl = document.getElementById("wasm-status");
    var instantiate = WebAssembly.instantiateStreaming
      ? WebAssembly.instantiateStreaming(fetch("/wasm/doc_extractor.wasm"), {})
          .catch(function () {
            return fetch("/wasm/doc_extractor.wasm")
              .then(function (r) { return r.arrayBuffer(); })
              .then(function (bytes) { return WebAssembly.instantiate(bytes, {}); });
          })
      : fetch("/wasm/doc_extractor.wasm")
          .then(function (r) { return r.arrayBuffer(); })
          .then(function (bytes) { return WebAssembly.instantiate(bytes, {}); });

    return instantiate
      .then(function (res) {
        state.wasm = res.instance.exports;
        state.wasmReady = true;
        wasmCall("ping", {});
        statusEl.textContent = "Browser Engine Ready";
        validateStudioOptions();
        runPlayground();
      })
      .catch(function () {
        statusEl.textContent = "Server Mode Only";
      });
  }

  function wasmCall(op, payloadObj) {
    if (!state.wasmReady || !state.wasm) {
      throw new Error("Browser engine not ready yet");
    }
    var t0 = performance.now();
    var exports = state.wasm;
    var opBytes = encoder.encode(op);
    var argBytes = encoder.encode(
      typeof payloadObj === "string" ? payloadObj : JSON.stringify(payloadObj || {})
    );

    var opPtr = exports.wasm_alloc(opBytes.length);
    var argPtr = exports.wasm_alloc(argBytes.length);
    try {
      var mem = new Uint8Array(exports.memory.buffer);
      mem.set(opBytes, opPtr);
      mem.set(argBytes, argPtr);
      var ok = exports.wasm_invoke(opPtr, opBytes.length, argPtr, argBytes.length);
      var outPtr = exports.wasm_output_ptr();
      var outLen = exports.wasm_output_len();
      var outBytes = new Uint8Array(exports.memory.buffer, outPtr, outLen);
      var jsonText = decoder.decode(outBytes);
      var elapsedMs = performance.now() - t0;
      var perfBadge = document.getElementById("wasm-perf");
      if (perfBadge) {
        perfBadge.textContent = elapsedMs.toFixed(1) + " ms";
      }
      var parsed = JSON.parse(jsonText);
      if (!ok || (parsed && parsed.error)) {
        throw new Error((parsed && parsed.error && parsed.error.message) || "Engine error");
      }
      parsed._wasm_ms = elapsedMs;
      return parsed;
    } finally {
      exports.wasm_dealloc(opPtr, opBytes.length);
      exports.wasm_dealloc(argPtr, argBytes.length);
    }
  }

  // ==================== HTTP HELPERS ====================
  function authHeaders() {
    var h = {};
    if (state.token) {
      h["Authorization"] = "Bearer " + state.token;
    }
    return h;
  }

  function apiGet(path) {
    return fetch(path, { headers: authHeaders() }).then(function (r) {
      return r.text().then(function (body) {
        if (!r.ok) {
          throw new Error("HTTP " + r.status + ": " + body.slice(0, 300));
        }
        try {
          return JSON.parse(body);
        } catch (_) {
          return body;
        }
      });
    });
  }

  function formatBytes(bytes) {
    if (!bytes || bytes <= 0) return "0 B";
    if (bytes < 1024) return bytes + " B";
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
    return (bytes / (1024 * 1024)).toFixed(2) + " MB";
  }

  function switchTab(tabName) {
    state.activeTab = tabName;
    document.querySelectorAll(".nav-item").forEach(function (btn) {
      var active = btn.getAttribute("data-tab") === tabName;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-selected", active ? "true" : "false");
    });
    document.querySelectorAll(".tab-panel").forEach(function (panel) {
      panel.classList.toggle("is-active", panel.id === "panel-" + tabName);
    });
    var hdr = TAB_HEADERS[tabName];
    if (hdr) {
      document.getElementById("app-title").textContent = hdr.title;
      document.getElementById("app-subtitle").textContent = hdr.subtitle;
    }
    if (tabName === "playground" && !state.pgResult && state.wasmReady) {
      runPlayground();
    }
  }

  function switchSubtab(subName) {
    state.activeSubtab = subName;
    document.querySelectorAll(".subtab-btn").forEach(function (btn) {
      btn.classList.toggle("is-active", btn.getAttribute("data-subtab") === subName);
    });
    document.querySelectorAll(".subtab-panel").forEach(function (panel) {
      panel.classList.toggle("is-active", panel.id === "subpanel-" + subName);
    });
  }

  // ==================== TELEMETRY & CAPABILITIES ====================
  function refreshTelemetry() {
    apiGet("/v1/stats")
      .then(function (stats) {
        var w = stats.workers || { active: 0, slots: 0 };
        document.getElementById("stat-workers").textContent = w.active + " of " + w.slots + " busy";
        var j = stats.jobs || {};
        document.getElementById("stat-jobs").textContent =
          (j.completed || 0) + " done" + (j.running ? " (" + j.running + " running)" : "");
        document.getElementById("stat-disk").textContent = formatBytes(stats.disk_bytes || 0);
      })
      .catch(function () {});
  }

  function refreshCapabilities() {
    apiGet("/v1/capabilities")
      .then(function (caps) {
        document.getElementById("capabilities-raw-json").textContent = JSON.stringify(caps, null, 2);
        var installed = (caps.ocr && caps.ocr.languages) || [];
        var langLabel = document.getElementById("installed-languages");
        if (langLabel) {
          langLabel.textContent = installed.length ? installed.join(", ") : "none";
        }
        var matrix = document.getElementById("capabilities-matrix");
        matrix.innerHTML = "";
        var items = [
          {
            title: "PDF Reader & Page Images",
            ok: caps.pdf && caps.pdf.pdftotext && caps.pdf.page_images,
            detail: "Reads digital PDFs and saves 150 DPI page images",
          },
          {
            title: "Scanned Text Reader (OCR)",
            ok: caps.ocr && caps.ocr.available,
            detail:
              "Installed languages: " +
              ((caps.ocr && caps.ocr.languages && caps.ocr.languages.join(", ")) || "none"),
          },
          {
            title: "Apple Vision OCR (macOS)",
            ok: caps.apple_vision && caps.apple_vision.available,
            detail:
              caps.apple_vision && caps.apple_vision.available
                ? "Installed and ready"
                : "Optional (not enabled)",
          },
          {
            title: "Office, Sheets & Diagrams",
            ok: true,
            detail: "Word, Excel, PowerPoint, CSV, HTML, Markdown, Mermaid, DBML, draw.io",
          },
          {
            title: "In-Browser Rust + WASM",
            ok: state.wasmReady,
            detail: state.wasmReady
              ? "Active — instant previews and highlights"
              : "Loading",
          }
        ];
        items.forEach(function (item) {
          var div = document.createElement("div");
          div.className = "cap-tile";
          div.innerHTML =
            "<div style='display:flex;justify-content:space-between;align-items:center;margin-bottom:4px'>" +
            "<strong>" + item.title + "</strong>" +
            "<span class='badge " + (item.ok ? "badge-success" : "badge-warn") + "'>" +
            (item.ok ? "Ready" : "Optional") +
            "</span></div>" +
            "<div class='muted' style='font-size:12px'>" + item.detail + "</div>";
          matrix.appendChild(div);
        });
      })
      .catch(function () {});
  }

  // ==================== STUDIO OPTIONS & UPLOAD ====================
  function collectStudioOptions() {
    var opts = {
      ocr: document.getElementById("opt-ocr").checked,
      ocr_provider: document.getElementById("opt-ocr-provider").value,
      ocr_auto_rotate: document.getElementById("opt-auto-rotate").checked,
      languages: document.getElementById("opt-languages").value.trim() || "eng",
      page_images: document.getElementById("opt-page-images").value,
      result_format: document.getElementById("opt-result-format").value,
      compact_tables: document.getElementById("opt-compact-tables").checked,
      interpret_diagrams: document.getElementById("opt-interpret-diagrams").checked,
      vision: document.getElementById("opt-vision").checked,
    };
    var pages = document.getElementById("opt-pages").value.trim();
    if (pages) opts.pages = pages;
    var cc = parseInt(document.getElementById("opt-chunk-chars").value, 10);
    if (cc > 0) opts.chunk_chars = cc;
    var co = parseInt(document.getElementById("opt-chunk-overlap").value, 10);
    if (co >= 0 && cc > 0) opts.chunk_overlap = co;
    return opts;
  }

  function validateStudioOptions() {
    if (!state.wasmReady) return;
    var opts = collectStudioOptions();
    var badge = document.getElementById("options-valid-badge");
    var hint = document.getElementById("opt-pages-feedback");
    try {
      var res = wasmCall("validate_options", { options: opts });
      if (res.valid) {
        badge.textContent = "Settings Ready";
        badge.className = "badge badge-success";
        if (res.selected_pages_count != null) {
          hint.textContent =
            "Will extract " +
            res.selected_pages_count +
            (res.open_ended ? "+ pages" : " pages") +
            " (" +
            res.selected_pages_preview.slice(0, 8).join(", ") +
            (res.selected_pages_count > 8 ? ", …" : "") +
            ")";
          hint.className = "field-help";
        } else {
          hint.textContent = "All pages will be extracted.";
          hint.className = "field-help";
        }
      } else {
        badge.textContent = "Check Settings";
        badge.className = "badge badge-danger";
        hint.textContent = (res.errors || []).join("; ");
        hint.className = "field-help is-error";
      }
    } catch (_) {}
  }

  function uploadFiles(files) {
    if (!files || !files.length) {
      document.getElementById("upload-status").textContent = "Please choose at least one file first.";
      return;
    }
    var statusEl = document.getElementById("upload-status");
    statusEl.textContent = "Uploading " + files.length + " file(s)…";
    var fd = new FormData();
    var opts = collectStudioOptions();
    fd.append("options", JSON.stringify(opts));
    for (var i = 0; i < files.length; i++) {
      fd.append("file", files[i], files[i].name);
    }
    fetch("/v1/extractions", {
      method: "POST",
      headers: authHeaders(),
      body: fd,
    })
      .then(function (r) {
        return r.json().then(function (body) {
          if (!r.ok) {
            throw new Error((body.error && body.error.message) || JSON.stringify(body));
          }
          return body;
        });
      })
      .then(function (resp) {
        var ids = resp.jobs
          ? resp.jobs.map(function (j) { return j.id; })
          : resp.id
          ? [resp.id]
          : [];
        statusEl.textContent = "Started " + ids.length + " file(s).";
        refreshQueue();
        refreshTelemetry();
        if (ids.length > 0) {
          selectJobForInspection(ids[0], false);
        }
      })
      .catch(function (err) {
        statusEl.textContent = "Could not upload: " + String(err.message || err);
      });
  }

  function runInstantDemoBatch() {
    var csvBlob = new Blob(
      [
        "invoice_id,customer,total_usd,status\nINV-1001,Ada Lovelace,1420.50,paid\nINV-1002,Alan Turing,980.00,paid\nINV-1003,Grace Hopper,3450.75,pending\n"
      ],
      { type: "text/csv" }
    );
    var dbmlBlob = new Blob([PRESETS.dbml.source], { type: "text/plain" });
    var mdBlob = new Blob([PRESETS.markdown.source], { type: "text/markdown" });

    var files = [
      new File([csvBlob], "sample-invoices.csv", { type: "text/csv" }),
      new File([dbmlBlob], "sample-database.dbml", { type: "text/plain" }),
      new File([mdBlob], "sample-summary.md", { type: "text/markdown" })
    ];
    uploadFiles(files);
  }

  // ==================== QUEUE LISTING ====================
  function refreshQueue() {
    var filter = document.getElementById("queue-state-filter").value;
    var url = "/v1/extractions?limit=50" + (filter ? "&state=" + encodeURIComponent(filter) : "");
    apiGet(url)
      .then(function (data) {
        state.jobs = data.jobs || [];
        renderJobsTable();
        populateJobSelector();
      })
      .catch(function () {});
  }

  function renderJobsTable() {
    var tbody = document.getElementById("jobs-tbody");
    tbody.innerHTML = "";
    if (!state.jobs.length) {
      var tr = document.createElement("tr");
      tr.innerHTML =
        "<td colspan='5' class='muted'>No documents yet. Choose a file on the left or click 'Try With Sample Files' at the top right.</td>";
      tbody.appendChild(tr);
      return;
    }

    state.jobs.forEach(function (job) {
      var tr = document.createElement("tr");
      var failures = job.unit_failures || [];
      var hasFailures = failures.length > 0;
      var stateBadgeClass = hasFailures
        ? "badge-warn"
        : job.state === "completed"
        ? "badge-success"
        : job.state === "running" || job.state === "queued"
        ? "badge-primary"
        : "badge-danger";
      var friendlyState = hasFailures
        ? "Needs Attention"
        : job.state === "completed"
        ? "Ready"
        : job.state === "running"
        ? "Extracting…"
        : job.state === "queued"
        ? "Waiting"
        : job.state;

      var opts = job.options || {};
      var optSummary = [];
      if (opts.ocr) {
        optSummary.push(opts.ocr_provider === "smoldocling" ? "SmolDocling OCR" : "OCR On");
      }
      if (opts.page_images === "failed") optSummary.push("Save unreadable pages");
      else if (opts.page_images === "all") optSummary.push("Save all page images");
      if (opts.pages) optSummary.push("Pages " + opts.pages);

      var tdDoc = document.createElement("td");
      tdDoc.innerHTML =
        "<strong>" +
        escapeHtml(job.filename || "Document") +
        "</strong><div class='muted' style='font-size:11.5px'>ID: " +
        escapeHtml(job.id.slice(0, 8)) +
        "</div>";

      var tdState = document.createElement("td");
      tdState.innerHTML =
        "<span class='badge " + stateBadgeClass + "'>" + escapeHtml(friendlyState) + "</span>";
      if (hasFailures) {
        var errLine = document.createElement("div");
        errLine.style.fontSize = "11.5px";
        errLine.style.color = "var(--danger)";
        errLine.style.marginTop = "3px";
        errLine.textContent = failures[0].message || failures[0].code || "Extraction warning";
        errLine.title = failures.map(function (f) { return (f.code || "") + ": " + (f.message || ""); }).join("\n");
        tdState.appendChild(errLine);
      }

      var tdProg = document.createElement("td");
      tdProg.textContent = (job.progress || 0) + (job.progress === 1 ? " page / section" : " pages / sections");

      var tdOpts = document.createElement("td");
      tdOpts.innerHTML =
        "<span class='muted' style='font-size:12.5px'>" +
        escapeHtml(optSummary.join(", ") || "Standard") +
        "</span>";

      var tdAct = document.createElement("td");
      var openBtn = document.createElement("button");
      openBtn.className = "btn btn-sm btn-primary";
      openBtn.textContent = "Open";
      openBtn.addEventListener("click", function () {
        selectJobForInspection(job.id, true);
      });
      tdAct.appendChild(openBtn);

      if (job.state === "completed") {
        var zipLink = document.createElement("a");
        zipLink.className = "btn btn-sm";
        zipLink.style.marginLeft = "6px";
        zipLink.href = "/v1/extractions/" + encodeURIComponent(job.id) + "/archive";
        zipLink.textContent = "Download .zip";
        tdAct.appendChild(zipLink);
      }

      var delBtn = document.createElement("button");
      delBtn.className = "btn btn-sm";
      delBtn.style.marginLeft = "6px";
      delBtn.textContent = "Remove";
      delBtn.addEventListener("click", function () {
        fetch("/v1/extractions/" + encodeURIComponent(job.id), {
          method: "DELETE",
          headers: authHeaders(),
        }).then(function () {
          refreshQueue();
          refreshTelemetry();
        });
      });
      tdAct.appendChild(delBtn);

      tr.appendChild(tdDoc);
      tr.appendChild(tdState);
      tr.appendChild(tdProg);
      tr.appendChild(tdOpts);
      tr.appendChild(tdAct);
      tbody.appendChild(tr);
    });
  }

  function populateJobSelector() {
    var sel = document.getElementById("inspector-job-select");
    var prev = state.currentJobId || sel.value;
    sel.innerHTML = "";
    state.jobs.forEach(function (job) {
      var opt = document.createElement("option");
      opt.value = job.id;
      opt.textContent = job.filename + " (" + job.id.slice(0, 8) + ")";
      sel.appendChild(opt);
    });
    if (prev && state.jobs.some(function (j) { return j.id === prev; })) {
      sel.value = prev;
    } else if (!state.currentJobId && state.jobs.length > 0) {
      sel.value = state.jobs[0].id;
      selectJobForInspection(state.jobs[0].id, false);
    }
  }

  // ==================== DOCUMENT VIEWER ====================
  function selectJobForInspection(jobId, navigateToTab) {
    state.currentJobId = jobId;
    var sel = document.getElementById("inspector-job-select");
    if (sel) sel.value = jobId;
    if (navigateToTab) {
      switchTab("inspector");
    }
    updateExportLinks(jobId);

    apiGet("/v1/extractions/" + encodeURIComponent(jobId))
      .then(function (jobMeta) {
        state.currentJobMeta = jobMeta;
        if (state.activeTab === "inspector") {
          document.getElementById("app-title").textContent =
            "Document Viewer — " + (jobMeta.filename || jobId.slice(0, 8));
        }
        document.getElementById("inspector-doc-status").textContent =
          jobMeta.state === "completed" ? "Completed" : jobMeta.state;

        if (jobMeta.state === "completed") {
          return apiGet("/v1/extractions/" + encodeURIComponent(jobId) + "/result?format=json").then(
            function (doc) {
              if (doc && typeof doc === "object" && Array.isArray(doc.units)) {
                state.currentDoc = doc;
                onDocumentLoaded(doc);
              } else {
                loadSingleUnitFromServer(jobId, "u1");
              }
            }
          );
        } else {
          loadSingleUnitFromServer(jobId, "u1");
        }
      })
      .catch(function () {});
  }

  function updateExportLinks(jobId) {
    var base = "/v1/extractions/" + encodeURIComponent(jobId);
    document.getElementById("dl-json").href = base + "/result?format=json";
    document.getElementById("dl-md").href = base + "/result?format=markdown";
    document.getElementById("dl-txt").href = base + "/result?format=text";
    document.getElementById("dl-jsonl").href = base + "/result?format=jsonl";
    document.getElementById("dl-html").href = base + "/result?format=html";
    document.getElementById("dl-zip").href = base + "/archive";
  }

  function onDocumentLoaded(doc) {
    var unitSelect = document.getElementById("inspector-unit-select");
    unitSelect.innerHTML = "";
    (doc.units || []).forEach(function (u, i) {
      var opt = document.createElement("option");
      opt.value = u.id;
      opt.textContent = (u.name || "Page " + (i + 1)) + " (" + u.blocks.length + " items)";
      unitSelect.appendChild(opt);
    });

    if (state.wasmReady) {
      try {
        state.currentDocSummary = wasmCall("inspect_document", { document: doc });
        renderPagesQueueTab(doc, state.currentDocSummary);
      } catch (_) {}
    }

    var firstUnit = (doc.units && doc.units[0] && doc.units[0].id) || "u1";
    selectUnit(firstUnit);
    renderPageFilmstrip();
  }

  function loadSingleUnitFromServer(jobId, unitId) {
    var from = document.getElementById("slice-from").value || "0";
    var count = document.getElementById("slice-count").value || "250";
    apiGet(
      "/v1/extractions/" +
        encodeURIComponent(jobId) +
        "/units/" +
        encodeURIComponent(unitId) +
        "?from=" +
        encodeURIComponent(from) +
        "&count=" +
        encodeURIComponent(count)
    )
      .then(function (data) {
        var unit = data.unit ? data.unit : data;
        state.currentUnitId = unit.id || unitId;
        state.currentUnit = unit;
        if (data.slice) {
          document.getElementById("slice-info").textContent =
            "Showing rows " + data.slice.from + " to " + (data.slice.from + data.slice.rows);
        }
        renderCurrentUnit();
      })
      .catch(function () {});
  }

  function selectUnit(unitId) {
    state.currentUnitId = unitId;
    state.activeBlockId = "";
    var unitSelect = document.getElementById("inspector-unit-select");
    if (unitSelect && unitSelect.value !== unitId) {
      unitSelect.value = unitId;
    }
    if (state.currentDoc && Array.isArray(state.currentDoc.units)) {
      var found = state.currentDoc.units.find(function (u) {
        return u.id === unitId;
      });
      if (found && found.blocks && found.blocks.length > 0) {
        state.currentUnit = found;
        renderCurrentUnit();
        return;
      }
    }
    if (state.currentJobId) {
      loadSingleUnitFromServer(state.currentJobId, unitId);
    }
  }

  function findUnitRasterAsset(unit, doc) {
    if (!unit || !unit.blocks) return null;
    var assetId = null;
    var dpi = 150;
    for (var i = 0; i < unit.blocks.length; i++) {
      var b = unit.blocks[i];
      if (b.attributes && b.attributes.page_image && b.attributes.page_image.asset_id) {
        assetId = b.attributes.page_image.asset_id;
        dpi = b.attributes.page_image.dpi || 150;
        break;
      }
    }
    if (!assetId) {
      for (var j = 0; j < unit.blocks.length; j++) {
        var blk = unit.blocks[j];
        if (blk.kind === "figure" && blk.asset_id) {
          assetId = blk.asset_id;
          break;
        }
      }
    }
    if (!assetId) return null;
    var assetMeta =
      doc &&
      Array.isArray(doc.assets) &&
      doc.assets.find(function (a) {
        return a.id === assetId && a.mime && a.mime.indexOf("image/") === 0;
      });
    return {
      asset_id: assetId,
      width: assetMeta && assetMeta.width ? assetMeta.width : null,
      height: assetMeta && assetMeta.height ? assetMeta.height : null,
      dpi: dpi,
    };
  }

  function renderPageFilmstrip() {
    var strip = document.getElementById("page-filmstrip");
    if (!strip || !state.currentDoc) return;
    var units = state.currentDoc.units || [];
    strip.innerHTML = "";
    units.forEach(function (u, i) {
      var raster = findUnitRasterAsset(u, state.currentDoc);
      var btn = document.createElement("button");
      btn.type = "button";
      btn.className = "page-film-thumb" + (u.id === state.currentUnitId ? " is-active" : "");
      btn.setAttribute("data-unit", u.id);
      btn.title = u.name || "Page " + (i + 1);
      var inner = "";
      if (raster && state.currentJobId) {
        inner =
          "<img loading='lazy' src='/v1/extractions/" +
          encodeURIComponent(state.currentJobId) +
          "/assets/" +
          encodeURIComponent(raster.asset_id) +
          "' alt='Page " +
          (i + 1) +
          "'>";
      } else {
        inner = "<span class='page-film-placeholder'>" + (i + 1) + "</span>";
      }
      btn.innerHTML =
        inner + "<span class='page-film-label'>Page " + (i + 1) + "</span>";
      btn.addEventListener("click", function () {
        selectUnit(u.id);
      });
      strip.appendChild(btn);
    });
    var caption = document.getElementById("page-filmstrip-caption");
    if (caption) {
      caption.textContent =
        "All " + units.length + (units.length === 1 ? " page" : " pages");
    }
    updateFilmstripActive();
  }

  function updateFilmstripActive() {
    document.querySelectorAll("#page-filmstrip .page-film-thumb").forEach(function (btn) {
      btn.classList.toggle("is-active", btn.getAttribute("data-unit") === state.currentUnitId);
    });
    var active = document.querySelector("#page-filmstrip .page-film-thumb.is-active");
    if (active && active.scrollIntoView) {
      active.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }

  function renderCurrentUnit() {
    var unit = state.currentUnit;
    if (!unit) return;

    var raster = findUnitRasterAsset(unit, state.currentDoc);
    var showRaster = document.getElementById("toggle-raster-bg").checked && Boolean(raster);
    var imgEl = document.getElementById("page-raster-img");
    var stageEl = document.getElementById("page-stage");

    if (showRaster && raster && state.currentJobId) {
      state.currentPageRasterAssetId = raster.asset_id;
      state.currentRasterDims = {
        width: raster.width,
        height: raster.height,
        dpi: raster.dpi || 150,
      };
      imgEl.src =
        "/v1/extractions/" +
        encodeURIComponent(state.currentJobId) +
        "/assets/" +
        encodeURIComponent(raster.asset_id);
      imgEl.classList.remove("hidden");
      stageEl.classList.add("has-raster");
    } else {
      state.currentPageRasterAssetId = null;
      state.currentRasterDims = { width: null, height: null, dpi: 150 };
      imgEl.classList.add("hidden");
      stageEl.classList.remove("has-raster");
    }

    var chunkChars = parseInt(document.getElementById("rng-chunk-chars").value, 10) || 320;
    var chunkOverlap = parseInt(document.getElementById("rng-chunk-overlap").value, 10) || 40;
    var compactTables = document.getElementById("fmt-compact-toggle").checked;

    if (state.wasmReady) {
      try {
        var derived = wasmCall("derive_unit", {
          unit: unit,
          chunk_chars: chunkChars,
          chunk_overlap: chunkOverlap,
          compact_tables: compactTables,
          image_width: state.currentRasterDims.width,
          image_height: state.currentRasterDims.height,
          dpi: state.currentRasterDims.dpi,
          filter: state.provFilter,
          active_block_id: state.activeBlockId,
          overlay_only: showRaster,
        });
        state.currentDerived = derived;
        updateProvenanceCounts(derived.stats);
        renderPageSvgLayer(derived.page_svg);
        renderUnitDiagrams(derived.diagrams);
        renderChunksSubtab(derived.chunks);
        renderDerivedFormatsSubtab(derived);
      } catch (_) {}
    }

    renderBlocksSubtab(unit);
    renderTablesSubtab(unit);
    updateFilmstripActive();
    updateUnitTimingBadge(unit);
  }

  function updateUnitTimingBadge(unit) {
    var chip = document.getElementById("canvas-timing");
    if (!chip) return;
    var timing = unit && unit.timing;
    if (!timing) {
      chip.classList.add("hidden");
      return;
    }
    chip.classList.remove("hidden");
    chip.textContent =
      (timing.method || "").toUpperCase() +
      " · " +
      (timing.total_ms / 1000).toFixed(1) +
      " s";
    chip.className =
      "badge " +
      (timing.method === "ocr"
        ? "badge-primary"
        : timing.method === "hybrid"
        ? "badge-warn"
        : "badge-success");
    var parts = [];
    if (timing.native_ms) parts.push("native " + (timing.native_ms / 1000).toFixed(2) + " s");
    if (timing.render_ms) parts.push("render " + (timing.render_ms / 1000).toFixed(2) + " s");
    if (timing.ocr_ms) parts.push("OCR " + (timing.ocr_ms / 1000).toFixed(2) + " s");
    if (timing.page_image_ms)
      parts.push("page image " + (timing.page_image_ms / 1000).toFixed(2) + " s");
    if (timing.ocr_words) parts.push(timing.ocr_words + " OCR words");
    if (timing.tables) parts.push(timing.tables + (timing.tables === 1 ? " table" : " tables"));
    chip.title = parts.join(" · ");
  }

  function updateProvenanceCounts(stats) {
    if (!stats) return;
    document.getElementById("cnt-prov-all").textContent = stats.blocks || 0;
    document.getElementById("cnt-prov-native").textContent = stats.native || 0;
    document.getElementById("cnt-prov-ocr").textContent = stats.ocr || 0;
    document.getElementById("cnt-prov-inferred").textContent = stats.inferred || 0;
  }

  function renderPageSvgLayer(svgHtml) {
    var layer = document.getElementById("page-svg-layer");
    layer.innerHTML = svgHtml || "";
    layer.querySelectorAll(".bbox-group").forEach(function (group) {
      var bid = group.getAttribute("data-block-id");
      group.addEventListener("mouseenter", function () {
        highlightBlockCard(bid, false);
      });
      group.addEventListener("click", function (ev) {
        ev.stopPropagation();
        state.activeBlockId = bid;
        highlightBlockCard(bid, true);
        refreshPageOverlayOnly();
      });
    });

    var svgEl = layer.querySelector("svg");
    if (svgEl) {
      svgEl.addEventListener("click", function (ev) {
        if (!state.wasmReady || !state.currentUnit) return;
        var rect = svgEl.getBoundingClientRect();
        var vbW = parseFloat(svgEl.getAttribute("data-canvas-width") || "612");
        var vbH = parseFloat(svgEl.getAttribute("data-canvas-height") || "792");
        var x = ((ev.clientX - rect.left) / Math.max(rect.width, 1)) * vbW;
        var y = ((ev.clientY - rect.top) / Math.max(rect.height, 1)) * vbH;
        try {
          var hit = wasmCall("hit_test_page", {
            unit: state.currentUnit,
            x: x,
            y: y,
            image_width: state.currentRasterDims.width,
            image_height: state.currentRasterDims.height,
            dpi: state.currentRasterDims.dpi,
            filter: state.provFilter,
          });
          if (hit && hit.hit) {
            state.activeBlockId = hit.hit;
            highlightBlockCard(hit.hit, true);
            refreshPageOverlayOnly();
          }
        } catch (_) {}
      });
    }
  }

  function refreshPageOverlayOnly() {
    if (!state.wasmReady || !state.currentUnit) return;
    var showRaster =
      document.getElementById("toggle-raster-bg").checked &&
      Boolean(state.currentPageRasterAssetId);
    try {
      var res = wasmCall("render_page_svg", {
        unit: state.currentUnit,
        image_width: state.currentRasterDims.width,
        image_height: state.currentRasterDims.height,
        dpi: state.currentRasterDims.dpi,
        filter: state.provFilter,
        active_block_id: state.activeBlockId,
        overlay_only: showRaster,
      });
      renderPageSvgLayer(res.svg);
    } catch (_) {}
  }

  function renderUnitDiagrams(diagrams) {
    var container = document.getElementById("unit-diagrams-container");
    var box = document.getElementById("unit-diagram-svg-box");
    if (!diagrams || !diagrams.length) {
      container.classList.add("hidden");
      box.innerHTML = "";
      return;
    }
    container.classList.remove("hidden");
    box.innerHTML = diagrams
      .map(function (d) {
        return (
          "<div style='margin-bottom:8px'><span class='badge badge-primary'>" +
          escapeHtml(d.source) +
          " (" +
          d.nodes +
          " items, " +
          d.edges +
          " connections)</span></div>" +
          d.svg
        );
      })
      .join("<hr style='border-color:var(--border)'>");
  }

  function renderBlocksSubtab(unit) {
    var warningsBox = document.getElementById("unit-warnings-box");
    warningsBox.innerHTML = "";
    (unit.errors || []).forEach(function (e) {
      var div = document.createElement("div");
      div.className = "alert-item is-error";
      div.textContent = "Could not read part of this page: " + (e.message || e.code);
      warningsBox.appendChild(div);
    });
    (unit.warnings || []).forEach(function (w) {
      var div = document.createElement("div");
      div.className = "alert-item";
      div.textContent = "Note: " + w;
      warningsBox.appendChild(div);
    });

    var list = document.getElementById("blocks-list");
    list.innerHTML = "";
    var blocks = (unit.blocks || []).filter(function (b) {
      return state.provFilter === "all" || (b.provenance && b.provenance.method === state.provFilter);
    });

    if (!blocks.length) {
      list.innerHTML = "<p class='muted'>No items match the selected filter on this page.</p>";
      return;
    }

    blocks.forEach(function (b) {
      var method = (b.provenance && b.provenance.method) || "native";
      var friendlyMethod = FRIENDLY_METHOD[method] || method;
      var card = document.createElement("div");
      card.className = "block-card" + (b.id === state.activeBlockId ? " is-active" : "");
      card.setAttribute("data-block-id", b.id);
      card.setAttribute("data-method", method);

      var badgeCls =
        method === "ocr"
          ? "badge-warn"
          : method === "inferred"
          ? "badge-primary"
          : "badge-success";

      card.innerHTML =
        "<div class='block-meta'>" +
        "<span><strong>" +
        escapeHtml(b.kind === "page_image" ? "Saved Page Image" : b.kind.toUpperCase()) +
        "</strong> · <span class='muted'>" +
        escapeHtml(b.id) +
        "</span></span>" +
        "<span class='badge " +
        badgeCls +
        "'>" +
        escapeHtml(friendlyMethod) +
        "</span>" +
        "</div>";

      if (b.text) {
        var pre = document.createElement("p");
        pre.className = "block-text";
        pre.textContent = b.text;
        card.appendChild(pre);
      }
      if (b.table) {
        var tNote = document.createElement("div");
        tNote.className = "muted";
        tNote.style.fontSize = "12.5px";
        tNote.textContent =
          "Table with " + b.table.rows + " rows and " + b.table.columns + " columns (click 'Tables' tab above to view grid)";
        card.appendChild(tNote);
      }
      if (b.diagram) {
        var dNote = document.createElement("div");
        dNote.className = "muted";
        dNote.style.fontSize = "12.5px";
        dNote.textContent =
          "Diagram with " + b.diagram.nodes.length + " nodes and " + b.diagram.edges.length + " connections";
        card.appendChild(dNote);
      }

      card.addEventListener("mouseenter", function () {
        state.activeBlockId = b.id;
        refreshPageOverlayOnly();
      });
      card.addEventListener("click", function () {
        state.activeBlockId = b.id;
        highlightBlockCard(b.id, false);
        refreshPageOverlayOnly();
      });
      list.appendChild(card);
    });
  }

  function highlightBlockCard(blockId, scrollIntoView) {
    document.querySelectorAll(".block-card").forEach(function (card) {
      var match = card.getAttribute("data-block-id") === blockId;
      card.classList.toggle("is-active", match);
      if (match && scrollIntoView) {
        card.scrollIntoView({ behavior: "smooth", block: "nearest" });
      }
    });
  }

  function renderTablesSubtab(unit) {
    var container = document.getElementById("tables-container");
    container.innerHTML = "";
    var tableBlocks = (unit.blocks || []).filter(function (b) {
      return Boolean(b.table);
    });
    if (!tableBlocks.length) {
      container.innerHTML = "<p class='muted'>No tables were found on this page.</p>";
      return;
    }

    tableBlocks.forEach(function (b) {
      var table = b.table;
      var wrap = document.createElement("div");
      wrap.style.marginBottom = "1.2rem";
      var method = (b.provenance && b.provenance.method) || "native";
      var hdr = document.createElement("div");
      hdr.style.marginBottom = "0.5rem";
      hdr.innerHTML =
        "<strong>Table (" +
        table.rows +
        " rows × " +
        table.columns +
        " columns)</strong> · <span class='badge'>" +
        escapeHtml(FRIENDLY_METHOD[method] || method) +
        "</span>";
      wrap.appendChild(hdr);

      var cellMap = {};
      (table.cells || []).forEach(function (c) {
        if (!cellMap[c.row]) cellMap[c.row] = {};
        cellMap[c.row][c.column] = c;
      });
      var rowIndices = Object.keys(cellMap)
        .map(Number)
        .sort(function (x, y) { return x - y; });

      var tbl = document.createElement("table");
      tbl.className = "grid-table";
      var thead = document.createElement("thead");
      var htr = document.createElement("tr");
      htr.innerHTML = "<th>Row</th>";
      for (var col = 0; col < table.columns; col++) {
        var th = document.createElement("th");
        th.textContent = "Column " + (col + 1);
        htr.appendChild(th);
      }
      thead.appendChild(htr);
      tbl.appendChild(thead);

      var tbody = document.createElement("tbody");
      rowIndices.slice(0, 300).forEach(function (rIdx) {
        var tr = document.createElement("tr");
        var rth = document.createElement("td");
        rth.className = "muted";
        rth.textContent = String(rIdx + 1);
        tr.appendChild(rth);
        for (var cIdx = 0; cIdx < table.columns; cIdx++) {
          var cell = cellMap[rIdx][cIdx];
          var td = document.createElement("td");
          if (cell) {
            td.textContent = cell.raw || "";
            if (cell.formula) {
              var fSpan = document.createElement("span");
              fSpan.className = "cell-formula";
              fSpan.textContent = "=" + cell.formula;
              td.appendChild(fSpan);
            }
          }
          tr.appendChild(td);
        }
        tbody.appendChild(tr);
      });
      tbl.appendChild(tbody);
      wrap.appendChild(tbl);
      container.appendChild(wrap);
    });
  }

  function renderChunksSubtab(chunks) {
    var badge = document.getElementById("chunk-wasm-stats");
    badge.textContent = (chunks ? chunks.length : 0) + " passages";
    var list = document.getElementById("chunks-list");
    list.innerHTML = "";
    if (!chunks || !chunks.length) {
      list.innerHTML = "<p class='muted'>No text on this page to split into passages.</p>";
      return;
    }
    chunks.forEach(function (ch) {
      var card = document.createElement("div");
      card.className = "chunk-card";
      card.innerHTML =
        "<div class='block-meta'>" +
        "<span><strong>Passage #" +
        (ch.index + 1) +
        "</strong></span>" +
        "<span class='badge badge-primary'>" +
        ch.chars +
        " characters</span></div>" +
        "<p class='block-text'>" +
        escapeHtml(ch.text || "") +
        "</p>";
      list.appendChild(card);
    });
  }

  function renderDerivedFormatsSubtab(derived) {
    if (!derived) return;
    var viewer = document.getElementById("derived-code-viewer");
    var fmt = state.activeFormat;
    viewer.textContent = derived[fmt] || "";
  }

  // ==================== SAVED PAGE IMAGES TAB ====================
  function unitTextStats(unit) {
    var chars = 0;
    var textBlocks = 0;
    var tables = 0;
    ((unit && unit.blocks) || []).forEach(function (b) {
      if (b.table) {
        tables += 1;
        (b.table.cells || []).forEach(function (cell) {
          chars += (cell.raw || "").length;
        });
      } else if (b.text) {
        chars += b.text.length;
        textBlocks += 1;
      }
    });
    return { chars: chars, textBlocks: textBlocks, tables: tables };
  }

  function unitSampleText(unit, limit) {
    var parts = [];
    ((unit && unit.blocks) || []).some(function (b) {
      if (b.table) return false;
      if (b.text) parts.push(b.text);
      return parts.join(" ").length > limit;
    });
    var text = parts.join(" ").replace(/\s+/g, " ").trim();
    return text.length > limit ? text.slice(0, limit) + "…" : text;
  }

  function renderPagesQueueTab(doc, summary) {
    var manifest = (summary && summary.page_images) || { mode: doc.options.page_images, count: 0, pages: [] };
    document.getElementById("pages-tab-count").textContent = String(manifest.count || 0);
    var modeLabel =
      manifest.mode === "failed"
        ? "Automatic (unreadable pages)"
        : manifest.mode === "all"
        ? "Every page"
        : "Off";
    document.getElementById("pages-manifest-mode").textContent = "Setting: " + modeLabel;
    document.getElementById("pages-manifest-count").textContent =
      (manifest.count || 0) + (manifest.count === 1 ? " saved page" : " saved pages");

    var units = doc.units || [];
    var statsByUnit = {};
    var extracted = 0;
    units.forEach(function (u) {
      var stats = unitTextStats(u);
      statsByUnit[u.id] = stats;
      if (stats.chars > 0) extracted += 1;
    });
    var totalTimingMs = 0;
    var ocrTimingMs = 0;
    units.forEach(function (u) {
      if (u.timing) {
        totalTimingMs += u.timing.total_ms || 0;
        ocrTimingMs += u.timing.ocr_ms || 0;
      }
    });
    var summaryEl = document.getElementById("pages-text-summary");
    if (summaryEl) {
      summaryEl.textContent =
        extracted +
        " of " +
        units.length +
        " pages have extracted text · " +
        (manifest.count || 0) +
        (manifest.count === 1 ? " page image saved" : " page images saved") +
        " for vision" +
        (totalTimingMs
          ? " · " +
            (totalTimingMs / 1000).toFixed(1) +
            " s page time" +
            (ocrTimingMs ? " (" + (ocrTimingMs / 1000).toFixed(1) + " s OCR)" : "")
          : "");
    }

    var gallery = document.getElementById("pages-gallery");
    gallery.innerHTML = "";
    if (!manifest.pages || !manifest.pages.length) {
      gallery.innerHTML =
        "<p class='muted'>No pages needed to be saved as images for this document. All text was readable directly.</p>";
      return;
    }

    manifest.pages.forEach(function (p, idx) {
      var card = document.createElement("div");
      card.className = "page-card" + (idx === 0 ? " is-active" : "");
      var imgUrl =
        "/v1/extractions/" +
        encodeURIComponent(doc.id) +
        "/assets/" +
        encodeURIComponent(p.asset_id);
      var friendlyReason = FRIENDLY_REASON[p.reason] || p.reason;
      var unit = units.find(function (u) {
        return u.id === p.unit_id;
      });
      var stats = statsByUnit[p.unit_id] || { chars: 0, textBlocks: 0, tables: 0 };
      var hasText = stats.chars > 0;
      var statusBadge = hasText
        ? "<span class='badge badge-success'>Text extracted</span>"
        : "<span class='badge badge-warn'>No text extracted</span>";
      var timing = unit && unit.timing;
      var timingNote = timing
        ? " · " +
          (timing.total_ms / 1000).toFixed(1) +
          " s total" +
          (timing.ocr_ms
            ? " (OCR " + (timing.ocr_ms / 1000).toFixed(1) + " s)"
            : timing.method
            ? " (" + timing.method + ")"
            : "")
        : "";
      var statsLine = hasText
        ? stats.chars.toLocaleString() +
          " characters" +
          (stats.tables
            ? " · " + stats.tables + (stats.tables === 1 ? " table" : " tables")
            : "") +
          timingNote
        : "OCR produced no text for this page" + timingNote;
      var preview = hasText
        ? "<div class='page-card-preview'>" + escapeHtml(unitSampleText(unit, 120)) + "</div>"
        : "";
      card.innerHTML =
        "<img class='page-card-thumb' src='" +
        imgUrl +
        "' alt='Page " +
        p.page +
        "'>" +
        "<div class='page-card-body'>" +
        "<div style='display:flex;justify-content:space-between;align-items:center;gap:6px;margin-bottom:4px'>" +
        "<strong>" +
        escapeHtml(p.unit_name || "Page " + p.page) +
        "</strong>" +
        statusBadge +
        "</div>" +
        "<div class='muted' style='font-size:12px'>" +
        (p.width ? p.width + " × " + p.height + " px" : "") +
        (p.dpi ? " · " + p.dpi + " DPI" : "") +
        " · " +
        formatBytes(p.bytes) +
        "</div>" +
        "<div class='page-card-stats'>" +
        escapeHtml(statsLine) +
        "</div>" +
        preview +
        "<div class='muted' style='font-size:11.5px;margin-top:4px'>Saved as image · " +
        escapeHtml(friendlyReason) +
        "</div>" +
        "<div class='page-card-actions'><button class='btn btn-sm' data-open-text='" +
        escapeHtml(p.unit_id) +
        "'>View extracted text</button></div>" +
        "</div>";

      card.addEventListener("click", function () {
        gallery.querySelectorAll(".page-card").forEach(function (c) {
          c.classList.remove("is-active");
        });
        card.classList.add("is-active");
        var allBox = document.getElementById("llm-payload-all");
        if (allBox) allBox.checked = false;
        renderLlmPayloadForUnit(doc, p.unit_id);
      });
      card.querySelector("[data-open-text]").addEventListener("click", function (ev) {
        ev.stopPropagation();
        selectUnit(p.unit_id);
        switchTab("inspector");
      });
      gallery.appendChild(card);
    });

    state.activePageUnit = manifest.pages[0].unit_id;
    var allBox = document.getElementById("llm-payload-all");
    if (allBox) allBox.checked = true;
    renderLlmPayloadForUnit(doc, null);
  }

  function renderLlmPayloadForUnit(doc, unitId) {
    if (!state.wasmReady || !doc) return;
    var allBox = document.getElementById("llm-payload-all");
    var all = allBox ? allBox.checked : true;
    if (unitId) state.activePageUnit = unitId;
    var target = all ? null : unitId || state.activePageUnit || null;
    try {
      var payload = wasmCall("build_llm_payload", {
        document: doc,
        unit_id: target,
        base_url: location.origin,
      });
      document.getElementById("llm-payload-viewer").textContent = JSON.stringify(payload, null, 2);
      var scope = document.getElementById("llm-payload-scope");
      if (scope) {
        if (all) {
          scope.textContent =
            "All " + payload.count + (payload.count === 1 ? " saved page" : " saved pages");
        } else if (payload.items && payload.items.length) {
          scope.textContent = "Page " + payload.items[0].page;
        } else {
          scope.textContent = "No saved page selected";
        }
      }
    } catch (e) {
      document.getElementById("llm-payload-viewer").textContent = String(e.message || e);
    }
  }

  // ==================== INSTANT TRY-OUT TAB ====================
  function loadPlaygroundPreset(presetKey) {
    var preset = PRESETS[presetKey];
    if (!preset) return;
    document.getElementById("pg-format-select").value = preset.format;
    document.getElementById("pg-source-input").value = preset.source;
    document.querySelectorAll("#pg-preset-bar button").forEach(function (btn) {
      btn.classList.toggle("is-active", btn.getAttribute("data-preset") === presetKey);
    });
    runPlayground();
  }

  function runPlayground() {
    if (!state.wasmReady) return;
    var format = document.getElementById("pg-format-select").value;
    var source = document.getElementById("pg-source-input").value;
    var timingBadge = document.getElementById("pg-timing-badge");
    try {
      var res = wasmCall("playground_extract", {
        source: source,
        format: format,
        chunk_chars: 280,
        chunk_overlap: 32,
      });
      state.pgResult = res;
      timingBadge.textContent = "Extracted in " + res._wasm_ms.toFixed(1) + " ms";
      timingBadge.className = "badge badge-success";
      renderPlaygroundOutput();
    } catch (err) {
      timingBadge.textContent = "Check Input";
      timingBadge.className = "badge badge-danger";
      document.getElementById("pg-visual-box").innerHTML =
        "<p style='color:#fca5a5;margin:0'>" +
        escapeHtml(String(err.message || err)) +
        "</p>";
    }
  }

  function renderPlaygroundOutput() {
    var res = state.pgResult;
    if (!res) return;
    var visualBox = document.getElementById("pg-visual-box");
    var codeBox = document.getElementById("pg-code-box");
    var mode = state.activePgOut;

    if (mode === "visual") {
      visualBox.classList.remove("hidden");
      codeBox.classList.add("hidden");
      if (res.diagrams && res.diagrams.length > 0) {
        visualBox.innerHTML = res.diagrams
          .map(function (d) {
            return (
              "<div style='margin-bottom:8px'>" +
              "<span class='badge badge-success'>Diagram: " +
              d.nodes +
              " boxes, " +
              d.edges +
              " arrows</span></div>" +
              d.svg
            );
          })
          .join("<hr>");
      } else if (res.stats && res.stats.bounds_blocks > 0) {
        visualBox.innerHTML =
          "<div style='margin-bottom:8px'><span class='badge badge-primary'>Auto-Detected Table &amp; Word Positions (" +
          res.stats.tables +
          " table found)</span></div>" +
          res.page_svg;
      } else {
        visualBox.innerHTML = "<div class='code-viewer'>" + res.html + "</div>";
      }
    } else {
      visualBox.classList.add("hidden");
      codeBox.classList.remove("hidden");
      if (mode === "chunks") {
        codeBox.textContent = JSON.stringify(res.chunks || [], null, 2);
      } else {
        codeBox.textContent = res[mode] || "";
      }
    }
  }

  function escapeHtml(str) {
    return String(str || "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  }

  // ==================== EVENT LISTENERS ====================
  function bindEvents() {
    document.querySelectorAll(".nav-item").forEach(function (btn) {
      btn.addEventListener("click", function () {
        switchTab(btn.getAttribute("data-tab"));
      });
    });

    document.querySelectorAll(".subtab-btn").forEach(function (btn) {
      btn.addEventListener("click", function () {
        switchSubtab(btn.getAttribute("data-subtab"));
      });
    });

    var tokenInput = document.getElementById("token-input");
    tokenInput.value = state.token;
    tokenInput.addEventListener("change", function () {
      state.token = tokenInput.value.trim();
      localStorage.setItem("docsmith-token", state.token);
      refreshTelemetry();
      refreshQueue();
    });

    var dropzone = document.getElementById("dropzone");
    var fileInput = document.getElementById("file-input");
    fileInput.addEventListener("change", function () {
      state.selectedFiles = Array.from(fileInput.files || []);
      if (state.selectedFiles.length) {
        document.getElementById("dropzone-title").textContent =
          state.selectedFiles.length +
          " file(s) selected: " +
          state.selectedFiles.map(function (f) { return f.name; }).join(", ");
      }
    });
    dropzone.addEventListener("dragover", function (ev) {
      ev.preventDefault();
      dropzone.classList.add("is-dragover");
    });
    dropzone.addEventListener("dragleave", function () {
      dropzone.classList.remove("is-dragover");
    });
    dropzone.addEventListener("drop", function (ev) {
      ev.preventDefault();
      dropzone.classList.remove("is-dragover");
      if (ev.dataTransfer && ev.dataTransfer.files && ev.dataTransfer.files.length) {
        state.selectedFiles = Array.from(ev.dataTransfer.files);
        document.getElementById("dropzone-title").textContent =
          state.selectedFiles.length +
          " file(s) selected: " +
          state.selectedFiles.map(function (f) { return f.name; }).join(", ");
        uploadFiles(state.selectedFiles);
      }
    });

    document.getElementById("submit-upload-btn").addEventListener("click", function () {
      uploadFiles(state.selectedFiles);
    });
    document.getElementById("sample-synth-btn").addEventListener("click", runInstantDemoBatch);

    [
      "opt-page-images",
      "opt-pages",
      "opt-ocr",
      "opt-ocr-provider",
      "opt-languages",
      "opt-chunk-chars",
      "opt-chunk-overlap",
      "opt-result-format",
      "opt-auto-rotate",
      "opt-compact-tables",
      "opt-interpret-diagrams",
      "opt-vision"
    ].forEach(function (id) {
      var el = document.getElementById(id);
      if (el) {
        el.addEventListener("input", validateStudioOptions);
        el.addEventListener("change", validateStudioOptions);
      }
    });

    document.getElementById("queue-state-filter").addEventListener("change", refreshQueue);
    document.getElementById("refresh-queue-btn").addEventListener("click", refreshQueue);

    document.getElementById("inspector-job-select").addEventListener("change", function (ev) {
      selectJobForInspection(ev.target.value, false);
    });
    document.getElementById("inspector-unit-select").addEventListener("change", function (ev) {
      selectUnit(ev.target.value);
    });
    document.getElementById("unit-prev-btn").addEventListener("click", function () {
      stepUnit(-1);
    });
    document.getElementById("unit-next-btn").addEventListener("click", function () {
      stepUnit(1);
    });
    document.getElementById("toggle-raster-bg").addEventListener("change", renderCurrentUnit);

    document.querySelectorAll(".filter-chip").forEach(function (chip) {
      chip.addEventListener("click", function () {
        state.provFilter = chip.getAttribute("data-prov") || "all";
        document.querySelectorAll(".filter-chip").forEach(function (c) {
          c.classList.toggle("is-active", c === chip);
        });
        renderCurrentUnit();
      });
    });

    document.getElementById("slice-load-btn").addEventListener("click", function () {
      if (state.currentJobId) loadSingleUnitFromServer(state.currentJobId, state.currentUnitId);
    });
    document.getElementById("slice-prev-btn").addEventListener("click", function () {
      var fromEl = document.getElementById("slice-from");
      var count = parseInt(document.getElementById("slice-count").value || "250", 10);
      fromEl.value = String(Math.max(0, (parseInt(fromEl.value || "0", 10) || 0) - count));
      if (state.currentJobId) loadSingleUnitFromServer(state.currentJobId, state.currentUnitId);
    });
    document.getElementById("slice-next-btn").addEventListener("click", function () {
      var fromEl = document.getElementById("slice-from");
      var count = parseInt(document.getElementById("slice-count").value || "250", 10);
      fromEl.value = String((parseInt(fromEl.value || "0", 10) || 0) + count);
      if (state.currentJobId) loadSingleUnitFromServer(state.currentJobId, state.currentUnitId);
    });

    ["rng-chunk-chars", "rng-chunk-overlap"].forEach(function (id) {
      document.getElementById(id).addEventListener("input", function () {
        document.getElementById("lbl-chunk-chars").textContent =
          document.getElementById("rng-chunk-chars").value;
        document.getElementById("lbl-chunk-overlap").textContent =
          document.getElementById("rng-chunk-overlap").value;
        renderCurrentUnit();
      });
    });

    document.querySelectorAll(".fmt-btn").forEach(function (btn) {
      btn.addEventListener("click", function () {
        state.activeFormat = btn.getAttribute("data-fmt");
        document.querySelectorAll(".fmt-btn").forEach(function (b) {
          b.classList.toggle("is-active", b === btn);
        });
        renderDerivedFormatsSubtab(state.currentDerived);
      });
    });
    document.getElementById("fmt-compact-toggle").addEventListener("change", renderCurrentUnit);

    document.getElementById("llm-payload-all").addEventListener("change", function () {
      if (state.currentDoc) {
        renderLlmPayloadForUnit(state.currentDoc, state.activePageUnit);
      }
    });

    document.getElementById("copy-llm-payload-btn").addEventListener("click", function () {
      var txt = document.getElementById("llm-payload-viewer").textContent;
      if (navigator.clipboard) {
        navigator.clipboard.writeText(txt);
      }
    });

    document.querySelectorAll("#pg-preset-bar button").forEach(function (btn) {
      btn.addEventListener("click", function () {
        loadPlaygroundPreset(btn.getAttribute("data-preset"));
      });
    });
    document.getElementById("pg-format-select").addEventListener("change", runPlayground);
    document.getElementById("pg-source-input").addEventListener("input", runPlayground);
    document.getElementById("pg-run-btn").addEventListener("click", runPlayground);
    document.querySelectorAll(".pg-out-btn").forEach(function (btn) {
      btn.addEventListener("click", function () {
        state.activePgOut = btn.getAttribute("data-pgout");
        document.querySelectorAll(".pg-out-btn").forEach(function (b) {
          b.classList.toggle("is-active", b === btn);
        });
        renderPlaygroundOutput();
      });
    });

    document.getElementById("refresh-caps-btn").addEventListener("click", refreshCapabilities);
  }

  function stepUnit(delta) {
    var sel = document.getElementById("inspector-unit-select");
    if (!sel || !sel.options.length) return;
    var nextIdx = Math.max(0, Math.min(sel.options.length - 1, sel.selectedIndex + delta));
    sel.selectedIndex = nextIdx;
    selectUnit(sel.options[nextIdx].value);
  }

  // ==================== BOOTSTRAP ====================
  document.addEventListener("DOMContentLoaded", function () {
    bindEvents();
    document.getElementById("pg-source-input").value = PRESETS.doctags.source;

    if (location.pathname.indexOf("/view/") === 0) {
      var viewId = decodeURIComponent(location.pathname.slice("/view/".length));
      if (viewId) {
        state.currentJobId = viewId;
        switchTab("inspector");
      }
    }

    initWasm().then(function () {
      refreshTelemetry();
      refreshCapabilities();
      refreshQueue();
      if (state.currentJobId) {
        selectJobForInspection(state.currentJobId, true);
      }
    });

    setInterval(function () {
      refreshTelemetry();
      if (state.activeTab === "studio") {
        refreshQueue();
      }
    }, 2500);
  });
})();
