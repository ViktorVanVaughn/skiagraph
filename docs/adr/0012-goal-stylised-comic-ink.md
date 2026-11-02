# 12. Goal: a stylised vector illustration (Comic Ink), not a match of VMD's image

Date: 2026-11-01 — Status: accepted (owner, stage 0 review). Supersedes ADRs 0003, 0009, 0010 and 0011.

## Decision

- **Product**: Skiagraph turns a 3D scene (VMD's Tachyon export now; OBJ and glTF from stage 4) into a
  stylised 2D vector SVG that still reads as 3D. The default style is **Comic Ink**: direction A of stage 0,
  calmed for far shapes (thinner, lighter lines, never faded away), hatching only in the darkest band and
  across the form. Its rules and numbers live in the style book (`docs/style-book/`).
- **Kept from the old goal**: the exact visibility engine (`geom`, `mesh`, `persp`, `cyl`), the scene parsing,
  the three judges, the test matrix and stress tools, and pure vector output (no embedded raster; no SVG
  filters in the core presets unless justified and passing the judges).
- **Dropped**: Tachyon's lighting function as the target, the per-pixel tolerance against VMD's picture, the
  gradient fitting that served it, the floors, and the speed goal counted in VMD's render time. Their code
  was deleted in stage 1 (`shade`, `cylshade`, `region`, `knobs`, `size`, `simplify`, `png`, the old renderer
  in `svg` and `mesh`, the Tachyon shading in `tachyon`, the scoring scripts); it stays in git history (commit
  96d1006 and the old branches).
- **Targets** (FINAL_PLAN §6): a picture of up to 4 megapixels in at most 1.5 MB; a ~300k-atom scene in at most
  60 s and 4 GB on the owner's laptop; readable at 25 % and at 400 %.

## Consequences

- `skiagraph svg scene.dat out.svg` draws Comic Ink; `--style NAME|recipe` picks another style. The
  VMD-matching output no longer exists.
- ADR 0006's "fitted gradients" part and ADR 0007's purpose of the test matrix (comparison with VMD) are
  obsolete in practice; their outline and matrix parts stand. Superseding them formally is the owner's call.
