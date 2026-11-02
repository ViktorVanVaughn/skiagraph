//! Triangles (`Tri`, `STri`, `VCSTri`, vertex-array strips and meshes) together with spheres, tubes
//! and rings (build step 5), in orthographic and perspective view. In perspective a sphere is
//! `persp::sphere` (its outline the exact cone conic), a triangle's depth is the exact distance along
//! the eye ray (the reciprocal of the unnormalised ray parameter is affine in the image, report 14 §6,
//! so equal-depth lines stay exact straight segments), and "in front of a plane" is decided with the
//! real eye.
//!
//! Same exact painter's scheme as `svg.rs`: primitives are drawn back to front and each one removes
//! only the parts where an earlier-drawn primitive is in front of it. A primitive's visible region is
//! found by `geom::region_loops` ("split and classify") from one membership test, `visible`, which
//! compares front depths of the primitive and its occluder candidates. The candidate curves are the
//! outlines of the occluders plus the curves where two surfaces have equal depth:
//! - triangle/triangle: the line where the two planes meet (projected 3D intersection segment), so
//!   crossing triangles are split along their intersection ("split geometry, never shading": every
//!   piece stays tied to its original triangle);
//! - triangle/sphere: the circle where the plane cuts the sphere (an ellipse in the image);
//! - sphere/sphere: the intersection circle (as in `svg.rs`).
//!
//! Exact predicates (`robust::orient2d/orient3d` on the input coordinates) decide which pairs can
//! interact at all: projected interiors overlap, one triangle wholly behind the other's plane,
//! crossing, coplanar. Coplanar overlapping triangles: identical copies by Tachyon's test order, others
//! the later one on top, and the scene is reported as approximate.
//!
//! Seams: a piece that shares a straight edge (or a curved joint with a tube) with a piece drawn *later* is
//! extended by a band of `knobs::band` pixels across that edge, clipped to the later piece's region (geometry,
//! not `clip-path`).
//! The later piece covers the band exactly, so visible outlines stay exact; the band only removes the
//! anti-aliasing hairline between abutting pieces (step-zero seam measurement).

use crate::cyl::{self, Cyl, Ring, Surf};
use crate::geom::{self, Arc, Curve, P2};
use crate::scene::V3;
use crate::svg::{self, Sph, Unsupported, View};
use crate::tachyon::{Prim, Tracer};
use robust::{Coord, Coord3D, orient2d, orient3d};
use std::sync::atomic::{AtomicUsize, Ordering};

/// How far an inner wall seen through a clip plane's cut reaches under its own outer wall (px).
const CUT_BAND: f64 = 1.0;
/// Two curved outlines closer than this fraction of the shape's size everywhere are merged into one.
const CURVE_MERGE: f64 = 1e-3;
fn sub2(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}
fn cross2(a: P2, b: P2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot3(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross3(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit3(a: V3) -> V3 {
    let l = dot3(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}
/// Sign with 0 for 0 (`f64::signum(0.0)` is 1).
fn sgn(x: f64) -> f64 {
    if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }
}
fn c2(p: P2) -> Coord<f64> {
    Coord { x: p[0], y: p[1] }
}
fn c3(p: V3) -> Coord3D<f64> {
    Coord3D { x: p[0], y: p[1], z: p[2] }
}

/// A projected triangle. Vertices are counterclockwise in the `orient2d` sense (interior on the left
/// of each edge, the same convention as `region_loops`).
#[derive(Clone)]
pub struct Tri {
    pub p: [P2; 3],
    pub z: [f64; 3],
    pub w: [V3; 3],
    /// depth gradient over the image (per pixel), so depth(x) = z0 + g·(x − p0)
    g: P2,
    /// unit normal pointing towards the viewer
    nf: V3,
    /// `orient3d(w0, w1, w2, x)` has this sign for points x in front of the plane
    front: f64,
    /// perspective: nf / nf·(w0 − eye), so that 1/λ = inv·D for the plane's hit eye + λD on the
    /// unnormalised eye ray D (`persp::ray`), affine in the image; None in orthographic view
    inv: Option<V3>,
}

impl Tri {
    pub fn new(view: &View, w: [V3; 3]) -> Option<Tri> {
        let mut w = w;
        let mut pz = w.map(|x| view.project(x));
        let o = orient2d(c2(pz[0].0), c2(pz[1].0), c2(pz[2].0));
        if o == 0.0 {
            return None; // seen exactly edge-on: no area
        }
        if o < 0.0 {
            w.swap(1, 2);
            pz.swap(1, 2);
        }
        let p = pz.map(|x| x.0);
        let z = pz.map(|x| x.1);
        let (e1, e2) = (sub2(p[1], p[0]), sub2(p[2], p[0]));
        let det = cross2(e1, e2);
        if det <= 1e-12 * (e1[0] * e1[0] + e1[1] * e1[1] + e2[0] * e2[0] + e2[1] * e2[1]) {
            return None; // a sliver thinner than any renderer can show
        }
        let (dz1, dz2) = (z[1] - z[0], z[2] - z[0]);
        let g = [(dz1 * e2[1] - dz2 * e1[1]) / det, (e1[0] * dz2 - e2[0] * dz1) / det];
        let n = unit3(cross3(sub3(w[1], w[0]), sub3(w[2], w[0])));
        let (nf, eye, inv) = match view.eye {
            None => {
                let nf = if dot3(n, view.v) > 0.0 { [-n[0], -n[1], -n[2]] } else { n };
                let ext = w.iter().flatten().fold(1.0f64, |m, v| m.max(v.abs()));
                (nf, [w[0][0] - view.v[0] * ext, w[0][1] - view.v[1] * ext, w[0][2] - view.v[2] * ext], None)
            }
            Some(e) => {
                let nf = if dot3(n, sub3(e, w[0])) < 0.0 { [-n[0], -n[1], -n[2]] } else { n };
                let h = dot3(nf, sub3(w[0], e));
                if h == 0.0 {
                    return None; // its plane passes through the eye: seen edge-on
                }
                (nf, e, Some(nf.map(|c| c / h)))
            }
        };
        let front = sgn(orient3d(c3(w[0]), c3(w[1]), c3(w[2]), c3(eye)));
        Some(Tri { p, z, w, g, nf, front, inv })
    }
    /// Depth at image point x: orthographic, affine; perspective, the distance along the eye ray.
    pub fn depth(&self, view: &View, x: P2) -> f64 {
        match self.inv {
            None => self.z[0] + self.g[0] * (x[0] - self.p[0][0]) + self.g[1] * (x[1] - self.p[0][1]),
            Some(inv) => {
                let d = crate::persp::ray(view, x);
                dot3(d, d).sqrt() / dot3(inv, d)
            }
        }
    }
    /// Perspective: 1/λ at image point x (larger = nearer), affine in x.
    fn inv_lambda(&self, view: &View, x: P2) -> f64 {
        dot3(self.inv.expect("perspective"), crate::persp::ray(view, x))
    }
    pub fn inside(&self, x: P2) -> bool {
        (0..3).all(|i| cross2(sub2(self.p[(i + 1) % 3], self.p[i]), sub2(x, self.p[i])) > 0.0)
    }
    /// Exact side of a world point: > 0 in front of the plane, < 0 behind, 0 on it.
    fn side(&self, x: V3) -> f64 {
        sgn(orient3d(c3(self.w[0]), c3(self.w[1]), c3(self.w[2]), c3(x))) * self.front
    }
    /// Signed distance of a world point from the plane, positive towards the viewer.
    fn dist_front(&self, x: V3) -> f64 {
        dot3(self.nf, sub3(x, self.w[0]))
    }
    fn edges(&self) -> [Curve; 3] {
        [0, 1, 2].map(|i| seg(self.p[i], self.p[(i + 1) % 3]))
    }
}

/// A segment in canonical orientation, so an edge shared by two triangles is the same curve.
fn seg(a: P2, b: P2) -> Curve {
    if (a[0], a[1]) <= (b[0], b[1]) { Curve::Seg { a, b } } else { Curve::Seg { a: b, b: a } }
}

/// Exact test: do the open projected triangles overlap? (Separating-axis test on the 6 edges.)
fn interiors_overlap(a: &Tri, b: &Tri) -> bool {
    let sep = |s: &Tri, t: &Tri| {
        (0..3).any(|i| t.p.iter().all(|&q| orient2d(c2(s.p[i]), c2(s.p[(i + 1) % 3]), c2(q)) <= 0.0))
    };
    !sep(a, b) && !sep(b, a)
}

#[derive(Clone)]
pub enum Shape {
    /// a sphere's outer wall, or its inner wall seen where a clip plane cuts the outer one away
    S(Sph, Surf),
    T(Tri),
    /// one surface of an open tube (step 4): its outer wall, or its inner wall seen through an end
    C(Cyl, Surf),
    R(Ring),
}

impl Shape {
    fn front(&self, view: &View, x: P2) -> Option<f64> {
        match self {
            Shape::S(s, Surf::Back) if view.eye.is_some() => crate::persp::back(view, s, x),
            Shape::S(s, f) => s.front(view, x).map(|t| if *f == Surf::Back { 2.0 * s.tc - t } else { t }),
            Shape::T(t) => t.inside(x).then(|| t.depth(view, x)),
            Shape::C(c, f) => c.depth(view, x, *f),
            Shape::R(g) => g.depth(view, x),
        }
    }
}

pub struct Item {
    pub prim: usize,
    /// place in Tachyon's test order (`Tracer::tie`): the lower one wins an exact depth tie
    tie: u64,
    pub shape: Shape,
    outline: Vec<Curve>,
    /// clip planes [a, b, c, d] that cut this primitive (`svg::cuts`), and the curves where they
    /// cut its surface (part of its outline)
    cut: Vec<[f64; 4]>,
    cutc: Vec<Curve>,
    pub lo: P2,
    pub hi: P2,
    pub zmin: f64,
    pub zmax: f64,
    pub key: f64,
}

/// Is the surface point at x and depth t kept by the clip planes `cut` (Tachyon drops it where
/// a·x + b·y + c·z > d)?
fn kept(view: &View, cut: &[[f64; 4]], x: P2, t: f64) -> bool {
    kept_by(view, cut, x, t, 0.0)
}

/// `kept` with every plane moved `shift` (world units) into the kept side.
fn kept_by(view: &View, cut: &[[f64; 4]], x: P2, t: f64, shift: f64) -> bool {
    if cut.is_empty() {
        return true;
    }
    let p = view.point(x, t);
    cut.iter().all(|c| c[0] * p[0] + c[1] * p[1] + c[2] * p[2] <= c[3] - shift * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt())
}

/// An inner wall seen through a cut is drawn first and reaches this far (world units: `CUT_BAND` pixels)
/// under its own outer wall, which covers it: the two never just abut, so no background shows through
/// their anti-aliased edges (the cut planes moved by it give the band's exact edge).
fn under_cut(view: &View) -> f64 {
    CUT_BAND * view.psx
}

impl Item {
    pub fn front(&self, view: &View, x: P2) -> Option<f64> {
        self.shape.front(view, x).filter(|&t| kept(view, &self.cut, x, t))
    }
    pub fn size(&self) -> f64 {
        (self.hi[0] - self.lo[0]).max(self.hi[1] - self.lo[1]).max(1e-9)
    }
    fn outline(&self) -> Vec<Curve> {
        let mut o = self.outline.clone();
        o.extend(self.cutc.iter().cloned());
        o
    }
    /// world size, for depth tie tolerances
    pub fn world(&self) -> f64 {
        match &self.shape {
            Shape::S(s, _) => s.rw,
            Shape::C(c, _) => c.r,
            _ => 0.0,
        }
    }
}

#[derive(Clone)]
struct Occ {
    j: usize,
    /// coplanar, overlapping and winning the tie (`pair`): hides the primitive wherever it covers it
    coplanar: bool,
    /// a licorice joint with item k (sphere and tube of one radius touching along a circle)
    lic: bool,
    /// the occluder moved slightly for this item (tube axes snapped to meet exactly)
    alt: Option<Box<Shape>>,
}

impl Occ {
    /// The occluder's shape and its depth at x (None where it does not cover x). Most side probes
    /// miss it, so its box is tested first, padded for `alt`'s shift (under 1e-3 of a tube radius)
    /// and rounding: the answer is the shape's own.
    fn front<'a>(&'a self, view: &View, items: &'a [Item], x: P2) -> Option<(&'a Shape, f64)> {
        let q = &items[self.j];
        let pad = 1e-2 * q.size() + 1e-9;
        if x[0] < q.lo[0] - pad || x[0] > q.hi[0] + pad || x[1] < q.lo[1] - pad || x[1] > q.hi[1] + pad {
            return None;
        }
        let s = self.alt.as_deref().unwrap_or(&q.shape);
        s.front(view, x).filter(|&t| kept(view, &q.cut, x, t)).map(|t| (s, t))
    }
}

/// Is item k painted at x: it covers x and no occluder candidate is in front of it there.
fn visible(view: &View, items: &[Item], k: usize, occ: &[Occ], x: P2) -> bool {
    let Some(tp) = items[k].front(view, x) else { return false };
    // an inner wall shows only through an open end or where a clip plane cuts the outer wall away:
    // under its own outer wall it would share the silhouette, anti-aliased twice
    let outer = match &items[k].shape {
        Shape::C(c, Surf::Back) => Some(c.depth(view, x, Surf::Front)),
        Shape::S(s, Surf::Back) => Some(s.front(view, x)),
        _ => None,
    };
    if outer.is_some_and(|t| t.is_some_and(|t| kept_by(view, &items[k].cut, x, t, under_cut(view)))) {
        return false;
    }
    let own_tube_front = matches!(items[k].shape, Shape::C(_, Surf::Front));
    for o in occ {
        if let Some((q, tq)) = o.front(view, items, x) {
            if o.lic && (tq - tp).abs() < 1e-9 * items[k].world() {
                // licorice joint: depths touch to second order. The tube's outer wall lies outside the
                // ball, so it wins the tie; against the inner wall (silhouette ends) the sphere wins.
                let tube_front = if let Shape::C(_, f) = q { *f == Surf::Front } else { own_tube_front };
                if tube_front == matches!(items[k].shape, Shape::C(..)) {
                    continue;
                }
                return false;
            }
            // exact ties (duplicated primitives): the one Tachyon tests first, as the oracle
            if o.coplanar || tq < tp || (tq == tp && items[o.j].tie < items[k].tie) {
                return false;
            }
        }
    }
    true
}

/// Add a curve unless (nearly) the same curve is already there: shared outlines, a cap's rim, one
/// tube crossing both halves of a bond. Conics closer than `merge` px (or than the side probe,
/// 3 x fuzz) become one curve whose fuzz covers both; segments merge only when identical.
fn push_curve(curves: &mut Vec<Curve>, fuzz: &mut Vec<f64>, c: Curve, f: f64, merge: f64) {
    // pieces of quartic chains are unique and numerous: no search
    if !(f > 0.0 && matches!(c, Curve::Seg { .. })) {
        for (d, g) in curves.iter().zip(fuzz.iter_mut()) {
            let tol = if matches!(c, Curve::Seg { .. }) { 1e-6 * merge } else { merge.max(3.0 * f).max(3.0 * *g) };
            // two segments are at least as far apart as the nearer of r, s from p: most are far,
            // which squared distances show without `curve_gap`'s four square roots
            if let (Curve::Seg { a: p, .. }, Curve::Seg { a: r, b: s }) = (&c, d) {
                let far = |x: &P2| (x[0] - p[0]).powi(2) + (x[1] - p[1]).powi(2) > 1.000001 * tol * tol;
                if far(r) && far(s) {
                    continue;
                }
            }
            let gap = geom::curve_gap(&c, d);
            if gap <= tol {
                *g = g.max(f).max(gap);
                return;
            }
        }
    }
    curves.push(c);
    fuzz.push(f);
}

/// Uniform grid over item bounding boxes.
pub struct Grid {
    cell: f64,
    lo: P2,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    pub fn new(boxes: &[(P2, P2)]) -> Grid {
        if boxes.is_empty() {
            return Grid { cell: 1.0, lo: [0.0, 0.0], nx: 1, ny: 1, cells: vec![vec![]] };
        }
        let lo = boxes.iter().fold([f64::MAX; 2], |m, b| [m[0].min(b.0[0]), m[1].min(b.0[1])]);
        let hi = boxes.iter().fold([f64::MIN; 2], |m, b| [m[0].max(b.1[0]), m[1].max(b.1[1])]);
        let mut sizes: Vec<f64> = boxes.iter().map(|b| (b.1[0] - b.0[0]).max(b.1[1] - b.0[1])).collect();
        let mid = sizes.len() / 2;
        let median = *sizes.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
        let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
        let cell = median.max(extent / 2000.0).max(1e-9);
        let nx = (((hi[0] - lo[0]) / cell).ceil() as usize).max(1);
        let ny = (((hi[1] - lo[1]) / cell).ceil() as usize).max(1);
        let mut g = Grid { cell, lo, nx, ny, cells: vec![vec![]; nx * ny] };
        for (i, b) in boxes.iter().enumerate() {
            let (x0, x1, y0, y1) = g.span(b.0, b.1);
            for gy in y0..=y1 {
                for gx in x0..=x1 {
                    g.cells[gy * nx + gx].push(i as u32);
                }
            }
        }
        g
    }
    fn span(&self, lo: P2, hi: P2) -> (usize, usize, usize, usize) {
        let f = |v: f64, l: f64, n: usize| (((v - l) / self.cell).floor().max(0.0) as usize).min(n - 1);
        (f(lo[0], self.lo[0], self.nx), f(hi[0], self.lo[0], self.nx), f(lo[1], self.lo[1], self.ny), f(hi[1], self.lo[1], self.ny))
    }
    /// Indices of boxes whose cells touch [lo, hi], sorted, without duplicates.
    pub fn query(&self, lo: P2, hi: P2) -> Vec<usize> {
        let (x0, x1, y0, y1) = self.span(lo, hi);
        let mut out = vec![];
        for gy in y0..=y1 {
            for gx in x0..=x1 {
                out.extend(self.cells[gy * self.nx + gx].iter().map(|&i| i as usize));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Projected 3D circle (centre m, unit normal n, radius rho); None in perspective if it reaches the
/// eye plane.
pub fn circle3(view: &View, m: V3, n: V3, rho: f64, thin: f64) -> Option<Curve> {
    if view.eye.is_some() {
        return crate::persp::circle_image(view, m, n, rho, thin);
    }
    let helper = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let e1 = unit3(cross3(n, helper));
    let e2 = cross3(n, e1);
    let (mc, _) = view.project(m);
    let u = view.dir2([e1[0] * rho, e1[1] * rho, e1[2] * rho]);
    let v = view.dir2([e2[0] * rho, e2[1] * rho, e2[2] * rho]);
    Some(Curve::conic_or_seg(mc, u, v, thin))
}

/// The circle where a triangle's plane cuts a sphere, projected (None if they don't meet).
fn plane_circle(view: &View, t: &Tri, s: &Sph) -> Option<Curve> {
    let d = t.dist_front(s.cw);
    if d.abs() >= s.rw {
        return None;
    }
    let m = [s.cw[0] - t.nf[0] * d, s.cw[1] - t.nf[1] * d, s.cw[2] - t.nf[2] * d];
    circle3(view, m, t.nf, (s.rw * s.rw - d * d).sqrt(), 1e-9 * s.rpx.max(1.0))
}

/// Where clip plane pl ([a, b, c, d]: the plane a·x + b·y + c·z = d) cuts a primitive's surface,
/// projected: a circle on a sphere, an ellipse (or two lines) on a tube, a segment on a triangle.
fn cut_curves(view: &View, shape: &Shape, pl: &[f64; 4]) -> Vec<Curve> {
    let l = (pl[0] * pl[0] + pl[1] * pl[1] + pl[2] * pl[2]).sqrt();
    let (n, d) = ([pl[0] / l, pl[1] / l, pl[2] / l], pl[3] / l);
    match shape {
        Shape::S(s, _) => {
            let h = dot3(n, s.cw) - d;
            if h.abs() >= s.rw {
                return vec![];
            }
            let m = [s.cw[0] - n[0] * h, s.cw[1] - n[1] * h, s.cw[2] - n[2] * h];
            let rho = (s.rw * s.rw - h * h).sqrt();
            circle3(view, m, n, rho, 1e-9 * s.rpx.max(1.0)).into_iter().collect()
        }
        Shape::T(t) => {
            let f = t.w.map(|w| dot3(n, w) - d);
            let mut pts: Vec<P2> = vec![];
            for i in 0..3 {
                let j = (i + 1) % 3;
                if f[i] == 0.0 {
                    pts.push(view.project(t.w[i]).0);
                } else if f[i] * f[j] < 0.0 {
                    // from the edge's lower end, so the triangles sharing it get the same point
                    let (i, j) = if t.w[i] < t.w[j] { (i, j) } else { (j, i) };
                    let u = f[i] / (f[i] - f[j]);
                    pts.push(view.project([0, 1, 2].map(|k| t.w[i][k] + u * (t.w[j][k] - t.w[i][k]))).0);
                }
            }
            if pts.len() < 2 || pts[0] == pts[1] {
                return vec![];
            }
            vec![seg(pts[0], pts[1])]
        }
        Shape::C(c, _) => cyl::clip_cut(view, c, n, d),
        Shape::R(_) => vec![], // refused by the caller
    }
}

/// Line where the planes of p and q have equal depth, clipped to p's projection (perspective: where
/// their affine 1/λ are equal, so the line is exact).
fn equal_depth_seg(view: &View, p: &Tri, q: &Tri) -> Option<Curve> {
    let f: Vec<f64> = match p.inv {
        None => (0..3).map(|i| p.z[i] - q.depth(view, p.p[i])).collect(),
        Some(_) => (0..3).map(|i| q.inv_lambda(view, p.p[i]) - p.inv_lambda(view, p.p[i])).collect(),
    };
    let mut pts: Vec<P2> = vec![];
    for i in 0..3 {
        let j = (i + 1) % 3;
        if f[i] == 0.0 {
            pts.push(p.p[i]);
        } else if f[i] * f[j] < 0.0 {
            let s = f[i] / (f[i] - f[j]);
            pts.push([p.p[i][0] + s * (p.p[j][0] - p.p[i][0]), p.p[i][1] + s * (p.p[j][1] - p.p[i][1])]);
        }
    }
    if pts.len() < 2 || geom::dist(pts[0], pts[1]) == 0.0 {
        return None;
    }
    Some(seg(pts[0], pts[1]))
}

/// Can the earlier-drawn item j be in front of item k somewhere? If so: the occluder record and the
/// curves it contributes to k's boundary candidates (its outline plus equal-depth curves).
fn pair(view: &View, items: &[Item], k: usize, j: usize, coplanar: &AtomicUsize, memo: &mut Memo) -> Option<(Occ, Vec<Curve>, f64)> {
    let (p, q) = (&items[k], &items[j]);
    if q.lo[0] >= p.hi[0] || p.lo[0] >= q.hi[0] || q.lo[1] >= p.hi[1] || p.lo[1] >= q.hi[1] || q.zmin > p.zmax || q.prim == p.prim {
        return None;
    }
    let occ = Occ { j, coplanar: false, lic: false, alt: None };
    let tube = |a: &Shape| matches!(a, Shape::C(..) | Shape::R(_));
    if tube(&p.shape) || tube(&q.shape) {
        return tube_pair(view, p, q, occ, memo);
    }
    let (occ, curves) = match (&p.shape, &q.shape) {
        (Shape::T(tp), Shape::T(tq)) => {
            let sq = tq.w.map(|x| tp.side(x)); // q's vertices against p's plane (> 0: in front)
            if sq.iter().all(|&s| s == 0.0) {
                if !interiors_overlap(tp, tq) {
                    return None;
                }
                coplanar.fetch_add(1, Ordering::Relaxed);
                // identical copies tie everywhere, in VMD's numbers too: Tachyon's test order decides. Two
                // different triangles tie only in the scene file's 6 digits; VMD decides with digits it does
                // not write, so the later one is drawn on top (what VMD's own picture shows in the
                // coincident_* test scenes; decisions-pending F4-a)
                let wins = if tq.w == tp.w { q.tie < p.tie } else { q.prim > p.prim };
                return wins.then(|| (Occ { j, coplanar: true, lic: false, alt: None }, q.outline(), 0.0));
            }
            if sq.iter().all(|&s| s <= 0.0) {
                return None; // q wholly behind p's plane
            }
            let sp = tp.w.map(|x| tq.side(x));
            if sp.iter().all(|&s| s >= 0.0) {
                return None; // p wholly in front of q's plane
            }
            if !interiors_overlap(tp, tq) {
                return None;
            }
            let mut curves = q.outline();
            let straddles = |s: &[f64; 3]| s.iter().any(|&x| x > 0.0) && s.iter().any(|&x| x < 0.0);
            if straddles(&sq) && straddles(&sp) {
                curves.extend(equal_depth_seg(view, tp, tq)); // the triangles may cross: split along it
            }
            (occ, curves)
        }
        (Shape::T(tp), Shape::S(sq, _)) => {
            if tp.dist_front(sq.cw) <= -sq.rw {
                return None; // sphere wholly behind p's plane
            }
            let mut curves = q.outline();
            curves.extend(plane_circle(view, tp, sq));
            (occ, curves)
        }
        (Shape::S(sp, _), Shape::T(tq)) => {
            if tq.dist_front(sp.cw) >= sp.rw {
                return None; // sphere p wholly in front of q's plane
            }
            let mut curves = q.outline();
            curves.extend(plane_circle(view, tq, sp));
            (occ, curves)
        }
        (Shape::S(sp, fp), Shape::S(sq, _)) => {
            // p's deepest point: on its outer wall (`front_max`), or the far pole (inner wall)
            let pmax = if *fp == Surf::Back { sp.tc + sp.rw } else { sp.front_max };
            if geom::dist(sp.c, sq.c) >= sp.rpx + sq.rpx || sq.near >= pmax {
                return None;
            }
            let mut curves = q.outline();
            curves.extend(svg::intersection_ellipse(view, sp, sq));
            (occ, curves)
        }
        _ => unreachable!(),
    };
    Some((occ, curves, 0.0))
}

/// The last tube-tube meeting curves of an `occluders` call, by the other tube's primitive.
type Memo = Option<(usize, (Vec<Curve>, f64))>;

/// `pair` when a tube or ring is involved: occluder record, q's outline and the meeting curves (with
/// their fuzz, nonzero for quartic chord chains).
fn tube_pair(view: &View, p: &Item, q: &Item, mut occ: Occ, memo: &mut Memo) -> Option<(Occ, Vec<Curve>, f64)> {
    // a tube whose axis passes within printing precision of p's axis is moved to meet it exactly
    if let (Shape::C(a, _), Shape::C(b, f)) = (&p.shape, &q.shape) {
        occ.alt = cyl::snap_to_meet(a, b).map(|b| Box::new(Shape::C(b, *f)));
    }
    let qs = occ.alt.as_deref().unwrap_or(&q.shape).clone();
    let mut curves = match &qs {
        Shape::C(c, _) => c.outline(view).into_iter().chain(q.cutc.iter().cloned()).collect(),
        _ => q.outline(),
    };
    let (meet, fuzz) = match (&p.shape, &qs) {
        (Shape::S(s, _), Shape::C(c, _)) | (Shape::C(c, _), Shape::S(s, _)) => {
            occ.lic = cyl::licorice_joint(s, c);
            cyl::sphere_cyl(view, s, c)
        }
        // a tube's two walls are neighbours in item order and meet p in the same curves: computed once
        (Shape::C(a, _), Shape::C(b, _)) => match memo {
            Some((prim, r)) if *prim == q.prim => r.clone(),
            _ => memo.insert((q.prim, cyl::cyl_cyl(view, a, b))).1.clone(),
        },
        (Shape::T(t), Shape::C(c, _)) | (Shape::C(c, _), Shape::T(t)) => cyl::plane_cut(view, c, t.w[0], t.nf),
        (Shape::R(g), Shape::S(s, _)) | (Shape::S(s, _), Shape::R(g)) => (cyl::ring_sphere(view, g, s), 0.0),
        (Shape::R(g), Shape::C(c, _)) | (Shape::C(c, _), Shape::R(g)) => cyl::ring_cyl(view, g, c),
        // ponytail: a ring crossing a triangle or another ring (a line segment) is not cut yet
        _ => (vec![], 0.0),
    };
    curves.extend(meet);
    Some((occ, curves, fuzz))
}

/// Occluder candidates of item k (only earlier-drawn ones unless `all`), leaving out the items `skip`
/// names (culled ones, or those the oracle does not see near k), and the curves they add to k's outline.
fn occluders(view: &View, items: &[Item], grid: &Grid, k: usize, all: bool, skip: &dyn Fn(usize) -> bool, coplanar: &AtomicUsize) -> (Vec<Occ>, Vec<Curve>, Vec<f64>) {
    let p = &items[k];
    let (mut occ, mut curves, mut fuzz) = (vec![], vec![], vec![]);
    // ponytail: conics closer than 1e-3 of the item's size everywhere are merged (with that fuzz); a
    // sliver thinner than that between them is not resolved
    let merge = CURVE_MERGE * match &p.shape { Shape::C(c, _) => c.rpx, _ => p.size() };
    // a clip plane's cut is exact and is merged only with the same curve: near the outline it bounds a
    // real sliver (an atom cut close to its centre by a plane facing the viewer leaves a ring 0.04 px wide)
    let cut_merge = 1e-9 * p.size();
    for c in p.outline() {
        let m = if p.cutc.contains(&c) { cut_merge } else { merge };
        push_curve(&mut curves, &mut fuzz, c, 0.0, m);
    }
    let mut memo: Memo = None;
    for j in grid.query(p.lo, p.hi) {
        // triangles are cut by every triangle in front (later ones too): at a fold the hidden back
        // triangle must not share the outline, or its anti-aliasing doubles the coverage there
        let both_tri = matches!(p.shape, Shape::T(_)) && matches!(items[j].shape, Shape::T(_));
        // so is an atom by the later tubes of its licorice joints: hidden inside them, its paint showed only
        // in the anti-aliased pixels where two tubes meet over it (SM2: dark lines with shadows and AO)
        let lic = matches!((&p.shape, &items[j].shape), (Shape::S(s, Surf::Front), Shape::C(c, Surf::Front)) if cyl::licorice_joint(s, c));
        if j == k || (!all && !both_tri && !lic && j > k) || skip(j) {
            continue;
        }
        if let Some((o, cs, f)) = pair(view, items, k, j, coplanar, &mut memo) {
            occ.push(o);
            let both_tri = matches!(p.shape, Shape::T(_)) && matches!(items[j].shape, Shape::T(_));
            for (i, c) in cs.into_iter().enumerate() {
                if both_tri {
                    if !curves.contains(&c) {
                        curves.push(c);
                        fuzz.push(0.0);
                    }
                } else {
                    // the occluder's own outline comes first and is exact
                    let m = if items[j].cutc.contains(&c) { cut_merge } else { merge };
                    push_curve(&mut curves, &mut fuzz, c, if i < items[j].outline.len() + items[j].cutc.len() { 0.0 } else { f }, m);
                }
            }
        }
    }
    (occ, curves, fuzz)
}

/// Item k's truly visible region: cut by every item in front of it, drawn earlier or later (`skip`: items
/// left out as occluders), as (curves, loops); None where it shows nowhere. Regions found this way never
/// overlap, so they can be drawn in any order (the flat style, `flat.rs`).
pub fn visible_region(view: &View, items: &[Item], grid: &Grid, k: usize, skip: &dyn Fn(usize) -> bool) -> Option<(Vec<Curve>, Vec<Vec<Arc>>)> {
    let (occ, curves, fuzz) = occluders(view, items, grid, k, true, skip, &AtomicUsize::new(0));
    let loops = geom::region_loops_fuzzy(&curves, &fuzz, &|x| visible(view, items, k, &occ, x), items[k].size());
    (!loops.is_empty()).then_some((curves, loops))
}

/// Item k's exact visibility (stage 3): its occluders and the curves of its outline, so the drawn light (`ink.rs`) can
/// cut parts of the visible region by its own curves (band edges, shadow edges) without finding the occluders again.
pub struct Vis {
    k: usize,
    occ: Vec<Occ>,
    curves: Vec<Curve>,
    fuzz: Vec<f64>,
}

pub fn visibility(view: &View, items: &[Item], grid: &Grid, k: usize, skip: &dyn Fn(usize) -> bool) -> Vis {
    let (occ, curves, fuzz) = occluders(view, items, grid, k, true, skip, &AtomicUsize::new(0));
    Vis { k, occ, curves, fuzz }
}

impl Vis {
    pub fn visible(&self, view: &View, items: &[Item], x: P2) -> bool {
        visible(view, items, self.k, &self.occ, x)
    }
    /// Nothing cuts this item: its visible region is its whole shape.
    pub fn whole(&self) -> bool {
        self.occ.is_empty()
    }
    /// The part of the visible region where `pred` holds, `extra` holding the curves where `pred` changes.
    pub fn part(&self, view: &View, items: &[Item], extra: &[Curve], pred: &dyn Fn(P2) -> bool) -> Option<(Vec<Curve>, Vec<Vec<Arc>>)> {
        let (mut curves, mut fuzz) = (self.curves.clone(), self.fuzz.clone());
        for c in extra {
            if !curves.contains(c) {
                curves.push(c.clone());
                fuzz.push(0.0);
            }
        }
        let loops = geom::region_loops_fuzzy(&curves, &fuzz, &|x| pred(x) && self.visible(view, items, x), items[self.k].size());
        (!loops.is_empty()).then_some((curves, loops))
    }
}

// Spheres among tubes and triangles sharing their material's stack too (`svg::shade_spheres`): knob
// `share_stacks_mesh`, off for now: Chrome draws a moved stack one code apart at a few pixels, as often nearer
// VMD as further, but on cyl_licorice_ligand one such pixel raises its Chrome max-blur 1.64 -> 1.81, past the
// noise margin (decisions-pending C-stacks-a; on saves 2.4 MB of the matrix and 0.5 MB of each large test view).

// Knob `ao_step` (1.6 px): spacing, along a triangle's longest edge, of the extra ambient-occlusion samples
// inside a triangle: corner values alone miss the AO's curvature across large triangles (measured 2026-10-12,
// docs/measurements/2026-10-12-a1-vertex-ao.md). Knob `ao_sets` (4): Tachyon's jitter sets averaged per AO
// sample at mesh vertices and inside small triangles: one set's AO carries a single pixel's grain (report 10
// section 3.3; measured on fx_cartoon_shadows_ao: max-blur 1.45 -> 1.13, its floor 1.12; 8 sets no better).

// Knobs `tile_err` (2.5 codes): worst fit error above which a triangle's shading is split into tiles (measured:
// finer splitting adds more shared-edge rounding than it removes fit error); `tile_min` (1 px): smallest tile
// altitude; `skirt` (2.5 px): how far the tiles on a triangle's outline carry on outwards (seam band plus a pixel);
// `spot_err` (30 codes), `spot_min` (0.25 px): where a fit misses by more than `spot_err` (a highlight drawn as a bright
// facet on small triangles: QuickSurf, isosurfaces, NewCartoon), tiles go down to `spot_min`: seen 4 times larger (ADR
// 0004's judging) such a facet is plain (F3: 30 keeps the files' growth to a few %; 12 or 20 gained little more for
// 2-3 times the growth; decisions-pending F3-a).

#[cfg(test)] // the visibility test draws regions as polygons
/// A piece's loops as point rings; conic arcs flattened to `chord` px chords (used for hidden band paint,
/// where a chord's tiny sagitta does not matter, and for the region tiles are kept near: with 0.3 px
/// chords within `shade::FLAT` of the loops).
pub fn rings_at(curves: &[Curve], loops: &[Vec<Arc>], chord: f64) -> Vec<Vec<P2>> {
    rings_sag(curves, loops, chord, f64::INFINITY)
}

#[cfg(test)] // the visibility test draws regions as polygons
/// `rings_at`, with curves also cut finely enough that no chord strays more than `sag` px from its arc.
fn rings_sag(curves: &[Curve], loops: &[Vec<Arc>], chord: f64, sag: f64) -> Vec<Vec<P2>> {
    loops
        .iter()
        .map(|lp| {
            let mut pts = vec![];
            for a in lp {
                let c = &curves[a.curve];
                let n = match c {
                    Curve::Seg { .. } => 1,
                    Curve::Hyp { .. } => 4000, // perspective only: no mesh piece has one
                    Curve::Conic { u, v, .. } => {
                        let r = u[0].hypot(u[1]).max(v[0].hypot(v[1]));
                        let h = if sag < r { 2.0 * (1.0 - sag / r).acos() } else { f64::INFINITY };
                        ((r * (a.t1 - a.t0).abs() / chord).max((a.t1 - a.t0).abs() / h).ceil() as usize).clamp(2, 10_000)
                    }
                };
                for i in 0..n {
                    pts.push(c.point(a.t0 + (a.t1 - a.t0) * i as f64 / n as f64));
                }
            }
            pts
        })
        .collect()
}

/// The scene's items (each surface of a primitive a piece can show), back to front (farthest first), the
/// opaque ones first; and how many are opaque. Shared by `build_pieces` and the flat style (`flat.rs`).
pub fn items(tr: &Tracer, view: &View) -> Result<(Vec<Item>, usize), Unsupported> {
    // clip planes: what each primitive keeps (None: cut away whole)
    let cuts: Vec<Option<Vec<[f64; 4]>>> = (0..tr.prims.len()).map(|i| svg::cuts(tr, i)).collect();
    // tubes first: half-bonds that continue each other get one shared end plane (mitre), and caps
    // take their tube's exact axis
    let mut tubes: Vec<Cyl> = tr
        .prims
        .iter()
        .enumerate()
        .filter(|(i, _)| cuts[*i].is_some())
        .filter_map(|(i, po)| if let Prim::FCyl { ctr, axis, rad } = &po.prim { Some(Cyl::new(view, i, *ctr, *axis, *rad)) } else { None })
        .collect();
    cyl::mitre(&mut tubes.iter_mut().collect::<Vec<_>>());
    let mut shapes: Vec<(usize, Shape)> = tubes.iter().flat_map(|c| [Surf::Back, Surf::Front].map(|f| (c.prim, Shape::C(c.clone(), f)))).collect();
    for (i, po) in tr.prims.iter().enumerate() {
        let Some(cut) = &cuts[i] else { continue };
        let shape = match &po.prim {
            Prim::FCyl { .. } => continue,
            Prim::Ring { .. } if !cut.is_empty() => return Err(Unsupported("a clip plane or the image plane cutting rings (disc caps)".into())),
            Prim::Ring { ctr, norm, inner, outer } => {
                let mut g = Ring::new(view, i, *ctr, *norm, *inner, *outer);
                let _ = tubes.iter().any(|c| g.snap_to_cap(view, c));
                Shape::R(g)
            }
            Prim::Sphere { ctr, rad } => {
                let s = match view.eye {
                    None => Sph::ortho(view, i, *ctr, *rad),
                    Some(_) => match crate::persp::sphere(view, i, *ctr, *rad)? {
                        // shortcut: drawn only in scenes of spheres alone (`svg::build_pieces`); here its outline
                        // would have to be cut by the frame too
                        Some(s) if s.cut() => return Err(Unsupported("perspective: a sphere reaching the camera's eye plane among triangles, tubes or clip planes (zoomed in too far)".into())),
                        Some(s) => s,
                        None => continue, // wholly behind the eye
                    },
                };
                // seen through the cut where a plane faces away from the eye ray (Tachyon drops n·x > d:
                // a front point dropped and the back point behind it kept needs n·d < 0; in perspective
                // some ray d of the sphere's cone of rays must give it)
                let faces_away = |c: &[f64; 4]| {
                    let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                    match view.eye {
                        None => c[0] * view.v[0] + c[1] * view.v[1] + c[2] * view.v[2] < -1e-9 * l,
                        Some(e) => {
                            let a = [ctr[0] - e[0], ctr[1] - e[1], ctr[2] - e[2]];
                            let la = dot3(a, a).sqrt();
                            dot3([c[0], c[1], c[2]], a) / (l * la) < rad / la - 1e-9
                        }
                    }
                };
                if cut.iter().any(faces_away) {
                    shapes.push((i, Shape::S(s.clone(), Surf::Back)));
                }
                Shape::S(s, Surf::Front)
            }
            Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => {
                let w = [*v0, [v0[0] + e1[0], v0[1] + e1[1], v0[2] + e1[2]], [v0[0] + e2[0], v0[1] + e2[1], v0[2] + e2[2]]];
                match Tri::new(view, w) {
                    Some(t) => Shape::T(t),
                    None => continue,
                }
            }
            Prim::Plane { .. } => return Err(Unsupported("planes".into())),
        };
        shapes.push((i, shape));
    }
    let mut items = vec![];
    for (i, shape) in shapes {
        let cut = cuts[i].clone().unwrap_or_default();
        let mut cutc: Vec<Curve> = cut.iter().flat_map(|pl| cut_curves(view, &shape, pl)).collect();
        if matches!(shape, Shape::S(_, Surf::Back) | Shape::C(_, Surf::Back)) {
            // where the outer wall's band ends (`under_cut`)
            let moved = cut.iter().map(|c| [c[0], c[1], c[2], c[3] - under_cut(view) * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()]);
            cutc.extend(moved.collect::<Vec<_>>().iter().flat_map(|pl| cut_curves(view, &shape, pl)));
        }
        let outline = match &shape {
            Shape::S(s, _) => vec![s.sil.clone()],
            Shape::T(t) => t.edges().to_vec(),
            Shape::C(c, _) => c.outline(view),
            Shape::R(g) => g.outline(view),
        };
        let bbox = || outline.iter().map(|c| c.bbox()).fold(([f64::MAX; 2], [f64::MIN; 2]), |(l, h), (a, b)| ([l[0].min(a[0]), l[1].min(a[1])], [h[0].max(b[0]), h[1].max(b[1])]));
        let (lo, hi, zmin, zmax, key) = match &shape {
            // depths: the outer wall from the nearest point on, the inner wall beyond the outer one's deepest;
            // an inner wall is drawn as deep as its far pole, under what lies in front of it
            Shape::S(s, f) => ([s.c[0] - s.rpx, s.c[1] - s.rpx], [s.c[0] + s.rpx, s.c[1] + s.rpx], if *f == Surf::Back { s.front_max } else { s.near }, s.tc + s.rw, if *f == Surf::Back { s.tc + s.rw } else { s.tc }),
            Shape::T(t) => (
                [t.p.iter().map(|p| p[0]).fold(f64::MAX, f64::min), t.p.iter().map(|p| p[1]).fold(f64::MAX, f64::min)],
                [t.p.iter().map(|p| p[0]).fold(f64::MIN, f64::max), t.p.iter().map(|p| p[1]).fold(f64::MIN, f64::max)],
                // perspective: the nearest point may lie inside; the plane's distance from the eye bounds it
                t.inv.map_or_else(|| t.z.iter().cloned().fold(f64::MAX, f64::min), |inv| 1.0 / dot3(inv, inv).sqrt()),
                t.z.iter().cloned().fold(f64::MIN, f64::max),
                (t.z[0] + t.z[1] + t.z[2]) / 3.0,
            ),
            Shape::C(c, _) => {
                let (z0, z1) = c.depth_range(view);
                let (lo, hi) = bbox();
                (lo, hi, z0, z1, view.project([0, 1, 2].map(|k| c.b[k] + 0.5 * c.len * c.a[k])).1)
            }
            Shape::R(g) => {
                let (z0, z1) = g.depth_range(view);
                let (lo, hi) = bbox();
                (lo, hi, z0, z1, view.project(g.c).1)
            }
        };
        items.push(Item { prim: i, tie: tr.tie[i], shape, outline, cut, cutc, lo, hi, zmin, zmax, key });
    }
    // back to front (farthest first), ties by primitive id, a tube's or cut sphere's inner wall
    // before its outer wall: deterministic
    let wall = |it: &Item| matches!(it.shape, Shape::C(_, Surf::Front) | Shape::S(_, Surf::Front));
    items.sort_by(|a, b| b.key.total_cmp(&a.key).then(a.prim.cmp(&b.prim)).then(wall(a).cmp(&wall(b))));
    // transparent items hide nothing: the opaque scene is built from the opaque ones alone (the stable
    // sort keeps their order), the transparent ones go on top (glass_pieces)
    let see_through = |it: &Item| (tr.sc.materials[tr.prims[it.prim].tex].opacity as f64) < 1.0 - crate::tachyon::MINCONTRIB;
    items.sort_by_key(|it| see_through(it));
    let all = items;
    let n_opaque = all.iter().filter(|it| !see_through(it)).count();
    Ok((all, n_opaque))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "Begin_Scene\nResolution 200 160\nShader_Mode Medium\nEnd_Shader_Mode\nCamera\n  Projection Orthographic\n  Zoom 0.5\n  Aspectratio 1\n  Antialiasing 0\n  Raydepth 50\n  Center 0 0 -2\n  Viewdir 0 0 2\n  Updir 0 1 0\nEnd_Camera\nDirectional_Light Direction 0.1 -0.1 1 Color 1 1 1\nBackground 1 1 1\n";
    const TEX: &str = "Texture Ambient 0.3 Diffuse 0.6 Specular 0 Opacity 1\n  Phong Plastic 0 Phong_size 2 Color 0.8 0.3 0.3 TexFunc 0\n";

    fn tri(v: [[f64; 3]; 3]) -> String {
        format!("Tri V0 {} {} {} V1 {} {} {} V2 {} {} {}\n{TEX}", v[0][0], v[0][1], v[0][2], v[1][0], v[1][1], v[1][2], v[2][0], v[2][1], v[2][2])
    }

    /// End to end (stage 1, in place of the old renderer's piece checks): at every sample point at least half a pixel
    /// from any outline, the shape whose exact visible region holds the point is the one the eye ray meets first,
    /// for crossing triangles and a sphere cutting through them.
    #[test]
    fn visible_regions_match_eye_rays() {
        let sc = crate::read_tachyon::parse(&format!(
            "{HEAD}{}{}{}Sphere Center 0.2 0.1 0.3 Rad 0.45\n{TEX}End_Scene\n",
            tri([[-1.0, -0.8, 0.0], [0.9, -0.6, 0.6], [0.0, 0.9, -0.4]]),
            tri([[-0.9, 0.7, 0.5], [0.8, 0.8, -0.5], [0.1, -0.9, 0.2]]),
            tri([[-0.6, -0.2, -0.8], [0.7, 0.1, 0.9], [-0.2, 0.6, 0.0]])
        ))
        .unwrap();
        let tr = Tracer::new(&sc);
        let view = View::new(&tr);
        let (all, _) = items(&tr, &view).unwrap();
        let grid = Grid::new(&all.iter().map(|it| (it.lo, it.hi)).collect::<Vec<_>>());
        let rings: Vec<Vec<Vec<P2>>> = (0..all.len())
            .map(|k| visible_region(&view, &all, &grid, k, &|_| false).map_or(vec![], |(c, l)| rings_at(&c, &l, 0.05)))
            .collect();
        let edges = |r: &Vec<P2>| (0..r.len()).map(|i| (r[i], r[(i + 1) % r.len()])).collect::<Vec<_>>();
        // even-odd over all of a region's rings (holes included)
        let inside = |rs: &[Vec<P2>], p: P2| {
            rs.iter().flat_map(edges).filter(|(a, b)| (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])).count() % 2 == 1
        };
        let dist = |(a, b): (P2, P2), p: P2| {
            let (d, w) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
            let t = ((w[0] * d[0] + w[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-12)).clamp(0.0, 1.0);
            (w[0] - t * d[0]).hypot(w[1] - t * d[1])
        };
        let (mut checked, mut bad) = (0, vec![]);
        for i in 0..100 {
            for j in 0..80 {
                let p = [i as f64 * 2.0 + 1.0, j as f64 * 2.0 + 1.0];
                if rings.iter().flatten().flat_map(edges).any(|e| dist(e, p) < 0.5) {
                    continue;
                }
                let (x, y) = view.to_tachyon(p);
                let hit: Vec<usize> = tr.first_opaque(x, y).map(|(q, _)| q as usize).into_iter().collect();
                let drawn: Vec<usize> = (0..all.len()).filter(|&k| inside(&rings[k], p)).map(|k| all[k].prim).collect();
                checked += 1;
                if drawn != hit {
                    bad.push((p, drawn, hit));
                }
            }
        }
        assert!(checked > 5000 && bad.is_empty(), "{checked} points; mismatches: {:?}", &bad[..bad.len().min(5)]);
    }
}
