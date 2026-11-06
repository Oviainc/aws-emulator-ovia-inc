// SPDX-License-Identifier: CC0-1.0

//! Which analyzer an index's field mappings may name.
//!
//! A mapped field names its text analyzer BY NAME, and the cluster resolves that name
//! when it parses the mapping: either the search engine ships the analyzer itself, or
//! the index's own analysis settings define one under that name. A mapping naming
//! anything else cannot be parsed, so it is refused and nothing of it is stored — the
//! rule this module states, for every operation that writes a field mapping.

use super::indexes::SCHEMA_ANALYSIS;
use super::*;

/// The members of a field mapping that name an analyzer BY NAME: a text field's
/// index-time analyzer and the two it may override for a query. All three are read by
/// the engine's one analyzer-valued mapping parameter, which is why one refusal covers
/// them.
pub(super) const MAPPING_ANALYZER_MEMBERS: [&str; 3] =
    ["analyzer", "search_analyzer", "search_quote_analyzer"];

/// The member an index's analysis settings define their named analyzers under: each
/// member of it is one analyzer that index configures, keyed by the name a mapping
/// refers to it by.
pub(super) const ANALYSIS_ANALYZERS: &str = "analyzer";

/// The analyzers an index resolves WITHOUT configuring any: the ones the search engine
/// ships, which every index has.
///
/// Source: the engine's own documented built-in analyzers — the generic ones
/// (`standard`, `simple`, `whitespace`, `stop`, `keyword`, `pattern`, `fingerprint`,
/// `snowball`), `default` for the analyzer an index uses where a field names none, and
/// one per supported language. The model publishes no such list (`IndexSchema` is an
/// unconstrained document, `smithy_query(query="operation",operation="UpdateIndex")`),
/// so the engine's catalog is the source and it is stated here once rather than per
/// operation. Sorted, so a reader can see what it holds.
pub const BUILT_IN_ANALYZERS: &[&str] = &[
    "arabic",
    "armenian",
    "basque",
    "bengali",
    "brazilian",
    "bulgarian",
    "catalan",
    "cjk",
    "czech",
    "danish",
    "default",
    "dutch",
    "english",
    "estonian",
    "fingerprint",
    "finnish",
    "french",
    "galician",
    "german",
    "greek",
    "hindi",
    "hungarian",
    "indonesian",
    "irish",
    "italian",
    "keyword",
    "latvian",
    "lithuanian",
    "norwegian",
    "pattern",
    "persian",
    "portuguese",
    "romanian",
    "russian",
    "simple",
    "snowball",
    "sorani",
    "spanish",
    "standard",
    "stop",
    "swedish",
    "thai",
    "turkish",
    "whitespace",
];

/// Every analyzer NAME `schema`'s field mappings refer to, in sorted order and without
/// repeats.
///
/// Only what is under a `mappings` section is read: an `analyzer` member elsewhere in a
/// schema is an analysis setting's own structure (`analysis.analyzer.<name>` is where one
/// is DEFINED), not a reference to a name. Within that section every nesting level is
/// walked, because a field mapping reaches an analyzer at any depth — an object field's
/// property, a multi-field, a dynamic template's mapping.
///
/// A member carrying something other than a name refers to no analyzer name, so it is
/// left to the rules about what a mapping member may hold. Iterative rather than
/// recursive, so a deeply nested document costs no stack.
pub fn mapping_analyzer_references(schema: &::aws_smithy_types::Document) -> Vec<String> {
    use ::aws_smithy_types::Document;

    // The mapping sections first, wherever they appear — the same reading
    // [`package_references`] makes of the analysis sections.
    let mut mappings: Vec<&Document> = Vec::new();
    let mut pending = vec![schema];
    while let Some(node) = pending.pop() {
        match node {
            Document::Object(members) => {
                for (name, child) in members {
                    if name == SCHEMA_MAPPINGS {
                        mappings.push(child);
                    } else {
                        pending.push(child);
                    }
                }
            }
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut pending = mappings;
    while let Some(node) = pending.pop() {
        match node {
            Document::Object(members) => {
                for (name, child) in members {
                    match child {
                        Document::String(value)
                            if MAPPING_ANALYZER_MEMBERS.contains(&name.as_str()) =>
                        {
                            referenced.insert(value.clone());
                        }
                        other => pending.push(other),
                    }
                }
            }
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }
    referenced.into_iter().collect()
}

/// Every analyzer name `schema`'s analysis settings DEFINE.
///
/// The analysis sections are looked for wherever they appear, for the reason
/// [`SCHEMA_ANALYSIS`] gives: the same settings reach the cluster both directly under
/// the index's settings and under the `index` section inside them. Iterative rather than
/// recursive, so a deeply nested document costs no stack.
pub fn configured_analyzers(schema: &::aws_smithy_types::Document) -> BTreeSet<String> {
    use ::aws_smithy_types::Document;

    let mut configured: BTreeSet<String> = BTreeSet::new();
    let mut pending = vec![schema];
    while let Some(node) = pending.pop() {
        match node {
            Document::Object(members) => {
                for (name, child) in members {
                    if name == SCHEMA_ANALYSIS {
                        if let Some(Document::Object(defined)) =
                            object_member(child, ANALYSIS_ANALYZERS)
                        {
                            configured.extend(defined.keys().cloned());
                        }
                    }
                    pending.push(child);
                }
            }
            Document::Array(elements) => pending.extend(elements.iter()),
            _ => {}
        }
    }
    configured
}

/// The member `name` of `document`, where `document` is a set of members at all.
fn object_member<'a>(
    document: &'a ::aws_smithy_types::Document,
    name: &str,
) -> Option<&'a ::aws_smithy_types::Document> {
    match document {
        ::aws_smithy_types::Document::Object(members) => members.get(name),
        _ => None,
    }
}

/// The analyzer `requested`'s field mappings name that nothing resolves — neither the
/// engine's own catalog ([`BUILT_IN_ANALYZERS`]) nor the analysis settings of any schema
/// in `configuring` — or `None` when every analyzer the mappings name resolves.
///
/// `configuring` is every schema the index's analysis settings come from: the request's
/// own schema, and for a change to an index that already exists the schema that index
/// carries, since an earlier write may have defined the analyzer this one refers to. The
/// LEAST unresolved name is reported, so the same request is always refused for the same
/// analyzer.
pub fn unconfigured_analyzer(
    requested: &::aws_smithy_types::Document,
    configuring: &[&::aws_smithy_types::Document],
) -> Option<String> {
    let referenced = mapping_analyzer_references(requested);
    if referenced.is_empty() {
        return None;
    }
    let mut configured: BTreeSet<String> = BTreeSet::new();
    for schema in configuring {
        configured.extend(configured_analyzers(schema));
    }
    referenced.into_iter().find(|name| {
        !BUILT_IN_ANALYZERS.contains(&name.as_str()) && !configured.contains(name.as_str())
    })
}

/// The `ValidationException` payload for a mapping that names an analyzer the index does
/// not resolve.
///
/// The cluster refuses such a mapping while PARSING it, and the managed layer surfaces
/// the engine's own sentence: the analyzer the mapping named, and that nothing configured
/// it. The name is the one the request carried.
pub fn analyzer_not_configured(name: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "analyzer [{name}] has not been configured in mappings"
        )),
    }
}
