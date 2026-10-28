//! Command-line contract tests: JSON output, exit codes, discovery commands
//! and help text. These are what scripts and agents rely on.

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ldraw")
}

/// Run with the fixture library and no user config.
fn cli(args: &[&str]) -> Output {
    let tmp = std::env::temp_dir();
    Command::new(env!("CARGO_BIN_EXE_ldraw2print"))
        .arg("--ldraw-dir")
        .arg(fixtures())
        .args(args)
        .current_dir(&tmp)
        .env_remove("LDRAWDIR")
        .env(
            "LDRAW2PRINT_CONFIG",
            tmp.join("ldraw2print-test-no-config.toml"),
        )
        .env("XDG_CONFIG_HOME", tmp.join("ldraw2print-test-no-xdg"))
        .output()
        .unwrap()
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}",
            String::from_utf8_lossy(&o.stdout)
        )
    })
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap()
}

#[test]
fn convert_json_reports_output_and_mesh() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hat.stl");
    let o = cli(&[
        "--json",
        "convert",
        "testhat",
        "--connector",
        "head-stud",
        "--fit",
        "normal",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0, "{}", String::from_utf8_lossy(&o.stderr));
    let v = json(&o);
    assert_eq!(v["ok"], true);
    assert_eq!(v["command"], "convert");
    assert_eq!(v["watertight"], true);
    assert_eq!(v["output"]["format"], "stl");
    assert!(v["output"]["bytes"].as_u64().unwrap() > 84);
    assert!((v["size_mm"][0].as_f64().unwrap() - 56.0).abs() < 1e-6);
    assert_eq!(v["connector"]["selected"]["how"], "auto");
    assert_eq!(v["connector"]["fit_applied"]["moved_vertices"], 32);
    assert!(out.exists());
}

#[test]
fn painted_3mf_json_lists_filaments() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("m.3mf");
    let o = cli(&[
        "--json",
        "convert",
        "testmirror",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0);
    let v = json(&o);
    assert_eq!(v["output"]["color_mode"], "paint");
    let f = v["filaments"].as_array().unwrap();
    assert_eq!(f.len(), 2);
    assert_eq!(f[0]["slot"], 1);
    assert_eq!(f[0]["base"], true);
}

#[test]
fn exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.stl");
    let o = out.to_str().unwrap();
    // 4: two equally good sockets → decision needed, JSON error with kind.
    let r = cli(&[
        "--json",
        "convert",
        "testtwosockets",
        "--connector",
        "head-stud",
        "--fit",
        "normal",
        "-o",
        o,
    ]);
    assert_eq!(code(&r), 4);
    let v = json(&r);
    assert_eq!(v["ok"], false);
    assert_eq!(v["kind"], "decision-needed");
    assert_eq!(v["exit_code"], 4);
    // …and resolving it works.
    let r = cli(&[
        "convert",
        "testtwosockets",
        "--connector",
        "head-stud",
        "--fit",
        "normal",
        "--connector-candidate",
        "1",
        "-o",
        o,
    ]);
    assert_eq!(code(&r), 0, "{}", String::from_utf8_lossy(&r.stderr));
    // inspect doesn't fail on an undecided connector.
    let r = cli(&[
        "--json",
        "inspect",
        "testtwosockets",
        "--connector",
        "head-stud",
    ]);
    assert_eq!(code(&r), 0);
    assert!(json(&r)["connector"]["selection_error"].is_string());
    // 5: missing referenced files.
    assert_eq!(code(&cli(&["convert", "testmissing", "-o", o])), 5);
    assert_eq!(
        code(&cli(&[
            "convert",
            "testmissing",
            "--allow-missing",
            "-o",
            o
        ])),
        0
    );
    // 3: --strict on a non-watertight mesh writes nothing.
    let strict_out = dir.path().join("strict.stl");
    assert_eq!(
        code(&cli(&[
            "convert",
            "testnocert",
            "--strict",
            "-o",
            strict_out.to_str().unwrap()
        ])),
        3
    );
    assert!(!strict_out.exists());
    // 2: usage (fit without connector).
    assert_eq!(code(&cli(&["convert", "testhat", "--fit", "tight"])), 2);
    // 1: unknown part.
    let r = cli(&["--json", "convert", "nosuchpart"]);
    assert_eq!(code(&r), 1);
    assert_eq!(json(&r)["kind"], "error");
}

#[test]
fn search_colors_profiles() {
    let r = cli(&["--json", "search", "test", "hat"]);
    assert_eq!(code(&r), 0);
    let v = json(&r);
    let parts: Vec<&str> = v["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["part"].as_str().unwrap())
        .collect();
    assert!(
        parts.contains(&"testhat.dat") && parts.contains(&"testhatraw.dat"),
        "{parts:?}"
    );
    assert!(!parts.contains(&"testbox.dat"));

    let v = json(&cli(&["--json", "colors", "red"]));
    assert!(v["colors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["code"] == 4 && c["hex"] == "#B40000"));
    let v = json(&cli(&["--json", "colors", "71"]));
    assert_eq!(v["colors"].as_array().unwrap().len(), 1);

    let v = json(&cli(&["--json", "profiles"]));
    let names: Vec<&str> = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    for n in ["head-stud", "head-body", "neck", "scarf", "hand-grip"] {
        assert!(names.contains(&n), "{n}");
    }
    let hs = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "head-stud")
        .unwrap();
    assert_eq!(hs["nominal_diameter_mm"], 24.0);
}

#[test]
fn calibrate_and_library_json() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("c.stl");
    let v = json(&cli(&[
        "--json",
        "calibrate",
        "--connector",
        "scarf",
        "-o",
        out.to_str().unwrap(),
    ]));
    assert_eq!(v["coupons"].as_array().unwrap().len(), 3);
    assert!(out.exists() && out.with_extension("txt").exists());
    let v = json(&cli(&["--json", "library"]));
    assert!(v["path"].as_str().unwrap().ends_with("ldraw"));
}

#[test]
fn guide_topics_and_help() {
    let v = json(&cli(&["--json", "guide"]));
    let topics: Vec<String> = v["topics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    for t in &topics {
        let r = cli(&["guide", t]);
        assert_eq!(code(&r), 0);
        assert!(r.stdout.len() > 200, "guide {t} too short");
    }
    assert!(topics.contains(&"agents".to_string()));
    assert_eq!(code(&cli(&["guide", "nonsense"])), 1);

    let top = String::from_utf8(cli(&["--help"]).stdout).unwrap();
    assert!(top.contains("EXIT CODES") && top.contains("guide agents"));
    for cmd in [
        "convert",
        "preview",
        "inspect",
        "calibrate",
        "search",
        "colors",
        "profiles",
    ] {
        let r = cli(&[cmd, "--help"]);
        assert_eq!(code(&r), 0);
        assert!(
            String::from_utf8(r.stdout).unwrap().contains("EXAMPLES"),
            "{cmd} --help has no examples"
        );
    }
}

#[test]
fn verdict_suggestions_and_calibration_status() {
    // testhat: watertight, one head-stud socket → ready, suggests head-stud.
    let v = json(&cli(&["--json", "inspect", "testhat"]));
    assert_eq!(v["verdict"]["status"], "ready");
    let sug = v["connector_suggestions"].as_array().unwrap();
    assert!(
        sug.iter()
            .any(|g| g["profile"] == "head-stud" && g["unique"] == true),
        "{sug:?}"
    );
    assert_eq!(v["main_color"]["source"], "default");
    // testnocert: not watertight → review with a reason.
    let v = json(&cli(&["--json", "inspect", "testnocert"]));
    assert_eq!(v["verdict"]["status"], "review");
    assert!(!v["verdict"]["reasons"].as_array().unwrap().is_empty());
    // No user config in tests → nothing calibrated.
    let v = json(&cli(&["--json", "profiles"]));
    assert!(v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["calibrated"] == false));
    let v = json(&cli(&[
        "--json",
        "inspect",
        "testhat",
        "--connector",
        "head-stud",
        "--color",
        "4",
    ]));
    assert_eq!(v["connector"]["calibrated"], false);
    assert_eq!(v["connector"]["default_fit_source"], "built-in default");
    assert_eq!(v["main_color"]["source"], "--color");
    // Exact colour name first.
    let v = json(&cli(&["--json", "colors", "red"]));
    assert_eq!(v["colors"][0]["code"], 4);
}

#[test]
fn calibrate_record_writes_config_and_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let cfgp = dir.path().join("config.toml");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/ldraw2print.toml"),
        &cfgp,
    )
    .unwrap();
    let c = cfgp.to_str().unwrap();

    // Preset coupon: #2 = normal.
    let r = cli(&[
        "--config",
        c,
        "--json",
        "calibrate",
        "--connector",
        "head-stud",
        "--record",
        "2",
        "--printer",
        "Prusa MK4",
        "--material",
        "PLA",
        "--date",
        "2026-10-27",
    ]);
    assert_eq!(code(&r), 0, "{}", String::from_utf8_lossy(&r.stderr));
    let v = json(&r);
    assert_eq!(v["default_fit"], "normal");
    assert_eq!(v["calibration"]["clearance_mm"], 0.2);
    let text = std::fs::read_to_string(&cfgp).unwrap();
    assert!(
        text.contains("# Dimensions come in three distinct kinds"),
        "comments kept"
    );

    let v = json(&cli(&["--config", c, "--json", "profiles"]));
    let hs = v["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "head-stud")
        .unwrap();
    assert_eq!(hs["calibrated"], true);
    assert_eq!(hs["calibration"]["printer"], "Prusa MK4");

    // Custom sweep: #3 of 0,0.025,0.05,0.075 = 0.05 → used exactly by --fit clearance.
    let r = cli(&[
        "--config",
        c,
        "calibrate",
        "--connector",
        "scarf",
        "--clearances",
        "0,0.025,0.05,0.075",
        "--record",
        "3",
        "--printer",
        "Prusa MK4",
        "--material",
        "PLA",
    ]);
    assert_eq!(code(&r), 0, "{}", String::from_utf8_lossy(&r.stderr));
    let out = dir.path().join("hat.stl");
    let v = json(&cli(&[
        "--config",
        c,
        "--json",
        "convert",
        "testhat",
        "--connector",
        "head-stud",
        "--fit",
        "clearance",
        "-o",
        out.to_str().unwrap(),
    ]));
    assert_eq!(v["connector"]["plan"]["clearance_mm"], 0.2);
    assert_eq!(v["fit_summary"]["calibrated"], true);

    // Out-of-range coupon and missing printer are refused.
    assert_eq!(
        code(&cli(&[
            "--config",
            c,
            "calibrate",
            "--connector",
            "head-stud",
            "--record",
            "9",
            "--printer",
            "x",
            "--material",
            "y"
        ])),
        1
    );
    assert_eq!(
        code(&cli(&[
            "--config",
            c,
            "calibrate",
            "--connector",
            "head-stud",
            "--record",
            "2"
        ])),
        2
    );
}
