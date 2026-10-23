// SPDX-License-Identifier: CC0-1.0

//! The Search Data Format (SDF) document batch `UploadDocuments` accepts.
//!
//! A batch is a list of `add` and `delete` entries, described either in JSON
//! (`application/json`)
//!
//! ```text
//! [ {"type":"add","id":"doc1","fields":{"title":"Hamlet","genre":["drama"]}},
//!   {"type":"delete","id":"doc2"} ]
//! ```
//!
//! or in XML (`application/xml`), the form this endpoint does NOT serve: see
//! [`XML_CONTENT_TYPE`] for why an XML batch is refused with the modelled error
//! rather than parsed.
//!
//! A batch that breaks a documented rule is REFUSED with the message this module
//! builds; the caller renders it as the modelled `DocumentServiceException`
//! (HTTP 400). A batch that is valid but carries something the document service
//! ignores parses with a [`Batch::warnings`] entry, which the response returns as
//! a `DocumentServiceWarning`.

use super::Document;
use std::collections::BTreeMap;

/// The whole batch size limit, from the `UploadDocuments` documentation: "The
/// entire batch cannot exceed 5 MB."
pub const MAX_BATCH_BYTES: usize = 5 * 1024 * 1024;

/// The per-document size limit, from the `UploadDocuments` documentation:
/// "Individual documents cannot contain more than 1 MB of data."
///
/// WHAT IS MEASURED: the document's DATA, as the sentence says — the bytes of its
/// field names and field values, the same quantity in both batch formats. The
/// id and the format's own structural bytes (an entry's braces or its `<add>`
/// element) are not data the document carries, so a batch at this ceiling can be
/// a little larger on the wire; the whole batch is bounded separately by
/// [`MAX_BATCH_BYTES`], which IS measured on the wire bytes.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Document ids are "at least 1 and no more than 128 characters long" (Preparing
/// Your Data, Amazon CloudSearch Developer Guide).
pub const MAX_ID_LEN: usize = 128;

/// A field name "must begin with a letter and be at least 1 and no more than 64
/// characters long" (Preparing Your Data, Amazon CloudSearch Developer Guide).
pub const MAX_FIELD_NAME_LEN: usize = 64;

/// The field name CloudSearch reserves for a document's relevance score, and so
/// refuses as an uploaded field name.
pub const RESERVED_FIELD_NAME: &str = "score";

/// The two batch formats the modelled `ContentType` enum declares.
pub const JSON_CONTENT_TYPE: &str = "application/json";
/// The second modelled batch format, which this endpoint REFUSES with the
/// modelled `DocumentServiceException` instead of parsing (see [`parse`]).
///
/// A faithful parse of the XML form needs a real XML parser, and the only honest
/// source of one is a maintained crate — which this emulator cannot add, because
/// a `Cargo.lock` change cannot pass the merge gate: its squash merge detects
/// conflicts per FILE, and mainline rewrites `Cargo.lock` about every 26 minutes
/// while a review takes about as long, so a lock edit never merges. Reading XML
/// with anything other than an XML parser (a hand-rolled scanner over angle
/// brackets) would answer some batches wrongly with a success status, which is
/// worse than refusing them: the refusal says exactly what this endpoint serves.
/// Lifting the restriction is the whole of the fix once a lock edit can land.
pub const XML_CONTENT_TYPE: &str = "application/xml";

/// One entry of a batch, in the order the batch listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// Insert or replace this document.
    Add(Document),
    /// Remove the document with this id.
    Delete(String),
}

/// The most warnings one response reports.
///
/// A response says what the document service ignored, one message per
/// occurrence, and a 5 MB batch can carry a great many occurrences — so the list
/// is capped and the last entry says the rest were omitted, instead of a small
/// request being amplified into a huge response.
pub const MAX_WARNINGS: usize = 100;

/// A parsed batch: the entries to apply, and the warnings parsing produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Batch {
    /// The entries, in batch order.
    pub entries: Vec<Entry>,
    /// What the document service ignored, one message per occurrence.
    pub warnings: Vec<String>,
}

/// Record one warning, up to [`MAX_WARNINGS`] of them.
fn warn(warnings: &mut Vec<String>, message: String) {
    match warnings.len() {
        length if length < MAX_WARNINGS => warnings.push(message),
        length if length == MAX_WARNINGS => warnings.push(format!(
            "Only the first {MAX_WARNINGS} warnings for this batch are reported; \
             the rest were omitted."
        )),
        _ => {}
    }
}

/// Parse `body` as a batch of the format `content_type` declares.
///
/// `Err` carries the message of the `DocumentServiceException` the request earns.
pub fn parse(content_type: &str, body: &[u8]) -> Result<Batch, String> {
    if body.len() > MAX_BATCH_BYTES {
        return Err(format!(
            "The batch is {} bytes; a batch cannot exceed {MAX_BATCH_BYTES} bytes.",
            body.len()
        ));
    }
    // A header value may carry parameters (`application/json; charset=utf-8`).
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match media_type.as_str() {
        JSON_CONTENT_TYPE => parse_json(body),
        // A MODELLED content type this endpoint cannot serve, so it is answered
        // with the modelled `DocumentServiceException` (HTTP 400) naming the form
        // it does serve — never with a 200 for a batch nothing applied, and never
        // by reading the XML with something that is not an XML parser.
        XML_CONTENT_TYPE => Err(format!(
            "This endpoint accepts a document batch in the {JSON_CONTENT_TYPE} form; \
             the {XML_CONTENT_TYPE} form is not served here."
        )),
        other => Err(format!(
            "Invalid content type '{other}'. The document service accepts \
             {JSON_CONTENT_TYPE} and {XML_CONTENT_TYPE}."
        )),
    }
}

/// Parse the JSON form of a batch.
fn parse_json(body: &[u8]) -> Result<Batch, String> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| format!("The batch is not valid JSON: {e}."))?;
    let entries = value.as_array().ok_or_else(|| {
        "The batch must be a JSON array of add and delete entries.".to_string()
    })?;
    let mut batch = Batch::default();
    for (position, entry) in entries.iter().enumerate() {
        parse_json_entry(position, entry, &mut batch)?;
    }
    Ok(batch)
}

/// Parse one JSON entry into `batch`.
fn parse_json_entry(
    position: usize,
    entry: &serde_json::Value,
    batch: &mut Batch,
) -> Result<(), String> {
    let object = entry
        .as_object()
        .ok_or_else(|| format!("Entry {position} is not a JSON object."))?;
    let entry_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            format!("Entry {position} is missing the required field 'type' ('add' or 'delete').")
        })?;
    let id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("Entry {position} is missing the required field 'id'."))?;
    validate_id(id)?;
    for key in object.keys() {
        if !matches!(key.as_str(), "type" | "id" | "fields") {
            warn(
                &mut batch.warnings,
                format!(
                    "The field '{key}' of entry {position} is not part of the document \
                     service format and was ignored."
                ),
            );
        }
    }
    match entry_type {
        "add" => {
            let fields = object.get("fields").ok_or_else(|| {
                format!("Entry {position} is an add and is missing the required field 'fields'.")
            })?;
            let fields = fields.as_object().ok_or_else(|| {
                format!("The 'fields' member of entry {position} must be a JSON object.")
            })?;
            let mut document = Document {
                id: id.to_string(),
                fields: BTreeMap::new(),
            };
            for (name, value) in fields {
                validate_field_name(name)?;
                let values = json_field_values(id, name, value, &mut batch.warnings)?;
                if values.is_empty() {
                    continue;
                }
                document
                    .fields
                    .entry(name.clone())
                    .or_default()
                    .extend(values);
            }
            if document.fields.is_empty() {
                return Err(format!(
                    "Document '{id}' specifies no field values; an add entry must \
                     carry at least one field."
                ));
            }
            guard_document_size(&document)?;
            batch.entries.push(Entry::Add(document));
        }
        "delete" => {
            if object.contains_key("fields") {
                warn(
                    &mut batch.warnings,
                    format!(
                        "The 'fields' member of the delete entry for document '{id}' was ignored."
                    ),
                );
            }
            batch.entries.push(Entry::Delete(id.to_string()));
        }
        other => {
            return Err(format!(
                "Entry {position} has type '{other}'; the document service accepts \
                 'add' and 'delete'."
            ));
        }
    }
    Ok(())
}

/// Refuse a document larger than [`MAX_DOCUMENT_BYTES`].
///
/// The size is the data the document stores — its field names and their values —
/// measured the same way for both batch formats, so the same document is
/// accepted or refused whichever format uploaded it.
fn guard_document_size(document: &Document) -> Result<(), String> {
    let size: usize = document
        .fields
        .iter()
        .map(|(name, values)| name.len() + values.iter().map(String::len).sum::<usize>())
        .sum();
    if size > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "Document '{}' is {size} bytes; a document cannot exceed \
             {MAX_DOCUMENT_BYTES} bytes.",
            document.id
        ));
    }
    Ok(())
}

/// The string values a JSON field value contributes.
///
/// CloudSearch field types are text, literal, int, double, date and latlon, plus
/// the array form of each, so a value is a string, a number, or an array of them.
/// A null is not a value: it is dropped with a warning, the way the document
/// service ignores an empty value.
fn json_field_values(
    id: &str,
    name: &str,
    value: &serde_json::Value,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    match value {
        serde_json::Value::String(s) => Ok(vec![s.clone()]),
        serde_json::Value::Number(n) => Ok(vec![n.to_string()]),
        serde_json::Value::Null => {
            warn(
                warnings,
                format!(
                    "The value of field '{name}' in document '{id}' is null and was ignored."
                ),
            );
            Ok(Vec::new())
        }
        serde_json::Value::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    serde_json::Value::String(s) => values.push(s.clone()),
                    serde_json::Value::Number(n) => values.push(n.to_string()),
                    serde_json::Value::Null => warn(
                        warnings,
                        format!(
                            "A null value of field '{name}' in document '{id}' was ignored."
                        ),
                    ),
                    _ => return Err(invalid_field_value(id, name)),
                }
            }
            Ok(values)
        }
        _ => Err(invalid_field_value(id, name)),
    }
}

/// The refusal for a field value that is not a string, a number, or an array of them.
fn invalid_field_value(id: &str, name: &str) -> String {
    format!(
        "The value of field '{name}' in document '{id}' must be a string, a number, \
         or an array of strings or numbers."
    )
}

/// Refuse a document id that breaks the documented id rule: 1 to
/// [`MAX_ID_LEN`] characters, each a letter, a digit, or one of `_ - = # ; : / ? @ &`.
pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.chars().count() > MAX_ID_LEN {
        return Err(format!(
            "The document id '{id}' is {} characters long; an id must be between 1 \
             and {MAX_ID_LEN} characters long.",
            id.chars().count()
        ));
    }
    if let Some(bad) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || "_-=#;:/?@&".contains(*c)))
    {
        return Err(format!(
            "The document id '{id}' contains the character '{bad}'; an id can contain \
             only letters, numbers and the characters _ - = # ; : / ? @ &."
        ));
    }
    Ok(())
}

/// Refuse a field name that breaks the documented naming rule: 1 to
/// [`MAX_FIELD_NAME_LEN`] characters, beginning with a lower-case letter, made of
/// `a-z`, `0-9` and `_`, and not the reserved name `score`.
pub fn validate_field_name(name: &str) -> Result<(), String> {
    if name == RESERVED_FIELD_NAME {
        return Err(format!(
            "The field name '{RESERVED_FIELD_NAME}' is reserved and cannot be used \
             as a field name."
        ));
    }
    let valid_length = (1..=MAX_FIELD_NAME_LEN).contains(&name.chars().count());
    let valid_start = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase());
    let valid_chars = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if valid_length && valid_start && valid_chars {
        return Ok(());
    }
    Err(format!(
        "The field name '{name}' is invalid; a field name must begin with a \
         lower-case letter, be between 1 and {MAX_FIELD_NAME_LEN} characters long, \
         and contain only the characters a-z, 0-9 and _."
    ))
}
