//! Picks the web frontend that `src/frontend.rs` embeds: `$DPDS_FRONTEND_DIST` if set and not
//! empty (it must contain `index.html`, else the build fails), else `frontend/dist` if it has an
//! `index.html`, else the placeholder page in `placeholder/`.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    println!("cargo:rerun-if-env-changed=DPDS_FRONTEND_DIST");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=placeholder");

    let default_dist = manifest.join("../../frontend/dist");
    // An empty variable counts as unset (`DPDS_FRONTEND_DIST= cargo build`).
    let user_dist = std::env::var_os("DPDS_FRONTEND_DIST").filter(|v| !v.is_empty());
    let (dir, kind) = if let Some(dist) = user_dist {
        let dist = PathBuf::from(dist);
        if !dist.join("index.html").is_file() {
            eprintln!(
                "error: DPDS_FRONTEND_DIST={} has no index.html: point it at a built frontend \
                 (npm run build in frontend/ writes frontend/dist) or unset it",
                dist.display()
            );
            std::process::exit(1);
        }
        (dist, "bundled")
    } else if default_dist.join("index.html").is_file() {
        (default_dist, "bundled")
    } else {
        (manifest.join("placeholder"), "placeholder")
    };
    // Watching a path that does not exist would rerun this script on every build, so the
    // placeholder case watches nothing.
    if kind == "bundled" {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    let dir = std::fs::canonicalize(&dir)
        .unwrap_or_else(|e| panic!("cannot resolve {}: {e}", dir.display()));
    println!("cargo:rustc-env=DPDS_FRONTEND_DIR={}", dir.display());
    println!("cargo:rustc-env=DPDS_FRONTEND_KIND={kind}");
}
