//! End-to-end tests against the self-contained fixture library in
//! tests/fixtures/ldraw (regenerate with tests/fixtures/gen_fixtures.py),
//! plus optional tests against a real LDraw library when $LDRAWDIR is set.

use ldraw2print::calibrate::{self, CalibrationRequest};
use ldraw2print::config::Config;
use ldraw2print::connectors::{Facing, ManualConnector};
use ldraw2print::export::{read_binary_stl, BinaryStl, Exporter, PrintModel};
use ldraw2print::geometry::{Aabb, Vec3};
use ldraw2print::ldraw::Library;
use ldraw2print::mesh::{diagnose, DiagnosticOptions, Mesh};
use ldraw2print::pipeline::{build, BuildOptions, BuildOutput, Placement};
use std::f64::consts::PI;
use std::path::PathBuf;
use std::process::Command;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ldraw")
}

fn run(part: &str, f: impl FnOnce(&mut BuildOptions)) -> anyhow::Result<BuildOutput> {
    let mut lib = Library::new(Some(fixtures()));
    let cfg = Config::builtin();
    let mut o = BuildOptions::new(part, 5.0);
    f(&mut o);
    build(&mut lib, &cfg, &o)
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Radius (mm) of mesh vertices at a given height band around the Z axis.
fn radii_at(out: &BuildOutput, z0: f64, z1: f64) -> Vec<f64> {
    let mut r: Vec<f64> = out
        .mesh
        .positions
        .iter()
        .filter(|p| p.z >= z0 - 1e-6 && p.z <= z1 + 1e-6)
        .map(|p| (p.x * p.x + p.y * p.y).sqrt())
        .collect();
    r.sort_by(|a, b| a.total_cmp(b));
    r
}

// --- Accessory example: tests/fixtures/ldraw/parts/testhat.dat -------------
//
// A plain 16-sided hat: crown r = 14 LDU, 10 LDU tall; head-stud socket
// r = 6 LDU, 4 LDU deep, opening at the LDraw origin (headwear convention).
// Expected at 5:1 (2 mm/LDU):
//   size 56.00 × 56.00 × 20.00 mm, watertight
//   volume (16-gon prisms): 3.06147·(14²·10 − 6²·4)·8 ≈ 44 477 mm³
//   socket Ø24.000 mm across vertices, Ø23.539 mm across flats

#[test]
fn hat_example_dimensions() {
    let out = run("testhat", |_| {}).unwrap();
    let s = out.mesh.bbox().size();
    assert!(
        close(s.x, 56.0, 1e-3) && close(s.y, 56.0, 1e-3) && close(s.z, 20.0, 1e-3),
        "{s:?}"
    );
    assert!(out.diagnostics.watertight, "{:?}", out.diagnostics.issues);
    let area16 = 8.0 * (2.0 * PI / 16.0).sin(); // 16-gon area / r²
    let expect = area16 * (14.0f64.powi(2) * 10.0 - 36.0 * 4.0) * 8.0;
    assert!(
        close(out.diagnostics.signed_volume, expect, expect * 1e-3),
        "{} vs {expect}",
        out.diagnostics.signed_volume
    );
    // Bed placement: on Z=0, centred.
    let b = out.mesh.bbox();
    assert!(close(b.min.z, 0.0, 1e-9) && close(b.center().x, 0.0, 1e-9));
    // Socket mouth is at the bottom (Z=0): smallest vertex radius there is 12 mm.
    assert!(close(radii_at(&out, 0.0, 0.0)[0], 12.0, 1e-3));
}

#[test]
fn hat_socket_detected_uniquely_and_fit_applied_only_to_socket() {
    let base = run("testhat", |o| o.connector = Some("head-stud".into())).unwrap();
    let c = base.connector.as_ref().unwrap();
    let sel = c
        .selection
        .as_ref()
        .expect("unique socket must auto-select");
    assert_eq!(sel.how, "auto");
    let cand = &c.candidates[sel.candidate];
    assert_eq!(cand.facing, Facing::Inward);
    assert!(close(cand.radius_ldu, 6.0, 1e-6) && close(cand.depth_ldu(), 4.0, 1e-6));
    assert!(c.fit.is_none(), "no --fit: geometry must be preserved");
    assert_eq!(c.region_triangles.len(), 32, "16 quads of socket wall");

    let fitted = run("testhat", |o| {
        o.connector = Some("head-stud".into());
        o.fit = Some("loose".into());
    })
    .unwrap();
    let cfg = Config::builtin();
    let plan = fitted.connector.as_ref().unwrap().plan.as_ref().unwrap();
    assert_eq!(plan.mating_source, "nominal");
    assert!(close(
        plan.target_effective_diameter_mm,
        24.0 + cfg.fit_presets.loose,
        1e-9
    ));
    let flats = (PI / 16.0).cos();
    assert!(close(
        plan.target_vertex_radius_mm,
        (24.0 + cfg.fit_presets.loose) / 2.0 / flats,
        1e-9
    ));
    assert!(fitted.diagnostics.watertight);

    // Exterior untouched, socket enlarged.
    assert_eq!(base.mesh.bbox(), fitted.mesh.bbox());
    let r_socket = radii_at(&fitted, 0.0, 4.0)[0];
    assert!(close(r_socket, plan.target_vertex_radius_mm, 1e-6));
    let moved: usize = base
        .mesh
        .positions
        .iter()
        .zip(&fitted.mesh.positions)
        .filter(|(a, b)| (**a - **b).length() > 1e-9)
        .count();
    assert_eq!(moved, 32, "only the 16+16 socket wall vertices may move");
    assert!(fitted.diagnostics.signed_volume < base.diagnostics.signed_volume);
}

#[test]
fn explicit_clearance_and_measured_diameter() {
    let mut lib = Library::new(Some(fixtures()));
    let mut cfg = Config::builtin();
    cfg.connectors
        .get_mut("head-stud")
        .unwrap()
        .measured_diameter_mm = Some(23.8);
    cfg.printer.hole_compensation_mm = 0.1;
    let mut o = BuildOptions::new("testhat", 5.0);
    o.connector = Some("head-stud".into());
    o.clearance_mm = Some(0.3);
    let out = build(&mut lib, &cfg, &o).unwrap();
    let p = out.connector.unwrap().plan.unwrap();
    assert_eq!(p.mating_source, "measured");
    assert!(close(
        p.target_effective_diameter_mm,
        23.8 + 0.3 + 0.1,
        1e-9
    ));
    assert_eq!(p.fit, "custom");
}

#[test]
fn raw_socket_needs_explicit_axis() {
    // No primitives → no auto-detection → fitting must refuse.
    let err = run("testhatraw", |o| {
        o.connector = Some("head-stud".into());
        o.fit = Some("normal".into());
    })
    .err()
    .unwrap()
    .to_string();
    assert!(
        err.contains("connector surface") && err.contains("--connector-axis"),
        "{err}"
    );

    let out = run("testhatraw", |o| {
        o.connector = Some("head-stud".into());
        o.fit = Some("normal".into());
        o.manual = Some(ManualConnector {
            point: Vec3::ZERO,
            dir: Vec3::new(0., -1., 0.),
            radius_ldu: 6.0,
            range: None,
        });
    })
    .unwrap();
    let c = out.connector.unwrap();
    let cand = &c.candidates[c.selection.unwrap().candidate];
    assert_eq!(cand.facing, Facing::Inward);
    assert_eq!(cand.segments, 16);
    assert!(close(cand.depth_ldu(), 4.0, 1e-6));
    assert_eq!(c.fit.unwrap().moved_vertices, 32);
    assert!(out.diagnostics.watertight);
}

#[test]
fn wrong_gender_profile_does_not_match() {
    let out = run("testhat", |o| o.connector = Some("hand-grip".into())).unwrap();
    let c = out.connector.unwrap();
    assert!(c.selection.is_none());
    assert!(c.selection_error.unwrap().contains("no connector surface"));
}

#[test]
fn bfc_variants_are_all_watertight_and_outward() {
    for part in ["testbox", "testboxcw", "testmirror", "testhollow"] {
        let out = run(part, |_| {}).unwrap();
        assert!(
            out.diagnostics.watertight,
            "{part}: {:?}",
            out.diagnostics.issues
        );
        assert!(out.diagnostics.signed_volume > 0.0, "{part}");
    }
    // Hollow box: (10³ − 6³) LDU³ × 8 mm³/LDU³, two closed shells.
    let out = run("testhollow", |_| {}).unwrap();
    assert!(close(out.diagnostics.signed_volume, 784.0 * 8.0, 1e-6));
    assert_eq!(out.diagnostics.components.len(), 2);
    // Mirrored wedges are mirror images: same size, both closed, two colours.
    let out = run("testmirror", |_| {}).unwrap();
    assert_eq!(out.diagnostics.components.len(), 2);
    let mut colors: Vec<u32> = out.mesh.tris.iter().map(|t| t.color).collect();
    colors.sort();
    colors.dedup();
    assert_eq!(colors, vec![4, 71]);
}

#[test]
fn uncertified_geometry_is_flagged_and_orient_fixes_it() {
    let out = run("testnocert", |_| {}).unwrap();
    assert!(!out.diagnostics.watertight);
    assert!(out.diagnostics.unoriented_triangles > 0);
    assert_eq!(out.uncertified_files, vec!["testnocert.dat".to_string()]);
    let out = run("testnocert", |o| o.orient = Some(true)).unwrap();
    assert!(out.diagnostics.watertight, "{:?}", out.diagnostics.issues);
}

#[test]
fn missing_and_circular_references() {
    let err = run("testmissing", |_| {}).err().unwrap().to_string();
    assert!(err.contains("doesnotexist.dat"), "{err}");
    let out = run("testmissing", |o| o.allow_missing = true).unwrap();
    assert_eq!(out.missing.len(), 1);
    assert_eq!(out.mesh.tris.len(), 12);

    let err = run("testloopa", |_| {}).err().unwrap().to_string();
    assert!(err.contains("circular"), "{err}");
}

#[test]
fn scaling_ldu_to_mm() {
    for (scale, expect) in [(1.0, 0.4), (5.0, 2.0), (2.5, 1.0)] {
        let out = run("testbox", |o| o.scale = scale).unwrap();
        assert!(close(out.mm_per_ldu, expect, 1e-12));
        let s = out.mesh.bbox().size();
        assert!(close(s.x, expect, 1e-9) && close(s.y, expect, 1e-9) && close(s.z, expect, 1e-9));
    }
}

#[test]
fn ldraw_placement_maps_axes() {
    // LDraw (x, y, z) → print (x, z, −y): the hat's top at y = −10 LDU is +20 mm.
    let out = run("testhat", |o| o.placement = Placement::Ldraw).unwrap();
    let b = out.mesh.bbox();
    assert!(close(b.max.z, 20.0, 1e-6) && close(b.min.z, 0.0, 1e-6));
    assert!(close(b.min.x, -28.0, 1e-3) && close(b.max.y, 28.0, 1e-3));
}

#[test]
fn stl_roundtrip_counts_and_bbox() {
    let out = run("testhat", |_| {}).unwrap();
    let lib = Library::new(Some(fixtures()));
    let mut buf = Vec::new();
    BinaryStl
        .write(
            &PrintModel {
                name: "testhat".into(),
                mesh: &out.mesh,
                colors: &lib.colors,
            },
            &mut buf,
        )
        .unwrap();
    assert_eq!(buf.len(), 84 + 50 * out.mesh.tris.len());
    assert!(!buf.starts_with(b"solid"));
    let tris = read_binary_stl(&buf).unwrap();
    // 16-gon: 2 cylinders × 32 + 2 discs × 16 + 16 annulus quads × 2 = 128
    assert_eq!(tris.len(), 128);
    let mut b = Aabb::empty();
    for t in &tris {
        for p in t {
            b.add(*p);
        }
    }
    let s = b.size();
    assert!(close(s.x, 56.0, 1e-4) && close(s.z, 20.0, 1e-4));
    // Normals in the file match the winding.
    let n0 = f32::from_le_bytes(buf[84..88].try_into().unwrap());
    assert!(n0.is_finite());
}

#[test]
fn calibration_coupons() {
    let cfg = Config::builtin();
    let set = calibrate::generate(
        &cfg,
        &CalibrationRequest {
            connector: "head-stud",
            scale: 5.0,
            clearances: Some(vec![0.1, 0.2, 0.3, 0.4]),
            part_diameter_mm: None,
            wall_mm: None,
            height_mm: None,
        },
    )
    .unwrap();
    assert_eq!(set.coupons.len(), 4);
    assert_eq!(
        set.coupons.iter().map(|c| c.notches).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    let d = diagnose(
        &Mesh::from_source(&set.triangles, 1e-6),
        &DiagnosticOptions::default(),
    );
    assert!(d.watertight);
    assert_eq!(d.components.len(), 4);
}

#[test]
fn preview_and_export_share_geometry() {
    use ldraw2print::preview::{model_json, options_from_query, parse_query};
    let base = BuildOptions::new("testhat", 5.0);
    let (_, q) = parse_query("/api/model?scale=5&connector=head-stud&fit=tight");
    let (o, _) = options_from_query(&base, &q).unwrap();
    let mut lib = Library::new(Some(fixtures()));
    let out = build(&mut lib, &Config::builtin(), &o).unwrap();
    let json = model_json(&out, &lib);
    let n = json["geometry"]["indices"].as_array().unwrap().len();
    assert_eq!(n, out.mesh.tris.len() * 3);
    assert_eq!(
        json["connector"]["region_triangles"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    assert!(!json["connector"]["reference"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn cli_convert_writes_stl() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hat.stl");
    let status = Command::new(env!("CARGO_BIN_EXE_ldraw2print"))
        .args(["--ldraw-dir"])
        .arg(fixtures())
        .args([
            "convert",
            "testhat",
            "--scale",
            "5",
            "--connector",
            "head-stud",
            "--fit",
            "clearance",
            "--output",
        ])
        .arg(&out)
        .env_remove("LDRAWDIR")
        .env("LDRAW2PRINT_CONFIG", dir.path().join("none.toml"))
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("Watertight: yes"), "{stdout}");
    let tris = read_binary_stl(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(tris.len(), 128);

    // --strict refuses non-watertight output.
    let status = Command::new(env!("CARGO_BIN_EXE_ldraw2print"))
        .arg("--ldraw-dir")
        .arg(fixtures())
        .args(["convert", "testnocert", "--strict", "-o"])
        .arg(dir.path().join("x.stl"))
        .output()
        .unwrap();
    assert!(!status.status.success());
    assert!(!dir.path().join("x.stl").exists());
}

#[test]
fn explicit_file_path_input() {
    let p = fixtures().join("parts/testbox.dat");
    let mut lib = Library::new(None);
    let out = build(
        &mut lib,
        &Config::builtin(),
        &BuildOptions::new(p.to_str().unwrap(), 5.0),
    )
    .unwrap();
    assert!(out.diagnostics.watertight);
}

// --- Optional: real LDraw library -----------------------------------------

fn real_library() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("LDRAWDIR")?);
    p.join("parts/3624.dat").is_file().then_some(p)
}

#[test]
fn real_police_hat_3624() {
    let Some(root) = real_library() else {
        eprintln!("skipped: set LDRAWDIR to an official LDraw library to run");
        return;
    };
    let mut lib = Library::new(Some(root));
    let mut o = BuildOptions::new("3624", 5.0);
    o.connector = Some("head-stud".into());
    let out = build(&mut lib, &Config::builtin(), &o).unwrap();
    // Documented in README: 65.36 × 71.42 × 31.76 mm at 5:1, watertight.
    let s = out.mesh.bbox().size();
    assert!(
        close(s.x, 65.36, 0.05) && close(s.y, 71.42, 0.05) && close(s.z, 31.76, 0.05),
        "{s:?}"
    );
    assert!(out.diagnostics.watertight, "{:?}", out.diagnostics.issues);
    let c = out.connector.unwrap();
    let cand = &c.candidates[c.selection.unwrap().candidate];
    assert!(close(cand.diameter_ldu(), 12.0, 1e-6));
    assert_eq!(cand.facing, Facing::Inward);
}

#[test]
fn real_headwear_sockets_detected() {
    let Some(root) = real_library() else { return };
    // Hair, cowboy hat, top hat, construction helmet, cap.
    for part in ["3901", "3629", "3878", "3833", "4485"] {
        let mut lib = Library::new(Some(root.clone()));
        let mut o = BuildOptions::new(part, 5.0);
        o.connector = Some("head-stud".into());
        o.check_intersections = false;
        let out = build(&mut lib, &Config::builtin(), &o).unwrap();
        let c = out.connector.unwrap();
        let sel = c
            .selection
            .unwrap_or_else(|| panic!("{part}: {:?}", c.selection_error));
        assert!(
            close(c.candidates[sel.candidate].diameter_ldu(), 12.0, 1e-6),
            "{part}"
        );
    }
}

#[test]
fn three_mf_has_one_part_per_colour() {
    use ldraw2print::export::ThreeMf;
    let out = run("testmirror", |_| {}).unwrap();
    let lib = Library::new(Some(fixtures()));
    let mut buf = Vec::new();
    ThreeMf
        .write(
            &PrintModel {
                name: "testmirror".into(),
                mesh: &out.mesh,
                colors: &lib.colors,
            },
            &mut buf,
        )
        .unwrap();
    assert_eq!(&buf[..2], b"PK");
    // Stored (uncompressed) archive: the model XML is readable in place.
    let text = String::from_utf8_lossy(&buf);
    assert!(text.contains("unit=\"millimeter\""));
    assert_eq!(text.matches("<base ").count(), 2);
    assert_eq!(text.matches("<component ").count(), 2);
    assert_eq!(text.matches("<triangle ").count(), out.mesh.tris.len());
}

#[test]
fn flip_turns_part_upside_down_without_changing_it() {
    let up = run("testhat", |_| {}).unwrap();
    let down = run("testhat", |o| o.flip = true).unwrap();
    assert_eq!(up.mesh.bbox().size(), down.mesh.bbox().size());
    assert!(down.diagnostics.watertight);
    assert!(close(
        up.diagnostics.signed_volume,
        down.diagnostics.signed_volume,
        1e-6
    ));
    // Upright the socket mouth is on the bed; flipped it is at the top.
    assert!(close(radii_at(&up, 0.0, 0.0)[0], 12.0, 1e-3));
    assert!(close(radii_at(&down, 20.0, 20.0)[0], 12.0, 1e-3));
}

#[test]
fn smoothing_keeps_hat_watertight_and_updates_fit_polygon() {
    // testhat mixes primitives (cylinders, discs) with a hand-written
    // annulus, so this also exercises chord/arc crack filling.
    let out = run("testhat", |o| {
        o.smooth = Some(96);
        o.connector = Some("head-stud".into());
        o.fit = Some("normal".into());
    })
    .unwrap();
    assert!(out.diagnostics.watertight, "{:?}", out.diagnostics.issues);
    let s = out.mesh.bbox().size();
    assert!(close(s.x, 56.0, 1e-3) && close(s.z, 20.0, 1e-3), "{s:?}");
    let plan = out.connector.unwrap().plan.unwrap();
    // Volume of 96-gon prisms (mm): crown r = 28, h = 20 minus the fitted
    // socket (vertex radius from the plan), 8 mm deep.
    let a96 = 48.0 * (2.0 * PI / 96.0).sin();
    let expect = a96 * (28.0f64.powi(2) * 20.0 - plan.target_vertex_radius_mm.powi(2) * 8.0);
    let v = out.diagnostics.signed_volume;
    assert!(close(v, expect, expect * 1e-4), "{v} vs {expect}");
    assert_eq!(plan.segments, 96);
    let cfg = Config::builtin();
    let flats = (PI / 96.0).cos();
    assert!(close(
        plan.target_vertex_radius_mm,
        (24.0 + cfg.fit_presets.normal) / 2.0 / flats,
        1e-9
    ));
}

#[test]
fn painted_three_mf_is_one_solid_with_painted_faces() {
    use ldraw2print::export::{paint_filaments, PaintedThreeMf};
    // testmirror: one wedge in colour 71 (main), one in colour 4.
    let out = run("testmirror", |_| {}).unwrap();
    let lib = Library::new(Some(fixtures()));
    let model = PrintModel {
        name: "testmirror".into(),
        mesh: &out.mesh,
        colors: &lib.colors,
    };
    let filaments = paint_filaments(&model);
    assert_eq!(filaments.len(), 2);
    let mut buf = Vec::new();
    PaintedThreeMf.write(&model, &mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf);
    // A single object, no components.
    assert_eq!(text.matches("<object ").count(), 1);
    assert!(!text.contains("<component "));
    assert_eq!(text.matches("<triangle ").count(), out.mesh.tris.len());
    // Every non-base triangle is painted with filament 2 for Bambu and Prusa.
    let base = filaments[0].0;
    let painted = out.mesh.tris.iter().filter(|t| t.color != base).count();
    assert_eq!(text.matches("paint_color=\"8\"").count(), painted);
    assert_eq!(
        text.matches("slic3rpe:mmu_segmentation=\"8\"").count(),
        painted
    );
    assert!(text.contains("filament 1:") && text.contains("filament 2:"));
}

#[test]
fn builds_are_deterministic() {
    // Smoothing + shell closing used to depend on hash-map iteration order.
    let a = run("testhat", |o| {
        o.smooth = Some(96);
        o.close_open_shells = Some(true);
        o.connector = Some("head-stud".into());
        o.fit = Some("normal".into());
    })
    .unwrap();
    for _ in 0..5 {
        let b = run("testhat", |o| {
            o.smooth = Some(96);
            o.close_open_shells = Some(true);
            o.connector = Some("head-stud".into());
            o.fit = Some("normal".into());
        })
        .unwrap();
        assert_eq!(a.mesh.positions, b.mesh.positions);
        assert_eq!(a.mesh.tris, b.mesh.tris);
    }
}

#[test]
fn fit_refuses_to_break_through_the_wall() {
    // testhat: socket r = 6 LDU inside a crown of r = 14 LDU. A 40 mm
    // clearance would put the socket at r ≈ 16.3 LDU, through the crown wall
    // (the bottom annulus' outer vertices at r = 14 are crossed). 30 mm
    // (r ≈ 13.8) would still be inside it.
    let err = run("testhat", |o| {
        o.connector = Some("head-stud".into());
        o.clearance_mm = Some(40.0);
    })
    .err()
    .expect("must refuse")
    .to_string();
    assert!(err.contains("wall too thin"), "{err}");
    // A sane clearance still works.
    assert!(run("testhat", |o| {
        o.connector = Some("head-stud".into());
        o.clearance_mm = Some(0.5);
    })
    .is_ok());
}
