# Third-party notices

## Tachyon

`src/tachyon.rs` is translated from the Tachyon parallel / multiprocessor ray tracer, version 0.99.5
(`src/shade.c`, `camera.c`, `trace.c`, `util.c`, `sphere.c`, `cylinder.c`, `triangle.c`, `ring.c`, `plane.c`,
`intersect.c`, `light.c`, `texture.c`), distributed under the following licence:

```
Copyright (c) 1994-2021 John E. Stone
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.
3. The name of the author may not be used to endorse or promote products
   derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR ``AS IS'' AND ANY EXPRESS
OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.
```

## Example models

`examples/meshes/knot.obj`, `knot.mtl` and `still_life.glb` were generated for this project by
`scripts/make_meshes.py` (no outside data) and are dedicated to the public domain under CC0 1.0
(<https://creativecommons.org/publicdomain/zero/1.0/>).

`examples/meshes/fox.glb` is the "Fox" sample from the Khronos glTF Sample Assets
(<https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/Fox>), unchanged: model © 2014 PixelMannen
(CC0 1.0); rigging and animation © 2014 tomkranis (CC BY 4.0, <https://creativecommons.org/licenses/by/4.0/>);
conversion to glTF © 2017 AsoboStudio and scurest (CC BY 4.0). Its licence and description are in
`examples/meshes/fox_LICENSE.md` and `fox_README.md`. Skiagraph draws its rest pose without the animation.

The avocado in the README gallery and the style book is drawn from the "Avocado" sample of the same collection
(<https://github.com/KhronosGroup/glTF-Sample-Assets/tree/main/Models/Avocado>), © 2017 Public, CC0 1.0; it is not
shipped (`examples/meshes/AVOCADO.md`).

## Molecular structures in the pictures

The molecules in `docs/gallery/` and the style book are drawn from public entries of the RCSB Protein Data Bank:
1HSG (HIV-1 protease with indinavir) and 1UBQ (ubiquitin).
