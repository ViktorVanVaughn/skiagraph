//! Bit-exact re-implementation of the parts of the Tachyon ray tracer that VMD scene files use.
//!
//! This is Skiagraph's *target function* and visibility *oracle* (ADR 0007 step 0): for any image
//! point it says which primitive is visible and what colour Tachyon gives it.
//!
//! Translated from Tachyon 0.99.5 (`src/shade.c`, `camera.c`, `trace.c`, `util.c`, `sphere.c`,
//! `cylinder.c`, `triangle.c`, `ring.c`, `plane.c`, `intersect.c`, `light.c`, `texture.c`):
//!
//!   Copyright (c) 1994-2022 John E. Stone. All rights reserved.
//!   SPDX-License-Identifier: BSD-3-Clause
//!   (Redistribution in source/binary form must retain this notice and disclaimer;
//!    the author's name may not be used to endorse derived products.)
//!
//! Precision follows Tachyon's default build: geometry and intermediate maths in `f64` (`flt`),
//! colours and material fields in `f32`, with the same widening/narrowing points.

use crate::scene::*;

pub const EPSILON: f64 = 0.000000001;
const SPEPSILON: f64 = 0.000000001;
const FHUGE: f64 = 1e18;
pub const MINCONTRIB: f64 = 0.001959;
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Col {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Col {
    fn from(c: Rgb) -> Col {
        Col { r: c.r, g: c.g, b: c.b }
    }
}

#[inline]
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
fn raypnt(o: V3, d: V3, t: f64) -> V3 {
    [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t]
}
fn vnorm(a: &mut V3) {
    let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if len != 0.0 {
        a[0] /= len;
        a[1] /= len;
        a[2] /= len;
    }
}
fn vlength(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

// ---------------------------------------------------------------- random numbers (util.c)

// ---------------------------------------------------------------- primitives

#[derive(Clone, Debug)]
pub enum Prim {
    Sphere { ctr: V3, rad: f64 },
    FCyl { ctr: V3, axis: V3, rad: f64 },
    Tri { v0: V3, e1: V3, e2: V3 },
    STri { v0: V3, e1: V3, e2: V3, n: [V3; 3], colors: Option<[Col; 3]> },
    Ring { ctr: V3, norm: V3, inner: f64, outer: f64 },
    Plane { ctr: V3, norm: V3 },
}

pub struct PrimObj {
    pub prim: Prim,
    pub tex: usize,
    pub clip: Option<usize>,
    /// Index of the source object in `Scene::objects`.
    pub obj: usize,
}

impl PrimObj {
    pub(crate) fn bbox(&self) -> Option<([f64; 3], [f64; 3])> {
        match &self.prim {
            Prim::Sphere { ctr, rad } => Some((
                [ctr[0] - rad, ctr[1] - rad, ctr[2] - rad],
                [ctr[0] + rad, ctr[1] + rad, ctr[2] + rad],
            )),
            Prim::FCyl { ctr, axis, rad } => {
                let apex = [ctr[0] + axis[0], ctr[1] + axis[1], ctr[2] + axis[2]];
                let mut lo = [0.0; 3];
                let mut hi = [0.0; 3];
                for k in 0..3 {
                    lo[k] = ctr[k].min(apex[k]) - rad;
                    hi[k] = ctr[k].max(apex[k]) + rad;
                }
                Some((lo, hi))
            }
            Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => {
                let v1 = [v0[0] + e1[0], v0[1] + e1[1], v0[2] + e1[2]];
                let v2 = [v0[0] + e2[0], v0[1] + e2[1], v0[2] + e2[2]];
                let mut lo = [0.0; 3];
                let mut hi = [0.0; 3];
                for k in 0..3 {
                    lo[k] = v0[k].min(v1[k]).min(v2[k]);
                    hi[k] = v0[k].max(v1[k]).max(v2[k]);
                }
                Some((lo, hi))
            }
            Prim::Ring { ctr, outer, .. } => Some((
                [ctr[0] - outer, ctr[1] - outer, ctr[2] - outer],
                [ctr[0] + outer, ctr[1] + outer, ctr[2] + outer],
            )),
            Prim::Plane { .. } => None,
        }
    }

    /// Calls `add(t)` for each ray–surface root, in Tachyon's order.
    fn intersect(&self, o: V3, d: V3, add: &mut impl FnMut(f64)) {
        match &self.prim {
            Prim::Sphere { ctr, rad } => {
                let v = sub(*ctr, o);
                let b = dot(v, d);
                let temp = dot(v, v);
                let mut disc = b * b + rad * rad - temp;
                if disc <= 0.0 {
                    return;
                }
                disc = disc.sqrt();
                let t2 = b + disc;
                if t2 <= SPEPSILON {
                    return;
                }
                add(t2);
                let t1 = b - disc;
                if t1 > SPEPSILON {
                    add(t1);
                }
            }
            Prim::FCyl { ctr, axis, rad } => {
                let rc = sub(o, *ctr);
                let mut n = cross(d, *axis);
                let mut ln = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if ln == 0.0 {
                    return;
                }
                n[0] /= ln;
                n[1] /= ln;
                n[2] /= ln;
                let dd = dot(rc, n).abs();
                if dd <= *rad {
                    let oo = cross(rc, *axis);
                    let mut t = dot(oo, n);
                    t = -t / ln;
                    let mut oo = cross(n, *axis);
                    ln = (oo[0] * oo[0] + oo[1] * oo[1] + oo[2] * oo[2]).sqrt();
                    oo[0] /= ln;
                    oo[1] /= ln;
                    oo[2] /= ln;
                    let s = dot(d, oo);
                    let s = ((rad * rad - dd * dd).sqrt() / s).abs();
                    let mut ctmp4 = *axis;
                    vnorm(&mut ctmp4);
                    let tmp3 = dot(*axis, *axis);
                    let tin = t - s;
                    let hit = raypnt(o, d, tin);
                    let tmp = dot(sub(hit, *ctr), ctmp4);
                    if tmp > 0.0 && tmp < tmp3.sqrt() {
                        add(tin);
                    }
                    let tout = t + s;
                    let hit = raypnt(o, d, tout);
                    let tmp = dot(sub(hit, *ctr), ctmp4);
                    if tmp > 0.0 && tmp < tmp3.sqrt() {
                        add(tout);
                    }
                }
            }
            Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => {
                let pvec = cross(d, *e2);
                let det = dot(*e1, pvec);
                if det > -EPSILON && det < EPSILON {
                    return;
                }
                let inv_det = 1.0 / det;
                let tvec = sub(o, *v0);
                let u = dot(tvec, pvec) * inv_det;
                if !(0.0..=1.0).contains(&u) {
                    return;
                }
                let qvec = cross(tvec, *e1);
                let v = dot(d, qvec) * inv_det;
                if v < 0.0 || u + v > 1.0 {
                    return;
                }
                add(dot(*e2, qvec) * inv_det);
            }
            Prim::Ring { ctr, norm, inner, outer } => {
                let dd = -dot(*ctr, *norm);
                let mut t = -(dd + dot(*norm, o));
                let td = dot(*norm, d);
                if td != 0.0 {
                    t /= td;
                    if t >= 0.0 {
                        let hit = raypnt(o, d, t);
                        let pnt = sub(hit, *ctr);
                        let td = dot(pnt, pnt).sqrt();
                        if td > *inner && td < *outer {
                            add(t);
                        }
                    }
                }
            }
            Prim::Plane { ctr, norm } => {
                let dd = -dot(*norm, *ctr);
                let t = -(dd + dot(*norm, o));
                let td = dot(*norm, d);
                if td != 0.0 {
                    let t = t / td;
                    if t > 0.0 {
                        add(t);
                    }
                }
            }
        }
    }

}

// ---------------------------------------------------------------- tie order (intersect.c, grid.c)

const BOUNDTHRESH: usize = 16;

/// Tachyon keeps the first object it tests among those at exactly the same distance (strict `<`), so
/// identical primitives are settled by its test order: unbounded ones (planes) first, newest first; then
/// the bounded ones: newest first in a scene of at most 16 (its object list, prepended to), and in a
/// larger one through its uniform grid (`engrid_scene`), whose cells list their objects oldest first,
/// except that a cell with more than 16 objects wholly inside it hands those to a sub-grid that is tested
/// first and lists them newest first (`engrid_cell`). Returns each primitive's place: lower wins a tie.
/// `lights`: point lights, which count as bounded objects. Measured on the real Tachyon and on VMD
/// 1.9.4a57's internal one: 16 objects, the last copy wins; 17, the first.
/// shortcut: an object spanning several cells is tested in the first cell the ray crosses (mailbox), which
/// can put it ahead of a tied object listed before it in the hit's cell; exact for identical copies.
fn tie_order(prims: &[PrimObj], lights: usize) -> Vec<u64> {
    let n = prims.len() as u64;
    let boxes: Vec<_> = prims.iter().map(|p| p.bbox()).collect();
    let bounded = boxes.iter().filter(|b| b.is_some()).count() + lights;
    // places: unbounded (0), sub-grid (1), grid or short list (2), then by age
    let place = |class: u64, i: usize, newest_first: bool| class << 40 | if newest_first { n - 1 - i as u64 } else { i as u64 };
    if bounded <= BOUNDTHRESH {
        return (0..prims.len()).map(|i| place(if boxes[i].is_some() { 2 } else { 0 }, i, true)).collect();
    }
    let size = (4.0 * bounded as f64).powf(1.0 / 3.0) as usize; // grid.c's cbrt is pow(x, 1/3)
    let (mut gmin, mut gmax) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for (lo, hi) in boxes.iter().flatten() {
        for k in 0..3 {
            gmin[k] = gmin[k].min(lo[k]);
            gmax[k] = gmax[k].max(hi[k]);
        }
    }
    let vox = [0, 1, 2].map(|k| (gmax[k] - gmin[k]) / size as f64);
    let cell = |p: &[f64; 3]| [0, 1, 2].map(|k| (((p[k] - gmin[k]) / vox[k]) as usize).min(size - 1));
    // objects wholly inside each cell they are listed in (`cellbound`)
    let mut inside: std::collections::HashMap<[usize; 3], Vec<usize>> = Default::default();
    for (i, b) in boxes.iter().enumerate() {
        let Some((lo, hi)) = b else { continue };
        let (a, z) = (cell(lo), cell(hi));
        if (0..3).any(|k| z[k] > a[k] + 1) {
            continue; // wider than a cell
        }
        for x in a[0]..=z[0] {
            for y in a[1]..=z[1] {
                for w in a[2]..=z[2] {
                    let c = [x, y, w];
                    let cmin = [0, 1, 2].map(|k| c[k] as f64 * vox[k] + gmin[k]);
                    if (0..3).all(|k| lo[k] >= cmin[k] && hi[k] <= cmin[k] + vox[k]) {
                        inside.entry(c).or_default().push(i);
                    }
                }
            }
        }
    }
    let mut sub = vec![false; prims.len()];
    for v in inside.values().filter(|v| v.len() > BOUNDTHRESH) {
        v.iter().for_each(|&i| sub[i] = true);
    }
    (0..prims.len())
        .map(|i| match boxes[i] {
            None => place(0, i, true),
            Some(_) if sub[i] => place(1, i, true),
            Some(_) => place(2, i, false),
        })
        .collect()
}

// ---------------------------------------------------------------- BVH (acceleration only)

struct Node {
    lo: [f64; 3],
    hi: [f64; 3],
    /// Leaf: `count > 0`, primitives `first..first+count` of `order`.
    /// Inner: left child is the next node (depth-first layout), right child is `first`.
    first: u32,
    count: u32,
}

struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
    unbounded: Vec<u32>,
    /// every box coordinate under 1e100 in size (`visit`'s fast box test)
    small: bool,
}

impl Bvh {
    fn build(prims: &[PrimObj]) -> Bvh {
        let mut boxes = vec![];
        let mut unbounded = vec![];
        for (i, p) in prims.iter().enumerate() {
            match p.bbox() {
                Some(b) => boxes.push((i as u32, b.0, b.1)),
                None => unbounded.push(i as u32),
            }
        }
        let mut nodes = vec![];
        let mut order = Vec::with_capacity(boxes.len());
        if !boxes.is_empty() {
            Self::split(&mut boxes[..], &mut nodes, &mut order, 0);
        }
        let small = nodes.first().is_some_and(|n: &Node| (0..3).all(|k| n.lo[k].abs() < 1e100 && n.hi[k].abs() < 1e100));
        Bvh { nodes, order, unbounded, small }
    }

    fn split(items: &mut [(u32, [f64; 3], [f64; 3])], nodes: &mut Vec<Node>, order: &mut Vec<u32>, depth: u32) -> usize {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for (_, a, b) in items.iter() {
            for k in 0..3 {
                lo[k] = lo[k].min(a[k]);
                hi[k] = hi[k].max(b[k]);
            }
        }
        let me = nodes.len();
        nodes.push(Node { lo, hi, first: 0, count: 0 });
        if items.len() <= 4 {
            nodes[me].first = order.len() as u32;
            nodes[me].count = items.len() as u32;
            order.extend(items.iter().map(|x| x.0));
            return me;
        }
        // surface-area heuristic; median splits below depth 64 keep the tree within visit's 128-entry
        // stack (a stack holds at most depth + 1 nodes; median splits add log2(n) levels)
        let mid = (depth < 64).then(|| Self::sah_partition(items)).flatten().unwrap_or_else(|| {
            let axis = (0..3).max_by(|&a, &b| (hi[a] - lo[a]).total_cmp(&(hi[b] - lo[b]))).unwrap();
            let mid = items.len() / 2;
            items.select_nth_unstable_by(mid, |x, y| (x.1[axis] + x.2[axis]).total_cmp(&(y.1[axis] + y.2[axis])));
            mid
        });
        let (l, r) = items.split_at_mut(mid);
        Self::split(l, nodes, order, depth + 1);
        let right = Self::split(r, nodes, order, depth + 1);
        nodes[me].first = right as u32;
        me
    }

    /// Binned surface-area-heuristic split (Wald 2007; 16 bins of the box centres on each axis): the
    /// split with the least (box area x primitive count) summed over both sides. Partitions `items`
    /// and returns the size of the first part; None when all box centres coincide.
    fn sah_partition(items: &mut [(u32, [f64; 3], [f64; 3])]) -> Option<usize> {
        const NB: usize = 16;
        type It = (u32, [f64; 3], [f64; 3]);
        let area = |l: [f64; 3], h: [f64; 3]| {
            let d = [0, 1, 2].map(|k| (h[k] - l[k]).max(0.0));
            d[0] * d[1] + d[1] * d[2] + d[2] * d[0]
        };
        let cen = |x: &It, k: usize| 0.5 * (x.1[k] + x.2[k]);
        let bin = |x: &It, k: usize, cl: f64, ch: f64| (((cen(x, k) - cl) / (ch - cl) * NB as f64) as usize).min(NB - 1);
        let grow = |l: &mut [f64; 3], h: &mut [f64; 3], bl: [f64; 3], bh: [f64; 3]| {
            for a in 0..3 {
                l[a] = l[a].min(bl[a]);
                h[a] = h[a].max(bh[a]);
            }
        };
        let mut best: Option<(f64, usize, usize, f64, f64)> = None; // cost, axis, first bin on the right, centre range
        for k in 0..3 {
            let (cl, ch) = items.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(cen(x, k)), b.max(cen(x, k))));
            if !(ch > cl) {
                continue;
            }
            let (mut cnt, mut bl, mut bh) = ([0usize; NB], [[f64::INFINITY; 3]; NB], [[f64::NEG_INFINITY; 3]; NB]);
            for x in items.iter() {
                let b = bin(x, k, cl, ch);
                cnt[b] += 1;
                grow(&mut bl[b], &mut bh[b], x.1, x.2);
            }
            // area and count of bins b.. for every b, then sweep the left side
            let mut right = [(0.0, 0usize); NB];
            let (mut l, mut h, mut n) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3], 0);
            for b in (1..NB).rev() {
                grow(&mut l, &mut h, bl[b], bh[b]);
                n += cnt[b];
                right[b] = (area(l, h), n);
            }
            let (mut l, mut h, mut n) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3], 0);
            for b in 0..NB - 1 {
                grow(&mut l, &mut h, bl[b], bh[b]);
                n += cnt[b];
                let (ra, rn) = right[b + 1];
                if n == 0 || rn == 0 {
                    continue;
                }
                let cost = area(l, h) * n as f64 + ra * rn as f64;
                if best.is_none_or(|bb| cost < bb.0) {
                    best = Some((cost, k, b + 1, cl, ch));
                }
            }
        }
        let (_, k, rb, cl, ch) = best?;
        // the same bin test as the counts, so neither side is empty
        let mut i = 0;
        for j in 0..items.len() {
            if bin(&items[j], k, cl, ch) < rb {
                items.swap(i, j);
                i += 1;
            }
        }
        Some(i)
    }

    /// Visit every primitive whose box the ray may hit within `maxdist` (callers re-check t).
    fn visit(&self, o: V3, d: V3, maxdist: impl Fn() -> f64, mut f: impl FnMut(u32)) {
        for &i in &self.unbounded {
            f(i);
        }
        if self.nodes.is_empty() {
            return;
        }
        // with no huge or tiny numbers (every real ray) no box test below can make a NaN, and the
        // plain comparisons of `walk_fast` decide exactly as the NaN-proof ones of `walk`
        if self.small && (0..3).all(|k| o[k].abs() < 1e100 && (d[k] == 0.0 || d[k].abs() > 1e-100)) {
            self.walk_fast(o, d, maxdist, f);
        } else {
            self.walk(o, d, maxdist, f);
        }
    }

    /// `walk` for rays and boxes under 1e100 (`visit`): each box's entry and exit distances found
    /// from the face the ray meets first, and the three axes compared once at the end. Same boxes
    /// culled, same order.
    fn walk_fast(&self, o: V3, d: V3, maxdist: impl Fn() -> f64, mut f: impl FnMut(u32)) {
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let neg = [inv[0] < 0.0, inv[1] < 0.0, inv[2] < 0.0];
        let mut stack = [0u32; 128];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp] as usize;
            let n = &self.nodes[ni];
            let (mut t0, mut t1, mut out) = (0.0f64, maxdist(), false);
            for k in 0..3 {
                if d[k] == 0.0 {
                    out |= o[k] < n.lo[k] || o[k] > n.hi[k];
                    continue;
                }
                let (near, far) = if neg[k] { (n.hi[k], n.lo[k]) } else { (n.lo[k], n.hi[k]) };
                let a = (near - o[k]) * inv[k];
                let b = (far - o[k]) * inv[k];
                // a <= b here, so max(|a|, |b|) = max(-a, b)
                let eps = 1e-9 * (if -a > b { -a } else { b } + 1.0);
                let (x, y) = (a - eps, b + eps);
                if x > t0 {
                    t0 = x;
                }
                if y < t1 {
                    t1 = y;
                }
                if t0 > t1 {
                    break;
                }
            }
            if out || t0 > t1 {
                continue;
            }
            if n.count > 0 {
                for &p in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    f(p);
                }
            } else {
                stack[sp] = n.first;
                stack[sp + 1] = (ni + 1) as u32;
                sp += 2;
            }
        }
    }

    fn walk(&self, o: V3, d: V3, maxdist: impl Fn() -> f64, mut f: impl FnMut(u32)) {
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut stack = [0u32; 128];
        let mut sp = 1usize;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp] as usize;
            let n = &self.nodes[ni];
            let mut t0 = 0.0f64;
            let mut t1 = maxdist();
            let mut miss = false;
            for k in 0..3 {
                if d[k] == 0.0 {
                    if o[k] < n.lo[k] || o[k] > n.hi[k] {
                        miss = true;
                        break;
                    }
                    continue;
                }
                let mut a = (n.lo[k] - o[k]) * inv[k];
                let mut b = (n.hi[k] - o[k]) * inv[k];
                if a > b {
                    std::mem::swap(&mut a, &mut b);
                }
                // widen slightly so roundoff never culls a primitive Tachyon would test
                let eps = 1e-9 * (a.abs().max(b.abs()) + 1.0);
                t0 = t0.max(a - eps);
                t1 = t1.min(b + eps);
                if t0 > t1 {
                    miss = true;
                    break;
                }
            }
            if miss {
                continue;
            }
            if n.count > 0 {
                for &p in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    f(p);
                }
            } else {
                stack[sp] = n.first;
                stack[sp + 1] = (ni + 1) as u32;
                sp += 2;
            }
        }
    }
}

// ---------------------------------------------------------------- tracer

thread_local! {
    /// Skiagraph: per ray direction (`blocker_slot`), the primitive that blocked this thread's last shadow
    /// ray in it (`Tracer::intersect`); a hint only
    static BLOCKER: [std::cell::Cell<usize>; 256] = const { [const { std::cell::Cell::new(usize::MAX) }; 256] };
}

/// Slot of direction d in `BLOCKER`. Tachyon's occlusion rays take the same few hundred directions at
/// every pixel, and a light's shadow rays one each, so neighbouring points meet the same blockers.
fn blocker_slot(d: V3) -> usize {
    let h = d[0].to_bits() ^ d[1].to_bits().rotate_left(21) ^ d[2].to_bits().rotate_left(42);
    (h.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as usize
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RayKind {
    Regular,
    Shadow,
}

#[derive(Clone)]
struct Ray {
    o: V3,
    d: V3,
    maxdist: f64,
}

struct Hits {
    num: u32,
    t: f64,
    obj: u32,
    shadowfilter: f64,
}

pub struct Tracer<'a> {
    pub sc: &'a Scene,
    pub prims: Vec<PrimObj>,
    /// each primitive's place in Tachyon's test order: the lower one wins an exact depth tie (`tie_order`)
    pub tie: Vec<u64>,
    bvh: Bvh,
    // camera
    pub rightvec: V3,
    pub upvec: V3,
    pub viewvec: V3,
    pub lowleft: V3,
    pub iplaneright: V3,
    pub iplaneup: V3,
    pub px: f64,
    pub py: f64,
}

impl<'a> Tracer<'a> {
    pub fn new(sc: &'a Scene) -> Tracer<'a> {
        let mut prims = vec![];
        for (i, o) in sc.objects.iter().enumerate() {
            let prim = match &o.shape {
                Shape::Sphere { center, rad } => Prim::Sphere { ctr: *center, rad: *rad },
                Shape::FCylinder { base, axis, rad } => Prim::FCyl { ctr: *base, axis: *axis, rad: *rad },
                Shape::Tri { v } | Shape::STri { v, .. } => {
                    let e1 = sub(v[1], v[0]);
                    let e2 = sub(v[2], v[0]);
                    let e3 = sub(v[2], v[1]);
                    // degenerate triangles are dropped at creation (triangle.c)
                    if vlength(e1) < EPSILON || vlength(e2) < EPSILON || vlength(e3) < EPSILON {
                        continue;
                    }
                    match &o.shape {
                        Shape::STri { n, colors, .. } => Prim::STri {
                            v0: v[0],
                            e1,
                            e2,
                            n: *n,
                            colors: colors.map(|c| c.map(Col::from)),
                        },
                        _ => Prim::Tri { v0: v[0], e1, e2 },
                    }
                }
                Shape::Ring { center, normal, inner, outer } => {
                    Prim::Ring { ctr: *center, norm: *normal, inner: *inner, outer: *outer }
                }
                Shape::Plane { center, normal } => {
                    let mut n = *normal;
                    vnorm(&mut n);
                    Prim::Plane { ctr: *center, norm: n }
                }
            };
            prims.push(PrimObj { prim, tex: o.tex, clip: o.clip, obj: i });
        }
        let bvh = Bvh::build(&prims);
        let tie = tie_order(&prims, sc.lights.iter().filter(|l| matches!(l, Light::Point { .. })).count());

        // cameraposition()
        let cam = &sc.camera;
        let mut right = cross(cam.updir, cam.viewdir);
        vnorm(&mut right);
        let mut up = cross(cam.viewdir, right);
        vnorm(&mut up);
        let mut view = cam.viewdir;
        vnorm(&mut view);
        // camera_init() recomputes from the stored (already unit) vectors
        let mut right2 = cross(up, view);
        vnorm(&mut right2);
        let mut up2 = cross(view, right2);
        vnorm(&mut up2);
        let mut view2 = view;
        vnorm(&mut view2);

        let sx = sc.hres as f64;
        let sy = sc.vres as f64;
        let mut px = ((sx / sy) / cam.aspect) / cam.zoom;
        let mut py = 1.0 / cam.zoom;
        let (left, right_f, bottom, top) = match cam.frustum {
            Some(f) => (f[0], f[1], f[2], f[3]),
            None => (-0.5 * px, 0.5 * px, -0.5 * py, 0.5 * py),
        };
        let ortho = cam.projection == Projection::Orthographic;
        let projcent = if ortho { cam.center } else { raypnt(cam.center, view2, 1.0) };
        let mut lowleft = [0.0; 3];
        for k in 0..3 {
            lowleft[k] = projcent[k] + (left * right2[k]) + (bottom * up2[k]);
            if !ortho {
                lowleft[k] -= cam.center[k];
            }
        }
        px = right_f - left;
        py = top - bottom;
        let iplaneright = [px * right2[0] / sx, px * right2[1] / sx, px * right2[2] / sx];
        let iplaneup = [py * up2[0] / sy, py * up2[1] / sy, py * up2[2] / sy];

        Tracer {
            sc,
            prims,
            tie,
            bvh,
            rightvec: right2,
            upvec: up2,
            viewvec: view2,
            lowleft,
            iplaneright,
            iplaneup,
            px,
            py,
        }
    }

    /// Does primitive a win an exact depth tie against b? By Tachyon's test order (`tie_order`), except two
    /// different triangles: they tie only in the scene file's 6 digits, VMD decides with the digits it does not
    /// write, and the later one is taken (what VMD's own picture shows in the coincident_* test scenes;
    /// decisions-pending F4-a). `mesh::pair` draws coplanar triangles by the same rule.
    pub fn wins_tie(&self, a: usize, b: usize) -> bool {
        let tri = |p: &Prim| match p {
            Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => Some((*v0, *e1, *e2)),
            _ => None,
        };
        match (tri(&self.prims[a].prim), tri(&self.prims[b].prim)) {
            (Some(x), Some(y)) if x != y => a > b,
            _ => self.tie[a] < self.tie[b],
        }
    }

    fn clipped(&self, p: &PrimObj, hit: V3) -> bool {
        if let Some(ci) = p.clip {
            for pl in &self.sc.clips[ci] {
                if pl[0] as f64 * hit[0] + pl[1] as f64 * hit[1] + pl[2] as f64 * hit[2] > pl[3] as f64 {
                    return true;
                }
            }
        }
        false
    }

    /// Does primitive p block shadow ray `ry` (`intersect`'s shadow test for one primitive)?
    fn blocks(&self, p: &PrimObj, ry: &Ray) -> bool {
        let mut hit = false;
        if self.sc.materials[p.tex].shadowcast() {
            p.intersect(ry.o, ry.d, &mut |t| hit = hit || (t > EPSILON && t < ry.maxdist && !self.clipped(p, raypnt(ry.o, ry.d, t))));
        }
        hit
    }

    fn intersect(&self, ry: &Ray, kind: RayKind) -> Hits {
        let mut h = Hits { num: 0, t: 0.0, obj: 0, shadowfilter: 1.0 };
        // Skiagraph: a shadow ray's answer is only whether something blocks it (its other fields are read
        // only when nothing does), so the primitive that blocked this thread's last one in this direction
        // is tried first
        let slot = blocker_slot(ry.d);
        if kind == RayKind::Shadow {
            let b = BLOCKER.with(|c| c[slot].get());
            if b < self.prims.len() && self.blocks(&self.prims[b], ry) {
                h.num = 1;
                return h;
            }
        }
        let maxdist = std::cell::Cell::new(ry.maxdist);
        let finished = std::cell::Cell::new(false);
        self.bvh.visit(ry.o, ry.d, || maxdist.get(), |i| {
            if finished.get() {
                return;
            }
            let p = &self.prims[i as usize];
            p.intersect(ry.o, ry.d, &mut |t| {
                // two surfaces at exactly the same distance (identical copies, triangles meeting at a
                // vertex): the one Tachyon tests first wins, whatever order the tree offers them in
                let tie = kind == RayKind::Regular && h.num > 0 && t == maxdist.get() && self.wins_tie(i as usize, h.obj as usize);
                if t > EPSILON && (t < maxdist.get() || tie) {
                    let tex = &self.sc.materials[p.tex];
                    if kind == RayKind::Shadow {
                        if !tex.shadowcast() {
                            if self.sc.shadow_filter {
                                h.shadowfilter *= 1.0 - tex.opacity as f64;
                            }
                            return;
                        }
                        if self.clipped(p, raypnt(ry.o, ry.d, t)) {
                            return;
                        }
                        // the answer is known: no box is nearer than -inf, so the tree walk ends here
                        maxdist.set(f64::NEG_INFINITY);
                        h.num = 1;
                        finished.set(true);
                        BLOCKER.with(|c| c[slot].set(i as usize));
                    } else {
                        if self.clipped(p, raypnt(ry.o, ry.d, t)) {
                            return;
                        }
                        maxdist.set(t);
                        h.num = 1;
                        h.t = t;
                        h.obj = i;
                    }
                }
            });
        });
        h
    }

    /// Skiagraph (stage 2, cast shadows): does an opaque shape block the ray from `o` along `d` (towards a light)?
    pub fn blocked(&self, o: V3, d: V3) -> bool {
        self.blocked_within(o, d, FHUGE)
    }

    /// `blocked`, counting only shapes closer than `maxdist` (contact accents: what lies just above a surface).
    pub fn blocked_within(&self, o: V3, d: V3, maxdist: f64) -> bool {
        self.intersect(&Ray { o, d, maxdist }, RayKind::Shadow).num > 0
    }

    /// Ray origin/direction for image-plane coordinates (x, y) in Tachyon's pixel units.
    pub fn camera_ray(&self, x: f64, y: f64) -> (V3, V3) {
        let ll = self.lowleft;
        let p = [
            ll[0] + (x * self.iplaneright[0]) + (y * self.iplaneup[0]),
            ll[1] + (x * self.iplaneright[1]) + (y * self.iplaneup[1]),
            ll[2] + (x * self.iplaneright[2]) + (y * self.iplaneup[2]),
        ];
        match self.sc.camera.projection {
            Projection::Orthographic => (p, self.viewvec),
            _ => {
                let len = 1.0 / (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
                (self.sc.camera.center, [p[0] * len, p[1] * len, p[2] * len])
            }
        }
    }

    /// Skiagraph: every surface the camera ray through (x, y) crosses, nearest first, up to the first
    /// opaque one (the surfaces a pixel of a transparent scene is made of), stepping as Tachyon's
    /// transmitted rays do.
    pub fn surfaces(&self, x: f64, y: f64) -> Vec<u32> {
        self.surfaces_t(x, y).into_iter().map(|s| s.0).collect()
    }

    /// `surfaces`, each with its distance along the ray.
    pub fn surfaces_t(&self, x: f64, y: f64) -> Vec<(u32, f64)> {
        let (o, d) = self.camera_ray(x, y);
        self.path(o, d)
    }

    /// `surfaces` along the ray (o, d), each with its distance from o.
    fn path(&self, mut o: V3, d: V3) -> Vec<(u32, f64)> {
        let (mut out, mut base) = (vec![], 0.0);
        for _ in 0..self.sc.camera.raydepth.max(1) {
            let ry = Ray { o, d, maxdist: FHUGE };
            let h = self.intersect(&ry, RayKind::Regular);
            if h.num == 0 {
                break;
            }
            out.push((h.obj, base + h.t));
            if self.sc.materials[self.prims[h.obj as usize].tex].opacity as f64 >= 1.0 - MINCONTRIB {
                break;
            }
            o = raypnt(raypnt(o, d, h.t), d, EPSILON);
            base += h.t + EPSILON;
        }
        out
    }

    /// Skiagraph: the first opaque surface the camera ray through (x, y) meets, through any transparent ones,
    /// and its distance along the ray (orthographic: from the image plane). No shading.
    pub fn first_opaque(&self, x: f64, y: f64) -> Option<(u32, f64)> {
        let (o0, d) = self.camera_ray(x, y);
        let (mut o, mut t0) = (o0, 0.0);
        for _ in 0..self.sc.camera.raydepth.max(1) {
            let ry = Ray { o, d, maxdist: FHUGE };
            let h = self.intersect(&ry, RayKind::Regular);
            if h.num == 0 {
                return None;
            }
            if self.sc.materials[self.prims[h.obj as usize].tex].opacity as f64 >= 1.0 - MINCONTRIB {
                return Some((h.obj, t0 + h.t));
            }
            t0 += h.t + EPSILON;
            o = raypnt(raypnt(o, d, h.t), d, EPSILON);
        }
        None
    }

}

