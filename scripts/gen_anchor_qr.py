"""
Generate a print-ready anchor QR for ALVR_Lynx.

Payload format: "<letter><size_cm>" (e.g. "A13.3").
  - letter   : distinguishes codes of the same size (A, B, ...)
  - size_cm  : the code's OWN black-module side length, in cm — this is the
               value the client uses for PnP, so the printed code must match it.

The script renders the code so its own square (excluding the quiet-zone border)
is exactly CODE_SIZE_CM at 300 DPI, and draws a ruler across the code itself so
you can verify after printing (measure the black square, not the sheet).
"""

import qrcode
from qrcode.constants import ERROR_CORRECT_M
from PIL import Image, ImageDraw, ImageFont

# ---- Config ----------------------------------------------------------------
LETTER = "A"
CODE_SIZE_CM = 13.3          # the code's own (black-module) side length
QR_STRING = f"{LETTER}{CODE_SIZE_CM}"   # -> "A13.3"
BORDER_MODULES = 4           # quiet zone (required for detection)
DPI = 300
OUT_PATH = rf"E:\CC_Project\ALVR\build\ALVR_Lynx_Anchor_{LETTER}_{CODE_SIZE_CM}cm.png"

A4_W = int(8.27 * DPI)
A4_H = int(11.69 * DPI)
CM_TO_PX = DPI / 2.54

# ---- Build QR with the code square == CODE_SIZE_CM --------------------------
qr = qrcode.QRCode(error_correction=ERROR_CORRECT_M, border=BORDER_MODULES)
qr.add_data(QR_STRING)
qr.make(fit=True)
modules = qr.modules_count  # code modules, excluding the border

code_px = round(CODE_SIZE_CM * CM_TO_PX)
box = max(1, round(code_px / modules))
qr.box_size = box

qr_img = qr.make_image(fill_color="black", back_color="white").convert("RGB")

code_actual_px = modules * box                  # the black square's size
border_px = BORDER_MODULES * box
total_px = qr_img.size[0]                        # (modules + 2*border) * box
code_actual_cm = code_actual_px / CM_TO_PX

# ---- Compose A4 page --------------------------------------------------------
page = Image.new("RGB", (A4_W, A4_H), "white")
draw = ImageDraw.Draw(page)

qr_x = (A4_W - total_px) // 2
qr_y = (A4_H - total_px) // 2
page.paste(qr_img, (qr_x, qr_y))

# Code-square edges on the page (the black square, inside the quiet zone).
code_left = qr_x + border_px
code_right = code_left + code_actual_px
code_top = qr_y + border_px
code_bottom = code_top + code_actual_px

try:
    font_big = ImageFont.truetype("arial.ttf", 70)
    font_mid = ImageFont.truetype("arial.ttf", 48)
    font_small = ImageFont.truetype("arial.ttf", 36)
except Exception:
    font_big = ImageFont.load_default()
    font_mid = ImageFont.load_default()
    font_small = ImageFont.load_default()

def center_text(y, text, font, fill="black"):
    w = draw.textbbox((0, 0), text, font=font)[2]
    draw.text(((A4_W - w) // 2, y), text, font=font, fill=fill)

center_text(qr_y - 220, f"ALVR_Lynx Anchor  '{LETTER}'", font_big)
center_text(qr_y - 130, f'Payload: "{QR_STRING}"', font_mid)

# Size guide spanning the CODE SQUARE itself (not the sheet).
guide_y = code_bottom + 70
draw.line([(code_left, guide_y), (code_right, guide_y)], fill="black", width=4)
for gx in (code_left, code_right):
    draw.line([(gx, guide_y - 20), (gx, guide_y + 20)], fill="black", width=4)
center_text(guide_y + 30,
            f"Black square side must be {code_actual_cm:.1f} cm (measure to verify)",
            font_small)
center_text(guide_y + 90,
            "Print at 100% (no 'fit to page'). If the measured size differs,",
            font_small)
center_text(guide_y + 140,
            "regenerate with CODE_SIZE_CM = measured value so payload matches.",
            font_small)

page.save(OUT_PATH, dpi=(DPI, DPI))
print(f"Saved: {OUT_PATH}")
print(f"Payload: {QR_STRING}")
print(f"Code square: target {CODE_SIZE_CM} cm, actual {code_actual_cm:.2f} cm "
      f"({modules} modules x {box}px)")
