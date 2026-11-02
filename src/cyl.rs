//! Cylinders (Tachyon `FCylinder`: an open finite tube, no caps) and rings (flat discs: VMD's caps
//! for "filled" cylinders), in orthographic and perspective view: depth of the visible surface at an
//! image point, outlines, and the curves where they meet spheres and each other (prior-art report 09,
//! §1.3). In perspective the silhouettes stay straight (the two generators whose tangent planes pass
//! through the eye) and every planar circle or ellipse is drawn as its exact image conic
//! (`persp::ellipse_image`).
//!
//! Everything is computed in world units along the eye ray of an SVG point, the same way Tachyon
//! does it, so "nearer" means exactly what Tachyon's nearest-hit rule means.
//!
//! A tube is seen as two surfaces:
//! - the *front* (outer) wall: where the ray enters the infinite cylinder, if that point lies on
//!   the finite tube;
//! - the *back* (inner) wall: where the ray leaves it, seen through an open end when the entry point
//!   is beyond the tube's ends.
//!
//! Tachyon shows the nearer valid one, which is the front wall whenever it is valid.

use crate::geom::{Curve, P2};
use crate::scene::V3;
use crate::svg::{Sph, View};
use std::f64::consts::TAU;

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> V3 {
    mul(a, 1.0 / norm(a))
}

/// Which surface of a primitive a piece shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surf {
    /// Sphere, ring, or a tube's outer wall.
    Front,
    /// A tube's inner wall, seen through an open end.
    Back,
}

/// Two unit vectors perpendicular to unit `n`; the first lies in the image plane when possible
/// (so it projects to full length).
fn plane_basis(n: V3, view_dir: V3) -> (V3, V3) {
    let mut e1 = cross(n, view_dir);
    if norm(e1) < 1e-12 {
        // seen exactly end-on: any perpendicular will do
        e1 = cross(n, if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] });
    }
    let e1 = unit(e1);
    (e1, cross(n, e1))
}

#[derive(Clone)]
pub struct Cyl {
    pub prim: usize,
    /// Base point (world).
    pub b: V3,
    /// Unit axis direction, base -> apex.
    pub a: V3,
    pub len: f64,
    pub r: f64,
    /// Radius in pixels.
    pub rpx: f64,
    e1: V3,
    e2: V3,
    /// The two end planes as (point, normal pointing into the tube). Normally the end circles
    /// (base, a) and (apex, -a); `mitre` tilts them where two tubes continue each other.
    pub ends: [(V3, V3); 2],
}

/// Eye-ray hits with an infinite cylinder: depths and world points of entry and exit.
struct Roots {
    t_in: f64,
    x_in: V3,
    t_out: f64,
    x_out: V3,
}

impl Cyl {
    pub fn new(view: &View, prim: usize, base: V3, axis: V3, rad: f64) -> Cyl {
        let len = norm(axis);
        let a = mul(axis, 1.0 / len);
        let (e1, e2) = plane_basis(a, view.v);
        let apex = add(base, mul(a, len));
        let rpx = match view.eye {
            None => rad / view.psx,
            Some(_) => rad * px_scale(view, &[base, apex], rad),
        };
        Cyl { prim, b: base, a, len, r: rad, rpx, e1, e2, ends: [(base, a), (apex, mul(a, -1.0))] }
    }

    pub fn apex(&self) -> V3 {
        add(self.b, mul(self.a, self.len))
    }

    /// Point on the tube surface at axial position `s` and angle `phi`.
    fn surface_point(&self, s: f64, phi: f64) -> V3 {
        self.surface_point_cs(s, (phi.cos(), phi.sin()))
    }

    /// `surface_point` from the angle's (cosine, sine).
    fn surface_point_cs(&self, s: f64, (cs, sn): (f64, f64)) -> V3 {
        add(add(self.b, mul(self.a, s)), add(mul(self.e1, self.r * cs), mul(self.e2, self.r * sn)))
    }

    fn roots(&self, view: &View, p: P2) -> Option<Roots> {
        let (o, v) = view.ray(p);
        // move the ray origin to the point level with the base, so the numbers stay small
        let w0 = sub(o, self.b);
        let t0 = -dot(w0, v);
        let w = add(w0, mul(v, t0));
        let wa = dot(w, self.a);
        let va = dot(v, self.a);
        let aa = 1.0 - va * va;
        if aa <= 0.0 {
            return None; // ray exactly along the axis: Tachyon reports no hit
        }
        let bb = -wa * va;
        let cc = dot(w, w) - wa * wa - self.r * self.r;
        let disc = bb * bb - aa * cc;
        if disc < 0.0 {
            return None;
        }
        let sq = disc.sqrt();
        let (ti, to) = ((-bb - sq) / aa, (-bb + sq) / aa);
        let x = |t: f64| add(o, mul(v, t0 + t));
        Some(Roots { t_in: t0 + ti, x_in: x(ti), t_out: t0 + to, x_out: x(to) })
    }

    /// Is a point of the infinite cylinder between the two end planes?
    fn on_tube(&self, x: V3) -> bool {
        self.ends.iter().all(|&(q, n)| dot(sub(x, q), n) > 0.0)
    }

    /// Depth of one surface at SVG point p (None where that surface is not hit).
    pub fn depth(&self, view: &View, p: P2, surf: Surf) -> Option<f64> {
        let r = self.roots(view, p)?;
        let (t, x) = match surf {
            Surf::Front => (r.t_in, r.x_in),
            Surf::Back => (r.t_out, r.x_out),
        };
        self.on_tube(x).then_some(t)
    }

    /// Projection of the tube's cross-section circle at axial position `s`.
    pub fn circle(&self, view: &View, s: f64) -> Option<Curve> {
        conic3(view, add(self.b, mul(self.a, s)), mul(self.e1, self.r), mul(self.e2, self.r), 1e-9 * self.rpx.max(1.0))
    }

    /// Perspective: the points b + r n of the two silhouette generators (lines along the axis whose
    /// tangent plane passes through the eye: n·(eye − b) = r). None if the eye is inside the infinite
    /// cylinder (no silhouette lines).
    pub fn persp_generators(&self, eye: V3) -> Option<[V3; 2]> {
        let w = sub(eye, self.b);
        let wp = sub(w, mul(self.a, dot(w, self.a)));
        let d = norm(wp);
        if d <= self.r {
            return None;
        }
        let wh = mul(wp, 1.0 / d);
        let ph = cross(self.a, wh);
        let (cs, sn) = (self.r / d, (1.0 - (self.r / d).powi(2)).sqrt());
        Some([1.0, -1.0].map(|sg| add(self.b, mul(add(mul(wh, cs), mul(ph, sg * sn)), self.r))))
    }

    /// Zero where the generator at angle phi is a silhouette (its normal perpendicular to the eye ray).
    fn rim_at(&self, view: &View, phi: f64) -> f64 {
        match view.eye {
            None => phi.sin(), // e1 lies in the image plane
            Some(e) => dot(add(mul(self.e1, phi.cos()), mul(self.e2, phi.sin())), sub(self.b, e)) + self.r,
        }
    }

    /// The curves bounding the tube's image: two silhouette segments and the two end ellipses.
    pub fn outline(&self, view: &View) -> Vec<Curve> {
        let mut out = vec![];
        let gens = match view.eye {
            None => (norm(cross(self.a, view.v)) > 1e-12).then(|| [1.0, -1.0].map(|sgn| add(self.b, mul(self.e1, sgn * self.r)))),
            Some(e) => self.persp_generators(e),
        };
        for off in gens.into_iter().flatten() {
            // silhouette line off + s a, cut by the two end planes
            let [a, b] = self.ends.map(|(q, n)| view.project(add(off, mul(self.a, dot(sub(q, off), n) / dot(self.a, n)))).0);
            out.push(Curve::Seg { a, b });
        }
        for (q, n) in self.ends {
            out.extend(plane_cut(view, self, q, n).0);
        }
        out
    }

    /// (nearest, farthest) depth of any point of the tube.
    pub fn depth_range(&self, view: &View) -> (f64, f64) {
        if let Some(e) = view.eye {
            // distances from the eye to the axis segment, widened by the radius (and mitred ends)
            let s = dot(sub(e, self.b), self.a).clamp(0.0, self.len);
            let near = norm(sub(e, add(self.b, mul(self.a, s))));
            let far = norm(sub(e, self.b)).max(norm(sub(e, self.apex())));
            return ((near - 1.001 * self.r).max(0.0), far + 1.001 * self.r);
        }
        let (_, d0) = view.project(self.b);
        let (_, d1) = view.project(self.apex());
        let va = dot(view.v, self.a);
        let ext = self.r * (1.0 - va * va).max(0.0).sqrt() + 1e-3 * self.r; // slack for mitred ends
        (d0.min(d1) - ext, d0.max(d1) + ext)
    }

    /// Axial position of a world point.
    fn axial(&self, x: V3) -> f64 {
        dot(sub(x, self.b), self.a)
    }
}

/// A flat ring or disc (Tachyon `Ring`), single-sided in geometry, seen from both sides.
#[derive(Clone)]
pub struct Ring {
    pub prim: usize,
    pub c: V3,
    /// Unit normal.
    pub n: V3,
    pub inner: f64,
    pub outer: f64,
    pub opx: f64,
    e1: V3,
    e2: V3,
}

impl Ring {
    pub fn new(view: &View, prim: usize, c: V3, n: V3, inner: f64, outer: f64) -> Ring {
        let n = unit(n);
        let (e1, e2) = plane_basis(n, view.v);
        let opx = match view.eye {
            None => outer / view.psx,
            Some(_) => outer * px_scale(view, &[c], outer),
        };
        Ring { prim, c, n, inner, outer, opx, e1, e2 }
    }

    pub fn depth(&self, view: &View, p: P2) -> Option<f64> {
        let (o, dv) = view.ray(p);
        let vn = dot(dv, self.n);
        if vn == 0.0 {
            return None;
        }
        let t = dot(sub(self.c, o), self.n) / vn;
        if t < 0.0 {
            return None;
        }
        let d = norm(sub(add(o, mul(dv, t)), self.c));
        (d > self.inner && d < self.outer).then_some(t)
    }

    fn ellipse(&self, view: &View, rad: f64) -> Option<Curve> {
        conic3(view, self.c, mul(self.e1, rad), mul(self.e2, rad), 1e-9 * self.opx.max(1.0))
    }

    pub fn outline(&self, view: &View) -> Vec<Curve> {
        let mut out: Vec<Curve> = self.ellipse(view, self.outer).into_iter().collect();
        if self.inner > 0.0 {
            out.extend(self.ellipse(view, self.inner));
        }
        out
    }

    pub fn depth_range(&self, view: &View) -> (f64, f64) {
        if let Some(e) = view.eye {
            let d = norm(sub(self.c, e));
            return ((d - self.outer).max(0.0), d + self.outer);
        }
        let (_, d) = view.project(self.c);
        let vn = dot(view.v, self.n);
        let ext = self.outer * (1.0 - vn * vn).max(0.0).sqrt();
        (d - ext, d + ext)
    }

    /// If this ring is the cap of `c` (centred on one end, same radius, normal along the axis to
    /// printing precision), take the tube's exact axis as normal, so the cap's rim and the tube's
    /// end ellipse are the same curve. Changes the disc by ~1e-6 of its radius.
    pub fn snap_to_cap(&mut self, view: &View, c: &Cyl) -> bool {
        let tol = 1e-6 * c.r;
        let at_end = norm(sub(self.c, c.b)) <= tol || norm(sub(self.c, c.apex())) <= tol;
        if at_end && (self.outer - c.r).abs() <= tol && norm(cross(self.n, c.a)) < 1e-4 {
            *self = Ring::new(view, self.prim, self.c, c.a, self.inner, self.outer);
            self.e1 = c.e1;
            self.e2 = c.e2;
            return true;
        }
        false
    }
}

/// Licorice joint: a sphere and a tube of the same radius, the tube ending at the sphere centre.
/// They touch along a whole circle (the tube's end), so their depths there differ only to second
/// order and plain comparison would flicker (ADR 0006 amendment).
pub fn licorice_joint(s: &Sph, c: &Cyl) -> bool {
    let tol = 1e-6 * c.r;
    (s.rw - c.r).abs() <= tol && (norm(sub(s.cw, c.b)) <= tol || norm(sub(s.cw, c.apex())) <= tol)
}

/// Chord error of the polyline approximation of quartic intersection curves, in pixels.
pub fn quartic_tol() -> f64 {
    5e-6 // px
}

/// Curves where a sphere meets a tube's surface.
pub fn sphere_cyl(view: &View, s: &Sph, c: &Cyl) -> (Vec<Curve>, f64) {
    let sc = c.axial(s.cw);
    let perp = norm(sub(sub(s.cw, c.b), mul(c.a, sc)));
    // nearest distance from the sphere centre to the finite axis
    let gap = if sc < 0.0 { (perp * perp + sc * sc).sqrt() } else if sc > c.len { (perp * perp + (sc - c.len).powi(2)).sqrt() } else { perp };
    if gap >= s.rw + c.r {
        return (vec![], 0.0);
    }
    if perp <= 1e-9 * c.r {
        // centre on the axis (a bond entering its atom): the surfaces meet in circles, exactly
        let h2 = s.rw * s.rw - c.r * c.r;
        if h2 < -1e-9 * c.r * c.r {
            return (vec![], 0.0); // sphere thinner than the tube
        }
        let h = h2.max(0.0).sqrt();
        let mut out = vec![];
        for st in [sc - h, sc + h] {
            if st >= -1e-9 * c.r && st <= c.len + 1e-9 * c.r {
                out.extend(c.circle(view, st));
            }
            if h == 0.0 {
                break;
            }
        }
        return (out, 0.0);
    }
    // general position: a space quartic, parametrised by the angle around the tube
    let quad = |x0: V3| {
        let w = sub(x0, s.cw);
        let wa = dot(w, c.a);
        (1.0, wa, dot(w, w) - s.rw * s.rw)
    };
    let m = c.r;
    let rim = |x: V3| dot(sub(x, s.cw), view.sight(x));
    (quartic_chains(view, c, &quad, &rim, &|x| (-m..=c.len + m).contains(&c.axial(x))), 2.0 * quartic_tol())
}

/// Curves where two tubes' surfaces meet.
pub fn cyl_cyl(view: &View, c1: &Cyl, c2: &Cyl) -> (Vec<Curve>, f64) {
    let tol = 1e-9 * c1.r.max(c2.r);
    let n = cross(c1.a, c2.a);
    let nn = norm(n);
    if nn < 1e-12 {
        // ponytail: parallel tubes (coincident or side by side) give no curves; add lines along
        // the generators if a scene ever has overlapping parallel tubes.
        return (vec![], 0.0);
    }
    for (q1, n1) in c1.ends {
        for (q2, n2) in c2.ends {
            if norm(sub(q1, q2)) <= 1e-9 && dot(n1, n2) <= -1.0 + 1e-12 {
                return (vec![], 0.0); // mitred joint: the shared end ellipse is in both outlines
            }
        }
    }
    let d = sub(c2.b, c1.b);
    let axis_gap = dot(d, n).abs() / nn;
    if axis_gap >= c1.r + c2.r || seg_dist(c1, c2) >= c1.r + c2.r {
        return (vec![], 0.0);
    }
    if axis_gap <= tol && (c1.r - c2.r).abs() <= tol {
        // equal radii and the axes meet at m: two planar ellipses, x·u1 = ±x·u2 (relative to m)
        // s1 on c1 where the axes meet: solve b1 + s1 a1 = b2 + s2 a2 (least squares)
        let s1 = dot(cross(d, c2.a), n) / (nn * nn);
        let s2 = dot(cross(d, c1.a), n) / (nn * nn);
        let m = add(c1.b, mul(c1.a, s1));
        let (lo1, hi1) = (-s1, c1.len - s1);
        let (lo2, hi2) = (-s2, c2.len - s2);
        let cdot = dot(c1.a, c2.a);
        let (alpha, beta) = (dot(c1.e1, c2.a), dot(c1.e2, c2.a));
        let mut out = vec![];
        for sigma in [1.0, -1.0] {
            // only a branch whose s1 = sigma s2 can lie inside both tubes matters
            let (l2, h2) = if sigma > 0.0 { (lo2, hi2) } else { (-hi2, -lo2) };
            if lo1.max(l2) >= hi1.min(h2) - tol {
                continue;
            }
            let k = sigma * c1.r / (1.0 - sigma * cdot);
            let u = add(mul(c1.e1, c1.r), mul(c1.a, k * alpha));
            let v = add(mul(c1.e2, c1.r), mul(c1.a, k * beta));
            out.extend(conic3(view, m, u, v, 1e-9 * c1.rpx.max(1.0)));
        }
        return (out, 0.0);
    }
    // general position: a space quartic
    let pa1 = sub(c1.a, mul(c2.a, dot(c1.a, c2.a)));
    let quad = |x0: V3| {
        let w = sub(x0, c2.b);
        let pw = sub(w, mul(c2.a, dot(w, c2.a)));
        (dot(pa1, pa1), dot(pw, pa1), dot(pw, pw) - c2.r * c2.r)
    };
    let (m1, m2) = (c1.r, c2.r);
    let keep = |x: V3| (-m1..=c1.len + m1).contains(&c1.axial(x)) && (-m2..=c2.len + m2).contains(&c2.axial(x));
    let rim = |x: V3| {
        let w = sub(x, c2.b);
        dot(sub(w, mul(c2.a, dot(w, c2.a))), view.sight(x))
    };
    (quartic_chains(view, c1, &quad, &rim, &keep), 2.0 * quartic_tol())
}

/// If `c2` has the radius of `c1` and its axis passes within 1e-3 r of `c1`'s axis without meeting
/// it exactly (VMD prints 6 digits, so bonds through one atom miss each other by up to ~1e-5), a
/// copy of `c2` moved so the axes meet. (Otherwise their meeting curve is a quartic so close to
/// two crossing ellipses that it needs thousands of chord segments, and classification near the
/// near-crossing is unreliable.)
pub fn snap_to_meet(c1: &Cyl, c2: &Cyl) -> Option<Cyl> {
    let n = cross(c1.a, c2.a);
    let nn = norm(n);
    if nn < 1e-12 || (c1.r - c2.r).abs() > 1e-9 * c1.r {
        return None;
    }
    let d = sub(c2.b, c1.b);
    let gap = dot(d, n) / nn; // signed distance between the axis lines along n
    if gap.abs() > 1e-3 * c1.r || gap == 0.0 {
        return None;
    }
    let shift = mul(n, -gap / nn);
    let mut out = c2.clone();
    out.b = add(out.b, shift);
    for e in out.ends.iter_mut() {
        e.0 = add(e.0, shift);
    }
    Some(out)
}

/// Shortest distance between the two tubes' axis segments.
fn seg_dist(c1: &Cyl, c2: &Cyl) -> f64 {
    // minimise |b1 + s a1 - b2 - t a2| over the box; check the interior solution and the edges
    let d = sub(c1.b, c2.b);
    let at = |s: f64, t: f64| norm(sub(add(d, mul(c1.a, s)), mul(c2.a, t)));
    let clamp_t = |s: f64| dot(add(d, mul(c1.a, s)), c2.a).clamp(0.0, c2.len);
    let clamp_s = |t: f64| (-dot(sub(d, mul(c2.a, t)), c1.a)).clamp(0.0, c1.len);
    let mut best = f64::MAX;
    for s in [0.0, c1.len] {
        best = best.min(at(s, clamp_t(s)));
    }
    for t in [0.0, c2.len] {
        best = best.min(at(clamp_s(t), t));
    }
    let c = dot(c1.a, c2.a);
    let den = 1.0 - c * c;
    if den > 1e-15 {
        let (e, f) = (dot(c1.a, d), dot(c2.a, d));
        let s = (c * f - e) / den;
        let t = (f - c * e) / den;
        if (0.0..=c1.len).contains(&s) && (0.0..=c2.len).contains(&t) {
            best = best.min(at(s, t));
        }
    }
    best
}

/// Curves where a ring's plane meets a sphere or a tube (circle / ellipse), skipping a ring's own
/// tube (the cap's rim is already the tube's end ellipse).
pub fn ring_sphere(view: &View, g: &Ring, s: &Sph) -> Vec<Curve> {
    let d = dot(sub(s.cw, g.c), g.n);
    if d.abs() >= s.rw {
        return vec![];
    }
    let rho = (s.rw * s.rw - d * d).sqrt();
    conic3(view, sub(s.cw, mul(g.n, d)), mul(g.e1, rho), mul(g.e2, rho), 1e-9 * s.rpx.max(1.0)).into_iter().collect()
}

pub fn ring_cyl(view: &View, g: &Ring, c: &Cyl) -> (Vec<Curve>, f64) {
    let an = dot(c.a, g.n);
    if an.abs() < 1e-12 {
        return (vec![], 0.0); // ponytail: plane parallel to the axis cuts the tube in lines; not needed yet
    }
    if norm(cross(g.n, c.a)) < 1e-9 && (norm(sub(g.c, c.b)) <= 1e-6 * c.r || norm(sub(g.c, c.apex())) <= 1e-6 * c.r) {
        return (vec![], 0.0); // the ring is this tube's cap
    }
    plane_cut(view, c, g.c, g.n)
}

/// The ellipse where the plane through `q` with normal `n` cuts the tube's (infinite) surface, and its
/// fuzz. Perspective: a plane nearly along the axis cuts it in a long ellipse that can reach the eye
/// plane (its image is then no ellipse); its part on the tube is traced as a chain instead.
pub fn plane_cut(view: &View, c: &Cyl, q: V3, n: V3) -> (Vec<Curve>, f64) {
    let an = dot(c.a, n);
    let s0 = dot(sub(q, c.b), n) / an;
    let u = sub(mul(c.e1, c.r), mul(c.a, c.r * dot(c.e1, n) / an));
    let v = sub(mul(c.e2, c.r), mul(c.a, c.r * dot(c.e2, n) / an));
    let m = add(c.b, mul(c.a, s0));
    if view.eye.is_some() {
        if let Some(k) = crate::persp::ellipse_image(view, m, u, v, 1e-9 * c.rpx.max(1.0)) {
            return (vec![k], 0.0);
        }
        // m + u cos φ + v sin φ is the tube's point at angle φ (u, v carry e1, e2): the tube's own rim
        // crossings become exact chain vertices
        let f = |t: f64| add(m, add(mul(u, t.cos()), mul(v, t.sin())));
        let mg = 1e-3 * c.r;
        let on = |t: f64| (-mg..=c.len + mg).contains(&c.axial(f(t)));
        let edge = |a: f64, b: f64| {
            let (mut lo, mut hi, want) = (a, b, on(a));
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if on(mid) == want { lo = mid } else { hi = mid }
            }
            if want { lo } else { hi }
        };
        const N: usize = 720;
        let (mut runs, mut start) = (vec![], on(0.0).then_some(0.0));
        for i in 0..N {
            let (a, b) = (TAU * i as f64 / N as f64, TAU * (i + 1) as f64 / N as f64);
            match (on(a), on(b)) {
                (false, true) => start = Some(edge(a, b)),
                (true, false) => runs.extend(start.take().map(|s| (s, edge(a, b)))),
                _ => {}
            }
        }
        runs.extend(start.map(|s| (s, TAU)));
        let mut out = vec![];
        for (a, b) in runs {
            let pts = branch(view, &f, &|_| 1.0, &|t| c.rim_at(view, t), a, b);
            out.extend(pts.windows(2).map(|w| (view.project(w[0]).0, view.project(w[1]).0)).filter(|(p, q)| p != q).map(|(a, b)| Curve::Seg { a, b }));
        }
        return (out, 2.0 * quartic_tol());
    }
    (conic3(view, m, u, v, 1e-9 * c.rpx.max(1.0)).into_iter().collect(), 0.0)
}

/// Image of the planar ellipse m + U cos θ + V sin θ (U, V world conjugate semi-axes). Perspective: None
/// if the ellipse reaches the eye plane. shortcut: such a curve (a tube's own circle, a ring's rim, where
/// a ring meets a sphere or two tubes meet: all near the shapes, which `svg::cut_scene` keeps in front of
/// the eye plane) is left out of the boundary candidates and the scene is reported approximate; trace it
/// as `plane_cut` does if a scene ever has one.
fn conic3(view: &View, m: V3, u: V3, v: V3, thin: f64) -> Option<Curve> {
    if view.eye.is_some() {
        let c = crate::persp::ellipse_image(view, m, u, v, thin);
        if c.is_none() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| eprintln!("approximate: perspective: a tube's or ring's circle reaches the eye plane (left out of the outlines)"));
        }
        return c;
    }
    let (c, _) = view.project(m);
    Some(Curve::conic_or_seg(c, view.dir2(u), view.dir2(v), thin))
}

/// Pixels per world unit at the nearest point (along the view) of the balls of radius `r` around `pts`,
/// in perspective (the largest scale over a shape: its tolerances in pixels).
fn px_scale(view: &View, pts: &[V3], r: f64) -> f64 {
    let e = view.eye.expect("perspective");
    let z = pts.iter().map(|&p| dot(sub(p, e), view.v)).fold(f64::MAX, f64::min) - r;
    1.0 / (view.psx * z.max(1e-3 * r))
}

/// Where the clip plane n·x = d (n a unit vector) cuts the tube's surface: an ellipse, or two lines
/// along the tube when the plane is parallel to its axis.
pub fn clip_cut(view: &View, c: &Cyl, n: V3, d: f64) -> Vec<Curve> {
    if dot(c.a, n).abs() > 1e-6 {
        return plane_cut(view, c, mul(n, d), n).0;
    }
    // shortcut: within 1e-6 of parallel the ellipse is taken as its two lines (off by under 1e-6 of
    // the tube's length); a nearly parallel plane farther off gives a long ellipse
    let (ea, eb, k) = (c.r * dot(c.e1, n), c.r * dot(c.e2, n), d - dot(c.b, n));
    let rr = ea.hypot(eb);
    if k.abs() >= rr {
        return vec![];
    }
    let (phi, w) = (eb.atan2(ea), (k / rr).acos());
    // the lines run a radius past each end, so tilted (mitred) ends are crossed too
    [phi - w, phi + w]
        .map(|f| Curve::Seg { a: view.project(c.surface_point(-c.r, f)).0, b: view.project(c.surface_point(c.len + c.r, f)).0 })
        .to_vec()
}

/// Skiagraph rule for tubes that continue each other (VMD's two half-bonds meeting at the bond
/// midpoint): same radius, a shared end point, axes collinear to printing precision. Their ends are
/// cut by one shared plane (the mirror plane between them) instead of square, so they join along
/// one ellipse with no sliver. Moves the ends by at most radius x angle (~1e-5 px).
pub fn mitre(tubes: &mut [&mut Cyl]) {
    let mut at: std::collections::HashMap<[u64; 3], Vec<(usize, usize)>> = Default::default();
    for (i, c) in tubes.iter().enumerate() {
        for e in 0..2 {
            // ponytail: end points matched on a 1e-9 grid (VMD prints shared points identically);
            // a point straddling a grid line is missed and keeps square ends.
            at.entry(c.ends[e].0.map(|x| (x * 1e9).round() as i64 as u64)).or_default().push((i, e));
        }
    }
    for list in at.values() {
        let &[(i, ei), (j, ej)] = list.as_slice() else { continue };
        let (di, dj) = (tubes[i].ends[ei].1, tubes[j].ends[ej].1);
        if i == j || (tubes[i].r - tubes[j].r).abs() > 1e-6 * tubes[i].r || dot(di, dj) > -(1.0 - 1e-6) {
            continue;
        }
        let m = unit(sub(di, dj));
        tubes[i].ends[ei].1 = m;
        tubes[j].ends[ej].1 = mul(m, -1.0);
    }
}

/// Polylines through the intersection of tube `c` with another quadric surface. `quad(x0)` gives
/// (A, B, C) of `A s² + 2 B s + C = 0` for the axial position s at the angle around `c` of its base
/// point x0 (`surface_point(0, phi)`). Each
/// branch is sampled adaptively until every chord is within `quartic_tol` px of the curve. Only
/// pieces with a vertex where `keep` holds are returned (the rest lies outside the finite tubes).
/// `rim(x)` changes sign where the curve crosses the other surface's rim (its normal turns
/// perpendicular to the view); there, and on `c`'s own rim (phi = 0, pi), the projected curve
/// touches a silhouette, so those points become exact chain vertices.
fn quartic_chains(view: &View, c: &Cyl, quad: &dyn Fn(V3) -> (f64, f64, f64), rim: &dyn Fn(V3) -> f64, keep: &dyn Fn(V3) -> bool) -> Vec<Curve> {
    // (A, B, C) at angle phi and the angle's (cosine, sine), for the point too: one of each per angle
    let quad_at = |phi: f64| {
        let cs = (phi.cos(), phi.sin());
        (quad(c.surface_point_cs(0.0, cs)), cs)
    };
    let disc = |phi: f64| {
        let ((a, b, cc), _) = quad_at(phi);
        b * b - a * cc
    };
    let point = |phi: f64, sign: f64| -> V3 {
        let ((a, b, cc), cs) = quad_at(phi);
        let s = (-b + sign * (b * b - a * cc).max(0.0).sqrt()) / a;
        c.surface_point_cs(s, cs)
    };
    const N: usize = 512;
    let ph: Vec<f64> = (0..=N).map(|i| TAU * i as f64 / N as f64).collect();
    let ds: Vec<f64> = ph.iter().map(|&p| disc(p)).collect();
    // branches as (phi start, phi end) intervals where the discriminant is >= 0
    let mut intervals: Vec<(f64, f64)> = vec![];
    if ds.iter().all(|&d| d >= 0.0) {
        intervals.push((0.0, TAU));
    } else {
        let start = ds.iter().position(|&d| d < 0.0).unwrap();
        let root = |mut lo: f64, mut hi: f64| {
            // disc(lo) < 0 <= disc(hi) or the reverse; bisect to the sign change
            let neg_lo = disc(lo) < 0.0;
            for _ in 0..80 {
                let mid = 0.5 * (lo + hi);
                if (disc(mid) < 0.0) == neg_lo { lo = mid } else { hi = mid }
            }
            if neg_lo { hi } else { lo }
        };
        let mut open: Option<f64> = None;
        for k in 0..N {
            let i = (start + k) % N;
            let (p0, p1) = (ph[i] + if i < start { TAU } else { 0.0 }, ph[i] + TAU / N as f64 + if i < start { TAU } else { 0.0 });
            let (d0, d1) = (ds[i], ds[i + 1]);
            if d0 < 0.0 && d1 >= 0.0 {
                open = Some(root(p0, p1));
            } else if d0 >= 0.0 && d1 < 0.0 {
                if let Some(a) = open.take() {
                    intervals.push((a, root(p0, p1)));
                }
            }
        }
    }
    let mut chains: Vec<Vec<V3>> = vec![];
    for &(a, b) in &intervals {
        let whole = a == 0.0 && b == TAU;
        let mut plus = branch(view, &|t| point(t, 1.0), rim, &|t| c.rim_at(view, t), a, b);
        let mut minus = branch(view, &|t| point(t, -1.0), rim, &|t| c.rim_at(view, t), b, a);
        if whole {
            chains.push(plus);
            chains.push(minus);
        } else {
            // + branch out, - branch back: one closed loop through the two branch points
            minus.remove(0);
            plus.append(&mut minus);
            chains.push(plus);
        }
    }
    let mut out = vec![];
    for ch in chains {
        for w in ch.windows(2) {
            if keep(w[0]) || keep(w[1]) {
                let (pa, _) = view.project(w[0]);
                let (pb, _) = view.project(w[1]);
                if pa != pb {
                    out.push(Curve::Seg { a: pa, b: pb });
                }
            }
        }
    }
    out
}

/// Adaptive samples of `f` over [a, b] (a may exceed b), chord error below `quartic_tol` px, with
/// the rim crossings (see `quartic_chains`; `own`: the tube's own rim by angle) as samples.
fn branch(view: &View, f: &dyn Fn(f64) -> V3, rim: &dyn Fn(V3) -> f64, own: &dyn Fn(f64) -> f64, a: f64, b: f64) -> Vec<V3> {
    // `xm`: f at the middle when the parent already has it (its quarter points are its halves' middles)
    #[allow(clippy::too_many_arguments)]
    fn rec(view: &View, f: &dyn Fn(f64) -> V3, t0: f64, x0: V3, t1: f64, x1: V3, xm: Option<V3>, depth: u32, out: &mut Vec<V3>) {
        let tm = 0.5 * (t0 + t1);
        let xm = xm.unwrap_or_else(|| f(tm));
        // perspective: a part behind the eye plane projects nowhere sensible (it is off the tubes: not kept)
        if view.eye.is_some_and(|e| [x0, xm, x1].iter().any(|&x| dot(sub(x, e), view.v) <= 0.0)) {
            out.push(x1);
            return;
        }
        let (p0, _) = view.project(x0);
        let (p1, _) = view.project(x1);
        let dev = |x: V3| {
            let (q, _) = view.project(x);
            seg_point_dist(p0, p1, q)
        };
        let (xa, xb) = (f(0.5 * (t0 + tm)), f(0.5 * (tm + t1)));
        let worst = dev(xm).max(dev(xa)).max(dev(xb));
        if worst > quartic_tol() && depth < 60 {
            rec(view, f, t0, x0, tm, xm, Some(xa), depth + 1, out);
            rec(view, f, tm, xm, t1, x1, Some(xb), depth + 1, out);
        } else {
            out.push(x1);
        }
    }
    const START: usize = 64;
    let h = |t: f64, x: V3| rim(x) * own(t);
    // the START + 1 interval ends, each evaluated once
    let ends: Vec<(f64, V3)> = (0..=START).map(|i| a + (b - a) * i as f64 / START as f64).map(|t| (t, f(t))).collect();
    let hs: Vec<f64> = ends.iter().map(|&(t, x)| h(t, x)).collect();
    let mut out = vec![f(a)];
    for i in 0..START {
        let (t0, t1) = (ends[i].0, ends[i + 1].0);
        let mut ts = vec![ends[i + 1]];
        if hs[i] * hs[i + 1] < 0.0 {
            let (mut lo, mut hi, mut hlo) = (t0, t1, hs[i]);
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                let hm = h(mid, f(mid));
                if hm * hlo > 0.0 {
                    (lo, hlo) = (mid, hm);
                } else {
                    hi = mid;
                }
            }
            let t = 0.5 * (lo + hi);
            ts.insert(0, (t, f(t)));
        }
        let mut tp = t0;
        for (t, x) in ts {
            let x0 = *out.last().unwrap();
            rec(view, f, tp, x0, t, x, None, 0, &mut out);
            tp = t;
        }
    }
    out
}

fn seg_point_dist(a: P2, b: P2, p: P2) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if l2 > 0.0 { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    crate::geom::dist(p, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simple orthographic view looking down -z: SVG x = world x, SVG y = -world y, 1 px = 1 unit.
    fn view() -> View {
        View {
            r: [1.0, 0.0, 0.0],
            u: [0.0, 1.0, 0.0],
            v: [0.0, 0.0, -1.0],
            ll: [0.0, 0.0, 100.0],
            psx: 1.0,
            psy: 1.0,
            xoff: 0.0,
            yoff: 0.0,
            w: 100,
            h: 100,
            eye: None,
        }
    }

    fn sph(view: &View, c: V3, r: f64) -> Sph {
        let (p, tc) = view.project(c);
        Sph { prim: 0, c: p, rpx: r, tc, rw: r, cw: c, sil: Curve::circle(p, r), near: tc - r, front_max: tc }
    }

    /// Distance from a world point's projection to the nearest of `curves` (dense sampling).
    fn dist_to(view: &View, curves: &[Curve], x: V3) -> f64 {
        let (p, _) = view.project(x);
        let mut best = f64::MAX;
        for c in curves {
            match c {
                Curve::Seg { a, b } => best = best.min(seg_point_dist(*a, *b, p)),
                _ => best = best.min(crate::geom::dist(c.point(c.param_of(p)), p)),
            }
        }
        best
    }

    #[test]
    fn coaxial_sphere_circle_is_exact() {
        let v = view();
        let c = Cyl::new(&v, 0, [0.0, 0.0, 0.0], [6.0, 2.0, 3.0], 0.5);
        let s = sph(&v, [0.0, 0.0, 0.0], 1.3);
        let (curves, _) = sphere_cyl(&v, &s, &c);
        assert_eq!(curves.len(), 1);
        let h = (1.3f64 * 1.3 - 0.25).sqrt();
        for k in 0..16 {
            let x = c.surface_point(h, TAU * k as f64 / 16.0);
            assert!(dist_to(&v, &curves, x) < 1e-6);
        }
    }

    #[test]
    fn equal_radius_meeting_tubes_give_planar_ellipse() {
        let v = view();
        let a = Cyl::new(&v, 0, [1.0, 2.0, 0.0], [5.0, 1.0, 2.0], 0.7);
        let b = Cyl::new(&v, 1, [1.0, 2.0, 0.0], [-1.0, 4.0, -2.0], 0.7);
        let (curves, _) = cyl_cyl(&v, &a, &b);
        assert_eq!(curves.len(), 1, "only the branch inside both tubes");
        // points on a with s1 = s2 lie on both surfaces and on the curve
        let mut checked = 0;
        for k in 0..64 {
            let phi = TAU * k as f64 / 64.0;
            // solve s on a so that the point is at distance r from b's axis
            for sgn in [1.0, -1.0] {
                let w = sub(a.surface_point(0.0, phi), b.b);
                let pa = sub(a.a, mul(b.a, dot(a.a, b.a)));
                let pw = sub(w, mul(b.a, dot(w, b.a)));
                let (qa, qb, qc) = (dot(pa, pa), dot(pw, pa), dot(pw, pw) - 0.49);
                let d = qb * qb - qa * qc;
                if d < 0.0 {
                    continue;
                }
                let s = (-qb + sgn * d.sqrt()) / qa;
                let x = a.surface_point(s, phi);
                let s2 = b.axial(x);
                if (s - s2).abs() < 1e-9 && s > 0.0 {
                    assert!(dist_to(&v, &curves, x) < 1e-6);
                    checked += 1;
                }
            }
        }
        assert!(checked > 10);
    }

    #[test]
    fn crossing_tubes_both_branches_lie_on_both_surfaces() {
        let v = view();
        // equal-radius tubes whose axes cross inside both (VMD Bonds overshoot the atom)
        let a = Cyl::new(&v, 0, [-6.0, 3.0, -2.0], [17.0, -47.0, 50.0], 2.2);
        let b = Cyl::new(&v, 1, [-0.9, 0.0, 3.3], [-62.0, -21.0, -34.0], 2.2);
        let m = add(a.b, mul(a.a, 0.2 * a.len)); // shift b so its axis passes through m at 20%
        let b = Cyl::new(&v, 1, sub(m, mul(b.a, 0.2 * b.len)), mul(b.a, b.len), 2.2);
        let (curves, _) = cyl_cyl(&v, &a, &b);
        assert_eq!(curves.len(), 2);
        for c in &curves {
            for k in 0..32 {
                let p = c.point(TAU * k as f64 / 32.0);
                // the eye ray through p must touch both surfaces at one common depth
                let da = [Surf::Front, Surf::Back].map(|f| a.roots(&v, p).map(|r| if f == Surf::Front { r.x_in } else { r.x_out }));
                let on_b = |x: V3| {
                    let w = sub(x, b.b);
                    (norm(sub(w, mul(b.a, dot(w, b.a)))) - 2.2).abs() < 1e-6
                };
                assert!(da.iter().flatten().any(|&x| on_b(x)), "point {p:?} not on both tubes");
            }
        }
    }

    #[test]
    fn quartic_chain_is_within_tolerance() {
        let v = view();
        // off-axis sphere through a tube, and two crossing tubes of different radii
        let c = Cyl::new(&v, 0, [0.0, 0.0, 0.0], [10.0, 1.0, 3.0], 1.0);
        let s = sph(&v, [4.0, 1.5, 1.0], 1.4);
        let c2 = Cyl::new(&v, 1, [5.0, -4.0, 0.0], [0.5, 8.0, 3.0], 0.6);
        for (curves, other) in [(sphere_cyl(&v, &s, &c).0, 0), (cyl_cyl(&v, &c, &c2).0, 1)] {
            assert!(curves.len() > 20);
            let mut n = 0;
            for k in 0..720 {
                let phi = TAU * k as f64 / 720.0;
                for sgn in [1.0, -1.0] {
                    let w = c.surface_point(0.0, phi);
                    let (qa, qb, qc) = if other == 0 {
                        let w = sub(w, s.cw);
                        (1.0, dot(w, c.a), dot(w, w) - s.rw * s.rw)
                    } else {
                        let w = sub(w, c2.b);
                        let pa = sub(c.a, mul(c2.a, dot(c.a, c2.a)));
                        let pw = sub(w, mul(c2.a, dot(w, c2.a)));
                        (dot(pa, pa), dot(pw, pa), dot(pw, pw) - c2.r * c2.r)
                    };
                    let d = qb * qb - qa * qc;
                    if d < 0.0 {
                        continue;
                    }
                    let x = c.surface_point((-qb + sgn * d.sqrt()) / qa, phi);
                    if (-1.0..=c.len + 1.0).contains(&c.axial(x)) && (other == 0 || (-0.6..=c2.len + 0.6).contains(&c2.axial(x))) {
                        // every true curve point is within the chord tolerance of the polyline
                        let dd = curves.iter().map(|cv| match cv {
                            Curve::Seg { a, b } => seg_point_dist(*a, *b, v.project(x).0),
                            _ => f64::MAX,
                        });
                        assert!(dd.fold(f64::MAX, f64::min) < 2.0 * quartic_tol());
                        n += 1;
                    }
                }
            }
            assert!(n > 100);
        }
    }

    #[test]
    fn licorice_joint_detected() {
        let v = view();
        let c = Cyl::new(&v, 0, [1.0, 1.0, 1.0], [2.0, 0.0, 1.0], 0.3);
        assert!(licorice_joint(&sph(&v, [1.0, 1.0, 1.0], 0.3), &c));
        assert!(licorice_joint(&sph(&v, [3.0, 1.0, 2.0], 0.3), &c));
        assert!(!licorice_joint(&sph(&v, [1.0, 1.0, 1.0], 0.5), &c));
    }
}
