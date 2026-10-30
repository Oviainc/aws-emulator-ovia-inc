// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of "which media type does a static file's extension name", for every
//! emulated data plane that SERVES deployed files.
//!
//! A plane that hands a browser the bytes of a built site has to say what they are: a
//! root document served as `application/octet-stream` is downloaded rather than
//! rendered, and a stylesheet served as text is ignored. The file's own name is all such
//! a plane has to go on, so the reading is extension -> media type, and it lives here so
//! two planes cannot answer one extension differently.
//!
//! This is NOT the same reading as mapping an extension onto a value of some service's
//! MODELLED enumeration (`services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice`
//! answers `FileType` that way): there the answer must be one of the values the model
//! lists, whatever the extension means, while here the answer is the HTTP `Content-Type`
//! a browser needs.
//!
//! The catalogue is deliberately the one a built web app carries. An extension it does
//! not list is served as [`GENERIC_MEDIA_TYPE`], which says only "these are bytes"
//! rather than claiming a type the file is not.

/// What a file whose extension names no media type is served as: the generic binary
/// type, which claims nothing about the content.
pub const GENERIC_MEDIA_TYPE: &str = "application/octet-stream";

/// The media type the extension of `path` names, or [`GENERIC_MEDIA_TYPE`] when it names
/// none.
///
/// The extension is the text after the last `.` of the path's own last segment, read
/// case-insensitively (a file may be named `INDEX.HTML`). A path with no `.` after its
/// last separator has no extension at all.
#[must_use]
pub fn media_type_of_path(path: &str) -> &'static str {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return GENERIC_MEDIA_TYPE;
    };
    media_type_of_extension(&extension.to_ascii_lowercase())
}

/// The media type `extension` (already lowercased, without its `.`) names, or
/// [`GENERIC_MEDIA_TYPE`].
///
/// Every entry is the media type the IANA registry publishes for that file suffix; the
/// set is the one a built web app deploys — markup, styles, scripts, data, images,
/// fonts, and the two binaries a site commonly carries.
#[must_use]
pub fn media_type_of_extension(extension: &str) -> &'static str {
    match extension {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" | "map" | "webmanifest" => "application/json",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/vnd.microsoft.icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => GENERIC_MEDIA_TYPE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paths_extension_names_its_media_type() {
        // The rows span what the reading generalises over: a root document, a nested
        // asset, an upper-case extension, a dotted file name, a path whose directories
        // carry dots, an extension nothing publishes, and a file with no extension.
        let cases: &[(&str, &str)] = &[
            ("index.html", "text/html"),
            ("assets/styles/main.css", "text/css"),
            ("INDEX.HTM", "text/html"),
            ("app.min.js", "text/javascript"),
            ("v1.2/data.json", "application/json"),
            ("fonts/inter.woff2", "font/woff2"),
            ("download.bin", GENERIC_MEDIA_TYPE),
            ("LICENSE", GENERIC_MEDIA_TYPE),
            ("archive.v2/README", GENERIC_MEDIA_TYPE),
            ("", GENERIC_MEDIA_TYPE),
        ];
        for (path, expected) in cases {
            let answered = media_type_of_path(path);
            assert_eq!(
                answered, *expected,
                "case {path:?}: expected {expected:?}, got {answered:?}"
            );
        }
    }
}
