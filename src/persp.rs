//! Spheres in perspective projection (build step 3, visibility stage V2; prior-art report 09 §1.2).
//!
//! Tachyon's perspective camera: the eye sits at `Center`, the image plane at distance 1 along the
//! view direction, and the ray through image point (x, y) (Tachyon pixel units) has direction
//! `lowleft + x·psx·right + y·psy·up` with `lowleft` taken relative to the eye (`Tracer::camera_ray`).
//!
//! Every outline we need is the image of a *cone of rays* from the eye: the rays tangent to a sphere
//! (its silhouette) or the rays through a 3D circle (a sphere–sphere intersection). A cone of rays is
//! a quadratic form `Dᵀ M D = 0` in the ray direction D. Putting `D = o + X·a + Y·b` (o = the image
//! point of a reference position, a and b = one SVG pixel to the right / down) gives a conic in SVG
//! pixels, and from that conic its own centre and axes. Note that the perspective image of a circle's
//! centre is *not* the centre of its image ellipse, which is why the centre is solved for here.

use crate::geom::{Curve, P2};
use crate::scene::V3;
use crate::svg::{Sph, Unsupported, View};

type M3 = [[f64; 3]; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mv(m: &M3, x: V3) -> V3 {
    [dot(m[0], x), dot(m[1], x), dot(m[2], x)]
}

/// Ray direction (not normalised; it lies on the image plane, distance 1 from the eye) through the
/// SVG point p, exactly as `Tracer::camera_ray` builds it.
pub fn ray(view: &View, p: P2) -> V3 {
    let (x, y) = view.to_tachyon(p);
    let mut d = [0.0; 3];
    for k in 0..3 {
        d[k] = view.ll[k] + x * view.psx * view.r[k] + y * view.psy * view.u[k];
    }
    d
}

/// The frame: the image with a 2 px margin (SVG corners). It closes the outline of a sphere whose image
/// has no end (it reaches behind the eye, or holds it); in a scene with such a sphere every region is
/// cut to it (`svg::build_pieces`).
pub fn frame(view: &View) -> (P2, P2) {
    ([-2.0, -2.0], [view.w as f64 + 2.0, view.h as f64 + 2.0])
}

/// The frame's four sides.
pub fn frame_sides(view: &View) -> Vec<Curve> {
    let (lo, hi) = frame(view);
    let c = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
    (0..4).map(|i| Curve::Seg { a: c[i], b: c[(i + 1) % 4] }).collect()
}

/// Whether SVG point p lies within the frame.
pub fn in_frame(view: &View, p: P2) -> bool {
    let (lo, hi) = frame(view);
    p[0] > lo[0] && p[0] < hi[0] && p[1] > lo[1] && p[1] < hi[1]
}

/// Perspective sphere: silhouette ellipse, bounding circle, depths. `None` if it is wholly behind
/// the eye (Tachyon never sees it). A sphere reaching behind the eye plane has one branch of a
/// hyperbola for its outline (the rays tangent to it in front of the eye), closed by the frame; a
/// sphere holding the eye is seen everywhere, from inside (its inner back wall), its outline the frame.
/// Both have the frame's circumcircle for bounding circle (`c`, `rpx`) and are shaded by tiles
/// (`Sph::cut`).
pub fn sphere(view: &View, prim: usize, ctr: V3, rad: f64) -> Result<Option<Sph>, Unsupported> {
    let eye = view.eye.expect("perspective view");
    let c = sub(ctr, eye);
    let cz = dot(c, view.v);
    if cz <= -rad {
        return Ok(None);
    }
    // tangent cone: (D·c)² = (|c|² − r²)|D|²
    let k = dot(c, c) - rad * rad;
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = c[i] * c[j] - if i == j { k } else { 0.0 };
        }
    }
    if cz <= rad {
        let (lo, hi) = frame(view);
        let fc = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
        let dist = dot(c, c).sqrt();
        let sides = frame_sides(view);
        let at = Sph { prim, c: fc, rpx: crate::geom::dist(lo, fc), tc: dist, rw: rad, cw: ctr, sil: sides[0].clone(), near: 0.0, front_max: 0.0 };
        if k <= 0.0 {
            // the eye inside: every ray meets the inner back wall, between r − |c| and r + |c| away
            return Ok(Some(Sph { near: rad - dist, front_max: dist + rad, ..at }));
        }
        let sil = cone_branch(view, &m, fc, &|d| dot(d, c) > 0.0)
            .ok_or_else(|| Unsupported("perspective: a sphere touching the eye plane (its outline a parabola)".into()))?;
        return Ok(Some(Sph { sil, near: dist - rad, front_max: k.sqrt(), ..at }));
    }
    let (pc, dist) = view.project(ctr);
    let sil = cone_conic(view, &m, pc, 0.0).ok_or_else(|| Unsupported("perspective: degenerate silhouette".into()))?;
    let Curve::Conic { c: e, u, v } = sil else { unreachable!() };
    let rpx = u[0].hypot(u[1]).max(v[0].hypot(v[1]));
    Ok(Some(Sph {
        prim,
        c: e,
        rpx,
        tc: dist,
        rw: rad,
        cw: ctr,
        sil,
        // every point of the sphere is at least |c| − r from the eye; every front-surface hit is at
        // most the tangent length sqrt(|c|² − r²) away (t_front · t_back = |c|² − r², t_front ≤ t_back)
        near: dist - rad,
        front_max: k.sqrt(),
    }))
}

/// Distance along the eye ray through p to the sphere's visible surface (None if the ray misses):
/// Tachyon's nearest root beyond the eye, the inner back wall when the eye is inside. Same arithmetic
/// as Tachyon's sphere intersection. A sphere the eye cuts (`Sph::cut`) ends at the frame.
pub fn front(view: &View, s: &Sph, p: P2) -> Option<f64> {
    if s.cut() && !in_frame(view, p) {
        return None;
    }
    let (f, b) = walls(view, s, p)?;
    [f, b].into_iter().find(|&t| t > crate::tachyon::EPSILON)
}

/// Distances along the eye ray through p to the sphere's front and far surfaces (None if it misses).
pub fn walls(view: &View, s: &Sph, p: P2) -> Option<(f64, f64)> {
    let mut d = ray(view, p);
    let len = 1.0 / dot(d, d).sqrt();
    d = [d[0] * len, d[1] * len, d[2] * len];
    let v = sub(s.cw, view.eye.unwrap());
    let b = dot(v, d);
    let disc = b * b + s.rw * s.rw - dot(v, v);
    if disc <= 0.0 { None } else { Some((b - disc.sqrt(), b + disc.sqrt())) }
}

/// Distance along the eye ray through p to the sphere's far (back) surface (None if the ray misses).
pub fn back(view: &View, s: &Sph, p: P2) -> Option<f64> {
    let mut d = ray(view, p);
    let len = 1.0 / dot(d, d).sqrt();
    d = [d[0] * len, d[1] * len, d[2] * len];
    let v = sub(s.cw, view.eye.unwrap());
    let b = dot(v, d);
    let disc = b * b + s.rw * s.rw - dot(v, v);
    if disc <= 0.0 { None } else { Some(b + disc.sqrt()) }
}

/// Image of the 3D circle with centre `m`, unit normal `n`, radius `rho` (world coordinates). A circle
/// reaching behind the eye plane: the branch of a hyperbola its part in front makes (`cone_branch`);
/// wholly behind it: None.
pub fn circle_image(view: &View, m: V3, n: V3, rho: f64, thin: f64) -> Option<Curve> {
    let q = sub(m, view.eye.unwrap());
    // the circle's points X have v·(X − eye) within qv ± reach
    let (qv, reach) = (dot(q, view.v), rho * (1.0 - dot(n, view.v).powi(2)).max(0.0).sqrt());
    if qv + reach <= 0.0 {
        return None;
    }
    // rays D through the circle: the point λD on the circle's plane (λ = h / n·D) is ρ from m:
    // |h D − (n·D) q|² = ρ² (n·D)²  =>  M = h² I − h (n qᵀ + q nᵀ) + (|q|² − ρ²) n nᵀ
    let h = dot(n, q);
    let g = dot(q, q) - rho * rho;
    let mut mm = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            mm[i][j] = -h * (n[i] * q[j] + q[i] * n[j]) + g * n[i] * n[j] + if i == j { h * h } else { 0.0 };
        }
    }
    if qv - reach <= 0.0 {
        // shortcut: the circle seen edge-on from the eye (h = 0) or touching the eye plane (a parabola)
        // gives None; measure-zero cases, the region then misses that curve
        let (lo, hi) = frame(view);
        return cone_branch(view, &mm, [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])], &|d| h * dot(n, d) > 0.0);
    }
    let (pm, _) = view.project(m);
    cone_conic(view, &mm, pm, thin)
}

/// The branch of the hyperbola where the cone `Dᵀ M D = 0` meets the image plane whose rays D `fwd`
/// accepts (the cone's nappe in front of the eye), as an arc reaching just out of the frame.
/// `pref`: an SVG point for conditioning. None if the section is no hyperbola.
pub fn cone_branch(view: &View, m: &M3, pref: P2, fwd: &dyn Fn(V3) -> bool) -> Option<Curve> {
    let o = ray(view, pref);
    let a = [view.psx * view.r[0], view.psx * view.r[1], view.psx * view.r[2]];
    let b = [-view.psy * view.u[0], -view.psy * view.u[1], -view.psy * view.u[2]];
    let (ma, mb, mo) = (mv(m, a), mv(m, b), mv(m, o));
    let (q00, q01, q11) = (dot(a, ma), dot(a, mb), dot(b, mb));
    let (l0, l1, f) = (dot(a, mo), dot(b, mo), dot(o, mo));
    let det = q00 * q11 - q01 * q01;
    if det == 0.0 {
        return None;
    }
    let x0 = (-l0 * q11 + l1 * q01) / det;
    let y0 = (-l1 * q00 + l0 * q01) / det;
    let f0 = f + l0 * x0 + l1 * y0;
    let th = 0.5 * (2.0 * q01).atan2(q00 - q11);
    let (cs, sn) = (th.cos(), th.sin());
    let la = q00 * cs * cs + 2.0 * q01 * cs * sn + q11 * sn * sn;
    let lb = q00 * sn * sn - 2.0 * q01 * cs * sn + q11 * cs * cs;
    let (sa, sb) = (-f0 / la, -f0 / lb);
    if !(sa * sb < 0.0 && sa.is_finite() && sb.is_finite()) {
        return None;
    }
    // X²/sa + Y²/sb = 1 in the principal axes: the branches lie along the axis with the positive term
    let (ea, eb) = ([cs, sn], [-sn, cs]);
    let ((pa, p), (na, n)) = if sa > 0.0 { ((ea, sa), (eb, -sb)) } else { ((eb, sb), (ea, -sa)) };
    let c = [pref[0] + x0, pref[1] + y0];
    let mut u = [pa[0] * p.sqrt(), pa[1] * p.sqrt()];
    let v = [na[0] * n.sqrt(), na[1] * n.sqrt()];
    if !fwd(ray(view, [c[0] + u[0], c[1] + u[1]])) {
        u = [-u[0], -u[1]];
    }
    // every point within the frame's circumcircle (centre fc, radius fr) has |s| below these bounds
    // (u ⊥ v, so |u + v| = |u − v|): |X − c| ≥ |u + v|·e^|s|/2 − |u − v|/2
    let (lo, hi) = frame(view);
    let fc = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let fr = crate::geom::dist(lo, fc);
    let uv = u[0].hypot(u[1]).hypot(v[0].hypot(v[1]));
    let smax = ((2.0 * (fr + crate::geom::dist(c, fc)) + uv) / uv).ln().max(1.0);
    let pt = |s: f64| [c[0] + u[0] * s.cosh() + v[0] * s.sinh(), c[1] + u[1] * s.cosh() + v[1] * s.sinh()];
    const N: usize = 4096;
    let step = 2.0 * smax / N as f64;
    let inside: Vec<usize> = (0..=N).filter(|&i| in_frame(view, pt(-smax + step * i as f64))).collect();
    let (s0, s1) = match (inside.first(), inside.last()) {
        (Some(&i), Some(&j)) => (-smax + step * (i as f64 - 1.0), -smax + step * (j as f64 + 1.0)),
        _ => (-1.0, 1.0), // wholly outside: any arc (it bounds nothing within the frame)
    };
    Some(Curve::Hyp { c, u, v, s0, s1 })
}

/// Image of the planar ellipse m + U cos θ + V sin θ (world; U, V conjugate semi-axes): a tube's
/// cross-section or end, a plane's cut through a tube. With n the plane's normal, q = m − eye, h = n·q
/// and U*, V* the dual basis of (U, V) in the plane, the rays D through the ellipse satisfy
/// (h U*·D − (n·D) U*·q)² + (h V*·D − (n·D) V*·q)² = (n·D)². A plane through the eye (h = 0) is seen
/// edge-on: the image is the segment between the extreme projected points. None if the ellipse
/// reaches the eye plane (its image is then no ellipse).
pub fn ellipse_image(view: &View, m: V3, u: V3, v: V3, thin: f64) -> Option<Curve> {
    let cr = |a: V3, b: V3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let n = cr(u, v);
    let n = [0, 1, 2].map(|k| n[k] / dot(n, n).sqrt());
    let (vn, nu) = (cr(v, n), cr(n, u));
    let (us, vs) = ([0, 1, 2].map(|k| vn[k] / dot(u, vn)), [0, 1, 2].map(|k| nu[k] / dot(v, nu)));
    let q = sub(m, view.eye.unwrap());
    let h = dot(n, q);
    let (g, k) = ([0, 1, 2].map(|i| h * us[i] - dot(us, q) * n[i]), [0, 1, 2].map(|i| h * vs[i] - dot(vs, q) * n[i]));
    let mut mm = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            mm[i][j] = g[i] * g[j] + k[i] * k[j] - n[i] * n[j];
        }
    }
    // the nearest point of the ellipse to the eye plane: v·(m − eye) − |(v·U, v·V)|
    if dot(q, view.v) - dot(u, view.v).hypot(dot(v, view.v)) <= 0.0 {
        return None;
    }
    let (pm, _) = view.project(m);
    if let Some(c) = cone_conic(view, &mm, pm, thin) {
        return Some(c);
    }
    // edge-on: the projected points lie on one line; its two extremes (shortcut: sampled every 0.1°;
    // only a plane exactly through the eye gets here)
    let pts: Vec<P2> = (0..3600)
        .map(|i| {
            let t = std::f64::consts::TAU * i as f64 / 3600.0;
            view.project([0, 1, 2].map(|k| m[k] + u[k] * t.cos() + v[k] * t.sin())).0
        })
        .collect();
    let far = |a: P2| pts.iter().copied().max_by(|p, q| crate::geom::dist(a, *p).total_cmp(&crate::geom::dist(a, *q))).unwrap();
    let a = far(pts[0]);
    Some(Curve::Seg { a, b: far(a) })
}

/// The ellipse where the cone `Dᵀ M D = 0` meets the image plane, as a `Curve` in SVG pixels.
/// `pref` is an SVG point near the ellipse (for conditioning). None if the section is not an ellipse.
/// With `thin > 0`, an ellipse thinner than `thin` pixels becomes a segment (circle seen edge-on).
pub fn cone_conic(view: &View, m: &M3, pref: P2, thin: f64) -> Option<Curve> {
    // D = o + X a + Y b, with (X, Y) the SVG offset from pref (SVG Y points down)
    let o = ray(view, pref);
    let a = [view.psx * view.r[0], view.psx * view.r[1], view.psx * view.r[2]];
    let b = [-view.psy * view.u[0], -view.psy * view.u[1], -view.psy * view.u[2]];
    let (ma, mb, mo) = (mv(m, a), mv(m, b), mv(m, o));
    // X² q00 + 2XY q01 + Y² q11 + 2X l0 + 2Y l1 + f = 0
    let (q00, q01, q11) = (dot(a, ma), dot(a, mb), dot(b, mb));
    let (l0, l1, f) = (dot(a, mo), dot(b, mo), dot(o, mo));
    let det = q00 * q11 - q01 * q01;
    if det == 0.0 {
        return None;
    }
    let x0 = (-l0 * q11 + l1 * q01) / det;
    let y0 = (-l1 * q00 + l0 * q01) / det;
    let f0 = f + l0 * x0 + l1 * y0;
    // principal axes of the quadratic part
    let th = 0.5 * (2.0 * q01).atan2(q00 - q11);
    let (cs, sn) = (th.cos(), th.sin());
    let la = q00 * cs * cs + 2.0 * q01 * cs * sn + q11 * sn * sn;
    let lb = q00 * sn * sn - 2.0 * q01 * cs * sn + q11 * cs * cs;
    let (sa, sb) = (-f0 / la, -f0 / lb);
    if !(sa > 0.0 && sb > 0.0) {
        return None;
    }
    let (sa, sb) = (sa.sqrt(), sb.sqrt());
    let c = [pref[0] + x0, pref[1] + y0];
    let u = [cs * sa, sn * sa];
    let v = [-sn * sb, cs * sb];
    if thin > 0.0 { Some(Curve::conic_or_seg(c, u, v, thin)) } else { Some(Curve::Conic { c, u, v }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A perspective view: eye at (0, 0, 10) looking down −z, 400×300 pixels, zoom 1.
    fn view() -> View {
        let (w, h) = (400u32, 300u32);
        let px = (w as f64 / h as f64) / 1.0;
        let py = 1.0;
        let (r, u, v) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]);
        let ll = [-0.5 * px, -0.5 * py, -1.0];
        View {
            r,
            u,
            v,
            ll,
            psx: px / w as f64,
            psy: py / h as f64,
            xoff: 0.5,
            yoff: h as f64 - 0.5,
            w,
            h,
            eye: Some([0.0, 0.0, 10.0]),
        }
    }

    fn unit(d: V3) -> V3 {
        let l = dot(d, d).sqrt();
        [d[0] / l, d[1] / l, d[2] / l]
    }

    #[test]
    fn silhouette_rays_are_tangent_to_the_sphere() {
        let vw = view();
        for (ctr, rad) in [([1.3, -0.7, -2.0], 1.5), ([0.0, 0.0, 0.0], 1.0), ([-6.0, 3.0, -20.0], 0.4)] {
            let Ok(Some(s)) = sphere(&vw, 0, ctr, rad) else { panic!() };
            let c = sub(ctr, vw.eye.unwrap());
            for i in 0..64 {
                let p = s.sil.point(i as f64 * 0.1);
                let d = unit(ray(&vw, p));
                // distance from the sphere centre to the ray equals the radius
                let b = dot(c, d);
                let dist = (dot(c, c) - b * b).sqrt();
                assert!((dist - rad).abs() < 1e-9 * rad, "ray misses tangency by {}", dist - rad);
                assert!(b > 0.0);
            }
            // the silhouette is not centred on the projected centre (except on the view axis)
            let (pc, _) = vw.project(ctr);
            let off = crate::geom::dist(pc, s.c);
            if ctr[0] != 0.0 {
                assert!(off > 1e-3, "offset {off}");
            }
            // just inside / outside the silhouette the ray hits / misses
            let Curve::Conic { c: e, u, .. } = s.sil else { panic!() };
            let pin = [e[0] + 0.999 * u[0], e[1] + 0.999 * u[1]];
            let pout = [e[0] + 1.001 * u[0], e[1] + 1.001 * u[1]];
            assert!(front(&vw, &s, pin).is_some() && front(&vw, &s, pout).is_none());
        }
    }

    #[test]
    fn ellipse_image_passes_through_projected_ellipse_points() {
        let vw = view();
        // a tilted ellipse given by non-orthogonal conjugate semi-axes (a plane's cut through a tube)
        let (m, u, v) = ([0.8, -0.4, -1.0], [0.9, 0.2, -0.5], [-0.3, 0.6, 0.4]);
        let Some(Curve::Conic { c, u: a, v: b }) = ellipse_image(&vw, m, u, v, 1e-9) else { panic!("expected an ellipse") };
        for i in 0..50 {
            let t = i as f64 * 0.13;
            let (p, _) = vw.project([0, 1, 2].map(|k| m[k] + u[k] * t.cos() + v[k] * t.sin()));
            let q = [p[0] - c[0], p[1] - c[1]];
            let det = a[0] * b[1] - a[1] * b[0];
            let s = [(q[0] * b[1] - q[1] * b[0]) / det, (a[0] * q[1] - a[1] * q[0]) / det];
            assert!((s[0].hypot(s[1]) - 1.0).abs() < 1e-9, "off the ellipse: {}", s[0].hypot(s[1]));
        }
        // reaching the eye plane (z = 10): no ellipse
        assert!(ellipse_image(&vw, [0.0, 0.0, 9.5], [1.0, 0.0, 0.0], [0.0, 0.0, 0.8], 1e-9).is_none());
        // a plane through the eye: seen edge-on, a segment
        assert!(matches!(ellipse_image(&vw, [1.0, 0.0, 0.0], [0.5, 0.0, 0.0], [0.0, 0.0, 0.5], 1e-9), Some(Curve::Seg { .. })));
    }

    #[test]
    fn circle_image_passes_through_projected_circle_points() {
        let vw = view();
        let m = [0.8, -0.4, -1.0];
        let n = unit([0.3, 0.5, 0.8]);
        let e1 = unit([n[1], -n[0], 0.0]);
        let e2 = [n[1] * e1[2] - n[2] * e1[1], n[2] * e1[0] - n[0] * e1[2], n[0] * e1[1] - n[1] * e1[0]];
        let rho = 1.2;
        let curve = circle_image(&vw, m, n, rho, 1e-9).unwrap();
        let Curve::Conic { c, u, v } = curve.clone() else { panic!("expected an ellipse") };
        let (pm, _) = vw.project(m);
        assert!(crate::geom::dist(pm, c) > 1e-3, "image centre must differ from the projected centre");
        for i in 0..50 {
            let t = i as f64 * 0.13;
            let x = [0, 1, 2].map(|k| m[k] + rho * (t.cos() * e1[k] + t.sin() * e2[k]));
            let (p, _) = vw.project(x);
            // p lies on the ellipse: its coordinates in the (u, v) frame have unit length
            let q = [p[0] - c[0], p[1] - c[1]];
            let det = u[0] * v[1] - u[1] * v[0];
            let s = [(q[0] * v[1] - q[1] * v[0]) / det, (u[0] * q[1] - u[1] * q[0]) / det];
            assert!((s[0].hypot(s[1]) - 1.0).abs() < 1e-9, "off the ellipse: {}", s[0].hypot(s[1]));
        }
    }
}
