//! ldraw2print: convert LDraw parts into 3D-printable meshes, with a focus
//! on 5:1 scale minifigure accessories. See README.md.

pub mod calibrate;
pub mod config;
pub mod connectors;
pub mod det;
pub mod errors;
pub mod export;
pub mod geometry;
pub mod ldraw;
pub mod mesh;
pub mod pipeline;
pub mod preview;
pub mod report;
pub mod smooth;
