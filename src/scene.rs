//! The internal scene: what every reader produces and every drawing step reads (stage 1, ADR 0012).
//!
//! A camera, lights, materials, primitives (spheres, open tubes, flat and smooth triangles, rings, planes) and
//! named groups. Nothing here is about molecules: a group is a free-form name the reader gives a run of primitives
//! (from VMD, "Mol[0] Rep[2] NewCartoon"; from a mesh file, its object names). Renderer hints a source may carry
//! (Tachyon's shader mode, fog style, transparency mode) keep their source's defaults when a reader has none.
//! Readers: `read_tachyon` (VMD's `render Tachyon` files); OBJ and glTF follow in stage 4.

pub type V3 = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    Orthographic,
    Perspective,
    PerspectiveDof,
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub projection: Projection,
    pub zoom: f64,
    pub aspect: f64,
    pub antialiasing: u32,
    pub raydepth: i32,
    pub center: V3,
    pub viewdir: V3,
    pub updir: V3,
    pub focaldist: f64,
    pub aperture: f64,
    pub frustum: Option<[f64; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhongType {
    Plastic,
    Metal,
}

/// One material (`Material` block). Field types follow Tachyon's `texture` struct (`float`).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub ambient: f32,
    pub diffuse: f32,
    pub specular: f32, // mirror reflection
    pub opacity: f32,
    pub raster3d: bool, // TransMode R3D
    pub outline: f32,
    pub outline_width: f32,
    pub phong: f32,
    pub phong_exp: f32,
    pub phong_type: PhongType,
    pub color: Rgb,
    pub texfunc: i32,
}

impl Material {
    pub fn shadowcast(&self) -> bool {
        (self.opacity as f64) >= 0.99999
    }
}

#[derive(Clone, Debug)]
pub enum Shape {
    Sphere { center: V3, rad: f64 },
    /// Open finite cylinder: `base` and `axis` = apex - base (not normalised), as Tachyon stores it.
    FCylinder { base: V3, axis: V3, rad: f64 },
    /// Flat triangle (normal from winding).
    Tri { v: [V3; 3] },
    /// Smooth triangle; `colors` set for VCSTri / vertex arrays with colours.
    STri { v: [V3; 3], n: [V3; 3], colors: Option<[Rgb; 3]> },
    Ring { center: V3, normal: V3, inner: f64, outer: f64 },
    /// Infinite plane.
    Plane { center: V3, normal: V3 },
}

#[derive(Clone, Debug)]
pub struct Object {
    pub shape: Shape,
    pub tex: usize,
    pub clip: Option<usize>,
    /// Index into `Scene::groups` (None: in no group, e.g. VMD's axes and user graphics).
    pub group: Option<usize>,
}

#[derive(Clone, Debug)]
pub enum Light {
    Directional { dir: V3, color: Rgb },
    Point { center: V3, rad: f64, color: Rgb },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FogKind {
    None,
    Linear,
    Exp,
    Exp2,
}

#[derive(Clone, Debug)]
pub struct Fog {
    pub kind: FogKind,
    pub start: f64,
    pub end: f64,
    pub density: f64,
    pub color: Rgb,
}

#[derive(Clone, Debug)]
pub enum Background {
    Solid(Rgb),
    Gradient { sky_sphere: bool, updir: V3, top: f64, bottom: f64, top_color: Rgb, bottom_color: Rgb },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderMode {
    Full,
    Medium,
    Low,
    Lowest,
    Auto,
}

#[derive(Clone, Debug)]
pub struct Ao {
    pub samples: u32,
    pub maxdist: f64,
    pub color: Rgb,
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub hres: u32,
    pub vres: u32,
    pub shader: ShaderMode,
    pub trans_vmd: bool,
    pub trans_raster3d: bool,
    pub fog_vmd: bool,
    pub shadow_filter: bool,
    pub light_scale: f64,
    pub ao: Option<Ao>,
    pub camera: Camera,
    pub lights: Vec<Light>,
    pub background: Background,
    pub fog: Fog,
    pub materials: Vec<Material>,
    pub clips: Vec<Vec<[f32; 4]>>,
    /// group names (VMD: "Mol[0] Rep[2] NewCartoon"); `Object::group` indexes them
    pub groups: Vec<String>,
    pub objects: Vec<Object>,
}

impl Scene {
    /// Tachyon's defaults, nothing in it (every reader starts here)
    pub fn empty() -> Scene {
        Scene {
            hres: 512,
            vres: 512,
            shader: ShaderMode::Auto,
            trans_vmd: false,
            trans_raster3d: false,
            fog_vmd: false,
            shadow_filter: true,
            light_scale: 1.0,
            ao: None,
            camera: Camera {
                projection: Projection::Perspective,
                zoom: 1.0,
                aspect: 1.0,
                antialiasing: 0,
                raydepth: 6,
                center: [0.0, 0.0, 0.0],
                viewdir: [0.0, 0.0, 1.0],
                updir: [0.0, 1.0, 0.0],
                focaldist: 1.0,
                aperture: 2.8,
                frustum: None,
            },
            lights: vec![],
            background: Background::Solid(Rgb { r: 0.0, g: 0.0, b: 0.0 }),
            fog: Fog { kind: FogKind::None, start: 0.0, end: 1.0, density: 1.0, color: Rgb { r: 0.0, g: 0.0, b: 0.0 } },
            materials: vec![],
            clips: vec![],
            groups: vec![],
            objects: vec![],
            }
    }
}
