# The avocado test model (not included)

The Khronos glTF sample "Avocado" (CC0 1.0, © 2017 Public) is one of Skiagraph's test scenes, but it is not shipped:
it is 8 MB, nearly all of it painted textures that Skiagraph does not read (its geometry is 24 kB). Get it with

    curl -LO https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/Avocado/glTF-Binary/Avocado.glb

and draw it with `skiagraph Avocado.glb avocado.svg`. It is a tiny model (6 cm across, in metres) whose colour lives
only in a texture: it tests that drawing does not depend on a model's size and that a texture-only colour gets a
neutral grey. The picture in the README gallery is drawn from it.
