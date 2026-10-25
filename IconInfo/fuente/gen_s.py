"""Eje central de la S de Swiftbeat y utilidades de curvas (lo importa build_kit.py)."""
import os

OUT = os.path.dirname(os.path.abspath(__file__))
INK = "#111111"

# eje central de la S: segmentos cubicos (p0, c1, c2, p3); dos curvas enlazadas por una espina diagonal
SEGS = [
    ((182, 84), (168, 62), (148, 52), (124, 52)),
    ((124, 52), (94, 52), (74, 70), (74, 94)),
    ((74, 94), (74, 118), (98, 124), (128, 130)),
    ((128, 130), (158, 136), (182, 144), (182, 170)),
    ((182, 170), (182, 194), (160, 206), (130, 206)),
    ((130, 206), (104, 206), (84, 196), (72, 176)),
]


def f(v):
    return f"{v:.2f}".rstrip("0").rstrip(".")


def split(seg, t):
    """De Casteljau: parte un cubico en t y devuelve (izquierda, derecha)."""
    p0, p1, p2, p3 = seg
    lerp = lambda a, b: (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)
    a, b, c = lerp(p0, p1), lerp(p1, p2), lerp(p2, p3)
    d, e = lerp(a, b), lerp(b, c)
    g = lerp(d, e)
    return (p0, a, d, g), (g, e, c, p3)


def path(segs):
    d = f"M{f(segs[0][0][0])} {f(segs[0][0][1])}"
    for _, c1, c2, p in segs:
        d += f" C{f(c1[0])} {f(c1[1])} {f(c2[0])} {f(c2[1])} {f(p[0])} {f(p[1])}"
    return d


def svg(name, body, w=256, h=256):
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as fh:
        fh.write(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}">'
                 f'<title>Swiftbeat S</title>\n{body}\n</svg>\n')


def stroke(d, sw):
    return f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{sw}" stroke-linecap="butt"/>'


def variant(name, sw=44, skew=0, vcut=None, gaps=(), slits=()):
    """vcut=(x_izq, x_der): recorta los remates en vertical. gaps: [(indice_segmento, t0, t1)] huecos."""
    segs = list(SEGS)
    pieces = [segs]
    for idx, t0, t1 in gaps:  # solo huecos en el primer o ultimo segmento
        new = []
        for pc in pieces:
            if pc[0] is SEGS[idx] or pc[-1] is SEGS[idx] or SEGS[idx] in pc:
                i = pc.index(SEGS[idx])
                left, _ = split(SEGS[idx], t0)
                _, right = split(SEGS[idx], t1)
                if pc[:i] or i == 0:
                    new.append(pc[:i] + [left])
                new.append([right] + pc[i + 1:])
            else:
                new.append(pc)
        pieces = new
    body = "".join(stroke(path(pc), sw) for pc in pieces)
    defs = ""
    holes = ""
    if vcut:  # solo en las zonas de los remates (arriba a la derecha, abajo a la izquierda)
        holes += f'<rect x="{vcut[1]}" y="0" width="{256 - vcut[1]}" height="104" fill="#000"/>'
        holes += f'<rect x="0" y="152" width="{vcut[0]}" height="104" fill="#000"/>'
    for (y, h, x0, x1) in slits:  # ranuras horizontales
        holes += f'<rect x="{x0}" y="{y}" width="{x1 - x0}" height="{h}" fill="#000"/>'
    if holes:
        defs = (f'<defs><mask id="m" maskUnits="userSpaceOnUse" x="0" y="0" width="256" height="256">'
                f'<rect width="256" height="256" fill="#fff"/>{holes}</mask></defs>')
        body = f'<g mask="url(#m)">{body}</g>'
    if skew:
        body = f'<g transform="translate(128 128) skewX({skew}) translate(-128 -128)">{body}</g>'
    svg(name, defs + body)
