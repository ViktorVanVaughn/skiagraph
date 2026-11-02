//! Continuous lines (stage 3, owner requirement A): the stretches of outline that get ink, found per region in
//! `flat.rs`, are chained here into one line per edge and drawn as one filled shape each (two offset curves).
//!
//! - Stretches whose ends meet, or face each other across a gap shorter than 2.5 times their width (heading the same
//!   way within 45 degrees), are joined, the best aligned pair first; three ends at one point join the two that continue
//!   each other. A line whose colour changes (`Piece::key`: the fog's steps, under a transparent surface or not) stays
//!   one line: its width is smoothed across the change, and only its paint is split there, the two runs overlapping.
//! - A line shorter than 1.5 times its width is a tick (a sliver, a seam reaching an outline) and is dropped.
//! - The width is smoothed along the line (a Gaussian over 1.5 widths of arc length): it varies, never steps.
//! - A free end tapers over up to three widths; an end that butts against another line (a shape in front covers the
//!   rest, a colour change) or the frame stays full width.
//! - Each line is written `M L0 .. Ln Rn .. R0 Z`: the left offset forward, the right one back, with as many points on
//!   each side, so a check can read the width at every point (`scripts/defects.py`). Closed lines too: the two rings
//!   are joined by a seam that cancels, and the nonzero rule leaves the inside open.

use crate::geom::P2;
use crate::svg::PathData;
use std::collections::HashMap;

pub struct Piece {
    /// lines join only within one key (under the glass or not, the colour)
    pub key: usize,
    pub pts: Vec<P2>,
    /// the line's width at each point (px)
    pub w: Vec<f64>,
}

fn d2(a: P2, b: P2) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}
fn unit(a: P2) -> P2 {
    let l = a[0].hypot(a[1]).max(1e-12);
    [a[0] / l, a[1] / l]
}

/// End e (0 first, 1 last) of a piece: its point, the unit direction out of the piece, its width.
fn end(p: &Piece, e: usize) -> (P2, P2, f64) {
    let n = p.pts.len();
    let (a, b, w) = if e == 0 { (p.pts[0], p.pts[2.min(n - 1)], p.w[0]) } else { (p.pts[n - 1], p.pts[n.saturating_sub(3)], p.w[n - 1]) };
    (a, unit([a[0] - b[0], a[1] - b[1]]), w)
}

/// Which pieces make one line: each as [(piece, reversed)] and whether it closes.
fn chain(pieces: &[Piece]) -> Vec<(Vec<(usize, bool)>, bool)> {
    let wmax = pieces.iter().flat_map(|p| p.w.iter()).fold(1.0f64, |m, &w| m.max(w));
    let cell = 2.5 * wmax + 0.1;
    let key = |p: P2| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<(usize, usize)>> = HashMap::new();
    for (i, p) in pieces.iter().enumerate() {
        for e in 0..2 {
            grid.entry(key(end(p, e).0)).or_default().push((i, e));
        }
    }
    let mut cand: Vec<(f64, (usize, usize), (usize, usize))> = vec![];
    for (i, p) in pieces.iter().enumerate() {
        for e in 0..2 {
            let (pa, ta, wa) = end(p, e);
            let c = key(pa);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &(j, f) in grid.get(&(c.0 + dx, c.1 + dy)).into_iter().flatten() {
                        if (j, f) <= (i, e) || (i == j && p.pts.len() < 4) {
                            continue;
                        }
                        let (pb, tb, wb) = end(&pieces[j], f);
                        let d = d2(pa, pb);
                        let turn = -(ta[0] * tb[0] + ta[1] * tb[1]); // 1: they continue each other
                        let ok = if d < 0.1 {
                            turn > -0.7 // meeting ends, unless the line folds back on itself
                        } else {
                            let u = unit([pb[0] - pa[0], pb[1] - pa[1]]);
                            d < 2.5 * wa.max(wb) && turn > 0.7 && ta[0] * u[0] + ta[1] * u[1] > 0.7 && -(tb[0] * u[0] + tb[1] * u[1]) > 0.7
                        };
                        if ok {
                            cand.push((d + 0.05 * (1.0 - turn), (i, e), (j, f)));
                        }
                    }
                }
            }
        }
    }
    cand.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut partner: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    for (_, a, b) in cand {
        if !partner.contains_key(&a) && !partner.contains_key(&b) {
            partner.insert(a, b);
            partner.insert(b, a);
        }
    }
    let mut used = vec![false; pieces.len()];
    let mut out = vec![];
    let walk = |start: usize, e_in: usize, used: &mut Vec<bool>| -> (Vec<(usize, bool)>, bool) {
        let (mut seq, mut i, mut e) = (vec![], start, e_in);
        loop {
            used[i] = true;
            seq.push((i, e == 1));
            match partner.get(&(i, 1 - e)) {
                Some(&(j, f)) if !used[j] => (i, e) = (j, f),
                Some(&(j, _)) => return (seq, j == start),
                None => return (seq, false),
            }
        }
    };
    // lines with a free end first (walked from it), then the closed ones
    for i in 0..pieces.len() {
        for e in 0..2 {
            if !used[i] && !partner.contains_key(&(i, e)) {
                out.push(walk(i, e, &mut used));
            }
        }
    }
    for i in 0..pieces.len() {
        if !used[i] {
            out.push(walk(i, 0, &mut used));
        }
    }
    out
}

/// The lines as path data, one string per key: every line one filled shape. `frame`: the picture's size; lines shorter
/// than `min_len` (and than 1.5 times their width) are dropped; `tol`: how far (px) a simplified outline may stray.
pub fn draw(pieces: &[Piece], frame: P2, min_len: f64, tol: f64) -> HashMap<usize, String> {
    // each line as points, widths and keys (a line may change colour along the way: it is painted in runs of one key)
    let mut lines: Vec<(Vec<P2>, Vec<f64>, Vec<usize>, bool)> = vec![];
    for (seq, closed) in chain(pieces) {
        let (mut pts, mut ws, mut ks): (Vec<P2>, Vec<f64>, Vec<usize>) = (vec![], vec![], vec![]);
        for &(i, rev) in &seq {
            let p = &pieces[i];
            let mut ix: Vec<usize> = (0..p.pts.len()).collect();
            if rev {
                ix.reverse();
            }
            for j in ix {
                if pts.last().is_some_and(|&q| d2(q, p.pts[j]) < 1e-3) {
                    continue;
                }
                pts.push(p.pts[j]);
                ws.push(p.w[j]);
                ks.push(p.key);
            }
        }
        if closed && pts.len() > 2 && d2(pts[0], pts[pts.len() - 1]) < 1e-3 {
            pts.pop();
            ws.pop();
            ks.pop();
        }
        let len: f64 = pts.windows(2).map(|w| d2(w[0], w[1])).sum();
        let wmax = ws.iter().fold(0.0f64, |m, &w| m.max(w));
        if pts.len() >= 2 && len >= (1.5 * wmax).max(min_len) {
            lines.push((pts, ws, ks, closed));
        }
    }
    // where every line passes, to tell a butting end (another line there) from a free one
    let cell = 8.0;
    let ck = |p: P2| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
    let mut near: HashMap<(i64, i64), Vec<(usize, usize)>> = HashMap::new();
    for (l, line) in lines.iter().enumerate() {
        for (i, &p) in line.0.iter().enumerate() {
            near.entry(ck(p)).or_default().push((l, i));
        }
    }
    let butts = |l: usize, p: P2, w: f64| -> bool {
        let edge = p[0] < 1.0 || p[1] < 1.0 || p[0] > frame[0] - 1.0 || p[1] > frame[1] - 1.0;
        let c = ck(p);
        edge || (-1..=1).any(|dx| (-1..=1).any(|dy| near.get(&(c.0 + dx, c.1 + dy)).into_iter().flatten().any(|&(m, i)| m != l && d2(lines[m].0[i], p) < 0.5 * (w + lines[m].1[i]) + 0.25)))
    };
    let mut out: HashMap<usize, String> = HashMap::new();
    for (l, (pts, ws, ks, closed)) in lines.iter().enumerate() {
        let n = pts.len();
        let mut s = vec![0.0];
        for w in pts.windows(2) {
            s.push(s[s.len() - 1] + d2(w[0], w[1]));
        }
        let total = s[n - 1] + if *closed { d2(pts[n - 1], pts[0]) } else { 0.0 };
        // the width smoothed along the line: a Gaussian of sigma = 1.5 times the line's median width
        let mut sorted = ws.clone();
        sorted.sort_by(f64::total_cmp);
        let sigma = 1.5 * sorted[n / 2].max(0.5);
        let along = |i: usize, j: usize| -> f64 {
            let d = (s[i] - s[j]).abs();
            if *closed { d.min(total - d) } else { d }
        };
        let mut w: Vec<f64> = (0..n)
            .map(|i| {
                let (mut sum, mut wt) = (0.0, 0.0);
                // ponytail: a window of +-60 points (lines are sampled about every pixel), wider sigmas are cut there
                for o in 0..n.min(121) {
                    let j = (i + n - n.min(121) / 2 + o) % n;
                    if *closed || (j as i64 - i as i64).abs() <= 60 {
                        let g = (-0.5 * (along(i, j) / sigma).powi(2)).exp();
                        sum += g * ws[j];
                        wt += g;
                    }
                }
                sum / wt
            })
            .collect();
        // free ends taper (smoothstep to 0.2 of the width over up to three widths)
        let free = if *closed { [false; 2] } else { [!butts(l, pts[0], w[0]), !butts(l, pts[n - 1], w[n - 1])] };
        let reach = |w: f64| (3.0 * w).min(0.5 * total).max(1e-9);
        let (r0, r1) = (reach(w[0]), reach(w[n - 1]));
        let f = |d: f64, r: f64| { let x = (d / r).clamp(0.0, 1.0); 0.2 + 0.8 * x * x * (3.0 - 2.0 * x) };
        for i in 0..n {
            let mut k = 1.0;
            if free[0] { k = f64::min(k, f(s[i], r0)); }
            if free[1] { k = f64::min(k, f(total - s[i], r1)); }
            w[i] *= k;
        }
        // runs of one key; a closed line of several keys is opened where its key changes
        let change: Vec<usize> = (0..n).filter(|&i| ks[i] != ks[(i + n - 1) % n]).collect();
        let runs: Vec<(Vec<usize>, bool)> = if change.is_empty() || (!*closed && change == [0]) {
            vec![((0..n).collect(), *closed)]
        } else {
            let mut cuts = change.clone();
            if !*closed {
                cuts.retain(|&i| i > 0);
                cuts.insert(0, 0);
                cuts.push(n - 1);
            } else {
                cuts.push(cuts[0] + n);
            }
            // each run ends on the next run's first point: they share it
            cuts.windows(2).map(|c| ((c[0]..=c[1]).map(|i| i % n).collect(), false)).collect()
        };
        for (ix, run_closed) in runs {
            let m = ix.len();
            if m < 2 {
                continue;
            }
            let mut p: Vec<P2> = ix.iter().map(|&i| pts[i]).collect();
            let wr: Vec<f64> = ix.iter().map(|&i| w[i]).collect();
            // an end that is not a free end of the whole line butts (against another line, or the next run): it reaches
            // 0.3 of its width further, under what it meets, so no hairline shows between them
            if !run_closed {
                let at_free = |i: usize| !*closed && ((i == 0 && free[0]) || (i == n - 1 && free[1]));
                for (e, g, i) in [(0, 1, ix[0]), (m - 1, m - 2, ix[m - 1])] {
                    if !at_free(i) {
                        let t = unit([p[e][0] - p[g][0], p[e][1] - p[g][1]]);
                        p[e] = [p[e][0] + 0.3 * wr[e] * t[0], p[e][1] + 0.3 * wr[e] * t[1]];
                    }
                }
            }
            out.entry(ks[ix[1.min(m - 1)]]).or_default().push_str(&shape(&p, &wr, run_closed, tol));
        }
    }
    out
}

/// One line as a filled shape: the left offset forward, the right one back (`M L0 .. Ln Rn .. R0 Z`), a corner sharper
/// than 15 degrees getting two points on each side (a bevel outside; inside, the overlap stays filled under the nonzero
/// rule); points kept by Douglas-Peucker on either side are kept on both.
fn shape(pts: &[P2], w: &[f64], closed: bool, tol: f64) -> String {
    let n = pts.len();
    let (mut left, mut right) = (vec![], vec![]);
    let seg_n = |a: P2, b: P2| { let u = unit([b[0] - a[0], b[1] - a[1]]); [-u[1], u[0]] };
    for i in 0..n {
        let prev = if i > 0 { Some(pts[i - 1]) } else if closed { Some(pts[n - 1]) } else { None };
        let next = if i + 1 < n { Some(pts[i + 1]) } else if closed { Some(pts[0]) } else { None };
        let ni = prev.map(|p| seg_n(p, pts[i]));
        let no = next.map(|q| seg_n(pts[i], q));
        let h = 0.5 * w[i];
        let mut put = |m: P2, k: f64| {
            left.push([pts[i][0] + h * k * m[0], pts[i][1] + h * k * m[1]]);
            right.push([pts[i][0] - h * k * m[0], pts[i][1] - h * k * m[1]]);
        };
        match (ni, no) {
            (Some(a), Some(b)) if a[0] * b[0] + a[1] * b[1] > 0.966 => {
                let m = unit([a[0] + b[0], a[1] + b[1]]);
                put(m, 1.0 / (m[0] * a[0] + m[1] * a[1]).max(0.5));
            }
            (Some(a), Some(b)) => {
                put(a, 1.0);
                put(b, 1.0);
            }
            (Some(a), None) | (None, Some(a)) => put(a, 1.0),
            (None, None) => {}
        }
    }
    if closed {
        left.push(left[0]);
        right.push(right[0]);
    }
    let keep: Vec<bool> = simplify_keep(&left, tol).iter().zip(simplify_keep(&right, tol)).map(|(a, b)| *a || b).collect();
    let mut d = PathData::default();
    let mut first = true;
    for (p, _) in left.iter().zip(&keep).filter(|x| *x.1) {
        if first { d.m(*p); first = false } else { d.l(*p) }
    }
    for (p, _) in right.iter().zip(&keep).rev().filter(|x| *x.1) {
        d.l(*p);
    }
    let mut d = d.finish();
    d.push('Z');
    d
}

/// Douglas-Peucker to `tol` px: which points of a polyline to keep.
pub fn simplify_keep(pts: &[P2], tol: f64) -> Vec<bool> {
    let mut keep = vec![pts.len() < 3; pts.len()];
    if pts.len() < 3 {
        return keep;
    }
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (pts[a], pts[b]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let l = dx.hypot(dy);
        let (mut worst, mut at) = (0.0, a);
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let dist = if l < 1e-12 { d2(*p, pa) } else { ((p[0] - pa[0]) * dy - (p[1] - pa[1]) * dx).abs() / l };
            if dist > worst {
                (worst, at) = (dist, i);
            }
        }
        if worst > tol {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(key: usize, a: P2, b: P2, w: f64) -> Piece {
        let n = (d2(a, b).ceil() as usize).max(1);
        let pts: Vec<P2> = (0..=n).map(|i| { let t = i as f64 / n as f64; [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])] }).collect();
        Piece { key, w: vec![w; pts.len()], pts }
    }

    /// Meeting pieces and a short gap become one line; a wide gap or a sharp fold back does not join; a colour change
    /// splits only the paint; a widths step is smoothed, never kept.
    #[test]
    fn pieces_chain_into_one_smooth_line() {
        let ps = vec![piece(0, [0.0, 0.0], [10.0, 0.0], 2.0), piece(0, [10.0, 0.0], [20.0, 0.0], 4.0), piece(0, [22.0, 0.0], [40.0, 0.0], 4.0), piece(0, [50.0, 0.0], [60.0, 0.0], 4.0), piece(1, [60.0, 0.0], [70.0, 0.0], 4.0)];
        let ch = chain(&ps);
        assert_eq!(ch.len(), 2, "{:?}", ch.iter().map(|c| c.0.clone()).collect::<Vec<_>>());
        assert!(ch.iter().any(|c| c.0.len() == 3));
        let d = draw(&ps, [100.0, 100.0], 0.0, 0.06);
        assert_eq!(d[&0].matches('M').count(), 2);
        assert_eq!(d[&1].matches('M').count(), 1);
        // a fold back is not a continuation
        let fold = vec![piece(0, [0.0, 0.0], [10.0, 0.0], 2.0), piece(0, [10.0, 0.0], [1.0, 0.0], 2.0)];
        assert_eq!(chain(&fold).len(), 2);
    }
}
