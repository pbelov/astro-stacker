# -*- coding: utf-8 -*-
"""Иконки приложения и файлов проекта из одного описания знака.

Знак — спиральная галактика «Вертушка». Рукав собран из тонких дуг, тем же
приёмом и той же палитрой, что иконка соседнего проекта star-trails: там дуги
концентрические, следы звёзд вокруг полюса, здесь раскручены в спираль.

Приём выбран не за красоту. Два других проверку мелким размером провалили:
россыпь точек на 16 пикселях усредняется в тёплое пятно, потому что точка
мельче пикселя не рисуется; сплошные лопасти размер держат, но читаются как
вентилятор. Пучок дуг даёт письмо соседа на 512 и сплошную массу на 16.

Мелкие размеры рисуются отдельным упрощённым знаком, а не уменьшением
большого: одиннадцать дуг в рукаве на 24 пикселях сливаются в кашу, пять
дуг потолще — нет. Силуэт у обоих один.

Растеризует headless Chrome или Edge: один из них на Windows есть всегда, а
тащить в проект зависимость ради семи PNG незачем. Скрипт запускается руками,
когда знак меняется; результат лежит в репозитории.

    python tools/gen-icons.py
"""
import io
import math
import os
import random
import shutil
import subprocess
import tempfile

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__))).replace("\\", "/")
ICONS = ROOT + "/apps/desktop/src-tauri/icons"
UI = ROOT + "/apps/desktop/src/ui"

# Палитра снята пипеткой с иконки star-trails, включая фон: семейство должно
# быть видно, а не заявлено.
BG_TOP, BG_BOTTOM = "#1a1f33", "#0e1326"
AMBER, BLUE, WHITE = "#e6b25e", "#9fb6e0", "#f2f5fb"
C = 256.0

BROWSERS = [
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
]


# --- знак -------------------------------------------------------------------
def spiral(theta0, sweep, r0, r1, g):
    def at(t):
        r = r0 + (r1 - r0) * (t ** g)
        th = theta0 + sweep * t
        return r * math.cos(th), r * math.sin(th)

    return at


def normal_at(at, t):
    ax, ay = at(min(1.0, t + 0.01))
    bx, by = at(max(0.0, t - 0.01))
    dx, dy = ax - bx, ay - by
    d = math.hypot(dx, dy) or 1.0
    return -dy / d, dx / d


def arc(rnd, at, u, t0, t1, w0, ti, to, peak, colour, sw, op, n):
    """Одна дуга вдоль спирали, смещённая поперёк рукава на долю u."""
    pts = []
    for i in range(n + 1):
        t = t0 + (t1 - t0) * i / n
        x, y = at(t)
        nx, ny = normal_at(at, t)
        w = w0 * (t ** ti) * ((1 - t) ** to) / peak * u
        pts.append((C + x + nx * w, C + y + ny * w))
    d = "M%.1f,%.1f" % pts[0] + "".join("L%.1f,%.1f" % p for p in pts[1:])
    return (
        '<path d="%s" fill="none" stroke="%s" stroke-width="%.1f"'
        ' stroke-linecap="round" opacity="%.2f"/>' % (d, colour, sw, op)
    )


def sheaf(rnd, theta0, sweep, r0, r1, w0, g, ti, to, m, sw, op, amber, n=40):
    """Рукав пучком дуг: у каждой своё смещение поперёк и своя длина."""
    at = spiral(theta0, sweep, r0, r1, g)
    tp = ti / (ti + to)
    peak = (tp ** ti) * ((1 - tp) ** to)
    out = []
    for s in range(m):
        u = -1 + 2 * (s + 0.5) / m + rnd.uniform(-0.10, 0.10)
        colour = AMBER if rnd.random() < amber else (WHITE if rnd.random() < 0.30 else BLUE)
        out.append(arc(rnd, at, u, rnd.uniform(0.0, 0.26), rnd.uniform(0.74, 1.0),
                       w0, ti, to, peak, colour, rnd.uniform(*sw), rnd.uniform(*op), n))
    return out


def wheel(arms=3, sweep=3.75, r0=40, r1=220, w0=26, g=0.74, ti=0.40, to=0.60,
          core=13, bulge=84, strokes=11, root=5, field=8, seed=12,
          sw=(4.0, 7.5), op=(0.55, 0.95)):
    rnd = random.Random(seed)
    out = ['<circle cx="256" cy="256" r="250" fill="url(#disc)"/>']
    band = []
    for k in range(arms):
        a0 = k * 2 * math.pi / arms - 1.2
        band += sheaf(rnd, a0, sweep, r0, r1, w0, g, ti, to, strokes, sw, op, 0.28)
        # Отдельный янтарный пучок у корня: в галактике старые звёзды внутри,
        # молодые голубые снаружи, и заодно это два цвета семьи в одном знаке.
        band += sheaf(rnd, a0, sweep * 0.46, r0, r0 + (r1 - r0) * 0.46, w0 * 0.80,
                      g, ti, to, root, sw, op, 0.88)
    # Дальние дуги уходят за край, как у соседа: фактура на 512, ничто на 16.
    for _ in range(field):
        a0 = rnd.uniform(0, 2 * math.pi)
        band += sheaf(rnd, a0, sweep * 0.55, r1 * 0.92, r1 * 1.55, w0 * 0.30,
                      g, ti, to, 1, sw, op, 0.28)
    out.append('<g mask="url(#fade)">' + "".join(band) + "</g>")
    out.append('<circle cx="256" cy="256" r="%d" fill="url(#bulge)"/>' % bulge)
    out.append('<circle cx="256" cy="256" r="%d" fill="%s"/>' % (core, WHITE))
    return "".join(out)


# На 24 и 16 пикселях подробность вредит: дуги сливаются в кашу, и от знака
# остаётся пятно. Тот же силуэт, но дуг втрое меньше и они толще.
def small_wheel():
    return wheel(strokes=5, root=3, field=3, w0=32, r1=234, core=16, bulge=92,
                 sw=(9.0, 13.5), op=(0.78, 1.0))


DEFS = (
    "<defs>"
    '<linearGradient id="bg" x1="0.15" y1="1" x2="0.9" y2="0">'
    '<stop offset="0" stop-color="' + BG_BOTTOM + '"/>'
    '<stop offset="1" stop-color="' + BG_TOP + '"/></linearGradient>'
    '<radialGradient id="bulge">'
    '<stop offset="0" stop-color="' + WHITE + '" stop-opacity="0.90"/>'
    '<stop offset="0.16" stop-color="' + AMBER + '" stop-opacity="0.86"/>'
    '<stop offset="0.52" stop-color="' + AMBER + '" stop-opacity="0.30"/>'
    '<stop offset="1" stop-color="' + AMBER + '" stop-opacity="0"/></radialGradient>'
    '<radialGradient id="disc">'
    '<stop offset="0" stop-color="' + BLUE + '" stop-opacity="0.30"/>'
    '<stop offset="0.42" stop-color="' + BLUE + '" stop-opacity="0.15"/>'
    '<stop offset="0.80" stop-color="' + BLUE + '" stop-opacity="0.04"/>'
    '<stop offset="1" stop-color="' + BLUE + '" stop-opacity="0"/></radialGradient>'
    '<radialGradient id="fadeg">'
    '<stop offset="0" stop-color="#fff" stop-opacity="1"/>'
    '<stop offset="0.60" stop-color="#fff" stop-opacity="1"/>'
    '<stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient>'
    '<mask id="fade"><rect width="512" height="512" fill="url(#fadeg)"/></mask>'
    "</defs>"
)

HEAD = '<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">'


def app_svg(body):
    """Иконка приложения: квадрат в край, без своего скругления — как у соседа."""
    return HEAD + DEFS + '<rect width="512" height="512" fill="url(#bg)"/>' + body + "</svg>"


def file_svg(body):
    """Иконка файла проекта: лист с загнутым углом, знак внутри.

    Вокруг листа прозрачно — проводник кладёт такие иконки на свой фон.
    """
    page = (
        '<path d="M104,36 H322 L408,122 V476 A20,20 0 0 1 388,496 H104'
        ' A20,20 0 0 1 84,476 V56 A20,20 0 0 1 104,36 Z" fill="url(#bg)"'
        ' stroke="#33406b" stroke-width="7"/>'
        '<path d="M322,36 L408,122 H342 A20,20 0 0 1 322,102 Z" fill="#2a3556"'
        ' stroke="#33406b" stroke-width="7" stroke-linejoin="round"/>'
    )
    # Знак меньше листа и сдвинут вниз: место наверху занял загнутый угол.
    s = 0.60
    mark = '<g transform="translate(%.1f,%.1f) scale(%.3f)">%s</g>' % (
        246 - 256 * s, 300 - 256 * s, s, body,
    )
    return HEAD + DEFS + page + mark + "</svg>"


# --- растеризация -----------------------------------------------------------
def browser():
    for path in BROWSERS:
        if os.path.exists(path):
            return path
    raise SystemExit("не найден ни Chrome, ни Edge — растеризовать нечем")


def render(svg, px, work):
    """SVG в PIL через headless-браузер. Фон прозрачный, если его нет в самом SVG."""
    page = os.path.join(work, "page.html")
    out = os.path.join(work, "shot.png")
    io.open(page, "w", encoding="utf-8").write(
        '<!doctype html><meta charset="utf-8"><style>html,body{margin:0;padding:0;'
        "background:transparent}svg{display:block;width:%dpx;height:%dpx}</style>%s"
        % (px, px, svg)
    )
    if os.path.exists(out):
        os.remove(out)
    subprocess.run(
        [browser(), "--headless=new", "--disable-gpu", "--hide-scrollbars",
         "--default-background-color=00000000", "--virtual-time-budget=4000",
         "--user-data-dir=" + os.path.join(work, "profile"),
         "--screenshot=" + out, "--window-size=%d,%d" % (px, px),
         "file:///" + page.replace("\\", "/")],
        capture_output=True, check=True,
    )
    if not os.path.exists(out):
        raise SystemExit("браузер не отдал картинку")
    return Image.open(out).convert("RGBA")


def at(im, size):
    return im.resize((size, size), Image.LANCZOS)


def main():
    detail, small = wheel(), small_wheel()
    work = tempfile.mkdtemp(prefix="astro-icons-")
    try:
        big = render(app_svg(detail), 1024, work)
        tiny = render(app_svg(small), 1024, work)
        page_big = render(file_svg(detail), 1024, work)
        page_tiny = render(file_svg(small), 1024, work)
    finally:
        shutil.rmtree(work, ignore_errors=True)

    io.open(ICONS + "/icon.svg", "w", encoding="utf-8", newline="\n").write(app_svg(detail))
    io.open(ICONS + "/file.svg", "w", encoding="utf-8", newline="\n").write(file_svg(detail))

    at(big, 512).save(ICONS + "/icon.png")
    at(big, 256).save(ICONS + "/128x128@2x.png")
    at(big, 128).save(ICONS + "/128x128.png")
    at(tiny, 32).save(ICONS + "/32x32.png")
    at(big, 256).save(UI + "/app-icon.png")

    # Ниже 48 берётся упрощённый знак, выше — подробный.
    for name, low, high in (("icon.ico", tiny, big), ("file.ico", page_tiny, page_big)):
        sizes = [16, 24, 32, 48, 64, 128, 256]
        frames = [at(low if s < 48 else high, s) for s in sizes]
        frames[-1].save(ICONS + "/" + name, format="ICO",
                        sizes=[(s, s) for s in sizes], append_images=frames)

    for f in ("icon.svg", "file.svg", "icon.png", "128x128@2x.png", "128x128.png",
              "32x32.png", "icon.ico", "file.ico"):
        print("%-18s %7d Б" % (f, os.path.getsize(ICONS + "/" + f)))


if __name__ == "__main__":
    main()
