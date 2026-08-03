# Real FBX test fixtures

Until these landed, the FBX importer was exercised **only** against a hand-written ASCII FBX string embedded
in its own test. That proved the parser could read a synthetic 8-vertex cube and nothing else: no binary FBX,
no compressed arrays, no real material graph, no rig, no DCC-exported pivot conventions. Those are exactly
where real `.fbx` files break importers.

## Source and licence

All files are from the **assimp** project's test-model suite, `test/models/FBX`:
<https://github.com/assimp/assimp/tree/master/test/models/FBX>

assimp is **BSD-3-Clause** and `test/models/` is its deliberately licence-clean model directory. Its sibling
`test/models-nonbsd/` is **not** clean and nothing here comes from it.

Downloaded 2026-08-03. Keep this file beside the fixtures — a binary blob with no recorded provenance is a
licence problem waiting to happen, and the repo already treats asset provenance as a first-class concern
(ADR-044).

## What each file is for

| File | Bytes | Encoding | Why it earns its place |
|---|---:|---|---|
| `box.fbx` | 17 200 | binary | The baseline binary-FBX path, which the ASCII-only test never touched. |
| `phong_cube.fbx` | 17 084 | binary | A classic Phong material graph — the legacy material path. |
| `spider.fbx` | 123 292 | binary | A real multi-mesh textured model, not a primitive. |
| `boxWithCompressedCTypeArray.FBX` | 13 824 | binary | Deflate-compressed array encoding — a real decoder branch, and an uppercase extension that catches case-sensitive routing bugs. |
| `cubes_with_mirroring_and_pivot.fbx` | 88 496 | ASCII | Mirrored transforms and pivot offsets: the classic source of flipped winding and misplaced geometry on DCC import. |
| `maxPbrMaterial_metalRough.fbx` | 20 439 | ASCII | A metallic-roughness PBR material graph from 3ds Max, matching the engine's own BRDF (ADR-041). |
| `animation_with_skeleton.fbx` | 195 420 | binary | A rigged, skinned mesh. ADR-040 states a rigged mesh imports **as geometry**; this is the fixture that holds that claim honest. |

Five binary, two ASCII, ~476 KB total. The mix is the point: FBX is not one format.

## Not committed

`Giftbox.fbx` from a local Unreal project was used as an additional real-world spot check during development.
It is **not** included here because its licence is unknown and this repository is pushed publicly.
