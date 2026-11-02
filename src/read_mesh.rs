//! Stage 5: OBJ (+ .mtl) and glTF 2.0 (.gltf, .glb) into the internal scene (`scene.rs`), with no new dependency.
//!
//! Triangles with normals (smooth triangles where the file has normals, flat ones where it has none); each
//! material mapped onto Comic Ink's five (`ink::Mat::of`): glass below opacity 0.99, metal for glTF metallic >= 0.5
//! (OBJ: `Pm`, or a mirror `illum` 3/5 with strong `Ks`), matte for roughness >= 0.75 (OBJ: no specular), plastic
//! otherwise. Each object (OBJ `o`/`g`, glTF node or mesh name) becomes a group, so the recipe's `mat=` can pick it.
//! The file's camera is used if it has one (glTF), otherwise an orthographic camera frames the whole model from the
//! front, a little from the right and above; the key light goes to the side its seen faces lean towards (`ink::Light`). What is not read (textures,
//! skins, morph targets, animation, points and lines, emission) is reported once on stderr and left out; a file
//! nothing can be read from is an error, never a crash.

use crate::scene::*;
use std::collections::BTreeSet;
use std::path::Path;

type Res<T> = Result<T, String>;

/// Reads `path` by its extension; warnings go to stderr.
pub fn read(path: &Path) -> Res<Scene> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut b = Build::default();
    match ext.as_str() {
        "obj" => obj(&mut b, path)?,
        "gltf" | "glb" => gltf(&mut b, path)?,
        _ => return Err(format!("{}: unknown scene type (read: .dat from VMD, .obj, .gltf, .glb)", path.display())),
    }
    for w in &b.warn {
        eprintln!("skiagraph: warning: {w}");
    }
    b.finish()
}

#[derive(Default)]
struct Build {
    sc: Option<Scene>,
    warn: BTreeSet<String>,
    camera: Option<Camera>,
    res: Option<(u32, u32)>,
}

impl Build {
    fn scene(&mut self) -> &mut Scene {
        self.sc.get_or_insert_with(Scene::empty)
    }
    fn group(&mut self, name: &str) -> usize {
        let sc = self.scene();
        sc.groups.iter().position(|g| g == name).unwrap_or_else(|| {
            sc.groups.push(name.to_string());
            sc.groups.len() - 1
        })
    }
    /// one triangle; with normals a smooth one (normals that are zero or missing fall back to the flat one).
    /// OBJ and glTF are right-handed, Tachyon (as VMD writes it) left-handed: z is mirrored, and the winding with it.
    fn tri(&mut self, v: [V3; 3], n: Option<[V3; 3]>, tex: usize, group: usize) {
        let (v, n) = ([v[0], v[2], v[1]].map(mirror), n.map(|n| [n[0], n[2], n[1]].map(mirror)));
        let (a, b) = (sub(v[1], v[0]), sub(v[2], v[0]));
        let e = cross(a, b);
        // degenerate (its sides in one line), whatever the model's size
        if dot(e, e) <= 1e-24 * dot(a, a) * dot(b, b) || v.iter().flatten().any(|x| !x.is_finite()) {
            return;
        }
        let shape = match n.filter(|n| n.iter().all(|m| dot(*m, *m) > 1e-20 && m.iter().all(|x| x.is_finite()))) {
            Some(n) => Shape::STri { v, n: n.map(unit), colors: None },
            None => Shape::Tri { v },
        };
        self.scene().objects.push(Object { shape, tex, clip: None, group: Some(group) });
    }
    fn material(&mut self, m: Material) -> usize {
        let sc = self.scene();
        sc.materials.iter().position(|x| *x == m).unwrap_or_else(|| {
            sc.materials.push(m);
            sc.materials.len() - 1
        })
    }
    fn finish(mut self) -> Res<Scene> {
        let mut sc = self.sc.take().unwrap_or_else(Scene::empty);
        if sc.objects.is_empty() {
            return Err("no triangles found in the file".into());
        }
        sc.shader = ShaderMode::Medium;
        sc.background = Background::Solid(Rgb { r: 1.0, g: 1.0, b: 1.0 });
        // the model moved and scaled to VMD's own scale (its middle at the origin, 2 units across): the tracer keeps
        // Tachyon's fixed tolerances (1e-9, faithful to VMD's renders), which suit only that scale. A model of any
        // size, 0.06 or 6000 units across, is then drawn the same.
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for o in &sc.objects {
            if let Shape::Tri { v } | Shape::STri { v, .. } = &o.shape {
                for p in v {
                    (lo, hi) = (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])));
                }
            }
        }
        let mid: V3 = std::array::from_fn(|i| 0.5 * (lo[i] + hi[i]));
        let k = 2.0 / (0..3).map(|i| hi[i] - lo[i]).fold(0.0, f64::max).max(1e-300);
        let fit = |p: V3| -> V3 { std::array::from_fn(|i| (p[i] - mid[i]) * k) };
        for o in &mut sc.objects {
            if let Shape::Tri { v } | Shape::STri { v, .. } = &mut o.shape {
                *v = v.map(fit);
            }
        }
        match (self.camera.take(), self.res) {
            (Some(mut c), Some((w, h))) => {
                c.center = fit(c.center);
                if c.projection == Projection::Orthographic {
                    c.zoom /= k;
                }
                sc.camera = c;
                (sc.hres, sc.vres) = (w, h);
            }
            _ => frame(&mut sc),
        }
        Ok(sc)
    }
}

/// The default camera: orthographic, looking at the model's middle from the front (+z), a little from the right and
/// above, the model filling about 88 % of the picture, the picture's shape following the model's (1600 px wide).
/// Distances are the model's own (any size frames the same).
fn frame(sc: &mut Scene) {
    let pts: Vec<V3> = sc.objects.iter().flat_map(|o| match &o.shape { Shape::Tri { v } | Shape::STri { v, .. } => v.to_vec(), _ => vec![] }).collect();
    // the file's front (+z) is -z here (`mirror`)
    let eye = unit([0.45, 0.3, -1.0]);
    let r = unit(cross([0.0, 1.0, 0.0], eye));
    let u = cross(eye, r);
    let ext = |d: V3| pts.iter().map(|p| dot(*p, d)).fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
    let ((r0, r1), (u0, u1), (e0, e1)) = (ext(r), ext(u), ext(eye));
    let (w, h) = ((r1 - r0).max(1e-9), (u1 - u0).max(1e-9));
    sc.hres = 1600;
    sc.vres = ((1600.0 * h / w).round() as u32).clamp(900, 2000);
    let height = (h.max(w * f64::from(sc.vres) / 1600.0)) / 0.88;
    let mid = [0.5 * (r0 + r1), 0.5 * (u0 + u1), e1 + (e1 - e0).max(1e-9 * w)];
    let center = [0, 1, 2].map(|i| mid[0] * r[i] + mid[1] * u[i] + mid[2] * eye[i]);
    sc.camera = Camera { projection: Projection::Orthographic, zoom: 1.0 / height, viewdir: eye.map(|v| -v), updir: u, center, ..Scene::empty().camera };
}

/// glTF's metallic-roughness material (and OBJ's, mapped onto it) as a Tachyon material `ink::Mat::of` sorts.
fn pbr(color: [f64; 3], alpha: f64, metallic: f64, rough: f64) -> Material {
    let metal = metallic >= 0.5;
    Material {
        ambient: 0.1,
        diffuse: if metal { 0.35 } else { 0.65 },
        specular: 0.0,
        opacity: alpha.clamp(0.0, 1.0) as f32,
        raster3d: false,
        outline: 0.0,
        outline_width: 0.0,
        phong: if metal { 0.5 } else if rough >= 0.75 { 0.0 } else { (0.1 + 0.4 * (1.0 - rough)) as f32 },
        phong_exp: (10.0 + 90.0 * (1.0 - rough)) as f32,
        phong_type: if metal { PhongType::Metal } else { PhongType::Plastic },
        color: Rgb { r: color[0] as f32, g: color[1] as f32, b: color[2] as f32 },
        texfunc: 0,
    }
}

fn plain() -> Material {
    pbr([0.8; 3], 1.0, 0.0, 0.5)
}

// ---------------------------------------------------------------------------------------------------------- OBJ

fn obj(b: &mut Build, path: &Path) -> Res<()> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (mut v, mut vn): (Vec<V3>, Vec<V3>) = (vec![], vec![]);
    let mut mtls: Vec<(String, Material)> = vec![];
    let mut tex = b.material(plain());
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh").to_string();
    let mut group = b.group(&stem);
    let num = |w: Option<&str>| w.and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    for (ln, line) in text.lines().enumerate() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("v") => v.push([num(w.next()), num(w.next()), num(w.next())]),
            Some("vn") => vn.push([num(w.next()), num(w.next()), num(w.next())]),
            Some("o" | "g") => {
                let name = w.collect::<Vec<_>>().join(" ");
                group = b.group(if name.is_empty() { &stem } else { &name });
            }
            Some("mtllib") => {
                for f in w {
                    match std::fs::read_to_string(path.with_file_name(f)) {
                        Ok(t) => mtls.extend(mtl(&t, &mut b.warn)),
                        Err(e) => _ = b.warn.insert(format!("material library {f}: {e}")),
                    }
                }
            }
            Some("usemtl") => {
                let name = w.next().unwrap_or("");
                tex = match mtls.iter().find(|(n, _)| n == name) {
                    Some((_, m)) => b.material(m.clone()),
                    None => {
                        b.warn.insert(format!("material {name:?} not in any .mtl: drawn light grey"));
                        b.material(plain())
                    }
                };
            }
            Some("f") => {
                // a corner: v, v/vt, v//vn or v/vt/vn; negative indices count from the end
                let idx = |s: &str, n: usize| -> Option<usize> {
                    let i: i64 = s.parse().ok()?;
                    let k = if i < 0 { n as i64 + i } else { i - 1 };
                    (0..n as i64).contains(&k).then_some(k as usize)
                };
                let mut cs = vec![];
                for c in w {
                    let mut p = c.split('/');
                    let Some(pv) = p.next().and_then(|s| idx(s, v.len())) else {
                        return Err(format!("{}:{}: face corner {c:?} is not a vertex", path.display(), ln + 1));
                    };
                    let pn = p.nth(1).and_then(|s| idx(s, vn.len()));
                    cs.push((v[pv], pn.map(|i| vn[i])));
                }
                // a polygon as a fan from its first corner
                for i in 1..cs.len().saturating_sub(1) {
                    let t = [cs[0], cs[i], cs[i + 1]];
                    let n = (t.iter().all(|c| c.1.is_some())).then(|| t.map(|c| c.1.unwrap_or_default()));
                    b.tri(t.map(|c| c.0), n, tex, group);
                }
            }
            Some("l" | "p") => _ = b.warn.insert("OBJ lines and points: left out".into()),
            Some("curv" | "surf" | "cstype") => _ = b.warn.insert("OBJ curves and surfaces: left out".into()),
            _ => {}
        }
    }
    Ok(())
}

fn mtl(text: &str, warn: &mut BTreeSet<String>) -> Vec<(String, Material)> {
    struct M {
        name: String,
        kd: [f64; 3],
        ks: f64,
        ns: f64,
        d: f64,
        illum: i64,
        pm: Option<f64>,
        pr: Option<f64>,
    }
    let done = |m: M| {
        let metallic = m.pm.unwrap_or(if matches!(m.illum, 3 | 5) && m.ks >= 0.5 { 1.0 } else { 0.0 });
        let rough = m.pr.unwrap_or(if m.ks < 0.05 { 1.0 } else { 1.0 - (m.ns / 1000.0).clamp(0.0, 1.0).sqrt() });
        (m.name, pbr(m.kd, m.d, metallic, rough))
    };
    let mut out = vec![];
    let mut cur: Option<M> = None;
    for line in text.lines() {
        let mut w = line.split_whitespace();
        let key = w.next().unwrap_or("");
        let nums: Vec<f64> = w.clone().filter_map(|x| x.parse().ok()).collect();
        let first = nums.first().copied().unwrap_or(0.0);
        if key == "newmtl" {
            out.extend(cur.take().map(done));
            cur = Some(M { name: w.next().unwrap_or("").into(), kd: [0.8; 3], ks: 0.0, ns: 0.0, d: 1.0, illum: 2, pm: None, pr: None });
            continue;
        }
        let Some(m) = cur.as_mut() else { continue };
        match key {
            "Kd" if nums.len() >= 3 => m.kd = [nums[0], nums[1], nums[2]],
            "Ks" => m.ks = nums.iter().take(3).fold(0.0, |a: f64, &x| a.max(x)),
            "Ns" => m.ns = first,
            "d" => m.d = first,
            "Tr" => m.d = 1.0 - first,
            "illum" => m.illum = first as i64,
            "Pm" => m.pm = Some(first),
            "Pr" => m.pr = Some(first),
            k if k.starts_with("map_") || k == "bump" || k == "disp" => _ = warn.insert("textures (the material's plain colour is used)".into()),
            _ => {}
        }
    }
    out.extend(cur.take().map(done));
    out
}

// --------------------------------------------------------------------------------------------------------- glTF

/// A JSON value (the little glTF needs: no escapes beyond the common ones).
#[derive(Debug, Clone)]
enum J {
    Null,
    B(bool),
    N(f64),
    S(String),
    A(Vec<J>),
    O(Vec<(String, J)>),
}

static NULL: J = J::Null;

impl J {
    fn get(&self, k: &str) -> &J {
        match self {
            J::O(m) => m.iter().find(|(a, _)| a == k).map_or(&NULL, |(_, v)| v),
            _ => &NULL,
        }
    }
    fn at(&self, i: usize) -> &J {
        match self {
            J::A(a) => a.get(i).unwrap_or(&NULL),
            _ => &NULL,
        }
    }
    fn arr(&self) -> &[J] {
        match self {
            J::A(a) => a,
            _ => &[],
        }
    }
    fn num(&self) -> Option<f64> {
        if let J::N(x) = self { Some(*x) } else { None }
    }
    fn idx(&self) -> Option<usize> {
        self.num().filter(|x| *x >= 0.0).map(|x| x as usize)
    }
    fn str(&self) -> Option<&str> {
        if let J::S(s) = self { Some(s) } else { None }
    }
    fn nums(&self) -> Vec<f64> {
        self.arr().iter().filter_map(J::num).collect()
    }
}

fn json(s: &str) -> Res<J> {
    let b = s.as_bytes();
    let mut i = 0;
    let v = value(b, &mut i)?;
    Ok(v)
}

fn value(b: &[u8], i: &mut usize) -> Res<J> {
    let ws = |i: &mut usize| {
        while *i < b.len() && b[*i].is_ascii_whitespace() {
            *i += 1;
        }
    };
    ws(i);
    let err = |i: usize| format!("glTF JSON: unexpected input at byte {i}");
    let c = *b.get(*i).ok_or_else(|| err(*i))?;
    let out = match c {
        b'{' | b'[' => {
            *i += 1;
            let (mut m, mut a) = (vec![], vec![]);
            loop {
                ws(i);
                if b.get(*i) == Some(&(c + 2)) {
                    *i += 1;
                    break;
                }
                if c == b'{' {
                    let J::S(k) = value(b, i)? else { return Err(err(*i)) };
                    ws(i);
                    if b.get(*i) != Some(&b':') {
                        return Err(err(*i));
                    }
                    *i += 1;
                    m.push((k, value(b, i)?));
                } else {
                    a.push(value(b, i)?);
                }
                ws(i);
                match b.get(*i) {
                    Some(b',') => *i += 1,
                    Some(&x) if x == c + 2 => {}
                    _ => return Err(err(*i)),
                }
            }
            if c == b'{' { J::O(m) } else { J::A(a) }
        }
        b'"' => {
            let mut s = Vec::new();
            *i += 1;
            while *i < b.len() && b[*i] != b'"' {
                if b[*i] == b'\\' {
                    *i += 1;
                    match b.get(*i) {
                        Some(b'n') => s.push(b'\n'),
                        Some(b't') => s.push(b'\t'),
                        Some(b'u') => {
                            let h = std::str::from_utf8(b.get(*i + 1..*i + 5).ok_or_else(|| err(*i))?).map_err(|_| err(*i))?;
                            let ch = char::from_u32(u32::from_str_radix(h, 16).map_err(|_| err(*i))?).unwrap_or('?');
                            s.extend(ch.to_string().as_bytes());
                            *i += 4;
                        }
                        Some(&x) => s.push(x),
                        None => return Err(err(*i)),
                    }
                } else {
                    s.push(b[*i]);
                }
                *i += 1;
            }
            *i += 1;
            J::S(String::from_utf8_lossy(&s).into_owned())
        }
        b't' | b'f' | b'n' => {
            let w = [("true", J::B(true)), ("false", J::B(false)), ("null", J::Null)].into_iter().find(|(w, _)| b[*i..].starts_with(w.as_bytes())).ok_or_else(|| err(*i))?;
            *i += w.0.len();
            w.1
        }
        _ => {
            let st = *i;
            while *i < b.len() && matches!(b[*i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                *i += 1;
            }
            J::N(std::str::from_utf8(&b[st..*i]).ok().and_then(|x| x.parse().ok()).ok_or_else(|| err(st))?)
        }
    };
    Ok(out)
}

fn base64(s: &str) -> Res<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let (mut out, mut acc, mut n) = (vec![], 0u32, 0);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        acc = acc << 6 | u32::from(val(c).ok_or("glTF: bad base64 data")?);
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((acc >> n) as u8);
        }
    }
    Ok(out)
}

type M4 = [[f64; 4]; 4];

fn mmul(a: &M4, b: &M4) -> M4 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| a[i][k] * b[k][j]).sum()))
}

/// a node's local transform (row-major `m[row][col]`), from `matrix` (column-major in glTF) or translation/rotation/scale
fn local(n: &J) -> M4 {
    let m = n.get("matrix").nums();
    if m.len() == 16 {
        return std::array::from_fn(|i| std::array::from_fn(|j| m[j * 4 + i]));
    }
    let t = Some(n.get("translation").nums()).filter(|v| v.len() == 3).unwrap_or(vec![0.0; 3]);
    let q = Some(n.get("rotation").nums()).filter(|v| v.len() == 4).unwrap_or(vec![0.0, 0.0, 0.0, 1.0]);
    let s = Some(n.get("scale").nums()).filter(|v| v.len() == 3).unwrap_or(vec![1.0; 3]);
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    std::array::from_fn(|i| if i < 3 { [r[i][0] * s[0], r[i][1] * s[1], r[i][2] * s[2], t[i]] } else { [0.0, 0.0, 0.0, 1.0] })
}

fn gltf(b: &mut Build, path: &Path) -> Res<()> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let le = |o: usize| bytes.get(o..o + 4).map_or(0, |x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize);
    // a .glb: a header, the JSON chunk, then an optional binary chunk (the first buffer)
    let (doc, blob) = if bytes.starts_with(b"glTF") {
        let jl = le(12);
        let js = bytes.get(20..20 + jl).ok_or("glb: truncated JSON chunk")?;
        let bin = (le(20 + jl + 4) == 0x004E_4942).then(|| bytes.get(28 + jl..28 + jl + le(20 + jl)).map(<[u8]>::to_vec)).flatten();
        (json(&String::from_utf8_lossy(js))?, bin)
    } else {
        (json(&String::from_utf8_lossy(&bytes))?, None)
    };
    if !doc.get("asset").get("version").str().unwrap_or("").starts_with('2') {
        return Err("glTF: only version 2.0 is read".into());
    }
    let buffers: Vec<Vec<u8>> = doc
        .get("buffers")
        .arr()
        .iter()
        .enumerate()
        .map(|(i, bu)| match bu.get("uri").str() {
            None if i == 0 => blob.clone().ok_or_else(|| "glTF: buffer 0 has no data".to_string()),
            None => Err(format!("glTF: buffer {i} has no data")),
            Some(u) if u.starts_with("data:") => base64(u.split_once(',').map_or("", |x| x.1)),
            Some(u) => std::fs::read(path.with_file_name(u.replace("%20", " "))).map_err(|e| format!("glTF buffer {u}: {e}")),
        })
        .collect::<Res<_>>()?;
    // an accessor's elements as f64s (normalised integers scaled to 0..1 / -1..1)
    let read = |a: usize| -> Res<Vec<Vec<f64>>> {
        let ac = doc.get("accessors").at(a);
        if !matches!(ac.get("sparse"), J::Null) {
            return Err("glTF: sparse accessors".into());
        }
        let n = ac.get("count").idx().unwrap_or(0);
        let comps = match ac.get("type").str().unwrap_or("") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            t => return Err(format!("glTF: accessor type {t}")),
        };
        let ct = ac.get("componentType").idx().unwrap_or(0);
        let size = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(format!("glTF: component type {ct}")),
        };
        let norm = matches!(ac.get("normalized"), J::B(true));
        let Some(bv) = ac.get("bufferView").idx() else { return Ok(vec![vec![0.0; comps]; n]) };
        let view = doc.get("bufferViews").at(bv);
        let buf = buffers.get(view.get("buffer").idx().unwrap_or(usize::MAX)).ok_or("glTF: missing buffer")?;
        let start = view.get("byteOffset").idx().unwrap_or(0) + ac.get("byteOffset").idx().unwrap_or(0);
        let stride = view.get("byteStride").idx().unwrap_or(comps * size).max(1);
        let one = |o: usize| -> Option<f64> {
            let x = buf.get(o..o + size)?;
            Some(match ct {
                5126 => f64::from(f32::from_le_bytes([x[0], x[1], x[2], x[3]])),
                5125 => f64::from(u32::from_le_bytes([x[0], x[1], x[2], x[3]])),
                5123 => f64::from(u16::from_le_bytes([x[0], x[1]])) / if norm { 65535.0 } else { 1.0 },
                5122 => (f64::from(i16::from_le_bytes([x[0], x[1]])) / if norm { 32767.0 } else { 1.0 }).max(-1.0),
                5121 => f64::from(x[0]) / if norm { 255.0 } else { 1.0 },
                _ => (f64::from(x[0] as i8) / if norm { 127.0 } else { 1.0 }).max(-1.0),
            })
        };
        (0..n).map(|i| (0..comps).map(|c| one(start + i * stride + c * size)).collect::<Option<Vec<f64>>>().ok_or_else(|| "glTF: accessor runs past its buffer".to_string())).collect()
    };
    // materials
    let mats: Vec<Material> = doc
        .get("materials")
        .arr()
        .iter()
        .map(|m| {
            let p = m.get("pbrMetallicRoughness");
            // textures are not decoded: the factors alone are used; a colour only in a texture becomes the light
            // neutral grey of a model without materials (white, glTF's default factor, leaves the lit side blank;
            // 0.6 grey sank the fox's mid band into hatching), and metalness or roughness only in a texture becomes a
            // plain dielectric (glTF's default factors, 1 and 1, made the avocado dark rough metal)
            let tex = |k: &str| !matches!(p.get(k), J::Null);
            let mut c = Some(p.get("baseColorFactor").nums()).filter(|c| c.len() == 4);
            if tex("baseColorTexture") {
                b.warn.insert("glTF textures are not read (the material's colour factor is used, or a neutral mid-tone)".into());
                c = c.or(Some(vec![0.8, 0.8, 0.8, 1.0]));
            }
            let c = c.unwrap_or(vec![1.0; 4]);
            let mr = tex("metallicRoughnessTexture");
            let metallic = p.get("metallicFactor").num().unwrap_or(if mr { 0.0 } else { 1.0 });
            let rough = p.get("roughnessFactor").num().unwrap_or(if mr { 0.5 } else { 1.0 });
            if m.get("emissiveFactor").nums().iter().any(|x| *x > 0.0) {
                b.warn.insert("glTF emission: not drawn (the recipe's mat=NAME:emissive draws a glow)".into());
            }
            let alpha = if m.get("alphaMode").str() == Some("BLEND") { c[3] } else { 1.0 };
            pbr([c[0], c[1], c[2]], alpha, metallic, rough)
        })
        .collect();
    for (k, w) in [("skins", "skins: left out (the mesh is drawn unposed)"), ("animations", "animations: left out (the rest pose is drawn)")] {
        if !doc.get(k).arr().is_empty() {
            b.warn.insert(format!("glTF {w}"));
        }
    }
    // the scene's node tree, each node with its world transform
    let nodes = doc.get("nodes");
    let sid = doc.get("scene").idx().unwrap_or(0);
    let roots: Vec<usize> = match doc.get("scenes").at(sid).get("nodes") {
        J::A(a) => a.iter().filter_map(J::idx).collect(),
        _ => (0..nodes.arr().len()).collect(),
    };
    let id: M4 = std::array::from_fn(|i| std::array::from_fn(|j| f64::from(u8::from(i == j))));
    let mut stack: Vec<(usize, M4, usize)> = roots.into_iter().map(|r| (r, id, 0)).collect();
    while let Some((ni, parent, depth)) = stack.pop() {
        let node = nodes.at(ni);
        if depth > 64 {
            return Err("glTF: node tree deeper than 64 (a loop?)".into());
        }
        let m = mmul(&parent, &local(node));
        for c in node.get("children").arr().iter().filter_map(J::idx) {
            stack.push((c, m, depth + 1));
        }
        if let (Some(ci), None) = (node.get("camera").idx(), &b.camera) {
            let cam = doc.get("cameras").at(ci);
            let col = |j: usize| [m[0][j], m[1][j], m[2][j]];
            let (persp, ortho) = (cam.get("perspective"), cam.get("orthographic"));
            let (zoom, proj, aspect) = match (persp.get("yfov").num(), ortho.get("ymag").num()) {
                (Some(f), _) => (1.0 / (2.0 * (0.5 * f).tan()), Projection::Perspective, persp.get("aspectRatio").num()),
                (None, Some(y)) => (1.0 / (2.0 * y), Projection::Orthographic, ortho.get("xmag").num().map(|x| x / y)),
                _ => continue,
            };
            let aspect = aspect.unwrap_or(4.0 / 3.0).clamp(0.25, 4.0);
            b.camera = Some(Camera { projection: proj, zoom, viewdir: mirror(unit(col(2).map(|v| -v))), updir: mirror(unit(col(1))), center: mirror(col(3)), ..Scene::empty().camera });
            b.res = Some((1600, (1600.0 / aspect).round() as u32));
        }
        let Some(mi) = node.get("mesh").idx() else { continue };
        let mesh = doc.get("meshes").at(mi);
        let name = node.get("name").str().or(mesh.get("name").str()).map_or_else(|| format!("mesh {mi}"), str::to_string);
        let group = b.group(&name);
        if !mesh.get("weights").arr().is_empty() {
            b.warn.insert("glTF morph targets: left out (the base shape is drawn)".into());
        }
        let pt = |p: &[f64]| -> V3 { std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3]) };
        // normals: by the inverse transpose of the 3x3 part (its cofactors: the scale does not matter, they are made unit)
        let cof = |i: usize, j: usize| {
            let (a, b2, c, d) = (m[(i + 1) % 3][(j + 1) % 3], m[(i + 2) % 3][(j + 2) % 3], m[(i + 1) % 3][(j + 2) % 3], m[(i + 2) % 3][(j + 1) % 3]);
            a * b2 - c * d
        };
        let flip = (0..3).map(|j| m[0][j] * cof(0, j)).sum::<f64>() < 0.0;
        let nm = |p: &[f64]| -> V3 { std::array::from_fn(|i| (0..3).map(|j| cof(i, j) * p[j]).sum::<f64>()) };
        for prim in mesh.get("primitives").arr() {
            let mode = prim.get("mode").idx().unwrap_or(4);
            if mode < 4 {
                b.warn.insert("glTF points and lines: left out".into());
                continue;
            }
            let attr = prim.get("attributes");
            let Some(pa) = attr.get("POSITION").idx() else { continue };
            let pos: Vec<V3> = read(pa)?.iter().map(|p| pt(p)).collect();
            let nor: Option<Vec<V3>> = attr.get("NORMAL").idx().map(read).transpose()?.map(|ns| ns.iter().map(|n| nm(n)).collect());
            let ix: Vec<usize> = match prim.get("indices").idx() {
                Some(a) => read(a)?.iter().map(|v| v[0] as usize).collect(),
                None => (0..pos.len()).collect(),
            };
            let tris: Vec<[usize; 3]> = match mode {
                5 => (2..ix.len()).map(|i| if i % 2 == 0 { [ix[i - 2], ix[i - 1], ix[i]] } else { [ix[i - 1], ix[i - 2], ix[i]] }).collect(),
                6 => (2..ix.len()).map(|i| [ix[0], ix[i - 1], ix[i]]).collect(),
                _ => ix.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            };
            let tex = match prim.get("material").idx().and_then(|i| mats.get(i)) {
                Some(mat) => b.material(mat.clone()),
                None => b.material(plain()),
            };
            for t in tris {
                if t.iter().any(|&i| i >= pos.len()) {
                    return Err("glTF: an index past the vertices".into());
                }
                let t = if flip { [t[0], t[2], t[1]] } else { t };
                b.tri(t.map(|i| pos[i]), nor.as_ref().filter(|n| n.len() == pos.len()).map(|n| t.map(|i| n[i])), tex, group);
            }
        }
    }
    Ok(())
}

/// right-handed (OBJ, glTF) to left-handed (Tachyon) and back
fn mirror(a: V3) -> V3 {
    [a[0], a[1], -a[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-300);
    a.map(|v| v / l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, data: &[u8]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("skiagraph-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn an_obj_quad_with_a_glass_material_and_normals() {
        tmp("t.mtl", b"newmtl clear\nKd 0.2 0.4 0.9\nd 0.4\nnewmtl steel\nKd 0.7 0.7 0.7\nKs 0.9 0.9 0.9\nillum 3\n");
        let p = tmp("t.obj", b"mtllib t.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvn 0 0 1\no lid\nusemtl clear\nf 1//1 2//1 3//1 4//1\nusemtl steel\nf -4 -3 -2\nl 1 2\n");
        let sc = read(&p).unwrap();
        assert_eq!(sc.objects.len(), 3, "a quad as two triangles, and one more");
        assert!(matches!(sc.objects[0].shape, Shape::STri { .. }) && matches!(sc.objects[2].shape, Shape::Tri { .. }));
        assert_eq!(crate::ink::Mat::of(&sc.materials[sc.objects[0].tex]), crate::ink::Mat::Glass);
        assert_eq!(crate::ink::Mat::of(&sc.materials[sc.objects[2].tex]), crate::ink::Mat::Metal);
        assert_eq!(sc.groups[sc.objects[0].group.unwrap()], "lid");
        assert!(sc.camera.zoom > 0.0 && sc.camera.projection == Projection::Orthographic);
    }

    #[test]
    fn a_glb_triangle_moved_by_its_node_with_a_matte_material() {
        let mut bin = vec![];
        for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            bin.extend(v.to_le_bytes());
        }
        let js = br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"name":"plate","mesh":0,"translation":[5,0,0]}],
"meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0}]}],"materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1,0,0,1],"metallicFactor":0,"roughnessFactor":0.9}}],
"accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}],"bufferViews":[{"buffer":0,"byteLength":36}],"buffers":[{"byteLength":36}]}  "#;
        let mut glb = b"glTF".to_vec();
        glb.extend(2u32.to_le_bytes());
        glb.extend(((12 + 8 + js.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend((js.len() as u32).to_le_bytes());
        glb.extend(b"JSON");
        glb.extend(js);
        glb.extend((bin.len() as u32).to_le_bytes());
        glb.extend(b"BIN\0");
        glb.extend(&bin);
        let sc = read(&tmp("t.glb", &glb)).unwrap();
        let Shape::Tri { v } = &sc.objects[0].shape else { panic!("a flat triangle") };
        // moved by its node to (5..6, 0..1, 0), mirrored into Tachyon's left-handed space (winding kept: corners 1
        // and 2 swap), then fitted to 2 units across around the origin
        assert_eq!(v[2], [1.0, -1.0, 0.0]);
        assert_eq!(v[1], [-1.0, 1.0, 0.0]);
        assert_eq!(crate::ink::Mat::of(&sc.materials[0]), crate::ink::Mat::Matte);
        assert_eq!(sc.groups[0], "plate");
        assert_eq!(base64("AAECAw==").unwrap(), vec![0, 1, 2, 3]);
        assert!(read(&tmp("bad.gltf", b"{\"asset\":{\"version\":\"2.0\"}")).is_err(), "broken JSON is an error, not a crash");
    }

    /// Stage 5b: OBJ and glTF are right-handed, Tachyon left-handed: a model is not drawn as its mirror image. Seen
    /// from the default camera (in front, +z), the file's +x lies on the picture's right and +y above.
    #[test]
    fn a_mesh_is_not_drawn_mirrored() {
        let sc = read(&tmp("hand.obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 0.2\nf 1 2 3\nf 1 2 4\n")).unwrap();
        let tr = crate::tachyon::Tracer::new(&sc);
        let view = crate::svg::View::new(&tr);
        let p = |v: V3| view.project(mirror(v)).0;
        assert!(p([1.0, 0.0, 0.0])[0] > p([0.0, 0.0, 0.0])[0] + 100.0, "+x on the right");
        assert!(p([0.0, 1.0, 0.0])[1] < p([0.0, 0.0, 0.0])[1] - 100.0, "+y above (SVG y grows downwards)");
        assert!(view.project(mirror([0.0, 0.0, 0.2])).1 < view.project(mirror([0.0, 0.0, 0.0])).1, "+z nearer the eye");
    }
}
