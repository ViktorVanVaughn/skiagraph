//! Comic Ink's drawn light (stage 3, ADR 0015): tone bands, cast shadows, highlights, spot blacks and hatching, every
//! shape found on the 3D geometry and projected, never traced from a sampled picture.
//!
//! - **Tone bands** where the key light N·L falls below `MID` and `DARK` (spot blacks: `SPOT`). Their edges lie on the
//!   surface: on a sphere a circle (its picture an ellipse, a conic with perspective), on a tube two lines along its
//!   axis, in a smooth triangle the iso-line of its interpolated normal (refined until it strays less than `TOL` px
//!   from the true curve; pieces of neighbouring triangles meet on their shared edge). Each band is the item's exact
//!   visible region (`mesh::Vis`, cut by every opaque item in front) cut by those curves.
//! - **Cast shadows**: the occluders' outlines as the light sees them (a sphere's circle, a tube's two side lines and end
//!   circles, a mesh's edges between faces towards and away from the light) projected along the light onto the
//!   receiving sphere, tube or triangle; the region is where a shadow ray says the point is blocked (a yes/no question
//!   about the exact geometry, asked only while classifying the pieces between those curves).
//! - **Highlights**: where N·H exceeds `GLOSS` (Blinn), found like the bands, on curved surfaces only.
//! - **Hatching**: sections of each surface by a family of parallel planes (tilted towards the eye, so their pictures
//!   curve around the form: cross-contour), kept where they lie in the shadow band (or, where a part has no shadow band,
//!   in the darker half of the mid band, thinner: spheres and tubes only), and chained across triangles into strokes (`line::draw`).
//! - The light: Comic Ink's own key light from the upper left (`KEY`), or the scene's first directional light.
//! - Far tones are calmer: each band's opacity falls with depth in three steps (`FADE`).
//! Contact accents (stage 2's short occlusion rays) are gone: they were a traced field; see ADR 0015.

use crate::geom::{Arc, Curve, P2};
use crate::mesh::{self, Grid, Item};
use crate::scene::V3;
use crate::svg::{self, View};
use crate::tachyon::{Prim, Tracer};
use rayon::prelude::*;
use std::collections::HashMap;

/// Band edges (Lambert light) and the ink's opacity over each band: (1 - MID_ALPHA) = 0.78, then 0.56 in all.
pub const MID: f64 = 0.62;
pub const DARK: f64 = 0.30;
const MID_ALPHA: f64 = 0.22;
const DARK_ALPHA: f64 = 0.282;
/// Spot blacks: the deepest core, and cast shadows, as near-solid ink.
const SPOT: f64 = 0.10;
const SPOT_ALPHA: f64 = 0.85;
/// A cast shadow: the shadow tone on a lit surface (0.56).
const SHADOW_ALPHA: f64 = 0.44;
/// Highlight: N.H of Blinn's half vector at least this.
const GLOSS: f64 = 0.992;
/// Hatching: px between strokes (per 2400 px of picture), stroke width (px).
const GAP: f64 = 12.0;
const HATCH_W: f64 = 0.9;
/// Stage 4: a ball or tube narrower on the picture than SMALL px (per 2400 px of picture; a shape in the back counts up
/// to half as wide, a mesh in the back third as SMALL) gets every other hatching plane; under half of it, no hatching
/// and no mid band (its dark band alone gives the form). Keeps crowds of small atoms light.
const SMALL: f64 = 24.0;
/// Stage 4: no hatching where a mesh is seen this obliquely (cosine to the eye): there the cross-contour strokes
/// bunch into short scraps (a helix's inner faces). Balls and tubes keep theirs: their shadow band lies along
/// the rim, where the strokes turn with the form.
const GRAZE: f64 = 0.35;
/// Stage 4: how near (px per 2400, in space) a ball or tube must come to a mesh for a contact accent
const CONTACT: f64 = 10.0;
const CONTACT_ALPHA: f64 = 0.55;
/// Stage 4 materials: metal's highlight threshold (sharper than GLOSS), its dark reflected band (N·up between these:
/// just below the form's middle, where a chrome ball mirrors the horizon) and that band's darkness; glass's rim (where
/// N·eye is below RIM) and its darkness
const GLOSS_METAL: f64 = 0.998;
const REFL: [f64; 2] = [-0.35, 0.05];
const REFL_ALPHA: f64 = 0.55;
const RIM: f64 = 0.45;
/// Stage 4 rim preset: where N·light passes this, the surface is drawn as a white lit rim
const RIM_LIT: f64 = 0.25;
const RIM_ALPHA: f64 = 0.28;
/// Stage 4: longest chord of a shadow edge cast onto a ball or tube (px)
const CAST_CHORD: f64 = 1.5;
/// A stroke keeps to where the light has fallen this far below the band's edge (twice as far on every second plane).
const DEEP: f64 = 0.05;
/// A band edge in a triangle strays at most this far (px) from the true iso-line.
const TOL: f64 = 0.05;
/// Facets meeting at a corner within this (cosine of 50 degrees) share a rebuilt normal there.
const CREASE_COS: f64 = 0.64;
/// Comic Ink's key light, towards the light in the camera's frame (right, up, towards the viewer): upper left, about 45
/// degrees up and 45 degrees left, slightly in front.
const KEY: [f64; 3] = [-0.62, 0.62, 0.48];
/// Band opacity at the front, middle and back third of the scene's depth.
const FADE: [f64; 3] = [1.0, 0.8, 0.6];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn unit(a: V3) -> V3 {
    let l = dot(a, a).sqrt().max(1e-300);
    a.map(|v| v / l)
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, s: f64) -> V3 {
    a.map(|v| v * s)
}
fn lin3(c: &[V3; 3], b: [f64; 3]) -> V3 {
    [0, 1, 2].map(|i| b[0] * c[0][i] + b[1] * c[1][i] + b[2] * c[2][i])
}
fn d2(a: P2, b: P2) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}
/// Any unit vector perpendicular to a (and a third completing the frame).
fn frame(a: V3) -> (V3, V3) {
    let e1 = unit(cross(a, if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }));
    (e1, cross(a, e1))
}

/// A visible surface point: unit normal facing the eye, way to the eye, and whether the surface is curved there.
#[derive(Clone, Copy)]
pub struct Surf {
    pub n: V3,
    pub e: V3,
    curved: bool,
}

/// The way to the eye from p.
fn to_eye(view: &View, p: V3) -> V3 {
    match view.eye {
        Some(eye) => unit(sub(eye, p)),
        None => view.v.map(|c| -c),
    }
}

/// The surface of primitive `prim` at its point p (smooth triangles interpolate their corner normals).
fn surf(tr: &Tracer, view: &View, sm: &Sm, prim: usize, p: V3) -> Surf {
    let (n, curved) = match &tr.prims[prim].prim {
        Prim::Sphere { ctr, .. } => (unit(sub(p, *ctr)), true),
        Prim::FCyl { ctr, axis, .. } => {
            let a = unit(*axis);
            let v = sub(p, *ctr);
            (unit(sub(v, mul(a, dot(v, a)))), true)
        }
        // a triangle's normals already face the eye (`corner_normals`): near a silhouette the smooth normal may turn
        // away a little, as it should
        Prim::Tri { v0, e1, e2, .. } | Prim::STri { v0, e1, e2, .. } => match corner_normals(tr, view, sm, prim) {
            Some(nn) => return Surf { n: unit(lin3(&nn, bary(*v0, *e1, *e2, p))), e: to_eye(view, p), curved: bent(&nn) },
            None => (unit(cross(*e1, *e2)), false),
        },
        Prim::Ring { norm, .. } | Prim::Plane { norm, .. } => (unit(*norm), false),
    };
    let e = to_eye(view, p);
    let n = if dot(n, e) < 0.0 { n.map(|v| -v) } else { n };
    Surf { n, e, curved }
}

/// A triangle's first corner.
fn tr_corner(tr: &Tracer, prim: usize) -> V3 {
    match &tr.prims[prim].prim {
        Prim::Tri { v0, .. } | Prim::STri { v0, .. } => *v0,
        _ => [0.0; 3],
    }
}

/// Barycentric weights of p on the triangle v0, v0 + e1, v0 + e2.
fn bary(v0: V3, e1: V3, e2: V3, p: V3) -> [f64; 3] {
    let w = sub(p, v0);
    let (d11, d12, d22, dw1, dw2) = (dot(e1, e1), dot(e1, e2), dot(e2, e2), dot(w, e1), dot(w, e2));
    let den = (d11 * d22 - d12 * d12).max(1e-300);
    let (u, v) = ((d22 * dw1 - d12 * dw2) / den, (d11 * dw2 - d12 * dw1) / den);
    [1.0 - u - v, u, v]
}

/// The surface item `it` shows at picture point x, and its 3D point.
fn surf_at(tr: &Tracer, view: &View, sm: &Sm, it: &Item, x: P2) -> Option<(Surf, V3)> {
    let t = it.front(view, x)?;
    let p = view.point(x, t);
    Some((surf(tr, view, sm, it.prim, p), p))
}

/// Per primitive, corner normals rebuilt for a flat-shaded triangle (`smooth_normals`).
type Sm = [Option<[V3; 3]>];

/// A triangle's corner normals: its own (a smooth triangle) or rebuilt ones (a flat-shaded one), all three turned to
/// the eye's side of the facet (VMD gives some meshes normals pointing inwards). The light and the band edges both use
/// these, so they agree everywhere.
fn corner_normals(tr: &Tracer, view: &View, sm: &Sm, prim: usize) -> Option<[V3; 3]> {
    let (nn, v0, e1, e2) = match &tr.prims[prim].prim {
        Prim::STri { n, v0, e1, e2, .. } if bent(&n.map(unit)) => (n.map(unit), *v0, *e1, *e2),
        Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => (sm[prim]?, *v0, *e1, *e2),
        _ => return None,
    };
    // decided by the corner normals themselves (neighbours share them; VMD's winding is not consistent)
    let towards = dot(add(add(nn[0], nn[1]), nn[2]), to_eye(view, add(v0, mul(add(e1, e2), 1.0 / 3.0)))) >= 0.0;
    Some(if towards { nn } else { nn.map(|v| v.map(|c| -c)) })
}

/// Do the corner normals differ (a curved surface)?
fn bent(nn: &[V3; 3]) -> bool {
    dot(nn[0], nn[1]).min(dot(nn[1], nn[2])).min(dot(nn[0], nn[2])) < 0.9995
}

/// Corner normals for flat-shaded triangles (VMD draws beta strands and some tubes with one normal per facet): at each
/// corner, the mean of the facet normals (turned towards the eye, weighted by area) of the triangles of the same scene
/// group meeting there within `CREASE_COS`, so a faceted mesh gets bands that follow its form while real edges stay.
fn smooth_normals(tr: &Tracer, view: &View, all: &[Item]) -> Vec<Option<[V3; 3]>> {
    let mut out = vec![None; tr.prims.len()];
    let q = |v: V3| v.map(|x| (x as f32).to_bits());
    let face = |prim: usize| -> Option<([V3; 3], V3)> {
        let (v0, e1, e2) = match &tr.prims[prim].prim {
            Prim::Tri { v0, e1, e2 } => (*v0, *e1, *e2),
            Prim::STri { v0, e1, e2, n, .. } if !bent(&n.map(unit)) => (*v0, *e1, *e2),
            _ => return None,
        };
        let c = [v0, add(v0, e1), add(v0, e2)];
        let n = cross(e1, e2);
        let e = to_eye(view, c[0]);
        Some((c, if dot(n, e) < 0.0 { n.map(|v| -v) } else { n }))
    };
    let mut at: HashMap<([u32; 3], Option<usize>), Vec<V3>> = HashMap::new();
    let faces: Vec<(usize, [V3; 3], V3)> = all.iter().filter_map(|it| face(it.prim).map(|(c, n)| (it.prim, c, n))).collect();
    for (prim, c, n) in &faces {
        let g = tr.sc.objects[tr.prims[*prim].obj].group;
        for v in c {
            at.entry((q(*v), g)).or_default().push(*n);
        }
    }
    for (prim, c, n) in faces {
        let g = tr.sc.objects[tr.prims[prim].obj].group;
        let own = unit(n);
        out[prim] = Some(c.map(|v| {
            let sum = at[&(q(v), g)].iter().filter(|m| dot(unit(**m), own) > CREASE_COS).fold([0.0; 3], |s, m| add(s, *m));
            unit(sum)
        }));
    }
    out
}

/// Where N·v = t on primitive `prim`, as picture curves: a sphere's circle, a tube's two lines along its axis, a smooth
/// triangle's iso-line (refined to `TOL` px). Flat shapes have none: their light is one value.
fn iso(tr: &Tracer, view: &View, sm: &Sm, prim: usize, v: V3, t: f64) -> Vec<Curve> {
    let px = |p: V3| view.project(p).0;
    match &tr.prims[prim].prim {
        Prim::Sphere { ctr, rad } if t.abs() < 1.0 => mesh::circle3(view, add(*ctr, mul(v, rad * t)), v, rad * (1.0 - t * t).sqrt(), 1e-9).into_iter().collect(),
        Prim::FCyl { ctr, axis, rad } => {
            let a = unit(*axis);
            let vp = sub(v, mul(a, dot(v, a)));
            let l = dot(vp, vp).sqrt();
            if t.abs() >= l {
                return vec![];
            }
            let b1 = mul(vp, 1.0 / l);
            let b2 = cross(a, b1);
            let phi = (t / l).acos();
            [-1.0, 1.0]
                .iter()
                .map(|s| {
                    let p0 = add(*ctr, mul(add(mul(b1, phi.cos()), mul(b2, s * phi.sin())), *rad));
                    Curve::Seg { a: px(p0), b: px(add(p0, *axis)) }
                })
                .collect()
        }
        Prim::Tri { v0, e1, e2, .. } | Prim::STri { v0, e1, e2, .. } => {
            let c = [*v0, add(*v0, *e1), add(*v0, *e2)];
            let Some((pts, _)) = tri_split(tr, view, sm, prim, v, t) else { return vec![] };
            let mut q: Vec<P2> = pts.iter().map(|b| px(lin3(&c, *b))).collect();
            q.dedup();
            if q.len() < 2 {
                return vec![];
            }
            // both ends reach 0.01 px past the triangle's edges: a clean crossing, never a touch, for the region engine
            let n = q.len();
            for (e, f) in [(0, 1), (n - 1, n - 2)] {
                let (dx, dy) = (q[e][0] - q[f][0], q[e][1] - q[f][1]);
                let l = dx.hypot(dy).max(1e-12);
                q[e] = [q[e][0] + 0.01 * dx / l, q[e][1] + 0.01 * dy / l];
            }
            q.windows(2).map(|w| Curve::Seg { a: w[0], b: w[1] }).collect()
        }
        _ => vec![],
    }
}

/// Is x inside the polygon (even-odd)?
pub(crate) fn inside(poly: &[P2], x: P2) -> bool {
    let mut c = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a[1] > x[1]) != (b[1] > x[1]) && x[0] < a[0] + (x[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
            c = !c;
        }
    }
    c
}

/// Points of an occluder's outline as the light sees it, whose shadow's convex hull is the occluder's shadow on a
/// plane: a sphere's circle across the light, a tube's two end circles, a triangle's corners.
fn outline_points(tr: &Tracer, prim: usize, lt: V3, (l1, l2): (V3, V3)) -> Vec<V3> {
    let ring = |c: V3, u: V3, v: V3, n: usize| -> Vec<V3> { (0..n).map(|i| { let a = std::f64::consts::TAU * i as f64 / n as f64; add(c, add(mul(u, a.cos()), mul(v, a.sin()))) }).collect() };
    let _ = lt;
    match &tr.prims[prim].prim {
        Prim::Sphere { ctr, rad } => ring(*ctr, mul(l1, *rad), mul(l2, *rad), 48),
        Prim::FCyl { ctr, axis, rad } => {
            let (f1, f2) = frame(unit(*axis));
            let mut v = ring(*ctr, mul(f1, *rad), mul(f2, *rad), 24);
            v.extend(ring(add(*ctr, *axis), mul(f1, *rad), mul(f2, *rad), 24));
            v
        }
        Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => vec![*v0, add(*v0, *e1), add(*v0, *e2)],
        _ => vec![],
    }
}

/// Convex hull (monotone chain), turning the positive way.
fn convex_hull(mut p: Vec<P2>) -> Vec<P2> {
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let turn = |o: P2, a: P2, b: P2| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut h: Vec<P2> = vec![];
    for pass in 0..2 {
        let start = h.len();
        let it: Box<dyn Iterator<Item = &P2>> = if pass == 0 { Box::new(p.iter()) } else { Box::new(p.iter().rev()) };
        for &q in it {
            while h.len() >= start + 2 && turn(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
                h.pop();
            }
            h.push(q);
        }
        h.pop();
    }
    h
}

/// Sutherland-Hodgman: the polygon `subject` clipped to the convex polygon `clip` (turning the positive way).
fn clip_convex(subject: &[P2], clip: &[P2]) -> Vec<P2> {
    let mut out = subject.to_vec();
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let side = |p: P2| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= 0.0;
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (p, q) = (input[j], input[(j + 1) % input.len()]);
            if side(p) {
                out.push(p);
            }
            if side(p) != side(q) {
                let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
                let den = (b[0] - a[0]) * dy - (b[1] - a[1]) * dx;
                if den.abs() > 1e-300 {
                    let s = ((b[1] - a[1]) * (p[0] - a[0]) - (b[0] - a[0]) * (p[1] - a[1])) / den;
                    out.push([p[0] + s * dx, p[1] + s * dy]);
                }
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// A smooth triangle cut by its iso-line N·v = t: the iso-line as barycentric points (from the crossing on its first
/// crossed edge to the one on its second, refined until it strays less than `TOL` px from the true curve; empty when
/// the line misses the triangle), and which corners lie below t. None for a triangle without corner normals.
fn tri_split(tr: &Tracer, view: &View, sm: &Sm, prim: usize, v: V3, t: f64) -> Option<(Vec<[f64; 3]>, Vec<bool>)> {
    let (Prim::Tri { v0, e1, e2, .. } | Prim::STri { v0, e1, e2, .. }) = &tr.prims[prim].prim else { return None };
    let c = [*v0, add(*v0, *e1), add(*v0, *e2)];
    let px = |p: V3| view.project(p).0;
    let nn = corner_normals(tr, view, sm, prim)?;
    let f = |b: [f64; 3]| dot(unit(lin3(&nn, b)), v) - t;
    let corner = |i: usize| { let mut b = [0.0; 3]; b[i] = 1.0; b };
    let mix = |a: [f64; 3], b: [f64; 3], s: f64| [0, 1, 2].map(|i| a[i] + s * (b[i] - a[i]));
    // the zero of f between a and b (f changes sign there), by halving
    let zero = |a: [f64; 3], b: [f64; 3]| {
        let neg = f(a) < 0.0;
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..40 {
            let m = 0.5 * (lo + hi);
            if (f(mix(a, b, m)) < 0.0) == neg { lo = m } else { hi = m }
        }
        mix(a, b, 0.5 * (lo + hi))
    };
    let sign: Vec<bool> = (0..3).map(|i| f(corner(i)) < 0.0).collect();
    let zs: Vec<[f64; 3]> = [(0, 1), (1, 2), (2, 0)].iter().filter(|&&(i, j)| sign[i] != sign[j]).map(|&(i, j)| zero(corner(i), corner(j))).collect();
    if zs.len() != 2 {
        return Some((vec![], sign));
    }
    // the corner alone on its side: the iso-line separates it from the other two
    let lone = (0..3).find(|&i| sign[i] != sign[(i + 1) % 3] && sign[i] != sign[(i + 2) % 3]).unwrap_or(0);
    let cl = corner(lone);
    let mut pts = vec![zs[0]];
    // refine between two points on the iso-line: the true curve crosses the line from the lone corner through
    // the chord's middle; split while it strays more than TOL px from the chord
    fn refine(a: [f64; 3], b: [f64; 3], depth: u32, cl: [f64; 3], f: &dyn Fn([f64; 3]) -> f64, zero: &dyn Fn([f64; 3], [f64; 3]) -> [f64; 3], far: &dyn Fn([f64; 3], [f64; 3]) -> f64, out: &mut Vec<[f64; 3]>) {
        let m = [0, 1, 2].map(|i| 0.5 * (a[i] + b[i]));
        // from the lone corner through m to the opposite edge (where another weight reaches 0)
        let d = [0, 1, 2].map(|i| m[i] - cl[i]);
        let s_max = (0..3).filter(|&i| d[i] < -1e-15).map(|i| -cl[i] / d[i]).fold(f64::MAX, f64::min).min(1e6);
        let e = [0, 1, 2].map(|i| cl[i] + s_max * d[i]);
        let z = if (f(cl) < 0.0) != (f(e) < 0.0) { zero(cl, e) } else { m };
        if depth < 5 && far(z, m) > TOL {
            refine(a, z, depth + 1, cl, f, zero, far, out);
            refine(z, b, depth + 1, cl, f, zero, far, out);
        } else {
            out.push(b);
        }
    }
    let far = |p: [f64; 3], q: [f64; 3]| d2(px(lin3(&c, p)), px(lin3(&c, q)));
    refine(zs[0], zs[1], 0, cl, &f, &zero, &far, &mut pts);
    Some((pts, sign))
}

/// The part of a whole (uncut) triangle where N·v < t, built directly: its corners on that side and the refined
/// iso-line, as one loop turning the same way as the region engine's (so shared edges cancel when merged).
fn tri_band(tr: &Tracer, view: &View, sm: &Sm, prim: usize, v: V3, t: f64) -> Option<(Vec<Curve>, Vec<Vec<Arc>>)> {
    let (Prim::Tri { v0, e1, e2, .. } | Prim::STri { v0, e1, e2, .. }) = &tr.prims[prim].prim else { return None };
    let c = [*v0, add(*v0, *e1), add(*v0, *e2)];
    let (iso, sign) = tri_split(tr, view, sm, prim, v, t)?;
    let corner = |i: usize| { let mut b = [0.0; 3]; b[i] = 1.0; b };
    // walk the corners: a corner below t, then on an edge where the side changes, the iso-line (entered at that edge)
    let mut poly: Vec<[f64; 3]> = vec![];
    let mut used_iso = false;
    for i in 0..3 {
        let j = (i + 1) % 3;
        if sign[i] {
            poly.push(corner(i));
        }
        // leaving the side below t: the iso-line from this edge's crossing (its end with zero weight on the third corner)
        // to where the walk comes back below t
        if sign[i] && !sign[j] && !used_iso && !iso.is_empty() {
            let k = 3 - i - j;
            let mut line = iso.clone();
            if line[0][k].abs() > line[line.len() - 1][k].abs() {
                line.reverse();
            }
            poly.extend(line);
            used_iso = true;
        }
    }
    if !sign.iter().any(|&s| s) && iso.is_empty() {
        return None;
    }
    poly_region(view, &c, &poly)
}

/// The fade's depth thirds (`FADE`) a whole triangle's band crosses: its barycentric polygon cut along the planes where
/// the depth (linear across the flat triangle) reaches `cuts`, one piece per third.
fn tri_band_slices(tr: &Tracer, view: &View, sm: &Sm, prim: usize, v: V3, t: f64, cuts: [f64; 2]) -> Vec<(usize, (Vec<Curve>, Vec<Vec<Arc>>))> {
    let Some((curves, _)) = tri_band(tr, view, sm, prim, v, t) else { return vec![] };
    let (Prim::Tri { v0, e1, e2, .. } | Prim::STri { v0, e1, e2, .. }) = &tr.prims[prim].prim else { return vec![] };
    let c = [*v0, add(*v0, *e1), add(*v0, *e2)];
    // back to barycentric points on the triangle (the polygon's corners), with their depths
    let poly: Vec<[f64; 3]> = curves.iter().map(|cv| {
        let x = cv.point(0.0);
        let p = c.map(|q| view.project(q).0);
        let den = (p[1][1] - p[2][1]) * (p[0][0] - p[2][0]) + (p[2][0] - p[1][0]) * (p[0][1] - p[2][1]);
        let a = ((p[1][1] - p[2][1]) * (x[0] - p[2][0]) + (p[2][0] - p[1][0]) * (x[1] - p[2][1])) / den;
        let b = ((p[2][1] - p[0][1]) * (x[0] - p[2][0]) + (p[0][0] - p[2][0]) * (x[1] - p[2][1])) / den;
        [a, b, 1.0 - a - b]
    }).collect();
    let dc = c.map(|q| view.project(q).1);
    let depth = |b: &[f64; 3]| b[0] * dc[0] + b[1] * dc[1] + b[2] * dc[2];
    // keep the part of a polygon where keep(depth) (Sutherland-Hodgman against one plane)
    let clip = |poly: &[[f64; 3]], c0: f64, above: bool| -> Vec<[f64; 3]> {
        let side = |b: &[f64; 3]| (depth(b) >= c0) == above;
        let mut out = vec![];
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if side(&a) {
                out.push(a);
            }
            if side(&a) != side(&b) {
                let (da, db) = (depth(&a), depth(&b));
                let s = (c0 - da) / (db - da);
                out.push([0, 1, 2].map(|k| a[k] + s * (b[k] - a[k])));
            }
        }
        out
    };
    let (lo, hi) = dc.iter().fold((f64::MAX, f64::MIN), |(a, b), &d| (a.min(d), b.max(d)));
    let mut out = vec![];
    for sl in 0..3 {
        let (from, to) = (if sl == 0 { f64::MIN } else { cuts[sl - 1] }, if sl == 2 { f64::MAX } else { cuts[sl] });
        if hi < from || lo >= to {
            continue;
        }
        let mut piece = poly.clone();
        if lo < from {
            piece = clip(&piece, from, true);
        }
        if hi >= to {
            piece = clip(&piece, to, false);
        }
        if piece.len() >= 3 {
            if let Some(r) = poly_region(view, &c, &piece) {
                out.push((sl, r));
            }
        }
    }
    out
}

/// A barycentric polygon on the triangle c as one picture loop turning the region engine's way.
fn poly_region(view: &View, c: &[V3; 3], poly: &[[f64; 3]]) -> Option<(Vec<Curve>, Vec<Vec<Arc>>)> {
    loop_region(poly.iter().map(|b| view.project(lin3(c, *b)).0).collect())
}

/// A picture polygon as one loop turning the region engine's way (None if it encloses nothing).
fn loop_region(mut p: Vec<P2>) -> Option<(Vec<Curve>, Vec<Vec<Arc>>)> {
    p.dedup_by(|a, b| d2(*a, *b) < 1e-9);
    if p.len() > 2 && d2(p[0], p[p.len() - 1]) < 1e-9 {
        p.pop();
    }
    if p.len() < 3 {
        return None;
    }
    let area: f64 = (0..p.len()).map(|i| { let (a, b) = (p[i], p[(i + 1) % p.len()]); a[0] * b[1] - b[0] * a[1] }).sum();
    if area < 0.0 {
        p.reverse();
    }
    let n = p.len();
    let curves: Vec<Curve> = (0..n).map(|i| Curve::Seg { a: p[i], b: p[(i + 1) % n] }).collect();
    let arcs = (0..n).map(|i| Arc { curve: i, t0: 0.0, t1: 1.0, v0: usize::MAX, v1: usize::MAX }).collect();
    Some((curves, vec![arcs]))
}

/// An occluder's outline as the light sees it: closed circles (centre, two radius vectors) and segments.
enum Sil {
    Circle(V3, V3, V3),
    Seg(V3, V3),
}

impl Sil {
    fn at(&self, s: f64) -> V3 {
        match self {
            Sil::Circle(c, u, v) => {
                let a = s * std::f64::consts::TAU;
                add(*c, add(mul(*u, a.cos()), mul(*v, a.sin())))
            }
            Sil::Seg(a, b) => add(*a, mul(sub(*b, *a), s)),
        }
    }
}

/// The first point where the ray o + s d (s > 0) meets primitive `prim`'s surface (a triangle: its plane).
fn hit(tr: &Tracer, prim: usize, o: V3, d: V3) -> Option<V3> {
    let quad = |a: f64, b: f64, c: f64| -> Option<f64> {
        let disc = b * b - a * c;
        if disc < 0.0 || a.abs() < 1e-300 {
            return None;
        }
        let s = (-b - disc.sqrt()) / a;
        (s > 0.0).then_some(s)
    };
    match &tr.prims[prim].prim {
        Prim::Sphere { ctr, rad } => {
            let w = sub(o, *ctr);
            quad(dot(d, d), dot(d, w), dot(w, w) - rad * rad).map(|s| add(o, mul(d, s)))
        }
        Prim::FCyl { ctr, axis, rad } => {
            let a = unit(*axis);
            let w = sub(o, *ctr);
            let (dp, wp) = (sub(d, mul(a, dot(d, a))), sub(w, mul(a, dot(w, a))));
            let s = quad(dot(dp, dp), dot(dp, wp), dot(wp, wp) - rad * rad)?;
            let p = add(o, mul(d, s));
            let h = dot(sub(p, *ctr), a);
            (h >= 0.0 && h <= dot(*axis, *axis).sqrt()).then_some(p)
        }
        Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => {
            let n = cross(*e1, *e2);
            let den = dot(n, d);
            if den.abs() < 1e-300 {
                return None;
            }
            let s = dot(n, sub(*v0, o)) / den;
            (s > 0.0).then(|| add(o, mul(d, s)))
        }
        _ => None,
    }
}

/// A closed loop of points on a ball (centre c, radius r) clipped to the side (p - c)·n >= d of a plane: where the loop
/// leaves that side and comes back, it follows the circle where the plane cuts the ball (the short way round), in steps
/// of at most `step` (world units).
fn clip_on_ball(lp: &[V3], c: V3, r: f64, n: V3, d: f64, step: f64) -> Vec<V3> {
    let f = |p: V3| dot(sub(p, c), n) - d;
    if d <= -r || lp.iter().all(|&p| f(p) >= 0.0) {
        return lp.to_vec();
    }
    if d >= r {
        return vec![];
    }
    let (m, rho) = (add(c, mul(n, d)), (r * r - d * d).sqrt());
    let (f1, f2) = frame(n);
    // a crossing pushed onto the circle, as its angle there
    let ang = |p: V3| { let w = sub(p, m); dot(w, f2).atan2(dot(w, f1)) };
    let on = |a: f64| add(m, add(mul(f1, rho * a.cos()), mul(f2, rho * a.sin())));
    let mut out = vec![];
    let mut exit: Option<f64> = None;
    // start the walk at a point inside so that every exit is followed by its entry
    let Some(s) = (0..lp.len()).find(|&i| f(lp[i]) >= 0.0) else { return vec![] };
    for i in 0..lp.len() {
        let (a, b) = (lp[(s + i) % lp.len()], lp[(s + i + 1) % lp.len()]);
        if f(a) >= 0.0 {
            out.push(a);
        }
        if (f(a) >= 0.0) != (f(b) >= 0.0) {
            let x = ang(add(a, mul(sub(b, a), f(a) / (f(a) - f(b)))));
            match exit.take() {
                None => {
                    out.push(on(x));
                    exit = Some(x);
                }
                Some(x0) => {
                    let mut da = x - x0;
                    while da > std::f64::consts::PI { da -= std::f64::consts::TAU; }
                    while da < -std::f64::consts::PI { da += std::f64::consts::TAU; }
                    let n = ((da.abs() * rho / step).ceil() as usize).max(1);
                    out.extend((1..=n).map(|k| on(x0 + da * k as f64 / n as f64)));
                }
            }
        }
    }
    out
}

/// Stage 4 (defect 4): the shadow of occluder `pj` on ball `prim`, built from the geometry: the occluder's outline as
/// the light sees it (a convex hull across the light) cut to the ball's part lit above the shadow band (a disc across
/// the light), lifted onto the ball, then cut by planes: below the occluder (along the light) and on the half that
/// faces the viewer. Returns the picture polygon (chords at most CAST_CHORD px).
fn ball_shadow(tr: &Tracer, view: &View, prim: usize, pj: usize, lt: V3, (l1, l2): (V3, V3)) -> Option<Vec<P2>> {
    let Prim::Sphere { ctr: c, rad: r } = &tr.prims[prim].prim else { return None };
    let (c, r) = (*c, *r);
    let to2 = |p: V3| [dot(sub(p, c), l1), dot(sub(p, c), l2)];
    let lift = |q: P2| add(c, add(add(mul(l1, q[0]), mul(l2, q[1])), mul(lt, (r * r - q[0] * q[0] - q[1] * q[1]).max(0.0).sqrt())));
    let step = CAST_CHORD * view.psx;
    // the occluder's lowest point along the light that still shadows: a ball's centre, a tube's axis point nearest the
    // ball (a tube bonded into the ball is skipped: its joint is covered), a triangle's lowest corner
    let low = match &tr.prims[pj].prim {
        // a ball overlapping this one (a bonded neighbour in a space-filling model) is skipped: in a packed model every
        // atom would sit in its neighbours' shadows and the picture turn murky (and the file half as big again)
        Prim::Sphere { ctr, rad } if dot(sub(c, *ctr), sub(c, *ctr)).sqrt() < r + rad => return None,
        Prim::Sphere { ctr, .. } => *ctr,
        Prim::FCyl { ctr, axis, .. } => {
            let s = (dot(sub(c, *ctr), *axis) / dot(*axis, *axis)).clamp(0.0, 1.0);
            let m = add(*ctr, mul(*axis, s));
            if dot(sub(c, m), sub(c, m)).sqrt() < r {
                return None;
            }
            m
        }
        Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => {
            [*v0, add(*v0, *e1), add(*v0, *e2)].into_iter().min_by(|a, b| dot(*a, lt).total_cmp(&dot(*b, lt)))?
        }
        _ => return None,
    };
    let hull = convex_hull(outline_points(tr, pj, lt, (l1, l2)).iter().map(|p| to2(*p)).collect());
    if hull.len() < 3 {
        return None;
    }
    let lr = r * (1.0 - DARK * DARK).sqrt();
    let n = ((std::f64::consts::TAU * lr / step).ceil() as usize).clamp(16, 1024);
    let disc: Vec<P2> = (0..n).map(|i| { let a = std::f64::consts::TAU * i as f64 / n as f64; [lr * a.cos(), lr * a.sin()] }).collect();
    let cut = clip_convex(&hull, &disc);
    if cut.len() < 3 {
        return None;
    }
    // edges split finely, then lifted onto the ball
    let mut lp = vec![];
    for i in 0..cut.len() {
        let (a, b) = (cut[i], cut[(i + 1) % cut.len()]);
        let k = ((d2(a, b) / step).ceil() as usize).max(1);
        lp.extend((0..k).map(|s| lift([a[0] + (b[0] - a[0]) * s as f64 / k as f64, a[1] + (b[1] - a[1]) * s as f64 / k as f64])));
    }
    let lp = clip_on_ball(&lp, c, r, lt.map(|v| -v), -dot(sub(low, c), lt), step);
    let (n_eye, d_eye) = match view.eye {
        Some(e) => (unit(sub(e, c)), r * r / dot(sub(e, c), sub(e, c)).sqrt()),
        None => (to_eye(view, c), 0.0),
    };
    let lp = clip_on_ball(&lp, c, r, n_eye, d_eye, step);
    (lp.len() >= 3).then(|| lp.iter().map(|p| view.project(*p).0).collect())
}

/// The occluder's silhouette `sil` cast along the light (away from it) onto primitive `prim`, as picture segments:
/// sampled finely enough that neighbouring points lie under 1.5 px apart (halving up to 7 times).
fn cast(tr: &Tracer, view: &View, prim: usize, sil: &Sil, to_light: V3, out: &mut Vec<Curve>) {
    let away = to_light.map(|v| -v);
    let map = |s: f64| hit(tr, prim, sil.at(s), away).map(|p| view.project(p).0);
    fn go(map: &dyn Fn(f64) -> Option<P2>, a: (f64, Option<P2>), b: (f64, Option<P2>), depth: u32, out: &mut Vec<Curve>) {
        let split = match (a.1, b.1) {
            (Some(p), Some(q)) => d2(p, q) > CAST_CHORD,
            (None, None) => false,
            _ => true,
        };
        if split && depth < 7 {
            let m = 0.5 * (a.0 + b.0);
            let mp = (m, map(m));
            go(map, a, mp, depth + 1, out);
            go(map, mp, b, depth + 1, out);
        } else if let (Some(p), Some(q)) = (a.1, b.1) {
            if p != q {
                out.push(Curve::Seg { a: p, b: q });
            }
        }
    }
    let n0 = if matches!(sil, Sil::Circle(..)) { 24 } else { 2 };
    let pts: Vec<(f64, Option<P2>)> = (0..=n0).map(|i| { let s = i as f64 / n0 as f64; (s, map(s)) }).collect();
    for w in pts.windows(2) {
        go(&map, w[0], w[1], 0, out);
    }
}

/// The overlays, as SVG groups.
pub struct Overlays {
    pub tones: String,
    pub shadows: String,
    pub highlights: String,
    pub hatching: String,
    /// glass rims and highlights (stage 4), drawn over the glass tint
    pub glass: String,
}

/// Stage 4: Comic Ink's materials. Each changes which light marks a shape gets and how dark they are:
/// matte: no highlight, softer bands; plastic: today's look; metal: hard dark bands, a dark band reflecting the horizon,
/// a small sharp highlight, no hatching; glass: a tint with a darker rim along its outline and a highlight; emissive: no
/// shadow band, no hatching, rings of glow around it (`flat.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Mat {
    Matte,
    Plastic,
    Metal,
    Glass,
    Emissive,
}

impl Mat {
    /// From VMD's material as Tachyon writes it (VMD's specular is Tachyon's `Phong`, its reflectivity Tachyon's
    /// `Specular`): see-through is glass; a mirror (RTChrome) or a weak diffuse under a strong highlight (BrushedMetal,
    /// MetallicPastel, Steel) is metal; no highlight (Diffuse, AOChalky) is matte. VMD has no glowing material (its unlit
    /// reps, Points and Lines, are written as Ambient 1 Diffuse 0 and stay plastic): emissive comes from the recipe only.
    pub fn of(m: &crate::scene::Material) -> Mat {
        if (m.opacity as f64) < 0.99 {
            Mat::Glass
        } else if m.phong_type == crate::scene::PhongType::Metal || m.specular >= 0.3 || (m.diffuse <= 0.4 && m.phong >= 0.3) {
            Mat::Metal
        } else if m.phong <= 0.05 {
            Mat::Matte
        } else {
            Mat::Plastic
        }
    }
    pub fn parse(s: &str) -> Option<Mat> {
        Some(match s {
            "matte" => Mat::Matte,
            "plastic" => Mat::Plastic,
            "metal" => Mat::Metal,
            "glass" => Mat::Glass,
            "emissive" => Mat::Emissive,
            _ => return None,
        })
    }
    /// how dark its bands are against plastic's
    fn band(self) -> f64 {
        match self {
            Mat::Matte => 0.7,
            Mat::Metal => 1.7,
            _ => 1.0,
        }
    }
}

/// Stage 4 lighting presets (`light=`): studio (Comic Ink's key from the upper left, slightly in front: the default),
/// rim (from behind and above: the forms dark, lit along their edges), low (from below at the front, the old monster-movie
/// light), night (a cool key from the upper right over a dark ground, `flat.rs`), scene (VMD's own first light).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    Studio,
    Rim,
    Low,
    Night,
    Scene,
}

impl Preset {
    pub fn parse(s: &str) -> Option<Preset> {
        Some(match s {
            "studio" | "comic" => Preset::Studio,
            "rim" => Preset::Rim,
            "low" => Preset::Low,
            "night" => Preset::Night,
            "scene" => Preset::Scene,
            _ => return None,
        })
    }
    /// towards the key light, in the picture's frame (right, up, towards the viewer)
    fn key(self) -> [f64; 3] {
        match self {
            Preset::Rim => [0.35, 0.55, -0.76],
            Preset::Low => [-0.45, -0.72, 0.53],
            Preset::Night => [0.62, 0.62, 0.48],
            _ => KEY,
        }
    }
}

/// Comic Ink's light: towards the key light, whether spot blacks are drawn, and whether shadows are cast.
pub struct Light {
    pub to_light: V3,
    pub spot: bool,
    pub cast: bool,
    pub preset: Preset,
}

impl Light {
    /// The preset's key light; `scene`: the scene's first light (a directional one as it is; a point light as seen
    /// from the scene's middle `centre`: one direction for the whole picture). `cast`: shadows drawn (VMD's shadow switch,
    /// unless the recipe says otherwise).
    pub fn new(tr: &Tracer, view: &View, preset: Preset, spot: bool, cast: bool, centre: V3) -> Light {
        let mut k = preset.key();
        // a model with no lights of its own (OBJ, glTF): put the key on the side its seen surfaces mostly face, so the
        // side we see is lit (the classic three-quarter light); VMD scenes keep the key on the upper left
        if tr.sc.lights.is_empty() {
            let lean: f64 = tr.sc.objects.iter().filter_map(|o| match &o.shape {
                crate::scene::Shape::Tri { v } | crate::scene::Shape::STri { v, .. } => Some(cross(sub(v[1], v[0]), sub(v[2], v[0]))),
                _ => None,
            })
            // area-weighted, two-sided, by how much each face is seen
            .map(|n| -dot(n, view.v).signum() * dot(n, view.r) * dot(n, view.v).abs() / dot(n, n).sqrt().max(1e-300))
            .sum();
            k[0] = k[0].abs() * lean.signum();
        }
        let own = unit(add(add(mul(view.r, k[0]), mul(view.u, k[1])), mul(view.v, -k[2])));
        let to_light = if preset == Preset::Scene {
            tr.sc.lights.first().map_or(own, |l| match l {
                crate::scene::Light::Directional { dir, .. } => unit(dir.map(|v| -v)),
                crate::scene::Light::Point { center, .. } => unit(sub(*center, centre)),
            })
        } else {
            own
        };
        Light { to_light, spot, cast, preset }
    }
}

/// Kinds of overlay region, in painting order.
const MIDB: usize = 0;
const DARKB: usize = 1;
/// stage 5: a small shape's dark band, drawn alone (no mid band under it)
const DARKS: usize = 8;

/// Stage 5: at most this many cast shadows per ball, the largest (`overlays`).
const BALL_SHADOWS: usize = 48;

/// Time (ns, summed over threads) and count of cast-shadow work per receiver kind: triangles, balls, tubes and the rest.
pub static SHADOW_NS: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];
pub static SHADOW_N: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];

/// Stage 5: a tone band's colour, as a comic colourist shades: the fill darker, more saturated and turned a little in
/// hue (warm colours towards red, cool ones towards blue-violet; a grey towards cool slate). `a` is the darkness the
/// black ink used to give (0.44 = the dark band): the value falls by 0.8 a, so the bands keep their old steps.
pub fn shade(c: [f64; 3], a: f64) -> [f64; 3] {
    let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let mut s = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
    let d = (mx - mn).max(1e-12);
    let mut h = if mx == mn { 0.0 } else if mx == c[0] { 60.0 * ((c[1] - c[2]) / d).rem_euclid(6.0) } else if mx == c[1] { 60.0 * ((c[2] - c[0]) / d + 2.0) } else { 60.0 * ((c[0] - c[1]) / d + 4.0) };
    let t = a / 0.44;
    if s < 0.12 {
        h = 220.0;
        s += 0.16 * t;
    } else {
        s = (s + (1.0 - s) * 0.3 * t).min(1.0);
        h = if h < 100.0 { (h - 16.0 * t).max(0.0) } else if h >= 330.0 { (h + 16.0 * t).min(360.0) } else { h + (245.0 - h).clamp(-12.0 * t, 12.0 * t) };
    }
    let v = mx * (1.0 - 0.8 * a);
    let f = |n: f64| { let k = (n + h / 60.0).rem_euclid(6.0); v - v * s * k.min(4.0 - k).clamp(0.0, 1.0) };
    [f(5.0), f(3.0), f(1.0)]
}

fn shade_hex(fill: &str, a: f64) -> String {
    let c = [1, 3, 5].map(|i| f64::from(u8::from_str_radix(fill.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0)) / 255.0);
    let s = shade(c, a).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{:02x}{:02x}{:02x}", s[0], s[1], s[2])
}
const CORE: usize = 2;
const CAST: usize = 3;
const GLINT: usize = 4;
const CONTACT_K: usize = 5;
const REFLB: usize = 6;
const RIMB: usize = 7;

/// `lit(k)`: item k gets light drawn on it; `hides(j)`: item j hides what lies behind it (casts shadows); `cuts(k, j)`:
/// item j cuts item k's visible region (a mesh triangle is not cut by its own mesh's colour: like the fills, its pieces
/// then meet on whole shared edges and merge into one shape); `depth(k)`: 0 front .. 1 back;
/// `thirds`: the depths (as `View::project` gives them) between the fade's thirds; `size`: the scene's depth span (world
/// units).
#[allow(clippy::too_many_arguments)]
pub fn overlays(tr: &Tracer, view: &View, all: &[Item], grid: &Grid, lit: &(dyn Fn(usize) -> bool + Sync), mat: &(dyn Fn(usize) -> Mat + Sync), hides: &(dyn Fn(usize) -> bool + Sync), cuts: &(dyn Fn(usize, usize) -> bool + Sync), depth: &(dyn Fn(usize) -> f64 + Sync), thirds: [f64; 2], ink: &str, size: f64, light: &Light, fill: &(dyn Fn(usize, Option<usize>) -> String + Sync), cells: &[(Vec<([f64; 3], Vec<P2>)>, Vec<Curve>)]) -> Overlays {
    let sc = (f64::from(view.w) / 2400.0).max(0.25);
    let lt = light.to_light;
    let eps = 4e-3 * size;
    let shadowed = |s: &Surf, p: V3| dot(s.n, lt) > 0.12 && tr.blocked(add(p, mul(add(s.n, lt), eps)), lt);
    let gloss = |s: &Surf, g: f64| s.curved && dot(s.n, unit(add(lt, s.e))) >= g;
    // occluders as the light sees them: every primitive's box across the light, and how near the light it reaches
    let (l1, l2) = frame(lt);
    let corners = |prim: usize| -> (Vec<V3>, f64) {
        match &tr.prims[prim].prim {
            Prim::Sphere { ctr, rad } => (vec![*ctr], *rad),
            Prim::FCyl { ctr, axis, rad } => (vec![*ctr, add(*ctr, *axis)], *rad),
            Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => (vec![*v0, add(*v0, *e1), add(*v0, *e2)], 0.0),
            _ => (vec![], 0.0),
        }
    };
    let lbox: Vec<(P2, P2, f64, f64)> = all
        .iter()
        .map(|it| {
            let (cs, r) = corners(it.prim);
            let (mut lo, mut hi, mut hlo, mut hhi) = ([f64::MAX; 2], [f64::MIN; 2], f64::MAX, f64::MIN);
            for c in cs {
                let (a, b, h) = (dot(c, l1), dot(c, l2), dot(c, lt));
                lo = [lo[0].min(a - r), lo[1].min(b - r)];
                hi = [hi[0].max(a + r), hi[1].max(b + r)];
                (hlo, hhi) = (hlo.min(h - r), hhi.max(h + r));
            }
            (lo, hi, hlo, hhi)
        })
        .collect();
    let smooth = smooth_normals(tr, view, all);
    let sm: &Sm = &smooth;
    let lgrid = Grid::new(&lbox.iter().map(|b| (b.0, b.1)).collect::<Vec<_>>());
    // a mesh's outline as the light sees it: edges between a face towards the light and one away, and open edges
    let mut edges: HashMap<([u32; 3], [u32; 3]), Vec<(usize, bool)>> = HashMap::new();
    let q = |v: V3| v.map(|x| (x as f32).to_bits());
    for (k, it) in all.iter().enumerate() {
        if !hides(k) {
            continue;
        }
        if let Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } = &tr.prims[it.prim].prim {
            let mut nf = cross(*e1, *e2);
            if let Prim::STri { n, .. } = &tr.prims[it.prim].prim {
                if dot(nf, add(add(n[0], n[1]), n[2])) < 0.0 {
                    nf = nf.map(|v| -v);
                }
            }
            let facing = dot(nf, lt) > 0.0;
            let c = [*v0, add(*v0, *e1), add(*v0, *e2)];
            for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                let (a, b) = (q(c[i]), q(c[j]));
                edges.entry(if a < b { (a, b) } else { (b, a) }).or_default().push((k, facing));
            }
        }
    }
    let mut sil_edges: HashMap<usize, Vec<(V3, V3)>> = HashMap::new();
    for (k, it) in all.iter().enumerate() {
        if let Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } = &tr.prims[it.prim].prim {
            let c = [*v0, add(*v0, *e1), add(*v0, *e2)];
            for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                let (a, b) = (q(c[i]), q(c[j]));
                if let Some(v) = edges.get(&if a < b { (a, b) } else { (b, a) }) {
                    if v.len() == 1 || v.iter().any(|e| e.1 != v[0].1) {
                        sil_edges.entry(k).or_default().push((c[i], c[j]));
                    }
                }
            }
        }
    }
    let sils = |j: usize| -> Vec<Sil> {
        match &tr.prims[all[j].prim].prim {
            Prim::Sphere { ctr, rad } => vec![Sil::Circle(*ctr, mul(l1, *rad), mul(l2, *rad))],
            Prim::FCyl { ctr, axis, rad } => {
                let a = unit(*axis);
                let b = cross(a, lt);
                let (f1, f2) = frame(a);
                let mut v = vec![Sil::Circle(*ctr, mul(f1, *rad), mul(f2, *rad)), Sil::Circle(add(*ctr, *axis), mul(f1, *rad), mul(f2, *rad))];
                if dot(b, b) > 1e-12 {
                    let b = mul(unit(b), *rad);
                    for s in [1.0, -1.0] {
                        let p = add(*ctr, mul(b, s));
                        v.push(Sil::Seg(p, add(p, *axis)));
                    }
                }
                v
            }
            _ => sil_edges.get(&j).map(|es| es.iter().map(|&(a, b)| Sil::Seg(a, b)).collect()).unwrap_or_default(),
        }
    };
    // hatching planes: normal D (across the light's direction on the picture, tilted 35 degrees towards the eye), one
    // every GAP px of picture; plane i keeps to where the light is DEEP (odd i: twice that) below the band's edge
    let ls = [dot(lt, view.r), dot(lt, view.u)];
    let ll = ls[0].hypot(ls[1]).max(1e-9);
    let pdir = add(mul(view.r, -ls[1] / ll), mul(view.u, ls[0] / ll));
    let dn = unit(add(pdir, mul(view.v, -0.7)));
    let spacing = GAP * sc * view.psx * dot(dn, pdir);
    type Region = (Vec<Curve>, Vec<Vec<Arc>>);
    let per_item: Vec<(Vec<(usize, usize, Region, Option<usize>)>, Vec<(usize, i64, Vec<P2>)>)> = (0..all.len())
        .into_par_iter()
        .map(|k| {
            if !lit(k) {
                return (vec![], vec![]);
            }
            let it = &all[k];
            let prim = it.prim;
            // a triangle seen edge-on (thinner than 0.2 px per 2400 on average) shows no light worth drawing
            if let Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } = &tr.prims[prim].prim {
                let c = [*v0, add(*v0, *e1), add(*v0, *e2)].map(|p| view.project(p).0);
                let area = 0.5 * ((c[1][0] - c[0][0]) * (c[2][1] - c[0][1]) - (c[2][0] - c[0][0]) * (c[1][1] - c[0][1])).abs();
                if area < 0.05 * sc * (d2(c[0], c[1]) + d2(c[1], c[2]) + d2(c[2], c[0])) {
                    return (vec![], vec![]);
                }
            }
            let m = mat(k);
            if m == Mat::Emissive {
                return (vec![], vec![]);
            }
            let vis = mesh::visibility(view, all, grid, k, &|j| !cuts(k, j));
            let slice = ((depth(k) * 3.0) as usize).min(2);
            // stage 4: small and far shapes get fewer marks (`SMALL`); a far mesh counts as small
            let wide = match &it.shape {
                mesh::Shape::S(s, _) => 2.0 * s.rpx,
                mesh::Shape::C(c, _) => 2.0 * c.rpx,
                _ => if slice == 2 { SMALL * sc } else { f64::MAX },
            } / sc * (1.0 - 0.5 * depth(k));
            let (tiny, small) = (wide < 0.5 * SMALL, wide < SMALL + 1e-9);
            let lam = |x: P2| surf_at(tr, view, sm, it, x).map_or(1.0, |(s, _)| dot(s.n, lt));
            let mut out = vec![];
            // the light's range on this primitive: a band nobody's light reaches below is skipped
            let (lmin, lmax) = match &tr.prims[prim].prim {
                Prim::Tri { .. } | Prim::STri { .. } => match corner_normals(tr, view, sm, prim) {
                    Some(nn) => nn.iter().map(|v| dot(*v, lt)).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v - 0.02), b.max(v + 0.02))),
                    None => { let v = surf(tr, view, sm, prim, tr_corner(tr, prim)).n; let v = dot(v, lt); (v, v) }
                },
                _ => (-1.0, 1.0),
            };
            let mut bands = if m == Mat::Glass { vec![] } else if tiny { vec![(DARKS, DARK)] } else { vec![(MIDB, MID), (DARKB, DARK)] };
            if light.spot {
                bands.push((CORE, SPOT));
            }
            for (kind, t) in bands {
                if lmin < t {
                    let extra = if lmax <= t { vec![] } else { iso(tr, view, sm, prim, lt, t) };
                    // a mesh triangle nothing cuts: its band built directly; otherwise the region engine
                    let whole_tri = vis.whole() && matches!(&tr.prims[prim].prim, Prim::Tri { .. } | Prim::STri { .. });
                    let is_tri = matches!(&tr.prims[prim].prim, Prim::Tri { .. } | Prim::STri { .. });
                    if whole_tri {
                        for (sl, r) in tri_band_slices(tr, view, sm, prim, lt, t, thirds) {
                            out.push((kind, sl, r));
                        }
                        continue;
                    }
                    let part = if is_tri {
                        // cut by the region engine, "below t" asked of the same polyline the band edge is drawn with
                        tri_band(tr, view, sm, prim, lt, t).and_then(|(cs, _)| {
                            let poly: Vec<P2> = cs.iter().map(|c| c.point(0.0)).collect();
                            vis.part(view, all, &extra, &|x| inside(&poly, x))
                        })
                    } else {
                        vis.part(view, all, &extra, &|x| lam(x) < t)
                    };
                    if let Some(r) = part {
                        out.push((kind, slice, r));
                    }
                }
            }
            // stage 4 material bands: the part where lo <= N·v < hi (a mesh triangle asks the same polygons its edges are
            // drawn with, as above); metal's dark band reflecting the horizon, glass's rim where it turns from the eye
            let is_tri = matches!(&tr.prims[prim].prim, Prim::Tri { .. } | Prim::STri { .. });
            let band_part = |v: V3, lo: f64, hi: f64| -> Option<Region> {
                if is_tri {
                    let poly = |t: f64| tri_band(tr, view, sm, prim, v, t).map(|(cs, _)| (cs.iter().map(|c| c.point(0.0)).collect::<Vec<P2>>(), cs));
                    let (ph, ch) = poly(hi)?;
                    let low = poly(lo);
                    let mut extra = ch;
                    if let Some((_, cl)) = &low {
                        extra.extend(cl.iter().cloned());
                    }
                    vis.part(view, all, &extra, &|x| inside(&ph, x) && !low.as_ref().is_some_and(|(pl, _)| inside(pl, x)))
                } else {
                    let mut extra = iso(tr, view, sm, prim, v, hi);
                    extra.extend(iso(tr, view, sm, prim, v, lo));
                    vis.part(view, all, &extra, &|x| surf_at(tr, view, sm, it, x).is_some_and(|(s, _)| (lo..hi).contains(&dot(s.n, v))))
                }
            };
            let eye = to_eye(view, view.point([0.5 * (it.lo[0] + it.hi[0]), 0.5 * (it.lo[1] + it.hi[1])], it.key));
            if m == Mat::Metal && !tiny {
                if let Some(r) = band_part(view.u, REFL[0], REFL[1]) {
                    out.push((REFLB, slice, r));
                }
            }
            // glass rim: balls and tubes, front walls only; a glass mesh gets none (every fold of a bumpy surface drew a
            // grey vein): its outline and its tint carry it
            let back_wall = match &tr.prims[prim].prim {
                Prim::STri { n, .. } => dot(add(add(n[0], n[1]), n[2]), eye) < 0.0,
                _ => false,
            };
            // rim preset: a white lit rim where the surface turns towards the light behind it
            if light.preset == Preset::Rim && m != Mat::Glass && !tiny {
                if let Some(r) = band_part(lt, RIM_LIT, 2.0) {
                    out.push((GLINT, 0, r));
                }
            }
            if m == Mat::Glass && !back_wall && !is_tri {
                if let Some(r) = band_part(eye, -2.0, RIM) {
                    out.push((RIMB, 0, r));
                }
            }
            // cast shadows: on a triangle, each occluder's shadow on the triangle's plane (its outline as the light sees it,
            // cast along the light: a convex shape) clipped to the triangle's lit part (and, if something covers the
            // triangle, to what shows); on spheres and tubes, the outlines cast onto the surface and the region engine
            let t_sh = std::time::Instant::now();
            let b = &lbox[k];
            let cands: Vec<usize> = lgrid.query(b.0, b.1).into_iter().filter(|&j| j != k && hides(j) && all[j].prim != prim && lbox[j].3 > b.2).collect();
            let whole_tri = is_tri && vis.whole();
            let mut shadow_edges = vec![];
            if m == Mat::Glass || !light.cast || tiny {
                // glass takes no shadows; none at all when shadows are off (its tint already lies over what is behind);
                // stage 5: nor does a tiny shape (its one band gives its form; a shadow there is a speck, and in a
                // 292 000-atom crowd it cost most of the light's time)
            } else if is_tri && lmax >= DARK {
                if let Prim::Tri { v0, e1, e2, .. } | Prim::STri { v0, e1, e2, .. } = &tr.prims[prim].prim {
                    let nrm = cross(*e1, *e2);
                    let den = dot(nrm, lt);
                    let lit: Option<Vec<P2>> = tri_band(tr, view, sm, prim, lt.map(|v| -v), -DARK).map(|(cs, _)| cs.iter().map(|c| c.point(0.0)).collect());
                    let group = |p: usize| tr.sc.objects[tr.prims[p].obj].group;
                    if let (Some(lit), true) = (lit, den.abs() > 1e-12) {
                        for &j in &cands {
                            let pj = all[j].prim;
                            // a mesh does not shadow itself here (its own triangles' shadows would be thousands of slivers)
                            // (stage 4, tried: letting triangles further than 5 % of the scene's depth shadow their own
                            // mesh cost only +2.5 % on a test frame but drew jagged shards on the helix's inner faces: left out)
                            if matches!(&tr.prims[pj].prim, Prim::Tri { .. } | Prim::STri { .. }) && group(pj) == group(prim) {
                                continue;
                            }
                            let mut flat = vec![];
                            let mut ahead = true;
                            for p in outline_points(tr, pj, lt, (l1, l2)) {
                                let s = dot(nrm, sub(p, *v0)) / den;
                                if s <= 0.0 {
                                    ahead = false;
                                    break;
                                }
                                flat.push(view.project(sub(p, mul(lt, s))).0);
                            }
                            if !ahead || flat.len() < 3 {
                                continue;
                            }
                            let hull = convex_hull(flat);
                            if hull.len() >= 3 {
                                let piece = clip_convex(&lit, &hull);
                                // a triangle something covers: the piece cut to what shows, asked of the piece itself
                                let r = if whole_tri {
                                    loop_region(piece)
                                } else {
                                    loop_region(piece.clone()).and_then(|(cs, _)| vis.part(view, all, &cs, &|x| inside(&piece, x)))
                                };
                                if let Some(r) = r {
                                    out.push((CAST, slice, r));
                                }
                            }
                        }
                    }
                }
            } else if matches!(&tr.prims[prim].prim, Prim::Sphere { .. }) {
                // a ball: each occluder's shadow built from the geometry (`ball_shadow`), cut to what shows
                if lmax >= DARK {
                    // all occluders' shadows on this ball at once: one pass of the region engine (one per pair made the
                    // 292 000-atom membrane many times slower)
                    let mut polys: Vec<Vec<P2>> = cands.iter().filter_map(|&j| ball_shadow(tr, view, prim, all[j].prim, lt, (l1, l2))).collect();
                    // stage 5: a ball deep in a crowd collects hundreds of shadows and the region engine paid for each
                    // (374 CPU s for 7 125 balls in the membrane): only the BALL_SHADOWS largest are kept; by then the
                    // ball's lit part is covered. Each is tested against its box first.
                    // ponytail: largest-area cap, a true union of the polygons if a buried ball ever shows a lit gap
                    let area = |p: &Vec<P2>| (0..p.len()).map(|i| { let (a, b) = (p[i], p[(i + 1) % p.len()]); a[0] * b[1] - b[0] * a[1] }).sum::<f64>().abs();
                    if polys.len() > BALL_SHADOWS {
                        polys.sort_by(|a, b| area(b).total_cmp(&area(a)));
                        polys.truncate(BALL_SHADOWS);
                    }
                    let boxes: Vec<[f64; 4]> = polys.iter().map(|p| p.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, q| [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])])).collect();
                    for poly in &polys {
                        shadow_edges.extend((0..poly.len()).map(|i| Curve::Seg { a: poly[i], b: poly[(i + 1) % poly.len()] }));
                    }
                    if !polys.is_empty() {
                        let any = |x: P2| polys.iter().zip(&boxes).any(|(p, b)| x[0] >= b[0] && x[0] <= b[2] && x[1] >= b[1] && x[1] <= b[3] && inside(p, x));
                        if let Some(r) = vis.part(view, all, &shadow_edges, &any) {
                            out.push((CAST, slice, r));
                        }
                    }
                }
            } else {
                for &j in &cands {
                    for s in sils(j) {
                        cast(tr, view, prim, &s, lt, &mut shadow_edges);
                    }
                }
                let dark_edges = iso(tr, view, sm, prim, lt, DARK);
                if !shadow_edges.is_empty() && lmax >= DARK {
                    let mut extra = shadow_edges.clone();
                    extra.extend(dark_edges.iter().cloned());
                    let r = vis.part(view, all, &extra, &|x| surf_at(tr, view, sm, it, x).is_some_and(|(s, p)| dot(s.n, lt) >= DARK && shadowed(&s, p)));
                    if let Some(r) = r {
                        out.push((CAST, slice, r));
                    }
                }
            }
            // time spent on cast shadows, by the receiver's kind (MOL25D_FLAT_TIMES)
            let kind = match &tr.prims[prim].prim { Prim::Tri { .. } | Prim::STri { .. } => 0, Prim::Sphere { .. } => 1, _ => 2 };
            SHADOW_NS[kind].fetch_add(t_sh.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            SHADOW_N[kind].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // contact accents (stage 4, defect 3): on a mesh triangle, the part within CONTACT px (in space) of a ball or
            // tube of another rep lying on the viewer's side: the plane's exact cut through the shape grown by that much
            // (a circle; for a tube an ellipse closed by its end discs' chords), clipped to the triangle and to what
            // shows. The shape covers the middle, leaving a dark rim along its outline where it rests on the surface.
            if let (true, Prim::Tri { v0, e1, e2, .. } | Prim::STri { v0, e1, e2, .. }) = (is_tri && m != Mat::Glass, &tr.prims[prim].prim) {
                let mut nrm = unit(cross(*e1, *e2));
                if dot(nrm, to_eye(view, *v0)) < 0.0 {
                    nrm = mul(nrm, -1.0);
                }
                let d = CONTACT * sc * view.psx;
                let tri2 = convex_hull([*v0, add(*v0, *e1), add(*v0, *e2)].map(|p| view.project(p).0).to_vec());
                let group = |p: usize| tr.sc.objects[tr.prims[p].obj].group;
                let pad = CONTACT * sc + 1.0;
                let mut done = vec![];
                for j in grid.query([it.lo[0] - pad, it.lo[1] - pad], [it.hi[0] + pad, it.hi[1] + pad]) {
                    let pj = all[j].prim;
                    if pj == prim || !hides(j) || group(pj) == group(prim) || done.contains(&pj) {
                        continue;
                    }
                    done.push(pj);
                    let h = |p: V3| dot(nrm, sub(p, *v0));
                    // points around a circle (centre m, unit frame f1 f2, radius r) fine enough for TOL px
                    let ring = |r: f64| ((std::f64::consts::PI / (1.0 - TOL / (r / view.psx).max(TOL)).clamp(-1.0, 1.0).acos()).ceil() as usize).clamp(12, 256);
                    let mut pts: Vec<V3> = vec![];
                    match &tr.prims[pj].prim {
                        Prim::Sphere { ctr, rad } => {
                            let (hc, r) = (h(*ctr), rad + d);
                            if hc <= 0.0 || hc >= r {
                                continue;
                            }
                            let rho = (r * r - hc * hc).sqrt();
                            let (f1, f2) = frame(nrm);
                            let m = sub(*ctr, mul(nrm, hc));
                            let n = ring(rho);
                            pts.extend((0..n).map(|i| { let a = std::f64::consts::TAU * i as f64 / n as f64; add(m, add(mul(f1, rho * a.cos()), mul(f2, rho * a.sin()))) }));
                        }
                        Prim::FCyl { ctr, axis, rad } => {
                            let r = rad + d;
                            if h(add(*ctr, mul(*axis, 0.5))) <= 0.0 {
                                continue;
                            }
                            let (f1, f2) = frame(unit(*axis));
                            let na = dot(nrm, *axis);
                            let n = ring(r);
                            for i in 0..n {
                                let a = std::f64::consts::TAU * i as f64 / n as f64;
                                let q = add(*ctr, add(mul(f1, r * a.cos()), mul(f2, r * a.sin())));
                                let s = -h(q) / na;
                                if na.abs() > 1e-12 && (0.0..=1.0).contains(&s) {
                                    pts.push(add(q, mul(*axis, s)));
                                }
                            }
                            // where each end disc crosses the plane: h(E) + A cos(phi - phi0) = 0
                            let (c1, c2) = (dot(nrm, f1), dot(nrm, f2));
                            let amp = r * c1.hypot(c2);
                            for e in [*ctr, add(*ctr, *axis)] {
                                if amp > 1e-12 && h(e).abs() <= amp {
                                    let (p0, da) = (c2.atan2(c1), (-h(e) / amp).acos());
                                    for phi in [p0 + da, p0 - da] {
                                        pts.push(add(e, add(mul(f1, r * phi.cos()), mul(f2, r * phi.sin()))));
                                    }
                                }
                            }
                        }
                        _ => continue,
                    }
                    let hull = convex_hull(pts.iter().map(|p| view.project(*p).0).collect());
                    if hull.len() < 3 {
                        continue;
                    }
                    let piece = clip_convex(&tri2, &hull);
                    if let Some(r) = loop_region(piece.clone()).and_then(|(cs, _)| vis.part(view, all, &cs, &|x| inside(&piece, x))) {
                        out.push((CONTACT_K, slice, r));
                    }
                }
            }
            // highlights (curved surfaces whose normals come near the half vector)
            let curved = matches!(&tr.prims[prim].prim, Prim::Sphere { .. } | Prim::FCyl { .. }) || corner_normals(tr, view, sm, prim).is_some_and(|nn| bent(&nn));
            if curved && m != Mat::Matte {
                let h = unit(add(lt, eye));
                // metal: a smaller, sharper highlight
                let g = if m == Mat::Metal { GLOSS_METAL } else { GLOSS };
                let mut extra = iso(tr, view, sm, prim, h, g);
                if !extra.is_empty() || matches!(&tr.prims[prim].prim, Prim::STri { .. }) {
                    extra.extend(shadow_edges.iter().cloned());
                    let part = if whole_tri {
                        tri_band(tr, view, sm, prim, h.map(|v| -v), -g)
                    } else {
                        vis.part(view, all, &extra, &|x| surf_at(tr, view, sm, it, x).is_some_and(|(s, p)| gloss(&s, g) && !shadowed(&s, p)))
                    };
                    if let Some(r) = part {
                        out.push((GLINT, 0, r));
                    }
                }
            }
            // hatching: plane sections of this primitive, kept where they are seen and dark enough
            let mut strokes = vec![];
            let sections = |c: f64| -> Vec<Vec<V3>> {
                let step = 0.75 * view.psx * sc.min(1.0);
                match &tr.prims[prim].prim {
                    Prim::Sphere { ctr, rad } => {
                        let d = c - dot(dn, *ctr);
                        if d.abs() >= *rad {
                            return vec![];
                        }
                        let rho = (rad * rad - d * d).sqrt();
                        let m = add(*ctr, mul(dn, d));
                        let (f1, f2) = frame(dn);
                        let n = ((std::f64::consts::TAU * rho / step).ceil() as usize).clamp(8, 4000);
                        vec![(0..=n).map(|i| { let a = std::f64::consts::TAU * i as f64 / n as f64; add(m, add(mul(f1, rho * a.cos()), mul(f2, rho * a.sin()))) }).collect()]
                    }
                    Prim::FCyl { ctr, axis, rad } => {
                        let da = dot(dn, *axis);
                        if da.abs() < 1e-9 {
                            return vec![];
                        }
                        let (f1, f2) = frame(unit(*axis));
                        let n = ((std::f64::consts::TAU * rad / step).ceil() as usize).clamp(8, 4000);
                        let mut runs: Vec<Vec<V3>> = vec![vec![]];
                        for i in 0..=n {
                            let a = std::f64::consts::TAU * i as f64 / n as f64;
                            let q = add(*ctr, add(mul(f1, rad * a.cos()), mul(f2, rad * a.sin())));
                            let s = (c - dot(dn, q)) / da;
                            if (0.0..=1.0).contains(&s) {
                                runs.last_mut().unwrap().push(add(q, mul(*axis, s)));
                            } else if !runs.last().unwrap().is_empty() {
                                runs.push(vec![]);
                            }
                        }
                        runs
                    }
                    Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => {
                        let cs = [*v0, add(*v0, *e1), add(*v0, *e2)];
                        let h = cs.map(|p| dot(dn, p) - c);
                        let mut ends = vec![];
                        for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                            if (h[i] < 0.0) != (h[j] < 0.0) {
                                let s = h[i] / (h[i] - h[j]);
                                ends.push(add(cs[i], mul(sub(cs[j], cs[i]), s)));
                            }
                        }
                        if ends.len() != 2 {
                            return vec![];
                        }
                        let len = dot(sub(ends[1], ends[0]), sub(ends[1], ends[0])).sqrt();
                        let n = ((len / step).ceil() as usize).clamp(1, 4000);
                        vec![(0..=n).map(|i| add(ends[0], mul(sub(ends[1], ends[0]), i as f64 / n as f64))).collect()]
                    }
                    _ => vec![],
                }
            };
            let range = {
                let (cs, r) = corners(prim);
                cs.iter().map(|c| dot(dn, *c)).fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v - r), b.max(v + r)))
            };
            if lmin < MID && spacing > 0.0 && range.0 < range.1 && !tiny && matches!(m, Mat::Matte | Mat::Plastic) && light.preset != Preset::Rim {
                // the shadow band first; a part with no stroke there gets lighter ones in the darker half of the mid band
                for (key, top) in [(0usize, DARK), (1, 0.5 * (MID + DARK))] {
                    if key == 1 && (!strokes.is_empty() || !matches!(&tr.prims[prim].prim, Prim::Sphere { .. } | Prim::FCyl { .. })) {
                        break;
                    }
                    for i in (range.0 / spacing).ceil() as i64..=(range.1 / spacing).floor() as i64 {
                        if small && i % 2 != 0 {
                            continue;
                        }
                        let thr = top - if i % 2 != 0 { 2.0 * DEEP } else { DEEP };
                        for sec in sections(i as f64 * spacing) {
                            let mut run: Vec<P2> = vec![];
                            for p in sec {
                                let (x, t) = view.project(p);
                                let s = surf(tr, view, sm, prim, p);
                                let ok = it.front(view, x).is_some_and(|tf| (tf - t).abs() < 1e-3 * size)
                                    && dot(s.n, lt) < thr
                                    && (!is_tri || dot(s.n, s.e) > GRAZE)
                                    && vis.visible(view, all, x);
                                if ok {
                                    run.push(x);
                                } else if run.len() > 1 {
                                    strokes.push((key, i, std::mem::take(&mut run)));
                                } else {
                                    run.clear();
                                }
                            }
                            if run.len() > 1 {
                                strokes.push((key, i, run));
                            }
                        }
                    }
                }
            }
            // stage 5: a triangle of two colours (flat's colour cells) has its tone regions cut along the same cells, so
            // each part takes the shade of its own colour
            let out: Vec<(usize, usize, Region, Option<usize>)> = match cells.get(k).filter(|c| !c.0.is_empty()) {
                None => out.into_iter().map(|(a, b, r)| (a, b, r, None)).collect(),
                Some((cl, edges)) => out
                    .into_iter()
                    .flat_map(|(kind, sl, r)| {
                        if !matches!(kind, MIDB | DARKB | DARKS | CAST) {
                            return vec![(kind, sl, r, None)];
                        }
                        let cs = &r.0;
                        let polys: Vec<Vec<P2>> = r.1.iter().map(|lp| lp.iter().flat_map(|a| (0..8).map(move |i| cs[a.curve].point(a.t0 + f64::from(i) / 8.0 * (a.t1 - a.t0)))).collect()).collect();
                        let mut extra = r.0.clone();
                        extra.extend(edges.iter().cloned());
                        cl.iter().enumerate().filter_map(|(i, (_, cell))| vis.part(view, all, &extra, &|x| inside(cell, x) && polys.iter().filter(|p| inside(p, x)).count() % 2 == 1).map(|q| (kind, sl, q, Some(i)))).collect()
                    })
                    .collect(),
            };
            (out, strokes)
        })
        .collect();
    // every kind of region merged per depth step into one path
    // (kind, depth third, material: only where it changes the paint)
    let mut groups: HashMap<(usize, usize, Mat), Vec<Region>> = HashMap::new();
    let mut tone_groups: std::collections::BTreeMap<(bool, String), Vec<Region>> = std::collections::BTreeMap::new();
    let mut pieces = vec![];
    let mut all_strokes = vec![];
    for (k, (regs, strokes)) in per_item.into_iter().enumerate() {
        let m = mat(k);
        for (kind, slice, r, cell) in regs {
            let km = match (kind, m) {
                (MIDB | DARKB, Mat::Matte | Mat::Metal) | (GLINT, Mat::Glass) => m,
                _ => Mat::Plastic,
            };
            // slivers (loops thinner than 0.2 px per 2400 on average: where neighbouring triangles touch) and loops the region
            // engine left open are dropped
            let (curves, loops) = r;
            let loops: Vec<Vec<Arc>> = loops
                .into_iter()
                .filter(|lp| {
                    let (a, b) = (&lp[0], &lp[lp.len() - 1]);
                    if d2(curves[a.curve].point(a.t0), curves[b.curve].point(b.t1)) > 0.01 {
                        return false;
                    }
                    let pts: Vec<P2> = lp.iter().flat_map(|a| (0..8).map(move |i| f64::from(i) / 8.0).map(|u| curves[a.curve].point(a.t0 + u * (a.t1 - a.t0)))).collect();
                    let n = pts.len();
                    let area = 0.5 * (0..n).map(|i| { let (p, q) = (pts[i], pts[(i + 1) % n]); p[0] * q[1] - q[0] * p[1] }).sum::<f64>().abs();
                    let perim: f64 = (0..n).map(|i| d2(pts[i], pts[(i + 1) % n])).sum();
                    area >= 0.05 * sc * perim
                })
                .collect();
            if loops.is_empty() {
                continue;
            }
            // stage 5: tone bands and cast shadows are solid colours made from the fill under them (`shade`), merged per
            // colour; mid bands first, the darker tones over them
            let (mid, dark) = ((MID_ALPHA * m.band()).min(0.6), (DARK_ALPHA * m.band()).min(0.6));
            let a = match kind {
                MIDB => Some(mid),
                DARKB => Some(1.0 - (1.0 - mid) * (1.0 - dark)),
                DARKS => Some(dark),
                CAST if !light.spot => Some(SHADOW_ALPHA),
                _ => None,
            };
            if let Some(a) = a {
                tone_groups.entry((kind != MIDB, shade_hex(&fill(k, cell), a * FADE[slice]))).or_default().push((curves, loops));
            } else {
                groups.entry((kind, slice, km)).or_default().push((curves, loops));
            }
        }
        all_strokes.extend(strokes);
    }
    // strokes never crowd: plane by plane, a stroke stops where it comes within 0.6 gap of a stroke of another plane
    // already kept (where the sections converge on the picture, near a form's edge)
    all_strokes.sort_by_key(|s| s.1);
    let near = 0.6 * GAP * sc;
    let ck = |p: P2| ((p[0] / near).floor() as i64, (p[1] / near).floor() as i64);
    let mut kept: HashMap<(i64, i64), Vec<(i64, P2)>> = HashMap::new();
    for (key, plane, pts) in all_strokes {
        let crowded = |p: P2, kept: &HashMap<(i64, i64), Vec<(i64, P2)>>| {
            let c = ck(p);
            (-1..=1).any(|dx| (-1..=1).any(|dy| kept.get(&(c.0 + dx, c.1 + dy)).is_some_and(|v| v.iter().any(|&(pl, q)| pl != plane && d2(p, q) < near))))
        };
        let mut run: Vec<P2> = vec![];
        let mut runs = vec![];
        for p in pts {
            if crowded(p, &kept) {
                if run.len() > 1 {
                    runs.push(std::mem::take(&mut run));
                }
                run.clear();
            } else {
                run.push(p);
            }
        }
        if run.len() > 1 {
            runs.push(run);
        }
        for r in runs {
            for &p in &r {
                kept.entry(ck(p)).or_default().push((plane, p));
            }
            let w = vec![HATCH_W * sc * if key == 1 { 0.6 } else { 1.0 }; r.len()];
            pieces.push(crate::line::Piece { key, pts: r, w });
        }
    }
    let path = |kind: usize, slice: usize, m: Mat, attrs: String| -> String {
        let Some(rs) = groups.get(&(kind, slice, m)) else { return String::new() };
        let (curves, loops) = crate::flat::dissolve(rs.iter());
        let d = svg::path_data(&curves, &loops);
        if d.is_empty() { String::new() } else { format!("<path{attrs} d=\"{d}\"/>\n") }
    };
    let alpha = |a: f64| svg::frac(a);
    let mut tones = String::new();
    for ((_, c), rs) in &tone_groups {
        let (curves, loops) = crate::flat::dissolve(rs.iter());
        let d = svg::path_data(&curves, &loops);
        if !d.is_empty() {
            tones += &format!("<path fill=\"{c}\" d=\"{d}\"/>\n");
        }
    }
    for slice in 0..3 {
        tones += &path(REFLB, slice, Mat::Plastic, format!(" class=\"metal\" fill=\"{ink}\" fill-opacity=\"{}\"", alpha(REFL_ALPHA * FADE[slice])));
    }
    let mut shadows = String::new();
    // with spot blacks, cast shadows stay near-solid ink
    for slice in 0..3 {
        shadows += &path(CAST, slice, Mat::Plastic, format!(" class=\"cast\" fill=\"{ink}\" fill-opacity=\"{}\"", alpha(SPOT_ALPHA)));
    }
    for slice in 0..3 {
        shadows += &path(CONTACT_K, slice, Mat::Plastic, format!(" class=\"contact\" fill=\"{ink}\" fill-opacity=\"{}\"", alpha(CONTACT_ALPHA * FADE[slice])));
    }
    for slice in 0..3 {
        shadows += &path(CORE, slice, Mat::Plastic, format!(" class=\"spot\" fill=\"{ink}\" fill-opacity=\"{}\"", alpha(SPOT_ALPHA)));
    }
    let highlights = path(GLINT, 0, Mat::Plastic, " fill=\"#ffffff\"".into());
    // glass: its rim and its highlight, drawn over its tint
    let glass = path(RIMB, 0, Mat::Plastic, format!(" class=\"rim\" fill=\"{ink}\" fill-opacity=\"{}\"", alpha(RIM_ALPHA))) + &path(GLINT, 0, Mat::Glass, " fill=\"#ffffff\" fill-opacity=\"0.85\"".into());
    let hd = crate::line::draw(&pieces, [f64::from(view.w), f64::from(view.h)], 2.0 * GAP * sc, 0.2);
    let mut hatching = String::new();
    for (key, op) in [(0usize, ""), (1, " fill-opacity=\"0.7\"")] {
        if let Some(d) = hd.get(&key) {
            hatching += &format!("<path fill=\"{ink}\"{op} d=\"{d}\"/>\n");
        }
    }
    Overlays { tones, shadows, highlights, hatching, glass }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Defect 4 (stage 4): a loop on a ball cut by a plane keeps its points on the kept side, closes along the circle
    /// where the plane cuts the ball, and every point stays on the ball.
    #[test]
    fn a_loop_cut_on_a_ball_stays_on_the_ball() {
        let (c, r) = ([0.0, 0.0, 0.0], 1.0);
        // a small circle of latitude around the +x pole, cut by the plane y >= 0
        let lp: Vec<V3> = (0..64).map(|i| { let a = std::f64::consts::TAU * i as f64 / 64.0; let (s, q) = (0.5f64, 0.75f64.sqrt()); [q, s * a.cos(), s * a.sin()] }).collect();
        let cut = clip_on_ball(&lp, c, r, [0.0, 1.0, 0.0], 0.0, 0.05);
        assert!(cut.len() > 32);
        for p in &cut {
            assert!(p[1] >= -1e-9, "kept side: {p:?}");
            assert!((dot(*p, *p).sqrt() - r).abs() < 1e-9, "on the ball: {p:?}");
        }
    }

    /// A sphere in front of a large triangle, lit from the upper left front: the sphere gets both tone bands and a
    /// highlight, and casts a shadow on the triangle behind it; the band edge on the ball is one conic, not a traced
    /// polygon.
    #[test]
    fn a_ball_gets_bands_a_highlight_and_casts_a_shadow() {
        let tex = "Texture Ambient 0.3 Diffuse 0.6 Specular 0 Opacity 1\n  Phong Plastic 0 Phong_size 2 Color 0.8 0.3 0.3 TexFunc 0\n";
        let sc = crate::read_tachyon::parse(&format!(
            "Begin_Scene\nResolution 240 200\nShader_Mode Medium\nEnd_Shader_Mode\nCamera\n  Projection Orthographic\n  Zoom 0.5\n  \
             Aspectratio 1\n  Antialiasing 0\n  Raydepth 50\n  Center 0 0 -2\n  Viewdir 0 0 2\n  Updir 0 1 0\nEnd_Camera\n\
             Directional_Light Direction 0.3 -0.3 1 Color 1 1 1\nBackground 1 1 1\n\
             Sphere Center 0 0 0 Rad 0.5\n{tex}Tri V0 -2 -2 0.8 V1 2 -2 0.8 V2 0 2 0.8\n{tex}End_Scene\n"
        ))
        .unwrap();
        let tr = Tracer::new(&sc);
        let view = View::new(&tr);
        let (all, _) = mesh::items(&tr, &view).unwrap();
        let grid = Grid::new(&all.iter().map(|it| (it.lo, it.hi)).collect::<Vec<_>>());
        let light = Light::new(&tr, &view, Preset::Scene, false, true, [0.0; 3]);
        let o = overlays(&tr, &view, &all, &grid, &|_| true, &|_| Mat::Plastic, &|_| true, &|_, _| true, &|_| 0.0, [f64::MAX; 2], "#000000", 2.0, &light, &|k, _| ["#ffff00", "#00ffff"][k.min(1)].into(), &[]);
        // the ball's mid and dark bands, and the shadow on the triangle in the triangle's own shade colour
        assert!(o.tones.matches("<path").count() >= 3, "mid and dark bands, cast shadow: {}", o.tones);
        assert!(!o.tones.contains("fill-opacity"), "bands are solid colours: {}", o.tones);
        assert!(o.tones.contains('A') || o.tones.contains('a'), "band edges are elliptic arcs: {}", o.tones);
        assert!(!o.highlights.is_empty(), "a highlight on the ball");
    }

    #[test]
    fn shading_turns_yellow_golden_and_grey_slate() {
        let y = shade([1.0, 1.0, 0.0], 0.44);
        assert!(y[0] > 0.6 && y[1] < 0.75 * y[0] && y[2] < 0.05, "yellow towards a deeper orange: {y:?}");
        let g = shade([0.6, 0.6, 0.6], 0.22);
        assert!(g[2] > g[0] && g[2] < 0.6, "grey towards cool slate: {g:?}");
        assert_eq!(shade([0.3, 0.5, 0.7], 0.0), [0.3, 0.5, 0.7].map(|v: f64| v));
    }
}
