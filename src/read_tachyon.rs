//! Reader for Tachyon scene files (`.dat`), as VMD's `render Tachyon` writes them, into the internal `Scene`.
//!
//! Follows Tachyon 0.99.5 `demosrc/parse.c` (Copyright (c) 1994-2022 John E. Stone, BSD-3-Clause):
//! keywords are case-insensitive, every number is read as `f32` and widened to `f64` (as Tachyon's
//! `fscanf("%f")` into `float`), lights are kept in file order (the renderer reverses them like
//! Tachyon's linked list). `# Mol[m] Rep[r] Name` comments name the group of what follows.

use crate::scene::*;
use std::fmt;

#[derive(Debug)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "scene parse error: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

type Res<T> = Result<T, ParseError>;

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn word(&mut self) -> Option<&'a str> {
        let b = self.src.as_bytes();
        while self.pos < b.len() && b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        if self.pos >= b.len() {
            return None;
        }
        let s = self.pos;
        while self.pos < b.len() && !b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        Some(&self.src[s..self.pos])
    }
    fn rest_of_line(&mut self) -> &'a str {
        let b = self.src.as_bytes();
        let s = self.pos;
        while self.pos < b.len() && b[self.pos] != b'\n' {
            self.pos += 1;
        }
        &self.src[s..self.pos]
    }
    fn need(&mut self) -> Res<&'a str> {
        self.word().ok_or_else(|| ParseError("unexpected end of file".into()))
    }
    fn expect(&mut self, kw: &str) -> Res<()> {
        let w = self.need()?;
        if w.eq_ignore_ascii_case(kw) {
            Ok(())
        } else {
            Err(ParseError(format!("expected {kw}, found {w}")))
        }
    }
    fn f(&mut self) -> Res<f64> {
        let w = self.need()?;
        // fscanf("%f") into a float, then widened.
        w.parse::<f32>().map(|x| x as f64).map_err(|_| ParseError(format!("bad number {w}")))
    }
    fn f32(&mut self) -> Res<f32> {
        let w = self.need()?;
        w.parse::<f32>().map_err(|_| ParseError(format!("bad number {w}")))
    }
    fn i(&mut self) -> Res<i64> {
        let w = self.need()?;
        w.parse::<i64>().map_err(|_| ParseError(format!("bad integer {w}")))
    }
    fn v(&mut self) -> Res<V3> {
        Ok([self.f()?, self.f()?, self.f()?])
    }
    fn rgb(&mut self) -> Res<Rgb> {
        Ok(Rgb { r: self.f32()?, g: self.f32()?, b: self.f32()? })
    }
}

fn is(w: &str, kw: &str) -> bool {
    w.eq_ignore_ascii_case(kw)
}

/// Parse the body of a `Texture` block (after the `Texture` keyword) into a material. `vcst` = VCSTri form.
fn tex_body(lx: &mut Lexer, vcst: bool) -> Res<Material> {
    lx.expect("Ambient")?;
    let ambient = lx.f32()?;
    lx.expect("Diffuse")?;
    let diffuse = lx.f32()?;
    lx.expect("Specular")?;
    let specular = lx.f32()?;
    lx.expect("Opacity")?;
    let opacity = lx.f32()?;
    let mut t = Material {
        ambient,
        diffuse,
        specular,
        opacity,
        raster3d: false,
        outline: 0.0,
        outline_width: 0.0,
        phong: 0.0,
        phong_exp: 100.0,
        phong_type: PhongType::Plastic,
        color: Rgb { r: 1.0, g: 1.0, b: 1.0 },
        texfunc: 0,
    };
    let mut w = lx.need()?;
    if is(w, "TransMode") {
        t.raster3d = is(lx.need()?, "R3D");
        w = lx.need()?;
    }
    if is(w, "Outline") {
        t.outline = lx.f32()?;
        lx.expect("Outline_Width")?;
        t.outline_width = lx.f32()?;
        w = lx.need()?;
    }
    if is(w, "Phong") {
        let ty = lx.need()?;
        if is(ty, "Metal") {
            t.phong_type = PhongType::Metal;
        }
        t.phong = lx.f32()?;
        lx.expect("Phong_size")?;
        t.phong_exp = lx.f32()?;
        w = lx.need()?;
    }
    if vcst {
        if !is(w, "VCST") {
            return Err(ParseError(format!("expected VCST, found {w}")));
        }
        return Ok(t);
    }
    if !is(w, "Color") {
        return Err(ParseError(format!("expected Color, found {w}")));
    }
    t.color = lx.rgb()?;
    lx.expect("TexFunc")?;
    t.texfunc = lx.i()? as i32;
    if t.texfunc != 0 {
        return Err(ParseError(format!("TexFunc {} (procedural/image materials) not supported", t.texfunc)));
    }
    Ok(t)
}

pub fn parse(src: &str) -> Res<Scene> {
    let mut lx = Lexer { src, pos: 0 };
    let mut sc = Scene::empty();
    let mut clip: Option<usize> = None;
    let mut group: Option<usize> = None;
    let mut tex_cache: Option<(Material, usize)> = None;

    let mut add_tex = |sc: &mut Scene, t: Material| -> usize {
        // Consecutive identical materials share one entry (VMD repeats the block per primitive).
        if let Some((prev, i)) = &tex_cache {
            if *prev == t {
                return *i;
            }
        }
        sc.materials.push(t.clone());
        let i = sc.materials.len() - 1;
        tex_cache = Some((t, i));
        i
    };

    // comment lines may precede Begin_Scene
    loop {
        let w = lx.need()?;
        if w.starts_with('#') {
            lx.rest_of_line();
        } else if is(w, "Begin_Scene") {
            break;
        } else {
            return Err(ParseError(format!("expected Begin_Scene, found {w}")));
        }
    }
    while let Some(w) = lx.word() {
        let push = |sc: &mut Scene, shape: Shape, tex: usize| sc.objects.push(Object { shape, tex, clip, group });
        if w.starts_with('#') {
            let line = lx.rest_of_line();
            let full = format!("{} {}", w, line);
            if let Some(name) = group_label(&full) {
                // one group, one group, whatever VMD calls its parts (a57 labels a Cartoon's coils
                // "Tube" and a Licorice's joints "VDW" under the same Mol[m] Rep[r]): the group's label
                // lists every name once
                let key = |s: &str| s.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                let part = name.split_whitespace().skip(2).collect::<Vec<_>>().join(" ");
                group = Some(match sc.groups.iter().position(|r| key(r) == key(&name)) {
                    Some(i) => {
                        if !part.is_empty() && !sc.groups[i].split_whitespace().skip(2).any(|w| w == part) {
                            sc.groups[i] = format!("{} {part}", sc.groups[i]);
                        }
                        i
                    }
                    None => {
                        sc.groups.push(name);
                        sc.groups.len() - 1
                    }
                });
            }
        } else if is(w, "End_Scene") {
            break;
        } else if is(w, "Resolution") {
            sc.hres = lx.i()? as u32;
            sc.vres = lx.i()? as u32;
        } else if is(w, "Shader_Mode") {
            let m = lx.need()?;
            sc.shader = match m.to_ascii_uppercase().as_str() {
                "FULL" => ShaderMode::Full,
                "MEDIUM" => ShaderMode::Medium,
                "LOW" => ShaderMode::Low,
                "LOWEST" => ShaderMode::Lowest,
                _ => return Err(ParseError(format!("bad shader mode {m}"))),
            };
            loop {
                let o = lx.need()?;
                match o.to_ascii_uppercase().as_str() {
                    "END_SHADER_MODE" => break,
                    "SHADOW_FILTER_ON" => sc.shadow_filter = true,
                    "SHADOW_FILTER_OFF" => sc.shadow_filter = false,
                    "TRANS_MAX_SURFACES" => {
                        lx.i()?;
                    }
                    "TRANS_ORIG" => {
                        sc.trans_vmd = false;
                        sc.trans_raster3d = false;
                    }
                    "TRANS_RASTER3D" => sc.trans_raster3d = true,
                    "TRANS_VMD" => sc.trans_vmd = true,
                    "FOG_VMD" => sc.fog_vmd = true,
                    "AMBIENT_OCCLUSION" => {
                        let mut maxdist = 3.402823e+38_f32 as f64;
                        let mut k = lx.need()?;
                        if is(k, "Maxdist") {
                            maxdist = lx.f()?;
                            k = lx.need()?;
                        }
                        if !is(k, "Ambient_Color") {
                            return Err(ParseError(format!("expected Ambient_Color, found {k}")));
                        }
                        let color = lx.rgb()?;
                        lx.expect("Rescale_Direct")?;
                        sc.light_scale = lx.f()?;
                        lx.expect("Samples")?;
                        let samples = lx.i()? as u32;
                        sc.ao = Some(Ao { samples, maxdist, color });
                    }
                    _ => return Err(ParseError(format!("bad shader mode option {o}"))),
                }
            }
        } else if is(w, "Camera") {
            let c = &mut sc.camera;
            let mut k = lx.need()?;
            if is(k, "Projection") {
                let p = lx.need()?;
                c.projection = match p.to_ascii_uppercase().as_str() {
                    "ORTHOGRAPHIC" => Projection::Orthographic,
                    "PERSPECTIVE" => Projection::Perspective,
                    "PERSPECTIVE_DOF" => {
                        lx.expect("FocalDist")?;
                        c.focaldist = lx.f()?;
                        lx.expect("Aperture")?;
                        c.aperture = lx.f()?;
                        Projection::PerspectiveDof
                    }
                    _ => return Err(ParseError(format!("projection {p} not supported"))),
                };
                k = lx.need()?;
            }
            if !is(k, "Zoom") {
                return Err(ParseError(format!("expected Zoom, found {k}")));
            }
            c.zoom = lx.f()?;
            lx.expect("Aspectratio")?;
            c.aspect = lx.f()?;
            lx.expect("Antialiasing")?;
            c.antialiasing = lx.i()?.max(0) as u32;
            lx.expect("Raydepth")?;
            c.raydepth = lx.i()? as i32;
            lx.expect("Center")?;
            c.center = lx.v()?;
            lx.expect("Viewdir")?;
            c.viewdir = lx.v()?;
            lx.expect("Updir")?;
            c.updir = lx.v()?;
            let e = lx.need()?;
            if is(e, "Frustum") {
                c.frustum = Some([lx.f()?, lx.f()?, lx.f()?, lx.f()?]);
                lx.expect("End_Camera")?;
            } else if !is(e, "End_Camera") {
                return Err(ParseError(format!("expected End_Camera, found {e}")));
            }
        } else if is(w, "Directional_Light") {
            lx.expect("Direction")?;
            let dir = lx.v()?;
            lx.expect("Color")?;
            let color = lx.rgb()?;
            sc.lights.push(Light::Directional { dir, color });
        } else if is(w, "Light") {
            lx.expect("Center")?;
            let center = lx.v()?;
            lx.expect("Rad")?;
            let rad = lx.f()?;
            let k = lx.need()?;
            if !is(k, "Color") {
                return Err(ParseError("light attenuation options not supported".into()));
            }
            let color = lx.rgb()?;
            sc.lights.push(Light::Point { center, rad, color });
        } else if is(w, "Background") {
            sc.background = Background::Solid(lx.rgb()?);
        } else if is(w, "Background_Gradient") {
            let kind = lx.need()?;
            let sky_sphere = is(kind, "Sky_Sphere");
            if !sky_sphere && !is(kind, "Sky_Ortho_Plane") {
                return Err(ParseError(format!("bad gradient {kind}")));
            }
            lx.expect("UpDir")?;
            let updir = lx.v()?;
            lx.expect("TopVal")?;
            let top = lx.f()?;
            lx.expect("BottomVal")?;
            let bottom = lx.f()?;
            lx.expect("TopColor")?;
            let top_color = lx.rgb()?;
            lx.expect("BottomColor")?;
            let bottom_color = lx.rgb()?;
            sc.background = Background::Gradient { sky_sphere, updir, top, bottom, top_color, bottom_color };
        } else if is(w, "Fog") {
            let k = lx.need()?;
            sc.fog.kind = match k.to_ascii_uppercase().as_str() {
                "LINEAR" => FogKind::Linear,
                "EXP" => FogKind::Exp,
                "EXP2" => FogKind::Exp2,
                _ => FogKind::None,
            };
            lx.expect("Start")?;
            sc.fog.start = lx.f()?;
            lx.expect("End")?;
            sc.fog.end = lx.f()?;
            lx.expect("Density")?;
            sc.fog.density = lx.f()?;
            lx.expect("Color")?;
            sc.fog.color = lx.rgb()?;
        } else if is(w, "Sphere") {
            lx.expect("Center")?;
            let center = lx.v()?;
            lx.expect("Rad")?;
            let rad = lx.f()?;
            lx.expect("Texture")?;
            let t = add_tex(&mut sc, tex_body(&mut lx, false)?);
            push(&mut sc, Shape::Sphere { center, rad }, t);
        } else if is(w, "FCylinder") {
            lx.expect("Base")?;
            let base = lx.v()?;
            lx.expect("Apex")?;
            let apex = lx.v()?;
            lx.expect("Rad")?;
            let rad = lx.f()?;
            lx.expect("Texture")?;
            let t = add_tex(&mut sc, tex_body(&mut lx, false)?);
            let axis = [apex[0] - base[0], apex[1] - base[1], apex[2] - base[2]];
            push(&mut sc, Shape::FCylinder { base, axis, rad }, t);
        } else if is(w, "Tri") {
            lx.expect("V0")?;
            let a = lx.v()?;
            lx.expect("V1")?;
            let b = lx.v()?;
            lx.expect("V2")?;
            let c = lx.v()?;
            lx.expect("Texture")?;
            let t = add_tex(&mut sc, tex_body(&mut lx, false)?);
            push(&mut sc, Shape::Tri { v: [a, b, c] }, t);
        } else if is(w, "STri") || is(w, "VCSTri") {
            let vc = is(w, "VCSTri");
            let mut v = [[0.0; 3]; 3];
            let mut n = [[0.0; 3]; 3];
            for (i, vi) in v.iter_mut().enumerate() {
                lx.expect(&format!("V{i}"))?;
                *vi = lx.v()?;
            }
            for (i, ni) in n.iter_mut().enumerate() {
                lx.expect(&format!("N{i}"))?;
                *ni = lx.v()?;
            }
            let colors = if vc {
                let mut c = [Rgb { r: 0.0, g: 0.0, b: 0.0 }; 3];
                for (i, ci) in c.iter_mut().enumerate() {
                    lx.expect(&format!("C{i}"))?;
                    *ci = lx.rgb()?;
                }
                Some(c)
            } else {
                lx.expect("Texture")?;
                None
            };
            let t = add_tex(&mut sc, tex_body(&mut lx, vc)?);
            push(&mut sc, Shape::STri { v, n, colors }, t);
        } else if is(w, "Ring") {
            lx.expect("Center")?;
            let center = lx.v()?;
            lx.expect("Normal")?;
            let normal = lx.v()?;
            lx.expect("Inner")?;
            let inner = lx.f()?;
            lx.expect("Outer")?;
            let outer = lx.f()?;
            lx.expect("Texture")?;
            let t = add_tex(&mut sc, tex_body(&mut lx, false)?);
            push(&mut sc, Shape::Ring { center, normal, inner, outer }, t);
        } else if is(w, "Plane") {
            lx.expect("Center")?;
            let center = lx.v()?;
            lx.expect("Normal")?;
            let normal = lx.v()?;
            lx.expect("Texture")?;
            let t = add_tex(&mut sc, tex_body(&mut lx, false)?);
            push(&mut sc, Shape::Plane { center, normal }, t);
        } else if is(w, "VertexArray") {
            vertex_array(&mut lx, &mut sc, &mut add_tex, clip, group)?;
        } else if is(w, "Start_ClipGroup") {
            lx.expect("NumPlanes")?;
            let k = lx.i()? as usize;
            let mut planes = Vec::with_capacity(k);
            for _ in 0..k {
                planes.push([lx.f32()?, lx.f32()?, lx.f32()?, lx.f32()?]);
            }
            sc.clips.push(planes);
            clip = Some(sc.clips.len() - 1);
        } else if is(w, "End_ClipGroup") {
            clip = None;
        } else {
            return Err(ParseError(format!("unsupported keyword {w}")));
        }
    }
    Ok(sc)
}

/// `# Mol[0] Rep[2] NewCartoon` -> "Mol[0] Rep[2] NewCartoon"; also `# VMD Axes`.
fn group_label(comment: &str) -> Option<String> {
    let s = comment.trim_start_matches('#').trim();
    if (s.starts_with("Mol[") && s.contains("Rep[")) || s == "VMD Axes" {
        Some(s.to_string())
    } else {
        None
    }
}

fn vertex_array(
    lx: &mut Lexer,
    sc: &mut Scene,
    add_tex: &mut impl FnMut(&mut Scene, Material) -> usize,
    clip: Option<usize>,
    group: Option<usize>,
) -> Res<()> {
    lx.expect("Numverts")?;
    let nv = lx.i()? as usize;
    let read = |lx: &mut Lexer, kw: &str| -> Res<Vec<V3>> {
        lx.expect(kw)?;
        (0..nv).map(|_| lx.v()).collect()
    };
    let v = read(lx, "Coords")?;
    let n = read(lx, "Normals")?;
    let mut c: Option<Vec<Rgb>> = None;
    let mut tex: Option<usize> = None;
    loop {
        let w = lx.need()?;
        if is(w, "Colors") {
            c = Some((0..nv).map(|_| lx.rgb()).collect::<Res<_>>()?);
        } else if is(w, "Texture") {
            let t = tex_body(lx, false)?;
            tex = Some(add_tex(sc, t));
        } else if is(w, "TriStrip") {
            let k = lx.i()? as usize;
            let f: Vec<usize> = (0..k).map(|_| lx.i().map(|x| x as usize)).collect::<Res<_>>()?;
            let t = tex.ok_or_else(|| ParseError("vertex array without texture".into()))?;
            for s in 0..k.saturating_sub(2) {
                let (a, b, d) = if s & 1 == 0 { (f[s], f[s + 1], f[s + 2]) } else { (f[s + 1], f[s], f[s + 2]) };
                tri_from_array(sc, &v, &n, &c, [a, b, d], t, clip, group)?;
            }
        } else if is(w, "TriMesh") {
            let k = lx.i()? as usize;
            let t = tex.ok_or_else(|| ParseError("vertex array without texture".into()))?;
            for _ in 0..k {
                let ids = [lx.i()? as usize, lx.i()? as usize, lx.i()? as usize];
                tri_from_array(sc, &v, &n, &c, ids, t, clip, group)?;
            }
        } else if is(w, "End_VertexArray") {
            return Ok(());
        } else {
            return Err(ParseError(format!("unexpected {w} in VertexArray")));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn tri_from_array(
    sc: &mut Scene,
    v: &[V3],
    n: &[V3],
    c: &Option<Vec<Rgb>>,
    ids: [usize; 3],
    tex: usize,
    clip: Option<usize>,
    group: Option<usize>,
) -> Res<()> {
    if ids.iter().any(|&i| i >= v.len()) {
        return Err(ParseError("vertex index out of range".into()));
    }
    let shape = Shape::STri {
        v: ids.map(|i| v[i]),
        n: ids.map(|i| n[i]),
        colors: c.as_ref().map(|c| ids.map(|i| c[i])),
    };
    sc.objects.push(Object { shape, tex, clip, group });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = "
Begin_Scene
Resolution 64 48
Shader_Mode Full
  Trans_VMD
  Fog_VMD
  Ambient_Occlusion
    Ambient_Color 0.8 0.8 0.8
    Rescale_Direct 0.3
    Samples 12
End_Shader_Mode
Camera
  Projection Orthographic
  Zoom 0.333333
  Aspectratio 1
  Antialiasing 12
  Raydepth 50
  Center  0 0 -2
  Viewdir -0 -0 2
  Updir   0 1 -0
End_Camera
Directional_Light Direction 0.1 -0.1 1 Color 1 1 1
Background 1 1 1
# Mol[0] Rep[0] VDW
Sphere
  Center 0 0 0
   Rad 0.5
Texture
  Ambient 0 Diffuse 0.65 Specular 0 Opacity 1
  Phong Plastic 0.5 Phong_size 40 Color 0 0 1 TexFunc 0
# Mol[0] Rep[1] NewCartoon
VertexArray  Numverts 4
Coords
0 0 0
1 0 0
0 1 0
1 1 0
Normals
0 0 -1
0 0 -1
0 0 -1
0 0 -1
Colors
1 0 0
0 1 0
0 0 1
1 1 1
Texture
  Ambient 0 Diffuse 0.85 Specular 0 Opacity 1
  Phong Plastic 0 Phong_size 38.9045 Color 0 0 1 TexFunc 0
TriStrip 4
0 1 2 3
End_VertexArray
End_Scene
";

    #[test]
    fn a_reps_parts_under_other_names_stay_one_rep() {
        let more = "# Mol[0] Rep[0] Tube\nSphere Center 1 1 1 Rad 0.5 Texture Ambient 0 Diffuse 0.65 Specular 0 Opacity 1\n\
                    Phong Plastic 0.5 Phong_size 40 Color 0 0 1 TexFunc 0\nEnd_Scene";
        let s = parse(&SMALL.replace("End_Scene", more)).unwrap();
        assert_eq!(s.groups, vec!["Mol[0] Rep[0] VDW Tube", "Mol[0] Rep[1] NewCartoon"]);
        assert_eq!(s.objects.last().unwrap().group, Some(0));
    }

    #[test]
    fn parses_header_objects_and_reps() {
        let s = parse(SMALL).unwrap();
        assert_eq!((s.hres, s.vres), (64, 48));
        assert!(s.trans_vmd && s.fog_vmd && s.ao.is_some());
        assert_eq!(s.light_scale, 0.3_f32 as f64);
        assert_eq!(s.camera.antialiasing, 12);
        assert_eq!(s.objects.len(), 3);
        assert_eq!(s.groups, vec!["Mol[0] Rep[0] VDW", "Mol[0] Rep[1] NewCartoon"]);
        assert_eq!(s.objects[0].group, Some(0));
        assert_eq!(s.objects[2].group, Some(1));
        // strip winding: (0,1,2) then (2,1,3)
        if let Shape::STri { v, colors, .. } = &s.objects[2].shape {
            assert_eq!(v[0], [0.0, 1.0, 0.0]);
            assert_eq!(v[1], [1.0, 0.0, 0.0]);
            assert_eq!(v[2], [1.0, 1.0, 0.0]);
            assert!(colors.is_some());
        } else {
            panic!()
        }
    }
}
