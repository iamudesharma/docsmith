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
    uploading: false,
    jobs: [],
    currentJobId: "",
    currentJobMeta: null,
    currentDoc: null,
    currentDocSummary: null,
    currentUnitId: "u1",
    currentUnit: null,
    currentDerived: null,
    currentPageRasterAssetId: null,
    currentPageRasterVisible: false,
    currentRasterDims: { width: null, height: null, dpi: 150 },
    previewRasters: {},
    renderingUnit: "",
    review: { schema_version: "review1", units: {} },
    reviewOriginals: {},
    reviewOriginalCells: {},
    reviewBusy: false,
    pgResult: null,
    pendingUnitId: "",
    pendingBlockId: "",
    scrollToBlockId: "",
    applyingRoute: false,
    searchTimer: null,
    searchAbort: null,
    searchResults: [],
    searchSelection: 0,
    jobStreams: {},
    notified: {},
    deleteTimers: {},
    pendingDeletes: {},
    lastJobStates: {},
    jobsSignature: "",
    selectedJobIds: {},
    libraryModalJobId: "",
    continuousMode: false,
    continuousLimit: 40,
    redactionJobId: "",
    redactionMatches: [],
    redactionSaved: [],
    redactionSelected: {},
    notify: localStorage.getItem("docsmith-notify") === "1",
  };

  var TERMINAL_STATES = { completed: true, failed: true, cancelled: true };

  var TAB_HEADERS = {
    studio: {
      title: "Documents",
      subtitle:
        "Upload PDF, Office, spreadsheet, or image files to extract clean text, tables, and diagrams.",
    },
    inspector: {
      title: "Reader",
      subtitle:
        "Read a page and its extracted text side by side. Click any highlighted box to jump to that text.",
    },
    pages: {
      title: "Page images",
      subtitle:
        "Every page is extracted to text. Pages that are blank, scanned or diagram-heavy are also kept as clean 150 DPI images for a vision model.",
    },
    playground: {
      title: "Instant try-out",
      subtitle:
        "Test how tables, diagrams and Markdown are read in your browser. Nothing leaves this machine.",
    },
    system: {
      title: "Status",
      subtitle:
        "Speed and accuracy across 20 PDFs and 316 pages, plus the local document tools installed here.",
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

  function applyTheme(theme) {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("docsmith-theme", theme);
    var button = document.getElementById("theme-toggle");
    var label = document.getElementById("theme-label");
    var meta = document.querySelector('meta[name="theme-color"]');
    if (button) {
      button.setAttribute("aria-pressed", theme === "dark" ? "true" : "false");
      var use = button.querySelector("use");
      if (use) use.setAttribute("href", theme === "dark" ? "#i-sun" : "#i-moon");
    }
    if (label) label.textContent = theme === "dark" ? "Light mode" : "Dark mode";
    if (meta) meta.setAttribute("content", theme === "dark" ? "#0b1417" : "#e8f1f2");
  }

  function initTheme() {
    var saved = localStorage.getItem("docsmith-theme");
    var preferred =
      saved ||
      (window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light");
    applyTheme(preferred === "dark" ? "dark" : "light");
  }

  function toggleTheme() {
    applyTheme(
      document.documentElement.dataset.theme === "dark" ? "light" : "dark"
    );
  }

  function formatDuration(seconds) {
    if (!isFinite(seconds) || seconds <= 0) return "";
    if (seconds < 60) return Math.ceil(seconds) + "s";
    var minutes = Math.floor(seconds / 60);
    var rest = Math.round(seconds % 60);
    return minutes + "m " + rest + "s";
  }

  function isTerminal(state) {
    return Boolean(TERMINAL_STATES[state]);
  }

  function copyText(text, message) {
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text).then(
        function () {
          showToast(message || "Copied to clipboard");
        },
        function () {
          fallbackCopy(text, message);
        }
      );
    } else {
      fallbackCopy(text, message);
    }
  }

  function fallbackCopy(text, message) {
    var area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    try {
      document.execCommand("copy");
      showToast(message || "Copied to clipboard");
    } catch (_) {
      showToast("Could not copy; select the text manually");
    }
    document.body.removeChild(area);
  }

  function downloadText(name, text, mime) {
    var blob = new Blob([text], { type: mime || "text/plain;charset=utf-8" });
    var url = URL.createObjectURL(blob);
    var link = document.createElement("a");
    link.href = url;
    link.download = name;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
    setTimeout(function () {
      URL.revokeObjectURL(url);
    }, 1500);
  }

  function showToast(message, actionLabel, action) {
    var region = document.getElementById("toast-region");
    if (!region) return null;
    var toast = document.createElement("div");
    toast.className = "toast";
    var text = document.createElement("span");
    text.className = "toast-message";
    text.textContent = message;
    toast.appendChild(text);
    if (actionLabel && action) {
      var actionBtn = document.createElement("button");
      actionBtn.className = "toast-action";
      actionBtn.textContent = actionLabel;
      actionBtn.addEventListener("click", function () {
        action();
        dismiss();
      });
      toast.appendChild(actionBtn);
    }
    var close = document.createElement("button");
    close.className = "toast-close";
    close.setAttribute("aria-label", "Dismiss notification");
    close.innerHTML = "<svg class='icon'><use href='#i-x'/></svg>";
    close.addEventListener("click", dismiss);
    toast.appendChild(close);
    region.appendChild(toast);
    var timer = setTimeout(dismiss, actionLabel ? 7500 : 4200);
    function dismiss() {
      clearTimeout(timer);
      if (toast.parentNode) toast.parentNode.removeChild(toast);
    }
    return toast;
  }

  function csvEscape(value) {
    var text = String(value == null ? "" : value);
    if (/[",\n\r]/.test(text)) {
      return '"' + text.replace(/"/g, '""') + '"';
    }
    return text;
  }

  function tableGrid(block) {
    var table = block.table;
    if (!table) return [];
    var byRow = {};
    (table.cells || []).forEach(function (cell) {
      if (!byRow[cell.row]) byRow[cell.row] = {};
      byRow[cell.row][cell.column] = cell.raw || "";
    });
    var rows = Object.keys(byRow)
      .map(Number)
      .sort(function (a, b) {
        return a - b;
      });
    var grid = [];
    var previous = -1;
    rows.forEach(function (row) {
      // Preserve empty rows in the CSV instead of silently closing the gap.
      for (var missing = previous + 1; missing < row; missing++) {
        grid.push([]);
      }
      var line = [];
      for (var col = 0; col < table.columns; col++) {
        line.push(byRow[row][col] != null ? byRow[row][col] : "");
      }
      grid.push(line);
      previous = row;
    });
    return grid;
  }

  function tableCsv(block) {
    return tableGrid(block)
      .map(function (row) {
        return row.map(csvEscape).join(",");
      })
      .join("\r\n");
  }

  function unitPlainText(unit) {
    if (!unit) return "";
    var parts = [];
    (unit.blocks || []).forEach(function (block) {
      if (block.table) {
        var grid = tableGrid(block);
        if (grid.length) {
          parts.push(
            grid
              .map(function (row) {
                return row.join("\t");
              })
              .join("\n")
          );
        }
      } else if (block.text) {
        parts.push(block.text);
      }
    });
    return parts.join("\n\n").trim();
  }

  function unitReview(unitId) {
    return (state.review.units && state.review.units[unitId]) || {};
  }

  function reviewCorrectionCount() {
    var count = 0;
    var units = state.review.units || {};
    Object.keys(units).forEach(function (unitId) {
      var blocks = units[unitId].blocks || {};
      count += Object.keys(blocks).length;
      var cells = units[unitId].cells || {};
      Object.keys(cells).forEach(function (blockId) {
        count += Object.keys(cells[blockId] || {}).length;
      });
    });
    return count;
  }

  function reviewHasCorrections() {
    return reviewCorrectionCount() > 0;
  }

  function unitQuality(unit) {
    var chars = 0;
    var tables = 0;
    var ocrWeighted = 0;
    var ocrWords = 0;
    var ocrBlocks = 0;
    var lowBlocks = 0;
    (unit.blocks || []).forEach(function (block) {
      if (block.table) {
        tables += 1;
        (block.table.cells || []).forEach(function (cell) {
          chars += (cell.raw || "").length;
        });
      } else if (block.text) {
        chars += block.text.length;
      }
      var attributes = block.attributes || {};
      if (typeof attributes.ocr_score_uncalibrated === "number") {
        var words = Math.max(1, Number(attributes.words) || 1);
        ocrWeighted += attributes.ocr_score_uncalibrated * words;
        ocrWords += words;
        ocrBlocks += 1;
        if (attributes.ocr_score_uncalibrated < 70) lowBlocks += 1;
      }
    });
    var notes = [
      "Scanned page:",
      "Embedded images retained;",
      "Poppler reading order",
    ];
    var warnings = (unit.warnings || []).filter(function (warning) {
      return !notes.some(function (prefix) {
        return warning.indexOf(prefix) === 0;
      });
    }).length;
    var errors = (unit.errors || []).length;
    var method = (unit.timing && unit.timing.method) || (ocrBlocks ? "ocr" : "native");
    var confidence = ocrWords ? Math.round(ocrWeighted / ocrWords) : null;
    var level = "good";
    var label = method === "native" ? "Native text" : "Text";
    if (confidence !== null) {
      label = "OCR ~" + confidence + "%";
      if (confidence < 60) level = "poor";
      else if (confidence < 78 || lowBlocks > 0) level = "check";
    }
    if (!chars && !tables) {
      if (method === "ocr") {
        level = "poor";
        label = "No text found";
      } else {
        label = "Blank page";
      }
    } else if (chars > 0 && chars < 24 && level === "good") {
      level = "check";
    }
    if (warnings && level === "good") level = "check";
    if (errors) {
      level = "poor";
      label = "Read error";
    }
    var reviewed = unitReview(unit.id).status === "reviewed";
    return {
      level: level,
      label: label,
      confidence: confidence,
      lowBlocks: lowBlocks,
      ocrBlocks: ocrBlocks,
      chars: chars,
      tables: tables,
      warnings: warnings,
      errors: errors,
      method: method,
      reviewed: reviewed,
    };
  }

  function qualityBadgeHtml(quality, includeVerified) {
    var html =
      "<span class='quality-badge quality-" +
      quality.level +
      "'>" +
      escapeHtml(quality.label) +
      "</span>";
    if (includeVerified && quality.reviewed) {
      html +=
        "<span class='quality-badge quality-verified'>Verified</span>";
    }
    return html;
  }

  function applyReviewToDocument(doc, review) {
    state.review =
      review && review.units && typeof review.units === "object"
        ? review
        : { schema_version: "review1", units: {} };
    state.reviewOriginals = {};
    state.reviewOriginalCells = {};
    (doc.units || []).forEach(function (unit) {
      var reviewed = state.review.units[unit.id];
      if (!reviewed) return;
      (unit.blocks || []).forEach(function (block) {
        var correction = reviewed.blocks && reviewed.blocks[block.id];
        if (correction && typeof correction.text === "string") {
          state.reviewOriginals[block.id] =
            typeof correction.original === "string"
              ? correction.original
              : block.text;
          block.text = correction.text;
          block.attributes = Object.assign({}, block.attributes || {}, {
            reviewed: true,
          });
        }
        var cells = reviewed.cells && reviewed.cells[block.id];
        if (!cells || !block.table) return;
        (block.table.cells || []).forEach(function (cell) {
          var entry = cells[cell.row + "," + cell.column];
          if (!entry || typeof entry.text !== "string") return;
          var key = block.id + ":" + cell.row + ":" + cell.column;
          state.reviewOriginalCells[key] =
            typeof entry.original === "string" ? entry.original : cell.raw;
          cell.raw = entry.text;
          cell.value = entry.text;
        });
      });
    });
  }

  function patchReview(payload) {
    if (!state.currentJobId) {
      return Promise.reject(new Error("No document is open"));
    }
    return fetch("/v1/extractions/" + encodeURIComponent(state.currentJobId), {
      method: "PATCH",
      headers: Object.assign({ "Content-Type": "application/json" }, authHeaders()),
      body: JSON.stringify(payload),
    }).then(function (response) {
      return response.json().then(function (body) {
        if (!response.ok) {
          throw new Error(
            (body.error && body.error.message) || "Could not save the review"
          );
        }
        return body;
      });
    });
  }

  function saveBlockCorrection(block, text) {
    if (!state.currentUnit || state.reviewBusy) return;
    var unitId = state.currentUnit.id;
    state.reviewBusy = true;
    var original = state.reviewOriginals[block.id];
    if (original == null) original = block.text;
    patchReview({
      unit_id: unitId,
      block_id: block.id,
      text: text,
      original: original,
      status: "needs_review",
    })
      .then(function (review) {
        state.review = review;
        invalidateRedactionScan();
        state.reviewOriginals[block.id] = original;
        findBlock(unitId, block.id).text = text;
        findBlock(unitId, block.id).attributes = Object.assign(
          {},
          findBlock(unitId, block.id).attributes || {},
          { reviewed: true }
        );
        updateExportLinks(state.currentJobId);
        renderCurrentUnit();
        renderPageFilmstrip();
        showToast("Correction saved");
      })
      .catch(function (error) {
        showToast("Could not save the correction: " + String(error.message || error));
      })
      .finally(function () {
        state.reviewBusy = false;
      });
  }

  function revertBlockCorrection(block) {
    if (!state.currentUnit || state.reviewBusy) return;
    var unitId = state.currentUnit.id;
    state.reviewBusy = true;
    patchReview({
      unit_id: unitId,
      block_id: block.id,
      revert: true,
    })
      .then(function (review) {
        state.review = review;
        invalidateRedactionScan();
        var target = findBlock(unitId, block.id);
        var original = state.reviewOriginals[block.id];
        if (original != null) target.text = original;
        if (target.attributes) delete target.attributes.reviewed;
        delete state.reviewOriginals[block.id];
        updateExportLinks(state.currentJobId);
        renderCurrentUnit();
        renderPageFilmstrip();
        showToast("Original text restored");
      })
      .catch(function (error) {
        showToast("Could not revert: " + String(error.message || error));
      })
      .finally(function () {
        state.reviewBusy = false;
      });
  }

  function saveCellCorrection(block, row, column, cell, text) {
    if (!state.currentUnit || state.reviewBusy) return;
    var unitId = state.currentUnit.id;
    state.reviewBusy = true;
    var key = block.id + ":" + row + ":" + column;
    var original = state.reviewOriginalCells[key];
    if (original == null) original = cell.raw;
    patchReview({
      unit_id: unitId,
      block_id: block.id,
      cell_row: row,
      cell_column: column,
      text: text,
      original: original,
      status: "needs_review",
    })
      .then(function (review) {
        state.review = review;
        invalidateRedactionScan();
        state.reviewOriginalCells[key] = original;
        var target = findCell(unitId, block.id, row, column);
        target.raw = text;
        target.value = text;
        updateExportLinks(state.currentJobId);
        renderCurrentUnit();
        renderPageFilmstrip();
        showToast("Cell corrected");
      })
      .catch(function (error) {
        showToast("Could not save the cell: " + String(error.message || error));
      })
      .finally(function () {
        state.reviewBusy = false;
      });
  }

  function revertCellCorrection(block, row, column) {
    if (!state.currentUnit || state.reviewBusy) return;
    var unitId = state.currentUnit.id;
    state.reviewBusy = true;
    var key = block.id + ":" + row + ":" + column;
    patchReview({
      unit_id: unitId,
      block_id: block.id,
      cell_row: row,
      cell_column: column,
      revert: true,
    })
      .then(function (review) {
        state.review = review;
        invalidateRedactionScan();
        var target = findCell(unitId, block.id, row, column);
        var original = state.reviewOriginalCells[key];
        if (original != null) {
          target.raw = original;
          target.value = original;
        }
        delete state.reviewOriginalCells[key];
        updateExportLinks(state.currentJobId);
        renderCurrentUnit();
        renderPageFilmstrip();
        showToast("Cell restored");
      })
      .catch(function (error) {
        showToast("Could not revert the cell: " + String(error.message || error));
      })
      .finally(function () {
        state.reviewBusy = false;
      });
  }

  function findCell(unitId, blockId, row, column) {
    var block = findBlock(unitId, blockId);
    return (
      (block.table &&
        block.table.cells.find(function (cell) {
          return cell.row === row && cell.column === column;
        })) || { raw: "", value: "" }
    );
  }

  function findBlock(unitId, blockId) {
    var unit = (state.currentDoc.units || []).find(function (candidate) {
      return candidate.id === unitId;
    });
    return (
      (unit && unit.blocks.find(function (block) {
        return block.id === blockId;
      })) || { text: "", attributes: {} }
    );
  }

  function saveReviewStatus(unitId, status) {
    if (state.reviewBusy) return;
    state.reviewBusy = true;
    patchReview({ unit_id: unitId, status: status })
      .then(function (review) {
        state.review = review;
        updateExportLinks(state.currentJobId);
        renderCurrentUnit();
        renderPageFilmstrip();
        showToast(status === "reviewed" ? "Page marked verified" : "Page reopened for review");
      })
      .catch(function (error) {
        showToast("Could not update the review: " + String(error.message || error));
      })
      .finally(function () {
        state.reviewBusy = false;
      });
  }

  function renderReviewPanel(unit) {
    var summary = document.getElementById("review-summary");
    var list = document.getElementById("review-pages");
    var count = document.getElementById("review-count");
    var pageState = document.getElementById("review-page-state");
    var markBtn = document.getElementById("mark-page-reviewed-btn");
    if (!summary || !list) return;
    var units = (state.currentDoc && state.currentDoc.units) || [];
    var flagged = 0;
    var verified = 0;
    var corrections = reviewCorrectionCount();
    var rows = units.map(function (page) {
      var quality = unitQuality(page);
      if (quality.level !== "good" && !quality.reviewed) flagged += 1;
      if (quality.reviewed) verified += 1;
      return { unit: page, quality: quality };
    });
    rows.sort(function (a, b) {
      var rank = { poor: 0, check: 1, good: 2 };
      return rank[a.quality.level] - rank[b.quality.level] || a.unit.index - b.unit.index;
    });
    summary.innerHTML =
      "<div class='review-stat'><strong>" +
      units.length +
      "</strong><span>Pages</span></div>" +
      "<div class='review-stat'><strong>" +
      flagged +
      "</strong><span>Need attention</span></div>" +
      "<div class='review-stat'><strong>" +
      verified +
      "</strong><span>Verified</span></div>" +
      "<div class='review-stat'><strong>" +
      corrections +
      "</strong><span>Corrections</span></div>";
    if (count) {
      count.textContent = String(flagged);
      count.classList.toggle("hidden", flagged === 0);
    }
    if (unit) {
      var current = unitReview(unit.id);
      if (pageState) {
        pageState.textContent =
          current.status === "reviewed"
            ? "This page is verified."
            : "Not verified yet.";
      }
      if (markBtn) {
        markBtn.innerHTML =
          current.status === "reviewed"
            ? "<svg class='icon'><use href='#i-check'/></svg> Reopen page"
            : "<svg class='icon'><use href='#i-check'/></svg> Mark this page verified";
      }
    }
    if (state.activeSubtab !== "review") {
      list.innerHTML = "";
      return;
    }
    list.innerHTML = "";
    rows.forEach(function (row) {
      var page = row.unit;
      var quality = row.quality;
      var review = unitReview(page.id);
      var edited = review.blocks ? Object.keys(review.blocks).length : 0;
      Object.keys(review.cells || {}).forEach(function (blockId) {
        edited += Object.keys(review.cells[blockId] || {}).length;
      });
      var item = document.createElement("div");
      item.className =
        "review-page-row" + (unit && page.id === unit.id ? " is-current" : "");
      var main = document.createElement("div");
      main.className = "review-page-main";
      main.innerHTML =
        "<strong>" +
        escapeHtml(page.name) +
        " " +
        qualityBadgeHtml(quality, false) +
        "</strong><small>" +
        escapeHtml(
          quality.label +
            (quality.level !== "good" ? " · needs a look" : "") +
            " · " +
            (quality.chars ? quality.chars.toLocaleString() + " chars" : "no text") +
            (quality.tables ? " · " + quality.tables + " table(s)" : "") +
            (quality.lowBlocks ? " · " + quality.lowBlocks + " low-confidence block(s)" : "") +
            (edited ? " · " + edited + " correction(s)" : "") +
            (quality.warnings ? " · " + quality.warnings + " note(s)" : "")
        ) +
        "</small>";
      main.addEventListener("click", function () {
        selectUnit(page.id);
      });
      item.appendChild(main);
      var actions = document.createElement("div");
      actions.className = "review-page-actions";
      if (quality.reviewed) {
        var verifiedBadge = document.createElement("span");
        verifiedBadge.className = "quality-badge quality-verified";
        verifiedBadge.textContent = "Verified";
        actions.appendChild(verifiedBadge);
      } else {
        var verify = document.createElement("button");
        verify.className = "btn btn-sm";
        verify.textContent = "Verify";
        verify.title = "Mark this page as reviewed";
        verify.addEventListener("click", function (event) {
          event.stopPropagation();
          saveReviewStatus(page.id, "reviewed");
        });
        actions.appendChild(verify);
      }
      var open = document.createElement("button");
      open.className = "btn btn-sm";
      open.textContent = "Open";
      open.addEventListener("click", function (event) {
        event.stopPropagation();
        selectUnit(page.id);
        switchSubtab("blocks");
      });
      actions.appendChild(open);
      item.appendChild(actions);
      list.appendChild(item);
    });
  }

  function reviewNextFlagged() {
    if (!state.currentDoc) return;
    var next = (state.currentDoc.units || []).find(function (unit) {
      var quality = unitQuality(unit);
      return quality.level !== "good" && !quality.reviewed;
    });
    if (!next) {
      var unverified = (state.currentDoc.units || []).find(function (unit) {
        return unitReview(unit.id).status !== "reviewed";
      });
      next = unverified;
    }
    if (next) {
      selectUnit(next.id);
      switchSubtab("blocks");
      showToast("Opened " + next.name + " for review");
    } else {
      showToast("Every page is verified");
    }
  }

  function redactionKey(item) {
    return [
      item.unit_id,
      item.block_id,
      item.row == null ? "" : item.row,
      item.column == null ? "" : item.column,
      item.value,
    ].join("|");
  }

  function invalidateRedactionScan() {
    state.redactionJobId = "";
    state.redactionMatches = [];
    state.redactionSaved = [];
    state.redactionSelected = {};
  }

  function loadRedactions(force) {
    if (!state.currentJobId) return;
    if (
      !force &&
      state.redactionJobId === state.currentJobId &&
      (state.redactionMatches.length || state.redactionSaved.length)
    ) {
      renderRedactionList();
      return;
    }
    var status = document.getElementById("redaction-status");
    if (status) status.textContent = "Scanning…";
    apiGet(
      "/v1/extractions/" +
        encodeURIComponent(state.currentJobId) +
        "/result?format=redactions"
    )
      .then(function (data) {
        state.redactionJobId = state.currentJobId;
        state.redactionMatches = data.matches || [];
        state.redactionSaved = data.saved || [];
        var savedKeys = {};
        state.redactionSaved.forEach(function (item) {
          savedKeys[redactionKey(item)] = true;
        });
        state.redactionSelected = {};
        state.redactionMatches.forEach(function (item, index) {
          state.redactionSelected[index] =
            state.redactionSaved.length === 0 || savedKeys[redactionKey(item)];
        });
        updateRedactedLinks();
        renderRedactionList();
      })
      .catch(function () {
        if (status) status.textContent = "Scan unavailable";
        showToast("Could not scan for identifiers");
      });
  }

  function renderRedactionList() {
    var list = document.getElementById("redaction-list");
    var status = document.getElementById("redaction-status");
    var save = document.getElementById("save-redactions-btn");
    if (!list) return;
    if (status) {
      status.textContent =
        (state.redactionSaved.length
          ? state.redactionSaved.length + " saved"
          : "Not saved") +
        " · " +
        state.redactionMatches.length +
        (state.redactionMatches.length === 1 ? " match" : " matches");
    }
    if (save) save.disabled = !state.redactionMatches.length;
    list.innerHTML = "";
    if (!state.redactionMatches.length) {
      list.innerHTML =
        "<p class='muted'>No obvious identifiers found. This is not a guarantee; review the page before sharing.</p>";
      return;
    }
    state.redactionMatches.forEach(function (item, index) {
      var row = document.createElement("label");
      row.className = "redaction-item";
      var box = document.createElement("input");
      box.type = "checkbox";
      box.checked = Boolean(state.redactionSelected[index]);
      box.addEventListener("change", function () {
        state.redactionSelected[index] = box.checked;
      });
      var copy = document.createElement("span");
      copy.className = "redaction-copy";
      copy.innerHTML =
        "<strong><span class='redaction-category'>" +
        escapeHtml(item.category) +
        "</span> " +
        escapeHtml(item.preview) +
        "</strong><small>" +
        escapeHtml((item.unit_name || item.unit_id) + " · " + item.context) +
        "</small>";
      row.appendChild(box);
      row.appendChild(copy);
      list.appendChild(row);
    });
  }

  function selectedRedactions() {
    var seen = {};
    var specs = [];
    state.redactionMatches.forEach(function (item, index) {
      if (!state.redactionSelected[index]) return;
      var key = redactionKey(item);
      if (seen[key]) return;
      seen[key] = true;
      specs.push({
        unit_id: item.unit_id,
        block_id: item.block_id,
        row: item.row,
        column: item.column,
        category: item.category,
        value: item.value,
      });
    });
    return specs;
  }

  function saveRedactions() {
    if (!state.currentJobId) return;
    var specs = selectedRedactions();
    fetch(
      "/v1/extractions/" + encodeURIComponent(state.currentJobId) + "?redaction=true",
      {
        method: "PATCH",
        headers: Object.assign({ "Content-Type": "application/json" }, authHeaders()),
        body: JSON.stringify({ redactions: specs }),
      }
    )
      .then(function (response) {
        return response.json().then(function (body) {
          if (!response.ok) {
            throw new Error(
              (body.error && body.error.message) || "Could not save redactions"
            );
          }
          return body;
        });
      })
      .then(function (body) {
        state.redactionSaved = body.redactions || [];
        updateRedactedLinks();
        renderRedactionList();
        showToast(
          state.redactionSaved.length
            ? state.redactionSaved.length + " redaction(s) saved"
            : "No redactions selected"
        );
      })
      .catch(function (error) {
        showToast(String(error.message || error));
      });
  }

  function clearRedactions() {
    if (!state.currentJobId) return;
    fetch(
      "/v1/extractions/" + encodeURIComponent(state.currentJobId) + "?redaction=true",
      {
        method: "PATCH",
        headers: Object.assign({ "Content-Type": "application/json" }, authHeaders()),
        body: JSON.stringify({ clear_redactions: true }),
      }
    )
      .then(function (response) {
        if (!response.ok) throw new Error("clear failed");
        return response.json();
      })
      .then(function () {
        state.redactionSaved = [];
        updateRedactedLinks();
        renderRedactionList();
        showToast("Saved redactions cleared");
      })
      .catch(function () {
        showToast("Could not clear redactions");
      });
  }

  function updateRedactedLinks() {
    if (!state.currentJobId) return;
    var base = "/v1/extractions/" + encodeURIComponent(state.currentJobId);
    var suffix = "&redact=true";
    var paths = {
      "dl-redacted-md": "/result?format=markdown" + suffix,
      "dl-redacted-txt": "/result?format=text" + suffix,
      "dl-redacted-report": "/result?format=report" + suffix,
    };
    var disabled = state.redactionSaved.length === 0;
    Object.keys(paths).forEach(function (id) {
      var link = document.getElementById(id);
      if (!link) return;
      link.href = base + paths[id];
      link.classList.toggle("is-disabled", disabled);
      link.setAttribute("aria-disabled", disabled ? "true" : "false");
      link.title = disabled ? "Save at least one redaction first" : "";
    });
  }

  function selectAllRedactions() {
    var anyUnchecked = state.redactionMatches.some(function (item, index) {
      return !state.redactionSelected[index];
    });
    state.redactionMatches.forEach(function (item, index) {
      state.redactionSelected[index] = anyUnchecked;
    });
    renderRedactionList();
  }

  function fileNameForUnit(unit, extension) {
    var stem = (state.currentJobMeta && state.currentJobMeta.filename) || "document";
    stem = stem.replace(/\.[^.]+$/, "").replace(/[^\w.-]+/g, "-");
    var page = (unit && (unit.name || unit.id)) || "page";
    page = page.toLowerCase().replace(/[^\w.-]+/g, "-");
    return stem + "-" + page + "." + extension;
  }

  function tabPath(tabName) {
    if (tabName === "inspector") {
      return state.currentJobId
        ? "/reader/" + encodeURIComponent(state.currentJobId) + "/" + encodeURIComponent(state.currentUnitId || "u1")
        : "/reader";
    }
    if (tabName === "pages") {
      return state.currentJobId
        ? "/pages/" + encodeURIComponent(state.currentJobId)
        : "/pages";
    }
    if (tabName === "playground") return "/try";
    if (tabName === "system") return "/status";
    return "/documents";
  }

  function setRoute(path, replace) {
    if (state.applyingRoute) return;
    var target = "#" + path;
    if (location.hash === target) return;
    try {
      if (replace) {
        history.replaceState(null, "", target);
      } else {
        history.pushState(null, "", target);
      }
    } catch (_) {
      location.hash = target;
    }
  }

  function switchTab(tabName, options) {
    options = options || {};
    var changed = state.activeTab !== tabName;
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
    if (!options.skipRoute) {
      setRoute(tabPath(tabName), options.replace === true);
    }
    if (changed && !options.keepScroll) {
      window.scrollTo({ top: 0, behavior: "auto" });
    }
  }

  function applyRoute() {
    var hash = location.hash || "";
    if (!hash) return;
    var parts = hash.replace(/^#\/?/, "").split("/");
    var head = parts[0] || "documents";
    state.applyingRoute = true;
    try {
      if (head === "reader") {
        switchTab("inspector", { skipRoute: true });
        if (parts[1]) {
          state.pendingUnitId = parts[2] || "";
          selectJobForInspection(decodeURIComponent(parts[1]), false, parts[2] || "");
        }
      } else if (head === "pages") {
        switchTab("pages", { skipRoute: true });
        if (parts[1]) {
          selectJobForInspection(decodeURIComponent(parts[1]), false);
        }
      } else if (head === "try") {
        switchTab("playground", { skipRoute: true });
      } else if (head === "status") {
        switchTab("system", { skipRoute: true });
      } else {
        switchTab("studio", { skipRoute: true });
      }
    } finally {
      state.applyingRoute = false;
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
    if (subName === "review" && state.currentUnit) {
      renderReviewPanel(state.currentUnit);
    }
    if (subName === "formats" && state.currentJobId) {
      loadRedactions(false);
    }
  }

  // ==================== COMMAND PALETTE / SEARCH ====================
  var searchReturnFocus = null;

  function openSearch() {
    var overlay = document.getElementById("search-overlay");
    if (!overlay) return;
    searchReturnFocus = document.activeElement;
    overlay.classList.remove("hidden");
    var input = document.getElementById("search-input");
    input.value = "";
    state.searchResults = [];
    state.searchSelection = 0;
    runSearch("");
    setTimeout(function () {
      input.focus();
    }, 20);
  }

  function closeSearch() {
    var overlay = document.getElementById("search-overlay");
    if (!overlay || overlay.classList.contains("hidden")) return;
    overlay.classList.add("hidden");
    if (state.searchAbort) {
      state.searchAbort.abort();
      state.searchAbort = null;
    }
    if (searchReturnFocus && searchReturnFocus.focus) {
      try {
        searchReturnFocus.focus();
      } catch (_) {}
    }
  }

  function scheduleSearch(query) {
    clearTimeout(state.searchTimer);
    state.searchTimer = setTimeout(function () {
      runSearch(query);
    }, 160);
  }

  function renderRecentSearchResults() {
    state.searchResults = state.jobs.slice(0, 8).map(function (job) {
      return {
        type: "doc",
        job_id: job.id,
        filename: job.filename,
        display_name: job.display_name,
        state: job.state,
        snippet:
          job.total > 0
            ? job.progress + " of " + job.total + " pages or sections"
            : job.state === "completed"
            ? "No pages or sections extracted"
            : job.state,
      };
    });
    state.searchSelection = 0;
    renderSearchResults(
      "No documents yet. Add files in Documents, then search their extracted text here."
    );
  }

  function runSearch(query) {
    var trimmed = (query || "").trim();
    if (state.searchAbort) state.searchAbort.abort();
    if (!trimmed) {
      if (state.jobs.length) {
        renderRecentSearchResults();
      } else {
        apiGet("/v1/extractions?limit=8")
          .then(function (data) {
            if ((document.getElementById("search-input").value || "").trim()) return;
            state.jobs = data.jobs || [];
            renderRecentSearchResults();
          })
          .catch(function () {
            if ((document.getElementById("search-input").value || "").trim()) return;
            renderRecentSearchResults();
          });
      }
      return;
    }
    if (trimmed.length < 2) {
      state.searchResults = [];
      state.searchSelection = 0;
      renderSearchResults("Type at least two characters, or press Enter to open the first document.");
      return;
    }
    var controller = new AbortController();
    state.searchAbort = controller;
    var results = document.getElementById("search-results");
    if (results) {
      results.innerHTML = "<div class='palette-empty'>Searching extracted text…</div>";
    }
    fetch(
      "/v1/extractions?limit=30&q=" + encodeURIComponent(trimmed),
      { headers: authHeaders(), signal: controller.signal }
    )
      .then(function (response) {
        if (!response.ok) throw new Error("search failed");
        return response.json();
      })
      .then(function (data) {
        if (controller.signal.aborted) return;
        var items = [];
        var lowered = trimmed.toLowerCase();
        (data.jobs || []).forEach(function (job) {
          if ((job.filename || "").toLowerCase().indexOf(lowered) !== -1) {
            items.push({
              type: "doc",
              job_id: job.id,
              filename: job.filename,
              display_name: job.display_name,
              state: job.state,
              snippet:
                job.total > 0
                  ? job.progress + " of " + job.total + " pages or sections"
                  : job.state === "completed"
                  ? "No pages or sections extracted"
                  : job.state,
            });
          }
        });
        (data.matches || []).forEach(function (match) {
          items.push({
            type: "match",
            job_id: match.job_id,
            filename: match.filename,
            display_name: match.display_name,
            unit_id: match.unit_id,
            unit_name: match.unit_name,
            page: match.page,
            block_id: match.block_id,
            kind: match.kind,
            snippet: match.snippet,
          });
        });
        state.searchResults = items;
        state.searchSelection = 0;
        renderSearchResults();
      })
      .catch(function (error) {
        if (error && error.name === "AbortError") return;
        if (results) {
          results.innerHTML =
            "<div class='palette-empty'>Search is unavailable right now. Try again.</div>";
        }
      });
  }

  function renderSearchResults(emptyMessage) {
    var box = document.getElementById("search-results");
    if (!box) return;
    box.innerHTML = "";
    if (!state.searchResults.length) {
      box.innerHTML =
        "<div class='palette-empty'>" +
        escapeHtml(emptyMessage || "No matches yet. Try a filename, a phrase or a table value.") +
        "</div>";
      return;
    }
    var currentGroup = "";
    state.searchResults.forEach(function (item, index) {
      var group = item.type === "match" ? "In extracted text" : "Documents";
      if (group !== currentGroup) {
        currentGroup = group;
        var label = document.createElement("div");
        label.className = "palette-group-label";
        label.textContent = group;
        box.appendChild(label);
      }
      var button = document.createElement("button");
      button.type = "button";
      button.className =
        "palette-result" + (index === state.searchSelection ? " is-selected" : "");
      button.setAttribute("role", "option");
      button.setAttribute("aria-selected", index === state.searchSelection ? "true" : "false");
      button.setAttribute("data-index", String(index));
      var icon = item.type === "match"
        ? item.kind === "table"
          ? "i-table"
          : item.block_id
          ? "i-book"
          : "i-file"
        : "i-file";
      var label = item.display_name || item.filename;
      var title = item.type === "match"
        ? escapeHtml(label) +
          " <span>" +
          escapeHtml(item.unit_name || "Page " + item.page) +
          "</span>"
        : escapeHtml(label) + " <span>" + escapeHtml(item.state || "") + "</span>";
      var snippet = item.type === "match" && item.snippet ? item.snippet : item.snippet || "";
      button.innerHTML =
        "<span class='palette-result-icon'><svg class='icon'><use href='#" +
        icon +
        "'/></svg></span>" +
        "<span class='palette-result-copy'><span class='palette-result-title'>" +
        title +
        "</span><span class='palette-result-snippet'>" +
        escapeHtml(snippet) +
        "</span></span>";
      button.addEventListener("mouseenter", function () {
        state.searchSelection = index;
        highlightSearchSelection();
      });
      button.addEventListener("click", function () {
        openSearchResult(item);
      });
      box.appendChild(button);
    });
    highlightSearchSelection();
  }

  function highlightSearchSelection() {
    var items = document.querySelectorAll("#search-results .palette-result");
    items.forEach(function (el, index) {
      var selected = index === state.searchSelection;
      el.classList.toggle("is-selected", selected);
      el.setAttribute("aria-selected", selected ? "true" : "false");
      if (selected) el.scrollIntoView({ block: "nearest" });
    });
  }

  function moveSearchSelection(delta) {
    if (!state.searchResults.length) return;
    state.searchSelection = Math.max(
      0,
      Math.min(state.searchResults.length - 1, state.searchSelection + delta)
    );
    highlightSearchSelection();
  }

  function openSearchResult(item) {
    closeSearch();
    if (!item) return;
    window.scrollTo({ top: 0, behavior: "auto" });
    state.provFilter = "all";
    document.querySelectorAll(".filter-chip").forEach(function (chip) {
      chip.classList.toggle("is-active", chip.getAttribute("data-prov") === "all");
    });
    switchSubtab(item.type === "match" && item.kind === "table" ? "tables" : "blocks");
    if (item.type === "match") {
      state.pendingBlockId = item.block_id || "";
      selectJobForInspection(item.job_id, true, item.unit_id, item.block_id || "");
    } else {
      selectJobForInspection(item.job_id, true, "", "");
    }
  }

  // ==================== TELEMETRY & CAPABILITIES ====================
  function refreshTelemetry() {
    apiGet("/v1/stats")
      .then(function (stats) {
        var w = stats.workers || { active: 0, slots: 0 };
        document.getElementById("stat-workers").textContent = w.active + " / " + w.slots;
        var j = stats.jobs || {};
        document.getElementById("stat-jobs").textContent = String(j.completed || 0);
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
            title: "PDF reader and page images",
            ok: Boolean(caps.pdf && caps.pdf.available),
            detail: "Poppler backend · 150 DPI page rasters",
          },
          {
            title: "Scanned text (OCR)",
            ok: Boolean(caps.ocr && (caps.ocr.tesseract || caps.ocr.apple_vision || caps.ocr.smoldocling || caps.ocr.paddle_onnx)),
            detail:
              (caps.ocr && caps.ocr.tesseract ? "Tesseract" : "No engine") +
              " · languages: " +
              ((caps.ocr && caps.ocr.languages && caps.ocr.languages.join(", ")) || "none"),
          },
          {
            title: "Apple Vision OCR (macOS)",
            ok: Boolean(caps.ocr && caps.ocr.apple_vision),
            detail:
              caps.ocr && caps.ocr.apple_vision
                ? "Installed and ready"
                : "Optional; not enabled",
          },
          {
            title: "Office, sheets and diagrams",
            ok: true,
            detail: "Word, Excel, PowerPoint, CSV, HTML, Markdown, Mermaid, DBML, draw.io",
          },
          {
            title: "In-browser Rust and WASM",
            ok: state.wasmReady,
            detail: state.wasmReady
              ? "Active: instant previews and highlights"
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
        badge.textContent = "Options ready";
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

  function stageFiles(files) {
    var incoming = Array.from(files || []);
    if (!incoming.length) return;
    var existing = {};
    state.selectedFiles.forEach(function (file) {
      existing[file.name + ":" + file.size + ":" + file.lastModified] = true;
    });
    var added = 0;
    incoming.forEach(function (file) {
      var key = file.name + ":" + file.size + ":" + file.lastModified;
      if (existing[key]) return;
      if (state.selectedFiles.length >= 32) return;
      state.selectedFiles.push(file);
      existing[key] = true;
      added += 1;
    });
    if (!added && incoming.length) {
      showToast("Those files are already selected");
    } else if (state.selectedFiles.length >= 32 && incoming.length > added) {
      showToast("The upload limit is 32 files per batch");
    }
    renderSelectedFiles();
    validateStudioOptions();
  }

  function renderSelectedFiles() {
    var box = document.getElementById("selected-files");
    var title = document.getElementById("dropzone-title");
    var sub = document.getElementById("dropzone-sub");
    if (!box || !title || !sub) return;
    box.innerHTML = "";
    if (!state.selectedFiles.length) {
      title.textContent = "Drop files here, or choose from your computer";
      sub.textContent = "PDF · Word · Excel · PowerPoint · CSV · Images · HTML · Markdown · Diagrams";
      return;
    }
    var total = 0;
    state.selectedFiles.forEach(function (file, index) {
      total += file.size || 0;
      var chip = document.createElement("span");
      chip.className = "file-chip";
      var name = document.createElement("span");
      name.className = "file-name";
      name.textContent = file.name;
      name.title = file.name;
      var size = document.createElement("span");
      size.className = "file-size";
      size.textContent = formatBytes(file.size || 0);
      var remove = document.createElement("button");
      remove.className = "file-remove";
      remove.type = "button";
      remove.setAttribute("aria-label", "Remove " + file.name);
      remove.innerHTML = "<svg class='icon'><use href='#i-x'/></svg>";
      remove.addEventListener("click", function () {
        state.selectedFiles.splice(index, 1);
        renderSelectedFiles();
      });
      chip.appendChild(name);
      chip.appendChild(size);
      chip.appendChild(remove);
      box.appendChild(chip);
    });
    title.textContent =
      state.selectedFiles.length +
      (state.selectedFiles.length === 1 ? " file ready" : " files ready");
    sub.textContent =
      formatBytes(total) +
      " total · review the list below, then choose Extract selected files. Add more files at any time.";
  }

  function sendExtraction(form, label) {
    return new Promise(function (resolve, reject) {
      if (state.uploading) {
        reject(new Error("An upload is already in progress"));
        return;
      }
      state.uploading = true;
      var statusEl = document.getElementById("upload-status");
      var submit = document.getElementById("submit-upload-btn");
      var progress = document.getElementById("upload-progress");
      var bar = document.getElementById("upload-progress-bar");
      if (submit) submit.disabled = true;
      if (progress) {
        progress.classList.remove("hidden");
        progress.setAttribute("aria-valuenow", "0");
      }
      if (bar) bar.style.width = "0%";
      statusEl.textContent = label + "…";
      var xhr = new XMLHttpRequest();
      xhr.open("POST", "/v1/extractions");
      var headers = authHeaders();
      Object.keys(headers).forEach(function (key) {
        xhr.setRequestHeader(key, headers[key]);
      });
      xhr.upload.onprogress = function (event) {
        if (!event.lengthComputable) return;
        var pct = Math.round((event.loaded / event.total) * 100);
        if (bar) bar.style.width = pct + "%";
        if (progress) progress.setAttribute("aria-valuenow", String(pct));
        statusEl.textContent = "Uploading… " + pct + "%";
      };
      xhr.onload = function () {
        var body = null;
        try {
          body = JSON.parse(xhr.responseText || "{}");
        } catch (_) {
          body = {};
        }
        if (xhr.status >= 200 && xhr.status < 300) {
          if (bar) bar.style.width = "100%";
          resolve(body);
        } else {
          reject(
            new Error(
              (body.error && body.error.message) ||
                (body.error && body.error.code) ||
                "Upload failed (" + xhr.status + ")"
            )
          );
        }
      };
      xhr.onerror = function () {
        reject(new Error("The service could not be reached"));
      };
      xhr.onloadend = function () {
        state.uploading = false;
        if (submit) submit.disabled = false;
        setTimeout(function () {
          if (progress) progress.classList.add("hidden");
          if (bar) bar.style.width = "0%";
        }, 700);
      };
      xhr.send(form);
    });
  }

  function uploadFiles(files, clearSelection) {
    var chosen = Array.from(files || state.selectedFiles || []);
    if (!chosen.length) {
      document.getElementById("upload-status").textContent =
        "Choose at least one file, or drop them above.";
      showToast("Choose at least one file first");
      return;
    }
    if (chosen.length > 32) {
      chosen = chosen.slice(0, 32);
      showToast("Uploading the first 32 files");
    }
    var fd = new FormData();
    var opts = collectStudioOptions();
    fd.append("options", JSON.stringify(opts));
    chosen.forEach(function (file) {
      fd.append("file", file, file.name);
    });
    var label =
      chosen.length === 1
        ? "Reading " + chosen[0].name
        : "Reading " + chosen.length + " documents";
    sendExtraction(fd, label)
      .then(function (resp) {
        var count = resp.jobs ? resp.jobs.length : resp.id ? 1 : 0;
        var cached = Boolean(
          resp.cached ||
            (resp.jobs &&
              resp.jobs.some(function (job) {
                return job.cached;
              }))
        );
        var message =
          count +
          (count === 1 ? " document " : " documents ") +
          (cached ? "reused from a previous identical extraction" : "queued");
        document.getElementById("upload-status").textContent = message;
        showToast(message);
        if (clearSelection !== false) {
          state.selectedFiles = [];
          renderSelectedFiles();
          var input = document.getElementById("file-input");
          if (input) input.value = "";
        }
        refreshQueue();
        refreshTelemetry();
        var firstId = resp.jobs && resp.jobs[0] ? resp.jobs[0].id : resp.id;
        if (firstId) selectJobForInspection(firstId, false);
      })
      .catch(function (err) {
        document.getElementById("upload-status").textContent =
          "Could not upload: " + String(err.message || err);
        showToast("Upload failed: " + String(err.message || err));
      });
  }

  function requestNotifyPermission() {
    var box = document.getElementById("opt-notify");
    if (!box) return;
    localStorage.setItem("docsmith-notify", box.checked ? "1" : "0");
    state.notify = box.checked;
    if (box.checked && "Notification" in window && Notification.permission === "default") {
      Notification.requestPermission().then(function (permission) {
        if (permission !== "granted") {
          box.checked = false;
          state.notify = false;
          localStorage.setItem("docsmith-notify", "0");
          showToast("Desktop notifications were not enabled");
        }
      });
    }
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
    uploadFiles(files, false);
  }

  // ==================== LIBRARY ====================
  function jobDisplayName(job) {
    return (job && job.display_name) || (job && job.filename) || "Document";
  }

  function renderLibraryOverview() {
    var documents = document.getElementById("overview-documents");
    if (!documents) return;
    var pages = 0;
    var favorites = 0;
    var collections = {};
    var tags = {};
    state.jobs.forEach(function (job) {
      pages += job.total || job.progress || 0;
      if (job.favorite) favorites += 1;
      if (job.collection) collections[job.collection] = true;
      (job.tags || []).forEach(function (tag) {
        tags[tag] = true;
      });
    });
    documents.textContent = state.jobs.length;
    document.getElementById("overview-pages").textContent = pages;
    document.getElementById("overview-favorites").textContent = favorites;
    document.getElementById("overview-collections").textContent =
      Object.keys(collections).length;
    document.getElementById("overview-tags-note").textContent =
      Object.keys(tags).length + " tags in use";
  }

  function patchLibrary(jobId, updates) {
    return fetch(
      "/v1/extractions/" + encodeURIComponent(jobId) + "?library=true",
      {
        method: "PATCH",
        headers: Object.assign({ "Content-Type": "application/json" }, authHeaders()),
        body: JSON.stringify(updates),
      }
    ).then(function (response) {
      return response.json().then(function (body) {
        if (!response.ok) {
          throw new Error(
            (body.error && body.error.message) || "Could not update document details"
          );
        }
        var index = state.jobs.findIndex(function (job) {
          return job.id === jobId;
        });
        if (index >= 0) state.jobs[index] = body;
        if (state.currentJobId === jobId) {
          state.currentJobMeta = body;
          if (state.activeTab === "inspector") {
            document.getElementById("app-title").textContent =
              "Reader — " + jobDisplayName(body);
          }
        }
        return body;
      });
    });
  }

  function visibleLibraryJobs() {
    var collection = document.getElementById("library-collection-filter").value;
    var tag = document.getElementById("library-tag-filter").value;
    var favoritesOnly = document.getElementById("library-favorites-filter").checked;
    var sort = document.getElementById("library-sort").value;
    var jobs = state.jobs.filter(function (job) {
      if (collection && job.collection !== collection) return false;
      if (tag && !(job.tags || []).includes(tag)) return false;
      if (favoritesOnly && !job.favorite) return false;
      return true;
    });
    jobs.sort(function (a, b) {
      if (sort === "oldest") return a.created_at - b.created_at;
      if (sort === "name") {
        return jobDisplayName(a).localeCompare(jobDisplayName(b), undefined, {
          sensitivity: "base",
        });
      }
      if (sort === "pages") return (b.total || 0) - (a.total || 0);
      if (sort === "favorite") {
        return Number(b.favorite) - Number(a.favorite) || b.created_at - a.created_at;
      }
      return b.created_at - a.created_at;
    });
    return jobs;
  }

  function refreshLibraryFilters() {
    var collectionSelect = document.getElementById("library-collection-filter");
    var tagSelect = document.getElementById("library-tag-filter");
    if (!collectionSelect || !tagSelect) return;
    var collections = {};
    var tags = {};
    state.jobs.forEach(function (job) {
      if (job.collection) collections[job.collection] = true;
      (job.tags || []).forEach(function (tag) {
        tags[tag] = true;
      });
    });
    var previousCollection = collectionSelect.value;
    var previousTag = tagSelect.value;
    function fill(select, values, emptyLabel, previous) {
      select.innerHTML = "";
      var empty = document.createElement("option");
      empty.value = "";
      empty.textContent = emptyLabel;
      select.appendChild(empty);
      values.forEach(function (value) {
        var option = document.createElement("option");
        option.value = value;
        option.textContent = value;
        select.appendChild(option);
      });
      select.value = values.includes(previous) ? previous : "";
    }
    fill(
      collectionSelect,
      Object.keys(collections).sort(),
      "All collections",
      previousCollection
    );
    fill(tagSelect, Object.keys(tags).sort(), "All tags", previousTag);
    var datalist = document.getElementById("library-collection-options");
    if (datalist) {
      datalist.innerHTML = "";
      Object.keys(collections)
        .sort()
        .forEach(function (collection) {
          var option = document.createElement("option");
          option.value = collection;
          datalist.appendChild(option);
        });
    }
  }

  function openLibraryModal(jobId) {
    var job = state.jobs.find(function (candidate) {
      return candidate.id === jobId;
    });
    if (!job) return;
    state.libraryModalJobId = jobId;
    document.getElementById("library-name-input").value = job.display_name || "";
    document.getElementById("library-collection-input").value = job.collection || "";
    document.getElementById("library-tags-input").value = (job.tags || []).join(", ");
    document.getElementById("library-favorite-input").checked = Boolean(job.favorite);
    var modal = document.getElementById("library-modal");
    modal.classList.remove("hidden");
    document.getElementById("library-name-input").focus();
  }

  function closeLibraryModal() {
    state.libraryModalJobId = "";
    document.getElementById("library-modal").classList.add("hidden");
  }

  function saveLibraryModal() {
    if (!state.libraryModalJobId) return;
    var jobId = state.libraryModalJobId;
    var tags = document
      .getElementById("library-tags-input")
      .value.split(",")
      .map(function (tag) {
        return tag.trim();
      })
      .filter(Boolean);
    var name = document.getElementById("library-name-input").value.trim();
    var collection = document.getElementById("library-collection-input").value.trim();
    patchLibrary(jobId, {
      display_name: name,
      collection: collection,
      tags: tags,
      favorite: document.getElementById("library-favorite-input").checked,
    })
      .then(function () {
        closeLibraryModal();
        refreshLibraryFilters();
        renderJobsTable();
        populateJobSelector();
        showToast("Document details saved");
      })
      .catch(function (error) {
        showToast(String(error.message || error));
      });
  }

  function toggleJobFavorite(job) {
    patchLibrary(job.id, { favorite: !job.favorite })
      .then(function (updated) {
        refreshLibraryFilters();
        renderJobsTable();
        populateJobSelector();
        showToast(updated.favorite ? "Added to favorites" : "Removed from favorites");
      })
      .catch(function (error) {
        showToast(String(error.message || error));
      });
  }

  function toggleJobSelection(jobId, selected) {
    if (selected) state.selectedJobIds[jobId] = true;
    else delete state.selectedJobIds[jobId];
    updateBulkBar();
  }

  function selectedJobIds() {
    return Object.keys(state.selectedJobIds).filter(function (id) {
      return state.jobs.some(function (job) {
        return job.id === id;
      });
    });
  }

  function updateBulkBar() {
    var ids = selectedJobIds();
    var bar = document.getElementById("bulk-bar");
    var count = document.getElementById("bulk-count");
    if (!bar || !count) return;
    count.textContent = ids.length + " selected";
    bar.classList.toggle("hidden", ids.length === 0);
  }

  function selectAllVisibleJobs() {
    var jobs = visibleLibraryJobs();
    var allSelected = jobs.every(function (job) {
      return state.selectedJobIds[job.id];
    });
    state.selectedJobIds = {};
    if (!allSelected) {
      jobs.forEach(function (job) {
        state.selectedJobIds[job.id] = true;
      });
    }
    renderJobsTable();
  }

  function downloadSelectedBundle() {
    var ids = selectedJobIds();
    if (!ids.length) return;
    if (ids.length > 50) {
      showToast("A bundle can contain at most 50 documents");
      return;
    }
    var unfinished = ids.filter(function (id) {
      var job = state.jobs.find(function (candidate) {
        return candidate.id === id;
      });
      return !job || job.state !== "completed";
    });
    if (unfinished.length) {
      showToast("Bundles can only include completed documents");
      return;
    }
    var anchor = ids[0];
    var url =
      "/v1/extractions/" +
      encodeURIComponent(anchor) +
      "/archive?jobs=" +
      ids.map(encodeURIComponent).join(",");
    var link = document.createElement("a");
    link.href = url;
    link.download = "library-" + ids.length + "-documents.zip";
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
    showToast("Building a bundle of " + ids.length + " documents…");
  }

  function bulkSetFavorite(favorite) {
    var ids = selectedJobIds();
    if (!ids.length) return;
    var chain = Promise.resolve();
    ids.forEach(function (id) {
      chain = chain.then(function () {
        return patchLibrary(id, { favorite: favorite });
      });
    });
    chain
      .then(function () {
        state.selectedJobIds = {};
        refreshLibraryFilters();
        renderJobsTable();
        showToast(favorite ? "Selected documents favorited" : "Favorites removed");
      })
      .catch(function (error) {
        showToast(String(error.message || error));
      });
  }

  // ==================== QUEUE LISTING ====================
  function refreshQueue() {
    var filter = document.getElementById("queue-state-filter").value;
    var url = "/v1/extractions?limit=200" + (filter ? "&state=" + encodeURIComponent(filter) : "");
    apiGet(url)
      .then(function (data) {
        var previous = state.lastJobStates;
        state.jobs = (data.jobs || []).filter(function (job) {
          return !state.pendingDeletes[job.id];
        });
        state.jobs.forEach(function (job) {
          var before = previous[job.id];
          if (before && !isTerminal(before) && isTerminal(job.state)) {
            maybeNotify(job);
          }
          previous[job.id] = job.state;
        });
        refreshLibraryFilters();
        var signature = JSON.stringify(
          state.jobs.map(function (job) {
            return [
              job.id,
              job.state,
              job.progress,
              job.total,
              job.updated_at,
              job.filename,
              job.display_name,
              job.collection,
              job.tags,
              job.favorite,
              job.unit_failures ? job.unit_failures.length : 0,
              job.options,
            ];
          })
        );
        if (signature !== state.jobsSignature) {
          state.jobsSignature = signature;
          renderJobsTable();
          populateJobSelector();
        }
        syncJobStreams();
      })
      .catch(function () {});
  }

  function syncJobStreams() {
    if (state.token || typeof EventSource === "undefined") return;
    var active = state.jobs
      .filter(function (job) {
        return job.state === "queued" || job.state === "running";
      })
      .slice(0, 6);
    var activeIds = {};
    active.forEach(function (job) {
      activeIds[job.id] = true;
      if (state.jobStreams[job.id]) return;
      var stream = new EventSource(
        "/v1/extractions/" + encodeURIComponent(job.id) + "/events"
      );
      state.jobStreams[job.id] = stream;
      stream.addEventListener("job", function (event) {
        var updated;
        try {
          updated = JSON.parse(event.data);
        } catch (_) {
          return;
        }
        var index = state.jobs.findIndex(function (candidate) {
          return candidate.id === updated.id;
        });
        if (index >= 0) {
          var before = state.jobs[index].state;
          state.jobs[index] = updated;
          if (before !== updated.state && isTerminal(updated.state)) {
            maybeNotify(updated);
          }
          state.lastJobStates[updated.id] = updated.state;
        }
        renderJobsTable();
        if (isTerminal(updated.state)) {
          stream.close();
          delete state.jobStreams[updated.id];
          setTimeout(refreshQueue, 250);
        }
      });
      stream.onerror = function () {
        stream.close();
        delete state.jobStreams[job.id];
      };
    });
    Object.keys(state.jobStreams).forEach(function (id) {
      if (!activeIds[id]) {
        state.jobStreams[id].close();
        delete state.jobStreams[id];
      }
    });
  }

  function maybeNotify(job) {
    if (!state.notify || state.notified[job.id] || !document.hidden) return;
    state.notified[job.id] = true;
    if (!("Notification" in window) || Notification.permission !== "granted") return;
    var title = job.state === "completed" ? "Extraction finished" : "Extraction stopped";
    var body = job.filename || "Document";
    try {
      new Notification(title, { body: body, tag: "doc-extractor-" + job.id });
    } catch (_) {}
  }

  function jobProgress(job) {
    var total = job.total || 0;
    var done = job.progress || 0;
    var pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
    return { done: done, total: total, pct: pct };
  }

  function jobProgressNote(job) {
    var progress = jobProgress(job);
    if (job.state === "queued") return "Waiting for a free worker";
    if (job.state === "running") {
      if (!progress.total) {
        return progress.done ? progress.done + " pages done" : "Starting…";
      }
      var note = progress.done + " of " + progress.total + " pages";
      if (progress.done > 0 && progress.total > progress.done) {
        var elapsed = Math.max(0, Date.now() / 1000 - (job.created_at || Date.now() / 1000));
        var eta = (elapsed * (progress.total - progress.done)) / progress.done;
        if (eta > 0 && eta < 86400) note += " · ~" + formatDuration(eta) + " left";
      }
      return note;
    }
    if (job.state === "completed") {
      if (!progress.done) return "No pages or sections extracted";
      return progress.done + (progress.done === 1 ? " page" : " pages");
    }
    return progress.done + " pages processed";
  }

  function renderJobsTable() {
    var tbody = document.getElementById("jobs-tbody");
    tbody.innerHTML = "";
    var jobs = visibleLibraryJobs();
    if (!jobs.length) {
      var tr = document.createElement("tr");
      tr.innerHTML =
        "<td colspan='4' class='muted'>" +
        (state.jobs.length
          ? "No documents match these library filters."
          : "No documents here yet. Add files on the left, or search your archive from the header.") +
        "</td>";
      tbody.appendChild(tr);
      updateBulkBar();
      renderLibraryOverview();
      return;
    }

    jobs.forEach(function (job) {
      var tr = document.createElement("tr");
      var failures = job.unit_failures || [];
      var hasFailures = failures.length > 0;
      var stateBadgeClass = hasFailures
        ? "badge-warn"
        : job.state === "completed"
        ? "badge-success"
        : job.state === "running" || job.state === "queued"
        ? "badge-primary"
        : job.state === "cancelled"
        ? "badge"
        : "badge-danger";
      var friendlyState = hasFailures
        ? "Needs Attention"
        : job.state === "completed"
        ? "Ready"
        : job.state === "running"
        ? "Extracting…"
        : job.state === "queued"
        ? "Waiting"
        : job.state === "cancelled"
        ? "Cancelled"
        : job.state;

      var opts = job.options || {};
      var optSummary = [];
      if (opts.parser === "xberg") optSummary.push("Xberg");
      if (opts.ocr) {
        optSummary.push(
          opts.ocr_provider === "smoldocling"
            ? "SmolDocling OCR"
            : opts.ocr_provider === "apple_vision"
            ? "Apple Vision OCR"
            : "OCR " + (opts.languages || "eng")
        );
      }
      if (opts.page_images === "failed") optSummary.push("Unreadable page images");
      else if (opts.page_images === "all") optSummary.push("All page images");
      if (opts.vision) optSummary.push("Vision");
      if (opts.pages) optSummary.push("Pages " + opts.pages);

      var tdDoc = document.createElement("td");
      var titleLine = document.createElement("div");
      titleLine.className = "row-title-line";
      var selectBox = document.createElement("input");
      selectBox.type = "checkbox";
      selectBox.className = "job-select";
      selectBox.checked = Boolean(state.selectedJobIds[job.id]);
      selectBox.setAttribute("aria-label", "Select " + jobDisplayName(job));
      selectBox.addEventListener("change", function () {
        toggleJobSelection(job.id, selectBox.checked);
      });
      titleLine.appendChild(selectBox);
      var favorite = document.createElement("button");
      favorite.type = "button";
      favorite.className =
        "favorite-btn" + (job.favorite ? " is-favorite" : "");
      favorite.title = job.favorite ? "Remove from favorites" : "Add to favorites";
      favorite.setAttribute("aria-label", favorite.title);
      favorite.innerHTML = "<svg class='icon'><use href='#i-star'/></svg>";
      favorite.addEventListener("click", function () {
        toggleJobFavorite(job);
      });
      titleLine.appendChild(favorite);
      var title = document.createElement("div");
      title.className = "row-title";
      title.textContent = jobDisplayName(job);
      titleLine.appendChild(title);
      var edit = document.createElement("button");
      edit.type = "button";
      edit.className = "favorite-btn";
      edit.title = "Edit title, collection and tags";
      edit.setAttribute("aria-label", edit.title);
      edit.innerHTML = "<svg class='icon'><use href='#i-pencil'/></svg>";
      edit.addEventListener("click", function () {
        openLibraryModal(job.id);
      });
      titleLine.appendChild(edit);
      tdDoc.appendChild(titleLine);
      var sub = document.createElement("div");
      sub.className = "row-sub";
      sub.textContent =
        (job.display_name ? job.filename + " · " : "") + job.id.slice(0, 8);
      tdDoc.appendChild(sub);
      if (job.collection || (job.tags && job.tags.length)) {
        var tagsLine = document.createElement("div");
        tagsLine.className = "row-tags";
        if (job.collection) {
          var collectionChip = document.createElement("button");
          collectionChip.type = "button";
          collectionChip.className = "collection-chip";
          collectionChip.textContent = job.collection;
          collectionChip.title = "Filter by this collection";
          collectionChip.addEventListener("click", function () {
            document.getElementById("library-collection-filter").value =
              job.collection;
            renderJobsTable();
          });
          tagsLine.appendChild(collectionChip);
        }
        (job.tags || []).forEach(function (tag) {
          var chip = document.createElement("button");
          chip.type = "button";
          chip.className = "tag-chip";
          chip.textContent = tag;
          chip.title = "Filter by this tag";
          chip.addEventListener("click", function () {
            document.getElementById("library-tag-filter").value = tag;
            renderJobsTable();
          });
          tagsLine.appendChild(chip);
        });
        tdDoc.appendChild(tagsLine);
      }
      var options = document.createElement("div");
      options.className = "row-options";
      options.title = optSummary.join(" · ") || "Standard options";
      options.textContent = optSummary.join(" · ") || "Standard";
      tdDoc.appendChild(options);

      var tdState = document.createElement("td");
      tdState.innerHTML =
        "<span class='badge " + stateBadgeClass + "'>" + escapeHtml(friendlyState) + "</span>";
      if (hasFailures) {
        var errLine = document.createElement("div");
        errLine.style.fontSize = "11px";
        errLine.style.color = "var(--danger)";
        errLine.style.marginTop = "3px";
        errLine.textContent = failures[0].message || failures[0].code || "Extraction warning";
        errLine.title = failures
          .map(function (f) {
            return (f.code || "") + ": " + (f.message || "");
          })
          .join("\n");
        tdState.appendChild(errLine);
      }
      var progress = jobProgress(job);
      var tdProg = document.createElement("td");
      if (job.state === "running" && progress.total > 0) {
        var bar = document.createElement("div");
        bar.className = "job-progress";
        bar.innerHTML = "<span style='width:" + progress.pct + "%'></span>";
        tdProg.appendChild(bar);
      }
      var progNote = document.createElement("div");
      progNote.className = "job-progress-note";
      progNote.textContent = jobProgressNote(job);
      tdProg.appendChild(progNote);

      var tdAct = document.createElement("td");
      tdAct.className = "row-actions";
      var openBtn = document.createElement("button");
      openBtn.className = "btn btn-sm btn-primary";
      openBtn.textContent = "Open";
      openBtn.disabled = job.state === "queued" || job.state === "cancelled";
      openBtn.addEventListener("click", function () {
        selectJobForInspection(job.id, true);
      });
      tdAct.appendChild(openBtn);

      if (job.state === "queued" || job.state === "running") {
        var cancelBtn = document.createElement("button");
        cancelBtn.className = "btn btn-sm btn-icon";
        cancelBtn.title = "Cancel this extraction";
        cancelBtn.setAttribute("aria-label", "Cancel this extraction");
        cancelBtn.innerHTML = "<svg class='icon'><use href='#i-stop'/></svg>";
        cancelBtn.addEventListener("click", function () {
          cancelExtraction(job);
        });
        tdAct.appendChild(cancelBtn);
      }

      if (job.state === "failed" || job.state === "cancelled") {
        var retryBtn = document.createElement("button");
        retryBtn.className = "btn btn-sm btn-icon";
        retryBtn.title = "Retry with the same options";
        retryBtn.setAttribute("aria-label", "Retry with the same options");
        retryBtn.innerHTML = "<svg class='icon'><use href='#i-refresh'/></svg>";
        retryBtn.addEventListener("click", function () {
          retryJob(job);
        });
        tdAct.appendChild(retryBtn);
      }

      if (job.state === "completed") {
        var zipLink = document.createElement("a");
        zipLink.className = "btn btn-sm btn-icon";
        zipLink.title = "Download the result archive";
        zipLink.setAttribute("aria-label", "Download the result archive");
        zipLink.href = "/v1/extractions/" + encodeURIComponent(job.id) + "/archive";
        zipLink.innerHTML = "<svg class='icon'><use href='#i-download'/></svg>";
        tdAct.appendChild(zipLink);
      }

      var delBtn = document.createElement("button");
      delBtn.className = "btn btn-sm btn-icon";
      delBtn.title = "Remove this document";
      delBtn.setAttribute("aria-label", "Remove this document");
      delBtn.innerHTML = "<svg class='icon'><use href='#i-x'/></svg>";
      delBtn.addEventListener("click", function () {
        scheduleDelete(job.id);
      });
      tdAct.appendChild(delBtn);

      tr.appendChild(tdDoc);
      tr.appendChild(tdState);
      tr.appendChild(tdProg);
      tr.appendChild(tdAct);
      tbody.appendChild(tr);
    });
    updateBulkBar();
    renderLibraryOverview();
  }

  function cancelExtraction(job) {
    fetch("/v1/extractions/" + encodeURIComponent(job.id) + "/cancel", {
      method: "POST",
      headers: authHeaders(),
    })
      .then(function (response) {
        if (!response.ok) throw new Error("cancel failed");
        showToast("Cancelling " + (job.filename || "extraction"));
        refreshQueue();
      })
      .catch(function () {
        showToast("Could not cancel this extraction");
      });
  }

  function retryJob(job) {
    if (state.uploading) {
      showToast("Wait for the current upload to finish");
      return;
    }
    showToast("Loading the original file…");
    fetch("/v1/extractions/" + encodeURIComponent(job.id) + "/result?format=original", {
      headers: authHeaders(),
    })
      .then(function (response) {
        if (!response.ok) throw new Error("original unavailable");
        return response.blob();
      })
      .then(function (blob) {
        var form = new FormData();
        form.append("options", JSON.stringify(job.options || {}));
        form.append("file", blob, job.filename || "document");
        return sendExtraction(form, "Retrying " + (job.filename || "document"));
      })
      .then(function () {
        document.getElementById("upload-status").textContent =
          "Retry queued for " + (job.filename || "document");
        showToast("Retry queued");
        refreshQueue();
        refreshTelemetry();
      })
      .catch(function () {
        showToast("Could not retry; the original file is no longer available");
      });
  }

  function scheduleDelete(jobId) {
    var index = state.jobs.findIndex(function (candidate) {
      return candidate.id === jobId;
    });
    var job = state.jobs[index];
    if (!job) return;
    state.pendingDeletes[jobId] = true;
    delete state.selectedJobIds[jobId];
    state.jobs.splice(index, 1);
    updateBulkBar();
    renderJobsTable();
    populateJobSelector();
    var timer = setTimeout(function () {
      delete state.deleteTimers[jobId];
      fetch("/v1/extractions/" + encodeURIComponent(jobId), {
        method: "DELETE",
        headers: authHeaders(),
      })
        .then(function (response) {
          if (!response.ok) throw new Error("delete failed");
          delete state.pendingDeletes[jobId];
          if (state.currentJobId === jobId) {
            state.currentJobId = "";
            state.currentDoc = null;
            state.currentUnit = null;
          }
          refreshQueue();
          refreshTelemetry();
        })
        .catch(function () {
          delete state.pendingDeletes[jobId];
          state.jobs.splice(Math.min(index, state.jobs.length), 0, job);
          renderJobsTable();
          populateJobSelector();
          showToast("Could not remove this document");
        });
    }, 6500);
    state.deleteTimers[jobId] = timer;
    showToast(
      "Removed " + (job.filename || "document"),
      "Undo",
      function () {
        clearTimeout(timer);
        delete state.deleteTimers[jobId];
        delete state.pendingDeletes[jobId];
        state.jobs.splice(Math.min(index, state.jobs.length), 0, job);
        renderJobsTable();
        populateJobSelector();
      }
    );
  }

  function populateJobSelector() {
    var sel = document.getElementById("inspector-job-select");
    var prev = state.currentJobId || sel.value;
    sel.innerHTML = "";
    state.jobs.forEach(function (job) {
      var opt = document.createElement("option");
      opt.value = job.id;
      opt.textContent = jobDisplayName(job) + " (" + job.id.slice(0, 8) + ")";
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
  function selectJobForInspection(jobId, navigateToTab, targetUnitId, targetBlockId) {
    var switchingJob = state.currentJobId !== jobId;
    state.currentJobId = jobId;
    if (switchingJob) {
      state.currentUnitId = targetUnitId || "u1";
    }
    state.pendingUnitId = targetUnitId || "";
    state.pendingBlockId = targetBlockId || "";
    if (switchingJob) {
      state.review = { schema_version: "review1", units: {} };
      state.reviewOriginals = {};
      state.reviewOriginalCells = {};
      state.previewRasters = {};
      state.redactionJobId = "";
      state.redactionMatches = [];
      state.redactionSaved = [];
      state.redactionSelected = {};
    }
    var sel = document.getElementById("inspector-job-select");
    if (sel) sel.value = jobId;
    if (navigateToTab) {
      switchTab("inspector");
    }
    updateExportLinks(jobId);
    updateRedactedLinks();

    apiGet("/v1/extractions/" + encodeURIComponent(jobId))
      .then(function (jobMeta) {
        state.currentJobMeta = jobMeta;
        if (state.activeTab === "inspector") {
          document.getElementById("app-title").textContent =
            "Reader — " + jobDisplayName(jobMeta);
        }
        document.getElementById("inspector-doc-status").textContent =
          jobMeta.state === "completed" ? "Completed" : jobMeta.state;
        var original = document.getElementById("dl-original");
        if (original) original.download = jobMeta.filename || "original";

        if (jobMeta.state === "completed") {
          return apiGet("/v1/extractions/" + encodeURIComponent(jobId) + "/result?format=json").then(
            function (doc) {
              if (doc && typeof doc === "object" && Array.isArray(doc.units)) {
                return apiGet(
                  "/v1/extractions/" +
                    encodeURIComponent(jobId) +
                    "/result?format=review"
                )
                  .catch(function () {
                    return { schema_version: "review1", units: {} };
                  })
                  .then(function (review) {
                    applyReviewToDocument(doc, review);
                    state.currentDoc = doc;
                    updateExportLinks(jobId);
                    onDocumentLoaded(doc);
                  });
              }
              loadSingleUnitFromServer(jobId, "u1");
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
    var reviewed = reviewHasCorrections() ? "&review=true" : "";
    document.getElementById("dl-json").href = base + "/result?format=json";
    document.getElementById("dl-md").href =
      base + "/result?format=markdown" + reviewed;
    document.getElementById("dl-txt").href = base + "/result?format=text" + reviewed;
    document.getElementById("dl-jsonl").href = base + "/result?format=jsonl";
    document.getElementById("dl-html").href = base + "/result?format=html" + reviewed;
    var report = document.getElementById("dl-report");
    if (report) report.href = base + "/result?format=report";
    document.getElementById("dl-zip").href = base + "/archive";
    var original = document.getElementById("dl-original");
    if (original) original.href = base + "/result?format=original";
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
    var target = state.pendingUnitId || firstUnit;
    var exists = (doc.units || []).some(function (u) {
      return u.id === target;
    });
    state.pendingUnitId = "";
    selectUnit(exists ? target : firstUnit);
    renderPageFilmstrip();
    if (state.activeSubtab === "formats") {
      loadRedactions(true);
    }
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
    if (state.continuousMode) setContinuousMode(false);
    state.currentUnitId = unitId;
    var pendingBlock = state.pendingBlockId;
    state.activeBlockId = pendingBlock || "";
    state.scrollToBlockId = pendingBlock || "";
    state.pendingBlockId = "";
    var unitSelect = document.getElementById("inspector-unit-select");
    if (unitSelect && unitSelect.value !== unitId) {
      unitSelect.value = unitId;
    }
    if (state.currentJobId && state.activeTab === "inspector") {
      setRoute(
        "/reader/" +
          encodeURIComponent(state.currentJobId) +
          "/" +
          encodeURIComponent(unitId),
        true
      );
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

  function imageAssetMeta(doc, assetId) {
    return (
      (assetId &&
        doc &&
        Array.isArray(doc.assets) &&
        doc.assets.find(function (asset) {
          return (
            asset.id === assetId &&
            asset.mime &&
            asset.mime.indexOf("image/") === 0
          );
        })) ||
      null
    );
  }

  function findUnitRasterAsset(unit, doc) {
    if (!unit || !unit.blocks) return null;
    var assetId = null;
    var dpi = 150;
    for (var i = 0; i < unit.blocks.length; i++) {
      var b = unit.blocks[i];
      if (
        b.attributes &&
        b.attributes.page_image &&
        b.attributes.page_image.asset_id &&
        imageAssetMeta(doc, b.attributes.page_image.asset_id)
      ) {
        assetId = b.attributes.page_image.asset_id;
        dpi = b.attributes.page_image.dpi || 150;
        break;
      }
    }
    var assetMeta = imageAssetMeta(doc, assetId);
    if (!assetMeta) {
      for (var j = 0; j < unit.blocks.length; j++) {
        var blk = unit.blocks[j];
        if (blk.kind === "figure" && blk.asset_id) {
          var candidate = imageAssetMeta(doc, blk.asset_id);
          if (candidate) {
            assetId = blk.asset_id;
            assetMeta = candidate;
            break;
          }
        }
      }
    }
    if (!assetId || !assetMeta) return null;
    return {
      asset_id: assetId,
      width: assetMeta.width || null,
      height: assetMeta.height || null,
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
      var preview = state.previewRasters[u.id];
      var quality = unitQuality(u);
      var btn = document.createElement("button");
      btn.type = "button";
      btn.className = "page-film-thumb" + (u.id === state.currentUnitId ? " is-active" : "");
      btn.setAttribute("data-unit", u.id);
      btn.title =
        (u.name || "Page " + (i + 1)) +
        " · " +
        quality.label +
        (quality.reviewed ? " · verified" : "");
      var inner = "";
      if (preview) {
        inner =
          "<img loading='lazy' src='" +
          escapeHtml(preview.url) +
          "' alt='Page " +
          (i + 1) +
          "'>";
      } else if (raster && state.currentJobId) {
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
        inner +
        "<span class='page-film-label'><span class='quality-dot quality-" +
        (quality.reviewed ? "verified" : quality.level) +
        "'></span>Page " +
        (i + 1) +
        "</span>";
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
      var active = btn.getAttribute("data-unit") === state.currentUnitId;
      btn.classList.toggle("is-active", active);
      if (active) btn.setAttribute("aria-current", "page");
      else btn.removeAttribute("aria-current");
    });
    var active = document.querySelector("#page-filmstrip .page-film-thumb.is-active");
    var strip = document.getElementById("page-filmstrip");
    if (active && strip) {
      var stripRect = strip.getBoundingClientRect();
      var activeRect = active.getBoundingClientRect();
      var delta =
        activeRect.left - stripRect.left - (strip.clientWidth - activeRect.width) / 2;
      strip.scrollLeft += delta;
    }
  }

  function setContinuousMode(enabled) {
    if (enabled && !state.currentDoc) {
      showToast("Open a document first");
      return;
    }
    state.continuousMode = enabled;
    var button = document.getElementById("continuous-toggle-btn");
    var split = document.getElementById("reader-split");
    var reader = document.getElementById("continuous-reader");
    if (!button || !split || !reader) return;
    button.setAttribute("aria-pressed", enabled ? "true" : "false");
    button.innerHTML =
      "<svg class='icon'><use href='#" +
      (enabled ? "i-file" : "i-book") +
      "'/></svg>" +
      (enabled ? "Single page" : "Read all pages");
    reader.classList.toggle("hidden", !enabled);
    split.classList.toggle("hidden", enabled);
    if (enabled) {
      state.continuousLimit = 40;
      renderContinuousReader();
    }
  }

  function renderContinuousReader() {
    var container = document.getElementById("continuous-reader");
    if (!container || !state.currentDoc) return;
    var units = state.currentDoc.units || [];
    container.innerHTML = "";
    units.slice(0, state.continuousLimit).forEach(function (unit) {
      var quality = unitQuality(unit);
      var section = document.createElement("article");
      section.className = "continuous-page";
      var head = document.createElement("div");
      head.className = "continuous-page-head";
      head.innerHTML =
        "<h3>" +
        escapeHtml(unit.name) +
        "</h3>" +
        qualityBadgeHtml(quality, true);
      var open = document.createElement("button");
      open.className = "btn btn-sm";
      open.textContent = "Open page";
      open.addEventListener("click", function () {
        selectUnit(unit.id);
        switchSubtab("blocks");
      });
      head.appendChild(open);
      section.appendChild(head);

      var body = document.createElement("div");
      body.className = "continuous-page-body";
      var raster = findUnitRasterAsset(unit, state.currentDoc);
      var preview = state.previewRasters[unit.id];
      var imageUrl = preview
        ? preview.url
        : raster && state.currentJobId
        ? "/v1/extractions/" +
          encodeURIComponent(state.currentJobId) +
          "/assets/" +
          encodeURIComponent(raster.asset_id)
        : "";
      if (imageUrl) {
        var image = document.createElement("div");
        image.className = "continuous-page-image";
        image.innerHTML =
          "<img loading='lazy' src='" +
          escapeHtml(imageUrl) +
          "' alt='" +
          escapeHtml(unit.name) +
          "'>";
        body.appendChild(image);
      } else {
        body.classList.add("no-image");
      }
      var text = document.createElement("div");
      text.className = "continuous-text";
      (unit.blocks || []).forEach(function (block) {
        if (block.table) {
          var grid = tableGrid(block);
          if (!grid.length) return;
          var wrapper = document.createElement("div");
          wrapper.className = "continuous-table";
          var table = document.createElement("table");
          var maxRows = Math.min(grid.length, 60);
          for (var r = 0; r < maxRows; r++) {
            var tr = document.createElement("tr");
            grid[r].forEach(function (value) {
              var cell = document.createElement(r === 0 ? "th" : "td");
              cell.textContent = value;
              tr.appendChild(cell);
            });
            table.appendChild(tr);
          }
          wrapper.appendChild(table);
          if (grid.length > maxRows) {
            var note = document.createElement("div");
            note.className = "continuous-empty";
            note.textContent =
              "Showing " + maxRows + " of " + grid.length + " table rows";
            wrapper.appendChild(note);
          }
          text.appendChild(wrapper);
        } else if (block.text) {
          var paragraph = document.createElement("p");
          paragraph.textContent = block.text;
          paragraph.style.marginBottom = "0.7rem";
          text.appendChild(paragraph);
        }
      });
      if (!text.children.length) {
        text.innerHTML = "<p class='continuous-empty'>No text on this page.</p>";
      }
      body.appendChild(text);
      section.appendChild(body);
      container.appendChild(section);
    });
    if (units.length > state.continuousLimit) {
      var more = document.createElement("div");
      more.className = "continuous-load-more";
      var button = document.createElement("button");
      button.className = "btn btn-sm";
      button.textContent =
        "Load " + Math.min(40, units.length - state.continuousLimit) + " more pages";
      button.addEventListener("click", function () {
        state.continuousLimit += 40;
        renderContinuousReader();
      });
      more.appendChild(button);
      container.appendChild(more);
    }
  }

  function renderCurrentUnit() {
    var unit = state.currentUnit;
    if (!unit) return;

    var raster = findUnitRasterAsset(unit, state.currentDoc);
    var preview = state.previewRasters[unit.id];
    var showRaster =
      document.getElementById("toggle-raster-bg").checked &&
      Boolean(raster || preview);
    var imgEl = document.getElementById("page-raster-img");
    var stageEl = document.getElementById("page-stage");

    if (showRaster && state.currentJobId) {
      if (preview) {
        state.currentPageRasterAssetId = null;
        state.currentRasterDims = {
          width: preview.width,
          height: preview.height,
          dpi: preview.dpi || 150,
        };
        imgEl.src = preview.url;
      } else {
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
      }
      state.currentPageRasterVisible = true;
      imgEl.classList.remove("hidden");
      stageEl.classList.add("has-raster");
    } else {
      state.currentPageRasterAssetId = null;
      state.currentPageRasterVisible = false;
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

    updatePageEmptyState();
    renderBlocksSubtab(unit);
    renderTablesSubtab(unit);
    updateFilmstripActive();
    updateUnitTimingBadge(unit);
    updateUnitQualityBadge(unit);
    renderReviewPanel(unit);
  }

  function updateUnitQualityBadge(unit) {
    var chip = document.getElementById("canvas-quality");
    if (!chip) return;
    var quality = unitQuality(unit);
    chip.classList.remove("hidden");
    chip.className =
      "badge quality-badge quality-" +
      (quality.reviewed ? "verified" : quality.level);
    chip.textContent =
      (quality.reviewed ? "Verified · " : "") + quality.label;
    chip.title =
      quality.label +
      (quality.lowBlocks ? " · " + quality.lowBlocks + " low-confidence block(s)" : "") +
      (quality.warnings ? " · " + quality.warnings + " note(s)" : "") +
      (quality.errors ? " · " + quality.errors + " error(s)" : "");
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

  function renderCurrentPagePreview() {
    if (!state.currentJobId || !state.currentUnit) return;
    if (state.renderingUnit) return;
    var unit = state.currentUnit;
    if (!state.currentJobMeta || !/\.pdf$/i.test(state.currentJobMeta.filename || "")) {
      showToast("Page previews are available for PDF jobs");
      return;
    }
    state.renderingUnit = unit.id;
    var buttons = [
      document.getElementById("render-page-btn"),
      document.getElementById("render-page-toolbar-btn"),
    ];
    buttons.forEach(function (button) {
      if (button) button.disabled = true;
    });
    showToast("Rendering " + (unit.name || "page") + "…");
    var url =
      "/v1/extractions/" +
      encodeURIComponent(state.currentJobId) +
      "/units/" +
      encodeURIComponent(unit.id) +
      "?render=true&dpi=150";
    var probe = new Image();
    probe.onload = function () {
      state.previewRasters[unit.id] = {
        url: url,
        width: probe.naturalWidth,
        height: probe.naturalHeight,
        dpi: 150,
      };
      state.renderingUnit = "";
      buttons.forEach(function (button) {
        if (button) button.disabled = false;
      });
      renderCurrentUnit();
      renderPageFilmstrip();
      showToast("Page preview rendered");
    };
    probe.onerror = function () {
      state.renderingUnit = "";
      buttons.forEach(function (button) {
        if (button) button.disabled = false;
      });
      showToast("Could not render this page; check that Poppler is installed");
    };
    probe.src = url;
  }

  function updatePageEmptyState() {
    var placeholder = document.getElementById("page-empty");
    var layer = document.getElementById("page-svg-layer");
    var img = document.getElementById("page-raster-img");
    if (!placeholder || !layer || !img) return;
    var hasRaster = !img.classList.contains("hidden");
    var hasBoxes = layer.querySelectorAll(".bbox-group").length > 0;
    placeholder.classList.toggle("hidden", hasRaster || hasBoxes);
    var isPdf = Boolean(
      state.currentJobMeta &&
        /\.pdf$/i.test(state.currentJobMeta.filename || "")
    );
    var canRender = isPdf && !hasRaster;
    var placeholderButton = document.getElementById("render-page-btn");
    var toolbarButton = document.getElementById("render-page-toolbar-btn");
    if (placeholderButton) {
      placeholderButton.classList.toggle("hidden", !canRender || hasBoxes);
    }
    if (toolbarButton) {
      toolbarButton.classList.toggle("hidden", !canRender);
    }
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
      state.currentPageRasterVisible;
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
    var notes = [];
    var visibleWarnings = [];
    (unit.warnings || []).forEach(function (w) {
      if (
        w.indexOf("Scanned page:") === 0 ||
        w.indexOf("Embedded images retained;") === 0 ||
        w.indexOf("Poppler reading order") === 0
      ) {
        notes.push(w);
      } else {
        visibleWarnings.push(w);
      }
    });
    visibleWarnings.forEach(function (w) {
      var div = document.createElement("div");
      div.className = "alert-item";
      div.textContent = w;
      warningsBox.appendChild(div);
    });
    if (notes.length) {
      var details = document.createElement("details");
      details.className = "reading-notes";
      var summary = document.createElement("summary");
      summary.textContent =
        "Page notes (" + notes.length + ") · provenance and reading order";
      details.appendChild(summary);
      notes.forEach(function (w) {
        var div = document.createElement("div");
        div.className = "alert-item";
        div.textContent = w;
        details.appendChild(div);
      });
      warningsBox.appendChild(details);
    }

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
      var unitReviewState = unitReview(unit.id);
      var edited = Boolean(
        unitReviewState.blocks && unitReviewState.blocks[b.id]
      );
      var card = document.createElement("div");
      card.className =
        "block-card" +
        (b.id === state.activeBlockId ? " is-active" : "") +
        (edited ? " is-edited" : "");
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
        "<span class='block-actions'></span>" +
        "</div>";
      var actions = card.querySelector(".block-actions");
      if (edited) {
        var editedBadge = document.createElement("span");
        editedBadge.className = "badge block-edited-badge";
        editedBadge.textContent = "Edited";
        actions.appendChild(editedBadge);
      }
      var methodBadge = document.createElement("span");
      methodBadge.className = "badge " + badgeCls;
      methodBadge.textContent = friendlyMethod;
      actions.appendChild(methodBadge);
      var blockScore =
        b.attributes && typeof b.attributes.ocr_score_uncalibrated === "number"
          ? b.attributes.ocr_score_uncalibrated
          : null;
      if (blockScore !== null && blockScore < 70) {
        var confidenceBadge = document.createElement("span");
        confidenceBadge.className = "badge badge-warn";
        confidenceBadge.textContent = "Low " + Math.round(blockScore) + "%";
        confidenceBadge.title =
          "Uncalibrated OCR score for this block; inspect it against the page";
        actions.appendChild(confidenceBadge);
      }
      if (b.text && !b.table) {
        var editBtn = document.createElement("button");
        editBtn.className = "btn btn-sm btn-icon";
        editBtn.type = "button";
        editBtn.title = "Correct this text";
        editBtn.setAttribute("aria-label", "Correct this text");
        editBtn.innerHTML = "<svg class='icon'><use href='#i-pencil'/></svg>";
        editBtn.addEventListener("click", function (event) {
          event.stopPropagation();
          startBlockEdit(card, b);
        });
        actions.appendChild(editBtn);
        if (edited) {
          var revertBtn = document.createElement("button");
          revertBtn.className = "btn btn-sm";
          revertBtn.type = "button";
          revertBtn.textContent = "Revert";
          revertBtn.title = "Restore the original extracted text";
          revertBtn.addEventListener("click", function (event) {
            event.stopPropagation();
            revertBlockCorrection(b);
          });
          actions.appendChild(revertBtn);
        }
      }

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

    if (state.scrollToBlockId) {
      var target = list.querySelector(
        "[data-block-id='" + String(state.scrollToBlockId).replace(/'/g, "\\'") + "']"
      );
      state.scrollToBlockId = "";
      if (target) {
        target.classList.add("is-hit");
        scrollBlockIntoList(target);
        setTimeout(function () {
          target.classList.remove("is-hit");
        }, 1800);
      }
    }
  }

  function startBlockEdit(card, block) {
    if (!state.currentUnit || card.querySelector(".block-edit-area")) return;
    var textEl = card.querySelector(".block-text");
    if (!textEl) return;
    var area = document.createElement("textarea");
    area.className = "block-edit-area";
    area.value = block.text;
    area.setAttribute("aria-label", "Correct the extracted text");
    textEl.replaceWith(area);
    var actions = document.createElement("div");
    actions.className = "block-edit-actions";
    var cancel = document.createElement("button");
    cancel.className = "btn btn-sm";
    cancel.type = "button";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", function (event) {
      event.stopPropagation();
      renderBlocksSubtab(state.currentUnit);
    });
    var save = document.createElement("button");
    save.className = "btn btn-sm btn-primary";
    save.type = "button";
    save.textContent = "Save correction";
    save.addEventListener("click", function (event) {
      event.stopPropagation();
      save.disabled = true;
      saveBlockCorrection(block, area.value);
    });
    actions.appendChild(cancel);
    actions.appendChild(save);
    area.addEventListener("keydown", function (event) {
      if (event.key === "Escape") {
        event.preventDefault();
        renderBlocksSubtab(state.currentUnit);
      } else if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
        event.preventDefault();
        save.click();
      }
    });
    card.appendChild(actions);
    area.focus();
    area.setSelectionRange(area.value.length, area.value.length);
  }

  function scrollBlockIntoList(target) {
    var list = document.getElementById("blocks-list");
    if (!list || !target) return;
    var listRect = list.getBoundingClientRect();
    var targetRect = target.getBoundingClientRect();
    var offset = targetRect.top - listRect.top - Math.max(12, list.clientHeight / 2 - targetRect.height / 2);
    list.scrollTo({ top: list.scrollTop + offset, behavior: "smooth" });
  }

  function highlightBlockCard(blockId, scrollIntoView) {
    document.querySelectorAll(".block-card").forEach(function (card) {
      var match = card.getAttribute("data-block-id") === blockId;
      card.classList.toggle("is-active", match);
      if (match && scrollIntoView) {
        scrollBlockIntoList(card);
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

    tableBlocks.forEach(function (b, tableIndex) {
      var table = b.table;
      var wrap = document.createElement("div");
      wrap.style.marginBottom = "1.2rem";
      var method = (b.provenance && b.provenance.method) || "native";
      var hdr = document.createElement("div");
      hdr.className = "table-card-head";
      var title = document.createElement("div");
      title.innerHTML =
        "<strong>Table (" +
        table.rows +
        " rows × " +
        table.columns +
        " columns)</strong> · <span class='badge'>" +
        escapeHtml(FRIENDLY_METHOD[method] || method) +
        "</span>";
      hdr.appendChild(title);

      var actions = document.createElement("div");
      actions.className = "table-card-actions";
      var copyBtn = document.createElement("button");
      copyBtn.className = "btn btn-sm";
      copyBtn.innerHTML = "<svg class='icon'><use href='#i-copy'/></svg> Copy CSV";
      copyBtn.title = "Copy this table as CSV";
      copyBtn.addEventListener("click", function () {
        copyText(tableCsv(b), "Table copied as CSV");
      });
      var csvBtn = document.createElement("button");
      csvBtn.className = "btn btn-sm";
      csvBtn.innerHTML = "<svg class='icon'><use href='#i-download'/></svg> CSV";
      csvBtn.title = "Download this table as a CSV file";
      csvBtn.addEventListener("click", function () {
        var base = fileNameForUnit(unit, "csv").replace(/\.csv$/, "");
        downloadText(
          base + "-table-" + (tableIndex + 1) + ".csv",
          tableCsv(b),
          "text/csv;charset=utf-8"
        );
      });
      actions.appendChild(copyBtn);
      actions.appendChild(csvBtn);
      hdr.appendChild(actions);
      wrap.appendChild(hdr);

      var cellMap = {};
      (table.cells || []).forEach(function (c) {
        if (!cellMap[c.row]) cellMap[c.row] = {};
        cellMap[c.row][c.column] = c;
      });
      var rowIndices = Object.keys(cellMap)
        .map(Number)
        .sort(function (x, y) {
          return x - y;
        });

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
      tbl.appendChild(tbody);
      var shown = 0;
      var pageSize = 300;
      var more = null;
      var moreText = null;

      function appendRows(limit) {
        var end = Math.min(rowIndices.length, limit);
        rowIndices.slice(shown, end).forEach(function (rIdx) {
          var tr = document.createElement("tr");
          var rth = document.createElement("td");
          rth.className = "muted";
          rth.textContent = String(rIdx + 1);
          tr.appendChild(rth);
          for (let cIdx = 0; cIdx < table.columns; cIdx++) {
            let cell = cellMap[rIdx][cIdx];
            let td = document.createElement("td");
            if (cell) {
              var cellKey = b.id + ":" + rIdx + ":" + cIdx;
              var cellEdited = state.reviewOriginalCells[cellKey] != null;
              td.textContent = cell.raw || "";
              if (cellEdited) td.classList.add("cell-edited");
              td.title = cellEdited
                ? "Corrected cell · double-click to edit or restore"
                : "Double-click to correct this cell";
              if (cell.formula) {
                var fSpan = document.createElement("span");
                fSpan.className = "cell-formula";
                fSpan.textContent = "=" + cell.formula;
                td.appendChild(fSpan);
              }
              td.addEventListener("dblclick", function (event) {
                event.stopPropagation();
                startCellEdit(td, b, rIdx, cIdx, cell);
              });
            }
            tr.appendChild(td);
          }
          tbody.appendChild(tr);
        });
        shown = end;
        if (moreText) {
          moreText.textContent =
            "Showing " +
            shown.toLocaleString() +
            " of " +
            rowIndices.length.toLocaleString() +
            " rows";
        }
      }

      if (rowIndices.length > pageSize) {
        more = document.createElement("div");
        more.className = "table-more-note";
        moreText = document.createElement("span");
        more.appendChild(moreText);
      }
      appendRows(pageSize);
      wrap.appendChild(tbl);

      if (more) {
        var moreBtn = document.createElement("button");
        moreBtn.className = "btn btn-sm";
        moreBtn.textContent = "Show " + pageSize + " more";
        moreBtn.addEventListener("click", function () {
          appendRows(shown + pageSize);
          if (shown >= rowIndices.length && moreBtn.parentNode) {
            moreBtn.parentNode.removeChild(moreBtn);
          }
        });
        more.appendChild(moreBtn);
        wrap.appendChild(more);
      }
      wrap.setAttribute("data-table-block-id", b.id);
      if (b.id === state.activeBlockId) {
        wrap.classList.add("is-active");
        setTimeout(function () {
          var wrapRect = wrap.getBoundingClientRect();
          var containerRect = container.getBoundingClientRect();
          container.scrollTo({
            top:
              container.scrollTop +
              wrapRect.top -
              containerRect.top -
              10,
            behavior: "smooth",
          });
        }, 0);
      }
      container.appendChild(wrap);
    });
  }

  function startCellEdit(td, block, row, column, cell) {
    if (td.querySelector(".cell-edit-input")) return;
    var originalHtml = td.innerHTML;
    var key = block.id + ":" + row + ":" + column;
    var input = document.createElement("input");
    input.className = "cell-edit-input";
    input.type = "text";
    input.value = cell.raw || "";
    input.setAttribute("aria-label", "Correct this table cell");
    var actions = document.createElement("span");
    actions.className = "cell-edit-actions";
    var save = document.createElement("button");
    save.className = "btn btn-sm btn-primary";
    save.type = "button";
    save.textContent = "Save";
    var cancel = document.createElement("button");
    cancel.className = "btn btn-sm";
    cancel.type = "button";
    cancel.textContent = "Cancel";
    var restore = null;
    if (state.reviewOriginalCells[key] != null) {
      restore = document.createElement("button");
      restore.className = "btn btn-sm";
      restore.type = "button";
      restore.textContent = "Restore";
      restore.title = "Restore the original cell value";
    }
    var done = false;
    function cancelEdit() {
      if (done) return;
      done = true;
      td.innerHTML = originalHtml;
    }
    save.addEventListener("click", function (event) {
      event.stopPropagation();
      if (done) return;
      done = true;
      saveCellCorrection(block, row, column, cell, input.value);
    });
    cancel.addEventListener("click", function (event) {
      event.stopPropagation();
      cancelEdit();
    });
    input.addEventListener("keydown", function (event) {
      if (event.key === "Enter") {
        event.preventDefault();
        save.click();
      } else if (event.key === "Escape") {
        event.preventDefault();
        cancelEdit();
      }
    });
    td.innerHTML = "";
    td.appendChild(input);
    actions.appendChild(save);
    actions.appendChild(cancel);
    if (restore) {
      restore.addEventListener("click", function (event) {
        event.stopPropagation();
        if (done) return;
        done = true;
        revertCellCorrection(block, row, column);
      });
      actions.appendChild(restore);
    }
    td.appendChild(actions);
    input.focus();
    input.select();
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
    var flagged = units.filter(function (unit) {
      var quality = unitQuality(unit);
      return quality.level !== "good" && !quality.reviewed;
    }).length;
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
        (flagged ? " · " + flagged + " need review" : " · no flagged pages") +
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
      var quality = unit ? unitQuality(unit) : null;
      var qualityHtml = quality ? qualityBadgeHtml(quality, true) : "";
      var timing = unit && unit.timing;
      var timingNote = timing
        ? " · " +
          (timing.total_ms / 1000).toFixed(1) +
          " s" +
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
        "<div style='display:flex;justify-content:space-between;align-items:center;flex-wrap:wrap;gap:5px;margin-bottom:4px'>" +
        "<strong>" +
        escapeHtml(p.unit_name || "Page " + p.page) +
        "</strong>" +
        "<span style='display:flex;gap:4px;align-items:center;flex-wrap:wrap'>" +
        statusBadge +
        qualityHtml +
        "</span>" +
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
    document.getElementById("theme-toggle").addEventListener("click", toggleTheme);
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
      stageFiles(fileInput.files);
      fileInput.value = "";
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
        stageFiles(ev.dataTransfer.files);
      }
    });

    document.getElementById("submit-upload-btn").addEventListener("click", function () {
      uploadFiles(state.selectedFiles);
    });
    document.getElementById("sample-synth-btn").addEventListener("click", runInstantDemoBatch);
    var notifyBox = document.getElementById("opt-notify");
    if (notifyBox) {
      notifyBox.checked = state.notify;
      if (!("Notification" in window)) {
        notifyBox.disabled = true;
        notifyBox.parentNode.style.opacity = "0.55";
      }
      notifyBox.addEventListener("change", requestNotifyPermission);
    }

    document.getElementById("open-search-btn").addEventListener("click", openSearch);
    var searchOverlay = document.getElementById("search-overlay");
    searchOverlay.addEventListener("click", function (ev) {
      if (ev.target === searchOverlay) closeSearch();
    });
    document.getElementById("search-input").addEventListener("input", function (ev) {
      scheduleSearch(ev.target.value);
    });
    document.getElementById("search-input").addEventListener("keydown", function (ev) {
      if (ev.key === "ArrowDown") {
        ev.preventDefault();
        moveSearchSelection(1);
      } else if (ev.key === "ArrowUp") {
        ev.preventDefault();
        moveSearchSelection(-1);
      } else if (ev.key === "Enter") {
        ev.preventDefault();
        openSearchResult(state.searchResults[state.searchSelection]);
      } else if (ev.key === "Escape") {
        ev.preventDefault();
        closeSearch();
      }
    });
    document.addEventListener("keydown", function (ev) {
      var key = ev.key.toLowerCase();
      if ((ev.metaKey || ev.ctrlKey) && key === "k") {
        ev.preventDefault();
        if (document.getElementById("search-overlay").classList.contains("hidden")) {
          openSearch();
        } else {
          closeSearch();
        }
      } else if (key === "escape") {
        closeSearch();
      } else if (
        state.activeTab === "inspector" &&
        !ev.metaKey &&
        !ev.ctrlKey &&
        !ev.altKey &&
        (key === "j" || key === "k")
      ) {
        var tag = (ev.target && ev.target.tagName) || "";
        if (tag !== "INPUT" && tag !== "TEXTAREA" && tag !== "SELECT") {
          ev.preventDefault();
          stepUnit(key === "j" ? 1 : -1);
        }
      }
    });

    document.getElementById("copy-link-btn").addEventListener("click", function () {
      if (!state.currentJobId) return;
      setRoute(tabPath("inspector"), true);
      copyText(location.href, "Page link copied");
    });
    document.getElementById("copy-page-text-btn").addEventListener("click", function () {
      if (!state.currentUnit) return;
      copyText(unitPlainText(state.currentUnit), "Page text copied");
    });
    document.getElementById("download-page-text-btn").addEventListener("click", function () {
      if (!state.currentUnit) return;
      downloadText(
        fileNameForUnit(state.currentUnit, "txt"),
        unitPlainText(state.currentUnit),
        "text/plain;charset=utf-8"
      );
    });
    ["render-page-btn", "render-page-toolbar-btn"].forEach(function (id) {
      var button = document.getElementById(id);
      if (button) button.addEventListener("click", renderCurrentPagePreview);
    });
    document.getElementById("review-next-btn").addEventListener("click", reviewNextFlagged);
    var markReviewed = document.getElementById("mark-page-reviewed-btn");
    if (markReviewed) {
      markReviewed.addEventListener("click", function () {
        if (!state.currentUnit || state.reviewBusy) return;
        var reviewed = unitReview(state.currentUnit.id).status === "reviewed";
        saveReviewStatus(state.currentUnit.id, reviewed ? "clear" : "reviewed");
      });
    }

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

    document.getElementById("refresh-queue-btn").addEventListener("click", refreshQueue);
    ["library-collection-filter", "library-tag-filter", "library-sort"].forEach(function (id) {
      document.getElementById(id).addEventListener("change", function () {
        state.selectedJobIds = {};
        renderJobsTable();
      });
    });
    document
      .getElementById("library-favorites-filter")
      .addEventListener("change", function () {
        state.selectedJobIds = {};
        renderJobsTable();
      });
    document
      .getElementById("bulk-select-all-btn")
      .addEventListener("click", selectAllVisibleJobs);
    document
      .getElementById("bulk-download-btn")
      .addEventListener("click", downloadSelectedBundle);
    document
      .getElementById("bulk-favorite-btn")
      .addEventListener("click", function () {
        bulkSetFavorite(true);
      });
    document
      .getElementById("bulk-unfavorite-btn")
      .addEventListener("click", function () {
        bulkSetFavorite(false);
      });
    document.getElementById("bulk-clear-btn").addEventListener("click", function () {
      state.selectedJobIds = {};
      renderJobsTable();
    });
    document.getElementById("library-modal-close").addEventListener("click", closeLibraryModal);
    document.getElementById("library-modal-cancel").addEventListener("click", closeLibraryModal);
    document.getElementById("library-modal-save").addEventListener("click", saveLibraryModal);
    document.getElementById("library-modal").addEventListener("click", function (ev) {
      if (ev.target === this) closeLibraryModal();
    });
    document.getElementById("library-modal").addEventListener("keydown", function (ev) {
      if (ev.key === "Escape") closeLibraryModal();
      else if ((ev.metaKey || ev.ctrlKey) && ev.key === "Enter") saveLibraryModal();
    });

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
    document
      .getElementById("continuous-toggle-btn")
      .addEventListener("click", function () {
        setContinuousMode(!state.continuousMode);
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

    document
      .getElementById("scan-redactions-btn")
      .addEventListener("click", function () {
        loadRedactions(true);
      });
    document
      .getElementById("redaction-select-all-btn")
      .addEventListener("click", selectAllRedactions);
    document
      .getElementById("save-redactions-btn")
      .addEventListener("click", saveRedactions);
    document
      .getElementById("clear-redactions-btn")
      .addEventListener("click", clearRedactions);

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
  initTheme();

  document.addEventListener("DOMContentLoaded", function () {
    bindEvents();
    document.getElementById("pg-source-input").value = PRESETS.doctags.source;

    var searchKbd = document.getElementById("search-kbd");
    if (searchKbd) {
      searchKbd.textContent = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘K" : "Ctrl K";
    }
    var savedFilter = localStorage.getItem("docsmith-filter");
    var filterEl = document.getElementById("queue-state-filter");
    if (savedFilter && filterEl) {
      filterEl.value = savedFilter;
    }
    filterEl.addEventListener("change", function () {
      localStorage.setItem("docsmith-filter", filterEl.value);
      refreshQueue();
    });

    if (location.pathname.indexOf("/view/") === 0) {
      var viewId = decodeURIComponent(location.pathname.slice("/view/".length));
      if (viewId) {
        history.replaceState(
          null,
          "",
          "#/reader/" + encodeURIComponent(viewId)
        );
      }
    }

    initWasm()
      .then(function () {
        refreshTelemetry();
        refreshCapabilities();
        refreshQueue();
        applyRoute();
      })
      .catch(function () {
        applyRoute();
      });

    window.addEventListener("hashchange", applyRoute);
    setInterval(function () {
      refreshTelemetry();
      refreshQueue();
    }, 5000);
  });
})();
