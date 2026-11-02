//! Skiagraph: a 3D scene -> a stylised vector SVG (ADR 0012). Readers fill the internal `scene::Scene`; the exact
//! visibility engine (`geom`, `mesh`, `persp`, `cyl`) finds what shows; the style (`flat`) draws it.

mod cyl;
mod flat;
mod geom;
mod ink;
mod line;
mod mesh;
mod persp;
mod read_mesh;
mod read_tachyon;
// the internal format carries all a reader finds, also what no drawing step reads yet (light colours, point lights,
// gradient backgrounds, AO settings: stage 3)
#[allow(dead_code)]
mod scene;
mod svg;
mod tachyon;

use std::process::ExitCode;

const HELP: &str = "\
skiagraph 1.0.0-rc1: a 3D scene drawn as a comic-style vector picture (SVG)

usage:  skiagraph SCENE OUT.svg [--preset NAME] [--style key=value,...]

SCENE   a Tachyon scene written by VMD (.dat: `render Tachyon`, or `render SVG` with the plugin),
        a Wavefront mesh (.obj, with its .mtl) or a glTF 2.0 model (.gltf, .glb)

--preset NAME   comic-ink  the default: black outlines, coloured shading, hatching, cast shadows
                rim        light from behind: dark forms with a lit white edge
                low        light from below, in front
                night      cool light over a dark blue ground
                scene      the scene's own first light (VMD's light 0)
                noir       comic-ink with solid black in the deepest shadows

--style key=value,...   fine-tuning on top of the preset, for example
                --style shadows=off,fog=0.2,mat=Rep[1]:glass
        lines=black|none|#hex  outline colour       fill=vmd|pale:0.3   paler colours
        fog=0..1               depth fade (0.4; 0 for meshes)           fogto=#hex   what far shapes fade to
        fogsat=0..1            colour fog removes   bg=#hex             background
        light=studio|rim|low|night|scene            shadows=on|off|scene (scene: VMD's switch)
        spot=0|1               spot blacks          detail=0|1          lines at small depth steps
        mat=NAME               matte, plastic, metal, glass or emissive, for everything; or per group:
                               mat=Rep[1]:glass;Licorice:metal (the first part of a group's name that matches)
        dec=N                  decimals in the file (2)

Everything is drawn directly from the 3D geometry: no pixels are traced.";

/// Presets: recipes on top of comic-ink (stage 5).
const PRESETS: [(&str, &str); 7] = [
    ("comic-ink", ""),
    ("studio", ""),
    ("rim", "light=rim"),
    ("low", "light=low"),
    ("night", "light=night"),
    ("scene", "light=scene"),
    ("noir", "spot=1"),
];

fn main() -> ExitCode {
    // `skiagraph SCENE OUT [--preset NAME] [--style RECIPE]`; a leading `svg` (the old form) is accepted
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "svg") {
        args.remove(0);
    }
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return if args.is_empty() { ExitCode::FAILURE } else { ExitCode::SUCCESS };
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("skiagraph {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let (mut files, mut preset, mut style) = (vec![], "comic-ink".to_string(), String::new());
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--preset" | "--style" => {
                let Some(v) = it.next() else {
                    eprintln!("skiagraph: {a} needs a value (see --help)");
                    return ExitCode::from(2);
                };
                if a == "--preset" { preset = v } else { style = v }
            }
            _ if a.starts_with("--") => {
                eprintln!("skiagraph: unknown option {a} (see --help)");
                return ExitCode::from(2);
            }
            _ => files.push(a),
        }
    }
    let [scene_file, out] = &files[..] else {
        eprintln!("skiagraph: give one scene file and one output file (see --help)");
        return ExitCode::from(2);
    };
    // `--style NAME` with a preset's name (the old form) still works
    if PRESETS.iter().any(|(n, _)| *n == style) {
        preset = std::mem::take(&mut style);
    }
    let Some(&(_, base)) = PRESETS.iter().find(|(n, _)| *n == preset) else {
        eprintln!("skiagraph: unknown preset {preset:?} (presets: {})", PRESETS.map(|p| p.0).join(", "));
        return ExitCode::from(2);
    };
    // a mesh file gets no depth fog unless asked: the fog is stepped per shape, and a mesh's large flat faces would show
    // the steps as seams (a molecule's small triangles hide them)
    let mesh = [".obj", ".gltf", ".glb"].iter().any(|e| scene_file.to_ascii_lowercase().ends_with(e));
    // the user's recipe checked alone first, so an error quotes only what they wrote
    let spec = match flat::Spec::named(&style).and_then(|_| flat::Spec::named(&format!("{}{base},{style}", if mesh { "fog=0," } else { "" }))) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("skiagraph: {e}");
            return ExitCode::from(2);
        }
    };
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let path = std::path::Path::new(scene_file);
        let mut sc = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("obj" | "gltf" | "glb") => read_mesh::read(path)?,
            _ => read_tachyon::parse(&std::fs::read_to_string(path).map_err(|e| format!("{scene_file}: {e}"))?)?,
        };
        flat::apply_materials(&mut sc, &spec);
        match flat::convert(&tachyon::Tracer::new(&sc), &spec) {
            Ok(s) => Ok(std::fs::write(out, s)?),
            Err(svg::Unsupported(what)) => Err(format!("unsupported: {what}").into()),
        }
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("skiagraph: {e}");
            ExitCode::from(2)
        }
    }
}
