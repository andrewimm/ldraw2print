//! Local preview server. Serves a single-page Three.js viewer (embedded in
//! the binary, including a vendored copy of three.js — no CDN, no Node) and
//! a small JSON API that runs the same [`pipeline::build`] as `convert`.
//!
//! Endpoints:
//! * `GET /`               viewer
//! * `GET /api/info`       profiles, defaults
//! * `GET /api/model?..`   geometry + diagnostics for the given parameters
//! * `GET /api/stl?..`     binary STL for the same parameters

use crate::config::Config;
use crate::export::{preview_geometry, BinaryStl, Exporter, PaintedThreeMf, PrintModel};
use crate::ldraw::{Library, PrimitiveResolution};
use crate::pipeline::{build, BuildOptions, BuildOutput, Placement};
use anyhow::{anyhow, Result};
use serde_json::json;
use std::collections::HashMap;
use tiny_http::{Header, Response, Server};

const INDEX_HTML: &str = include_str!("../assets/preview.html");
const THREE_JS: &[u8] = include_bytes!("../assets/vendor/three.module.min.js");
const ORBIT_JS: &[u8] = include_bytes!("../assets/vendor/OrbitControls.js");

pub struct PreviewServer {
    pub lib: Library,
    pub cfg: Config,
    pub base: BuildOptions,
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn parse_query(url: &str) -> (String, HashMap<String, String>) {
    let (path, q) = url.split_once('?').unwrap_or((url, ""));
    let map = q
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect();
    (path.to_string(), map)
}

/// Apply query parameters on top of the CLI's base options.
pub fn options_from_query(
    base: &BuildOptions,
    q: &HashMap<String, String>,
) -> Result<(BuildOptions, Option<PrimitiveResolution>)> {
    let mut o = base.clone();
    let get = |k: &str| q.get(k).map(|s| s.trim()).filter(|s| !s.is_empty());
    let bool_of = |v: &str| matches!(v, "1" | "true" | "on" | "yes");
    if let Some(v) = get("scale") {
        o.scale = v.parse().map_err(|_| anyhow!("bad scale '{v}'"))?;
    }
    if let Some(v) = get("color") {
        o.color = Some(v.parse().map_err(|_| anyhow!("bad color '{v}'"))?);
    }
    if let Some(v) = q.get("connector") {
        o.connector = Some(v.clone()).filter(|s| !s.is_empty() && s != "none");
    }
    if let Some(v) = q.get("fit") {
        o.fit = Some(v.clone()).filter(|s| !s.is_empty() && s != "none");
    }
    if let Some(v) = q.get("clearance") {
        o.clearance_mm = if v.trim().is_empty() {
            None
        } else {
            Some(
                v.trim()
                    .parse()
                    .map_err(|_| anyhow!("bad clearance '{v}'"))?,
            )
        };
    }
    if let Some(v) = q.get("candidate") {
        o.candidate = if v.trim().is_empty() || v == "auto" {
            None
        } else {
            Some(
                v.trim()
                    .parse()
                    .map_err(|_| anyhow!("bad candidate '{v}'"))?,
            )
        };
    }
    if let Some(v) = get("fixtj") {
        o.fix_t_junctions = Some(bool_of(v));
    }
    if let Some(v) = get("orient") {
        o.orient = Some(bool_of(v));
    }
    if let Some(v) = get("intersections") {
        o.check_intersections = bool_of(v);
    }
    if let Some(v) = q.get("smooth") {
        o.smooth = match v.trim() {
            "" => None,
            s => Some(s.parse().map_err(|_| anyhow!("bad smooth '{s}'"))?),
        };
    }
    if let Some(v) = get("closeshells") {
        o.close_open_shells = Some(bool_of(v));
    }
    if let Some(v) = get("flip") {
        o.flip = bool_of(v);
    }
    if let Some(v) = get("placement") {
        o.placement = match v {
            "ldraw" => Placement::Ldraw,
            _ => Placement::Bed,
        };
    }
    let res = match get("res") {
        Some("high") => Some(PrimitiveResolution::High),
        Some("low") => Some(PrimitiveResolution::Low),
        Some("standard") => Some(PrimitiveResolution::Standard),
        _ => None,
    };
    Ok((o, res))
}

pub fn model_json(out: &BuildOutput, lib: &Library) -> serde_json::Value {
    let geo = preview_geometry(&out.mesh, &out.components, &lib.colors, &out.edges);
    let b = out.mesh.bbox();
    json!({
        "ok": true,
        "name": out.name,
        "description": out.description,
        "mm_per_ldu": out.mm_per_ldu,
        "bbox": { "min": b.min.to_array(), "max": b.max.to_array(), "size": b.size().to_array() },
        "geometry": geo,
        "files": out.files,
        "missing": out.missing.iter().map(|m| json!({"name": m.name, "from": m.referenced_from})).collect::<Vec<_>>(),
        "uncertified_files": out.uncertified_files,
        "warnings": out.warnings,
        "repairs": out.repairs,
        "diagnostics": out.diagnostics,
        "connector": out.connector,
        "source_triangles": out.source_triangles,
    })
}

impl PreviewServer {
    fn build_with(&mut self, q: &HashMap<String, String>) -> Result<BuildOutput> {
        let (opts, res) = options_from_query(&self.base, q)?;
        let prev = self.lib.resolution;
        if let Some(r) = res {
            self.lib.resolution = r;
        }
        let r = build(&mut self.lib, &self.cfg, &opts);
        self.lib.resolution = prev;
        r
    }

    fn info(&self) -> serde_json::Value {
        let profiles: Vec<_> = self
            .cfg
            .connectors
            .iter()
            .map(|(k, p)| json!({"name": k, "description": p.description, "gender": p.gender, "default_fit": p.default_fit}))
            .collect();
        json!({
            "input": self.base.input,
            "scale": self.base.scale,
            "color": self.base.color.unwrap_or(self.cfg.default_color),
            "connector": self.base.connector,
            "fit": self.base.fit,
            "candidate": self.base.candidate,
            "flip": self.base.flip,
            "smooth": self.base.smooth.or(self.cfg.mesh.smooth_segments),
            "fit_presets": self.cfg.fit_presets,
            "profiles": profiles,
            "resolution": self.lib.resolution,
        })
    }

    pub fn serve(mut self, host: &str, port: u16, open: bool) -> Result<()> {
        let server = Server::http((host, port))
            .map_err(|e| anyhow!("cannot listen on {host}:{port}: {e}"))?;
        let url = format!(
            "http://{host}:{}/",
            server
                .server_addr()
                .to_ip()
                .map(|a| a.port())
                .unwrap_or(port)
        );
        println!("Preview of {} at {url}  (Ctrl+C to stop)", self.base.input);
        if open {
            open_browser(&url);
        }
        for req in server.incoming_requests() {
            let (path, q) = parse_query(req.url());
            let ctype = |v: &str| Header::from_bytes("Content-Type", v).unwrap();
            let resp = match path.as_str() {
                "/" | "/index.html" => Response::from_data(INDEX_HTML.as_bytes().to_vec())
                    .with_header(ctype("text/html; charset=utf-8")),
                "/vendor/three.module.min.js" => {
                    Response::from_data(THREE_JS.to_vec()).with_header(ctype("text/javascript"))
                }
                "/vendor/OrbitControls.js" => {
                    Response::from_data(ORBIT_JS.to_vec()).with_header(ctype("text/javascript"))
                }
                "/api/info" => Response::from_data(self.info().to_string().into_bytes())
                    .with_header(ctype("application/json")),
                "/api/model" => {
                    let body = match self.build_with(&q) {
                        Ok(out) => model_json(&out, &self.lib),
                        Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
                    };
                    Response::from_data(body.to_string().into_bytes())
                        .with_header(ctype("application/json"))
                }
                "/api/stl" | "/api/3mf" => match self.build_with(&q) {
                    Ok(out) => {
                        let mut buf = Vec::new();
                        let model = PrintModel {
                            name: out.name.clone(),
                            mesh: &out.mesh,
                            colors: &self.lib.colors,
                        };
                        let (exporter, ext, mime): (&dyn Exporter, &str, &str) =
                            if path == "/api/3mf" {
                                (&PaintedThreeMf, "3mf", "model/3mf")
                            } else {
                                (&BinaryStl, "stl", "model/stl")
                            };
                        match exporter.write(&model, &mut buf) {
                            Ok(()) => {
                                let fname = format!(
                                    "{}.{ext}",
                                    out.name.trim_end_matches(".dat").replace('/', "_")
                                );
                                Response::from_data(buf)
                                    .with_header(ctype(mime))
                                    .with_header(
                                        Header::from_bytes(
                                            "Content-Disposition",
                                            format!("attachment; filename=\"{fname}\""),
                                        )
                                        .unwrap(),
                                    )
                            }
                            Err(e) => Response::from_data(format!("{e:#}").into_bytes())
                                .with_status_code(500),
                        }
                    }
                    Err(e) => {
                        Response::from_data(format!("{e:#}").into_bytes()).with_status_code(400)
                    }
                },
                _ => Response::from_data(b"not found".to_vec()).with_status_code(404),
            };
            let _ = req.respond(resp);
        }
        Ok(())
    }
}

fn open_browser(url: &str) {
    let cmd = if cfg!(target_os = "macos") {
        ("open", vec![url])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", url])
    } else {
        ("xdg-open", vec![url])
    };
    let _ = std::process::Command::new(cmd.0).args(cmd.1).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing() {
        let (p, q) = parse_query("/api/model?scale=5&connector=head-stud&clearance=0.25&x=a%20b+c");
        assert_eq!(p, "/api/model");
        assert_eq!(q["scale"], "5");
        assert_eq!(q["x"], "a b c");
        let base = BuildOptions::new("3901", 5.0);
        let (o, _) = options_from_query(&base, &q).unwrap();
        assert_eq!(o.connector.as_deref(), Some("head-stud"));
        assert_eq!(o.clearance_mm, Some(0.25));
    }
}
