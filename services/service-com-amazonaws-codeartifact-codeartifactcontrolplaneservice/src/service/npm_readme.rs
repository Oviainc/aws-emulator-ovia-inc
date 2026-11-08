// SPDX-License-Identifier: CC0-1.0

//! The README of an npm version, as the publish that created it carried it.
//!
//! `GetPackageVersionReadme` "gets the readme file or descriptive text for a package
//! version", and for npm that text travels two ways in one `npm publish`:
//!
//! * inline, as the `readme` member of the version's own metadata document — which is where
//!   the npm CLI puts the contents of the package's README file when it builds the publish
//!   body; and
//! * as a FILE inside the published tarball, `package/README.md` (or any other extension npm
//!   accepts), which is the copy a client that unpacks the distribution reads.
//!
//! The inline member is read first because it is the publisher's own declaration of the
//! text; the tarball is opened only when the document declares none. Either way the result
//! is the bytes that were published, decoded as the UTF-8 they arrived as, so a readme
//! carrying multi-byte characters round-trips unchanged.
//!
//! Read ONCE, at ingest ([`crate::state::PackageVersion::readme`]): a readme fetch then
//! costs a map lookup instead of unpacking a version's whole tarball.

/// The longest readme this plane retains for one version, in bytes.
///
/// The emulator's OWN bound, like every other ceiling in this store: a README is kilobytes
/// in practice, the text is retained caller content, and the file inside a tarball is
/// COMPRESSED on the wire — so what one publish can leave behind needs a declared ceiling of
/// its own. A readme past it is not retained; the version and its assets still are.
const MAX_README_BYTES: usize = 1024 * 1024;

/// The most bytes ONE REQUEST will decompress while looking for READMEs, across every
/// version its publish document declares.
///
/// Two things make this the shape of the bound. A gzip member can expand to far more than it
/// carries, so what is bounded is the DECOMPRESSED stream rather than the compressed one. And
/// a publish document declares many versions, each attaching a tarball, so a per-tarball cap
/// would bound each factor and not their product: the budget is therefore spent across the
/// whole request ([`InflateBudget`]), and a document that exhausts it stores its remaining
/// versions without a readme rather than inflating again.
///
/// The value is a README's own ceiling ([`MAX_README_BYTES`]) with room for the entries npm
/// packs ahead of it, which is what the walk has to read past; a package whose README sits
/// behind more than this simply reports none.
const MAX_DECOMPRESSED_BYTES: u64 = 4 * 1024 * 1024;

/// What one request may still decompress ([`MAX_DECOMPRESSED_BYTES`]).
///
/// Threaded through the versions of one publish document so the inflating is bounded for the
/// REQUEST and not merely per tarball. Created once per request, by the reading of the
/// publish document — which happens before the store is locked, so none of this work is done
/// inside the critical section.
#[derive(Debug)]
pub struct InflateBudget {
    remaining: u64,
}

impl InflateBudget {
    /// The budget one request starts with.
    #[must_use]
    pub fn for_one_request() -> Self {
        Self {
            remaining: MAX_DECOMPRESSED_BYTES,
        }
    }
}

/// The file name, without its extension, npm reads a package's readme from.
const README_STEM: &str = "README";

/// The readme the publish of one npm version carried: the `readme` its metadata document
/// declares, or the `README` file inside its tarball — the tarball only when the document
/// declares none, and only while `budget` has room left.
///
/// `None` when the publish carried neither, when the text is past [`MAX_README_BYTES`], or
/// when this request has spent its decompression budget — each of which is "this version has
/// no readme this emulator retains" rather than a truncation a client could not tell from the
/// published text.
#[must_use]
pub fn published_readme(
    manifest: Option<&str>,
    tarball: &[u8],
    budget: &mut InflateBudget,
) -> Option<String> {
    let declared = manifest
        .and_then(crate::service::npm_manifest::read)
        .and_then(|manifest| manifest.readme);
    declared
        .or_else(|| packaged_readme(tarball, budget))
        .filter(|text| !text.is_empty())
        .filter(|text| {
            let retained = text.len() <= MAX_README_BYTES;
            if !retained {
                ::tracing::warn!(
                    bytes = text.len(),
                    "the published readme is larger than this emulator retains, so the version \
                     is stored without one"
                );
            }
            retained
        })
}

/// The `README` file inside a published npm tarball, or `None` when it carries none — and
/// for a tarball this reading cannot open at all, which is caller content this service
/// stores verbatim either way.
///
/// npm packs a package under one root directory (`package/`), so the README is looked for
/// directly under the root as well as at the top; any extension is accepted, because npm
/// itself accepts `README`, `README.md` and `README.markdown` alike.
///
/// Whatever the walk inflated is SPENT out of `budget`, found readme or not, because the
/// bytes were decompressed either way; a budget with nothing left opens no tarball at all.
/// The walk does not stop at the first entry under a subdirectory: npm writes entries in
/// packing order, so a package that packs `package/lib/…` before `package/README.md` would
/// then report no readme — what bounds the work is the budget, not the shape of the archive.
fn packaged_readme(tarball: &[u8], budget: &mut InflateBudget) -> Option<String> {
    use ::std::io::Read as _;

    if budget.remaining == 0 {
        ::tracing::warn!(
            "this request has decompressed as much as this emulator will, so the version is \
             stored without a readme"
        );
        return None;
    }
    let unframed = ::flate2::read::GzDecoder::new(tarball).take(budget.remaining);
    let mut archive = ::tar::Archive::new(unframed);
    let mut found: Option<String> = None;
    if let Ok(entries) = archive.entries() {
        for entry in entries.flatten() {
            let named = entry
                .path()
                .ok()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default();
            if !is_readme_path(&named) {
                continue;
            }
            let mut text = String::new();
            // Bounded by the ceiling plus one byte, so a file AT the ceiling is retained
            // whole and a larger one is recognisably over it.
            let bounded = u64::try_from(MAX_README_BYTES)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            if entry.take(bounded).read_to_string(&mut text).is_err() {
                // Not UTF-8 text, so not a readme this operation can return.
                continue;
            }
            if !text.is_empty() {
                found = Some(text);
                break;
            }
        }
    }
    budget.remaining = archive.into_inner().limit();
    found
}

/// Whether `path`, as a tarball entry names it, is the package's own README: a file called
/// `README` with any extension, at the top of the archive or directly under its one root
/// directory.
fn is_readme_path(path: &str) -> bool {
    let components: Vec<&str> = path
        .split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect();
    if components.len() > 2 {
        return false;
    }
    let Some(file_name) = components.last() else {
        return false;
    };
    let stem = file_name
        .split_once('.')
        .map_or(*file_name, |(stem, _extension)| stem);
    stem.eq_ignore_ascii_case(README_STEM)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gzip-framed tar carrying one file at `path`.
    fn tarball(path: &str, content: &[u8]) -> Vec<u8> {
        use ::std::io::Write as _;

        let mut builder = ::tar::Builder::new(Vec::new());
        let mut header = ::tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, content)
            .expect("append the entry");
        let packed = builder.into_inner().expect("finish the archive");
        let mut encoder =
            ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
        encoder.write_all(&packed).expect("compress the archive");
        encoder.finish().expect("finish the compression")
    }

    /// One row of the readme table: the case, the document the publish declared, the tarball
    /// it attached, and the text the reading must return.
    type ReadmeRow = (
        &'static str,
        Option<&'static str>,
        Vec<u8>,
        Option<&'static str>,
    );

    /// RULE: the readme a publish carried is the `readme` its document declares, or the
    /// `README` file inside its tarball — decoded as the UTF-8 it was published as.
    #[test]
    fn the_readme_is_read_from_the_document_or_from_the_tarball() {
        // The rows span what the rule generalises over: where the text travelled, the
        // extension the file carried, multi-byte text, and a publish carrying no readme at
        // all.
        let multibyte = "# 概要\nПривет — café ☕";
        let rows: &[ReadmeRow] = &[
            (
                "declared inline by the document",
                Some(r##"{"name":"pkg","version":"1.0.0","readme":"# inline"}"##),
                tarball("package/README.md", b"# packed"),
                Some("# inline"),
            ),
            (
                "only inside the tarball",
                Some(r##"{"name":"pkg","version":"1.0.0"}"##),
                tarball("package/README.md", b"# packed"),
                Some("# packed"),
            ),
            (
                "inside the tarball with no extension",
                None,
                tarball("package/README", b"plain"),
                Some("plain"),
            ),
            (
                "multi-byte text inline",
                Some(r##"{"name":"pkg","readme":"# 概要\nПривет — café ☕"}"##),
                Vec::new(),
                Some(multibyte),
            ),
            (
                "no readme anywhere",
                Some(r##"{"name":"pkg","version":"1.0.0"}"##),
                tarball("package/index.js", b"module.exports = 1;"),
                None,
            ),
            (
                "a tarball this reading cannot open",
                None,
                b"not a tarball at all".to_vec(),
                None,
            ),
        ];
        for (case, manifest, tarball, expected) in rows {
            let mut budget = InflateBudget::for_one_request();
            let read = published_readme(*manifest, tarball, &mut budget);
            assert_eq!(
                read.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: the decompression budget belongs to the REQUEST, not to one tarball: once the
    /// versions of one publish document have spent it, a further tarball is not opened at
    /// all and its version is stored without a readme.
    #[test]
    fn one_request_spends_one_decompression_budget_across_its_versions() {
        // A readme as large as this emulator retains, so one tarball spends a quarter of the
        // budget; the rows then show the budget falling and the walk stopping.
        let large = "x".repeat(MAX_README_BYTES);
        let packed = tarball("package/README.md", large.as_bytes());
        let mut budget = InflateBudget::for_one_request();
        let mut spent_at_least_once = false;
        let mut readmes = 0;
        // More versions than the budget can serve, which is the product a per-tarball cap
        // would not bound.
        for version in 0..16 {
            let before = budget.remaining;
            let read = published_readme(None, &packed, &mut budget);
            if read.is_some() {
                readmes += 1;
            }
            assert!(
                budget.remaining <= before,
                "case \"version {version}\": the budget must never grow, {before} -> {}",
                budget.remaining
            );
            if budget.remaining == 0 {
                spent_at_least_once = true;
                let after_spent = published_readme(None, &packed, &mut budget);
                assert_eq!(
                    after_spent, None,
                    "case \"version {version}\": a spent budget opens no tarball, got \
                     {after_spent:?}"
                );
                break;
            }
        }
        assert!(
            spent_at_least_once,
            "case \"a document of many versions\": expected the shared budget to run out, it \
             still has {} bytes",
            budget.remaining
        );
        assert!(
            readmes > 0 && readmes < 16,
            "case \"a document of many versions\": expected the first versions to get their \
             readme and the rest not to, got {readmes}"
        );
    }

    /// RULE: only a file named `README` (any extension) at the top of the archive or under
    /// its one root directory is the package's readme.
    #[test]
    fn only_the_packages_own_readme_file_is_read() {
        let rows: &[(&str, bool)] = &[
            ("package/README.md", true),
            ("package/readme", true),
            ("README.rst", true),
            ("./package/README.markdown", true),
            ("package/docs/README.md", false),
            ("package/READMEISH.md", false),
            ("package/index.js", false),
        ];
        for (path, expected) in rows {
            let read = is_readme_path(path);
            assert_eq!(
                read, *expected,
                "case {path:?}: expected {expected}, got {read}"
            );
        }
    }
}
