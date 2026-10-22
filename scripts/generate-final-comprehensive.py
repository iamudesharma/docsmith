#!/usr/bin/env python3
"""Generate the final all-in-one OCR test PDF and its ground-truth TXT.

Six raster pages at 150 DPI (scanner simulation) cover:
  1. Hindi/English marks certificate with merged table headers, photo, seal, barcode
  2. Tax invoice with line-item table, GST totals, QR, signature, stamp
  3. Architecture flowchart + ER diagram with Hindi/English labels
  4. Bar, pie and line charts plus a values table
  5. Dense two-column policy text, formulas, bullets, footnotes and a merged table
  6. A sideways (90° rotated) compliance form with checkboxes, signature and stamp

The PDF and TXT are written under fixtures/ and copied to ~/Downloads for manual testing.

    python3 scripts/generate-final-comprehensive.py
"""

import pathlib
import random
import shutil
import subprocess

from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parents[1]
FIX = ROOT / "fixtures"
PDF_PATH = FIX / "final-comprehensive.pdf"
TRUTH_PATH = FIX / "final-comprehensive.ground-truth.txt"
MARKERS_PATH = FIX / "final-comprehensive.markers.json"
IMAGE_DIR = ROOT / "target/final-comprehensive"
DOWNLOADS = pathlib.Path.home() / "Downloads"

W, H = 1240, 1754
MARGIN = 64
INK = (24, 28, 36)
MUTED = (72, 80, 94)
ACCENT = (20, 77, 140)
GREEN = (22, 112, 78)
RED = (166, 30, 46)
PURPLE = (100, 55, 160)
ORANGE = (190, 105, 20)
LIGHT = (250, 249, 245)

DEV = "/System/Library/Fonts/Supplemental/Devanagari Sangam MN.ttc"
LAT = "/System/Library/Fonts/Supplemental/Arial.ttf"
LAT_B = "/System/Library/Fonts/Supplemental/Arial Bold.ttf"
SCRIPT = "/System/Library/Fonts/Supplemental/SnellRoundhand.ttc"


def font(path, size):
    try:
        return ImageFont.truetype(path, size, index=0)
    except OSError as error:
        raise SystemExit(f"missing font {path}: {error}")


def dev(text):
    return any("\u0900" <= ch <= "\u097f" for ch in text)


def segments(text):
    if not text:
        return []
    out = []
    cur = text[0]
    cur_dev = dev(text[0])
    for ch in text[1:]:
        d = dev(ch)
        if d == cur_dev:
            cur += ch
        else:
            out.append((cur_dev, cur))
            cur = ch
            cur_dev = d
    out.append((cur_dev, cur))
    return out


def measure(draw, text, size, bold=False):
    total = 0.0
    for is_dev, run in segments(text):
        f = font(DEV if is_dev else (LAT_B if bold else LAT), size)
        total += draw.textlength(run, font=f)
    return total


def mixed(draw, xy, text, size, fill=INK, bold=False, anchor=None):
    runs = segments(text)
    widths = []
    fonts = []
    for is_dev, run in runs:
        f = font(DEV if is_dev else (LAT_B if bold else LAT), size)
        fonts.append(f)
        widths.append(draw.textlength(run, font=f))
    total = sum(widths)
    x, y = xy
    anchor = anchor or "la"
    horizontal = anchor[0]
    vertical = anchor[1] if len(anchor) > 1 else "a"
    if horizontal == "m":
        x -= total / 2
    elif horizontal == "r":
        x -= total
    base = "l" + vertical
    for (_, run), f, width in zip(runs, fonts, widths):
        draw.text((x, y), run, font=f, fill=fill, anchor=base)
        x += width
    return x


def wrap(draw, text, size, max_width, bold=False):
    words = text.split()
    lines = []
    line = ""
    for word in words:
        candidate = f"{line} {word}".strip()
        if line and measure(draw, candidate, size, bold) > max_width:
            lines.append(line)
            line = word
        else:
            line = candidate
    if line:
        lines.append(line)
    return lines


def para(draw, xy, text, size, width, gap=8, fill=INK, bold=False):
    x, y = xy
    for line in wrap(draw, text, size, width, bold):
        mixed(draw, (x, y), line, size, fill, bold)
        y += size + gap
    return y


def table(draw, x, y, widths, rows, row_height=56, size=20, header=True):
    """rows: list of rows; each row is list of (text, colspan)."""
    total = sum(widths)
    height = row_height * len(rows)
    draw.rectangle((x, y, x + total, y + height), outline=INK, width=2)
    for r, row in enumerate(rows):
        cy = y + r * row_height
        if r:
            draw.line((x, cy, x + total, cy), fill=MUTED, width=1)
        col = 0
        cx = x
        for text, span in row:
            cw = sum(widths[col : col + span])
            draw.line((cx, cy, cx, cy + row_height), fill=MUTED, width=1)
            is_header = header and r == 0
            lines = wrap(draw, str(text), size, cw - 14, is_header)
            ty = cy + (row_height - len(lines) * (size + 2)) / 2 + 2
            for line in lines:
                mixed(draw, (cx + 7, ty), line, size, ACCENT if is_header else INK, is_header)
                ty += size + 2
            cx += cw
            col += span
        draw.line((x + total, cy, x + total, cy + row_height), fill=MUTED, width=1)


def seal(draw, center, radius, top, bottom, color=RED):
    cx, cy = center
    draw.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), outline=color, width=5)
    draw.ellipse(
        (cx - radius + 10, cy - radius + 10, cx + radius - 10, cy + radius - 10),
        outline=color,
        width=1,
    )
    mixed(draw, (cx, cy - 18), top, 26, color, True, anchor="mm")
    mixed(draw, (cx, cy + 20), bottom, 21, color, False, anchor="mm")


def barcode(draw, rect, seed=11):
    x0, y0, x1, y1 = rect
    rng = random.Random(seed)
    x = x0
    while x < x1 - 2:
        w = rng.choice([2, 3, 4, 6])
        if rng.random() > 0.32:
            draw.rectangle((x, y0, min(x + w, x1), y1), fill=INK)
        x += w + rng.choice([2, 3, 5])


def qr_like(draw, x, y, size=150, seed=7):
    rng = random.Random(seed)
    cell = size // 21
    for row in range(21):
        for col in range(21):
            dark = rng.random() < 0.45
            if (row < 7 and col < 7) or (row < 7 and col > 13) or (row > 13 and col < 7):
                dark = (row in (0, 6) or col in (0, 6)) or (2 <= row <= 4 and 2 <= col <= 4)
            if dark:
                draw.rectangle(
                    (x + col * cell, y + row * cell, x + (col + 1) * cell - 1, y + (row + 1) * cell - 1),
                    fill=INK,
                )


def signature(draw, x, y, name, size=30):
    f = ImageFont.truetype(SCRIPT, size, index=0)
    draw.text((x, y), name, font=f, fill=(20, 45, 90))


def noise(image, strength=5):
    grain = Image.effect_noise(image.size, strength).convert("L")
    return Image.blend(image, Image.merge("RGB", (grain, grain, grain)), 0.035)


def watermark(image, text):
    layer = Image.new("RGBA", image.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    mixed(draw, (W // 2, H // 2), text, 100, (130, 140, 155, 22), True, anchor="mm")
    image.paste(Image.alpha_composite(image.convert("RGBA"), layer.rotate(26)).convert("RGB"), (0, 0))


def page_certificate():
    image = Image.new("RGB", (W, H), LIGHT)
    draw = ImageDraw.Draw(image)
    truth = []

    def t(text):
        truth.append(text)
        return text

    mixed(draw, (W // 2, 48), t("॥ सत्यमेव जयते ॥"), 26, ACCENT, True, "mm")
    mixed(draw, (W // 2, 94), t("राज्य शिक्षा बोर्ड / STATE EDUCATION BOARD"), 32, INK, True, "mm")
    mixed(draw, (W // 2, 136), t("हाई स्कूल प्रमाणपत्र / HIGH SCHOOL CERTIFICATE"), 26, ACCENT, True, "mm")
    draw.line((MARGIN, 162, W - MARGIN, 162), fill=ACCENT, width=2)

    # Photo placeholder + QR
    draw.rectangle((W - 270, 185, W - 110, 345), outline=MUTED, width=2)
    draw.ellipse((W - 215, 208, W - 165, 258), outline=MUTED, width=2)
    draw.arc((W - 225, 255, W - 155, 335), 200, 340, fill=MUTED, width=2)
    mixed(draw, (W - 190, 352), t("PHOTO / फोटो"), 15, MUTED, False, "ma")

    info = [
        ("नाम / Name", "उदेश शर्मा (Udesh Sharma)"),
        ("पिता का नाम / Father's Name", "श्रीकांत शर्मा (Shrikant Sharma)"),
        ("माता का नाम / Mother's Name", "इंद्रावती देवी (Indrawati Devi)"),
        ("अनुक्रमांक / Roll No.", "7534469"),
        ("जन्म तिथि / Date of Birth", "17 November 2000"),
        ("विद्यालय / School", "ST MARY'S C GORAKHPUR"),
        ("परीक्षा वर्ष / Exam Year", "2026"),
    ]
    y = 192
    for label, value in info:
        mixed(draw, (MARGIN, y), t(f"{label}: {value}"), 19, INK)
        y += 31

    y = 430
    mixed(draw, (MARGIN, y), t("विषयवार अंक विवरण / SUBJECT-WISE MARKS"), 24, ACCENT, True)
    y += 38
    rows = [
        [
            ("विषय / Subject", 1),
            ("अधिकतम / Max", 1),
            ("प्राप्तांक / Marks Obtained", 3),
            ("श्रेणी / Grade", 1),
        ],
        [
            ("", 1),
            ("", 1),
            ("सैद्धांतिक / Theory", 1),
            ("प्रयोगात्मक / Practical", 1),
            ("कुल / Total", 1),
            ("", 1),
        ],
        [("हिंदी / Hindi", 1), ("100", 1), ("40", 1), ("28", 1), ("68", 1), ("B2", 1)],
        [("अंग्रेज़ी / English", 1), ("100", 1), ("50", 1), ("29", 1), ("79", 1), ("B1", 1)],
        [("गणित / Mathematics", 1), ("100", 1), ("52", 1), ("29", 1), ("81", 1), ("A2", 1)],
        [("विज्ञान / Science", 1), ("100", 1), ("55", 1), ("28", 1), ("83", 1), ("A1", 1)],
        [("सामाजिक विज्ञान / Social Science", 1), ("100", 1), ("55", 1), ("28", 1), ("83", 1), ("A1", 1)],
        [("चित्रकला / Drawing", 1), ("100", 1), ("53", 1), ("29", 1), ("82", 1), ("A2", 1)],
        [("कुल योग / Grand Total", 4), ("476 / 600", 1), ("A1", 1)],
    ]
    for row in rows:
        t(" | ".join(cell for cell, _ in row if cell))
    table(draw, MARGIN, y, [280, 138, 150, 158, 122, 108], rows, row_height=58, size=20)

    y += 58 * len(rows) + 24
    mixed(draw, (MARGIN, y), t("परिणाम / Result: PASSED"), 25, GREEN, True)
    mixed(draw, (MARGIN + 300, y), t("प्रतिशत / Percentage: 79.33%"), 22, INK, True)
    y += 38
    mixed(
        draw,
        (MARGIN, y),
        t("यह प्रमाणपत्र केवल शैक्षणिक परीक्षण के लिए है। / This certificate is for educational testing only."),
        17,
        MUTED,
    )
    seal(draw, (W - 205, H - 240), 88, "VERIFIED", "सत्यापित")
    barcode(draw, (MARGIN, H - 170, MARGIN + 320, H - 124), seed=31)
    mixed(draw, (MARGIN, H - 108), t("Roll 7534469 · Certificate 2497006 · TEST ONLY"), 15, MUTED)
    qr_like(draw, MARGIN + 420, H - 220, 130, seed=32)
    watermark(image, "SPECIMEN / नमूना")
    return noise(image), truth


def page_invoice():
    image = Image.new("RGB", (W, H), (253, 252, 249))
    draw = ImageDraw.Draw(image)
    truth = []

    def t(text):
        truth.append(text)
        return text

    draw.ellipse((MARGIN, 54, MARGIN + 64, 118), fill=ACCENT)
    mixed(draw, (MARGIN + 32, 86), "DE", 22, (255, 255, 255), True, "mm")
    mixed(draw, (MARGIN + 84, 54), t("Doc Extractor Supplies Pvt. Ltd."), 27, INK, True)
    mixed(draw, (MARGIN + 84, 90), t("डॉक एक्सट्रैक्टर सप्लाईज़ प्राइवेट लिमिटेड"), 20, MUTED)
    mixed(draw, (MARGIN + 84, 118), t("42 Knowledge Park, Gorakhpur, Uttar Pradesh 273001"), 16, MUTED)
    mixed(draw, (MARGIN + 84, 140), t("GSTIN: 09AABCD1234E1Z5 · support@example.test"), 16, MUTED)
    mixed(draw, (W - MARGIN, 58), t("TAX INVOICE / कर चालान"), 30, ACCENT, True, "ra")
    mixed(draw, (W - MARGIN, 98), t("Invoice No.: INV-2026-0042"), 18, INK, False, "ra")
    mixed(draw, (W - MARGIN, 124), t("Date: 22 October 2026"), 18, INK, False, "ra")
    mixed(draw, (W - MARGIN, 150), t("Place of Supply: Uttar Pradesh (09)"), 16, MUTED, False, "ra")
    draw.line((MARGIN, 180, W - MARGIN, 180), fill=ACCENT, width=2)

    mixed(draw, (MARGIN, 200), t("Billed To / प्राप्तकर्ता"), 19, MUTED, True)
    mixed(draw, (MARGIN, 230), t("Udesh Sharma"), 23, INK, True)
    mixed(draw, (MARGIN, 260), t("12 Civil Lines, Gorakhpur, Uttar Pradesh 273001"), 17, INK)
    mixed(draw, (MARGIN, 284), t("GSTIN: 09AAAPZ1234F1Z2 · +91 98765 43210"), 17, MUTED)
    mixed(draw, (W - 470, 200), t("Shipped To / भेजा गया"), 19, MUTED, True)
    mixed(draw, (W - 470, 230), t("Same as billing address"), 18, INK)

    y = 330
    rows = [
        [("#", 1), ("Description / विवरण", 3), ("HSN", 1), ("Qty", 1), ("Rate", 1), ("GST", 1), ("Amount", 1)],
        [("1", 1), ("Document OCR licence (annual) / दस्तावेज़ OCR लाइसेंस", 3), ("998314", 1), ("1", 1), ("24,000.00", 1), ("18%", 1), ("24,000.00", 1)],
        [("2", 1), ("Hindi + English language pack / हिंदी और अंग्रेज़ी भाषा पैक", 3), ("998314", 1), ("1", 1), ("6,500.00", 1), ("18%", 1), ("6,500.00", 1)],
        [("3", 1), ("On-site installation (2 days) / ऑन-साइट इंस्टॉलेशन", 3), ("998313", 1), ("2", 1), ("3,000.00", 1), ("18%", 1), ("6,000.00", 1)],
        [("4", 1), ("Priority support (12 months) / प्राथमिकता सहायता", 3), ("998314", 1), ("1", 1), ("4,500.00", 1), ("18%", 1), ("4,500.00", 1)],
        [("5", 1), ("Training workshop / प्रशिक्षण कार्यशाला", 3), ("999293", 1), ("1", 1), ("2,500.00", 1), ("18%", 1), ("2,500.00", 1)],
    ]
    for row in rows:
        t(" | ".join(cell for cell, _ in row))
    table(draw, MARGIN, y, [45, 170, 150, 120, 80, 70, 110, 70, 120], rows, row_height=72, size=18)
    y += 72 * len(rows) + 24

    totals = [
        ("Subtotal / उप-योग", "43,500.00"),
        ("CGST @ 9% / केंद्रीय GST", "3,915.00"),
        ("SGST @ 9% / राज्य GST", "3,915.00"),
        ("Grand Total / कुल योग", "51,330.00"),
    ]
    for label, value in totals:
        t(f"{label}: {value}")
    box_x = W - MARGIN - 390
    for index, (label, value) in enumerate(totals):
        yy = y + index * 38
        fill = ACCENT if index == len(totals) - 1 else INK
        mixed(draw, (box_x, yy), label, 19, fill, index == len(totals) - 1)
        mixed(draw, (W - MARGIN, yy), value, 19, fill, True, "ra")
    draw.rectangle((box_x - 8, y - 6, W - MARGIN + 4, y + 38 * len(totals) + 2), outline=ACCENT, width=2)

    y += 38 * len(totals) + 18
    mixed(draw, (MARGIN, y), t("Amount in words / शब्दों में राशि: Fifty-one thousand three hundred thirty rupees only."), 18, INK, True)
    y += 42
    mixed(draw, (MARGIN, y), t("Terms / शर्तें: Payment due within 15 days. Goods once licensed are non-refundable. GST as applicable."), 16, MUTED)
    y += 34
    mixed(draw, (MARGIN, y), t("Declaration / घोषणा: We declare that this invoice shows the actual price of the goods described and that all particulars are true and correct."), 15, MUTED)

    signature(draw, W - 560, H - 250, "Anita Verma")
    mixed(draw, (W - 560, H - 210), t("Authorised Signatory / अधिकृत हस्ताक्षरकर्ता"), 15, MUTED)
    seal(draw, (W - 180, H - 210), 78, "PAID", "भुगतान")
    barcode(draw, (MARGIN, H - 150, MARGIN + 300, H - 105), seed=42)
    mixed(draw, (MARGIN, H - 82), t("INV-2026-0042 · GSTIN 09AABCD1234E1Z5 · TEST ONLY"), 14, MUTED)
    qr_like(draw, MARGIN + 380, H - 230, 135, seed=43)
    watermark(image, "SAMPLE / नमूना")
    return noise(image), truth


def page_diagrams():
    image = Image.new("RGB", (W, H), (252, 252, 250))
    draw = ImageDraw.Draw(image)
    truth = []

    def t(text):
        truth.append(text)
        return text

    mixed(draw, (MARGIN, 54), t("System Architecture & Data Model / सिस्टम आर्किटेक्चर और डेटा मॉडल"), 27, ACCENT, True)
    draw.line((MARGIN, 92, W - MARGIN, 92), fill=ACCENT, width=2)
    mixed(draw, (MARGIN, 112), t("A. Document extraction flow / दस्तावेज़ निष्कर्षण प्रवाह"), 21, INK, True)

    boxes = [
        (MARGIN, 170, 170, 76, "Upload\nअपलोड"),
        (MARGIN + 235, 170, 170, 76, "Validate\nजाँच"),
        (MARGIN + 470, 170, 180, 76, "OCR / PDF\nपाठ निष्कर्षण"),
        (MARGIN + 715, 170, 180, 76, "Canonical JSON\nकैनोनिकल JSON"),
        (MARGIN + 960, 170, 150, 76, "Web UI / API\nवेब UI / API"),
    ]
    for x, y, w, h, label in boxes:
        draw.rounded_rectangle((x, y, x + w, y + h), radius=8, outline=ACCENT, width=2, fill=(240, 247, 253))
        top, bottom = label.split("\n")
        mixed(draw, (x + w / 2, y + 24), top, 16, INK, True, "mm")
        mixed(draw, (x + w / 2, y + 50), bottom, 15, MUTED, False, "mm")
        t(label.replace("\n", " / "))
    for i in range(4):
        x1 = boxes[i][0] + boxes[i][2]
        x2 = boxes[i + 1][0]
        y = boxes[i][1] + 38
        draw.line((x1 + 8, y, x2 - 14, y), fill=ACCENT, width=3)
        draw.polygon([(x2 - 14, y - 6), (x2 - 2, y), (x2 - 14, y + 6)], fill=ACCENT)

    mixed(draw, (MARGIN, 292), t("B. Entity relationship model / एंटिटी रिलेशनशिप मॉडल"), 21, INK, True)
    entities = {
        "CUSTOMER / ग्राहक": (MARGIN, 340, 250, 128, ["id: UUID PK", "name: VARCHAR", "email: VARCHAR UK", "gstin: CHAR(15)"]),
        "INVOICE / चालान": (MARGIN + 400, 340, 260, 152, ["id: UUID PK", "customer_id: UUID FK", "invoice_no: VARCHAR UK", "total: NUMERIC", "issued_at: DATE"]),
        "LINE_ITEM / पंक्ति": (MARGIN + 800, 340, 280, 152, ["id: UUID PK", "invoice_id: UUID FK", "product_id: UUID FK", "qty: INT", "amount: NUMERIC"]),
        "PRODUCT / उत्पाद": (MARGIN + 800, 570, 280, 128, ["id: UUID PK", "sku: VARCHAR UK", "name: VARCHAR", "price: NUMERIC"]),
    }
    for title, (x, y, w, h, fields) in entities.items():
        draw.rectangle((x, y, x + w, y + h), outline=PURPLE, width=2, fill=(249, 246, 255))
        draw.rectangle((x, y, x + w, y + 30), fill=(235, 226, 250))
        mixed(draw, (x + 10, y + 8), title, 16, PURPLE, True)
        t(title)
        for i, field in enumerate(fields):
            mixed(draw, (x + 10, y + 38 + i * 20), field, 14, INK)
            t(field)
    # relationships
    draw.line((MARGIN + 250, 404, MARGIN + 400, 404), fill=PURPLE, width=2)
    mixed(draw, (MARGIN + 300, 380), "1..*", 14, PURPLE, True, "mm")
    t("CUSTOMER 1..* INVOICE")
    draw.line((MARGIN + 660, 416, MARGIN + 800, 416), fill=PURPLE, width=2)
    mixed(draw, (MARGIN + 730, 392), "1..*", 14, PURPLE, True, "mm")
    t("INVOICE 1..* LINE_ITEM")
    draw.line((MARGIN + 940, 492, MARGIN + 940, 570), fill=PURPLE, width=2)
    mixed(draw, (MARGIN + 950, 525), "1..*", 14, PURPLE, True)
    t("PRODUCT 1..* LINE_ITEM")

    mixed(draw, (MARGIN, 740), t("Legend / संकेत"), 20, INK, True)
    draw.line((MARGIN, 780, MARGIN + 80, 780), fill=ACCENT, width=3)
    mixed(draw, (MARGIN + 95, 767), t("Solid arrow = verified data flow / सत्यापित डेटा प्रवाह"), 16, MUTED)
    draw.rectangle((MARGIN, 805, MARGIN + 80, 835), outline=PURPLE, width=2, fill=(249, 246, 255))
    mixed(draw, (MARGIN + 95, 808), t("Box = entity / diagram node; relationships are inferred from layout"), 16, MUTED)
    mixed(draw, (MARGIN, 870), t("Note / ध्यान दें: OCR recovers labels and boxes, but connector direction and cardinality remain unverified."), 17, ORANGE, True)
    barcode(draw, (MARGIN, H - 130, MARGIN + 280, H - 88), seed=51)
    mixed(draw, (MARGIN, H - 62), t("DIAGRAM TEST · FIG-2026-03 · TEST ONLY"), 14, MUTED)
    watermark(image, "DIAGRAM / आरेख")
    return noise(image), truth


def page_charts():
    image = Image.new("RGB", (W, H), (253, 253, 251))
    draw = ImageDraw.Draw(image)
    truth = []

    def t(text):
        truth.append(text)
        return text

    mixed(draw, (MARGIN, 50), t("Quarterly Performance Report / तिमाही प्रदर्शन रिपोर्ट"), 28, ACCENT, True)
    mixed(draw, (MARGIN, 90), t("Documents processed, accuracy and language mix — 2026"), 19, MUTED)
    draw.line((MARGIN, 120, W - MARGIN, 120), fill=ACCENT, width=2)

    # Bar chart
    chart_x, chart_y, chart_w, chart_h = MARGIN, 160, 700, 380
    mixed(draw, (chart_x, chart_y - 28), t("A. Documents by quarter / तिमाही में दस्तावेज़"), 20, INK, True)
    draw.line((chart_x + 50, chart_y, chart_x + 50, chart_y + chart_h - 40), fill=MUTED, width=2)
    draw.line((chart_x + 50, chart_y + chart_h - 40, chart_x + chart_w, chart_y + chart_h - 40), fill=MUTED, width=2)
    values = [("Q1", 68), ("Q2", 79), ("Q3", 83), ("Q4", 92)]
    bar_w = 90
    for i, (label, value) in enumerate(values):
        bx = chart_x + 95 + i * 145
        bh = (value / 100) * (chart_h - 90)
        draw.rectangle((bx, chart_y + chart_h - 40 - bh, bx + bar_w, chart_y + chart_h - 40), fill=ACCENT)
        mixed(draw, (bx + bar_w / 2, chart_y + chart_h - 30), label, 17, INK, True, "ma")
        mixed(draw, (bx + bar_w / 2, chart_y + chart_h - 50 - bh), str(value), 21, INK, True, "ma")
        t(f"{label}: {value} documents")
    for tick in [0, 25, 50, 75, 100]:
        ty = chart_y + chart_h - 40 - (tick / 100) * (chart_h - 90)
        draw.line((chart_x + 45, ty, chart_x + 52, ty), fill=MUTED, width=1)
        mixed(draw, (chart_x + 10, ty - 8), str(tick), 13, MUTED)

    # Pie chart
    pie_x, pie_y, pie_d = W - 430, 205, 300
    mixed(draw, (pie_x + pie_d / 2, 138), t("B. Language mix / भाषा मिश्रण"), 20, INK, True, "ma")
    slices = [("Hindi / हिंदी", 30, ACCENT), ("English / अंग्रेज़ी", 35, GREEN), ("Tables / तालिका", 20, ORANGE), ("Charts / चार्ट", 10, PURPLE), ("Other / अन्य", 5, RED)]
    start = -90
    for label, pct, color in slices:
        end = start + pct * 3.6
        draw.pieslice((pie_x, pie_y, pie_x + pie_d, pie_y + pie_d), start, end, fill=color, outline=(255, 255, 255), width=3)
        start = end
    for i, (label, pct, color) in enumerate(slices):
        ly = pie_y + pie_d + 18 + i * 25
        draw.rectangle((pie_x, ly + 4, pie_x + 16, ly + 20), fill=color)
        mixed(draw, (pie_x + 25, ly), f"{label}: {pct}%", 16, INK)
        t(f"{label}: {pct}%")

    # Line chart
    lx, ly, lw, lh = MARGIN, 640, 700, 340
    mixed(draw, (lx, ly - 28), t("C. Monthly accuracy / मासिक शुद्धता"), 20, INK, True)
    draw.line((lx + 50, ly, lx + 50, ly + lh - 35), fill=MUTED, width=2)
    draw.line((lx + 50, ly + lh - 35, lx + lw, ly + lh - 35), fill=MUTED, width=2)
    points = [("Jan", 91), ("Feb", 92), ("Mar", 93), ("Apr", 94), ("May", 95), ("Jun", 96)]
    coords = []
    for i, (month, value) in enumerate(points):
        px = lx + 90 + i * 100
        py = ly + lh - 55 - ((value - 88) / 10) * (lh - 80)
        coords.append((px, py))
        mixed(draw, (px, ly + lh - 25), month, 15, MUTED, False, "ma")
        mixed(draw, (px, py - 18), str(value), 17, INK, True, "ma")
        t(f"{month}: {value}% accuracy")
    draw.line(coords, fill=GREEN, width=3)
    for px, py in coords:
        draw.ellipse((px - 5, py - 5, px + 5, py + 5), fill=GREEN, outline=(255, 255, 255), width=2)

    # Values table
    tx, ty = W - 440, 700
    mixed(draw, (tx, ty - 28), t("D. Summary table / सारांश तालिका"), 20, INK, True)
    rows = [
        [("Quarter / तिमाही", 1), ("Docs", 1), ("Accuracy", 1)],
        [("Q1", 1), ("68", 1), ("91%", 1)],
        [("Q2", 1), ("79", 1), ("92%", 1)],
        [("Q3", 1), ("83", 1), ("93%", 1)],
        [("Q4", 1), ("92", 1), ("96%", 1)],
    ]
    for row in rows:
        t(" | ".join(cell for cell, _ in row))
    table(draw, tx, ty, [170, 100, 120], rows, row_height=48, size=17)
    barcode(draw, (MARGIN, H - 120, MARGIN + 290, H - 78), seed=61)
    mixed(draw, (MARGIN, H - 52), t("CHART TEST · CH-2026-04 · TEST ONLY"), 14, MUTED)
    watermark(image, "CHART / चार्ट")
    return noise(image), truth


def page_multicolumn():
    image = Image.new("RGB", (W, H), (252, 251, 248))
    draw = ImageDraw.Draw(image)
    truth = []

    def t(text):
        truth.append(text)
        return text

    mixed(draw, (MARGIN, 48), t("Policy, Mathematics & Reading Order / नीति, गणित और पठन क्रम"), 27, ACCENT, True)
    mixed(draw, (MARGIN, 86), t("Two columns, formulas, lists, footnotes and a merged table"), 18, MUTED)
    draw.line((MARGIN, 116, W - MARGIN, 116), fill=ACCENT, width=2)

    left_x, right_x = MARGIN, W // 2 + 24
    col_w = W // 2 - MARGIN - 40
    draw.line((W // 2, 140, W // 2, 790), fill=MUTED, width=1)

    hindi = (
        "यह दस्तावेज़ दो स्तंभों में व्यवस्थित है। बायाँ स्तंभ हिंदी पाठ, सूची और "
        "गणितीय सूत्र दिखाता है। OCR को पंक्तियों और स्तंभों का सही क्रम पहचानना चाहिए। "
        "दस्तावेज़ में तकनीकी शब्द, संख्याएँ और मिश्रित भाषा का प्रयोग किया गया है।"
    )
    y = 142
    y = para(draw, (left_x, y), t(hindi), 19, col_w, 7)
    y += 18
    mixed(draw, (left_x, y), t("मुख्य बिंदु / Key points"), 20, ACCENT, True)
    y += 30
    for i, item in enumerate(
        [
            "१. पाठ की सटीकता और पठन क्रम दोनों महत्वपूर्ण हैं।",
            "२. तालिकाओं में मर्ज किए गए कक्षों का परीक्षण करें।",
            "३. सूत्रों और विशेष चिह्नों को सुरक्षित रखें।",
        ]
    ):
        y = para(draw, (left_x, y), t(item), 17, col_w, 4)
        y += 5
    y += 18
    mixed(draw, (left_x, y), t("गणितीय सूत्र / Formulas"), 20, ACCENT, True)
    y += 32
    formulas = ["E = mc²", "क्षेत्रफल = πr²", "H2O + CO2", "1/2 + 1/3 = 5/6"]
    for formula in formulas:
        draw.rectangle((left_x, y - 3, left_x + col_w, y + 28), fill=(240, 246, 252), outline=(200, 216, 234))
        mixed(draw, (left_x + 10, y + 3), t(formula), 21, INK, True)
        y += 38

    english = (
        "This page deliberately mixes two columns, a dense paragraph, numbered steps, "
        "mathematical formulas, footnotes and a table with a merged total row. The OCR "
        "engine must retain numbers, punctuation and Devanagari conjuncts while preserving "
        "a useful reading order. Column boundaries are marked by a vertical rule."
    )
    y = 142
    y = para(draw, (right_x, y), t(english), 18, col_w, 7)
    y += 18
    mixed(draw, (right_x, y), t("Procedure / प्रक्रिया"), 20, ACCENT, True)
    y += 30
    for item in [
        "1. Upload the document and select Hindi + English.",
        "2. Render scanned pages at 150 DPI and run OCR.",
        "3. Preserve native text, OCR text and inferred layout separately.",
        "4. Download JSON, Markdown, plain text or the page archive.",
    ]:
        y = para(draw, (right_x, y), t(item), 17, col_w, 4)
        y += 5
    y += 16
    mixed(draw, (right_x, y), t("Notes / टिप्पणियाँ"), 20, ACCENT, True)
    y += 30
    y = para(
        draw,
        (right_x, y),
        t("Footnotes: 1. OCR confidence is uncalibrated. 2. Table structure is not inferred from scan lines. 3. Rotation is selected by relative OCR scores."),
        15,
        col_w,
        4,
        MUTED,
    )

    ty = 820
    mixed(draw, (MARGIN, ty - 30), t("Merged summary table / मर्ज सारांश तालिका"), 20, ACCENT, True)
    rows = [
        [("Metric / मापदंड", 2), ("Value / मान", 1), ("Status / स्थिति", 1)],
        [("Native text pages / मूल पाठ", 2), ("Digital", 1), ("Pass", 1)],
        [("Scanned pages / स्कैन", 2), ("3", 1), ("Pass", 1)],
        [("Table cells / तालिका कक्ष", 2), ("30", 1), ("Pass", 1)],
        [("Merged total row / मर्ज पंक्ति", 3), ("Verified", 1)],
    ]
    for row in rows:
        t(" | ".join(cell for cell, _ in row))
    table(draw, MARGIN, ty, [260, 250, 200, 200], rows, row_height=54, size=18)
    barcode(draw, (MARGIN, H - 125, MARGIN + 280, H - 83), seed=71)
    mixed(draw, (MARGIN, H - 55), t("MULTI-COLUMN TEST · MC-2026-05 · TEST ONLY"), 14, MUTED)
    watermark(image, "POLICY / नीति")
    return noise(image), truth


def page_rotated_form():
    lw, lh = H, W
    image = Image.new("RGB", (lw, lh), (252, 251, 248))
    draw = ImageDraw.Draw(image)
    truth = []

    def t(text):
        truth.append(text)
        return text

    mixed(draw, (MARGIN, 46), t("Compliance Inspection Form / अनुपालन निरीक्षण फॉर्म"), 30, ACCENT, True)
    mixed(draw, (MARGIN, 88), t("Form No.: CI-2026-06 · Date: 22 October 2026"), 18, MUTED)
    draw.line((MARGIN, 116, lw - MARGIN, 116), fill=ACCENT, width=2)

    fields = [
        ("Institution / संस्थान", "ST MARY'S C GORAKHPUR"),
        ("Inspector / निरीक्षक", "Dr. Anita Verma"),
        ("Designation / पद", "District Education Officer"),
        ("Attendance / उपस्थिति", "92 percent"),
        ("Library books / पुस्तकालय पुस्तकें", "2,450"),
        ("Working computers / कार्यशील कंप्यूटर", "8"),
    ]
    y = 140
    for label, value in fields:
        mixed(draw, (MARGIN, y), t(f"{label}: {value}"), 18, INK)
        y += 30

    checks = [
        ("Science laboratory equipment adequate / विज्ञान प्रयोगशाला पर्याप्त", "Yes / हाँ"),
        ("Drinking water satisfactory / पेयजल संतोषजनक", "Yes / हाँ"),
        ("Toilets need improvement / शौचालय सुधार आवश्यक", "No / नहीं"),
        ("Fire safety certificate valid / अग्नि सुरक्षा प्रमाणपत्र", "Yes / हाँ"),
    ]
    y += 12
    mixed(draw, (MARGIN, y), t("Checklist / जाँच सूची"), 20, ACCENT, True)
    y += 32
    for label, value in checks:
        draw.rectangle((MARGIN, y, MARGIN + 24, y + 24), outline=INK, width=2)
        mixed(draw, (MARGIN + 8, y + 3), "X" if value.startswith("Yes") else "", 16, GREEN, True)
        mixed(draw, (MARGIN + 38, y + 2), t(f"{label}: {value}"), 17, INK)
        y += 34

    y += 10
    rows = [
        [("Area / क्षेत्र", 1), ("Score / अंक", 1), ("Remarks / टिप्पणी", 2)],
        [("Infrastructure", 1), ("8 / 10", 1), ("Good", 2)],
        [("Library", 1), ("7 / 10", 1), ("Satisfactory", 2)],
        [("Laboratory", 1), ("9 / 10", 1), ("Good", 2)],
        [("Computer Lab", 1), ("5 / 10", 1), ("Needs improvement", 2)],
    ]
    for row in rows:
        t(" | ".join(cell for cell, _ in row))
    table(draw, MARGIN, y, [200, 150, 160, 150], rows, row_height=52, size=18)
    y += 52 * len(rows) + 24
    mixed(draw, (MARGIN, y), t("Overall rating / समग्र श्रेणी: SATISFACTORY"), 22, GREEN, True)
    y += 42
    mixed(draw, (MARGIN, y), t("Inspector signature / निरीक्षक हस्ताक्षर:"), 18, MUTED, True)
    signature(draw, MARGIN + 320, y - 18, "Anita Verma", 28)
    mixed(draw, (MARGIN, y + 38), t("Date: 22 October 2026 · Place: Gorakhpur"), 16, MUTED)
    seal(draw, (lw - 210, lh - 200), 88, "INSPECTED", "निरीक्षित")
    barcode(draw, (MARGIN, lh - 120, MARGIN + 300, lh - 78), seed=81)
    mixed(draw, (MARGIN, lh - 54), t("FORM CI-2026-06 · ROTATED 90° · TEST ONLY"), 15, MUTED)
    qr_like(draw, lw - 520, lh - 260, 130, seed=82)
    watermark(image, "FORM / फॉर्म")
    rotated = noise(image).rotate(-90, expand=True)
    return rotated, truth


def main():
    FIX.mkdir(exist_ok=True)
    IMAGE_DIR.mkdir(parents=True, exist_ok=True)
    pages = []
    truths = []
    builders = [
        page_certificate,
        page_invoice,
        page_diagrams,
        page_charts,
        page_multicolumn,
        page_rotated_form,
    ]
    for builder in builders:
        image, truth = builder()
        pages.append(image)
        truths.append(truth)
    for index, image in enumerate(pages, 1):
        image.save(IMAGE_DIR / f"page-{index}.png", optimize=True)
    pages[0].save(
        PDF_PATH,
        "PDF",
        resolution=150.0,
        save_all=True,
        append_images=pages[1:],
        title="Final comprehensive Hindi English OCR test",
        author="doc-extractor test fixture",
    )
    truth_text = "\n\n".join(
        f"=== PAGE {index} ===\n" + "\n".join(lines) for index, lines in enumerate(truths, 1)
    )
    TRUTH_PATH.write_text(truth_text + "\n", encoding="utf-8")
    MARKERS_PATH.write_text(
        __import__("json").dumps(
            {
                "pages": {str(i): markers for i, markers in enumerate(
                    [
                        ["उदेश शर्मा", "7534469", "PASSED", "79.33%", "476 / 600", "सत्यापित", "STATE EDUCATION BOARD", "B2", "A1"],
                        ["INV-2026-0042", "51,330.00", "Doc Extractor Supplies", "GSTIN", "PAID", "Fifty-one thousand"],
                        ["CUSTOMER", "INVOICE", "LINE_ITEM", "PRODUCT", "1..*", "Canonical JSON", "Data Model"],
                        ["Q1", "Q4", "96% accuracy", "Language mix", "Hindi", "English", "Summary table"],
                        ["E = mc²", "क्षेत्रफल = πr²", "H2O + CO2", "1/2 + 1/3 = 5/6", "Merged total row", "Footnotes"],
                        ["Compliance Inspection", "Dr. Anita Verma", "SATISFACTORY", "Yes", "No", "INSPECTED"],
                    ],
                    1,
                )},
                "table_tokens": ["476 / 600", "51,330.00", "24,000.00", "8 / 10", "2,450", "96%"],
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {PDF_PATH} ({PDF_PATH.stat().st_size} bytes)")
    print(f"wrote {TRUTH_PATH} ({TRUTH_PATH.stat().st_size} bytes)")
    print(f"wrote {MARKERS_PATH}")
    if DOWNLOADS.is_dir():
        shutil.copyfile(PDF_PATH, DOWNLOADS / "final-comprehensive-test.pdf")
        shutil.copyfile(TRUTH_PATH, DOWNLOADS / "final-comprehensive-test.ground-truth.txt")
        print(f"copied to {DOWNLOADS / 'final-comprehensive-test.pdf'}")
        print(f"copied to {DOWNLOADS / 'final-comprehensive-test.ground-truth.txt'}")
    info = subprocess.run(["pdfinfo", str(PDF_PATH)], capture_output=True, text=True, check=True)
    for line in info.stdout.splitlines():
        if line.startswith(("Pages:", "Page size:")):
            print(line)


if __name__ == "__main__":
    main()
