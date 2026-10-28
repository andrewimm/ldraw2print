# ldraw2print

Convert LEGO® LDraw part files into 3D-printable models, with a focus on
**scaled-up minifigure accessories**: hats, helmets, hair, scarves, neckwear
and handheld items that must actually attach to a printed minifigure of the
same scale. Any scale works (`--scale`); the default is 5×.

- Pure Rust CLI. No Blender, CAD tools or Node.js needed.
- Binary STL export in millimetres, Z up, usable in any slicer. Bambu Studio was used to confirm the files import at the intended size.
- A browser preview (Three.js, served locally, works offline) that shares the export pipeline.
- Connector profiles for the head stud, neck post, scarf collar and hand grip.
  Fit changes move only the mating surface, never the outside of the part.
- Calibration coupons for test-fitting a connector before printing the full part.
- Mesh diagnostics that report what is wrong instead of claiming a mesh is printable.
- Multicolour 3MF export: LDraw colours painted onto one solid (verified in Bambu Studio on an H2S).

> LEGO® is a trademark of the LEGO Group, which does not sponsor or endorse this project.
> LDraw™ is a trademark of the Estate of James Jessiman.

**Using this from a script or an AI agent?** Read [`AGENTS.md`](AGENTS.md)
(also `ldraw2print guide agents`). In short: pass `--json` for one JSON
object per run, branch on the [exit code](#exit-codes), find parts with
`ldraw2print search`, and never run `preview` unattended.

**Built-in help:** `ldraw2print --help` gives the quick start,
`ldraw2print <command> --help` gives options and examples, and
`ldraw2print guide` lists topics (workflow, connectors, fit, smoothing,
multicolor, repair, orientation, slicers, units, json, agents). The guide
text lives in [`docs/guide/`](docs/guide/).

---

## Contents

1. [Install](#install)
2. [LDraw library setup](#ldraw-library-setup)
3. [Quick start](#quick-start)
4. [Units and coordinate system](#units-and-coordinate-system)
5. [Commands](#commands)
6. [Minifigure compatibility](#minifigure-compatibility)
7. [Calibration workflow](#calibration-workflow)
8. [Mesh validation and repair](#mesh-validation-and-repair)
9. [Bambu Studio workflow](#bambu-studio-workflow)
10. [Other slicers (PrusaSlicer, OrcaSlicer, Cura)](#other-slicers-prusaslicer-orcaslicer-cura)
11. [Worked example: police hat 3624](#worked-example-police-hat-3624)
12. [Architecture](#architecture)
13. [Testing](#testing)
14. [Limitations](#limitations)
15. [Licensing](#licensing)

---

## Install

Prebuilt binaries for Linux (x86_64, aarch64) and macOS (Apple Silicon and
Intel in one binary) are on the
[releases page](https://github.com/andrewimm/ldraw2print/releases). Download
the archive for your system, unpack it and put `ldraw2print` on your PATH:

```bash
tar -xzf ldraw2print-*-macos.tar.gz          # or …-linux-x86_64 / …-linux-aarch64
mv ldraw2print-*/ldraw2print ~/.local/bin/   # any directory on your PATH
ldraw2print --version
```

The macOS binary is not notarised by Apple. If you downloaded it with a
browser and macOS refuses to open it, clear the quarantine flag:
`xattr -d com.apple.quarantine ~/.local/bin/ldraw2print`.

Each archive has signed build provenance; check one with
`gh attestation verify <archive> --repo andrewimm/ldraw2print`.

### From source

Requires Rust 1.88 or newer (rustup picks up the pinned version from `rust-toolchain.toml`).

```bash
cargo install --git https://github.com/andrewimm/ldraw2print --locked
# or, from a clone:
cargo install --path . --locked   # installs `ldraw2print` into ~/.cargo/bin
cargo build --release             # binary at target/release/ldraw2print
```

The preview's Three.js (r160, MIT, `assets/vendor/`) is compiled into the
binary, so the preview works offline.

## LDraw library setup

You need the official LDraw parts library. It is free and licensed
CC BY 2.0 / CC BY 4.0.

1. Download **https://library.ldraw.org/library/updates/complete.zip** (~150 MB).
2. Unzip it. This creates a folder named `ldraw` containing `parts/`, `p/` and `LDConfig.ldr`.
   A common place for it is `~/ldraw`.
3. Tell ldraw2print where it is. The first of these that is set wins:
   - `--ldraw-dir ~/ldraw` on any command
   - the `LDRAWDIR` environment variable, which other LDraw tools also use
   - `ldraw_dir = "/Users/you/ldraw"` in the config file
   - auto-detection of `~/ldraw`, `~/Documents/LDraw`, `/usr/share/ldraw`, `C:\LDraw` and similar locations

`~` is expanded in all three, and a doubly nested `ldraw/ldraw` folder (a
common unzip result) is accepted. Run `ldraw2print library` to see which
library is in use and how it was found, or why a configured path was rejected.
Unofficial parts go in `<ldraw>/UnOfficial/parts` and `<ldraw>/UnOfficial/p`.

Lookup follows LDraw conventions. A reference is searched for in:

1. the input file's directory and any MPD sub-files
2. `parts/` (this also covers `parts/s/`, because references are written as `s\xxxx.dat`)
3. `p/`, which also covers `p/48/` and `p/8/` (written as `48\…` and `8\…`)
4. `models/`
5. the unofficial folders

Matching is case-insensitive and accepts `\` as a path separator.

## Quick start

```bash
# Plain conversion at 5:1 (the default scale)
ldraw2print convert 3901.dat --scale 5 --output hair.stl

# Interactive preview in the browser
ldraw2print preview 3901.dat --scale 5

# Dimensions, diagnostics and connector candidates
ldraw2print inspect 3901.dat --scale 5 --connector head-stud

# Hat with a fit adjustment on the head-stud socket only
ldraw2print convert 3624.dat \
  --scale 5 \
  --connector head-stud \
  --fit clearance \
  --output hat.stl

# Test-fit coupons for the head stud (prints in a few minutes)
ldraw2print calibrate --connector head-stud --scale 5 -o head-stud-coupons.stl
```

The input can be a part number (`3901`), a file name (`3901.dat`), a
library-relative name (`s/3901s01.dat`), or the path to any `.dat`, `.ldr` or
`.mpd` file.

## Units and coordinate system

| | LDraw | Output (STL, 3MF, preview) |
|---|---|---|
| Unit | LDU (1 LDU = 0.4 mm at 1:1) | millimetre |
| Up | **−Y** | **+Z** |
| Handedness | right-handed | right-handed |

At scale *s*, 1 LDU = 0.4 × *s* mm, so the default 5:1 gives **2 mm per LDU**.
A stud pitch of 20 LDU becomes 40 mm.

Points convert as:

```
print.x =  ldraw.x × mm_per_ldu
print.y =  ldraw.z × mm_per_ldu
print.z = −ldraw.y × mm_per_ldu
```

This is a proper rotation (−90° about X, determinant +1), so triangle winding
and outward normals survive the conversion. It also keeps a part upright: a
hat's top is at +Z, and its socket opens downwards.

`--placement bed` (the default) moves the lowest point to Z = 0 and centres
X and Y. `--placement ldraw` keeps the LDraw origin, which helps when you
export several parts that must line up.

## Commands

Global options: `--ldraw-dir DIR`, `--config FILE`, `--primitives standard|high|low`,
`--json` (one JSON object on stdout, errors included; field list in
`ldraw2print guide json`).

| Command | Purpose |
|---|---|
| `convert <part>` | Write STL or painted 3MF |
| `inspect <part>` | Size, diagnostics, colours, connector candidates (writes nothing) |
| `calibrate --connector P` | Test-fit coupons |
| `preview <part>` | Interactive browser preview (a server; not for scripts) |
| `search <words…>` | Find parts by number, name, category or LDraw keywords (themes, BrickLink ids, set numbers) |
| `colors [name\|code]` | LDraw colour codes for `--color` |
| `profiles` | Connector profiles with nominal sizes and your calibrated fits |
| `guide [topic]` | Built-in documentation |
| `config init\|show\|path`, `library` | Configuration and LDraw library setup |

### Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | error (bad input, part not found, I/O) |
| 2 | command-line usage error |
| 3 | `--strict` and the mesh isn't watertight (nothing written) |
| 4 | connector needs a decision: none or several surfaces match; rerun with `--connector-candidate ID` |
| 5 | referenced LDraw files are missing (use `--allow-missing` to continue) |

With `--json`, failures print `{"ok": false, "error", "kind", "exit_code"}`.

### `convert <part>`

| Option | Meaning |
|---|---|
| `-o, --output FILE` | Output path. The default is `<part>.stl`; a `.3mf` extension selects 3MF. |
| `--format stl\|3mf` | Force an output format. |
| `--color-mode paint\|parts` | How 3MF stores colour (default `paint`). See [Multicolour](#multicolour). |
| `--scale N` | Scale factor (default 5). |
| `--color CODE` | LDraw colour used for "main colour" 16 (default 71, Light Bluish Grey). |
| `--connector NAME` | Connector profile to detect: `head-stud`, `neck`, `scarf`, `hand-grip`, or your own. |
| `--fit loose\|normal\|tight\|clearance\|none` | Apply a fit preset to that connector. `clearance` uses the profile's `default_fit`. With no `--fit`, the geometry is left as modelled. |
| `--clearance-mm X` | Explicit diametral clearance. Overrides the preset. |
| `--connector-candidate ID` | Use this detected surface when auto-selection is ambiguous. |
| `--connector-axis x,y,z --connector-radius R [--connector-dir dx,dy,dz] [--connector-range a,b]` | Describe a connector by hand, in LDraw coordinates. Use this for parts not built from cylinder primitives. |
| `--placement bed\|ldraw` | See above. |
| `--smooth N` | Re-tessellate round primitives to N segments per circle (multiple of 16; 96 recommended). See [Smooth curves](#smooth-curves). |
| `--flip` | Print upside down (180° about X). Use it for hats whose stud tube would otherwise start in mid-air. |
| `--fix-t-junctions` | Split triangles at T-junctions. Never moves vertices. |
| `--close-open-shells` | Close separate open pieces (ribs, bosses) that slicers would drop, and fuse them to the body. See [Mesh validation and repair](#mesh-validation-and-repair). |
| `--orient` | Infer facing for non-BFC-certified triangles. |
| `--no-cleanup` | Keep degenerate and exact-duplicate triangles. |
| `--allow-missing` | Write output even if referenced files are missing (otherwise this is an error). |
| `--strict` | Exit with an error and write nothing unless the mesh is watertight. |
| `--no-intersections` | Skip the self-intersection check, which is faster. |

### `preview <part>`

Takes the same build options, plus `--port 8008`, `--host 127.0.0.1` and
`--no-open`. It opens `http://127.0.0.1:8008/`, which provides:

- orbit (left drag), pan (right drag) and zoom (wheel), plus Iso, Front, Side, Top and Bottom view buttons
- original LDraw colours (`LDConfig.ldr`), including transparency, or a neutral grey
- solid and wireframe modes, LDraw edge lines, and **problem edges** (open edges in red, non-manifold edges in magenta)
- back faces tinted red, so inverted or open geometry stands out
- the bounding box with X × Y × Z in mm, a ground plane, a 10 mm grid and toggleable axes
- a component picker that isolates one disconnected component; clicking a triangle shows its component, colour and source file
- the connector panel: choose a profile, a surface, a fit and a clearance, then press Regenerate.
  The **attachment region** is highlighted in cyan, and a transparent **reference connector**
  (the mating stud or post, or a grip ring for pins) plus an optional reference body (the head) are drawn
- diagnostics and warnings
- **Export STL**, which downloads exactly the geometry on screen

The page calls the same `pipeline::build` as `convert`, with the same
parameters, so the preview always matches the exported file. URL parameters
such as `?view=bottom&refbody=1&bbox=0` set the initial view and toggles.

### `inspect <part>`

Prints size, mesh statistics, every diagnostic, per-colour triangle counts
and the component list. Adding `--connector NAME` lists every cylindrical
candidate and shows whether it matches the profile. `--problems N` lists
open and non-manifold edges with coordinates and the source file of each.
`--json` gives machine-readable output.

### `calibrate --connector NAME`

Writes test-fit coupons (see [Calibration workflow](#calibration-workflow)).
Options are `--scale`, `--clearances 0.1,0.2,0.3`, `--part P` (takes the
mating size from a part when the profile has none), `--wall-mm`,
`--height-mm` and `-o`. A `.txt` summary is written next to the STL.

### `search`, `colors`, `profiles`, `guide`

```bash
ldraw2print search pirate bicorne        # 2528a, 2528ap30, … (keywords include "Pirate")
ldraw2print search 'set 6285'            # parts that list set 6285 in their keywords
ldraw2print colors black                 # 0 Black #1B2A34, …
ldraw2print profiles                     # head-stud Ø24.000 mm nominal, fit tight (+0.05 mm), …
ldraw2print guide connectors
```

`search` hides obsolete, moved, alias and physical-colour entries unless you
pass `--all`.

### `config init | show | path`, `library`

`config init` writes the commented default config to
`~/.config/ldraw2print/config.toml` (or `--path`). The full default file is
[`examples/ldraw2print.toml`](examples/ldraw2print.toml).

## Minifigure compatibility

Scaling an accessory up does not guarantee it fits a printed minifigure of
the same scale. Different printed figures use different connector sizes, printers
produce holes undersized and pins oversized, and LDraw itself models round
features as 16-sided polygons. ldraw2print keeps these concerns separate.

### Three kinds of dimension

| Kind | Where | Meaning |
|---|---|---|
| **Nominal** | `nominal_diameter_ldu` | Reference geometry at 1:1 in LDU, taken from the LDraw model of the mating part. Multiplied by the scale. |
| **Measured** | `measured_diameter_mm` | What **your** physical figure's mating feature measures, using calipers. When set, it replaces nominal × scale. |
| **Fit / compensation** | `[fit_presets]`, per-profile `fit`, `[printer]` | The clearance you want (diametral, mm) and the correction your printer needs. Determined by calibration. |

For a **socket**, the target diameter is mating + clearance + `hole_compensation_mm`.
For a **pin**, it is mating − clearance − `pin_compensation_mm`.

### Built-in profiles

All nominal values come from the LDraw geometry of the mating part. None of
them assumes a particular printed figure. **No measured values ship with the
tool**; you must enter your own.

| Profile | Gender | Nominal Ø (1:1) | At 5× (default) | Source in LDraw |
|---|---|---|---|---|
| `head-stud` | socket | 12 LDU (4.8 mm) | 24 mm | Head 3626c top stud (`stud2.dat`, r = 6). Headwear sockets are a `stud4.dat` inner wall, r = 6, opening at the part origin. |
| `head-body` | socket | 26 LDU | 52 mm | Head 3626c body (r = 13). Headwear that slides over the head itself, such as bicorne 2528, whose r = 13 recess is modelled with zero clearance. A clearance surface, so the default fit is loose. |
| `neck` | socket | 12 LDU | 24 mm | Torso 973 neck post (r = 6, 12 LDU tall). Used by backpack 2524, airtanks 3838 and similar parts. |
| `scarf` | socket | 16 LDU | 32 mm | Head 3626c bottom collar (r = 8). Used by scarves 25376 and 24504. |
| `hand-grip` | pin | 8 LDU (3.2 mm) | 16 mm | Hand 3820 grip and bar items such as spear 4497 (r = 4). |

`neck` and `scarf` accept partial circles (`min_coverage = 0.5`), because many
neck items are C-clips.

### Detection: from the part's geometry, never assumed

LDraw parts build round connection features from cylinder primitives
(`n-4cyli.dat`, often nested inside `stud4.dat`, `4-4cylo.dat` and so on).
Each instance has an exact world transform, which gives the axis, radius,
height and coverage directly, with no mesh fitting. Instances that share an
axis, radius and facing are merged into **candidates**. Facing (towards or
away from the axis) comes from the BFC-normalised triangles, so the inside
of a hat socket is never confused with the outside of a tube.

A candidate matches a profile only if all of these hold:

- the facing is correct (inward for sockets, outward for pins)
- the axis is right (vertical for headwear and neckwear)
- the diameter is within `match_tolerance` (10%) of nominal
- it covers at least `min_coverage` of a circle

**Auto-selection happens only when exactly one candidate matches.** With zero
or several matches, `convert --fit` refuses, lists every candidate with the
reason it does or doesn't match, and asks for `--connector-candidate ID`.
Surfaces from non-certified geometry have unknown facing and are never
auto-selected. Parts whose connector isn't built from primitives can be
described with `--connector-axis/--connector-radius`.

Detection matches geometry, not part semantics. For example, `head-stud`
on the head 3626c matches the head's own neck hole, which is also a Ø12
socket opening downwards. Always check the chosen surface in `inspect` or the
preview.

### Fit: change the mating surface, keep everything else

With `--fit`, only the vertices of the selected cylinder surface's own faces
move, and only radially. Other geometry that merely touches the circle (a rib
corner, say) is never dragged along. Every other vertex keeps its exact position: the exterior,
brim and ceiling are unchanged (the tests check this). If the change would
invert an adjacent triangle because the wall is too thin, the adjustment is
refused.

**Polygon correction.** A 16-sided LDraw socket of radius *r* is only
2*r*·cos(π/16) = 0.981 × 2*r* across its flats. At 5:1 a Ø12 LDU socket is
24.00 mm across the vertices but **23.54 mm across the flats**, so the
literally scaled part interferes with a 24 mm stud. The fit targets the
across-flats (narrowest) diameter for sockets and the across-corners (widest)
diameter for pins, and solves for the vertex radius that achieves it.

Without `--fit`, connector geometry is exported exactly as modelled.
`inspect` still shows what a fit would change.

### Example profile

```toml
[connectors.head-stud]
description = "Hat / hair / helmet socket over the head's top stud"
gender = "socket"
axis = "vertical"
nominal_diameter_ldu = 12.0
nominal_depth_ldu = 4.0
measured_diameter_mm = 24.35     # measured on MY figure's head stud
measured_at_scale = 5.0
default_fit = "normal"
fit = { loose = 0.45, normal = 0.25, tight = 0.10 }   # per-connector override
reference_body = { diameter_ldu = 26.0, length_ldu = 24.0 }

[printer]
hole_compensation_mm = 0.15      # this printer prints holes 0.15 mm small

[fit_presets]                    # global diametral clearances, mm
loose = 0.40
normal = 0.20
tight = 0.05
```

You can add your own profiles, such as `[connectors.my-visor-clip]`. The
built-in ones stay available unless you redefine them.

### Smooth curves

LDraw draws every round feature as a 16-sided polygon, so at 5:1 a hat brim
has flats 10 mm wide. Slicers can't fix this: Bambu Studio's "arc fitting"
only changes how already-round toolpaths are written to G-code, and it has no
subdivide or smooth tool for meshes.

`--smooth 96` (or `smooth_segments = 96` in `[mesh]`) re-tessellates the
geometry instead. Every circular LDraw primitive (cylinders, discs, rings,
cones, tori) is a surface of revolution about its local axis. The tool checks
that from the primitive's actual vertices, then re-samples it at N segments
per circle, keeping each vertex's exact radius and height. Profiles such as a
torus cross-section keep their authored resolution.

- All smoothed primitives share one angle set, so neighbouring primitives stay connected.
- Where a smoothed arc meets geometry that wasn't smoothed (hand-modelled
  polygons, square-cornered primitives such as `ndis`), the leftover flat
  sliver between chord and arc is closed with new faces. No vertex is moved.
- Geometry that merely touches a smoothed curved surface (for example a rib
  whose edge runs along a cone wall) has those contact vertices moved onto
  the true surface, so the contact survives smoothing. Flat discs and rings
  never move touching geometry.
- Hand-modelled geometry drawn flat on a smoothed surface's facets, which is
  how LDraw printed patterns (faces, logos) are made, is re-curved with it.
  It's cut at the same station angles, and each vertex keeps its angle and
  height while its radius goes from the flat facet to the true curve. This
  works on cylinders, cones, discs and tori, including where the pattern
  covers an area the primitive itself doesn't (for example the front of a
  head whose back half is a cylinder primitive). Small primitives lying flat
  on such a surface, like the eye dots in a face print, follow the surface
  instead of being rounded in their own plane.
- Anything that isn't a verified surface of revolution, or drawn on one, is
  left exactly as authored, and the report names it.
- Connector fits use the new polygon count. A 96-gon socket is 0.999 × its
  vertex diameter across the flats (16-gon: 0.981), so fits need much smaller
  adjustments.
- Use a multiple of 48 (48, 96, 144) so `p/48` high-resolution primitives are
  smoothed as well.

Results: police hat 3624 and scarf 25376 stay watertight at 96 segments, and
top hat 3878 smooths all 29 of its primitives. The printed heads tested
(3626cp01, cp02, cpx1, cpm0, cpb6) stay watertight with the print curved
onto the head: every face-print vertex on 3626cp01 ends at 26.000 mm radius
(it was 25.50–26.00 mm on the flat facets). Parts made mostly of
hand-modelled polygons (most of scarf 25376's drape, hair) only get rounder
where they use primitives; organic surfaces would need subdivision, which
also changes dimensions, so it isn't offered.

## Calibration workflow

1. Measure the mating feature on your figure (the head stud, neck post or
   hand opening) and put it in `measured_diameter_mm`.
2. Print coupons: `ldraw2print calibrate --connector head-stud`. This
   produces one coupon per preset, or one per value in `--clearances`.
   Socket coupons are short tubes; pin coupons are pins on a base.
   **Coupon *k* has *k* notches on its rim**, and the `.txt` summary lists
   each coupon's clearance and diameter.
3. Test each coupon on the figure.
   - If one fits well, record it:
     `ldraw2print calibrate --connector head-stud --record 3 --printer "Bambu Lab H2S" --material PETG`
     (pass the same `--scale`/`--clearances` as for the coupons). This
     writes a calibration record, with the coupon's exact clearance, into
     your config and keeps its comments; `--fit clearance` then uses it.
     The record notes the printer and material, since the result only
     holds for those.
   - If the best one is the tightest (or loosest) coupon, it may not be the
     true optimum: print a finer sweep around it, such as
     `--clearances -0.05,0,0.05,0.1`.
   - If every coupon is uniformly too tight or too loose, adjust
     `[printer] hole_compensation_mm` / `pin_compensation_mm` instead.
4. Convert the accessory with `--connector … --fit clearance`.

Coupons are watertight surfaces of revolution with 144 segments, so the
across-flats error is under 0.01 mm. Printing calibration pieces is the only
way to know a part fits; the tool never claims mechanical compatibility.

## Mesh validation and repair

LDraw geometry is designed for rendering. Open seams, T-junctions between
primitives, zero-thickness faces and overlapping shells are all normal in
LDraw, so diagnostics separate **errors** from **warnings**:

| Check | Severity |
|---|---|
| Open boundary edges | error |
| Non-manifold edges (shared by more than 2 triangles) | error |
| Inconsistent winding between neighbours | error |
| Closed mesh with non-positive volume (inverted or flat) | error |
| Empty mesh | error |
| Collapsed triangles (two corners at the same vertex) | warning; blocks "watertight" |
| Needle-thin triangles (three distinct corners) | info; kept when removing them would open the mesh |
| Exact duplicate triangles | warning |
| Back-to-back triangle pairs (zero-thickness walls) | warning |
| Non-BFC-certified triangles (facing is a guess) | warning |
| Triangle–triangle intersections / overlapping components | warning |
| T-junction vertices on open edges | info |
| Disconnected components | info |

**Watertight** is reported only when the mesh is closed, manifold,
consistently wound and has positive volume. Exporting triangles successfully
does not count. Bambu Studio's own checks (`BambuStudio --info`) agree:
3624 is manifold, and 3901 has the same 34 open edges and 1 non-manifold edge
that ldraw2print reports.

**Default cleanup** removes collapsed triangles, needle-thin triangles whose
removal can't open the mesh, and exact duplicates.
**Opt-in repairs** never move a vertex:

- `--fix-t-junctions` splits a triangle wherever another vertex lies on its open edge.
- `--orient` makes winding consistent per component, seeded from certified
  triangles. A fully uncertified closed shell is flipped to positive volume.
- `--close-open-shells` handles a common LDraw shortcut: features such as
  the ribs inside a hat or a boss on its surface are drawn as separate pieces
  without the faces where they meet the body, because those faces are never
  seen. Slicers drop or garble such open pieces. Without this option, the
  bicorne 2528's four stud-gripping ribs simply vanish from the slice. For
  every open piece except the main body, the repair:
  1. caps flat openings, or
  2. completes the piece to its convex shape, but only when its existing
     faces lie on that shape and cover at least 60% of it, so no solid is
     invented from an open sheet; then
  3. adds a thin hidden anchor (0.25 LDU into each side) wherever the closed
     piece only touches the body, so the slicer fuses them instead of
     printing separate islands. An anchor is added only after a
     winding-number test confirms it lies entirely inside the body's
     material.

  No existing vertex moves. Pieces that can't be closed safely are left and
  reported. Diagnostics warn about separate open shells and suggest this
  option.

Nothing closes holes, unions solids or remeshes, because any of those could
change connector fit. Repairs implement the `mesh::RepairStep` trait so that a
heavier backend, such as a mesh-boolean library, can be added later.

**BFC.** `CERTIFY CCW/CW`, `NOCERTIFY`, `CCW`/`CW`, `CLIP`/`NOCLIP` and
`INVERTNEXT` are implemented, including mirrored (negative-determinant)
transforms. Geometry from non-certified files, and everything below them, is
flagged `oriented = false`.

### Results on real parts (official library, 5:1, default settings)

| Part | Size (mm) | Watertight | Connector detected |
|---|---|---|---|
| 3624 Police Hat | 65.36 × 71.42 × 31.76 | yes | head-stud socket Ø12 LDU (auto) |
| 3901 Hair Male | 60.05 × 61.84 × 48.14 | no | head-stud (auto) |
| 3629 Cowboy Hat | 92.00 × 92.00 × 40.00 | no | head-stud (auto) |
| 3878 Top Hat | 80.00 × 80.00 × 42.00 | no | head-stud (auto) |
| 3833 Construction Helmet | 68.00 × 78.00 × 34.00 | no | head-stud (auto) |
| 25376 Scarf Long Wrapped | 68.18 × 60.75 × 68.54 | yes | scarf Ø16 LDU (auto) |
| 24504 Face Scarf | 67.39 × 60.18 × 23.69 | yes | scarf, 76% C-clip (auto) |
| 2524 Backpack | 60.00 × 70.00 × 88.00 | no | neck (auto) |
| 4497 Spear | 28.00 × 16.00 × 292.00 | no | hand-grip pin Ø8 LDU (auto) |
| 3959 Torch | 40.00 × 104.00 × 52.00 | no | 3 matching bars: **refuses to guess** |

Several official parts are not closed as authored. In 3901, for example,
the internal tube wall stops where the inner hair surface meets it, which
leaves a surface-on-surface seam that only boolean surgery could close.
`inspect --problems 50` shows exactly where the open edges are. Bambu
Studio usually prints such parts acceptably, but check the sliced preview.

## Bambu Studio workflow

1. `ldraw2print convert 3624 --connector head-stud --fit clearance -o hat.stl`
2. In Bambu Studio choose **File → Import → Import 3MF/STL**. The file is in mm,
   so accept the size as-is and don't use the "inches?" prompt. Verified with
   Bambu Studio's CLI, `BambuStudio --info hat.stl`, which reports
   `size_x = 65.36, size_y = 71.42, size_z = 31.76, manifold = yes`.
3. Orientation: parts arrive upright (Z up) on the plate. For headwear, the
   socket faces the plate. Keeping the socket axis vertical keeps its layer
   lines circular, which is usually best for fit. If the slicer warns about
   floating regions, or would place supports inside the socket, convert with
   `--flip` to print upside down. Hats whose stud tube hangs from the crown,
   such as top hat 3878, need this. Use tree supports under brims.
4. If Bambu Studio offers to repair a non-watertight part, check that the
   repair doesn't touch the connector area.
5. Multicolour: see below.

### Multicolour

LDraw printed patterns (faces, torso prints, decorated hats) aren't separate
solid pieces: the coloured regions are patches of the part's one closed
surface. `convert … -o part.3mf` therefore writes **one solid with painted
faces** (`--color-mode paint`, the default):

- The most-used colour is filament 1 and is left unpainted. Every other
  colour is painted with filament 2, 3, … on the triangles LDraw gives it.
- Paint is stored as `paint_color` (Bambu Studio / OrcaSlicer) and
  `slic3rpe:mmu_segmentation` (PrusaSlicer) on each triangle, plus core-3MF
  base materials, so plain 3MF viewers also show the colours.
- `convert` prints which LDraw colour goes in which filament slot, for example:

  ```text
  Load these filaments in the slicer, in this order:
    filament 1: Yellow (LDraw 14, #FAC80A) — 3490 triangles, unpainted base
    filament 2: Black (LDraw 0, #1B2A34) — 202 triangles
  ```

  The same list is in the 3MF's Description metadata. Load filaments in that
  order (for an AMS, map the slots). The 3MF doesn't set filament colours;
  pick them in the slicer.
- The preview has an **Export 3MF (painted)** button.

Bambu Studio fills painted colour inward from the surface, so details are
solid colour a few millimetres deep, not a surface skin.

Verified on head 3626cp01 at 5:1 with Bambu Studio's command line (H2S,
two PETG filaments): it sliced with 42 filament selects, about 0.54 m of
black filament, and 1 h 40 min. Reading the black extrusions back out of the
G-code reproduces the grin's eyes and smile. 98% of black extrusions lie
behind black-painted surface, and no yellow extrusions do.

`--color-mode parts` writes the older layout: one object with one part per
colour. Use it only when each colour is a closed solid of its own
(assemblies); for printed patterns each part would be an open shell.

On a single-nozzle printer every colour change is a filament swap with a
purge, so small details can add many swaps (42 for the grin head). Bambu
Studio shows the flush volume before printing.

## Other slicers (PrusaSlicer, OrcaSlicer, Cura)

ldraw2print has no slicer-specific output. STL files are plain binary STL in
millimetres with Z up. 3MF files use the core 3MF specification plus the
per-triangle paint attributes that Bambu Studio, OrcaSlicer and PrusaSlicer
read (see [Multicolour](#multicolour)).
Everything about fit (connector adjustment, `--flip`, placement) is baked
into the geometry before export. Only Bambu Studio has been tested so far;
the notes below describe how each slicer's features relate to the tool and
haven't been verified against a real slice. That includes painted 3MF in
these slicers.

**In every slicer**

- Import the STL as-is. It's already in mm and on the plate (Z = 0). If a
  slicer offers to convert from inches or metres, decline.
- Don't scale in the slicer. Change `--scale` instead, so the connector fit
  is recalculated for the new size.
- Orientation is chosen at conversion time. If you rotate the part in the
  slicer, keep the socket or pin axis vertical for the most accurate fit.
- Don't compensate twice. If you use a slicer's hole or XY-size
  compensation, leave `[printer] hole_compensation_mm` and
  `pin_compensation_mm` at 0, and the other way round. Calibrate with the
  same settings you'll print with.
- Turn off the slicer's own auto-orientation and "arrange with rotation" for
  these parts; they would undo `--flip`.
- Non-watertight LDraw parts may trigger an automatic repair message. Check
  the sliced preview of the connector area before printing (see
  [Mesh validation and repair](#mesh-validation-and-repair)).

**PrusaSlicer**

- Import with File → Import → Import STL/3MF/STEP (Ctrl+I).
- PrusaSlicer has a general "XY Size Compensation" setting (Print Settings →
  Advanced), which shrinks or grows the whole outline, holes included. It
  affects the exterior as well, so prefer ldraw2print's connector-only
  compensation and leave XY Size Compensation at 0.
- Use organic supports for brims and scarf tails. To keep supports out of a
  socket, use a support blocker or "Support on build plate only".
- Painted 3MF carries `slic3rpe:mmu_segmentation` paint, which PrusaSlicer's
  multi-material painting reads, for MMU or XL tools. Untested.

**OrcaSlicer**

- OrcaSlicer is a fork of Bambu Studio, so the Bambu Studio workflow above
  applies almost unchanged.
- Painted 3MF uses the same `paint_color` attribute as Bambu Studio, so it
  should carry over. Untested.
- It has separate "X-Y hole compensation" and "X-Y contour compensation"
  settings. Hole compensation affects every hole, not just the connector, so
  using ldraw2print's per-connector fit is more targeted. Use one or the
  other.

**Cura**

- Check Preferences → General → "Scale large models" and "Scale extremely
  small models" are off. Either can silently resize a scaled accessory.
- Cura's "Hole Horizontal Expansion" affects all holes. Same advice as
  above: use it or the tool's compensation, not both.
- Tree supports work well for brims and scarves; use a support blocker to
  keep supports out of sockets.
- Cura doesn't read per-triangle paint, so painted 3MF imports as a single
  colour there. For multiple extruders in Cura, `--color-mode parts` gives
  separate parts, with the open-shell caveat. Untested.

**Calibration in any slicer**

Calibration coupons are plain STLs. Slice them with the exact profile
(material, layer height, wall count, compensation) you'll use for the
accessory, otherwise the measured fit won't transfer.

## Worked example: police hat 3624

```text
$ ldraw2print convert 3624 --scale 5 --connector head-stud --fit clearance -o hat.stl
3624.dat — Minifig Police Hat
Scale 5:1 (2.000 mm per LDU), Z up, millimetres
Size  65.36 × 71.42 × 31.76 mm (X × Y × Z)
Mesh  742 triangles (742 from LDraw), 373 vertices, 1 component(s), 8 files
Watertight: yes
Volume 46.199 cm³
  warning no measured_diameter_mm for 'head-stud': using nominal Ø12 LDU × scale. Measure your figure and calibrate before trusting the fit.
  warning surface is a 16-sided polygon: across-flats and across-corners differ by 0.46 mm; ...

Connector profile 'head-stud'
  Surface #1 (auto): socket Ø12.000 LDU, depth 4.000 LDU
  Modelled    Ø24.000 mm across vertices, Ø23.539 mm effective (16-gon)
  Mating      Ø24.000 mm (nominal)
  Fit         normal (profile default) clearance 0.200 mm + compensation 0.000 mm → target Ø24.200 mm effective
  Applied     moved 32 vertices radially by +0.337 mm; all other geometry unchanged
```

Expected dimensions at 5:1: overall 65.36 × 71.42 × 31.76 mm, watertight.
The socket is Ø24.00 mm across the vertices and Ø23.54 mm across the flats as
modelled, and 8 mm deep. With `--fit clearance` it is Ø24.20 mm across the
flats.

The self-contained fixture `tests/fixtures/ldraw/parts/testhat.dat` is a plain
16-sided hat (crown r = 14 LDU, 10 LDU tall; socket r = 6 LDU, 4 LDU deep).
At 5:1 it is **56.00 × 56.00 × 20.00 mm** and **44 477 mm³**, with a socket of
Ø24.00 mm across the vertices and Ø23.54 mm across the flats. The integration
tests assert all of these values.

## Architecture

```
src/
  ldraw.rs       parsing, MPD, library lookup, colours (LDConfig.ldr), recursive
                 resolution, colour 16/24 inheritance, BFC winding, circular-ref detection
  geometry.rs    Vec3 / affine transforms, quad triangulation, LDraw → print conversion
  mesh.rs        welding, topology diagnostics, intersections, RepairStep pipeline
  connectors.rs  primitive-based connector detection, profile matching, fit planning,
                 mating-surface adjustment, reference shapes
  calibrate.rs   watertight test-fit coupons
  config.rs      TOML config, fit presets, connector profiles
  pipeline.rs    the single build pipeline shared by convert / inspect / preview
  export.rs      Exporter trait: binary STL, painted and per-part 3MF, preview JSON
  errors.rs      error kinds → exit codes (decision needed, missing files, not watertight)
  smooth.rs      re-tessellation of circular primitives (--smooth)
  preview.rs     tiny_http server + JSON API
  report.rs      text reports
  main.rs        CLI (clap): commands, --json, exit codes
  guide.rs       built-in guide topics (docs/guide/*.md, AGENTS.md)
assets/preview.html          viewer (vanilla JS module + Three.js)
assets/vendor/               three.js r160 + OrbitControls (MIT)
examples/ldraw2print.toml    default/annotated configuration
tests/fixtures/ldraw/        self-contained mini LDraw library (gen_fixtures.py)
```

Pipeline: resolve → weld (0.005 LDU) → cleanup → optional repairs →
connector detection and fit (in LDraw space, where the primitive transforms
live) → convert to mm, Z up → placement → diagnostics. Colour codes, source
files and component ids stay attached to every triangle, so a full 3MF
exporter only needs to implement `Exporter`.

Dependencies: `clap`, `serde`, `serde_json`, `toml`, `tiny_http`, `anyhow`.

## Testing

```bash
cargo test                             # unit + integration tests (fixtures only)
LDRAWDIR=~/ldraw cargo test            # also runs the real-library tests (3624, headwear)
```

Covered:

- parsing triangles, quads and type 1 lines (including names with spaces and backslashes)
- nested references and matrix composition
- mirrored transforms, BFC CW, INVERTNEXT, NOCLIP and uncertified geometry
- colour 16/24 inheritance and LDConfig parsing
- LDU → mm scaling and axis mapping
- STL triangle counts, bounding boxes and round trips
- missing and circular references
- welding, degenerate/duplicate detection, open, non-manifold and inverted meshes
- T-junction repair (which must not move vertices) and orientation repair
- connector detection, fit plans and adjustment (only socket vertices move, exterior bbox identical),
  manual connectors and refusal on ambiguity
- calibration coupons (watertight, correct bore)
- preview/export parity
- the 3MF packages (painted single solid, per-colour parts) and the paint encoding
- the CLI end to end

To regenerate the fixtures, run `python3 tests/fixtures/gen_fixtures.py`.

## Limitations

- **Fit is not guaranteed.** Nominal values describe idealised LDraw geometry.
  Real printed figures vary, so measure and calibrate.
- Connector detection only recognises cylinder primitives (`n-4cyli`).
  Conical, ribbed or freeform sockets need `--connector-axis`, and fits are
  radial only (no tapers or clips).
- Detection matches geometry, not meaning. It can't tell a hat socket from
  any other downward Ø12 hole, so check the selected surface.
- Most official parts are not watertight. The tool reports this accurately
  but doesn't close holes or union shells (by design: no silent geometry
  changes).
- The self-intersection check reports crossing triangles but ignores
  coplanar overlaps.
- `--primitives high` substitutes 48-segment primitives where they exist.
  This can leave seams where they meet standard-resolution geometry.
- Painted 3MF assigns whole LDraw triangles to filaments (no sub-triangle
  brush detail), doesn't set filament colours, and has only been verified
  in Bambu Studio. `--color-mode parts` is still experimental.

## Licensing

- **ldraw2print's code** is under the [MIT licence](LICENSE).
- **The LDraw parts library** is not included. You download it separately,
  and it is licensed CC BY 2.0 / CC BY 4.0 by its authors.
- **Converted models are derived from LDraw parts.** If you share the STL or
  3MF files, or prints made from them, credit LDraw.org and the part's
  author. The author is on the `0 Author:` line of the part's `.dat` file.
- **`examples/3878-print.dat`** is a corrected copy of LDraw part 3878 by
  Chris Dee. It stays under CC BY 4.0, as its header states.
- **`tests/fixtures/ldraw/`** holds small test parts written for this project
  (MIT), plus a few colour definitions taken from LDraw.org's `LDConfig.ldr`.
- **Three.js** (`assets/vendor/`) is MIT; see `assets/vendor/LICENSE-three.txt`.
