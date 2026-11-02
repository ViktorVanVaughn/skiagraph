//! 2D curves and exact region extraction by "split and classify" (prior-art report 09, section 2).
//!
//! A region is described only by a membership predicate `inside(p)` and a list of candidate curves
//! that contain its whole boundary (circles, ellipses = projected 3D circles, segments). Every curve
//! is cut at its intersections with the others; each piece is tested on both sides with `inside`;
//! pieces with different answers are boundary edges, oriented with the region on their left and
//! chained into closed loops. Curves stay exact (no flattening): output is arcs of the input curves.

use std::f64::consts::TAU;

pub type P2 = [f64; 2];

/// FxHash (rustc's hasher: one rotate, xor and multiply per word) for the vertex and arc lookups of
/// `region_loops`, which hash millions of small integer keys. The default SipHash resists crafted
/// keys, which these are not. Nothing iterates these maps, so the output does not depend on the hash.
#[derive(Default, Clone, Copy)]
struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(w));
        }
    }
    fn write_u64(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_usize(&mut self, w: usize) {
        self.write_u64(w as u64);
    }
}

type FxMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<Fx>>;
type FxSet<K> = std::collections::HashSet<K, std::hash::BuildHasherDefault<Fx>>;

#[inline]
fn add(a: P2, b: P2) -> P2 {
    [a[0] + b[0], a[1] + b[1]]
}
#[inline]
fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}
#[inline]
fn scale(a: P2, s: f64) -> P2 {
    [a[0] * s, a[1] * s]
}
#[inline]
fn dot(a: P2, b: P2) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
#[inline]
fn cross(a: P2, b: P2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
#[inline]
pub fn dist(a: P2, b: P2) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    /// `c + cos(t)·u + sin(t)·v`, t in [0, 2π): circles, ellipses, projected 3D circles.
    Conic { c: P2, u: P2, v: P2 },
    /// `a + t·(b − a)`, t in [0, 1].
    Seg { a: P2, b: P2 },
    /// An arc of one hyperbola branch, `c + cosh(s)·u + sinh(s)·v` with s = s0 + t·(s1 − s0), t in
    /// [0, 1]: the image of a cone of rays that reaches behind the eye (perspective).
    Hyp { c: P2, u: P2, v: P2, s0: f64, s1: f64 },
}

/// Coordinates of p in a conic's frame: p = c + a·u + b·v.
fn frame_ab(c: P2, u: P2, v: P2, p: P2) -> [f64; 2] {
    let d = cross(u, v);
    let q = sub(p, c);
    [cross(q, v) / d, cross(u, q) / d]
}

impl Curve {
    pub fn circle(c: P2, r: f64) -> Curve {
        Curve::Conic { c, u: [r, 0.0], v: [0.0, r] }
    }

    /// A projected 3D circle; becomes a segment when it is seen edge-on (ellipse thinner than `thin`).
    pub fn conic_or_seg(c: P2, u: P2, v: P2, thin: f64) -> Curve {
        let det = cross(u, v).abs();
        let big = dot(u, u).max(dot(v, v)).sqrt();
        if big == 0.0 {
            return Curve::Seg { a: c, b: c };
        }
        if det / big < thin {
            // seen edge-on: the ellipse collapses onto its major axis
            let (uu, vv) = (dot(u, u), dot(v, v));
            let uv = dot(u, v);
            // direction of the major axis = eigenvector of [[uu, uv],[uv, vv]]
            let th = 0.5 * (2.0 * uv).atan2(uu - vv);
            let axis = add(scale(u, th.cos()), scale(v, th.sin()));
            return Curve::Seg { a: sub(c, axis), b: add(c, axis) };
        }
        Curve::Conic { c, u, v }
    }

    pub fn closed(&self) -> bool {
        matches!(self, Curve::Conic { .. })
    }

    pub fn point(&self, t: f64) -> P2 {
        match self {
            Curve::Conic { c, u, v } => add(*c, add(scale(*u, t.cos()), scale(*v, t.sin()))),
            Curve::Seg { a, b } => add(*a, scale(sub(*b, *a), t)),
            Curve::Hyp { c, u, v, s0, s1 } => {
                let s = s0 + t * (s1 - s0);
                add(*c, add(scale(*u, s.cosh()), scale(*v, s.sinh())))
            }
        }
    }

    pub fn deriv(&self, t: f64) -> P2 {
        match self {
            Curve::Conic { u, v, .. } => add(scale(*u, -t.sin()), scale(*v, t.cos())),
            Curve::Seg { a, b } => sub(*b, *a),
            Curve::Hyp { u, v, s0, s1, .. } => {
                let s = s0 + t * (s1 - s0);
                scale(add(scale(*u, s.sinh()), scale(*v, s.cosh())), s1 - s0)
            }
        }
    }

    /// Parameter of a point assumed to lie on the curve.
    pub fn param_of(&self, p: P2) -> f64 {
        match self {
            Curve::Conic { c, u, v } => {
                let d = cross(*u, *v);
                let q = sub(p, *c);
                let s = [cross(q, *v) / d, cross(*u, q) / d];
                let t = s[1].atan2(s[0]);
                if t < 0.0 { t + TAU } else { t }
            }
            Curve::Seg { a, b } => {
                let d = sub(*b, *a);
                let l2 = dot(d, d);
                if l2 == 0.0 { 0.0 } else { dot(sub(p, *a), d) / l2 }
            }
            Curve::Hyp { c, u, v, s0, s1 } => (frame_ab(*c, *u, *v, p)[1].asinh() - s0) / (s1 - s0),
        }
    }

    /// Implicit function of a conic: < 0 inside, 0 on the curve, > 0 outside (dimensionless). A
    /// hyperbola's is zero on both branches (callers keep the points of its own, `on_hyp`).
    fn implicit(&self, p: P2) -> f64 {
        match self {
            Curve::Conic { c, u, v } => {
                let d = cross(*u, *v);
                let q = sub(p, *c);
                let s = [cross(q, *v) / d, cross(*u, q) / d];
                s[0] * s[0] + s[1] * s[1] - 1.0
            }
            Curve::Seg { a, b } => cross(sub(*b, *a), sub(p, *a)),
            Curve::Hyp { c, u, v, .. } => {
                let s = frame_ab(*c, *u, *v, p);
                s[1] * s[1] + 1.0 - s[0] * s[0]
            }
        }
    }

    fn conditioning(&self) -> f64 {
        match self {
            Curve::Conic { u, v, .. } => cross(*u, *v).abs() / dot(*u, *u).max(dot(*v, *v)),
            Curve::Seg { .. } | Curve::Hyp { .. } => 0.0,
        }
    }

    /// For a point found on a hyperbola's implicit curve: its parameter on this arc, if it lies on the
    /// arc's own branch and within it (a little slack, then clamped).
    fn on_hyp(&self, p: P2) -> Option<f64> {
        let Curve::Hyp { c, u, v, .. } = self else { return None };
        if frame_ab(*c, *u, *v, p)[0] <= 0.0 {
            return None;
        }
        let t = self.param_of(p);
        (-1e-9..=1.0 + 1e-9).contains(&t).then(|| t.clamp(0.0, 1.0))
    }

    pub fn bbox(&self) -> (P2, P2) {
        match self {
            Curve::Conic { c, u, v } => {
                let ex = u[0].hypot(v[0]);
                let ey = u[1].hypot(v[1]);
                ([c[0] - ex, c[1] - ey], [c[0] + ex, c[1] + ey])
            }
            Curve::Seg { a, b } => ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]),
            Curve::Hyp { u, v, s0, s1, .. } => {
                // the ends, and per axis the point where cosh·u + sinh·v turns (tanh s = −v/u)
                let mut ts = vec![0.0, 1.0];
                for k in 0..2 {
                    if v[k].abs() < u[k].abs() {
                        let s = (-v[k] / u[k]).atanh();
                        ts.push((s - s0) / (s1 - s0));
                    }
                }
                let pts: Vec<P2> = ts.into_iter().filter(|t| (0.0..=1.0).contains(t)).map(|t| self.point(t)).collect();
                let lo = [pts.iter().map(|p| p[0]).fold(f64::MAX, f64::min), pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min)];
                let hi = [pts.iter().map(|p| p[0]).fold(f64::MIN, f64::max), pts.iter().map(|p| p[1]).fold(f64::MIN, f64::max)];
                (lo, hi)
            }
        }
    }
}

/// How far apart two curves of the same kind are (largest distance; infinite for a segment vs a
/// conic). A conic's parametrisation may differ.
pub fn curve_gap(a: &Curve, b: &Curve) -> f64 {
    match (a, b) {
        (Curve::Seg { a: p, b: q }, Curve::Seg { a: r, b: s }) => {
            (dist(*p, *r).max(dist(*q, *s))).min(dist(*p, *s).max(dist(*q, *r)))
        }
        (Curve::Conic { c: ca, .. }, Curve::Conic { c: cb, .. }) => {
            let (u, v) = (dist(*ca, *cb), b.bbox());
            if u > (v.1[0] - v.0[0]).max(v.1[1] - v.0[1]) {
                return u; // far apart
            }
            (0..8)
                .map(|k| {
                    let q = a.point(TAU * k as f64 / 8.0);
                    dist(b.point(b.param_of(q)), q)
                })
                .fold(u, f64::max)
        }
        _ => f64::INFINITY,
    }
}

/// All intersections of two curves as parameter pairs (ta, tb).
#[cfg(test)]
pub fn intersect(a: &Curve, b: &Curve) -> Vec<(f64, f64)> {
    intersect_boxed(a, b, a.bbox(), b.bbox())
}

/// `intersect` with the curves' boxes (`Curve::bbox`) already at hand.
fn intersect_boxed(a: &Curve, b: &Curve, (la, ha): (P2, P2), (lb, hb): (P2, P2)) -> Vec<(f64, f64)> {
    let pad = 1e-9 * (1.0 + ha[0].abs() + ha[1].abs());
    if la[0] > hb[0] + pad || lb[0] > ha[0] + pad || la[1] > hb[1] + pad || lb[1] > ha[1] + pad {
        return vec![];
    }
    match (a, b) {
        (Curve::Seg { .. } | Curve::Conic { .. }, Curve::Hyp { .. }) => intersect_boxed(b, a, (lb, hb), (la, ha)).into_iter().map(|(x, y)| (y, x)).collect(),
        (Curve::Hyp { c, u, v, .. }, Curve::Seg { a: p, b: q }) => {
            // the line p + t(q − p) in the hyperbola's frame (α, β): α² − β² = 1
            let s0 = frame_ab(*c, *u, *v, *p);
            let d = cross(*u, *v);
            let s1 = [cross(sub(*q, *p), *v) / d, cross(*u, sub(*q, *p)) / d];
            let aa = s1[0] * s1[0] - s1[1] * s1[1];
            let bb = 2.0 * (s0[0] * s1[0] - s0[1] * s1[1]);
            let cc = s0[0] * s0[0] - s0[1] * s0[1] - 1.0;
            let mut roots = vec![];
            if aa.abs() <= 1e-12 * (s1[0] * s1[0] + s1[1] * s1[1]) {
                if bb != 0.0 {
                    roots.push(-cc / bb); // along an asymptote: one crossing
                }
            } else {
                let disc = bb * bb - 4.0 * aa * cc;
                if -disc / (4.0 * aa.abs()) > 1e-9 {
                    return vec![];
                }
                let sq = disc.max(0.0).sqrt();
                let qq = -0.5 * (bb + bb.signum() * sq);
                if qq != 0.0 && disc > 0.0 {
                    roots.extend([qq / aa, cc / qq]);
                } else {
                    roots.push(-bb / (2.0 * aa));
                }
            }
            roots
                .into_iter()
                .filter(|t| (-1e-12..=1.0 + 1e-12).contains(t))
                .filter_map(|t| {
                    let t = t.clamp(0.0, 1.0);
                    a.on_hyp(b.point(t)).map(|th| (th, t))
                })
                .collect()
        }
        (Curve::Hyp { .. }, Curve::Conic { .. }) => conic_roots(b, a).into_iter().filter_map(|t| a.on_hyp(b.point(t)).map(|th| (th, t))).collect(),
        (Curve::Hyp { .. }, Curve::Hyp { .. }) => open_roots(a, b).into_iter().filter_map(|t| b.on_hyp(a.point(t)).map(|tb| (t, tb))).collect(),
        (Curve::Seg { a: p, b: q }, Curve::Seg { a: r, b: s }) => {
            let d1 = sub(*q, *p);
            let d2 = sub(*s, *r);
            let den = cross(d1, d2);
            if den == 0.0 {
                return vec![];
            }
            let w = sub(*r, *p);
            let t = cross(w, d2) / den;
            let u = cross(w, d1) / den;
            if (-1e-12..=1.0 + 1e-12).contains(&t) && (-1e-12..=1.0 + 1e-12).contains(&u) {
                vec![(t.clamp(0.0, 1.0), u.clamp(0.0, 1.0))]
            } else {
                vec![]
            }
        }
        (Curve::Seg { .. }, Curve::Conic { .. }) => {
            intersect_boxed(b, a, (lb, hb), (la, ha)).into_iter().map(|(x, y)| (y, x)).collect()
        }
        (Curve::Conic { c, u, v }, Curve::Seg { a: p, b: q }) => {
            // line p + t(q - p) into the conic's implicit: |s0 + t s1|^2 = 1
            let d = cross(*u, *v);
            let tr = |w: P2| [cross(w, *v) / d, cross(*u, w) / d];
            let s0 = tr(sub(*p, *c));
            let s1 = tr(sub(*q, *p));
            let aa = dot(s1, s1);
            let bb = 2.0 * dot(s0, s1);
            let cc = dot(s0, s0) - 1.0;
            let disc = bb * bb - 4.0 * aa * cc;
            // a line missing the conic by rounding (gap below 1e-9 of its size) touches it; these are
            // structural (a curve on a tube touches the tube's silhouette)
            if aa == 0.0 || -disc / (4.0 * aa) > 1e-9 {
                return vec![];
            }
            let sq = disc.max(0.0).sqrt();
            // numerically stable roots
            let qq = -0.5 * (bb + bb.signum() * sq);
            let mut roots = vec![];
            if qq != 0.0 && disc > 0.0 {
                roots.push(qq / aa);
                roots.push(cc / qq);
            } else {
                roots.push(-bb / (2.0 * aa));
            }
            roots
                .into_iter()
                .filter(|t| (-1e-12..=1.0 + 1e-12).contains(t))
                .map(|t| {
                    let t = t.clamp(0.0, 1.0);
                    (a.param_of(b.point(t)), t)
                })
                .collect()
        }
        (Curve::Conic { .. }, Curve::Conic { .. }) => {
            // parametrise the better-conditioned... no: use the better-conditioned one as implicit
            let (pa, imp, swap) = if a.conditioning() >= b.conditioning() { (b, a, true) } else { (a, b, false) };
            conic_roots(pa, imp)
                .into_iter()
                .map(|t| {
                    let p = pa.point(t);
                    let ti = imp.param_of(p);
                    if swap { (ti, t) } else { (t, ti) }
                })
                .collect()
        }
    }
}

/// Roots t of `imp.implicit(pa.point(t)) = 0` over [0, 2π). The function is a trigonometric
/// polynomial of degree 2 (at most 4 roots); found by sampling, bracketing and minimum search so
/// that close and tangential roots are not lost.
fn conic_roots(pa: &Curve, imp: &Curve) -> Vec<f64> {
    const N: usize = 128;
    let f = |t: f64| imp.implicit(pa.point(t));
    let ts: Vec<f64> = (0..=N).map(|i| TAU * i as f64 / N as f64).collect();
    // the sampling angles' cosines and sines are the same for every pair: worked out once
    static CS: std::sync::OnceLock<Vec<(f64, f64)>> = std::sync::OnceLock::new();
    let cs = CS.get_or_init(|| (0..=N).map(|i| TAU * i as f64 / N as f64).map(|t| (t.cos(), t.sin())).collect());
    let fs: Vec<f64> = match pa {
        Curve::Conic { c, u, v } => cs.iter().map(|&(co, si)| imp.implicit(add(*c, add(scale(*u, co), scale(*v, si))))).collect(),
        Curve::Seg { .. } | Curve::Hyp { .. } => ts.iter().map(|&t| f(t)).collect(),
    };
    let mut roots = vec![];
    let refine = |mut lo: f64, mut hi: f64, mut flo: f64| -> f64 {
        for _ in 0..200 {
            let mid = 0.5 * (lo + hi);
            if mid <= lo || mid >= hi {
                break;
            }
            let fm = f(mid);
            if fm == 0.0 {
                return mid;
            }
            if (fm < 0.0) == (flo < 0.0) {
                lo = mid;
                flo = fm;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    };
    for i in 0..N {
        let (f0, f1) = (fs[i], fs[i + 1]);
        if f0 == 0.0 {
            roots.push(ts[i]);
        } else if (f0 < 0.0) != (f1 < 0.0) && f1 != 0.0 {
            roots.push(refine(ts[i], ts[i + 1], f0));
        }
    }
    // local minima of |f| without a sign change: tangencies or close root pairs
    for i in 0..N {
        let prev = fs[(i + N - 1) % N];
        let cur = fs[i];
        let next = fs[i + 1];
        if cur.abs() <= prev.abs() && cur.abs() <= next.abs() && (prev < 0.0) == (cur < 0.0) && (cur < 0.0) == (next < 0.0)
        {
            // golden-section search for the extremum of f towards zero in [t_{i-1}, t_{i+1}]
            let (mut lo, mut hi) = (ts[i] - TAU / N as f64, ts[i] + TAU / N as f64);
            let sgn = if cur < 0.0 { -1.0 } else { 1.0 };
            let g = |t: f64| sgn * f(t);
            let phi = 0.5 * (5f64.sqrt() - 1.0);
            let mut x1 = hi - phi * (hi - lo);
            let mut x2 = lo + phi * (hi - lo);
            let (mut g1, mut g2) = (g(x1), g(x2));
            for _ in 0..100 {
                if g1 < g2 {
                    hi = x2;
                    x2 = x1;
                    g2 = g1;
                    x1 = hi - phi * (hi - lo);
                    g1 = g(x1);
                } else {
                    lo = x1;
                    x1 = x2;
                    g1 = g2;
                    x2 = lo + phi * (hi - lo);
                    g2 = g(x2);
                }
            }
            let tm = 0.5 * (lo + hi);
            let gm = g(tm);
            if gm < 0.0 {
                // the function dips through zero between samples: two close roots
                let l = ts[i] - TAU / N as f64;
                let r = ts[i] + TAU / N as f64;
                roots.push(refine(l, tm, f(l)));
                roots.push(refine(tm, r, f(tm)));
            } else if gm < 1e-7 {
                // tangency or near-miss (structural tangencies of projected circles and outlines land
                // either side of zero in floating point); an extra split point is always harmless
                roots.push(tm);
            }
        }
    }
    roots
        .into_iter()
        .map(|t| t.rem_euclid(TAU))
        .collect()
}

/// `conic_roots` along an open curve (t in [0, 1]): sign changes on a fine grid, refined by bisection,
/// and grid minima of |f| that dip through zero or nearly touch it (close roots, tangencies).
fn open_roots(pa: &Curve, imp: &Curve) -> Vec<f64> {
    const N: usize = 512;
    let f = |t: f64| imp.implicit(pa.point(t));
    let ts: Vec<f64> = (0..=N).map(|i| i as f64 / N as f64).collect();
    let fs: Vec<f64> = ts.iter().map(|&t| f(t)).collect();
    let refine = |mut lo: f64, mut hi: f64| {
        let neg = f(lo) < 0.0;
        for _ in 0..100 {
            let mid = 0.5 * (lo + hi);
            if (f(mid) < 0.0) == neg { lo = mid } else { hi = mid }
        }
        0.5 * (lo + hi)
    };
    let mut roots = vec![];
    for i in 0..N {
        if fs[i] == 0.0 {
            roots.push(ts[i]);
        } else if (fs[i] < 0.0) != (fs[i + 1] < 0.0) && fs[i + 1] != 0.0 {
            roots.push(refine(ts[i], ts[i + 1]));
        }
    }
    if fs[N] == 0.0 {
        roots.push(1.0);
    }
    for i in 1..N {
        let (prev, cur, next) = (fs[i - 1], fs[i], fs[i + 1]);
        if cur.abs() <= prev.abs() && cur.abs() <= next.abs() && (prev < 0.0) == (cur < 0.0) && (cur < 0.0) == (next < 0.0) {
            // the extremum of f towards zero between the neighbours (golden section)
            let sgn = if cur < 0.0 { -1.0 } else { 1.0 };
            let (mut lo, mut hi) = (ts[i - 1], ts[i + 1]);
            for _ in 0..100 {
                let (x1, x2) = (hi - 0.618 * (hi - lo), lo + 0.618 * (hi - lo));
                if sgn * f(x1) < sgn * f(x2) { hi = x2 } else { lo = x1 }
            }
            let tm = 0.5 * (lo + hi);
            let gm = sgn * f(tm);
            if gm < 0.0 {
                roots.push(refine(ts[i - 1], tm));
                roots.push(refine(tm, ts[i + 1]));
            } else if gm < 1e-7 {
                roots.push(tm);
            }
        }
    }
    roots
}

/// A piece of a curve, traversed from `t0` to `t1` (for closed curves `t1` may exceed 2π, or be below
/// `t0` when reversed).
#[derive(Clone, Debug)]
pub struct Arc {
    pub curve: usize,
    pub t0: f64,
    pub t1: f64,
    pub v0: usize,
    pub v1: usize,
}

impl Arc {
    fn reversed(self) -> Arc {
        Arc { curve: self.curve, t0: self.t1, t1: self.t0, v0: self.v1, v1: self.v0 }
    }
}

const NO_VERTEX: usize = usize::MAX;

/// `region_loops` for curves known only to within `fuzz[i]` px of the true boundary (0 = exact),
/// e.g. chord chains of quartic curves: their sides are probed beyond that distance.
pub fn region_loops_fuzzy(curves: &[Curve], fuzz: &[f64], inside: &dyn Fn(P2) -> bool, scale: f64) -> Vec<Vec<Arc>> {
    let (found, verts, tol) = boundary_arcs(curves, fuzz, inside, scale);
    // oriented with the inside on the arc's left
    let arcs = found.into_iter().map(|(l, _, a)| if l { a } else { a.reversed() }).collect();
    chain(curves, arcs, &verts, tol)
}

/// The arrangement of `curves` and its arcs whose two sides differ in `status`: (left, right, arc).
#[allow(clippy::type_complexity)]
fn boundary_arcs<S: Copy + PartialEq + Default>(curves: &[Curve], fuzz: &[f64], status: &dyn Fn(P2) -> S, scale: f64) -> (Vec<(S, S, Arc)>, Vec<P2>, f64) {
    // ponytail: features below 1e-6 of the region size (~1e-5 px) are merged away; renderers are f32
    let tol = 1e-6 * scale.max(1e-9);
    // coincident curves (e.g. two identical spheres) would intersect everywhere: keep one copy
    let same = |a: &Curve, b: &Curve| match (a, b) {
        (Curve::Conic { c: c1, u: u1, v: v1 }, Curve::Conic { c: c2, u: u2, v: v2 }) => {
            dist(*c1, *c2) <= tol && dist(*u1, *u2) <= tol && dist(*v1, *v2) <= tol
        }
        (Curve::Seg { a: a1, b: b1 }, Curve::Seg { a: a2, b: b2 }) => {
            (dist(*a1, *a2) <= tol && dist(*b1, *b2) <= tol) || (dist(*a1, *b2) <= tol && dist(*b1, *a2) <= tol)
        }
        (Curve::Hyp { .. }, Curve::Hyp { .. }) => [0.0, 0.5, 1.0].iter().all(|&t| dist(a.point(t), b.point(b.param_of(a.point(t)).clamp(0.0, 1.0))) <= tol),
        _ => false,
    };
    // curves sorted by the left edge of their box: equal curves are neighbours within tol
    let boxes: Vec<(P2, P2)> = curves.iter().map(|c| c.bbox()).collect();
    let mut order: Vec<usize> = (0..curves.len()).collect();
    order.sort_by(|&a, &b| boxes[a].0[0].total_cmp(&boxes[b].0[0]));
    let mut active = vec![true; curves.len()];
    for (oi, &i) in order.iter().enumerate() {
        for &j in order[oi + 1..].iter().take_while(|&&j| boxes[j].0[0] <= boxes[i].0[0] + 2.0 * tol) {
            if active[i] && active[j] && same(&curves[i], &curves[j]) {
                active[i.max(j)] = false; // keep the first copy
            }
        }
    }
    // vertices and per-curve split parameters
    let mut verts: Vec<P2> = vec![];
    let mut splits: Vec<Vec<(f64, usize)>> = vec![vec![]; curves.len()];
    // vertices within tol are one vertex; a hash grid of cell tol finds them (3x3 cells)
    let mut grid: FxMap<(i64, i64), Vec<usize>> = Default::default();
    let mut vertex = |p: P2, verts: &mut Vec<P2>| -> usize {
        let key = ((p[0] / tol).floor() as i64, (p[1] / tol).floor() as i64);
        for (dx, dy) in (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (dx, dy))) {
            for &i in grid.get(&(key.0 + dx, key.1 + dy)).into_iter().flatten() {
                if dist(p, verts[i]) <= tol {
                    return i;
                }
            }
        }
        verts.push(p);
        grid.entry(key).or_default().push(verts.len() - 1);
        verts.len() - 1
    };
    for i in 0..curves.len() {
        if !curves[i].closed() {
            let (a, b) = match &curves[i] {
                Curve::Seg { a, b } => (*a, *b),
                c => (c.point(0.0), c.point(1.0)),
            };
            let va = vertex(a, &mut verts);
            let vb = vertex(b, &mut verts);
            splits[i].push((0.0, va));
            splits[i].push((1.0, vb));
        }
    }
    // pairs whose boxes overlap in x (sweep over curves sorted by left edge)
    for (oi, &a) in order.iter().enumerate() {
        for &b in &order[oi + 1..] {
            if boxes[b].0[0] > boxes[a].1[0] + 1e-6 * (1.0 + scale) {
                break;
            }
            let (i, j) = (a.min(b), a.max(b));
            if !active[i] || !active[j] {
                continue;
            }
            for (ti, tj) in intersect_boxed(&curves[i], &curves[j], boxes[i], boxes[j]) {
                let p = curves[i].point(ti);
                let v = vertex(p, &mut verts);
                splits[i].push((ti, v));
                splits[j].push((tj, v));
            }
        }
    }
    // a vertex found on one curve also splits every other curve passing through it
    let mut by_x: Vec<usize> = (0..verts.len()).collect();
    by_x.sort_by(|&a, &b| verts[a][0].total_cmp(&verts[b][0]));
    for i in 0..curves.len() {
        if !active[i] {
            continue;
        }
        let (lo, hi) = boxes[i];
        let first = by_x.partition_point(|&v| verts[v][0] < lo[0] - tol);
        for &v in by_x[first..].iter().take_while(|&&v| verts[v][0] <= hi[0] + tol) {
            let p = &verts[v];
            if p[1] < lo[1] - tol || p[1] > hi[1] + tol || splits[i].iter().any(|s| s.1 == v) {
                continue;
            }
            let t = curves[i].param_of(*p);
            let t = if curves[i].closed() { t } else { t.clamp(0.0, 1.0) };
            if dist(curves[i].point(t), *p) <= tol {
                splits[i].push((t, v));
            }
        }
    }

    let mut arcs: Vec<(S, S, Arc)> = vec![];
    for (ci, curve) in curves.iter().enumerate() {
        if !active[ci] {
            continue;
        }
        let mut s = splits[ci].clone();
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
        s.dedup_by(|a, b| a.1 == b.1);
        let pieces: Vec<(f64, f64, usize, usize)> = if curve.closed() {
            if s.is_empty() {
                vec![(0.0, TAU, NO_VERTEX, NO_VERTEX)]
            } else {
                let n = s.len();
                (0..n)
                    .map(|k| {
                        let (t0, v0) = s[k];
                        let (t1, v1) = if k + 1 < n { s[k + 1] } else { (s[0].0 + TAU, s[0].1) };
                        (t0, t1, v0, v1)
                    })
                    .collect()
            }
        } else {
            s.windows(2).map(|w| (w[0].0, w[1].0, w[0].1, w[1].1)).collect()
        };
        for (t0, t1, v0, v1) in pieces {
            let mid = curve.point(0.5 * (t0 + t1));
            if t1 - t0 <= 0.0 || (v0 == v1 && v0 != NO_VERTEX && dist(curve.point(t0), mid) <= tol) {
                continue; // empty, or a sliver between two merged vertices
            }
            classify(curve, ci, t0, t1, v0, v1, status, scale, 3.0 * fuzz[ci], &mut arcs, &mut verts, 0);
        }
    }
    (arcs, verts, tol)
}

/// The status on each side of the curve at parameter t: (left, right).
fn sides<S: Default>(curve: &Curve, t: f64, inside: &dyn Fn(P2) -> S, sz: f64, eta_min: f64) -> (S, S) {
    let p = curve.point(t);
    let d = curve.deriv(t);
    let len = dot(d, d).sqrt();
    if len == 0.0 {
        return (S::default(), S::default());
    }
    let n = [-d[1] / len, d[0] / len];
    let eta = (1e-7 * sz.max(1e-9)).max(eta_min);
    (inside(add(p, scale(n, eta))), inside(sub(p, scale(n, eta))))
}

#[allow(clippy::too_many_arguments)]
fn classify<S: Copy + PartialEq + Default>(
    curve: &Curve,
    ci: usize,
    t0: f64,
    t1: f64,
    v0: usize,
    v1: usize,
    inside: &dyn Fn(P2) -> S,
    scale: f64,
    eta_min: f64,
    arcs: &mut Vec<(S, S, Arc)>,
    verts: &mut Vec<P2>,
    depth: u32,
) {
    // status at several interior points; a change means a missed split point: find it by bisection
    const K: usize = 7;
    // near a tangency two curves are only ~κL²/2 apart along a short arc of length L, so short arcs
    // are probed closer to the curve (between 1e-9 and 1e-7 of the region size)
    let d = curve.deriv(0.5 * (t0 + t1));
    let len = (t1 - t0).abs() * d[0].hypot(d[1]);
    let sz = scale.min((1e3 * len * len / scale).max(1e-2 * scale));
    let st: Vec<(S, S)> = (1..=K).map(|k| sides(curve, t0 + (t1 - t0) * k as f64 / (K + 1) as f64, inside, sz, eta_min)).collect();
    // no bisection below 1e-6 of the region size (status noise where curves touch)
    if depth < 40 && len > 1e-6 * scale {
        if let Some(k) = (1..K).find(|&k| st[k] != st[k - 1]) {
            let (mut lo, mut hi) = (t0 + (t1 - t0) * k as f64 / (K + 1) as f64, t0 + (t1 - t0) * (k + 1) as f64 / (K + 1) as f64);
            let slo = st[k - 1];
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if sides(curve, mid, inside, sz, eta_min) == slo {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let tm = 0.5 * (lo + hi);
            verts.push(curve.point(tm));
            let vm = verts.len() - 1;
            classify(curve, ci, t0, tm, v0, vm, inside, scale, eta_min, arcs, verts, depth + 1);
            classify(curve, ci, tm, t1, vm, v1, inside, scale, eta_min, arcs, verts, depth + 1);
            return;
        }
    }
    let s = st[K / 2];
    if s.0 != s.1 {
        arcs.push((s.0, s.1, Arc { curve: ci, t0, t1, v0, v1 }));
    }
}

fn chain(curves: &[Curve], mut arcs: Vec<Arc>, verts: &[P2], tol: f64) -> Vec<Vec<Arc>> {
    // drop short dead-end fragments (side tests are ambiguous next to near-tangent curves);
    // a real boundary arc always has a successor and a predecessor
    let short = 1e4 * tol; // 1% of the region size
    // two arcs joining the same vertices (near-tangent curves): keep one
    let mut seen = FxSet::default();
    arcs.retain(|a| a.v0 == NO_VERTEX || seen.insert((a.v0, a.v1)));
    loop {
        let starts: FxSet<usize> = arcs.iter().map(|a| a.v0).collect();
        let ends: FxSet<usize> = arcs.iter().map(|a| a.v1).collect();
        let n0 = arcs.len();
        arcs.retain(|a| {
            let len = dist(curves[a.curve].point(a.t0), curves[a.curve].point(a.t1));
            a.v0 == NO_VERTEX || len > short || (starts.contains(&a.v1) && ends.contains(&a.v0))
        });
        if arcs.len() == n0 {
            break;
        }
    }
    let mut used = vec![false; arcs.len()];
    let mut loops = vec![];
    let endpoint = |a: &Arc, end: bool| curves[a.curve].point(if end { a.t1 } else { a.t0 });
    let ix = Starts::new(&arcs, &|a| endpoint(a, false), tol);
    // pass 0: closed cycles only (with backtracking); pass 1: greedy chains of the leftovers
    for (pass, start) in (0..2).flat_map(|p| (0..arcs.len()).map(move |s| (p, s))) {
        if used[start] {
            continue;
        }
        if pass == 0 && arcs[start].v0 != NO_VERTEX {
            if let Some(path) = closed_cycle(curves, &arcs, &mut used, start, tol, &ix) {
                loops.push(path.into_iter().map(|k| arcs[k].clone()).collect());
            }
            continue;
        }
        used[start] = true;
        let mut lp = vec![arcs[start].clone()];
        if arcs[start].v0 == NO_VERTEX {
            loops.push(lp);
            continue;
        }
        loop {
            let cur = lp.last().unwrap();
            if cur.v1 == lp[0].v0 {
                break;
            }
            let pend = endpoint(cur, true);
            // next arc: same vertex id, else nearest start point
            let mut best: Option<(usize, f64)> = None;
            for (k, a) in arcs.iter().enumerate() {
                if used[k] {
                    continue;
                }
                let d = if a.v0 == cur.v1 { 0.0 } else { dist(endpoint(a, false), pend) };
                if d <= tol.max(1e-7 * (1.0 + pend[0].abs() + pend[1].abs())) && best.is_none_or(|b| d < b.1) {
                    best = Some((k, d));
                }
            }
            match best {
                Some((k, _)) => {
                    used[k] = true;
                    lp.push(arcs[k].clone());
                }
                None => {
                    // not closed by vertex id: accept if it closes by position, drop short leftovers
                    let gap = dist(pend, endpoint(&lp[0], false));
                    if gap > 1e-5 * (1.0 + pend[0].abs()) {
                        let len: f64 = lp.iter().map(|a| dist(endpoint(a, false), endpoint(a, true))).sum();
                        // leftovers are short or enclose no area (a stray sliver run forth and back)
                        let pts: Vec<P2> = lp.iter().flat_map(|a| (0..8).map(move |i| curves[a.curve].point(a.t0 + (a.t1 - a.t0) * i as f64 / 8.0))).collect();
                        let area = (0..pts.len()).map(|i| cross(pts[i], pts[(i + 1) % pts.len()])).sum::<f64>().abs() / 2.0;
                        if len > short * 5.0 && area > 1e-6 * len * len {
                            // a developer's diagnostic (the loop is dropped later, `ink.rs`): silent unless MOL25D_DEBUG is set
                            if std::env::var("MOL25D_DEBUG").is_ok() {
                                eprintln!("warning: open boundary loop near ({:.3}, {:.3})", pend[0], pend[1]);
                                eprintln!("curves: {curves:?}");
                                for a in &arcs {
                                    eprintln!("  arc c{} {:.12}..{:.12} v{} -> v{}", a.curve, a.t0, a.t1, a.v0 as i64, a.v1 as i64);
                                }
                            }
                        } else {
                            lp.clear();
                        }
                    }
                    break;
                }
            }
        }
        let _ = verts;
        if !lp.is_empty() {
            loops.push(lp);
        }
    }
    // chains still open (an arc missed next to near-tangent curves): join each to the open chain
    // starting nearest its end, or close it on itself if that is nearer
    let start_of = |lp: &Vec<Arc>| endpoint(&lp[0], false);
    let end_of = |lp: &Vec<Arc>| endpoint(lp.last().unwrap(), true);
    let closes = |lp: &Vec<Arc>| {
        lp[0].v0 == NO_VERTEX || lp.last().unwrap().v1 == lp[0].v0 || dist(end_of(lp), start_of(lp)) <= 1e-5 * (1.0 + start_of(lp)[0].abs())
    };
    let (mut closed, mut open): (Vec<_>, Vec<_>) = loops.into_iter().partition(closes);
    while let Some(mut lp) = open.pop() {
        let e = end_of(&lp);
        let best = open.iter().enumerate().map(|(k, o)| (k, dist(e, start_of(o)))).min_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((k, d)) if d < dist(e, start_of(&lp)) => {
                lp.extend(open.swap_remove(k));
                open.push(lp);
            }
            _ => closed.push(lp),
        }
    }
    closed
}

/// Arcs by start vertex and by start point (grid as coarse as the largest position tolerance), so
/// finding an arc's possible successors is not a scan of all arcs (long chord chains).
struct Starts {
    cell: f64,
    by_cell: FxMap<(i64, i64), Vec<usize>>,
    by_v0: FxMap<usize, Vec<usize>>,
}

impl Starts {
    fn new(arcs: &[Arc], start: &dyn Fn(&Arc) -> P2, tol: f64) -> Starts {
        let cell = 2.0 * arcs.iter().map(|a| { let q = start(a); 1e-7 * (1.0 + q[0].abs() + q[1].abs()) }).fold(tol, f64::max);
        let mut ix = Starts { cell, by_cell: Default::default(), by_v0: Default::default() };
        for (k, a) in arcs.iter().enumerate() {
            ix.by_cell.entry(ix.key(start(a))).or_default().push(k);
            ix.by_v0.entry(a.v0).or_default().push(k);
        }
        ix
    }
    fn key(&self, q: P2) -> (i64, i64) {
        ((q[0] / self.cell).floor() as i64, (q[1] / self.cell).floor() as i64)
    }
    /// Arcs starting at vertex v or near point q (a superset of the possible successors).
    fn near(&self, v: usize, q: P2) -> Vec<usize> {
        let k = self.key(q);
        let mut out: Vec<usize> = self.by_v0.get(&v).cloned().unwrap_or_default();
        for (dx, dy) in (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (dx, dy))) {
            out.extend(self.by_cell.get(&(k.0 + dx, k.1 + dy)).into_iter().flatten());
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Depth-first search for a closed chain of unused arcs starting with `start` (marked used on
/// success). Same continuation rule as the greedy chaining: same vertex id, else a start point
/// within tolerance; a dead end closes if it is back at the start by position.
fn closed_cycle(curves: &[Curve], arcs: &[Arc], used: &mut [bool], start: usize, tol: f64, ix: &Starts) -> Option<Vec<usize>> {
    let endpoint = |a: &Arc, end: bool| curves[a.curve].point(if end { a.t1 } else { a.t0 });
    let p0 = endpoint(&arcs[start], false);
    let next_of = |i: usize, used: &[bool]| -> Vec<usize> {
        let pend = endpoint(&arcs[i], true);
        let lim = tol.max(1e-7 * (1.0 + pend[0].abs() + pend[1].abs()));
        let mut c: Vec<(f64, usize)> = ix
            .near(arcs[i].v1, pend)
            .into_iter()
            .filter(|&k| !used[k])
            .filter_map(|k| {
                let d = if arcs[k].v0 == arcs[i].v1 { 0.0 } else { dist(endpoint(&arcs[k], false), pend) };
                (d <= lim).then_some((d, k))
            })
            .collect();
        c.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1))); // best last (popped first)
        c.into_iter().map(|x| x.1).collect()
    };
    used[start] = true;
    let mut path = vec![start];
    let mut todo = vec![next_of(start, used)];
    let mut steps = 0usize;
    loop {
        let cur = *path.last().unwrap();
        if arcs[cur].v1 == arcs[start].v0 {
            return Some(path);
        }
        let dead_end = todo.last().unwrap().is_empty();
        if dead_end && path.len() > 1 && dist(endpoint(&arcs[cur], true), p0) <= 1e-5 * (1.0 + p0[0].abs()) {
            return Some(path);
        }
        steps += 1;
        if steps > 16 * arcs.len() + 1000 {
            for &k in &path {
                used[k] = false;
            }
            return None;
        }
        match todo.last_mut().unwrap().pop() {
            Some(k) if !used[k] => {
                used[k] = true;
                path.push(k);
                todo.push(next_of(k, used));
            }
            Some(_) => {}
            None => {
                todo.pop();
                let k = path.pop().unwrap();
                used[k] = false;
                if path.is_empty() {
                    return None;
                }
            }
        }
    }
}

/// Cubic Bézier segments approximating an arc to within `delta` (exact for segments).
/// Returns (start point, list of [c1, c2, end]).
pub fn arc_cubics(curve: &Curve, t0: f64, t1: f64, delta: f64) -> (P2, Vec<[P2; 3]>) {
    match curve {
        Curve::Seg { .. } => {
            let a = curve.point(t0);
            let b = curve.point(t1);
            (a, vec![[a, b, b]])
        }
        Curve::Hyp { .. } => {
            // Hermite cubics on n equal parameter steps, n doubled until each step's quarter and middle
            // points lie within delta of the curve
            let bez = |p0: P2, c: &[P2; 3], s: f64| {
                let m = 1.0 - s;
                add(add(scale(p0, m * m * m), scale(c[0], 3.0 * m * m * s)), add(scale(c[1], 3.0 * m * s * s), scale(c[2], s * s * s)))
            };
            let mut n = 1usize;
            loop {
                let h = (t1 - t0) / n as f64;
                let out: Vec<[P2; 3]> = (0..n)
                    .map(|i| {
                        let (a, b) = (t0 + h * i as f64, t0 + h * (i + 1) as f64);
                        let (pa, pb) = (curve.point(a), curve.point(b));
                        [add(pa, scale(curve.deriv(a), h / 3.0)), sub(pb, scale(curve.deriv(b), h / 3.0)), pb]
                    })
                    .collect();
                let err = (0..n)
                    .map(|i| {
                        let p0 = curve.point(t0 + h * i as f64);
                        [0.25, 0.5, 0.75].iter().map(|&s| dist(bez(p0, &out[i], s), curve.point(t0 + h * (i as f64 + s)))).fold(0.0, f64::max)
                    })
                    .fold(0.0, f64::max);
                if err <= delta || n >= 4096 {
                    return (curve.point(t0), out);
                }
                n *= 2;
            }
        }
        Curve::Conic { u, v, .. } => {
            let r = dot(*u, *u).max(dot(*v, *v)).sqrt().max(1e-12);
            // kurbo's error law for arc -> cubic: n = (1.1163 r / delta)^(1/6) per full turn
            let per_turn = (1.1163 * r / delta).powf(1.0 / 6.0).ceil().max(4.0);
            let span = t1 - t0;
            let n = ((span.abs() / TAU) * per_turn).ceil().max(1.0) as usize;
            let h = span / n as f64;
            let k = 4.0 / 3.0 * (h / 4.0).tan();
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let a = t0 + h * i as f64;
                let b = a + h;
                let pa = curve.point(a);
                let pb = curve.point(b);
                let da = curve.deriv(a);
                let db = curve.deriv(b);
                out.push([add(pa, scale(da, k)), sub(pb, scale(db, k)), pb]);
            }
            (curve.point(t0), out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_circle_intersections() {
        let a = Curve::circle([0.0, 0.0], 1.0);
        let b = Curve::circle([1.0, 0.0], 1.0);
        let r = intersect(&a, &b);
        assert_eq!(r.len(), 2);
        for (ta, tb) in r {
            let p = a.point(ta);
            let q = b.point(tb);
            assert!(dist(p, q) < 1e-12);
            assert!((p[0] - 0.5).abs() < 1e-12);
        }
    }

    #[test]
    fn tangent_circles_give_one_point() {
        let a = Curve::circle([0.0, 0.0], 1.0);
        let b = Curve::circle([2.0, 0.0], 1.0);
        let r = intersect(&a, &b);
        assert!(!r.is_empty());
        for (ta, _) in r {
            assert!(dist(a.point(ta), [1.0, 0.0]) < 1e-6);
        }
    }

    #[test]
    fn chaining_skips_a_stray_arc() {
        // unit square plus a stray arc leaving corner 1: greedy chaining would follow the stray
        let p = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [1.5, 0.5]];
        let e = [(0, 1), (1, 4), (1, 2), (2, 3), (3, 0)];
        let curves: Vec<Curve> = e.iter().map(|&(a, b)| Curve::Seg { a: p[a], b: p[b] }).collect();
        let arcs = e.iter().enumerate().map(|(i, &(a, b))| Arc { curve: i, t0: 0.0, t1: 1.0, v0: a, v1: b }).collect();
        let loops = chain(&curves, arcs, &p, 1e-9);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].iter().map(|a| a.curve).collect::<Vec<_>>(), vec![0, 2, 3, 4]);
    }

    #[test]
    fn line_missing_a_circle_by_rounding_touches_it() {
        // a tube's silhouette touching the projection of a curve on the tube (structural tangency)
        let c = Curve::circle([1.0, 2.0], 3.0);
        let s = Curve::Seg { a: [-3.0, 5.0 + 1e-13], b: [6.0, 5.0 + 1e-13] };
        let r = intersect(&s, &c);
        assert_eq!(r.len(), 1);
        assert!(dist(s.point(r[0].0), [1.0, 5.0]) < 1e-5);
        let far = Curve::Seg { a: [-3.0, 5.001], b: [6.0, 5.001] };
        assert!(intersect(&far, &c).is_empty());
    }

    /// A hyperbola branch meets a segment, an ellipse and another branch where both curves pass (and
    /// never on its other branch); its cubics follow it within the tolerance.
    #[test]
    fn hyperbola_branch_intersections_and_cubics() {
        let h = Curve::Hyp { c: [10.0, 5.0], u: [3.0, 1.0], v: [-1.0, 4.0], s0: -2.0, s1: 2.5 };
        let other = Curve::Hyp { c: [12.0, 2.0], u: [-2.0, 2.0], v: [3.0, 3.0], s0: -2.0, s1: 2.0 };
        let mid = h.point(0.4);
        let cases = [
            (Curve::Seg { a: [0.0, 0.0], b: [40.0, 30.0] }, 1),
            (Curve::Seg { a: [mid[0] - 30.0, mid[1]], b: [mid[0] + 30.0, mid[1]] }, 1),
            (Curve::circle(mid, 3.0), 2),
            (Curve::Conic { c: [4.0, 4.0], u: [12.0, 2.0], v: [-1.0, 3.0] }, 0),
            (other.clone(), 0),
        ];
        for (c, at_least) in cases {
            let r = intersect(&h, &c);
            assert!(r.len() >= at_least, "{c:?}: {r:?}");
            for (th, tc) in r {
                assert!((0.0..=1.0).contains(&th));
                assert!(dist(h.point(th), c.point(tc)) < 1e-6, "{c:?}: {:?} vs {:?}", h.point(th), c.point(tc));
            }
            // the same points with the curves swapped
            let mut r1: Vec<f64> = intersect(&h, &c).iter().map(|x| x.0).collect();
            let mut r2: Vec<f64> = intersect(&c, &h).iter().map(|x| x.1).collect();
            r1.sort_by(f64::total_cmp);
            r2.sort_by(f64::total_cmp);
            assert_eq!(r1.len(), r2.len());
        }
        // a point on the other branch (c − u) is no crossing
        let q = sub([10.0, 5.0], [3.0, 1.0]);
        assert!(intersect(&h, &Curve::circle(q, 0.5)).is_empty());
        assert!((h.param_of(h.point(0.3)) - 0.3).abs() < 1e-12);
        let (p0, cs) = arc_cubics(&h, 0.1, 0.9, 1e-5);
        let mut p = p0;
        for seg in &cs {
            for k in 0..=10 {
                let s = k as f64 / 10.0;
                let m = 1.0 - s;
                let b = add(add(scale(p, m * m * m), scale(seg[0], 3.0 * m * m * s)), add(scale(seg[1], 3.0 * m * s * s), scale(seg[2], s * s * s)));
                assert!(h.implicit(b).abs() < 1e-5, "off the hyperbola by {}", h.implicit(b));
            }
            p = seg[2];
        }
        assert!(dist(p, h.point(0.9)) < 1e-12);
    }

    #[test]
    fn cubics_match_circle() {
        let c = Curve::circle([0.0, 0.0], 100.0);
        let (_, cs) = arc_cubics(&c, 0.0, TAU, 1e-5);
        let mut p0 = c.point(0.0);
        let mut worst: f64 = 0.0;
        for seg in &cs {
            for k in 0..=20 {
                let t = k as f64 / 20.0;
                let mt = 1.0 - t;
                let p = add(
                    add(scale(p0, mt * mt * mt), scale(seg[0], 3.0 * mt * mt * t)),
                    add(scale(seg[1], 3.0 * mt * t * t), scale(seg[2], t * t * t)),
                );
                worst = worst.max((dist(p, [0.0, 0.0]) - 100.0).abs());
            }
            p0 = seg[2];
        }
        assert!(worst < 2e-5, "worst {worst}");
    }
}
