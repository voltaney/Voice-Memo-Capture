# 音声メモツールのアイコンを生成する。
# 角丸タイル（赤グラデ）＋白いマイク。高解像度で描いて各サイズへ縮小し .ico にする。
#
# 実行: python assets/make_icon.py
#   → assets/icon.ico（マルチサイズ）と assets/icon.png（256px）を出力する。
# 依存: Pillow（pip install pillow）
import os
from PIL import Image, ImageDraw

S = 1024
# 既定では自身と同じディレクトリ（assets/）へ出力する。
OUT = os.environ.get("ICON_OUT_DIR", os.path.dirname(os.path.abspath(__file__)))
os.makedirs(OUT, exist_ok=True)

TOP = (239, 91, 80)      # #ef5b50 明るめの赤
BOTTOM = (198, 52, 42)   # #c6342a 深い赤
WHITE = (255, 255, 255, 255)


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def make_tile(size):
    # 縦グラデーション
    grad = Image.new("RGBA", (size, size))
    gd = ImageDraw.Draw(grad)
    for y in range(size):
        gd.line([(0, y), (size, y)], fill=lerp(TOP, BOTTOM, y / (size - 1)) + (255,))

    # 角丸マスク
    mask = Image.new("L", (size, size), 0)
    md = ImageDraw.Draw(mask)
    radius = int(size * 0.235)
    md.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)

    tile = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    tile.paste(grad, (0, 0), mask)

    # マイク（白）を描く
    d = ImageDraw.Draw(tile)
    cx = size / 2
    u = size / 1024.0  # 1024基準のスケール

    import math

    # カプセル（マイク本体）：やや縦長でヘッドっぽく見えないように
    cap_w = 280 * u
    x0, x1 = cx - cap_w / 2, cx + cap_w / 2
    y0, y1 = 200 * u, 580 * u
    d.rounded_rectangle([x0, y0, x1, y1], radius=cap_w / 2, fill=WHITE)

    # クレードル（U字の受け）：本体下部を抱えるように、腕先を本体の下側面に寄せる
    lw = int(round(46 * u))
    ex0, ex1 = cx - 220 * u, cx + 220 * u
    ey0, ey1 = 360 * u, 660 * u
    d.arc([ex0, ey0, ex1, ey1], start=18, end=162, fill=WHITE, width=lw)
    # 弧の端を丸める
    r = lw / 2
    for ang in (18, 162):
        px = (ex0 + ex1) / 2 + (ex1 - ex0) / 2 * math.cos(math.radians(ang))
        py = (ey0 + ey1) / 2 + (ey1 - ey0) / 2 * math.sin(math.radians(ang))
        d.ellipse([px - r, py - r, px + r, py + r], fill=WHITE)

    # ステム（縦の支柱）
    stem_top = 640 * u
    stem_bot = 762 * u
    d.line([(cx, stem_top), (cx, stem_bot)], fill=WHITE, width=lw)

    # ベース（土台）
    bw = 250 * u
    by0, by1 = 760 * u, 808 * u
    d.rounded_rectangle([cx - bw / 2, by0, cx + bw / 2, by1], radius=(by1 - by0) / 2, fill=WHITE)

    return tile


base = make_tile(S)

# PNG（実行中ウィンドウ用 & プレビュー用）
png256 = base.resize((256, 256), Image.LANCZOS)
png256.save(os.path.join(OUT, "icon.png"))

# .ico（各サイズをスーパーサンプルから個別に縮小）
sizes = [16, 24, 32, 48, 64, 128, 256]
imgs = [base.resize((s, s), Image.LANCZOS) for s in sizes]
imgs[-1].save(
    os.path.join(OUT, "icon.ico"),
    format="ICO",
    sizes=[(s, s) for s in sizes],
    append_images=imgs[:-1],
)

# プレビューシート（明/暗の背景に各サイズを並べる）
pv_w, pv_h = 720, 340
pv = Image.new("RGBA", (pv_w, pv_h), (255, 255, 255, 255))
d = ImageDraw.Draw(pv)
d.rectangle([pv_w // 2, 0, pv_w, pv_h], fill=(32, 33, 36, 255))  # 右半分ダーク
x = 40
for s in (256, 128, 64, 48, 32, 24, 16):
    img = base.resize((s, s), Image.LANCZOS)
    y = (pv_h - s) // 2 - 20
    pv.alpha_composite(img, (x, y))
    d.text((x, y + s + 8), f"{s}px", fill=(120, 120, 120, 255))
    x += s + 24
pv.save(os.path.join(OUT, "icon-preview.png"))

print("生成完了:", os.listdir(OUT))
