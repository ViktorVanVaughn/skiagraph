//! Comic Ink (ADR 0012, stage 2): the fills and the lines; `ink.rs` adds the drawn light (tones, shadows, highlights,
//! hatching) between them.
//!
//! - Fills: every visible shape gets VMD's colour, fogged by depth (`fog`, `fogto`, `fogsat`). A shape is cut only by
//!   the shapes of other colours in front of it (`mesh::visible_region`, exact); one colour's regions become one path
//!   per scene group, painted back to front.
//! - Lines: along every region's outline, where the picture has an edge: a shape in front of the background or of
//!   something deeper, or two parts (molecules, found from the geometry alone) touching. The weight rule: a silhouette
//!   is 15 % of the shape's width on the picture (2-4.5 px per 2400 px, thinner with depth), creases and contacts half;
//!   a line is cut where its stroke would fall on another part in front; pieces too short for their width (ticks) are
//!   dropped; a line's free ends taper. Where lines go is found with eye rays along each outline.
//! - What no eye ray sees (4 x 4 per pixel) is left out (`seen`).
//! - A transparent surface becomes one tinted layer: the union of its walls, painted once (at least 15 % opaque).

use crate::geom::{Arc, Curve, P2};
use crate::mesh::{self, Grid};
use crate::svg::{self, Unsupported, View};
use crate::tachyon::{Prim, Tracer};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The lines' colour.
const INK: &str = "#1c1c1c";
/// Eye rays per pixel side for the culling (`seen`).
const RAYS: u32 = 4;
/// The least opacity of a transparent surface's tinted layer.
const VEIL_MIN: f32 = 0.15;
/// The outer silhouette (against the background) against an inner one; a crease or contact against an inner one.
const OUTER: f64 = 1.75;
/// Stage 4: spacing of an emissive shape's glow rings (px per 2400)
const GLOW: f64 = 5.0;
/// Stage 4: the lightest a glass tint may be (luminance 0..1)
const GLASS_LUM: f64 = 0.75;
/// Stage 4 night preset: the ground, and what the colours are multiplied by
const NIGHT_BG: [f64; 3] = [0.17, 0.2, 0.28];
const NIGHT_TINT: [f64; 3] = [0.6, 0.66, 0.85];
const CREASE: f64 = 0.4;

/// A line's weight (0 = none, 1 = a silhouette's) where a shape's outline borders what lies across:
/// `dz` = depth across minus the shape's depth (None: the background), `near` the smallest jump worth a
/// line, `far` the jump of a full silhouette, `other` = a different molecule across, `owner` = this side
/// draws a line both sides see alike (contacts are drawn once).
fn weight(dz: Option<f64>, near: f64, far: f64, other: bool, owner: bool) -> f64 {
    match dz {
        None => 1.0,
        Some(dz) if dz < -near => 0.0, // the shape across is in front: its own outline
        Some(dz) if dz > near => 0.3 + 0.7 * ((dz - near) / (far - near).max(1e-12)).min(1.0),
        Some(_) if other && owner => 0.3, // two molecules touch
        Some(_) => 0.0,
    }
}

/// How shapes are filled: VMD's colour, or moved this far towards white.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FillMode {
    Vmd,
    Pale(f64),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineMode {
    Off,
    Ink([f64; 3]),
}

/// Comic Ink (the only style since stage 2) and its short recipe, written `key=value,...` on top of the default:
/// lines=black|none|#hex, fill=vmd|pale:0.3, detail=0|1 (lines also at small depth jumps), fog=0..1 (how far the
/// deepest shapes move towards `fogto`), fogto=#hex, foglines=0|1, fogsat=0..1 (colour the fog takes away), bg=#hex,
/// dec=N (decimals of positions), light=comic|scene (Comic Ink's key light from the upper left, or the scene's), spot=0|1
/// (spot blacks).
#[derive(Clone, Debug)]
pub struct Spec {
    pub fill: FillMode,
    pub lines: LineMode,
    pub detail: bool,
    pub fog: f64,
    pub fog_to: Option<[f64; 3]>,
    pub fog_lines: bool,
    pub fog_sat: f64,
    pub bg: Option<[f64; 3]>,
    pub dec: Option<u32>,
    /// stage 4: the lighting preset (`light=studio|rim|low|night|scene`; `comic` = studio)
    pub light: crate::ink::Preset,
    /// cast shadows: on by default (owner, stage 5); `shadows=scene` (None) follows VMD's shadow switch (Tachyon's
    /// `Shader_Mode Full`); `shadows=off` turns them off
    pub shadows: Option<bool>,
    /// spot blacks (`spot=1`)
    pub spot: bool,
    /// stage 4: materials by group (`mat=metal` for all, `mat=Rep[1]:glass;Licorice:metal`: the first part of a group's
    /// name that matches wins); otherwise read from the scene's material (`ink::Mat::of`)
    pub mats: Vec<(String, crate::ink::Mat)>,
}

fn rgb(s: &str) -> Option<[f64; 3]> {
    let s = s.trim_start_matches('#');
    (s.len() == 6).then(|| [0, 2, 4].map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_or(0.0, |v| f64::from(v) / 255.0)))
}

impl Spec {
    /// `comic-ink`, or a recipe on top of it.
    pub fn named(name: &str) -> Result<Spec, String> {
        let base = Spec { fill: FillMode::Vmd, lines: LineMode::Ink([0.0; 3]), detail: true, fog: 0.4, fog_to: rgb("#e4eaf1"), fog_lines: true, fog_sat: 0.4, bg: None, dec: Some(2), light: crate::ink::Preset::Studio, shadows: Some(true), spot: false, mats: vec![] };
        if name == "comic-ink" { Ok(base) } else { Spec::parse(name, base) }
    }

    fn parse(s: &str, mut sp: Spec) -> Result<Spec, String> {
        let bad = |k: &str| format!("style: bad value for {k} in {s:?}");
        for kv in s.split(',').filter(|kv| !kv.is_empty()) {
            let (k, v) = kv.split_once('=').ok_or_else(|| format!("style: {kv:?} is not key=value"))?;
            let num = || v.parse::<f64>().map_err(|_| bad(k));
            let col = || rgb(v).ok_or_else(|| bad(k));
            match k {
                "fill" => {
                    let p: Vec<&str> = v.split(':').collect();
                    sp.fill = match p[0] {
                        "vmd" => FillMode::Vmd,
                        "pale" => FillMode::Pale(p.get(1).and_then(|x| x.parse().ok()).unwrap_or(0.3)),
                        _ => return Err(bad(k)),
                    }
                }
                "lines" => sp.lines = match v { "none" => LineMode::Off, "black" => LineMode::Ink([0.0; 3]), _ => LineMode::Ink(col()?) },
                "detail" => sp.detail = v != "0",
                "fog" => sp.fog = num()?,
                "fogto" => sp.fog_to = Some(col()?),
                "foglines" => sp.fog_lines = v != "0",
                "fogsat" => sp.fog_sat = num()?,
                "bg" => sp.bg = Some(col()?),
                "dec" => sp.dec = Some(num()? as u32),
                "light" => sp.light = crate::ink::Preset::parse(v).ok_or_else(|| bad(k))?,
                "shadows" => sp.shadows = match v { "on" => Some(true), "off" => Some(false), "scene" => None, _ => return Err(bad(k)) },
                "spot" => sp.spot = v != "0",
                "mat" => {
                    for part in v.split(';') {
                        let (pat, m) = part.rsplit_once(':').unwrap_or(("", part));
                        sp.mats.push((pat.to_string(), crate::ink::Mat::parse(m).ok_or_else(|| bad(k))?));
                    }
                }
                _ => return Err(format!("style: unknown key {k:?} (keys: lines, fill, detail, fog, fogto, foglines, fogsat, bg, dec, light, shadows, spot, mat)")),
            }
        }
        Ok(sp)
    }
}

/// Stage 4: the recipe's materials (`Spec::mats`) written into the scene before anything is traced, so that a group made
/// glass is also see-through for the eye rays: each matching object gets its own copy of its material, set to that
/// material's typical VMD values (colour kept).
pub fn apply_materials(sc: &mut crate::scene::Scene, spec: &Spec) {
    use crate::ink::Mat;
    if spec.mats.is_empty() {
        return;
    }
    let mut made: BTreeMap<(usize, Mat), usize> = BTreeMap::new();
    for i in 0..sc.objects.len() {
        let name = sc.objects[i].group.map_or("", |g| sc.groups[g].as_str());
        let Some(&(_, m)) = spec.mats.iter().find(|(pat, _)| name.contains(pat.as_str())) else { continue };
        let tex = sc.objects[i].tex;
        let n = *made.entry((tex, m)).or_insert_with(|| {
            let mut t = sc.materials[tex].clone();
            (t.ambient, t.diffuse, t.specular, t.phong, t.phong_type) = (0.0, 0.65, 0.0, 0.5, crate::scene::PhongType::Plastic);
            t.opacity = if m == Mat::Glass { t.opacity.min(0.3) } else { 1.0 };
            match m {
                Mat::Matte => t.phong = 0.0,
                Mat::Metal => t.phong_type = crate::scene::PhongType::Metal,
                _ => {}
            }
            sc.materials.push(t);
            sc.materials.len() - 1
        });
        sc.objects[i].tex = n;
    }
}

/// Depth steps of the fog.
const FOG_STEPS: f64 = 8.0;

fn background(sc: &crate::scene::Scene) -> [f64; 3] {
    match &sc.background {
        crate::scene::Background::Solid(c) => [c.r, c.g, c.b].map(f64::from),
        // shortcut: a gradient background is drawn as its middle colour (the style has no gradients)
        crate::scene::Background::Gradient { top_color: t, bottom_color: b, .. } => {
            [0.5 * (t.r + b.r), 0.5 * (t.g + b.g), 0.5 * (t.b + b.b)].map(f64::from)
        }
    }
}

fn hex(c: [f64; 3]) -> String {
    let q = c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{:02x}{:02x}{:02x}", q[0], q[1], q[2])
}

pub fn convert(tr: &Tracer, spec: &Spec) -> Result<String, Unsupported> {
    let draw_lines = spec.lines != LineMode::Off;
    let ink: String = match spec.lines { LineMode::Ink(c) => hex(c), _ => INK.to_string() };
    let (fog, fog_lines) = (spec.fog, spec.fog_lines);
    // night (stage 4): a dark blue ground the far shapes fade into, and cooler, darker colours
    let night = spec.light == crate::ink::Preset::Night;
    let bg = spec.bg.unwrap_or_else(|| if night { NIGHT_BG } else { background(tr.sc) });
    let fog_to = if night { bg } else { spec.fog_to.unwrap_or(bg) };
    if let Some(d) = spec.dec {
        svg::DEC.store(d, std::sync::atomic::Ordering::Relaxed);
    }
    let clock = std::time::Instant::now();
    let sc = tr.sc;
    let view = View::new(tr);
    svg::geometry_checks(tr, &view)?;
    let (all, n_opaque) = mesh::items(tr, &view)?;
    let glass = |k: usize| k >= n_opaque;
    // what no eye ray sees is left out: hidden shapes would only add bytes
    let seen = seen(tr, &view, RAYS);
    let shown: Vec<bool> = (0..all.len()).map(|k| glass(k) || seen[all[k].prim]).collect();
    let t_cull = clock.elapsed().as_secs_f64();
    let mol = molecules(tr);
    // depth (0 front, 1 back) of an item's centre among the shown ones
    let (zlo, zhi) = (0..all.len()).filter(|&k| shown[k]).fold((f64::MAX, f64::MIN), |(a, b), k| (a.min(all[k].key), b.max(all[k].key)));
    let rel = |k: usize| if zhi > zlo { ((all[k].key - zlo) / (zhi - zlo)).clamp(0.0, 1.0) } else { 0.0 };
    // fill colour: darker with depth, in steps; a transparent surface keeps one colour, so its walls make one
    // union, a quarter darker than its material (a veil that shows on a light background)
    // the fog's step of an item (0 front .. FOG_STEPS - 1 back) and how far it fades towards the background
    let fog_steps = FOG_STEPS;
    let fog_t = |k: usize| fog * (rel(k) * (fog_steps - 1.0)).round() / (fog_steps - 1.0);
    // VMD's light (the first directional one; towards the light) and the way to the camera
    let base_rgb = |k: usize| -> [f64; 3] {
        let h = fill_colour(tr, all[k].prim, 1.0, 0.0, bg, 0.0);
        [1, 3, 5].map(|i| f64::from(u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0)) / 255.0)
    };
    // each item's flat colour, then fogged with its depth
    let paint = |k: usize, c: [f64; 3]| -> String {
        let c = c.map(|v| v.clamp(0.0, 1.0));
        let c = if night { [0, 1, 2].map(|i| c[i] * NIGHT_TINT[i]) } else { c };
        let c = match spec.fill {
            FillMode::Pale(p) => c.map(|v| p + (1.0 - p) * v),
            FillMode::Vmd => c,
        };
        let t = fog_t(k);
        // (5) the fog also takes the colour away, fully (fogsat) at its deepest
        let g = 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
        let c = c.map(|v| v + (g - v) * spec.fog_sat * t / fog.max(1e-9));
        hex([0, 1, 2].map(|i| c[i] + (fog_to[i] - c[i]) * t))
    };
    // stage 4: a glass tint no lighter than GLASS_LUM, so a white transparent surface shows as more than a ghost
    let colour: Vec<String> = (0..all.len())
        .map(|k| {
            let c = base_rgb(k);
            let lum = 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
            paint(k, if glass(k) && lum > GLASS_LUM { c.map(|v| v * GLASS_LUM / lum) } else { c })
        })
        .collect();
    // triangles of more than one colour, split by the geometry (`colour_cells`)
    let cells: Vec<(Vec<([f64; 3], Vec<P2>)>, Vec<Curve>)> = (0..all.len()).map(|k| if glass(k) || !shown[k] { (vec![], vec![]) } else { colour_cells(tr, &view, all[k].prim) }).collect();
    let mixed = |k: usize| !cells[k].0.is_empty();
    // an opaque item is cut by the shown opaque items of other colours; a transparent one by the opaque ones; a
    // triangle of two colours cuts and is cut by all
    let grid = Grid::new(&all.iter().map(|it| (it.lo, it.hi)).collect::<Vec<_>>());
    // stage 5: two triangles of one colour leave each other uncut only where their depth ranges overlap (neighbours, a
    // smooth sheet); one lying wholly in front hides the other (a mesh passing behind itself: a ring, a fold)
    let tri = |k: usize| matches!(all[k].shape, mesh::Shape::T(_));
    let same = |k: usize, j: usize| colour[j] == colour[k] && (!(tri(k) && tri(j)) || (all[j].zmin <= all[k].zmax && all[k].zmin <= all[j].zmax));
    let skip2 = |k: usize, j: usize| glass(j) || !shown[j] || (!glass(k) && !mixed(k) && !mixed(j) && same(k, j));
    let regions: Vec<Option<(Vec<Curve>, Vec<Vec<Arc>>)>> = (0..all.len())
        .into_par_iter()
        .map(|k| if shown[k] { mesh::visible_region(&view, &all, &grid, k, &|j| skip2(k, j)) } else { None })
        .collect();
    // a split triangle's fill: one part per colour (the region of the part's cell)
    let parts: Vec<Vec<(String, (Vec<Curve>, Vec<Vec<Arc>>))>> = (0..all.len())
        .into_par_iter()
        .map(|k| {
            if !mixed(k) || regions[k].is_none() {
                return vec![];
            }
            let vis = mesh::visibility(&view, &all, &grid, k, &|j| skip2(k, j));
            cells[k].0.iter().filter_map(|(c, poly)| vis.part(&view, &all, &cells[k].1, &|x| crate::ink::inside(poly, x)).map(|r| (paint(k, *c), r))).collect()
        })
        .collect();
    let t_regions = clock.elapsed().as_secs_f64() - t_cull;
    // a full silhouette: a jump of 15 % of the scene's depth; base line width (pixels) grows with the picture
    let zr = (0..all.len()).filter(|&k| regions[k].is_some()).fold((f64::MAX, f64::MIN), |(a, b), k| (a.min(all[k].zmin), b.max(all[k].zmax)));
    let span = (zr.1 - zr.0).max(1e-9);
    let far = 0.15 * span;
    // a line's scale is its silhouette floor (2 px per 2400 px of picture): only true hairlines (VMD's Lines, Points)
    // are drawn as strokes in their own colour, and only gaps narrower than two such lines close
    let base = 2.0 * f64::from(view.w) / 2400.0;
    let has_glass = n_opaque < all.len();
    // a shape thinner than a line (VMD's Lines and Points) is drawn as a stroke of a line's width in its own
    // colour, with no outline: an ink outline would hide its colour
    let thin = |k: usize| match &all[k].shape {
        mesh::Shape::S(s, _) => s.rpx < 0.4 * base,
        mesh::Shape::C(c, _) => c.rpx < 0.4 * base,
        _ => false,
    };
    // fills: (rep, colour, opacity, stroke width in 1/100 px) -> path data, in order of first appearance
    let mut fills: Vec<(Fill, String)> = vec![];
    let layer = |_: usize| 0usize;
    let mut fill_layer: Vec<usize> = vec![];
    let mut at: BTreeMap<(Option<usize>, String, Option<u32>, u32, usize), usize> = BTreeMap::new();
    // members: (item, its part: WHOLE or an index into parts[item])
    const WHOLE: usize = usize::MAX;
    let mut members: Vec<Vec<(usize, usize)>> = vec![];
    for k in (0..all.len()).filter(|&k| regions[k].is_some()) {
        let prim = all[k].prim;
        let rep = sc.objects[tr.prims[prim].obj].group;
        // painted once, a very faint surface (which Tachyon shows by stacking its walls) would vanish: at least 0.15
        let alpha = glass(k).then(|| sc.materials[tr.prims[prim].tex].opacity.max(VEIL_MIN));
        let pen = if thin(k) { (80.0 * base).round() as u32 } else { 50 };
        let whole = [(colour[k].clone(), WHOLE)];
        let split: Vec<(String, usize)> = parts[k].iter().enumerate().map(|(i, p)| (p.0.clone(), i)).collect();
        for (c, part) in if mixed(k) { split } else { whole.to_vec() } {
            let n = *at.entry((rep, c.clone(), alpha.map(f32::to_bits), pen, layer(k))).or_insert_with(|| {
                fills.push(((rep, c, alpha, pen), String::new()));
                fill_layer.push(layer(k));
                members.push(vec![]);
                fills.len() - 1
            });
            members[n].push((k, part));
        }
    }
    let ds: Vec<String> = members
        .par_iter()
        .map(|ks| {
            let (curves, loops) = dissolve(ks.iter().filter_map(|&(k, i)| if i == WHOLE { regions[k].as_ref() } else { Some(&parts[k][i].1) }));
            svg::path_data(&curves, &loops)
        })
        .collect();
    for (f, d) in fills.iter_mut().zip(ds) {
        f.1 = d;
    }
    // lines (stage 3, owner requirement A): along every region's outline, the stretches where the item itself is seen
    // just inside and the picture has an edge (`weight`), sampled about every pixel with their width; `line::draw`
    // chains them into one filled shape per edge. Line classes: 1 the outer silhouette (against the background), 2 an
    // inner silhouette (a deep jump), 3 a crease or contact; +4 seen through a transparent surface.
    let px = f64::from(view.w) / 2400.0;
    let pieces: Vec<Vec<(usize, u8, Vec<P2>, Vec<f64>)>> = (0..all.len())
        .into_par_iter()
        .map(|k| {
            let Some((curves, loops)) = regions[k].as_ref().filter(|_| !thin(k) && draw_lines) else { return vec![] };
            let it = &all[k];
            let eps = (1e-2 * it.size()).min(0.05);
            // the smallest jump worth a line: 1 % of the scene's depth, or half the shape's radius
            let near = (0.01 * span).max(0.5 * it.world()) * if spec.detail { 0.25 } else { 1.0 };
            // Comic Ink's weight rule (FINAL_PLAN §7, stage 3): an inner silhouette is 15 % of the shape's width on the
            // picture, at most 4.5 px, thinned by up to 40 % with depth, never below 2 px; the outer silhouette OUTER
            // times that, but at most 20 % of the width; creases and contacts CREASE times it, never below 1.25 px (per
            // 2400 px of picture width)
            let ink_width = |class: u8, q: P2, n: P2| -> f64 {
                let tq = it.front(&view, [q[0] - eps * n[0], q[1] - eps * n[1]]).unwrap_or(zr.1);
                let dep = ((tq - zr.0) / span).clamp(0.0, 1.0);
                let wide = match &it.shape {
                    mesh::Shape::S(s, _) => 2.0 * s.rpx,
                    mesh::Shape::C(c, _) => 2.0 * c.rpx,
                    // a mesh: how far inwards the same part goes on at about this depth (up to where the rule tops out)
                    _ => {
                        let mut d = 1.0;
                        while d < 50.0 * px {
                            let (x, y) = view.to_tachyon([q[0] - d * n[0], q[1] - d * n[1]]);
                            match tr.first_opaque(x, y) {
                                Some((p, t)) if mol[p as usize] == mol[it.prim] && (t - tq).abs() < 0.05 * span => d += 1.5,
                                _ => break,
                            }
                        }
                        d
                    }
                };
                let thin_by = 1.0 - 0.4 * dep;
                let sil = ((0.15 * wide).min(4.5 * px) * thin_by).max(2.0 * px);
                match class {
                    1 => (OUTER * sil).min(sil.max(0.2 * wide * thin_by)),
                    2 => sil,
                    _ => (CREASE * sil).max(1.25 * px),
                }
            };
            let line = |q: P2, n: P2| -> u8 {
                let at = |s: f64| view.to_tachyon([q[0] + s * n[0], q[1] + s * n[1]]);
                let ((xi, yi), (xa, ya)) = (at(-eps), at(eps)); // just inside and just across
                let class = if glass(k) {
                    // a transparent surface: where its union ends (no wall of its material across), unless
                    // an opaque shape lies in front of it there
                    let Some(t_in) = it.front(&view, [q[0] - eps * n[0], q[1] - eps * n[1]]) else { return 0 };
                    let tex = tr.prims[it.prim].tex;
                    let same = tr.surfaces(xa, ya).into_iter().any(|p| tr.prims[p as usize].tex == tex);
                    let front = tr.first_opaque(xa, ya).is_some_and(|(_, t)| t < t_in);
                    if same || front { 0 } else { 3 }
                } else {
                    // opaque: only where this item is seen just inside, or its molecule at about its depth (at a
                    // corner of two triangles the probe may fall on the other one)
                    let mine = it.front(&view, [q[0] - eps * n[0], q[1] - eps * n[1]]);
                    let ours = |&(p, t): &(u32, f64)| p as usize == it.prim || (mol[p as usize] == mol[it.prim] && mine.is_some_and(|m| (t - m).abs() < near));
                    // a shape the culling left out (a sliver, e.g. a ribbon's rim seen edge-on) is not drawn: the
                    // probe steps past it, up to 8 times as far
                    let drawn = |s: f64| {
                        let hit = |s: f64| { let (x, y) = at(s); tr.first_opaque(x, y) };
                        (0..4).map(|i| hit(s * f64::from(1 << i))).find(|h| h.is_none_or(|(p, _)| seen[p as usize])).unwrap_or_else(|| hit(8.0 * s))
                    };
                    let Some((p, t_in)) = drawn(-eps).filter(ours) else { return 0 };
                    let across = drawn(eps);
                    let w = match across {
                        None => weight(None, near, far, false, false),
                        Some((o, t)) => weight(Some(t - t_in), near, far, mol[o as usize] != mol[p as usize], it.prim < o as usize),
                    };
                    // across a gap narrower than two lines the same molecule goes on at about this depth: no edge
                    let gap = || {
                        let (xb, yb) = at(eps + 2.0 * base);
                        tr.first_opaque(xb, yb).is_some_and(|(o, t)| mol[o as usize] == mol[p as usize] && (t - t_in).abs() < near)
                    };
                    let deeper = across.is_none_or(|(_, t)| t - t_in > near);
                    if w == 0.0 || (deeper && gap()) { 0 } else if across.is_none() { 1 } else if w >= 1.0 { 2 } else { 3 }
                };
                if class == 0 {
                    return 0;
                }
                // seen through a transparent surface: drawn under its layer
                let under = has_glass && tr.surfaces(xi, yi).first().is_some_and(|&s| glass_prim(tr, s as usize));
                // either edge of the stroke on another part in front of this shape: no line here
                if !glass(k) {
                    let w = ink_width(class, q, n);
                    let t_here = it.front(&view, [q[0] - eps * n[0], q[1] - eps * n[1]]).unwrap_or(f64::MAX);
                    let front_at = |s: f64| { let (x, y) = at(s); tr.first_opaque(x, y).is_some_and(|(p, t)| t < t_here - near && mol[p as usize] != mol[it.prim]) };
                    if front_at(0.5 * w + eps) || front_at(-0.5 * w - eps) {
                        return 0;
                    }
                }
                class | if under { 4 } else { 0 }
            };
            // each stretch of one class sampled about every pixel, with its width there
            let mut out = vec![];
            for lp in loops {
                for a in lp {
                    let c = &curves[a.curve];
                    let s = if a.t1 >= a.t0 { 1.0 } else { -1.0 };
                    for (t0, t1, code) in runs(c, a, &line) {
                        if code == 0 {
                            continue;
                        }
                        let len: f64 = (0..16).map(|i| c.deriv(t0 + (t1 - t0) * (i as f64 + 0.5) / 16.0)).map(|d| d[0].hypot(d[1])).sum::<f64>() * (t1 - t0).abs() / 16.0;
                        let n = (len.ceil() as usize).clamp(1, 20_000);
                        let (pts, ws): (Vec<P2>, Vec<f64>) = (0..=n)
                            .map(|i| {
                                let t = t0 + (t1 - t0) * i as f64 / n as f64;
                                let (q, d) = (c.point(t), c.deriv(t));
                                let l = d[0].hypot(d[1]).max(1e-12);
                                (q, ink_width(code & 3, q, [s * d[1] / l, -s * d[0] / l]))
                            })
                            .unzip();
                        out.push((k, code, pts, ws));
                    }
                }
            }
            out
        })
        .collect();
    // one key per (under the glass, colour): black, or fading with the fog
    let mut keys: BTreeMap<(bool, String), usize> = BTreeMap::new();
    let mut line_pieces = vec![];
    for (k, code, pts, w) in pieces.into_iter().flatten() {
        let colour = match spec.lines {
            LineMode::Ink(c) if fog_lines => hex([0, 1, 2].map(|i| c[i] + (fog_to[i] - c[i]) * fog_t(k))),
            _ => ink.clone(),
        };
        let n = keys.len();
        let key = *keys.entry((code & 4 != 0, colour)).or_insert(n);
        line_pieces.push(crate::line::Piece { key, pts, w });
    }
    let drawn = crate::line::draw(&line_pieces, [f64::from(view.w), f64::from(view.h)], 0.0, 0.06);
    let by_width: BTreeMap<(bool, String), String> = keys.into_iter().filter_map(|(kc, n)| drawn.get(&n).map(|d| (kc, d.clone()))).collect();
    if std::env::var_os("MOL25D_FLAT_TIMES").is_some() {
        let n_drawn = regions.iter().filter(|r| r.is_some()).count();
        let n_left = (0..n_opaque).filter(|&k| !shown[k]).count();
        let t_lines = clock.elapsed().as_secs_f64() - t_cull - t_regions;
        eprintln!("flat: {} items ({n_opaque} opaque, {n_left} unseen), {n_drawn} drawn; seen {t_cull:.2} s, regions {t_regions:.2} s, fills and lines {t_lines:.2} s", all.len());
    }
    {
        let n_shown = (0..all.len()).filter(|&k| regions[k].is_some()).count().max(1) as f64;
        let centre = (0..all.len()).filter(|&k| regions[k].is_some()).map(|k| view.point([0.5 * (all[k].lo[0] + all[k].hi[0]), 0.5 * (all[k].lo[1] + all[k].hi[1])], all[k].key)).fold([0.0; 3], |a, p| [a[0] + p[0] / n_shown, a[1] + p[1] / n_shown, a[2] + p[2] / n_shown]);
        let cast = spec.shadows.unwrap_or(sc.shader == crate::scene::ShaderMode::Full);
        let light = crate::ink::Light::new(tr, &view, spec.light, spec.spot, cast, centre);
        let lit = |k: usize| regions[k].is_some() && !thin(k);
        // the recipe's material for the item's group, else the scene's
        let mat = |k: usize| {
            let name = sc.objects[tr.prims[all[k].prim].obj].group.map_or("", |g| sc.groups[g].as_str());
            spec.mats.iter().find(|(pat, _)| name.contains(pat.as_str())).map_or_else(|| crate::ink::Mat::of(&sc.materials[tr.prims[all[k].prim].tex]), |&(_, m)| m)
        };
        let hides = |j: usize| !glass(j) && shown[j];
        let cuts = |k: usize, j: usize| hides(j) && !(tri(k) && tri(j) && same(k, j));
        let thirds = [zlo + (zhi - zlo) / 3.0, zlo + 2.0 * (zhi - zlo) / 3.0];
        let t_ov = clock.elapsed().as_secs_f64();
        let ov = crate::ink::overlays(tr, &view, &all, &grid, &lit, &mat, &hides, &cuts, &rel, thirds, &ink, span, &light, &|k, cell| cell.map_or_else(|| colour[k].clone(), |i| paint(k, cells[k].0[i].0)), &cells);
        if std::env::var_os("MOL25D_FLAT_TIMES").is_some() {
            let ld = |a: &[std::sync::atomic::AtomicU64; 3]| a.each_ref().map(|x| x.load(std::sync::atomic::Ordering::Relaxed));
            let (ns, n) = (ld(&crate::ink::SHADOW_NS), ld(&crate::ink::SHADOW_N));
            eprintln!("flat: light (tones, shadows, hatching) {:.2} s; cast shadows (CPU s, items) on triangles {:.1} {}, balls {:.1} {}, tubes {:.1} {}", clock.elapsed().as_secs_f64() - t_ov, ns[0] as f64 * 1e-9, n[0], ns[1] as f64 * 1e-9, n[1], ns[2] as f64 * 1e-9, n[2]);
        }
        // fills back to front (the mean depth of their items): a fill's own-colour edge stroke spills a quarter pixel over
        // its neighbours and must land under what lies in front, never over it
        let depth: Vec<f64> = members.iter().map(|ks| ks.iter().map(|&(k, _)| rel(k)).sum::<f64>() / ks.len().max(1) as f64).collect();
        let mut order: Vec<usize> = (0..fills.len()).collect();
        order.sort_by(|&a, &b| depth[b].total_cmp(&depth[a]));
        // emissive fills get rings of glow (stage 4)
        let glows: Vec<bool> = order.iter().map(|&i| members[i].iter().any(|&(k, _)| mat(k) == crate::ink::Mat::Emissive)).collect();
        let fills: Vec<(Fill, String)> = order.into_iter().map(|i| fills[i].clone()).collect();
        Ok(write_ink(sc, &view, &fills, &glows, &by_width, &ink, &hex(bg), &ov))
    }
}

fn glass_prim(tr: &Tracer, p: usize) -> bool {
    (tr.sc.materials[tr.prims[p].tex].opacity as f64) < 1.0 - crate::tachyon::MINCONTRIB
}

/// The regions of one fill merged into one outline: straight edges that two regions share in opposite
/// directions (inside a mesh of one colour) cancel, and what is left is chained into loops. The union is
/// unchanged; the path is shorter, and no renderer draws the inner edges (each one anti-aliased, and with
/// the fill's own-colour stroke, blended again and again: Chrome drifted 2 codes on a dense surface).
pub fn dissolve<'a>(parts: impl Iterator<Item = &'a (Vec<Curve>, Vec<Vec<Arc>>)>) -> (Vec<Curve>, Vec<Vec<Arc>>) {
    type K = [i64; 2];
    // corners match to 1/1000 px: the region engine's corners carry its tolerance (output keeps 1/100 px)
    let q = |p: P2| -> K { [(p[0] * 1e3).round() as i64, (p[1] * 1e3).round() as i64] };
    // every loop as a cyclic list of (curve, t0, t1, start, end)
    let (mut curves, mut loops): (Vec<Curve>, Vec<Vec<(usize, f64, f64, K, K)>>) = (vec![], vec![]);
    for (cs, ls) in parts {
        let off = curves.len();
        curves.extend(cs.iter().cloned());
        for lp in ls {
            loops.push(lp.iter().map(|a| (off + a.curve, a.t0, a.t1, q(cs[a.curve].point(a.t0)), q(cs[a.curve].point(a.t1)))).collect());
        }
    }
    // stage 5: the pieces as they came, the fallback when merging would lose some (below)
    let as_arcs = |lps: Vec<Vec<(usize, f64, f64, K, K)>>| -> Vec<Vec<Arc>> { lps.into_iter().map(|lp| lp.into_iter().map(|a| Arc { curve: a.0, t0: a.1, t1: a.2, v0: usize::MAX, v1: usize::MAX }).collect()).collect() };
    let pieces = loops.clone();
    // a straight edge that another loop's corner lies on is split there (exact regions meet at T-junctions), so edges
    // shared only in part cancel too
    let cell = 4e3; // 4 px
    let ck = |k: K| ((k[0] as f64 / cell).floor() as i64, (k[1] as f64 / cell).floor() as i64);
    let mut corners: std::collections::HashMap<(i64, i64), Vec<K>> = Default::default();
    for a in loops.iter().flatten() {
        corners.entry(ck(a.3)).or_default().push(a.3);
    }
    for v in corners.values_mut() {
        v.sort();
        v.dedup();
    }
    for lp in loops.iter_mut() {
        let mut out = Vec::with_capacity(lp.len());
        for a in lp.iter() {
            if !matches!(curves[a.0], Curve::Seg { .. }) || a.3 == a.4 {
                out.push(*a);
                continue;
            }
            let (p, q) = (a.3.map(|v| v as f64), a.4.map(|v| v as f64));
            let d = [q[0] - p[0], q[1] - p[1]];
            let l2 = d[0] * d[0] + d[1] * d[1];
            let (c0, c1) = (ck(a.3), ck(a.4));
            let mut on: Vec<(f64, K)> = vec![];
            for cx in c0.0.min(c1.0)..=c0.0.max(c1.0) {
                for cy in c0.1.min(c1.1)..=c0.1.max(c1.1) {
                    for &v in corners.get(&(cx, cy)).into_iter().flatten() {
                        let w = [v[0] as f64 - p[0], v[1] as f64 - p[1]];
                        let s = (w[0] * d[0] + w[1] * d[1]) / l2;
                        // within 2/1000 px of the edge, strictly between its ends
                        if s > 0.0 && s < 1.0 && v != a.3 && v != a.4 && (w[0] * d[1] - w[1] * d[0]).abs() < 2.0 * l2.sqrt() {
                            on.push((s, v));
                        }
                    }
                }
            }
            if on.is_empty() || on.len() > 64 {
                out.push(*a);
                continue;
            }
            on.sort_by(|x, y| x.0.total_cmp(&y.0));
            let (mut s0, mut k0) = (0.0, a.3);
            for (s, v) in on.into_iter().chain([(1.0, a.4)]) {
                out.push((a.0, a.1 + (a.2 - a.1) * s0, a.1 + (a.2 - a.1) * s, k0, v));
                (s0, k0) = (s, v);
            }
        }
        *lp = out;
    }
    // straight edges met in both directions cancel
    let mut open: std::collections::HashMap<(K, K), Vec<(usize, usize)>> = Default::default();
    let mut gone: std::collections::HashSet<(usize, usize)> = Default::default();
    for (i, lp) in loops.iter().enumerate() {
        for (j, a) in lp.iter().enumerate() {
            if !matches!(curves[a.0], Curve::Seg { .. }) || a.3 == a.4 {
                continue;
            }
            if let Some(other) = open.get_mut(&(a.4, a.3)).and_then(|v| v.pop()) {
                gone.extend([other, (i, j)]);
            } else {
                open.entry((a.3, a.4)).or_default().push((i, j));
            }
        }
    }
    // what is left: loops untouched as they are; the others' runs between cancelled edges, linked end to start
    let mut out: Vec<Vec<(usize, f64, f64, K, K)>> = vec![];
    let mut chains: Vec<Vec<(usize, f64, f64, K, K)>> = vec![];
    for (i, lp) in loops.into_iter().enumerate() {
        let Some(g) = (0..lp.len()).find(|&j| gone.contains(&(i, j))) else {
            out.push(lp);
            continue;
        };
        let mut run = vec![];
        for s in 1..=lp.len() {
            let j = (g + s) % lp.len();
            if !gone.contains(&(i, j)) {
                run.push(lp[j]);
            } else if !run.is_empty() {
                chains.push(std::mem::take(&mut run));
            }
        }
    }
    let mut by_start: std::collections::HashMap<K, Vec<usize>> = Default::default();
    for (c, ch) in chains.iter().enumerate() {
        by_start.entry(ch[0].3).or_default().push(c);
    }
    let mut used = vec![false; chains.len()];
    for c0 in 0..chains.len() {
        if used[c0] {
            continue;
        }
        let (mut lp, mut c) = (vec![], c0);
        loop {
            used[c] = true;
            lp.extend_from_slice(&chains[c]);
            let end = chains[c].last().map(|a| a.4);
            // a chain whose end meets no other is closed by a straight step if the gap is rounding (under 0.05 px);
            // a wider gap would draw a wedge across whatever lies between: such a loop is dropped
            match end.filter(|&e| e != chains[c0][0].3).and_then(|e| by_start.get(&e)?.iter().copied().find(|&n| !used[n])) {
                Some(n) => c = n,
                None => break,
            }
        }
        let (e, s) = (lp[lp.len() - 1].4, lp[0].3);
        let gap = ((e[0] - s[0]) as f64).hypot((e[1] - s[1]) as f64);
        if gap > 50.0 {
            // stage 5: a loop that does not close would be dropped, and with it a stretch of fill (a ribbon's curl,
            // a ring passing behind itself: same-colour pieces that overlap where they fold). Keep the pieces unmerged
            // instead: a longer path, the whole fill.
            return (curves, as_arcs(pieces));
        }
        out.push(lp);
    }
    // a loop of straight edges enclosing nothing (an edge there and back that did not cancel) is dropped
    out.retain(|lp| {
        if lp.iter().any(|a| !matches!(curves[a.0], Curve::Seg { .. })) {
            return true;
        }
        let area: f64 = lp.iter().map(|a| (a.3[0] as f64) * (a.4[1] as f64) - (a.4[0] as f64) * (a.3[1] as f64)).sum();
        area.abs() > 2e3 // 0.001 px^2, in (1/1000 px)^2 doubled
    });
    (curves, as_arcs(out))
}

/// Per primitive: is it the first opaque surface of some eye ray (n x n rays per pixel)? The style's
/// culling, a level of detail: a shape that shows only in slivers thinner than 1/n pixel is left out, and
/// the shape behind it shows there. (The converter's exact test, `mesh::cull`, proves a shape hidden but
/// costs minutes on a membrane.)
fn seen(tr: &Tracer, view: &View, n: u32) -> Vec<bool> {
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    let seen: Vec<AtomicBool> = (0..tr.prims.len()).map(|_| AtomicBool::new(false)).collect();
    (0..view.h * n).into_par_iter().for_each(|r| {
        let mut last = u32::MAX; // neighbouring rays mostly meet the same shape
        for c in 0..view.w * n {
            let (x, y) = view.to_tachyon([(c as f64 + 0.5) / n as f64, (r as f64 + 0.5) / n as f64]);
            if let Some((p, _)) = tr.first_opaque(x, y) {
                if p != last {
                    seen[p as usize].store(true, Relaxed);
                    last = p;
                }
            }
        }
    });
    seen.into_iter().map(AtomicBool::into_inner).collect()
}

/// Arc `a` of curve `c` cut into stretches by the line each part gets (`line(q, n)`, q on the curve, n the
/// unit normal pointing out of the region, which lies left of its arcs): sampled every half pixel, each
/// change found to 1/256 of that by halving. A change between two samples is missed.
fn runs<K: Copy + PartialEq + Default>(c: &Curve, a: &Arc, line: &dyn Fn(P2, P2) -> K) -> Vec<(f64, f64, K)> {
    let s = if a.t1 >= a.t0 { 1.0 } else { -1.0 };
    let at = |t: f64| {
        let d = c.deriv(t);
        let len = d[0].hypot(d[1]);
        if len == 0.0 { K::default() } else { line(c.point(t), [s * d[1] / len, -s * d[0] / len]) }
    };
    let len: f64 = (0..16).map(|i| c.deriv(a.t0 + (a.t1 - a.t0) * (i as f64 + 0.5) / 16.0)).map(|d| d[0].hypot(d[1])).sum::<f64>() * (a.t1 - a.t0).abs() / 16.0;
    let n = ((len / 0.5).ceil() as usize).clamp(1, 4096);
    let t_at = |i: usize| a.t0 + (a.t1 - a.t0) * (i as f64 + 0.5) / n as f64;
    let mut out = vec![];
    let (mut start, mut prev_t, mut prev) = (a.t0, t_at(0), at(t_at(0)));
    for i in 1..n {
        let (t, key) = (t_at(i), at(t_at(i)));
        if key != prev {
            let (mut lo, mut hi) = (prev_t, t);
            for _ in 0..8 {
                let m = 0.5 * (lo + hi);
                if at(m) == prev { lo = m } else { hi = m }
            }
            let cut = 0.5 * (lo + hi);
            out.push((start, cut, prev));
            (start, prev) = (cut, key);
        }
        prev_t = t;
    }
    out.push((start, a.t1, prev));
    out
}

/// Stage 4 (defect 1): a triangle whose corners carry different colours (VMD's per-vertex colours where a cartoon
/// changes colour) split along the geometry, not given its corners' majority colour (which drew a saw-tooth along the
/// strip). Each corner's colour holds where the barycentric weight of its colour's corners is largest: the cut runs
/// through the middles of the edges that change colour, which the neighbouring triangle shares, so the boundary is one
/// clean line across the tube halfway between VMD's two rings of points. Per colour: (rgb, cell outline on the
/// picture); with the cut lines inside the triangle. Empty for a one-colour triangle.
fn colour_cells(tr: &Tracer, view: &View, prim: usize) -> (Vec<([f64; 3], Vec<P2>)>, Vec<Curve>) {
    let Prim::STri { v0, e1, e2, colors: Some(col), .. } = &tr.prims[prim].prim else { return (vec![], vec![]) };
    if col[0] == col[1] && col[1] == col[2] {
        return (vec![], vec![]);
    }
    let w = [*v0, [0, 1, 2].map(|i| v0[i] + e1[i]), [0, 1, 2].map(|i| v0[i] + e2[i])];
    let at = |b: [f64; 3]| view.project([0, 1, 2].map(|i| b[0] * w[0][i] + b[1] * w[1][i] + b[2] * w[2][i])).0;
    let weight = |c: usize, b: &[f64; 3]| (0..3).filter(|&j| col[j] == col[c]).map(|j| b[j]).sum::<f64>();
    let mid = |i: usize, j: usize| { let mut b = [0.0; 3]; b[i] = 0.5; b[j] = 0.5; b };
    let cuts: Vec<[[f64; 3]; 2]> = match (0..3).find(|&i| col[(i + 1) % 3] == col[(i + 2) % 3]) {
        Some(i) => vec![[mid(i, (i + 1) % 3), mid(i, (i + 2) % 3)]],
        None => (0..3).map(|i| [mid(i, (i + 1) % 3), [1.0 / 3.0; 3]]).collect(),
    };
    let mut cells = vec![];
    for c in (0..3).filter(|&c| (0..c).all(|j| col[j] != col[c])) {
        // the triangle clipped by weight(c) >= weight(d) for each other colour (Sutherland-Hodgman, barycentric)
        let mut poly: Vec<[f64; 3]> = vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for d in (0..3).filter(|&d| col[d] != col[c]) {
            let f = |b: &[f64; 3]| weight(c, b) - weight(d, b);
            let mut out = vec![];
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if f(&a) >= 0.0 {
                    out.push(a);
                }
                if (f(&a) >= 0.0) != (f(&b) >= 0.0) {
                    let s = f(&a) / (f(&a) - f(&b));
                    out.push([0, 1, 2].map(|k| a[k] + s * (b[k] - a[k])));
                }
            }
            poly = out;
        }
        cells.push(([col[c].r, col[c].g, col[c].b].map(f64::from), poly.into_iter().map(at).collect()));
    }
    (cells, cuts.into_iter().map(|[a, b]| Curve::Seg { a: at(a), b: at(b) }).collect())
}

/// The flat colour of primitive `prim`, darkened to k (1: as it is), as `#rrggbb`.
fn fill_colour(tr: &Tracer, prim: usize, k: f64, pale: f64, bg: [f64; 3], fog: f64) -> String {
    let t = &tr.sc.materials[tr.prims[prim].tex].color;
    let c = match &tr.prims[prim].prim {
        // the corners' majority colour (an average would invent in-between colours where colours change)
        Prim::STri { colors: Some(c), .. } => {
            let v = if c[1] == c[2] { c[1] } else { c[0] };
            [v.r, v.g, v.b].map(f64::from)
        }
        _ => [t.r, t.g, t.b].map(f64::from),
    };
    let v = c.map(|v| (pale + (1.0 - pale) * v.clamp(0.0, 1.0)) * k);
    hex([0, 1, 2].map(|i| v[i] + (bg[i] - v[i]) * fog))
}

/// Molecule of every primitive, found from the geometry alone (no names): within one rep, atoms that
/// overlap deeply (centres closer than 0.7 of their radii's sum, as bonded atoms do; atoms that merely
/// touch, as neighbouring molecules do, stay apart), tubes that end in an atom or near another tube's end,
/// and triangles that share a corner. Returns a representative primitive per primitive.
fn molecules(tr: &Tracer) -> Vec<u32> {
    let n = tr.prims.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn root(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            p[i as usize] = p[p[i as usize] as usize];
            i = p[i as usize];
        }
        i
    }
    let join = |p: &mut Vec<u32>, a: usize, b: usize| {
        let (ra, rb) = (root(p, a as u32), root(p, b as u32));
        if ra != rb {
            p[ra.max(rb) as usize] = ra.min(rb);
        }
    };
    let rep = |i: usize| tr.sc.objects[tr.prims[i].obj].group;
    // atoms and tube ends as points with a radius, hashed on a grid of twice the largest radius
    let mut pts: Vec<(usize, [f64; 3], f64, bool)> = vec![]; // (prim, point, radius, is an atom)
    let mut corners: std::collections::HashMap<([u32; 3], Option<usize>), usize> = Default::default();
    for (i, p) in tr.prims.iter().enumerate() {
        match &p.prim {
            Prim::Sphere { ctr, rad } => pts.push((i, *ctr, *rad, true)),
            Prim::FCyl { ctr, axis, rad } => {
                pts.push((i, *ctr, *rad, false));
                pts.push((i, [ctr[0] + axis[0], ctr[1] + axis[1], ctr[2] + axis[2]], *rad, false));
            }
            Prim::Tri { v0, e1, e2 } | Prim::STri { v0, e1, e2, .. } => {
                for v in [*v0, [v0[0] + e1[0], v0[1] + e1[1], v0[2] + e1[2]], [v0[0] + e2[0], v0[1] + e2[1], v0[2] + e2[2]]] {
                    // corners shared exactly (strips and meshes repeat the same numbers)
                    let key = (v.map(|x| (x as f32).to_bits()), rep(i));
                    match corners.get(&key) {
                        Some(&j) => join(&mut parent, i, j),
                        None => {
                            corners.insert(key, i);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let reach = pts.iter().fold(0.0f64, |m, p| m.max(p.2));
    if reach > 0.0 {
        let cell = 2.0 * reach;
        let key = |x: [f64; 3]| x.map(|v| (v / cell).floor() as i64);
        let mut grid: std::collections::HashMap<[i64; 3], Vec<usize>> = Default::default();
        for (n, p) in pts.iter().enumerate() {
            grid.entry(key(p.1)).or_default().push(n);
        }
        for (a, p) in pts.iter().enumerate() {
            let c = key(p.1);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(v) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) else { continue };
                        for &b in v.iter().filter(|&&b| b > a) {
                            let q = &pts[b];
                            if p.0 == q.0 || rep(p.0) != rep(q.0) {
                                continue;
                            }
                            let d = ((p.1[0] - q.1[0]).powi(2) + (p.1[1] - q.1[1]).powi(2) + (p.1[2] - q.1[2]).powi(2)).sqrt();
                            let bonded = match (p.3, q.3) {
                                (true, true) => d < 0.7 * (p.2 + q.2),
                                (true, false) => d < p.2 + 0.5 * q.2, // a tube ending in an atom
                                (false, true) => d < q.2 + 0.5 * p.2,
                                // tubes meeting at an atom: VMD's Bonds rep leaves their ends about a radius apart
                                (false, false) => d < 1.5 * p.2.min(q.2),
                            };
                            if bonded {
                                join(&mut parent, p.0, q.0);
                            }
                        }
                    }
                }
            }
        }
    }
    (0..n as u32).map(|i| root(&mut parent, i)).collect()
}

/// The SVG: background; per rep one path per fill colour; the lines seen through transparent surfaces;
/// the transparent layers; the other lines, one path per width.
/// A fill path's rep, colour, opacity (None: opaque) and stroke width in its own colour (1/100 px).
type Fill = (Option<usize>, String, Option<f32>, u32);


/// Comic Ink's file (stage 2): named groups an editor finds at once: the fills (one group per scene group), tones,
/// shadows, hatching, highlights and lines; a transparent surface's veil goes over the lines seen through it.
#[allow(clippy::too_many_arguments)]
fn write_ink(sc: &crate::scene::Scene, view: &View, fills: &[(Fill, String)], glows: &[bool], lines: &BTreeMap<(bool, String), String>, ink: &str, bg: &str, ov: &crate::ink::Overlays) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">", w = view.w, h = view.h);
    let _ = writeln!(out, "<rect id=\"background\" width=\"{}\" height=\"{}\" fill=\"{bg}\"/>", view.w, view.h);
    let mut ids = std::collections::HashSet::new();
    out.push_str("<g id=\"fills\">\n");
    let mut current: Option<Option<usize>> = None;
    // an emissive shape's glow: its outline stroked three times, each wider and as faint (rings of vector paint, no filter)
    let glow_w = GLOW * f64::from(view.w) / 2400.0;
    for (((rep, colour, _, pen), d), &glow) in fills.iter().zip(glows).filter(|f| f.0.0.2.is_none()) {
        if current != Some(*rep) {
            if current.is_some() {
                out.push_str("</g>\n");
            }
            let label = rep.map(|r| sc.groups[r].clone()).unwrap_or_else(|| "graphics".into());
            let _ = writeln!(out, "<g id=\"{}\">", svg::xml_id(&label, &mut ids));
            current = Some(*rep);
        }
        if glow {
            for i in 1..=3 {
                let _ = writeln!(out, "<path class=\"glow\" fill=\"none\" stroke=\"{colour}\" stroke-opacity=\"0.22\" stroke-width=\"{}\" stroke-linejoin=\"round\" d=\"{d}\"/>", svg::num(2.0 * glow_w * f64::from(i)));
            }
        }
        // a stroke in the fill's own colour: half a pixel closes the hairlines where colours meet
        let _ = writeln!(out, "<path fill=\"{colour}\" stroke=\"{colour}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"{d}\"/>", svg::num(f64::from(*pen) / 100.0));
    }
    if current.is_some() {
        out.push_str("</g>\n");
    }
    out.push_str("</g>\n");
    for (id, body) in [("tones", &ov.tones), ("shadows", &ov.shadows), ("hatching", &ov.hatching), ("highlights", &ov.highlights)] {
        let _ = write!(out, "<g id=\"{id}\">\n{body}</g>\n");
    }
    // every line one filled shape (`line.rs`), one path per colour
    let put_lines = |out: &mut String, under: bool, id: &str| {
        // a 0.3 px edge in the line's own colour closes the hairline slits where a shape folds on its inner side
        let _ = writeln!(out, "<g id=\"{id}\" fill=\"{ink}\" stroke=\"{ink}\" stroke-width=\"0.3\" stroke-linejoin=\"round\">");
        for ((_, c), d) in lines.iter().filter(|(k, _)| k.0 == under) {
            let colour = if c == ink { String::new() } else { format!(" fill=\"{c}\" stroke=\"{c}\"") };
            let _ = writeln!(out, "<path{colour} d=\"{d}\"/>");
        }
        out.push_str("</g>\n");
    };
    if lines.keys().any(|k| k.0) {
        put_lines(&mut out, true, "lines under glass");
    }
    let glass: Vec<_> = fills.iter().filter(|f| f.0.2.is_some()).collect();
    if !glass.is_empty() {
        out.push_str("<g id=\"glass\">\n");
        for ((_, colour, alpha, _), d) in glass {
            let _ = writeln!(out, "<path fill=\"{colour}\" fill-opacity=\"{}\" d=\"{d}\"/>", svg::frac(f64::from(alpha.unwrap_or(1.0))));
        }
        out.push_str(&ov.glass);
        out.push_str("</g>\n");
    }
    put_lines(&mut out, false, "lines");
    out.push_str("</svg>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Defect 1 (stage 4): a triangle with corners A A B splits along the line through the middles of its two A-B edges
    /// (shared with the neighbouring triangles, so the cut runs straight across the strip), into an A quad and a B triangle.
    #[test]
    fn two_colour_triangle_splits_at_edge_middles() {
        let sc = crate::read_tachyon::parse(
            "Begin_Scene\nResolution 200 200\nShader_Mode Medium\nEnd_Shader_Mode\nCamera\n  Projection Orthographic\n  Zoom 0.5\n  Aspectratio 1\n  \
             Antialiasing 0\n  Raydepth 50\n  Center 0 0 -2\n  Viewdir 0 0 2\n  Updir 0 1 0\nEnd_Camera\nDirectional_Light Direction 0 0 1 Color 1 1 1\n\
             Background 1 1 1\nVCSTri V0 -1 -1 0 V1 1 -1 0 V2 0 1 0 N0 0 0 -1 N1 0 0 -1 N2 0 0 -1 C0 1 0 0 C1 1 0 0 C2 0 0 1\n\
             Ambient 0 Diffuse 0.65 Specular 0 Opacity 1\n  Phong Plastic 0.5 Phong_size 40 VCST\nEnd_Scene\n",
        )
        .unwrap();
        let tr = Tracer::new(&sc);
        let view = View::new(&tr);
        let (cells, cuts) = colour_cells(&tr, &view, 0);
        let px = |p: [f64; 3]| view.project(p).0;
        let (m02, m12) = (px([-0.5, 0.0, 0.0]), px([0.5, 0.0, 0.0]));
        assert_eq!(cuts.len(), 1);
        let Curve::Seg { a, b } = cuts[0] else { panic!() };
        let near = |p: P2, q: P2| (p[0] - q[0]).hypot(p[1] - q[1]) < 1e-9;
        assert!((near(a, m02) && near(b, m12)) || (near(a, m12) && near(b, m02)), "cut {a:?} {b:?}");
        assert_eq!(cells.iter().map(|c| c.1.len()).collect::<Vec<_>>(), vec![4, 3], "an A quad and a B triangle");
    }

    /// The line rule: the background and deep jumps get a full line, a moderate jump a thinner one (heavier
    /// the deeper), the shape across in front draws nothing here, and two shapes at about the same depth get
    /// a line only between two molecules, drawn by one side.
    #[test]
    fn lines_follow_depth_jumps_and_molecules() {
        let (near, far) = (0.1, 1.0);
        assert_eq!(weight(None, near, far, false, false), 1.0);
        assert_eq!(weight(Some(2.0), near, far, false, false), 1.0);
        let (a, b) = (weight(Some(0.3), near, far, false, false), weight(Some(0.6), near, far, false, false));
        assert!(0.3 < a && a < b && b < 1.0);
        assert_eq!(weight(Some(-0.5), near, far, true, true), 0.0);
        assert_eq!(weight(Some(0.01), near, far, false, true), 0.0);
        assert_eq!(weight(Some(0.01), near, far, true, true), 0.3);
        assert_eq!(weight(Some(-0.01), near, far, true, false), 0.0);
    }
}
