# Vendored `moxcms`

This directory contains H-Gripe Studio's vendored fork of `moxcms`, the
pure-Rust colour-management engine used by the ProPhoto / CMYK / ICC pipeline.

| | |
| --- | --- |
| Upstream | https://github.com/awxkee/moxcms |
| Crates.io version | 0.8.1 |
| License | BSD-3-Clause OR Apache-2.0 |
| Local resolver | Workspace `[patch.crates-io]` in `Cargo.toml` |

## Why vendored

The colour pipeline is a production contract, not a best-effort helper:

- ProPhoto RGB generation and embedding must stay byte-stable enough for
  `working_image::is_prophoto_icc`.
- CMYK ICC transforms must keep the validated interpolation / weight behaviour.
- The 16-bit ProPhoto path currently relies on a local policy decision: moxcms
  `High` barycentric weights are avoided for that path because they collapse the
  LUT to white in the validated cases.

Relying directly on upstream releases would let this behaviour drift whenever
the cloud updates dependencies. H-Gripe now owns this copy and updates it
deliberately.

## Local modifications

None yet. This is an unmodified copy of `moxcms` 0.8.1 from the local Cargo
registry cache, vendored so future colour-management changes can be made here
first.

When modifying the fork, record the change here with the files touched and the
reason. Keep changes narrow and covered by the colour golden tests.

## Upgrade procedure

1. Replace this directory with the target upstream release.
2. Re-apply the local modifications listed above.
3. Keep the workspace `[patch.crates-io]` pointing at `third_party/moxcms`.
4. Run:

   ```powershell
   cargo test -p hgripe-desktop studio::color
   .venv\Scripts\python.exe -m pytest python/bridge/tests/test_wide_gamut.py python/bridge/tests/test_linear_light.py
   ```

5. If any golden changes intentionally, update `docs/design/colour-pipeline.md`
   in the same commit.

