/// Embedded, read-only web server for the Map and Status pages. Runs on its own
/// small thread pool and reads snapshots from the shared AIS state. It is meant
/// to sit behind an nginx reverse proxy (see deploy/nginx.conf) which handles
/// TLS, caching and rate limiting; this server only speaks plain HTTP and only
/// answers GET/HEAD.
///
/// All asset URLs in the HTML are relative (no leading slash, no <base> tag) so
/// the whole app works unchanged whether it is served at `/` (direct) or under
/// a subpath like `/ais-harlingen/` (behind nginx, which strips the prefix).
use std::sync::Arc;
use std::thread::Builder;

use tiny_http::{Header, Method, Request, Response, Server};

use crate::state::Shared;

/// A static file baked into the binary.
struct Asset {
    /// Request path the app sees after nginx strips any prefix.
    path: &'static str,
    mime: &'static str,
    body: &'static [u8],
}

const ASSETS: &[Asset] = &[
    Asset {
        path: "/",
        mime: "text/html; charset=utf-8",
        body: include_bytes!("web/assets/index.html"),
    },
    Asset {
        path: "/status",
        mime: "text/html; charset=utf-8",
        body: include_bytes!("web/assets/status.html"),
    },
    Asset {
        path: "/style.css",
        mime: "text/css; charset=utf-8",
        body: include_bytes!("web/assets/style.css"),
    },
    Asset {
        path: "/map.js",
        mime: "text/javascript; charset=utf-8",
        body: include_bytes!("web/assets/map.js"),
    },
    Asset {
        path: "/status.js",
        mime: "text/javascript; charset=utf-8",
        body: include_bytes!("web/assets/status.js"),
    },
    Asset {
        path: "/maplibre-gl.js",
        mime: "text/javascript; charset=utf-8",
        body: include_bytes!("web/assets/maplibre-gl.js"),
    },
    Asset {
        path: "/maplibre-gl.css",
        mime: "text/css; charset=utf-8",
        body: include_bytes!("web/assets/maplibre-gl.css"),
    },
];

/// Number of worker threads accepting requests. Behind an nginx microcache the
/// backend sees almost no traffic, so a handful is plenty.
const WORKERS: usize = 4;

pub fn spawn(bind: &str, state: Shared) {
    let server = match Server::http(bind) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            log::error!("Web server failed to bind {}: {}", bind, e);
            return;
        }
    };
    log::info!("Web server listening on http://{}", bind);

    for i in 0..WORKERS {
        let server = server.clone();
        let state = state.clone();
        Builder::new()
            .name(format!("web-{}", i))
            .spawn(move || {
                for request in server.incoming_requests() {
                    handle(request, &state);
                }
            })
            .expect("failed to spawn web worker");
    }
}

fn handle(request: Request, state: &Shared) {
    // Only GET/HEAD; nginx enforces this too but defend in depth.
    match request.method() {
        Method::Get | Method::Head => {}
        _ => {
            let _ = request.respond(Response::empty(405));
            return;
        }
    }

    // Strip any query string; route on the path only.
    let url = request.url();
    let path = url.split(['?', '#']).next().unwrap_or("/");

    match path {
        "/api/vessels" => {
            let body = {
                let mut s = state.lock().unwrap();
                s.vessels_json().to_string()
            };
            respond_json(request, body);
        }
        "/api/stats" => {
            let body = {
                let mut s = state.lock().unwrap();
                s.stats_json().to_string()
            };
            respond_json(request, body);
        }
        _ => match ASSETS.iter().find(|a| a.path == path) {
            Some(asset) => respond_asset(request, asset),
            None => {
                let _ = request.respond(Response::from_string("Not found").with_status_code(404));
            }
        },
    }
}

fn respond_json(request: Request, body: String) {
    let response = Response::from_string(body)
        .with_header(header("Content-Type", "application/json"))
        // The app data turns over slowly; let nginx microcache it.
        .with_header(header("Cache-Control", "public, max-age=1"));
    let _ = request.respond(response);
}

fn respond_asset(request: Request, asset: &Asset) {
    let response = Response::from_data(asset.body)
        .with_header(header("Content-Type", asset.mime))
        .with_header(header("Cache-Control", "public, max-age=3600"));
    let _ = request.respond(response);
}

fn header(key: &str, value: &str) -> Header {
    Header::from_bytes(key.as_bytes(), value.as_bytes()).unwrap()
}
