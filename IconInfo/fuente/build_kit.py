"""Kit final del concepto A (play con la S calada): geometria rellena, sin trazos ni mascaras.

Uso: python build_kit.py <carpeta_salida> <fuente_inter.ttf>
"""
import math, os, sys
from gen_s import SEGS, split

OUT = sys.argv[1]
FONT = sys.argv[2]
os.makedirs(OUT, exist_ok=True)

LAV = "#8486c4"
INK = "#111111"


def f(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


# ---------- S: contorno de un trazo de 42 u (curvas paralelas aproximadas por subdivision) ----------
def bez(seg, t):
    p0, p1, p2, p3 = seg
    u = 1 - t
    return tuple(u**3 * p0[i] + 3 * u * u * t * p1[i] + 3 * u * t * t * p2[i] + t**3 * p3[i] for i in range(2))


def tangent(seg, t):
    p0, p1, p2, p3 = seg
    u = 1 - t
    d = [3 * u * u * (p1[i] - p0[i]) + 6 * u * t * (p2[i] - p1[i]) + 3 * t * t * (p3[i] - p2[i]) for i in range(2)]
    n = math.hypot(*d)
    return d[0] / n, d[1] / n


def offset_seg(seg, dist):
    """Desplaza un cubico: extremos y asas por la normal de su extremo, longitud de asas corregida."""
    p0, p1, p2, p3 = seg
    t0, t1 = tangent(seg, 0), tangent(seg, 1)
    n0, n1 = (-t0[1], t0[0]), (-t1[1], t1[0])
    q0 = (p0[0] + n0[0] * dist, p0[1] + n0[1] * dist)
    q3 = (p3[0] + n1[0] * dist, p3[1] + n1[1] * dist)
    # escala las asas segun el cambio de radio aproximado (cuerda desplazada / cuerda original)
    k = math.dist(q0, q3) / max(math.dist(p0, p3), 1e-9)
    q1 = (q0[0] + (p1[0] - p0[0]) * k, q0[1] + (p1[1] - p0[1]) * k)
    q2 = (q3[0] + (p2[0] - p3[0]) * k, q3[1] + (p2[1] - p3[1]) * k)
    return q0, q1, q2, q3


def subdivide(segs, n):
    out = []
    for seg in segs:
        rest = seg
        for i in range(n, 1, -1):
            a, rest = split(rest, 1 / i)
            out.append(a)
        out.append(rest)
    return out


def s_outline(sw, n=4):
    fine = subdivide(SEGS, n)
    left = [offset_seg(s, sw / 2) for s in fine]
    right = [offset_seg(s, -sw / 2) for s in fine]
    # error maximo frente al desplazamiento exacto
    err = 0
    for s, l in zip(fine, left):
        for k in range(1, 10):
            t = k / 10
            p, tg = bez(s, t), tangent(s, t)
            exact = (p[0] - tg[1] * sw / 2, p[1] + tg[0] * sw / 2)
            err = max(err, min(math.dist(exact, bez(l, j / 40)) for j in range(41)))
    rev = [(c[3], c[2], c[1], c[0]) for c in reversed(right)]
    return left, rev, err


def affine(m, p):
    a, b, c, d, e, g = m
    return (a * p[0] + c * p[1] + e, b * p[0] + d * p[1] + g)


def mat_mul(m1, m2):
    a1, b1, c1, d1, e1, f1 = m1
    a2, b2, c2, d2, e2, f2 = m2
    return (a1 * a2 + c1 * b2, b1 * a2 + d1 * b2, a1 * c2 + c1 * d2, b1 * c2 + d1 * d2,
            a1 * e2 + c1 * f2 + e1, b1 * e2 + d1 * f2 + f1)


def s_transform(scale, cx, cy, skew):
    t = math.tan(math.radians(skew))
    m = (1, 0, 0, 1, cx, cy)
    m = mat_mul(m, (1, 0, t, 1, 0, 0))
    m = mat_mul(m, (scale, 0, 0, scale, 0, 0))
    m = mat_mul(m, (1, 0, 0, 1, -128, -129))
    return m


def s_path(m, sw):
    left, rev, err = s_outline(sw)
    pts = lambda c: [affine(m, p) for p in c]
    L, R = [pts(c) for c in left], [pts(c) for c in rev]
    d = f"M{f(L[0][0][0])} {f(L[0][0][1])}"
    for c in L:
        d += f"C{f(c[1][0])} {f(c[1][1])} {f(c[2][0])} {f(c[2][1])} {f(c[3][0])} {f(c[3][1])}"
    d += f"L{f(R[0][0][0])} {f(R[0][0][1])}"  # remate recto (butt)
    for c in R:
        d += f"C{f(c[1][0])} {f(c[1][1])} {f(c[2][0])} {f(c[2][1])} {f(c[3][0])} {f(c[3][1])}"
    d += "Z"
    samples = [bez(c, t / 8) for c in L + R for t in range(9)]
    return d, err, samples


# ---------- triangulo con esquinas redondeadas (offset de radio r) ----------
def rounded_triangle(pts, r):
    n = len(pts)
    area = sum(pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1] for i in range(n))
    sgn = 1 if area > 0 else -1
    normals = []
    for i in range(n):
        a, b = pts[i], pts[(i + 1) % n]
        dx, dy = b[0] - a[0], b[1] - a[1]
        L = math.hypot(dx, dy)
        normals.append((sgn * dy / L, -sgn * dx / L))  # hacia fuera
    d = ""
    for i in range(n):
        a, b = pts[i], pts[(i + 1) % n]
        nx, ny = normals[i]
        s, e = (a[0] + nx * r, a[1] + ny * r), (b[0] + nx * r, b[1] + ny * r)
        d += (f"M{f(s[0])} {f(s[1])}" if i == 0 else f"L{f(s[0])} {f(s[1])}") + f"L{f(e[0])} {f(e[1])}"
        nn = normals[(i + 1) % n]
        e2 = (b[0] + nn[0] * r, b[1] + nn[1] * r)
        d += f"A{r} {r} 0 0 {1 if sgn > 0 else 0} {f(e2[0])} {f(e2[1])}"
    return d + "Z"


def point_in_tri(p, t):
    def side(a, b):
        return (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    s = [side(t[i], t[(i + 1) % 3]) for i in range(3)]
    return all(v > 0 for v in s) or all(v < 0 for v in s)


def dist_to_tri(p, t):
    best = 1e9
    for i in range(3):
        a, b = t[i], t[(i + 1) % 3]
        ab = (b[0] - a[0], b[1] - a[1])
        k = max(0, min(1, ((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / (ab[0] ** 2 + ab[1] ** 2)))
        best = min(best, math.dist(p, (a[0] + ab[0] * k, a[1] + ab[1] * k)))
    return best


def symbol_d(s_scale, s_sw, s_cx, label):
    tri = [(50, 32), (50, 224), (216, 128)]
    td = rounded_triangle(tri, 10)
    m = s_transform(s_scale, s_cx, 128, -12)
    sd, err, samples = s_path(m, s_sw)
    inside = all(point_in_tri(p, tri) for p in samples)
    margin = min(dist_to_tri(p, tri) for p in samples)
    print(f"[{label}] error de la curva paralela: {err:.2f} u · S dentro del play: {inside} · margen minimo: {margin:.1f} u")
    return td + sd


def svg_doc(d_or_body, w, h, title, raw=False, fill=LAV):
    body = d_or_body if raw else f'<path fill="{fill}" fill-rule="evenodd" d="{d_or_body}"/>'
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {f(w)} {f(h)}" width="{f(w)}" height="{f(h)}" '
            f'role="img" aria-labelledby="title"><title id="title">{title}</title>\n{body}\n</svg>\n')


def write(name, text):
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as fh:
        fh.write(text)


# ---------- wordmark: "Swiftbeat" en Inter Bold, trazado a curvas ----------
def wordmark(text="Swiftbeat", wght=700, opsz=32, tracking=-0.02):
    from fontTools.ttLib import TTFont
    from fontTools.varLib.instancer import instantiateVariableFont
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.transformPen import TransformPen
    from fontTools.pens.boundsPen import BoundsPen
    font = instantiateVariableFont(TTFont(FONT), {"wght": wght, "opsz": opsz})
    gs, cmap, hmtx = font.getGlyphSet(), font.getBestCmap(), font["hmtx"]
    upm = font["head"].unitsPerEm
    kern = pair_kerning(font)
    pen = SVGPathPen(gs)
    bpen = BoundsPen(gs)
    x = 0
    names = [cmap[ord(ch)] for ch in text]
    for i, g in enumerate(names):
        tp = TransformPen(pen, (1, 0, 0, -1, x, 0))  # y hacia abajo
        gs[g].draw(tp)
        gs[g].draw(TransformPen(bpen, (1, 0, 0, -1, x, 0)))
        x += hmtx[g][0] + tracking * upm
        if i + 1 < len(names):
            x += kern.get((g, names[i + 1]), 0)
    cap = font["OS/2"].sCapHeight
    return pen.getCommands(), bpen.bounds, cap, upm


def pair_kerning(font):
    """Lee los pares de kerning de GPOS (PairPos formatos 1 y 2) para las letras usadas."""
    kern = {}
    if "GPOS" not in font:
        return kern
    gpos = font["GPOS"].table
    for lookup in gpos.LookupList.Lookup:
        subs = lookup.SubTable
        for st in subs:
            if lookup.LookupType == 9:
                st = st.ExtSubTable
            if getattr(st, "LookupType", lookup.LookupType) != 2 and lookup.LookupType not in (2, 9):
                continue
            if not hasattr(st, "Format"):
                continue
            cov = st.Coverage.glyphs
            if st.Format == 1:
                for i, first in enumerate(cov):
                    for pvr in st.PairSet[i].PairValueRecord:
                        v = getattr(pvr.Value1, "XAdvance", 0) if pvr.Value1 else 0
                        if v:
                            kern.setdefault((first, pvr.SecondGlyph), v)
            elif st.Format == 2:
                c1, c2 = st.ClassDef1.classDefs, st.ClassDef2.classDefs
                glyphs2 = font.getGlyphOrder()
                for first in cov:
                    k1 = c1.get(first, 0)
                    row = st.Class1Record[k1].Class2Record
                    for second in glyphs2:
                        if second not in c2 and second.isalpha() is False:
                            pass
                        k2 = c2.get(second, 0)
                        v = getattr(row[k2].Value1, "XAdvance", 0) if row[k2].Value1 else 0
                        if v:
                            kern.setdefault((first, second), v)
    return kern


if __name__ == "__main__":
    # simbolo maestro (S de 42 u a escala 0,54) y corte para tamanos pequenos (S mas grande y gruesa)
    master = symbol_d(0.54, 42, 100, "maestro")
    small = symbol_d(0.57, 52, 104, "pequeno")
    # centra la caja del triangulo (40..226 x 22..234) en el lienzo de 256: desplaza x -5
    sym = lambda d: f'<g transform="translate(-5 0)"><path fill="{{c}}" fill-rule="evenodd" d="{d}"/></g>'
    for key, d in (("symbol", master), ("symbol-small", small)):
        for cname, col in (("color", LAV), ("black", "#000000"), ("white", "#ffffff")):
            write(f"swiftbeat-{key}-{cname}.svg", svg_doc(sym(d).replace("{c}", col), 256, 256,
                                                         "Swiftbeat", raw=True))

    # wordmark
    cmds, (x0, y0, x1, y1), cap, upm = wordmark()
    WM_CAP = 96  # altura de mayuscula en el lockup horizontal (simbolo de 256 de alto)
    s = WM_CAP / cap
    wm_w = (x1 - x0) * s
    print(f"wordmark: ancho {wm_w:.0f} u con mayuscula de {WM_CAP} u")

    def wm_group(tx, ty, scale, col):
        return (f'<g transform="translate({f(tx)} {f(ty)}) scale({f(scale)}) translate({f(-x0)} 0)">'
                f'<path fill="{col}" d="{cmds}"/></g>')

    # solo wordmark (alto = caja de la palabra)
    wm_h = (y1 - y0) * s
    for cname, col in (("color", INK), ("black", "#000000"), ("white", "#ffffff")):
        write(f"swiftbeat-wordmark-{cname}.svg",
              svg_doc(wm_group(0, -y0 * s, s, col), wm_w, wm_h, "Swiftbeat", raw=True))

    # lockup horizontal: simbolo 256 alto (caja visible 22..234), palabra centrada en la caja visible
    gap = 56
    sym_vis_left, sym_vis_right = 35, 221  # caja del triangulo tras el desplazamiento
    tx = sym_vis_right + gap
    base_y = 128 + WM_CAP / 2  # mayusculas centradas verticalmente con el play
    W = tx + wm_w + 35
    for cname, sc, wc in (("color", LAV, INK), ("color-dark", LAV, "#ffffff"),
                          ("black", "#000000", "#000000"), ("white", "#ffffff", "#ffffff")):
        body = sym(master).replace("{c}", sc) + wm_group(tx, base_y, s, wc)
        write(f"swiftbeat-horizontal-{cname}.svg", svg_doc(body, W, 256, "Swiftbeat", raw=True))

    # lockup apilado: simbolo arriba, palabra debajo y centrada
    s2 = 72 / cap
    w2 = (x1 - x0) * s2
    SW = max(w2 + 70, 256)
    sx = (SW - 256) / 2
    top_word = 256 + 40
    SH = top_word + 72 + 40
    for cname, sc, wc in (("color", LAV, INK), ("color-dark", LAV, "#ffffff"),
                          ("black", "#000000", "#000000"), ("white", "#ffffff", "#ffffff")):
        body = (f'<g transform="translate({f(sx)} 0)">{sym(master).replace("{c}", sc)}</g>'
                + wm_group((SW - w2) / 2, top_word + 72, s2, wc))
        write(f"swiftbeat-stacked-{cname}.svg", svg_doc(body, SW, SH, "Swiftbeat", raw=True))
    print("ok")
