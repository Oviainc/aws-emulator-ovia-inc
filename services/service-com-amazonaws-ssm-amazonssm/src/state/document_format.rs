// SPDX-License-Identifier: CC0-1.0

//! Rendering a persisted document's content in the format a read asks for.
//!
//! A document's content is persisted exactly as the create supplied it, in the
//! `DocumentFormat` the create declared. A read may ask for another one — "Returns the
//! document in the specified format. The document format can be either JSON or YAML"
//! (`smithy_query(query="doc",shape="GetDocumentRequest",member="DocumentFormat")`) —
//! so the content a read reports is the persisted content RE-RENDERED in the format the
//! request asked for, and the format the read reports is the format of the content it
//! actually returned.
//!
//! Both renderings go through the one parser this service reads document content with
//! (`yaml_rust2`, whose YAML 1.2 is a superset of JSON), so the document tree a read
//! re-renders is the same tree every other read derives its report from: nothing is
//! re-written by hand and no value is invented along the way.
//!
//! Two kinds of content are reported exactly as persisted, because no re-rendering is
//! defined for them: content held in (or asked for in) the free-text format, which
//! carries no document tree at all ([`super::DOCUMENT_FORMAT_TEXT`]), and content the
//! parser cannot read back as a single document tree.
//!
//! The work a read does here is bounded by what the store already holds: a document's
//! content is admitted only within the documented size limit
//! ([`super::MAX_DOCUMENT_CONTENT`]) and only after its nesting was guarded, so a
//! re-render walks a tree whose size and depth were both bounded when it was persisted,
//! and the JSON rendering carries that depth bound with it.

use super::{DEFAULT_DOCUMENT_FORMAT, DOCUMENT_FORMAT_TEXT, DOCUMENT_FORMAT_YAML};

/// The content a read reports for a document persisted as `stored_format`, and the
/// format that content is in, when the request asks for `requested_format`.
///
/// The formats agreeing is the common case and reports the persisted bytes verbatim, so
/// a document a caller created round-trips byte for byte through a read that asks for
/// the format it was created in (or for none, which is [`DEFAULT_DOCUMENT_FORMAT`]).
pub fn rendered_content(
    content: &str,
    stored_format: &str,
    requested_format: &str,
) -> (String, String) {
    let unchanged = || (content.to_string(), stored_format.to_string());
    if requested_format == stored_format
        || requested_format == DOCUMENT_FORMAT_TEXT
        || stored_format == DOCUMENT_FORMAT_TEXT
    {
        return unchanged();
    }
    let rendered = match requested_format {
        DEFAULT_DOCUMENT_FORMAT => as_json(content),
        DOCUMENT_FORMAT_YAML => as_yaml(content),
        // Every other `DocumentFormat` value is handled above; a value outside the
        // modelled enum is refused before a read reaches here.
        _ => None,
    };
    rendered.map_or_else(unchanged, |rendered| {
        (rendered, requested_format.to_string())
    })
}

/// `content`'s document tree rendered as JSON, or `None` when the content is not one
/// document tree this service can read.
fn as_json(content: &str) -> Option<String> {
    let document = super::load_single_document(content).ok()?;
    let value = super::document_as_json(&document, super::MAX_DOCUMENT_DEPTH)?;
    ::serde_json::to_string_pretty(&value).ok()
}

/// `content`'s document tree rendered as YAML, or `None` when the content is not one
/// document tree this service can read.
fn as_yaml(content: &str) -> Option<String> {
    let document = super::load_single_document(content).ok()?;
    let mut rendered = String::new();
    ::yaml_rust2::emitter::YamlEmitter::new(&mut rendered)
        .dump(&document)
        .ok()?;
    // The emitter opens what it writes with YAML's document-start marker; a document's
    // content is the document itself, which is what the create of a YAML document
    // supplies and what a read of one reports.
    let body = rendered.strip_prefix("---\n").unwrap_or(&rendered);
    Some(body.to_string())
}
