//! The web frontend, embedded at compile time and served next to `/api`.
//!
//! `build.rs` picks the directory (`$DPDS_FRONTEND_DIST`, else `frontend/dist`, else the
//! placeholder page in `placeholder/`). Files are looked up by exact name in the embedded
//! directory, never on disk, so a request path cannot reach anything outside it.

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::Response;
use include_dir::{include_dir, Dir};

static EMBEDDED: Dir<'static> = include_dir!("$DPDS_FRONTEND_DIR");
static PLACEHOLDER: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/placeholder");

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The app loads only its own files, plus the release version file from `raw.githubusercontent.com`
/// (`connect-src`; the browser makes that request, see `frontend/src/common/updateCheck.ts`).
/// Styles may be inline because React and the UI library set `style` attributes. There is no
/// inline or `eval`ed script.
///
/// `'unsafe-inline'` in `style-src` is untested in a browser: React sets styles through the
/// CSSOM, which a CSP does not block, so `style-src 'self'` may work. Check the console for
/// violations before tightening.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                   img-src 'self' data:; font-src 'self'; \
                   connect-src 'self' https://raw.githubusercontent.com; object-src 'none'; \
                   base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

/// What answers every GET and HEAD outside `/api`. Cheap to clone.
#[derive(Clone)]
pub struct Frontend {
    dir: &'static Dir<'static>,
    kind: &'static str,
}

impl Frontend {
    /// What `build.rs` embedded.
    pub fn embedded() -> Frontend {
        Frontend {
            dir: &EMBEDDED,
            kind: env!("DPDS_FRONTEND_KIND"),
        }
    }

    /// The crate's placeholder page, whatever was embedded.
    pub fn placeholder() -> Frontend {
        Frontend {
            dir: &PLACEHOLDER,
            kind: "placeholder",
        }
    }

    /// A bundled frontend served from `dir`, which has an `index.html`.
    pub fn from_dir(dir: &'static Dir<'static>) -> Frontend {
        Frontend {
            dir,
            kind: "bundled",
        }
    }

    /// `"bundled"` or `"placeholder"`.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The answer for a request outside `/api`.
    pub fn respond(&self, method: &Method, path: &str) -> Response {
        if method != Method::GET && method != Method::HEAD {
            let mut r = plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            r.headers_mut()
                .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            return r;
        }
        let name = path.trim_start_matches('/');
        if let Some(file) = self.dir.get_file(name) {
            let cache = if name.starts_with("assets/") {
                Some(IMMUTABLE)
            } else if name == "index.html" {
                Some("no-cache")
            } else {
                None
            };
            return file_response(name, file.contents(), cache);
        }
        // A missing hashed asset is a real error: an HTML page served as a script only
        // produces confusing browser messages.
        if name
            .strip_prefix("assets/")
            .is_some_and(|rest| !rest.is_empty())
        {
            return plain(StatusCode::NOT_FOUND, "not found");
        }
        match self.dir.get_file("index.html") {
            Some(index) => file_response("index.html", index.contents(), Some("no-cache")),
            None => plain(StatusCode::NOT_FOUND, "the frontend has no index.html"),
        }
    }
}

fn file_response(name: &str, body: &'static [u8], cache: Option<&'static str>) -> Response {
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let content_type = if mime.type_() == mime_guess::mime::TEXT {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    };
    let mut r = secured(Response::new(Body::from(body)));
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Some(cache) = cache {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    }
    r
}

fn plain(status: StatusCode, text: &'static str) -> Response {
    let mut r = secured(Response::new(Body::from(text)));
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

fn secured(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    r
}
