// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of an npm METADATA document, for everything this service reports out of
//! one.
//!
//! CodeArtifact does not invent a package version's metadata: it "extracts the dependencies
//! for a package version from the metadata file for the package format (for example, the
//! package.json file for npm packages)"
//! (`smithy_query(query="doc",shape="ListPackageVersionDependencies")`), and reports the
//! same document's summary, home page, licenses and source-code repository on the version's
//! description. Two readers would let `ListPackageVersionDependencies` and
//! `DescribePackageVersion` disagree about one published document, so there is one.
//!
//! ## Why this does not read `serde_json::Value`
//!
//! A dependency list is reported "in the order they are declared" — the order the document
//! writes its members in — and `serde_json::Map` keeps insertion order only while the build
//! turns on that crate's additive `preserve_order` feature, which this crate's build does
//! not control. The document is therefore read through the STREAMING deserializer, which
//! visits an object's members in document order whatever the map type stores them in, so
//! the order reported here is the declared one by construction.

use ::std::fmt;

use ::serde::de::{self, Deserializer, MapAccess, Visitor};
use ::serde::de::DeserializeSeed as _;

/// The `dependencyType` npm's `dependencies` section carries, from `PackageDependency`'s own
/// documentation ("npm: `regular`, `dev`, `peer`, `optional`").
pub const DEPENDENCY_TYPE_REGULAR: &str = "regular";

/// The `dependencyType` of npm's `devDependencies` section.
pub const DEPENDENCY_TYPE_DEV: &str = "dev";

/// The `dependencyType` of npm's `peerDependencies` section.
pub const DEPENDENCY_TYPE_PEER: &str = "peer";

/// The `dependencyType` of npm's `optionalDependencies` section.
pub const DEPENDENCY_TYPE_OPTIONAL: &str = "optional";

/// One declared dependency of a package version: which section declared it, the package it
/// names, and the version requirement written for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// One of the `dependencyType` values above.
    pub dependency_type: &'static str,
    /// The dependency's name as the document writes it, scope included (`@scope/name`).
    pub package: String,
    /// The requirement the document declares (`^1.0.0`, `~2.3`, `*`).
    pub version_requirement: String,
}

impl Dependency {
    /// The scope the dependency's name carries, WITHOUT its `@` — the `namespace`
    /// coordinate an npm package is addressed by — and the name under it.
    #[must_use]
    pub fn coordinate(&self) -> (Option<String>, String) {
        match self.package.strip_prefix('@').and_then(|scoped| {
            scoped
                .split_once('/')
                .filter(|(scope, name)| !scope.is_empty() && !name.is_empty())
        }) {
            Some((scope, name)) => (Some(scope.to_string()), name.to_string()),
            None => (None, self.package.clone()),
        }
    }
}

/// What an npm metadata document declares, for the members this service reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NpmManifest {
    /// `description`, which is what CodeArtifact reports as a version's `summary`.
    pub description: Option<String>,
    /// `homepage`, reported as `homePage`.
    pub homepage: Option<String>,
    /// The license the document names, from `license` — either the SPDX expression npm
    /// writes today or the `{"type": …}` object the legacy form writes.
    pub license: Option<String>,
    /// The repository URL the document names, from `repository` — either the shorthand
    /// string npm allows or the `{"url": …}` object.
    pub repository_url: Option<String>,
    /// `readme`: the package's README as the publishing client declared it inline, which is
    /// what CodeArtifact reports as the version's readme
    /// ([`crate::service::npm_readme`]).
    pub readme: Option<String>,
    /// Every declared dependency, in the order the document declares them, section by
    /// section in the order the sections appear.
    pub dependencies: Vec<Dependency>,
}

/// Read an npm metadata document, or `None` when it is not a JSON object at all.
///
/// Unknown members are skipped rather than refused: a `package.json` carries whatever its
/// author wrote, and this reads the members CodeArtifact reports.
#[must_use]
pub fn read(document: &str) -> Option<NpmManifest> {
    let mut deserializer = ::serde_json::Deserializer::from_str(document);
    ManifestSeed.deserialize(&mut deserializer).ok()
}

/// The seed that drives the streaming read of one metadata document.
struct ManifestSeed;

impl<'de> de::DeserializeSeed<'de> for ManifestSeed {
    type Value = NpmManifest;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(ManifestVisitor)
    }
}

/// The visitor that walks a metadata document's members IN DOCUMENT ORDER.
struct ManifestVisitor;

impl<'de> Visitor<'de> for ManifestVisitor {
    type Value = NpmManifest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an npm metadata document")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut members: M) -> Result<Self::Value, M::Error> {
        let mut manifest = NpmManifest::default();
        while let Some(member) = members.next_key::<String>()? {
            match member.as_str() {
                "description" => manifest.description = members.next_value::<Option<String>>()?,
                "homepage" => manifest.homepage = members.next_value::<Option<String>>()?,
                "readme" => manifest.readme = members.next_value::<Option<String>>()?,
                // `license` and `repository` are each written either as a string or as an
                // object, so both are read as a value and the member is taken out of it.
                "license" => {
                    manifest.license = text_or_member(members.next_value()?, "type");
                }
                "repository" => {
                    manifest.repository_url = text_or_member(members.next_value()?, "url");
                }
                "dependencies" => {
                    declared(&mut manifest, members.next_value()?, DEPENDENCY_TYPE_REGULAR);
                }
                "devDependencies" => {
                    declared(&mut manifest, members.next_value()?, DEPENDENCY_TYPE_DEV);
                }
                "peerDependencies" => {
                    declared(&mut manifest, members.next_value()?, DEPENDENCY_TYPE_PEER);
                }
                "optionalDependencies" => {
                    declared(&mut manifest, members.next_value()?, DEPENDENCY_TYPE_OPTIONAL);
                }
                _ => {
                    members.next_value::<de::IgnoredAny>()?;
                }
            }
        }
        Ok(manifest)
    }
}

/// One dependency SECTION, read as the ordered list of its members.
///
/// The requirement is read as text; a member whose value is not text declares no
/// requirement this service can report, and is left out rather than reported as something
/// it is not.
#[derive(Debug, Default)]
struct Section(Vec<(String, String)>);

impl<'de> de::Deserialize<'de> for Section {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SectionVisitor;

        impl<'de> Visitor<'de> for SectionVisitor {
            type Value = Section;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an npm dependency section")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut members: M) -> Result<Self::Value, M::Error> {
                let mut declared = Vec::new();
                while let Some(name) = members.next_key::<String>()? {
                    match members.next_value::<::serde_json::Value>()? {
                        ::serde_json::Value::String(requirement) => {
                            declared.push((name, requirement));
                        }
                        _ => continue,
                    }
                }
                Ok(Section(declared))
            }

            // A section written as anything but an object declares nothing.
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(Section::default())
            }
        }

        deserializer.deserialize_map(SectionVisitor)
    }
}

/// Append the dependencies `section` declares, in the order it declares them.
fn declared(manifest: &mut NpmManifest, section: Section, dependency_type: &'static str) {
    manifest
        .dependencies
        .extend(section.0.into_iter().map(|(package, version_requirement)| {
            Dependency {
                dependency_type,
                package,
                version_requirement,
            }
        }));
}

/// A member npm writes either as a string or as an object: the string itself, or the named
/// member of the object.
fn text_or_member(value: ::serde_json::Value, member: &str) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text).filter(|text| !text.is_empty()),
        ::serde_json::Value::Object(fields) => fields
            .get(member)
            .and_then(::serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: every declared dependency is read, with the `dependencyType` of the section
    /// that declared it, IN THE ORDER the document declares it — across sections, in the
    /// order the sections appear.
    #[test]
    fn dependencies_are_read_in_the_order_the_document_declares_them() {
        let document = r#"{
            "name": "@acme/widget",
            "version": "1.0.0",
            "dependencies": {
                "zeta": "^3.0.0",
                "alpha": "~1.2.3",
                "@scope/mid": "2.x"
            },
            "devDependencies": { "tester": "*" },
            "peerDependencies": { "react": ">=17" },
            "optionalDependencies": { "fsevents": "^2" },
            "unknownSection": { "ignored": true }
        }"#;
        let manifest = read(document).expect("the document is an object");
        let read_back: Vec<(&str, &str, &str)> = manifest
            .dependencies
            .iter()
            .map(|dependency| {
                (
                    dependency.dependency_type,
                    dependency.package.as_str(),
                    dependency.version_requirement.as_str(),
                )
            })
            .collect();
        let expected = vec![
            (DEPENDENCY_TYPE_REGULAR, "zeta", "^3.0.0"),
            (DEPENDENCY_TYPE_REGULAR, "alpha", "~1.2.3"),
            (DEPENDENCY_TYPE_REGULAR, "@scope/mid", "2.x"),
            (DEPENDENCY_TYPE_DEV, "tester", "*"),
            (DEPENDENCY_TYPE_PEER, "react", ">=17"),
            (DEPENDENCY_TYPE_OPTIONAL, "fsevents", "^2"),
        ];
        assert_eq!(
            read_back, expected,
            "case \"a document declaring four sections\": expected {expected:?}, got {read_back:?}"
        );
    }

    /// One row of the coordinate table: the case, the dependency's declared name, and the
    /// `(namespace, name)` coordinate it resolves to.
    type CoordinateRow = (&'static str, &'static str, (Option<&'static str>, &'static str));

    /// RULE: a scoped dependency's name resolves to the `(namespace, name)` coordinate an
    /// npm package is addressed by.
    #[test]
    fn a_dependency_name_resolves_to_its_coordinate() {
        let rows: &[CoordinateRow] = &[
            ("a plain name", "left-pad", (None, "left-pad")),
            ("a scoped name", "@acme/widget", (Some("acme"), "widget")),
            ("a name that is only a scope", "@acme", (None, "@acme")),
        ];
        for (case, package, expected) in rows {
            let dependency = Dependency {
                dependency_type: DEPENDENCY_TYPE_REGULAR,
                package: (*package).to_string(),
                version_requirement: "*".to_string(),
            };
            let (namespace, name) = dependency.coordinate();
            assert_eq!(
                (namespace.as_deref(), name.as_str()),
                *expected,
                "case {case:?}: expected {expected:?}"
            );
        }
    }

    /// One row of the reported-members table: the case, the document, and the summary, home
    /// page, license and repository it declares.
    type MemberRow = (
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    );

    /// RULE: the members CodeArtifact reports off a document are read whichever of npm's
    /// two spellings the document uses, and a document that declares none reports none.
    #[test]
    fn the_reported_members_are_read_in_either_spelling() {
        // (case, the document, the summary, the home page, the license, the repository)
        let rows: &[MemberRow] = &[
            (
                "the modern spelling",
                r#"{"description":"a widget","homepage":"https://acme.test",
                    "license":"Apache-2.0","repository":{"type":"git","url":"git+https://g.test/w.git"}}"#,
                Some("a widget"),
                Some("https://acme.test"),
                Some("Apache-2.0"),
                Some("git+https://g.test/w.git"),
            ),
            (
                "the legacy spelling",
                r#"{"license":{"type":"MIT","url":"https://l.test"},
                    "repository":"github:acme/widget"}"#,
                None,
                None,
                Some("MIT"),
                Some("github:acme/widget"),
            ),
            ("a document declaring none of them", r#"{"name":"bare"}"#, None, None, None, None),
            (
                "members written as something else",
                r#"{"description":null,"license":7,"repository":[]}"#,
                None,
                None,
                None,
                None,
            ),
        ];
        for (case, document, summary, home_page, license, repository) in rows {
            let manifest = read(document).expect("the document is an object");
            assert_eq!(
                manifest.description.as_deref(),
                *summary,
                "case {case:?}: expected the summary {summary:?}"
            );
            assert_eq!(
                manifest.homepage.as_deref(),
                *home_page,
                "case {case:?}: expected the home page {home_page:?}"
            );
            assert_eq!(
                manifest.license.as_deref(),
                *license,
                "case {case:?}: expected the license {license:?}"
            );
            assert_eq!(
                manifest.repository_url.as_deref(),
                *repository,
                "case {case:?}: expected the repository {repository:?}"
            );
        }
        assert_eq!(
            read("[]"),
            None,
            "case \"a document that is not an object\": expected nothing to be read"
        );
    }
}
