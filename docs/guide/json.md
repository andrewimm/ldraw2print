# JSON output (--json)

`--json` is a global flag: put it anywhere (`ldraw2print --json convert …`).
Each command prints exactly one JSON object on stdout; human text is
suppressed. Diagnostics that are part of the result are inside the JSON.

Every object has `"ok": true` and `"command"`. On failure:

    {"ok": false, "error": "…", "kind": "error|decision-needed|missing-files|not-watertight", "exit_code": N}

and the process exits with that code (see `guide agents`).

convert / inspect (shared fields)
- `part` {name, description}, `scale`, `mm_per_ldu`
- `size_mm` [x,y,z], `bbox_min_mm`, `bbox_max_mm`, `triangles`
- `verdict` {status: "ready"|"review", reasons [string], notes [string]}:
  "ready" = geometry should slice cleanly (says nothing about fit);
  `reasons` say what to do; `notes` are informational
- `watertight` (bool), `volume_cm3` (only when watertight, else null)
- `component_count`, `components` (the 20 largest) [{id, triangles, closed, size_mm}]
- `main_color` {code, name, source: "--color"|"default"}
- `placement` {mode: bed|ldraw, flip}
- `smoothing_new_seams_mm` (null without --smooth): length of open seams
  that smoothing created; > 0 means build without --smooth for this part
- `fit_summary` (null without --connector) {profile, applied, mating_source,
  calibrated, status}. status: "not-applied", "measured+calibrated" (the only
  verified state), "calibrated-preset-nominal-size", "measured-uncalibrated",
  "unverified"
- `connector_suggestions` (only without --connector) [{profile, candidates, unique, diameter_ldu}]:
  profiles whose detection matches a surface; geometric, so judge relevance
- `colors` [{code, name, triangles}]
- `issues` [{severity: error|warning|info, code, message}]  (see `guide repair`)
- `warnings` [string], `repairs` [{step, changed, note}], `missing_files`
- `connector` (null without --connector):
  {profile, calibrated (bool: a calibration record exists), calibration {date, printer,
   material, scale, result, notes} | null, default_fit_source: "user config"|"built-in default", customized [..],
   selected {id, how: auto|explicit, facing, diameter_ldu, depth_ldu, segments} | null,
   selection_error | null, plan {mating_diameter_mm, mating_source, clearance_mm,
   compensation_mm, target_effective_diameter_mm, segments, …} | null,
   fit_applied {moved_vertices, radial_change_mm, max_displacement_mm} | null,
   candidates [{id, facing, diameter_ldu, depth_ldu, coverage, segments, axis_dir, summary,
                matches_profile, reject_reason}]}

convert adds
- `output` {path, format: stl|3mf, color_mode: paint|parts|null, bytes}
- `filaments` [{slot, code, name, hex, triangles, base}] (painted 3MF only)

inspect adds `files`, `uncertified_files`, `diagnostics` (counts; issues and
components are at top level), and with
`--problems N`: `problem_edges` {total, edges [{kind, from_mm, to_mm, files}]}
(print-frame mm after placement; connector candidates use LDraw LDU/axes).

calibrate: `connector`, `gender`, `scale`, `mating_diameter_mm`,
`mating_source`, `compensation_mm`, `coupons` [{index, notches, label,
clearance_mm, diameter_mm, position_mm}], `output` {stl, summary}.

search: `query`, `total`, `hits` [{part, description, category, keywords, unofficial, secondary}].
colors: `source`, `colors` [{code, name, hex, alpha, edge_hex}].
profiles: `scale`, `config_file`, `printer`, `profiles` [{name, calibrated, calibration,
customized, default_fit_source, description,
gender, axis, nominal_diameter_ldu, nominal_diameter_mm, nominal_depth_ldu,
measured_diameter_mm, mating_diameter_mm, default_fit, default_clearance_mm,
fit_presets_mm, min_coverage, match_tolerance}].
library: `path`, `found_via`, `parts`, `colours`, `problem`.
guide: `topics` [{name, summary}] or `topic`, `text`.
config show: `path`, `config`. config path: `paths` [{path, exists}].

`preview` has no JSON mode (it is an interactive server).
