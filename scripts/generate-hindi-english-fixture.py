#!/usr/bin/env python3
"""Generate the committed Hindi + English OCR stress fixture and its ground truth.

The PDF is deliberately raster-only (like a scanner): three A4 pages at 150 DPI with dense
Devanagari and Latin text, a marks table, bullets, stamps, a barcode-like block, subtle noise,
and one page scanned sideways. It exercises Tesseract `eng+hin`, page rendering, rotation
selection and OCR block ordering without requiring any PDF text-shaping library.

macOS fonts are used because this repository's fixtures are generated on macOS. The generated
PDF/TXT are committed so other platforms do not need these fonts to run the accuracy test.

    python3 scripts/generate-hindi-english-fixture.py
"""

import pathlib
import random
import subprocess

from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parents[1]
FIX = ROOT / "fixtures"
PDF_PATH = FIX / "hindi-english-complex.pdf"
TRUTH_PATH = FIX / "hindi-english-complex.ground-truth.txt"
IMAGE_DIR = ROOT / "target/hindi-english-complex"

W, H = 1240, 1754  # A4 at 150 DPI
MARGIN = 72
INK = (24, 28, 36)
MUTED = (70, 78, 92)
ACCENT = (21, 82, 140)
RED = (170, 34, 52)

DEVANAGARI_FONT = "/System/Library/Fonts/Supplemental/Devanagari Sangam MN.ttc"
LATIN_FONT = "/System/Library/Fonts/Supplemental/Arial.ttf"
LATIN_BOLD = "/System/Library/Fonts/Supplemental/Arial Bold.ttf"


def font(path, size):
    try:
        return ImageFont.truetype(path, size, index=0)
    except OSError as error:
        raise SystemExit(f"missing font {path}: {error}")


def has_devanagari(text):
    return any("\u0900" <= ch <= "\u097f" for ch in text)


def mixed_segments(text):
    """Split into runs of Devanagari vs Latin so each run is shaped by the right font."""
    if not text:
        return []
    runs = []
    current = text[0]
    current_dev = has_devanagari(text[0])
    for ch in text[1:]:
        dev = has_devanagari(ch)
        if dev == current_dev:
            current += ch
        else:
            runs.append((current_dev, current))
            current = ch
            current_dev = dev
    runs.append((current_dev, current))
    return runs


def measure(draw, text, size, bold=False):
    total = 0.0
    for dev, run in mixed_segments(text):
        f = font(DEVANAGARI_FONT if dev else (LATIN_BOLD if bold else LATIN_FONT), size)
        total += draw.textlength(run, font=f)
    return total


def draw_mixed(draw, xy, text, size, fill=INK, bold=False, anchor=None):
    runs = mixed_segments(text)
    widths = []
    fonts = []
    for dev, run in runs:
        f = font(DEVANAGARI_FONT if dev else (LATIN_BOLD if bold else LATIN_FONT), size)
        fonts.append(f)
        widths.append(draw.textlength(run, font=f))
    total = sum(widths)
    x, y = xy
    anchor = anchor or "la"
    horizontal = anchor[0] if anchor else "l"
    vertical = anchor[1] if len(anchor) > 1 else "a"
    if horizontal == "m":
        x -= total / 2
    elif horizontal == "r":
        x -= total
    base_anchor = "l" + vertical
    for (dev, run), f, width in zip(runs, fonts, widths):
        draw.text((x, y), run, font=f, fill=fill, anchor=base_anchor)
        x += width
    return x


def wrap_mixed(draw, text, size, max_width, bold=False):
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


def paragraph(draw, xy, text, size, max_width, line_gap=10, fill=INK, bold=False):
    x, y = xy
    for line in wrap_mixed(draw, text, size, max_width, bold):
        draw_mixed(draw, (x, y), line, size, fill, bold)
        y += size + line_gap
    return y


def box(draw, rect, width=2, fill=INK):
    draw.rectangle(rect, outline=fill, width=width)


def seal(draw, center, radius, label, sublabel):
    cx, cy = center
    draw.ellipse(
        (cx - radius, cy - radius, cx + radius, cy + radius),
        outline=RED,
        width=5,
    )
    draw.ellipse(
        (cx - radius + 10, cy - radius + 10, cx + radius - 10, cy + radius - 10),
        outline=RED,
        width=1,
    )
    draw_mixed(draw, (cx, cy - 18), label, 28, RED, True, anchor="mm")
    draw_mixed(draw, (cx, cy + 20), sublabel, 22, RED, False, anchor="mm")


def barcode(draw, rect):
    x0, y0, x1, y1 = rect
    random.seed(20261022)
    x = x0
    while x < x1 - 2:
        width = random.choice([2, 3, 4, 6])
        if random.random() > 0.35:
            draw.rectangle((x, y0, min(x + width, x1), y1), fill=INK)
        x += width + random.choice([2, 3, 5])


def noise(image, strength=6):
    grain = Image.effect_noise(image.size, strength).convert("L")
    return Image.blend(image, Image.merge("RGB", (grain, grain, grain)), 0.045)


def watermark(image, text):
    layer = Image.new("RGBA", image.size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(layer)
    draw_mixed(draw, (W // 2, H // 2), text, 110, (120, 130, 150, 24), True, anchor="mm")
    rotated = layer.rotate(28, expand=False)
    image.paste(Image.alpha_composite(image.convert("RGBA"), rotated).convert("RGB"), (0, 0))


def table(draw, x, y, widths, rows, header=True, row_height=58, font_size=22, header_size=20):
    total_width = sum(widths)
    height = row_height * len(rows)
    draw.rectangle((x, y, x + total_width, y + height), outline=INK, width=2)
    for index, row in enumerate(rows):
        cy = y + index * row_height
        if index > 0:
            draw.line((x, cy, x + total_width, cy), fill=MUTED, width=1)
        cx = x
        for col, (width, cell) in enumerate(zip(widths, row)):
            if col > 0:
                draw.line((cx, cy, cx, cy + row_height), fill=MUTED, width=1)
            is_header = header and index == 0
            text = str(cell)
            size = header_size if is_header else font_size
            lines = wrap_mixed(draw, text, size, width - 16, bold=is_header)
            ty = cy + (row_height - len(lines) * (size + 2)) / 2 + 2
            for line in lines:
                draw_mixed(
                    draw,
                    (cx + 8, ty),
                    line,
                    size,
                    ACCENT if is_header else INK,
                    bold=is_header,
                )
                ty += size + 2
            cx += width


def page_one():
    image = Image.new("RGB", (W, H), (252, 251, 247))
    draw = ImageDraw.Draw(image)
    draw_mixed(draw, (W // 2, 52), "॥ सत्यमेव जयते ॥", 28, ACCENT, True, anchor="mm")
    draw_mixed(draw, (W // 2, 96), "माध्यमिक शिक्षा परिषद, उत्तर प्रदेश", 36, INK, True, anchor="mm")
    draw_mixed(
        draw,
        (W // 2, 142),
        "Board of High School and Intermediate Education",
        24,
        MUTED,
        False,
        anchor="mm",
    )
    draw_mixed(
        draw,
        (W // 2, 184),
        "प्रमाणपत्र-सह-अंकतालिका / CERTIFICATE-CUM-MARKS SHEET",
        27,
        ACCENT,
        True,
        anchor="mm",
    )
    draw.line((MARGIN, 210, W - MARGIN, 210), fill=ACCENT, width=2)

    info = [
        ("नाम / Name", "उदेश शर्मा (Udesh Sharma)"),
        ("पिता का नाम / Father's Name", "श्रीकांत शर्मा (Shrikant Sharma)"),
        ("माता का नाम / Mother's Name", "इंद्रावती देवी (Indrawati Devi)"),
        ("अनुक्रमांक / Roll No.", "7534469"),
        ("जन्म तिथि / Date of Birth", "17 November 2000"),
        ("विद्यालय / School", "ST MARY'S C GORAKHPUR"),
    ]
    y = 238
    for label, value in info:
        draw_mixed(draw, (MARGIN, y), f"{label}:", 22, MUTED, True)
        draw_mixed(draw, (MARGIN + 430, y), value, 22, INK)
        y += 38

    y += 20
    rows = [
        ["विषय / Subject", "अधिकतम / Max", "सैद्धांतिक / Theory", "प्रयोगात्मक / Practical", "कुल / Total", "श्रेणी / Grade"],
        ["हिंदी / Hindi", "100", "40", "28", "68", "B2"],
        ["अंग्रेज़ी / English", "100", "50", "29", "79", "B1"],
        ["गणित / Mathematics", "100", "52", "29", "81", "A2"],
        ["विज्ञान / Science", "100", "55", "28", "83", "A1"],
        ["सामाजिक विज्ञान / Social Science", "100", "55", "28", "83", "A1"],
        ["चित्रकला / Drawing", "100", "53", "29", "82", "A2"],
    ]
    table(draw, MARGIN, y, [300, 150, 160, 170, 130, 130], rows)
    y += 58 * len(rows) + 26
    draw_mixed(draw, (MARGIN, y), "परिणाम / Result: PASSED", 28, ACCENT, True)
    y += 48
    draw_mixed(
        draw,
        (MARGIN, y),
        "यह प्रमाणपत्र केवल शैक्षणिक परीक्षण के लिए है। / This certificate is for educational testing only.",
        19,
        MUTED,
    )

    seal(draw, (W - 200, H - 230), 92, "VERIFIED", "सत्यापित")
    barcode(draw, (MARGIN, H - 150, MARGIN + 330, H - 104))
    draw_mixed(draw, (MARGIN, H - 92), "7534469 · 2497006 · TEST DOCUMENT", 16, MUTED)
    watermark(image, "SPECIMEN / नमूना")
    return noise(image)


def page_two():
    image = Image.new("RGB", (W, H), (253, 252, 250))
    draw = ImageDraw.Draw(image)
    draw_mixed(draw, (MARGIN, 60), "जिला शिक्षा कार्यालय, गोरखपुर", 34, ACCENT, True)
    draw_mixed(draw, (MARGIN, 106), "District Education Office, Gorakhpur", 23, MUTED)
    draw.line((MARGIN, 140, W - MARGIN, 140), fill=ACCENT, width=2)
    draw_mixed(draw, (MARGIN, 165), "विद्यालय निरीक्षण रिपोर्ट / SCHOOL INSPECTION REPORT", 27, INK, True)
    draw_mixed(draw, (MARGIN, 210), "दिनांक / Date: 12 August 2026", 20, MUTED)

    hindi = (
        "दिनांक 12 अगस्त 2026 को विद्यालय का निरीक्षण किया गया। कक्षा 10 के "
        "विद्यार्थियों की उपस्थिति 92 प्रतिशत पाई गई। विज्ञान प्रयोगशाला में "
        "उपकरण पर्याप्त हैं, परंतु कंप्यूटर कक्ष में केवल आठ कार्यशील मशीनें थीं। "
        "पुस्तकालय में 2,450 पुस्तकें हैं और पेयजल की व्यवस्था संतोषजनक है।"
    )
    english = (
        "The school was inspected on 12 August 2026. Attendance in Grade 10 was "
        "92 percent. Science laboratory equipment is adequate, but the computer "
        "laboratory has only eight working machines. The library holds 2,450 books "
        "and drinking water is satisfactory."
    )
    y = 248
    y = paragraph(draw, (MARGIN, y), hindi, 22, W - 2 * MARGIN, 8)
    y += 12
    y = paragraph(draw, (MARGIN, y), english, 22, W - 2 * MARGIN, 8)
    y += 26
    draw_mixed(draw, (MARGIN, y), "मुख्य अवलोकन / Key Findings", 26, ACCENT, True)
    y += 42
    bullets = [
        "पुस्तकालय में 2,450 पुस्तकें हैं। / Library holds 2,450 books.",
        "पेयजल की व्यवस्था संतोषजनक है। / Drinking water is satisfactory.",
        "शौचालयों की सफाई में सुधार आवश्यक है। / Toilets need cleaning improvements.",
        "कंप्यूटर कक्ष में आठ मशीनें कार्यशील हैं। / Eight computers are working.",
    ]
    for item in bullets:
        draw_mixed(draw, (MARGIN, y), "•", 24, ACCENT, True)
        y = paragraph(draw, (MARGIN + 28, y), item, 20, W - 2 * MARGIN - 28, 5)
        y += 8

    y += 12
    rows = [
        ["क्र.सं. / No.", "मद / Item", "स्थिति / Status", "अंक / Score"],
        ["1", "Infrastructure / अधोसंरचना", "Good / अच्छा", "8 / 10"],
        ["2", "Library / पुस्तकालय", "Satisfactory / संतोषजनक", "7 / 10"],
        ["3", "Laboratory / प्रयोगशाला", "Good / अच्छा", "9 / 10"],
        ["4", "Computer Lab / कंप्यूटर कक्ष", "Needs Improvement / सुधार आवश्यक", "5 / 10"],
    ]
    table(draw, MARGIN, y, [150, 330, 390, 170], rows, row_height=64, font_size=20)
    y += 64 * len(rows) + 42
    draw_mixed(draw, (MARGIN, y), "निरीक्षण अधिकारी / Inspecting Officer", 20, MUTED, True)
    draw_mixed(draw, (MARGIN, y + 34), "डॉ. अनीता वर्मा (Dr. Anita Verma)", 23, INK)
    draw_mixed(draw, (MARGIN, y + 70), "हस्ताक्षर / Signature", 18, MUTED)

    seal(draw, (W - 210, H - 210), 88, "APPROVED", "स्वीकृत")
    barcode(draw, (MARGIN, H - 130, MARGIN + 300, H - 88))
    draw_mixed(draw, (MARGIN, H - 70), "रिपोर्ट संख्या / Report No.: DEO/2026/0812", 17, MUTED)
    watermark(image, "OFFICIAL / कार्यालय")
    return noise(image)


def page_three_rotated():
    # Draw upright on a landscape canvas, then rotate the whole scan 90° clockwise.
    lw, lh = H, W
    image = Image.new("RGB", (lw, lh), (251, 250, 246))
    draw = ImageDraw.Draw(image)
    draw_mixed(draw, (MARGIN, 70), "अनुपालन प्रमाण / COMPLIANCE CERTIFICATE", 36, ACCENT, True)
    draw.line((MARGIN, 118, lw - MARGIN, 118), fill=ACCENT, width=2)
    y = 150
    text = (
        "यह प्रमाणित किया जाता है कि यह दस्तावेज़ OCR परीक्षण के लिए तैयार है। "
        "इसमें हिंदी और अंग्रेज़ी दोनों भाषाओं का पाठ, एक तालिका और आधिकारिक मुद्रा है।"
    )
    y = paragraph(draw, (MARGIN, y), text, 24, lw - 2 * MARGIN, 9)
    y += 14
    text_en = (
        "This is to certify that the document is ready for OCR testing. It contains "
        "Hindi and English text, a table and an official stamp. The page was scanned "
        "sideways to exercise automatic rotation."
    )
    y = paragraph(draw, (MARGIN, y), text_en, 24, lw - 2 * MARGIN, 9)
    y += 24
    rows = [
        ["Parameter / मापदंड", "Value / मान", "Status / स्थिति"],
        ["Language / भाषा", "Hindi + English", "Verified / सत्यापित"],
        ["Pages / पृष्ठ", "3", "Complete / पूर्ण"],
        ["Rotation / घूर्णन", "90 degrees", "Recovered / पुनर्प्राप्त"],
    ]
    table(draw, MARGIN, y, [380, 360, 330], rows, row_height=66, font_size=22)
    y += 66 * len(rows) + 36
    draw_mixed(draw, (MARGIN, y), "प्रमाणित करने वाला अधिकारी / Certifying Officer", 21, MUTED, True)
    draw_mixed(draw, (MARGIN, y + 34), "रजिस्ट्रार / Registrar", 24, INK)
    seal(draw, (lw - 230, lh - 210), 92, "TESTED", "परीक्षित")
    barcode(draw, (MARGIN, lh - 120, MARGIN + 320, lh - 78))
    draw_mixed(draw, (MARGIN, lh - 58), "TEST ONLY · NO LEGAL VALIDITY", 17, MUTED)
    watermark(image, "TEST / परीक्षण")
    return noise(image.rotate(-90, expand=True))


def main():
    FIX.mkdir(exist_ok=True)
    IMAGE_DIR.mkdir(exist_ok=True)
    pages = [page_one(), page_two(), page_three_rotated()]
    for index, page in enumerate(pages, 1):
        page.save(IMAGE_DIR / f"page-{index}.png", optimize=True)
    first, rest = pages[0], pages[1:]
    first.save(
        PDF_PATH,
        "PDF",
        resolution=150.0,
        save_all=True,
        append_images=rest,
        title="Hindi English OCR stress fixture",
        author="doc-extractor test fixture",
    )
    TRUTH_PATH.write_text(
        """=== PAGE 1 ===
॥ सत्यमेव जयते ॥
माध्यमिक शिक्षा परिषद, उत्तर प्रदेश
Board of High School and Intermediate Education
प्रमाणपत्र-सह-अंकतालिका / CERTIFICATE-CUM-MARKS SHEET
नाम / Name: उदेश शर्मा (Udesh Sharma)
पिता का नाम / Father's Name: श्रीकांत शर्मा (Shrikant Sharma)
माता का नाम / Mother's Name: इंद्रावती देवी (Indrawati Devi)
अनुक्रमांक / Roll No.: 7534469
जन्म तिथि / Date of Birth: 17 November 2000
विद्यालय / School: ST MARY'S C GORAKHPUR
विषय / Subject: अधिकतम / Max: सैद्धांतिक / Theory: प्रयोगात्मक / Practical: कुल / Total: श्रेणी / Grade
हिंदी / Hindi: 100: 40: 28: 68: B2
अंग्रेज़ी / English: 100: 50: 29: 79: B1
गणित / Mathematics: 100: 52: 29: 81: A2
विज्ञान / Science: 100: 55: 28: 83: A1
सामाजिक विज्ञान / Social Science: 100: 55: 28: 83: A1
चित्रकला / Drawing: 100: 53: 29: 82: A2
परिणाम / Result: PASSED
यह प्रमाणपत्र केवल शैक्षणिक परीक्षण के लिए है। / This certificate is for educational testing only.
VERIFIED
सत्यापित
7534469 · 2497006 · TEST DOCUMENT

=== PAGE 2 ===
जिला शिक्षा कार्यालय, गोरखपुर
District Education Office, Gorakhpur
विद्यालय निरीक्षण रिपोर्ट / SCHOOL INSPECTION REPORT
दिनांक / Date: 12 August 2026
दिनांक 12 अगस्त 2026 को विद्यालय का निरीक्षण किया गया। कक्षा 10 के विद्यार्थियों की उपस्थिति 92 प्रतिशत पाई गई। विज्ञान प्रयोगशाला में उपकरण पर्याप्त हैं, परंतु कंप्यूटर कक्ष में केवल आठ कार्यशील मशीनें थीं। पुस्तकालय में 2,450 पुस्तकें हैं और पेयजल की व्यवस्था संतोषजनक है।
The school was inspected on 12 August 2026. Attendance in Grade 10 was 92 percent. Science laboratory equipment is adequate, but the computer laboratory has only eight working machines. The library holds 2,450 books and drinking water is satisfactory.
मुख्य अवलोकन / Key Findings
• पुस्तकालय में 2,450 पुस्तकें हैं। / Library holds 2,450 books.
• पेयजल की व्यवस्था संतोषजनक है। / Drinking water is satisfactory.
• शौचालयों की सफाई में सुधार आवश्यक है। / Toilets need cleaning improvements.
• कंप्यूटर कक्ष में आठ मशीनें कार्यशील हैं। / Eight computers are working.
क्र.सं. / No.: मद / Item: स्थिति / Status: अंक / Score
1: Infrastructure / अधोसंरचना: Good / अच्छा: 8 / 10
2: Library / पुस्तकालय: Satisfactory / संतोषजनक: 7 / 10
3: Laboratory / प्रयोगशाला: Good / अच्छा: 9 / 10
4: Computer Lab / कंप्यूटर कक्ष: Needs Improvement / सुधार आवश्यक: 5 / 10
निरीक्षण अधिकारी / Inspecting Officer
डॉ. अनीता वर्मा (Dr. Anita Verma)
हस्ताक्षर / Signature
APPROVED
स्वीकृत
रिपोर्ट संख्या / Report No.: DEO/2026/0812

=== PAGE 3 ===
अनुपालन प्रमाण / COMPLIANCE CERTIFICATE
यह प्रमाणित किया जाता है कि यह दस्तावेज़ OCR परीक्षण के लिए तैयार है। इसमें हिंदी और अंग्रेज़ी दोनों भाषाओं का पाठ, एक तालिका और आधिकारिक मुद्रा है।
This is to certify that the document is ready for OCR testing. It contains Hindi and English text, a table and an official stamp. The page was scanned sideways to exercise automatic rotation.
Parameter / मापदंड: Value / मान: Status / स्थिति
Language / भाषा: Hindi + English: Verified / सत्यापित
Pages / पृष्ठ: 3: Complete / पूर्ण
Rotation / घूर्णन: 90 degrees: Recovered / पुनर्प्राप्त
प्रमाणित करने वाला अधिकारी / Certifying Officer
रजिस्ट्रार / Registrar
TESTED
परीक्षित
TEST ONLY · NO LEGAL VALIDITY
""",
        encoding="utf-8",
    )
    print(f"wrote {PDF_PATH} ({PDF_PATH.stat().st_size:} bytes)")
    print(f"wrote {TRUTH_PATH} ({TRUTH_PATH.stat().st_size:} bytes)")
    info = subprocess.run(["pdfinfo", str(PDF_PATH)], capture_output=True, text=True, check=True)
    for line in info.stdout.splitlines():
        if line.startswith(("Pages:", "Page size:")):
            print(line)


if __name__ == "__main__":
    main()
