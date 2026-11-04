//! The remote control page: files built into the program and answered by name, so no request ever
//! forms a path or opens a file (`SRV-20`). The kit's guard sends them with its strict policy.

use appkit_hub::{Page, Pages};
use axum::body::Bytes;

/// The files, by the path a request asks for.
const FILES: [(&str, &[u8], &str); 5] = [
    (
        "/",
        include_bytes!("../ui/index.html"),
        "text/html; charset=utf-8",
    ),
    (
        "/index.html",
        include_bytes!("../ui/index.html"),
        "text/html; charset=utf-8",
    ),
    (
        "/assets/app.js",
        include_bytes!("../ui/app.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "/assets/style.css",
        include_bytes!("../ui/style.css"),
        "text/css; charset=utf-8",
    ),
    (
        "/assets/favicon.svg",
        include_bytes!("../ui/favicon.svg"),
        "image/svg+xml",
    ),
];

/// The page of pult.
pub(crate) struct Remote;

impl Pages for Remote {
    fn page(&self, request_path: &str) -> Option<Page> {
        FILES
            .iter()
            .find(|(path, _, _)| *path == request_path)
            .map(|(_, bytes, content_type)| Page {
                bytes: Bytes::from_static(bytes),
                content_type,
            })
    }

    fn has_index(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SRV-20`.
    const MOST_BYTES: usize = 4 * 1024 * 1024;

    #[test]
    fn only_the_files_of_the_table_are_answered() {
        assert!(Remote.has_index());
        for (path, bytes, _) in FILES {
            let page = Remote.page(path).expect(path);
            assert!(!page.bytes.is_empty(), "{path}");
            assert!(bytes.len() <= MOST_BYTES, "{path}");
        }
        for path in [
            "/assets/",
            "/assets/app.js/",
            "/assets/../hub.json",
            "/assets/..%2fhub.json",
            "/control.secret",
            "/hub.json",
            "/app.js",
            "",
        ] {
            assert!(Remote.page(path).is_none(), "{path}");
        }
    }

    #[test]
    fn the_page_needs_nothing_the_policy_of_the_kit_refuses() {
        let html = std::str::from_utf8(FILES[0].1).expect("text");
        let script = std::str::from_utf8(FILES[2].1).expect("text");
        // Inline, or from elsewhere: `script-src 'self'; style-src 'self'`.
        assert!(!html.contains("<style"), "inline style");
        assert!(!html.contains(" style="), "inline style");
        assert!(!html.contains(" on"), "inline handler");
        assert!(html.matches("<script").count() == 1 && html.contains(r#"src="/assets/app.js""#));
        assert!(!html.contains("http"), "something from elsewhere");
        assert!(!script.contains("http"), "something from elsewhere");
        // Sinks that read a string as markup or code: `require-trusted-types-for 'script'`.
        for sink in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "eval(",
            "document.write",
        ] {
            assert!(!script.contains(sink), "{sink}");
        }
        // The token stays in the tab (`WEB-07`), the code leaves the address first (`WEB-06`).
        assert!(!script.contains("localStorage") && !script.contains("cookie"));
        let taken = script
            .find("const pairingCode = takePairingCode();")
            .expect("taken");
        let first_use = script.find("let token =").expect("a token");
        assert!(taken < first_use);
    }
}
