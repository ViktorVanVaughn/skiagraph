# 14. One internal scene format; readers per input; no molecule logic in the drawing

Date: 2026-11-01 — Status: accepted (stage 1).

## Decision

- **The internal scene** (`src/scene.rs`) is what every reader produces and every drawing step reads: a camera
  (orthographic or perspective), lights, materials, primitives (spheres, open tubes, flat and smooth triangles
  with optional vertex colours, rings, planes), clip planes, and named **groups**. Renderer hints a source may
  carry (Tachyon's shader mode, fog style, transparency mode, ambient occlusion) are optional fields with the
  source's defaults.
- **Readers** turn one format each into it: `src/read_tachyon.rs` (VMD's `render Tachyon` files, following
  Tachyon 0.99.5's parser, BSD notice kept); OBJ and glTF readers come in stage 4 and add no field the drawing
  must special-case.
- **No molecule logic in the drawing.** Group names are free text that the drawing never reads (stage 1
  removed the last name-based switch, `MOL25D_FLAT_SKIP`). What the style needs to know about structure, it
  finds from the geometry: **parts** (shapes that overlap deeply or join) stand for molecules or mesh objects.
  Anything specific to molecules (VMD's representation names, element colours) lives in readers and presets.
- **The tracer** (`src/tachyon.rs`, Tachyon's bounding-volume tree and ray tests, BSD notice kept) is the
  ray-casting engine over the internal scene: eye rays and first-opaque hits today; the shadow-ray branch of its
  intersection test is kept, unused, for drawn shadows (stage 2).

## Consequences

- Renaming in stage 1: `Texture` → `Material`, `Scene::reps` → `Scene::groups`, `Object::rep` →
  `Object::group`. The Tachyon keyword stays `Texture` in the reader.
- A new input format is one new reader file and its tests; nothing downstream changes.
