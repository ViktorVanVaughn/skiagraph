//! Scene -> SVG. Projection, draw order, exact visible regions, output.
//!
//! Drawing scheme (exact painter's order): primitives are drawn back to front, and each one removes
//! only the parts where an *earlier-drawn* primitive is in front of it. Whatever is truly visible at a
//! point is then drawn last there, and every shared edge has the earlier primitive's paint underneath
//! (no anti-aliasing seams).

use crate::geom::{self, Curve, P2};
use crate::scene::{Projection, Scene, V3};
use crate::tachyon::{Prim, Tracer};
use std::fmt::Write as _;

/// How far a drawn Bezier curve may stray from an exact outline arc (px).
const ARC_TOL: f64 = 1e-5;
/// Decimal places of every position in the file, unless a style sets its own.
const DECIMALS: u32 = 4;
/// Path steps written relative to the last point before one is written absolute again.
const REL_RUN: usize = 15;

#[derive(Debug)]
pub struct Unsupported(pub String);

/// Maps world points to SVG pixel coordinates (X right, Y down) and depth along the view.
pub struct View {
    pub r: V3,
    pub u: V3,
    pub v: V3,
    pub ll: V3,
    pub psx: f64,
    pub psy: f64,
    pub xoff: f64,
    pub yoff: f64,
    pub w: u32,
    pub h: u32,
    /// Perspective: the eye position (`ll` is then relative to it). None: orthographic.
    pub eye: Option<V3>,
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add3(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul3(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm3(a: V3) -> f64 {
    dot(a, a).sqrt()
}

impl View {
    pub fn new(tr: &Tracer) -> View {
        let sc = tr.sc;
        let psx = tr.px / sc.hres as f64;
        let psy = tr.py / sc.vres as f64;
        let xoff = View::half_pixel(sc);
        let yoff = sc.vres as f64 - xoff;
        let eye = if sc.camera.projection == Projection::Orthographic { None } else { Some(sc.camera.center) };
        View { r: tr.rightvec, u: tr.upvec, v: tr.viewvec, ll: tr.lowleft, psx, psy, xoff, yoff, w: sc.hres, h: sc.vres, eye }
    }
    /// SVG x of image-plane x = 0. Tachyon samples pixel i at image-plane x = i (dynamic scheduler: +0.5) or
    /// i + 1 (static: -0.5), i.e. at a pixel's corner, not its centre.
    pub fn half_pixel(sc: &Scene) -> f64 {
        let dynamic = sc.ao.as_ref().map(|a| a.samples > 0).unwrap_or(false) || sc.camera.antialiasing > 4;
        if dynamic { 0.5 } else { -0.5 }
    }
    /// SVG position and depth. Orthographic: depth = distance from the image plane along the view.
    /// Perspective: the point is first moved along its eye ray onto the image plane (distance 1);
    /// depth = distance from the eye (Tachyon's ray t).
    pub fn project(&self, p: V3) -> (P2, f64) {
        let (p, depth) = match self.eye {
            None => (p, None),
            Some(e) => {
                let q = sub3(p, e);
                (mul3(q, 1.0 / dot(q, self.v)), Some(norm3(q)))
            }
        };
        let d = sub3(p, self.ll);
        let x = dot(d, self.r) / self.psx;
        let y = dot(d, self.u) / self.psy;
        ([x + self.xoff, self.yoff - y], depth.unwrap_or_else(|| dot(d, self.v)))
    }
    /// Tachyon image-plane coordinates of an SVG point (for the oracle).
    pub fn to_tachyon(&self, p: P2) -> (f64, f64) {
        (p[0] - self.xoff, self.yoff - p[1])
    }
    /// World origin of the eye ray through SVG point p (orthographic: on the image plane).
    pub fn origin(&self, p: P2) -> V3 {
        let (x, y) = ((p[0] - self.xoff) * self.psx, (self.yoff - p[1]) * self.psy);
        add3(self.ll, add3(mul3(self.r, x), mul3(self.u, y)))
    }
    /// SVG displacement for a world direction lying in the image plane.
    pub fn dir2(&self, d: V3) -> P2 {
        [dot(d, self.r) / self.psx, -dot(d, self.u) / self.psy]
    }
    /// World point at SVG point p and depth (`project`'s inverse).
    pub fn point(&self, p: P2, depth: f64) -> V3 {
        match self.eye {
            None => add3(self.origin(p), mul3(self.v, depth)),
            Some(e) => {
                let d = crate::persp::ray(self, p);
                add3(e, mul3(d, depth / norm3(d)))
            }
        }
    }
    /// Tachyon's camera ray through SVG point p: (origin, unit direction). Orthographic: from the image
    /// plane along the view; perspective: from the eye.
    pub fn ray(&self, p: P2) -> (V3, V3) {
        match self.eye {
            None => (self.origin(p), self.v),
            Some(e) => {
                let d = crate::persp::ray(self, p);
                (e, mul3(d, 1.0 / norm3(d)))
            }
        }
    }
    /// The direction of the eye ray reaching world point x (perspective: not normalised).
    pub fn sight(&self, x: V3) -> V3 {
        self.eye.map_or(self.v, |e| sub3(x, e))
    }
}

#[derive(Clone)]
pub struct Sph {
    #[allow(dead_code)] // which primitive it came from: read only when debugging
    pub prim: usize,
    pub c: P2,      // SVG centre
    pub rpx: f64,   // radius in pixels
    pub tc: f64,    // depth of the centre
    pub rw: f64,    // radius in world units
    pub cw: V3,     // world centre
    // in perspective, `c` and `rpx` are only a bounding circle of `sil`
    pub sil: Curve,     // exact outline (circle, or ellipse in perspective; `cut`: a hyperbola branch or a side of the frame)
    pub near: f64,      // smallest depth of any point of the sphere
    pub front_max: f64, // largest depth of any visible point
}

impl Sph {
    /// Sphere `prim` in orthographic view (its outline a circle).
    pub fn ortho(view: &View, prim: usize, ctr: V3, rad: f64) -> Sph {
        let (c, tc) = view.project(ctr);
        let rpx = rad / view.psx;
        Sph { prim, c, rpx, tc, rw: rad, cw: ctr, sil: Curve::circle(c, rpx), near: tc - rad, front_max: tc }
    }
    /// Perspective: the eye lies inside it or it reaches behind the eye (`persp::sphere`); its outline is
    /// no ellipse and it ends at the frame (`persp::frame`). Orthographic spheres are never cut here (the
    /// image plane cuts them in the mesh module, `cuts`).
    pub fn cut(&self) -> bool {
        !matches!(self.sil, Curve::Conic { .. })
    }
    /// Depth of the visible surface at SVG point p (None outside the outline).
    pub fn front(&self, view: &View, p: P2) -> Option<f64> {
        if view.eye.is_some() {
            return crate::persp::front(view, self, p);
        }
        self.front_ortho(p)
    }
    pub fn front_ortho(&self, p: P2) -> Option<f64> {
        let d2 = ((p[0] - self.c[0]).powi(2) + (p[1] - self.c[1]).powi(2)) / (self.rpx * self.rpx);
        if d2 >= 1.0 { None } else { Some(self.tc - self.rw * (1.0 - d2).sqrt()) }
    }
}

/// What the visibility code cannot draw, whatever the shading (also the flat style's checks). Returns
/// whether a clip plane (or the image plane) cuts the scene.
pub fn geometry_checks(tr: &Tracer, view: &View) -> Result<bool, Unsupported> {
    let sc = tr.sc;
    if sc.camera.projection == Projection::PerspectiveDof {
        return Err(Unsupported("depth of field".into()));
    }
    if (tr.px / sc.hres as f64 - tr.py / sc.vres as f64).abs() > 1e-12 * tr.px {
        return Err(Unsupported("non-square pixels".into()));
    }
    cut_scene(tr, view)?;
    // clip planes: the mesh module cuts every kind of primitive (`mesh::Item::cut`)
    let clipped = (0..tr.prims.len()).any(|i| cuts(tr, i).is_none_or(|c| !c.is_empty()));
    // U3 drew cuts in perspective on spheres alone and on triangles alone; tubes and spheres among
    // triangles in perspective came with U1, and their cuts are not checked yet
    let has = |f: &dyn Fn(&Prim) -> bool| tr.prims.iter().any(|p| f(&p.prim));
    if clipped && view.eye.is_some() {
        if has(&|p| matches!(p, Prim::FCyl { .. } | Prim::Ring { .. })) {
            return Err(Unsupported("perspective: clip planes cutting tubes".into()));
        }
        if has(&|p| matches!(p, Prim::Sphere { .. })) && has(&|p| matches!(p, Prim::Tri { .. } | Prim::STri { .. })) {
            return Err(Unsupported("perspective: clip planes cutting spheres among triangles".into()));
        }
    }
    Ok(clipped)
}

/// Scenes Tachyon cuts and the converter does not (report 14 §1.2): refused rather than drawn uncut.
fn cut_scene(tr: &Tracer, view: &View) -> Result<(), Unsupported> {
    for p in &tr.prims {
        if matches!(p.prim, Prim::Plane { .. }) {
            continue; // refused on their own
        }
        // Tachyon's rays start at the eye in perspective: whatever reaches the eye plane is cut open there
        // (an outline through the eye plane is no ellipse). Perspective spheres are checked in
        // `persp::sphere`; the orthographic image plane cuts like a clip plane (`cuts`).
        if let (Some(e), false) = (view.eye, matches!(p.prim, Prim::Sphere { .. })) {
            if extent(&p.prim, view.v).0 <= dot(e, view.v) {
                return Err(Unsupported("geometry reaching the camera's eye plane (zoomed in too far)".into()));
            }
        }
    }
    Ok(())
}

/// The clip planes [a, b, c, d] of primitive i that cut it (Tachyon drops a hit where a·x + b·y + c·z
/// > d): empty if none does, None if one cuts it away whole. In orthographic view the image plane is
/// one more for every primitive: Tachyon's rays start on it, so what lies in front of it is dropped.
pub fn cuts(tr: &Tracer, i: usize) -> Option<Vec<[f64; 4]>> {
    let p = &tr.prims[i];
    let mut out = vec![];
    let v = tr.viewvec;
    let image = (tr.sc.camera.projection == Projection::Orthographic).then(|| [-v[0], -v[1], -v[2], -dot(tr.lowleft, v)]);
    let planes = p.clip.map_or(&[][..], |ci| &tr.sc.clips[ci][..]).iter().map(|pl| pl.map(|v| v as f64));
    for pl in planes.chain(image) {
        let (lo, hi) = extent(&p.prim, [pl[0], pl[1], pl[2]]);
        if lo > pl[3] {
            return None;
        }
        if hi > pl[3] {
            out.push(pl);
        }
    }
    Some(out)
}

/// Smallest and largest n·p over a primitive. shortcut: cylinders and rings are widened by their full
/// radius in every direction, so a primitive within a radius of a plane can be refused for nothing.
fn extent(p: &Prim, n: V3) -> (f64, f64) {
    let (pts, r) = match p {
        Prim::Sphere { ctr, rad } => (vec![*ctr], *rad),
        Prim::FCyl { ctr, axis, rad } => (vec![*ctr, add3(*ctr, *axis)], *rad),
        Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => (vec![*v0, add3(*v0, *e1), add3(*v0, *e2)], 0.0),
        Prim::Ring { ctr, outer, .. } => (vec![*ctr], *outer),
        Prim::Plane { .. } => return (f64::NEG_INFINITY, f64::INFINITY),
    };
    let r = r * norm3(n);
    let d: Vec<f64> = pts.iter().map(|q| dot(*q, n)).collect();
    (d.iter().cloned().fold(f64::INFINITY, f64::min) - r, d.iter().cloned().fold(f64::NEG_INFINITY, f64::max) + r)
}

/// Projected intersection circle of two spheres (None if the spheres do not intersect).
pub fn intersection_ellipse(view: &View, a: &Sph, b: &Sph) -> Option<Curve> {
    let dv = sub3(b.cw, a.cw);
    let d = norm3(dv);
    if d <= (a.rw - b.rw).abs() || d >= a.rw + b.rw || d == 0.0 {
        return None;
    }
    let n = mul3(dv, 1.0 / d);
    let t = (d * d + a.rw * a.rw - b.rw * b.rw) / (2.0 * d);
    let rho = (a.rw * a.rw - t * t).max(0.0).sqrt();
    let m = add3(a.cw, mul3(n, t));
    if view.eye.is_some() {
        return crate::persp::circle_image(view, m, n, rho, 1e-9 * a.rpx.max(1.0));
    }
    // orthonormal basis of the circle's plane
    let helper = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let e1 = {
        let c = [n[1] * helper[2] - n[2] * helper[1], n[2] * helper[0] - n[0] * helper[2], n[0] * helper[1] - n[1] * helper[0]];
        mul3(c, 1.0 / norm3(c))
    };
    let e2 = [n[1] * e1[2] - n[2] * e1[1], n[2] * e1[0] - n[0] * e1[2], n[0] * e1[1] - n[1] * e1[0]];
    let (mc, _) = view.project(m);
    let u = view.dir2(mul3(e1, rho));
    let v = view.dir2(mul3(e2, rho));
    Some(Curve::conic_or_seg(mc, u, v, 1e-9 * a.rpx.max(1.0)))
}

pub fn path_data(curves: &[Curve], loops: &[Vec<geom::Arc>]) -> String {
    let mut d = PathData::default();
    for lp in loops {
        for (i, a) in lp.iter().enumerate() {
            let c = &curves[a.curve];
            let (p0, cs) = geom::arc_cubics(c, a.t0, a.t1, ARC_TOL);
            if i == 0 {
                d.m(p0);
            }
            for c in cs {
                if let Curve::Seg { .. } = curves[a.curve] {
                    d.l(c[2]);
                } else {
                    d.c(c[0], c[1], c[2]);
                }
            }
        }
        d.z();
    }
    d.finish()
}

/// Path data as it is written (svg-size.md 4.2): coordinates at 4 decimals (`num`), a command
/// letter only where the command changes (an `L` after `M` is implicit) and no separator a reader does
/// not need (none before a `-`, none between `.5` and `.25`); `H`/`V` for a step along an axis, `S` for a
/// cubic whose first control point is the previous one mirrored, and a last straight step back to the
/// start left to `Z` (svg-size.md idea 8). Each step is written relative to the current point (`l`, `c`,
/// ...) when that is shorter (a straight step only where Chrome reads its corner to exactly the floats
/// of the absolute numbers), but no coordinate is the sum of more than 15 relative steps since it was
/// last written absolute, and every `M` is absolute (svg-size.md idea 6, owner's B2-size-a). Every test
/// is on the rounded numbers and the deltas are differences of rounded numbers, so a reader adding them
/// up exactly gets the same numbers as from the absolute long form; Chrome gets the corners of straight
/// steps to the bit; a reader adding the steps up in 32-bit floats is off by at most 16 roundings plus
/// 15 of the deltas, however long the outline (test `relative_steps_cannot_drift`).
#[derive(Default)]
pub struct PathData {
    d: String,
    /// the command the next numbers continue (0 at the start and after `Z`)
    cmd: u8,
    /// the last thing written was a number: whether it has a point
    dot: Option<bool>,
    /// current point and the subpath's start, in units of the 4th decimal
    cur: [i64; 2],
    start: [i64; 2],
    /// the last segment's second control point, when it was a cubic
    c2: Option<[i64; 2]>,
    /// a straight step not written yet (left out if a `Z` back to the start follows)
    line: Option<[i64; 2]>,
    /// per axis: relative steps added to the current point's x (y) since it was last written absolute
    rel: [u32; 2],
    /// the current point as Chrome holds it (its own float32 reading, see `chrome_f32`)
    chrome: [f32; 2],
}

fn units2(p: P2) -> [i64; 2] {
    let d = decimals();
    [units(p[0], d), units(p[1], d)]
}

/// Decimal places of positions set by a style (`flat::Spec::dec`); u32::MAX: the knob's.
pub static DEC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

/// Decimal places of positions (`DECIMALS`, or a style's) and units per px.
fn decimals() -> u32 {
    match DEC.load(std::sync::atomic::Ordering::Relaxed) {
        u32::MAX => DECIMALS,
        d => d,
    }
}
fn per_px() -> i64 {
    10i64.pow(decimals())
}

impl PathData {
    pub fn m(&mut self, p: P2) {
        self.flush();
        let q = units2(p);
        self.letter(b'M');
        self.n(q[0]);
        self.n(q[1]);
        (self.cur, self.start, self.c2, self.rel) = (q, q, None, [0; 2]);
        self.chrome = q.map(chrome_f32);
    }

    pub fn l(&mut self, p: P2) {
        self.flush();
        self.line = Some(units2(p));
    }

    pub fn c(&mut self, c1: P2, c2: P2, p: P2) {
        self.flush();
        let (a, b, e) = (units2(c1), units2(c2), units2(p));
        // a reader takes `S`'s first control point as the previous one mirrored (the current point
        // after a non-cubic)
        let mirror = self.c2.map_or(self.cur, |k| [2 * self.cur[0] - k[0], 2 * self.cur[1] - k[1]]);
        if a == mirror {
            self.step(b'S', &[b[0], b[1], e[0], e[1]]);
        } else {
            self.step(b'C', &[a[0], a[1], b[0], b[1], e[0], e[1]]);
        }
        (self.cur, self.c2) = (e, Some(b));
    }

    pub fn z(&mut self) {
        if self.line == Some(self.start) {
            self.line = None; // `Z` draws it
        }
        self.flush();
        self.d.push('Z');
        (self.cmd, self.dot, self.cur, self.c2, self.rel) = (0, None, self.start, None, [0; 2]);
        self.chrome = self.start.map(chrome_f32);
    }

    fn flush(&mut self) {
        let Some(t) = self.line.take() else { return };
        if t[1] == self.cur[1] {
            self.step(b'H', &[t[0]]); // also a step of length 0
        } else if t[0] == self.cur[0] {
            self.step(b'V', &[t[1]]);
        } else {
            self.step(b'L', &t);
        }
        (self.cur, self.c2) = (t, None);
    }

    pub fn finish(mut self) -> String {
        self.flush();
        self.d
    }
}

impl PathData {
    /// One step: absolute command `c` with the numbers `abs`, or the same step relative (lower-case
    /// letter, numbers less the current point) when that is shorter (ties stay absolute); for a straight
    /// step (`L`, `H`, `V`) only when Chrome reads its corner to exactly the floats it reads from the
    /// absolute numbers (neighbouring triangles share corners, and Chrome's anti-aliasing shows a
    /// one-float-step mismatch there; curves are never shared exactly, each piece cuts its own); and only
    /// while the axes it moves (x for `H`, y for `V`, both otherwise) have had fewer than 15 relative
    /// steps since they were last written absolute: no coordinate is the sum of more than 15.
    fn step(&mut self, c: u8, abs: &[i64]) {
        let axes: &[usize] = match c {
            b'H' => &[0],
            b'V' => &[1],
            _ => &[0, 1],
        };
        let mut rel = [0; 6];
        for (k, (r, v)) in rel.iter_mut().zip(abs).enumerate() {
            *r = v - self.cur[axes[k % axes.len()]];
        }
        let rel = &rel[..abs.len()];
        // where Chrome puts the end point from the relative and from the absolute numbers
        let (mut via_rel, mut via_abs, end) = (self.chrome, self.chrome, abs.len() - axes.len());
        for (j, &a) in axes.iter().enumerate() {
            via_rel[a] = self.chrome[a] + chrome_f32(rel[end + j]);
            via_abs[a] = chrome_f32(abs[end + j]);
        }
        let exact = matches!(c, b'C' | b'S') || via_rel == via_abs;
        let relative = exact && axes.iter().all(|&a| (self.rel[a] as usize) < REL_RUN) && self.cost(c + 32, rel) < self.cost(c, abs);
        self.chrome = if relative { via_rel } else { via_abs };
        for &a in axes {
            self.rel[a] = if relative { self.rel[a] + 1 } else { 0 };
        }
        let (c, nums) = if relative { (c + 32, rel) } else { (c, abs) };
        self.letter(c);
        for &v in nums {
            self.n(v);
        }
    }

    /// Whether command c needs its letter: numbers after `M` continue as `L`; a new `M` is always written.
    fn explicit(&self, c: u8) -> bool {
        c == b'M' || !(c == self.cmd || (c == b'L' && self.cmd == b'M'))
    }

    /// The bytes `letter(c)` and `n` of each number would add.
    fn cost(&self, c: u8, nums: &[i64]) -> usize {
        let explicit = self.explicit(c);
        let (mut bytes, mut dot) = (explicit as usize, if explicit { None } else { self.dot });
        for &u in nums {
            bytes += space(dot, u) as usize + units_len(u);
            dot = Some(u % per_px() != 0);
        }
        bytes
    }

    fn letter(&mut self, c: u8) {
        if self.explicit(c) {
            self.d.push(c as char);
            self.dot = None;
        }
        self.cmd = c;
    }

    fn n(&mut self, u: i64) {
        if space(self.dot, u) {
            self.d.push(' ');
        }
        write_units(&mut self.d, u, decimals());
        self.dot = Some(u % per_px() != 0);
    }
}

/// Whether a reader needs a space between the last number written (`dot`: whether it has a point; `None`
/// after a letter) and u (in units of the last decimal).
fn space(dot: Option<bool>, u: i64) -> bool {
    let starts_with_point = u > -per_px() && u < per_px() && u != 0;
    dot.is_some_and(|dot| !(u < 0 || (starts_with_point && dot)))
}

/// The float Chrome reads from `write_units(u, decimals())`'s text: Blink's `GenericParseNumber` works in float32,
/// the integer digits right to left, then each decimal digit times `frac *= 0.1f`, so small numbers are
/// often not the nearest float (measured: 21 % of 4-decimal numbers below 4, none above 512).
fn chrome_f32(u: i64) -> f32 {
    let pp = per_px() as u64;
    let (mut i, f) = (u.unsigned_abs() / pp, u.unsigned_abs() % pp);
    let (mut integer, mut mult) = (0f32, 1f32);
    while i > 0 {
        integer += mult * (i % 10) as f32;
        mult *= 10.0;
        i /= 10;
    }
    let (mut decimal, mut frac) = (0f32, 1f32);
    let mut p = pp / 10;
    while p > 0 {
        frac *= 0.1;
        decimal += (f / p % 10) as f32 * frac; // trailing zeros, not written, add 0
        p /= 10;
    }
    if u < 0 { -(integer + decimal) } else { integer + decimal }
}

/// The length of `write_units(u, decimals())`'s text.
fn units_len(u: i64) -> usize {
    let pp = per_px() as u64;
    let (i, mut f) = (u.unsigned_abs() / pp, u.unsigned_abs() % pp);
    let mut n = (u < 0) as usize;
    if i > 0 || f == 0 {
        n += i.checked_ilog10().map_or(1, |l| l as usize + 1);
    }
    if f > 0 {
        n += 1 + decimals() as usize; // the point and the digits, less the trailing zeros
        while f % 10 == 0 {
            (f, n) = (f / 10, n - 1);
        }
    }
    n
}

/// v in units of its last decimal, rounded as `format!("{:.N}")` rounds v.
fn units(v: f64, decimals: u32) -> i64 {
    let x = v * 10f64.powi(decimals as i32);
    if !x.is_finite() {
        return 0; // shortcut: never seen in an output (it used to be written as NaN); find its source if it is
    }
    let r = x.round();
    if ((x - r).abs() - 0.5).abs() > 1e-6 {
        return r as i64;
    }
    // within a hair of a tie the product may have rounded across it: decide on v's exact value
    format!("{:.*}", decimals as usize, v).replace('.', "").parse().unwrap()
}

/// The shortest text of u units of the last of `decimals` decimals: no leading zero (`.5`, `-.5`), no
/// trailing zeros, no `-0`.
fn write_units(out: &mut String, u: i64, decimals: u32) {
    let p = 10u64.pow(decimals);
    let (i, mut f) = (u.unsigned_abs() / p, u.unsigned_abs() % p);
    if u < 0 {
        out.push('-');
    }
    if i > 0 || f == 0 {
        let _ = write!(out, "{i}");
    }
    if f > 0 {
        let mut w = decimals as usize;
        while f % 10 == 0 {
            (f, w) = (f / 10, w - 1);
        }
        let _ = write!(out, ".{f:0w$}");
    }
}

/// A position or length in px: 4 decimals (ADR 0003, owner 2026-10-13: rounding moves an edge by at most
/// 0.00005 px, 0.05 px at 1000× zoom), trailing zeros trimmed.
pub fn num(v: f64) -> String {
    fixed(v, decimals())
}

/// A fraction of a gradient's length (stop offset) or an opacity: 5 decimals, because the error is
/// multiplied by the gradient's length (up to ~1000 px) where the stops crowd at a sphere's rim
/// (svg-size.md §2).
pub fn frac(v: f64) -> String {
    fixed(v, 5)
}

fn fixed(v: f64, decimals: u32) -> String {
    let mut s = String::new();
    write_units(&mut s, units(v, decimals), decimals);
    s
}

/// A group id from a rep label, unique among the group ids given so far (`used`; the other ids in the
/// file are short generated ones a label cannot match). Searching the SVG text instead was quadratic:
/// over 40 minutes on a large test scene's thousands of groups.
pub fn xml_id(label: &str, used: &mut std::collections::HashSet<String>) -> String {
    let base: String = label.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let mut id = base.clone();
    let mut k = 1;
    while used.contains(&id) {
        k += 1;
        id = format!("{base}_{k}");
    }
    used.insert(id.clone());
    id
}

#[cfg(test)]
mod tests {
    

    /// Relative steps cannot make an outline drift (owner's B2-size-a). One outline of 4,000 lines and
    /// cubics, three turns of radius 690 px around (1300, 1000) (coordinates up to 2,000 px, like the
    /// acceptance scenes): (1) a reader adding the steps up exactly gets back every rounded point, so the
    /// file itself does not drift; (2) no coordinate is the sum of more than 15 relative steps (an
    /// absolute `H` resets x only); (3) a reader parsing each number to the nearest 32-bit float and
    /// adding the steps up in 32-bit floats is off in each coordinate by at most half a float step for
    /// its last absolute value plus, for each of the k relative steps since, half a step for the delta
    /// and half a step for the sum: (1 + k) h + k h_d, at most 16 h + 15 h_d (h: half the float step at
    /// the largest coordinate, 0.00006 px below 2,048 px, 0.06 px at 1000× zoom; h_d: at the largest
    /// delta); (4) the same outline with every step relative drifts further than that bound; (5) Chrome
    /// (numbers parsed as Blink parses them, digit by digit in 32-bit floats, steps added in 32-bit
    /// floats) reads every corner of a straight step to exactly the floats it reads from the absolute
    /// numbers.
    #[test]
    fn relative_steps_cannot_drift() {
        use super::{units2, PathData, P2};
        let n = 4000;
        let p = |k: usize| {
            let a = 3.0 * std::f64::consts::TAU * k as f64 / n as f64;
            [1300.0 + 690.0 * a.cos() + 0.0371 * (k % 7) as f64, 1000.0 + 690.0 * a.sin()]
        };
        let (mut d, mut want) = (PathData::default(), vec![vec![units2(p(0))]]);
        d.m(p(0));
        for k in 1..n {
            let (a, b) = (p(k - 1), p(k));
            if k % 3 == 0 {
                let c1: P2 = [a[0] + 0.3 * (b[0] - a[0]), a[1] + 0.4 * (b[1] - a[1])];
                let c2: P2 = [a[0] + 0.7 * (b[0] - a[0]), a[1] + 0.6 * (b[1] - a[1])];
                d.c(c1, c2, b);
                want.push(vec![units2(c1), units2(c2), units2(b)]);
            } else {
                let b = if k % 10 == 0 { [b[0], a[1]] } else { b }; // some `H`
                d.l(b);
                want.push(vec![units2(b)]);
            }
        }
        let s = d.finish();
        // the tokens: letters and numbers (".5.25" is two numbers)
        let (by, mut toks, mut i) = (s.as_bytes(), vec![], 0);
        while i < by.len() {
            let start = i;
            i += 1;
            if by[start] == b' ' {
                continue;
            }
            if !by[start].is_ascii_alphabetic() {
                let mut dot = by[start] == b'.';
                while i < by.len() && (by[i].is_ascii_digit() || (by[i] == b'.' && !dot)) {
                    dot |= by[i] == b'.';
                    i += 1;
                }
            }
            toks.push(&s[start..i]);
        }
        let unit = |t: &str| (t.parse::<f64>().unwrap() * 1e4).round() as i64;
        let off = |e: [i64; 2], f: [f32; 2]| (0..2).map(|a| (f[a] as f64 - e[a] as f64 / 1e4).abs()).fold(0.0, f64::max);
        let half_step = |v: f64| 2f64.powf(v.abs().max(1e-30).log2().floor() - 24.0);
        // Blink's GenericParseNumber: float32, integer digits right to left, then decimals with frac *= 0.1f
        let blink = |t: &str| {
            let (neg, t) = t.strip_prefix('-').map_or((false, t), |r| (true, r));
            let (ip, fp) = t.split_once('.').unwrap_or((t, ""));
            let (mut int, mut mult, mut dec, mut frac) = (0f32, 1f32, 0f32, 1f32);
            for c in ip.bytes().rev() {
                int += mult * (c - b'0') as f32;
                mult *= 10.0;
            }
            for c in fp.bytes() {
                frac *= 0.1;
                dec += (c - b'0') as f32 * frac;
            }
            if neg { -(int + dec) } else { int + dec }
        };
        let (mut cb, mut chrome_off) = ([0f32; 2], 0);
        let (mut cur, mut cf, mut c2) = ([0i64; 2], [0f32; 2], [0i64; 2]);
        // k: per axis, relative steps added up since the coordinate was last read absolute
        let (mut cmd, mut k, mut got, mut seen) = (b'M', [0usize; 2], vec![], vec![]);
        let (mut max_k, mut max_coord, mut max_delta) = (0, 0f64, 0f64);
        // the endpoints written every one relative, added up in floats: (exact, float, largest offset)
        let mut all_rel = (cur, cf, 0f64);
        let mut t = 0;
        while t < toks.len() {
            if toks[t].as_bytes()[0].is_ascii_alphabetic() {
                cmd = toks[t].as_bytes()[0];
                t += 1;
            }
            let (rel, up) = (cmd.is_ascii_lowercase(), cmd.to_ascii_uppercase());
            let (count, axes): (usize, &[usize]) = match up {
                b'M' | b'L' => (2, &[0, 1]),
                b'H' => (1, &[0]),
                b'V' => (1, &[1]),
                b'C' => (6, &[0, 1]),
                b'S' => (4, &[0, 1]),
                _ => panic!("{}", cmd as char),
            };
            let nums = &toks[t..t + count];
            t += count;
            for &a in axes {
                k[a] = if rel { k[a] + 1 } else { 0 };
                max_k = max_k.max(k[a]);
            }
            // the points this step gives, exactly and in 32-bit floats (each relative to the current point)
            let mut pts = vec![];
            let mut g = cb;
            for chunk in nums.chunks(axes.len()) {
                let (mut e, mut f) = (cur, cf);
                for (&a, v) in axes.iter().zip(chunk) {
                    let (de, df) = (unit(v), v.parse::<f32>().unwrap());
                    if rel {
                        max_delta = max_delta.max(de.abs() as f64 / 1e4);
                    }
                    (e[a], f[a]) = if rel { (cur[a] + de, cf[a] + df) } else { (de, df) };
                    g[a] = if rel { cb[a] + blink(v) } else { blink(v) };
                }
                max_coord = max_coord.max(e[0].abs().max(e[1].abs()) as f64 / 1e4);
                for a in 0..2 {
                    seen.push((k[a], (f[a] as f64 - e[a] as f64 / 1e4).abs()));
                }
                pts.push((e, f));
            }
            let mut exact: Vec<[i64; 2]> = pts.iter().map(|q| q.0).collect();
            if up == b'S' {
                exact.insert(0, [2 * cur[0] - c2[0], 2 * cur[1] - c2[1]]);
            }
            if up == b'C' || up == b'S' {
                c2 = exact[1];
            }
            let (e, f) = *pts.last().unwrap();
            // g is Chrome's reading of the end point (the last chunk); from the absolute text of a straight
            // step's corner it reads:
            if matches!(up, b'M' | b'L' | b'H' | b'V') {
                chrome_off += axes.iter().filter(|&&a| g[a] != blink(&format!("{:.4}", e[a] as f64 / 1e4))).count();
            }
            cb = g;
            all_rel = if up == b'M' {
                (e, f, 0.0)
            } else {
                let (pe, mut pf, worst) = all_rel;
                for a in 0..2 {
                    pf[a] += ((e[a] - pe[a]) as f64 / 1e4) as f32;
                }
                (e, pf, worst.max(off(e, pf)))
            };
            (cur, cf) = (e, f);
            got.push(exact);
            if up == b'M' {
                cmd = b'L';
            }
        }
        assert_eq!(got, want, "(1) the exact sums are the rounded points");
        assert!(max_coord < 2048.0 && s.contains('l') && s.contains('c') && s.contains('C'));
        assert!(max_k <= 15, "(2) no coordinate sums more than 15 relative steps");
        assert_eq!(chrome_off, 0, "(5) corners Chrome reads otherwise than from the absolute numbers");
        let (h, hd) = (half_step(max_coord), half_step(max_delta));
        let worst = seen.iter().map(|&(_, off)| off).fold(0.0, f64::max);
        for &(k, off) in &seen {
            assert!(off <= (1 + k) as f64 * h + k as f64 * hd, "(3) {off} after {k} relative steps");
        }
        let bound = 16.0 * h + 15.0 * hd;
        assert!(worst <= bound, "(2) and (3): {worst} px, bound {bound} px");
        assert!(all_rel.2 > bound, "(4) every step relative drifts {} px, bound {bound} px", all_rel.2);
        let rel_steps = s.bytes().filter(|c| b"lhvcs".contains(c)).count();
        println!("{} bytes, {rel_steps} relative letters, at most {max_k} relative steps summed; drift {worst:.6} px, bound {bound:.6} px; \
                  every step relative {:.6} px", s.len(), all_rel.2);
        // the limit itself: steps Chrome adds up exactly (multiples of 0.25 px), so only the limit makes them absolute
        let mut d = PathData::default();
        d.m([100.0, 100.0]);
        for k in 1..=40 {
            d.l([100.0 + 0.25 * k as f64, 100.0 + 0.5 * k as f64]);
        }
        let s = d.finish();
        assert_eq!((s.matches('l').count(), s.matches('L').count()), (3, 2), "(2) 15 relative, 1 absolute, ...: {s}");
    }

}
