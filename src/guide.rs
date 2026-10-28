//! Built-in guide topics (`ldraw2print guide <topic>`), compiled in from
//! docs/guide/*.md and AGENTS.md so the CLI, README and repo stay in sync.

pub struct Topic {
    pub name: &'static str,
    pub summary: &'static str,
    pub text: &'static str,
}

pub const TOPICS: &[Topic] = &[
    Topic {
        name: "workflow",
        summary: "From a part name to a print: search, inspect, calibrate, convert, slice",
        text: include_str!("../docs/guide/workflow.md"),
    },
    Topic {
        name: "connectors",
        summary: "Connector profiles, detection, candidates, manual axes, exit code 4",
        text: include_str!("../docs/guide/connectors.md"),
    },
    Topic {
        name: "fit",
        summary: "Nominal vs measured vs compensation, fit presets, calibration coupons",
        text: include_str!("../docs/guide/fit.md"),
    },
    Topic {
        name: "smoothing",
        summary: "--smooth: rounder curves, printed patterns, what can't be smoothed",
        text: include_str!("../docs/guide/smoothing.md"),
    },
    Topic {
        name: "multicolor",
        summary: "Painted 3MF, filament order, colour codes, swap cost",
        text: include_str!("../docs/guide/multicolor.md"),
    },
    Topic {
        name: "repair",
        summary: "Diagnostics, watertight, --close-open-shells and other repairs",
        text: include_str!("../docs/guide/repair.md"),
    },
    Topic {
        name: "orientation",
        summary: "Placement, --flip, supports, which way up to print",
        text: include_str!("../docs/guide/orientation.md"),
    },
    Topic {
        name: "slicers",
        summary: "Bambu Studio, OrcaSlicer, PrusaSlicer, Cura notes",
        text: include_str!("../docs/guide/slicers.md"),
    },
    Topic {
        name: "units",
        summary: "Scale, LDU vs mm, axes, config file locations",
        text: include_str!("../docs/guide/units.md"),
    },
    Topic {
        name: "json",
        summary: "--json output fields for each command",
        text: include_str!("../docs/guide/json.md"),
    },
    Topic {
        name: "agents",
        summary: "Automation playbook: rules, exit codes, recipes, pitfalls (AGENTS.md)",
        text: include_str!("../AGENTS.md"),
    },
];
