<p align="center"><img src="assets/logo/banner.svg" width="830" alt="Skiagraph banner: in a firelit cave painted like a Greek black-figure vase, a man holds up the black silhouette of a protein (ubiquitin) on a staff, and on the cave wall its image appears as Skiagraph's own flat SVG drawing. Below: SKIAGRAPH, an SVG renderer for VMD."></p>

<p align="center">
<img alt="version 1.0.0-rc1" src="https://img.shields.io/badge/version-1.0.0--rc1-e8b600">
<img alt="MIT licence" src="https://img.shields.io/badge/licence-MIT-1c1c1c">
<img alt="Rust" src="https://img.shields.io/badge/made%20with-Rust-b7410e">
<img alt="pure vector SVG" src="https://img.shields.io/badge/output-pure%20vector%20SVG-3a9fb0">
<img alt="VMD plugin" src="https://img.shields.io/badge/VMD-render%20SVG-7a3f9d">
</p>

# Skiagraph

Skiagraph turns a 3D scene into a vector drawing in a comic-book style that still reads as 3D: ink outlines,
two or three hard tones of shading, drawn shadows and highlights. The default style is **Comic Ink**. The result is
a plain SVG file: sharp at any zoom, small enough for a paper or a slide, and editable in Inkscape or Illustrator.

It reads molecular scenes from [VMD](https://www.ks.uiuc.edu/Research/vmd/) (type `render SVG figure.svg` inside
VMD) and ordinary 3D models (OBJ, glTF). Version 1.0.0-rc1.

<p align="center"><img src="docs/gallery/hsg_cartoon.png" width="760" alt="HIV-1 protease (RCSB 1HSG) as a cartoon with the drug indinavir as sticks, drawn in Comic Ink: yellow strands shaded towards golden orange, purple helices, black outlines, hatching on the shadow sides."><br>
<sub>HIV-1 protease with indinavir (RCSB 1HSG), from VMD. The SVG: <a href="docs/gallery/hsg_cartoon.svg">hsg_cartoon.svg</a> (1.1 MB).</sub></p>

## VMD's render and Skiagraph's, side by side

The same VMD scene (HIV-1 protease with indinavir, RCSB 1HSG), drawn by VMD's own ray tracer and by Skiagraph:

<p align="center"><img src="docs/gallery/vmd_vs_skiagraph.png" width="900" alt="Left: VMD's Tachyon render of HIV-1 protease, photo-real with ambient occlusion. Right: the same scene in Skiagraph's Comic Ink, with black outlines, golden shading and hatching."><br>
<sub><b>left</b> VMD 1.9.4 TachyonInternal, shadows and ambient occlusion · <b>right</b> Skiagraph, Comic Ink, no options</sub></p>

| same scene, same computer | VMD (Tachyon, shadows + AO) | Skiagraph (Comic Ink SVG) |
|---|---|---|
| screen size, 1800 × 1500 px | 15.9 s · 0.40 MB PNG | **4.3 s** · 1.02 MB SVG |
| poster, 7200 × 6000 px (300 dpi at 60 cm) | 271 s · 3.4 MB PNG (130 MB raw) | **the same file:** 4.3 s · 1.02 MB |
| zoom in 4× | pixels | still sharp |
| edit colours or lines afterwards | re-render | open it in Inkscape or Illustrator: named layers for fills, tones, shadows, hatching and lines |

A pixel picture has to be rendered again, bigger and slower, for every print size; Skiagraph's drawing has no
pixels, so one file serves a slide, a paper and a poster. At poster size it is about **60 times faster and a third
of the size**. (At screen size a PNG is smaller: the SVG's edge is that it never needs a second render.)

<p align="center"><img src="docs/gallery/zoom_4x.png" width="900" alt="Zoomed 4 times: on the left VMD's screen render turns into visible pixels; on the right Skiagraph's SVG stays crisp."><br>
<sub>the drug pocket at 4× zoom: VMD's screen render (left) against the same Skiagraph file (right)</sub></p>

## The design document

Comic Ink was designed like a studio art department works: directions explored, one chosen, then rules for line,
tone, shadow, hatching, colour and depth, refined stage by stage with every defect recorded.
**[Read the Comic Ink style book (PDF, 33 pages)](docs/style-book/comic-ink-style-book.pdf).**

<p align="center"><a href="docs/style-book/comic-ink-style-book.pdf"><img src="docs/gallery/stylebook_pages.png" width="900" alt="Five pages of the Comic Ink style book: the cover, three style directions, the iteration history, the shading palette and the material chart."></a></p>

## Gallery

| | | |
|---|---|---|
| <img src="docs/gallery/surface.png" alt="a molecular surface"><br><sub>surface (QuickSurf, 1UBQ)</sub> | <img src="docs/gallery/transparent_surface.png" alt="a transparent surface over a ligand"><br><sub>transparent surface over a ligand (1HSG)</sub> | <img src="docs/gallery/ball_and_stick.png" alt="ball and stick"><br><sub>ball-and-stick (indinavir, 1HSG)</sub> |
| <img src="docs/gallery/large_crowd.png" alt="a crowd of 96 proteins"><br><sub>a large scene: 96 ubiquitins, cartoon and spheres</sub> | <img src="docs/gallery/materials.png" alt="material chart"><br><sub>materials: matte, plastic, metal, glass, glowing</sub> | <img src="docs/gallery/noir.png" alt="noir preset"><br><sub><code>--preset noir</code>: solid black in the deepest shadow</sub> |
| <img src="docs/gallery/knot.png" alt="a red trefoil knot on a stone plinth"><br><sub>beyond molecules: an OBJ model (<a href="docs/gallery/knot.svg">SVG</a>)</sub> | <img src="docs/gallery/still_life.png" alt="an apple, a brass ring and a glass goblet on a table"><br><sub>a glTF model: glass, brass, plastic, wood (<a href="docs/gallery/still_life.svg">SVG</a>)</sub> | <img src="docs/gallery/lighting.png" alt="four lighting presets"><br><sub>lighting presets: studio, rim, low, night</sub> |
| <img src="docs/gallery/fox.png" alt="a low-poly fox standing, grey, hatched on its shadow side"><br><sub>a third-party glTF model: the Khronos Fox (<a href="docs/gallery/fox.svg">SVG</a>)</sub> | <img src="docs/gallery/avocado.png" alt="half an avocado seen from the cut side, the stone raised in the middle"><br><sub>the Khronos Avocado: 6 cm across, colour only in a texture (drawn grey)</sub> | |

The molecules are public structures from the RCSB Protein Data Bank (1HSG, 1UBQ). The knot and the still life in
`examples/meshes/` were made for this project by `scripts/make_meshes.py` and are free to use (CC0). The fox and the
avocado are Khronos glTF sample models: Fox by PixelMannen (CC0), rigged by tomkranis (CC BY 4.0), converted by
AsoboStudio and scurest (CC BY 4.0); Avocado (CC0, not shipped: `examples/meshes/AVOCADO.md`). See
`THIRD-PARTY-NOTICES.md`.

## Install

You need Rust (a recent stable version, from <https://rustup.rs>). Then, in this folder:

```
cargo install --path .
```

This puts the `skiagraph` program in `~/.cargo/bin` (on the PATH after installing Rust). Check with
`skiagraph --help`.

**What was tested.** Linux: built and run on Ubuntu under WSL2 (Linux 6.18) with Rust 1.99, with VMD 1.9.4a57 for
Linux. **Windows:** the same WSL2 set-up is how it runs on Windows today (install WSL, then follow the Linux steps
inside it). A native Windows build (`cargo install --path .` in a Windows terminal) and the VMD plugin with the
Windows version of VMD were **not tested**; the code has nothing Linux-specific, so they should work, but this is
unchecked. macOS: not tested.

### The VMD plugin

Add one line to your `~/.vmdrc` (create the file if needed):

```
source /path/to/skiagraph/vmd/skiagraph.tcl
```

Then, in VMD's console:

```
render SVG figure.svg                        ;# Comic Ink
render SVG figure.svg night                  ;# a preset
render SVG figure.svg "shadows=off,fog=0.2"  ;# a recipe (see below)
```

VMD writes its scene to a temporary Tachyon file next to the SVG; Skiagraph draws it and deletes the file. The plugin
finds the program as `$SKIAGRAPH_BIN`, else `skiagraph` on the PATH, else `~/.cargo/bin/skiagraph`. Every other
`render` command goes to VMD unchanged.

## Use without VMD

```
skiagraph SCENE OUT.svg [--preset NAME] [--style key=value,...]
```

`SCENE` is a Tachyon file written by VMD (`.dat`, from `render Tachyon`), a Wavefront model (`.obj` with its `.mtl`)
or a glTF 2.0 model (`.gltf` or `.glb`).

```
skiagraph examples/meshes/knot.obj knot.svg
skiagraph examples/meshes/still_life.glb still_life.svg --preset rim
skiagraph protein.dat protein.svg --style shadows=off,mat=Licorice:metal
```

### Presets

| preset | what it does |
|---|---|
| `comic-ink` | the default: black outlines, shading in deeper colours, hatching, cast shadows, light from the upper left |
| `rim` | light from behind: the shapes dark, a white lit edge where they turn towards the light |
| `low` | light from below, in front |
| `night` | a cool light over a dark blue background; far shapes fade into it |
| `scene` | the scene's own first light (VMD's light 0) instead of Comic Ink's |
| `noir` | Comic Ink with solid black in the deepest shadows |

### The recipe: `--style key=value,...`

Fine-tuning on top of the preset, separated by commas:

| key | values | meaning |
|---|---|---|
| `shadows` | `on` (default), `off`, `scene` | cast shadows; `scene` follows VMD's shadow switch |
| `light` | `studio`, `rim`, `low`, `night`, `scene` | the lighting, as the presets |
| `mat` | `matte`, `plastic`, `metal`, `glass`, `emissive` | one material for all; or per group: `mat=Rep[1]:glass;Licorice:metal` (the first part of a group's name that matches wins; VMD's groups are named like `Mol[0] Rep[1] NewCartoon`, a model's after its objects) |
| `fog` | 0 to 1 (0.4; 0 for models) | how far the deepest shapes fade towards `fogto` |
| `fogto`, `bg` | `#rrggbb` | the colour far shapes fade to; the background |
| `fogsat` | 0 to 1 | how much colour the fog takes away |
| `fill` | `vmd`, `pale:0.3` | the scene's colours, or paler ones |
| `lines` | `black`, `none`, `#rrggbb` | the outline colour |
| `spot` | `0`, `1` | solid black in the deepest shadows |
| `detail` | `0`, `1` | lines also at small steps in depth |
| `dec` | a number (2) | decimals in the file: fewer make it smaller |

`skiagraph --help` prints all of this.

### Models (OBJ, glTF)

Triangles with their normals are read; each material is mapped onto Comic Ink's five: glass where it is
see-through, metal where glTF says metallic (OBJ: `Pm`, or a mirror `illum` with strong `Ks`), matte where it is
rough (OBJ: no specular), plastic otherwise. The file's camera is used if it has one (glTF); otherwise the model is
framed from the front, a little from the right and above, and Comic Ink's key light goes to the side its seen
faces lean towards, so the side you see is lit. Textures, animation, skins, points and lines are not
drawn: Skiagraph says so on the terminal and draws the rest with the material's plain colour (its base colour
factor; a colour that lives only in a texture becomes a light neutral grey, and metalness or roughness only in a
texture a plain non-metal). A model of any size is drawn the same: it is scaled to a fixed size before drawing.

## How it is drawn

Nothing is rendered to pixels and traced back. Every mark is computed from the 3D shapes:

- **What shows**: an exact visibility calculation for spheres, tubes and triangles (with a bit-exact copy of the
  Tachyon ray tracer as its check) gives each shape the outline of its visible part.
- **Lines**: one continuous filled shape along each edge, thicker for outer silhouettes, thinner for inner edges and
  creases, thinner with depth.
- **Shading**: two hard tone bands where the light on the 3D surface falls below fixed levels (on a ball, a circle;
  on a tube, two lines; in a triangle, a refined curve). Each band is a deeper, slightly hue-shifted version of the
  colour under it, as a comic colourist shades (yellow to golden orange, cyan to deep teal, grey to cool slate):
  black is kept for lines and spot blacks.
- **Shadows**: each shape's outline as the light sees it, cast onto the surfaces behind.
- **Hatching**: strokes that curve around the form, only in the shadow band.
- **Glass**: one translucent layer per depth, with the lines behind it drawn under it.

The file uses no filters, so it looks the same in browsers, Inkscape and resvg (checked on every release picture).

## License

Skiagraph is released under the MIT licence (`LICENSE`). `src/tachyon.rs` is translated from the Tachyon ray tracer
by John E. Stone, under its BSD 3-clause licence: see `THIRD-PARTY-NOTICES.md`. Skiagraph contains no VMD code; VMD
is developed by the Theoretical and Computational Biophysics Group, University of Illinois at Urbana-Champaign.
