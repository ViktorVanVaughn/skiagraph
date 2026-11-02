"""make_meshes.py OUTDIR: the two example meshes (CC0, made by this script; no outside data).

knot.obj + knot.mtl : a trefoil knot tube (red plastic) on a plinth (matte grey): the OBJ reader, smooth normals,
                      self-overlap, cast shadows.
still_life.glb      : a glass goblet, a metal ring, an apple and a wooden table top (glTF PBR: glass, metal, plastic,
                      matte), placed by node transforms.
"""
import json
import math
import os
import struct
import sys


def tube(path, n, m, r):
    """a closed tube of radius r along path(t), t in [0, 1): vertices, normals, quads (as index lists)"""
    vs, ns = [], []
    pts = [path(i / n) for i in range(n)]
    for i in range(n):
        p, q = pts[i], pts[(i + 1) % n]
        t = norm(sub(q, pts[i - 1]))
        a = norm(cross(t, (0.3, 0.5, 0.8)))
        b = cross(t, a)
        for j in range(m):
            th = 2 * math.pi * j / m
            d = tuple(math.cos(th) * a[k] + math.sin(th) * b[k] for k in range(3))
            vs.append(tuple(p[k] + r * d[k] for k in range(3)))
            ns.append(d)
    faces = [(i * m + j, ((i + 1) % n) * m + j, ((i + 1) % n) * m + (j + 1) % m, i * m + (j + 1) % m) for i in range(n) for j in range(m)]
    return vs, ns, faces


def lathe(prof, m):
    """a surface of revolution about y from a profile [(radius, y)]: vertices, normals, quads"""
    vs, ns = [], []
    for i, (r, y) in enumerate(prof):
        a, b = prof[max(i - 1, 0)], prof[min(i + 1, len(prof) - 1)]
        dr, dy = b[0] - a[0], b[1] - a[1]
        nr, ny = dy, -dr
        ln = math.hypot(nr, ny) or 1.0
        for j in range(m):
            th = 2 * math.pi * j / m
            vs.append((r * math.cos(th), y, r * math.sin(th)))
            ns.append((nr / ln * math.cos(th), ny / ln, nr / ln * math.sin(th)))
    faces = [(i * m + j, i * m + (j + 1) % m, (i + 1) * m + (j + 1) % m, (i + 1) * m + j) for i in range(len(prof) - 1) for j in range(m)]
    return vs, ns, faces


def box(sx, sy, sz):
    vs, ns, faces = [], [], []
    for ax in range(3):
        for s in (-1, 1):
            nrm = [0, 0, 0]
            nrm[ax] = s
            u, v = [(ax + 1) % 3, (ax + 2) % 3]
            base = len(vs)
            for a, b in ((-1, -1), (1, -1), (1, 1), (-1, 1)):
                p = [0, 0, 0]
                p[ax], p[u], p[v] = s, a, b
                vs.append((p[0] * sx, p[1] * sy, p[2] * sz))
                ns.append(tuple(nrm))
            f = (base, base + 1, base + 2, base + 3)
            faces.append(f if s > 0 else f[::-1])
    return vs, ns, faces


def sub(a, b):
    return tuple(x - y for x, y in zip(a, b))


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def norm(a):
    ln = math.sqrt(sum(x * x for x in a)) or 1.0
    return tuple(x / ln for x in a)


def trefoil(t):
    s = 2 * math.pi * t
    return (math.sin(s) + 2 * math.sin(2 * s), math.cos(s) - 2 * math.cos(2 * s) + 0.6, -math.sin(3 * s) * 0.9)


def write_obj(out):
    lines = ["# trefoil knot on a plinth: CC0 1.0, made by scripts/make_meshes.py", "mtllib knot.mtl"]
    off = 0
    for name, mat, (vs, ns, faces) in [("knot", "red", tube(trefoil, 240, 20, 0.42)), ("plinth", "stone", box(3.6, 0.3, 2.0))]:
        if name == "plinth":
            vs = [(x, y - 3.4, z) for x, y, z in vs]
        lines += [f"o {name}", f"usemtl {mat}"]
        lines += ["v %.5f %.5f %.5f" % v for v in vs] + ["vn %.5f %.5f %.5f" % n for n in ns]
        lines += ["f " + " ".join(f"{off + i + 1}//{off + i + 1}" for i in f) for f in faces]
        off += len(vs)
    open(os.path.join(out, "knot.obj"), "w").write("\n".join(lines) + "\n")
    open(os.path.join(out, "knot.mtl"), "w").write(
        "# CC0 1.0, made by scripts/make_meshes.py\nnewmtl red\nKd 0.86 0.16 0.12\nKs 0.5 0.5 0.5\nNs 200\nillum 2\n"
        "newmtl stone\nKd 0.72 0.70 0.66\nKs 0 0 0\nillum 1\n")


def write_glb(out):
    goblet = [(0.0, 0.0), (0.9, 0.0), (0.9, 0.08), (0.25, 0.2), (0.12, 0.5), (0.1, 1.3), (0.3, 1.55), (0.75, 1.9), (0.95, 2.4),
              (1.0, 3.0), (0.94, 3.0), (0.89, 2.45), (0.7, 1.98), (0.25, 1.65), (0.0, 1.62)]
    apple = [(0.02 + 0.98 * math.sin(math.pi * i / 24) * (1 - 0.12 * math.cos(math.pi * i / 24)), -math.cos(math.pi * i / 24)) for i in range(25)]
    apple = [(r, y - 0.25 * math.exp(-((r / 0.25) ** 2)) * (1 if y > 0 else -1)) for r, y in apple]  # dimples
    ring = tube(lambda t: (1.1 * math.cos(2 * math.pi * t), 0.0, 1.1 * math.sin(2 * math.pi * t)), 96, 24, 0.24)
    parts = [  # name, mesh, material, translation, rotation quaternion, scale
        ("goblet", lathe(goblet, 72), 0, (-1.1, 0, -0.3), (0, 0, 0, 1), 1.0),
        ("ring", ring, 1, (1.1, 0.24, 0.9), (0, 0, 0, 1), 1.0),
        ("apple", lathe(apple, 48), 2, (1.0, 0.95, -0.7), (0, 0, 0.0998, 0.995), 0.95),
        ("table", box(2.7, 0.12, 1.9), 3, (0, -0.12, 0), (0, 0, 0, 1), 1.0),
    ]
    mats = [
        {"name": "glass", "alphaMode": "BLEND", "pbrMetallicRoughness": {"baseColorFactor": [0.55, 0.78, 0.86, 0.35], "metallicFactor": 0, "roughnessFactor": 0.05}},
        {"name": "brass", "pbrMetallicRoughness": {"baseColorFactor": [0.85, 0.66, 0.28, 1], "metallicFactor": 1, "roughnessFactor": 0.3}},
        {"name": "apple", "pbrMetallicRoughness": {"baseColorFactor": [0.42, 0.72, 0.18, 1], "metallicFactor": 0, "roughnessFactor": 0.45}},
        {"name": "wood", "pbrMetallicRoughness": {"baseColorFactor": [0.62, 0.42, 0.26, 1], "metallicFactor": 0, "roughnessFactor": 0.9}},
    ]
    bin_, views, accs, meshes, nodes = b"", [], [], [], []
    for k, (name, (vs, ns, faces), mat, t, q, s) in enumerate(parts):
        tris = [x for f in faces for x in (f[0], f[1], f[2], f[0], f[2], f[3])]
        for data, typ, ct, fmt in [(vs, "VEC3", 5126, "<3f"), (ns, "VEC3", 5126, "<3f"), (tris, "SCALAR", 5125, "<I")]:
            raw = b"".join(struct.pack(fmt, *(d if isinstance(d, tuple) else (d,))) for d in data)
            views.append({"buffer": 0, "byteOffset": len(bin_), "byteLength": len(raw)})
            acc = {"bufferView": len(views) - 1, "componentType": ct, "count": len(data), "type": typ}
            if len(accs) % 3 == 0:  # POSITION: glTF wants its bounds
                acc["min"] = [min(v[i] for v in vs) for i in range(3)]
                acc["max"] = [max(v[i] for v in vs) for i in range(3)]
            accs.append(acc)
            bin_ += raw
        meshes.append({"name": name, "primitives": [{"attributes": {"POSITION": 3 * k, "NORMAL": 3 * k + 1}, "indices": 3 * k + 2, "material": mat}]})
        nodes.append({"name": name, "mesh": k, "translation": list(t), "rotation": list(q), "scale": [s, s, s]})
    doc = {"asset": {"version": "2.0", "generator": "Skiagraph scripts/make_meshes.py", "copyright": "CC0 1.0"},
           "scene": 0, "scenes": [{"nodes": list(range(len(nodes)))}], "nodes": nodes, "meshes": meshes, "materials": mats,
           "accessors": accs, "bufferViews": views, "buffers": [{"byteLength": len(bin_)}]}
    js = json.dumps(doc, separators=(",", ":")).encode()
    js += b" " * (-len(js) % 4)
    bin_ += b"\0" * (-len(bin_) % 4)
    glb = struct.pack("<4sII", b"glTF", 2, 12 + 8 + len(js) + 8 + len(bin_)) + struct.pack("<I4s", len(js), b"JSON") + js + struct.pack("<I4s", len(bin_), b"BIN\0") + bin_
    open(os.path.join(out, "still_life.glb"), "wb").write(glb)


if __name__ == "__main__":
    os.makedirs(sys.argv[1], exist_ok=True)
    write_obj(sys.argv[1])
    write_glb(sys.argv[1])
