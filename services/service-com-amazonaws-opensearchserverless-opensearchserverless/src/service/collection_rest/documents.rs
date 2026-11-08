// SPDX-License-Identifier: CC0-1.0

//! The DOCUMENTS of an index, over the collection's own REST endpoint: the write a client
//! indexes a document with, the read of one by its id, the delete, and the bulk write
//! several of them arrive in.
//!
//! Every write lands on the `IndexRecord` the collection holds
//! ([`crate::state::IndexRecord::write_document`]), so a document is readable, searchable
//! and countable the moment the write is answered, and a document a read reports is the
//! one a write stored.

use super::*;

/// The engine error type for a body that is not a document the index can map.
const MAPPER_PARSING_EXCEPTION: &str = "mapper_parsing_exception";

/// The engine error type for an update of a document the index does not hold.
const DOCUMENT_MISSING_EXCEPTION: &str = "document_missing_exception";

/// The `_primary_term` every write of a collection carries.
///
/// A collection is serverless: nothing elects a new primary for a caller to observe, so
/// the term a write reports is the first one.
const PRIMARY_TERM: i64 = 1;

/// The most actions this plane reads out of ONE bulk body.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: a bulk body is a caller-chosen number of
/// writes in one request, so the number of writes the request performs — and the size of
/// the answer it builds — is bounded here as well as by
/// [`MAX_REQUEST_BODY_BYTES`](super::MAX_REQUEST_BODY_BYTES).
pub(super) const MAX_BULK_ACTIONS: usize = 1_000;

/// Serve a request addressed to one document: `PUT`/`POST` write it, `GET` reports it,
/// `HEAD` answers whether it is there, `DELETE` removes it.
pub(super) fn document_request(
    collection: &mut CollectionRecord,
    request: &Request<'_>,
    index_name: &str,
    id: &str,
) -> RtResponse {
    match request.method {
        "PUT" | "POST" => write(
            collection,
            request,
            index_name,
            Some(id),
            requested_op_type(request),
        ),
        "GET" => read(collection, index_name, id),
        "HEAD" => head(collection, index_name, id),
        "DELETE" => delete(collection, index_name, id),
        _ => wrong_method(request, "DELETE, GET, HEAD, POST, PUT"),
    }
}

/// What `op_type` asks of a `_doc` write: `create` is the one value that forbids a
/// replacement, and every other (including none) leaves the write an upsert.
fn requested_op_type(request: &Request<'_>) -> Creates {
    match request.parameter("op_type") {
        Some("create") => Creates::Only,
        _ => Creates::Upsert,
    }
}

/// `PUT|POST /{index}/_doc[/{id}]` — index one document.
///
/// A write that names no id is answered with the id the service minted for it, which is
/// what `POST /{index}/_doc` is for.
pub(super) fn write(
    collection: &mut CollectionRecord,
    request: &Request<'_>,
    index_name: &str,
    id: Option<&str>,
    creates: Creates,
) -> RtResponse {
    // A write to an id names it in the path, so `PUT` is allowed there; a write that lets
    // the service mint the id is a `POST` to the collection of documents.
    let addressed = id.is_some();
    if !(request.method == "POST" || (request.method == "PUT" && addressed)) {
        return wrong_method(request, if addressed { "POST, PUT" } else { "POST" });
    }
    let source = match request.json() {
        Ok(Some(source)) => source,
        Ok(None) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: a document write carries a document body",
            )
        }
        Err(refusal) => return *refusal,
    };
    match store_document(collection, index_name, id, source, creates) {
        Ok(stored) => answer_write(index_name, &stored),
        Err(refusal) => refusal.rendered(),
    }
}

/// `GET /{index}/_doc/{id}` — the document, as the index holds it.
fn read(collection: &CollectionRecord, index_name: &str, id: &str) -> RtResponse {
    let Some(index) = collection.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    match index.documents.get(id) {
        Some(held) => ok(&json!({
            "_index": index_name,
            "_id": id,
            "_version": held.version,
            "_seq_no": held.seq_no,
            "_primary_term": PRIMARY_TERM,
            "found": true,
            "_source": json_from_document(&held.source),
        })),
        None => rendered(
            404,
            &json!({ "_index": index_name, "_id": id, "found": false }),
        ),
    }
}

/// `HEAD /{index}/_doc/{id}` — whether the index holds the document, with no body.
fn head(collection: &CollectionRecord, index_name: &str, id: &str) -> RtResponse {
    let held = collection
        .indexes
        .get(index_name)
        .is_some_and(|index| index.documents.contains_key(id));
    status_only(if held { 200 } else { 404 })
}

/// `DELETE /{index}/_doc/{id}` — remove the document the id names.
fn delete(collection: &mut CollectionRecord, index_name: &str, id: &str) -> RtResponse {
    match remove_document(collection, index_name, id) {
        Ok(answer) => answer,
        Err(refusal) => refusal.rendered(),
    }
}

/// The delete of one document, as both the document route and a bulk action perform it:
/// the answer the engine renders for it, or the refusal of a request that addressed no
/// index.
fn remove_document(
    collection: &mut CollectionRecord,
    index_name: &str,
    id: &str,
) -> Result<RtResponse, Refusal> {
    let Some(index) = collection.indexes.get_mut(index_name) else {
        return Err(Refusal::index_not_found(index_name));
    };
    let removed = index.documents.remove(id);
    let seq_no = index.take_seq_no();
    // A delete is a write of the id: it advances the version the id is at and takes the
    // next sequence number, which is what lets a client order a delete against the write
    // before it.
    let version = removed
        .as_ref()
        .map_or(1, |held| held.version.saturating_add(1));
    let status = if removed.is_some() { 200 } else { 404 };
    Ok(rendered(
        status,
        &json!({
            "_index": index_name,
            "_id": id,
            "_version": version,
            "result": if removed.is_some() { "deleted" } else { "not_found" },
            "_shards": shards(),
            "_seq_no": seq_no,
            "_primary_term": PRIMARY_TERM,
        }),
    ))
}

/// One document stored by a write: the id it is under (minted when the request named
/// none), the record the index now holds, and whether the write created it.
struct Stored {
    id: String,
    record: state::DocumentRecord,
    created: bool,
}

/// A refusal of one write, rendered either as the whole answer (a single-document write)
/// or as one item of a bulk answer.
struct Refusal {
    status: u16,
    error_type: &'static str,
    reason: String,
    /// The index the refusal is about, where the error names one.
    index: Option<String>,
}

impl Refusal {
    /// The refusal for an index the collection does not hold.
    fn index_not_found(index_name: &str) -> Self {
        Self {
            status: 404,
            error_type: INDEX_NOT_FOUND_EXCEPTION,
            reason: format!("no such index [{index_name}]"),
            index: Some(index_name.to_string()),
        }
    }

    /// This refusal as the whole answer to one request.
    fn rendered(&self) -> RtResponse {
        match &self.index {
            Some(index) => indexed_error(self.status, self.error_type, &self.reason, index, None),
            None => error(self.status, self.error_type, &self.reason),
        }
    }

    /// This refusal as the `error` member of one bulk item.
    fn item(&self) -> Value {
        json!({ "type": self.error_type, "reason": self.reason })
    }
}

/// Store one document in `index_name`, as both the document routes and a bulk action do
/// it — so a write performed either way lands the same record under the same rules.
///
/// The index is CREATED when the collection does not hold it yet, which is what the
/// engine's own `action.auto_create_index` default does for a write to an unknown index;
/// that create is held to the same per-collection index bound a `PUT /{index}` is.
fn store_document(
    collection: &mut CollectionRecord,
    index_name: &str,
    id: Option<&str>,
    source: Value,
    creates: Creates,
) -> Result<Stored, Refusal> {
    if !source.is_object() {
        return Err(Refusal {
            status: 400,
            error_type: MAPPER_PARSING_EXCEPTION,
            reason: "failed to parse: expected the document source to be an object".to_string(),
            index: Some(index_name.to_string()),
        });
    }
    let stored = stored_source(&source, index_name)?;
    if !collection.indexes.contains_key(index_name) {
        if state::validate_index_name(index_name).is_err() {
            return Err(Refusal {
                status: 400,
                error_type: INVALID_INDEX_NAME_EXCEPTION,
                reason: format!("Invalid index name [{index_name}]"),
                index: Some(index_name.to_string()),
            });
        }
        if collection.indexes.len() >= state::MAX_INDEXES_PER_COLLECTION {
            return Err(Refusal {
                status: 400,
                error_type: ILLEGAL_ARGUMENT_EXCEPTION,
                reason: format!(
                    "no more than {} indexes can exist in one collection",
                    state::MAX_INDEXES_PER_COLLECTION
                ),
                index: Some(index_name.to_string()),
            });
        }
        collection.indexes.insert(
            index_name.to_string(),
            state::IndexRecord::new(None, state::now_epoch_millis()),
        );
    }
    let minted = match id {
        Some(id) => id.to_string(),
        // Minted only for a write that named no id, so the common write does not draw
        // bytes it discards.
        None => state::mint_document_id(),
    };
    let id = minted;
    let Some(index) = collection.indexes.get_mut(index_name) else {
        // Unreachable: the index is present or was just inserted, under this one lock.
        return Err(Refusal::index_not_found(index_name));
    };
    let held = index.documents.get(&id).cloned();
    if let (Creates::Only, Some(held)) = (creates, &held) {
        return Err(Refusal {
            status: 409,
            error_type: VERSION_CONFLICT_ENGINE_EXCEPTION,
            reason: format!(
                "[{id}]: version conflict, document already exists (current version [{}])",
                held.version
            ),
            index: Some(index_name.to_string()),
        });
    }
    if held.is_none() && index.documents.len() >= state::MAX_DOCUMENTS_PER_INDEX {
        return Err(Refusal {
            status: 400,
            error_type: ILLEGAL_ARGUMENT_EXCEPTION,
            reason: format!(
                "no more than {} documents can exist in one index",
                state::MAX_DOCUMENTS_PER_INDEX
            ),
            index: Some(index_name.to_string()),
        });
    }
    let record = index.write_document(&id, stored);
    Ok(Stored {
        id,
        record,
        created: held.is_none(),
    })
}

/// The stored form of a document source, refused when it is past the bounds this emulator
/// holds one under ([`state::MAX_DOCUMENT_DEPTH`], [`state::MAX_DOCUMENT_SOURCE_NODES`]).
fn stored_source(
    source: &Value,
    index_name: &str,
) -> Result<::aws_smithy_types::Document, Refusal> {
    let refused = || Refusal {
        status: 400,
        error_type: ILLEGAL_ARGUMENT_EXCEPTION,
        reason: format!(
            "a document must not nest deeper than {} levels or carry more than {} values",
            state::MAX_DOCUMENT_DEPTH,
            state::MAX_DOCUMENT_SOURCE_NODES
        ),
        index: Some(index_name.to_string()),
    };
    let document = state::document_from_json(source).ok_or_else(refused)?;
    if !state::document_within_bounds(&document, state::MAX_DOCUMENT_SOURCE_NODES) {
        return Err(refused());
    }
    Ok(document)
}

/// What a write answers: the id, the version it is now at, the sequence number the write
/// took, and whether it created the document or replaced one.
fn answer_write(index_name: &str, stored: &Stored) -> RtResponse {
    rendered(
        if stored.created { 201 } else { 200 },
        &write_result(index_name, stored),
    )
}

/// The members a write reports, shared by the single answer and the bulk item.
fn write_result(index_name: &str, stored: &Stored) -> Value {
    json!({
        "_index": index_name,
        "_id": stored.id,
        "_version": stored.record.version,
        "result": if stored.created { "created" } else { "updated" },
        "_shards": shards(),
        "_seq_no": stored.record.seq_no,
        "_primary_term": PRIMARY_TERM,
    })
}

/// `POST /_bulk` and `POST /{index}/_bulk` — the actions an NDJSON body carries, performed
/// in the order they were written.
///
/// Each action is answered on its own, as the bulk API does: one action's refusal is
/// reported in its own item and does not stop the ones after it, and `errors` says whether
/// any item was refused.
pub(super) fn bulk(
    collection: &mut CollectionRecord,
    request: &Request<'_>,
    default_index: Option<&str>,
) -> RtResponse {
    if !matches!(request.method, "POST" | "PUT") {
        return wrong_method(request, "POST, PUT");
    }
    let mut lines = request
        .body
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace));
    let mut items: Vec<Value> = Vec::new();
    let mut errors = false;
    while let Some(line) = lines.next() {
        if items.len() >= MAX_BULK_ACTIONS {
            return error(
                400,
                ILLEGAL_ARGUMENT_EXCEPTION,
                &format!("no more than {MAX_BULK_ACTIONS} actions can be sent in one bulk request"),
            );
        }
        let action: Value = match ::serde_json::from_slice(line) {
            Ok(action) => action,
            Err(failed) => {
                return error(
                    400,
                    PARSE_EXCEPTION,
                    &format!("failed to parse bulk request: {failed}"),
                )
            }
        };
        let Some((name, metadata)) = action
            .as_object()
            .and_then(|members| members.iter().next())
            .map(|(name, metadata)| (name.clone(), metadata.clone()))
        else {
            return error(
                400,
                ILLEGAL_ARGUMENT_EXCEPTION,
                "malformed action/metadata line, expected an action naming object",
            );
        };
        // Every action but a delete is followed by the source line it acts with.
        let source = if name == "delete" {
            None
        } else {
            match lines.next().map(::serde_json::from_slice::<Value>) {
                Some(Ok(source)) => Some(source),
                Some(Err(failed)) => {
                    return error(
                        400,
                        PARSE_EXCEPTION,
                        &format!("failed to parse bulk request: {failed}"),
                    )
                }
                None => {
                    return error(
                        400,
                        ILLEGAL_ARGUMENT_EXCEPTION,
                        &format!("the action [{name}] is missing its source line"),
                    )
                }
            }
        };
        let item = bulk_action(collection, default_index, &name, &metadata, source);
        errors |= item.get("error").is_some() || item.get("status").and_then(Value::as_u64).is_some_and(|status| status >= 300);
        items.push(json!({ name: item }));
    }
    ok(&json!({
        "took": request.took(),
        "errors": errors,
        "items": items,
    }))
}

/// One action of a bulk body, as the item the answer reports for it.
fn bulk_action(
    collection: &mut CollectionRecord,
    default_index: Option<&str>,
    name: &str,
    metadata: &Value,
    source: Option<Value>,
) -> Value {
    let named_index = metadata.get("_index").and_then(Value::as_str);
    let Some(index_name) = named_index.or(default_index).map(ToString::to_string) else {
        return bulk_item_error(
            "",
            metadata.get("_id").and_then(Value::as_str),
            &Refusal {
                status: 400,
                error_type: ILLEGAL_ARGUMENT_EXCEPTION,
                reason: "no index name was named in the action or the request path".to_string(),
                index: None,
            },
        );
    };
    let id = metadata.get("_id").and_then(Value::as_str);
    match (name, source) {
        ("index", Some(source)) => bulk_write(collection, &index_name, id, source, Creates::Upsert),
        ("create", Some(source)) => bulk_write(collection, &index_name, id, source, Creates::Only),
        ("update", Some(source)) => bulk_update(collection, &index_name, id, &source),
        ("delete", None) => bulk_delete(collection, &index_name, id),
        _ => bulk_item_error(
            &index_name,
            id,
            &Refusal {
                status: 400,
                error_type: ILLEGAL_ARGUMENT_EXCEPTION,
                reason: format!("the action [{name}] is not one this collection serves"),
                index: Some(index_name.clone()),
            },
        ),
    }
}

/// A bulk `index` or `create` action.
fn bulk_write(
    collection: &mut CollectionRecord,
    index_name: &str,
    id: Option<&str>,
    source: Value,
    creates: Creates,
) -> Value {
    match store_document(collection, index_name, id, source, creates) {
        Ok(stored) => {
            let mut item = write_result(index_name, &stored);
            if let Value::Object(members) = &mut item {
                members.insert(
                    "status".to_string(),
                    json!(if stored.created { 201 } else { 200 }),
                );
            }
            item
        }
        Err(refusal) => bulk_item_error(index_name, id, &refusal),
    }
}

/// A bulk `update` action: the `doc` it carries is merged into the document the id names,
/// which is what an update is as against a write that replaces the whole source.
fn bulk_update(
    collection: &mut CollectionRecord,
    index_name: &str,
    id: Option<&str>,
    source: &Value,
) -> Value {
    let Some(id) = id else {
        return bulk_item_error(
            index_name,
            None,
            &Refusal {
                status: 400,
                error_type: ILLEGAL_ARGUMENT_EXCEPTION,
                reason: "an update action names the document it updates".to_string(),
                index: Some(index_name.to_string()),
            },
        );
    };
    let held = collection
        .indexes
        .get(index_name)
        .and_then(|index| index.documents.get(id))
        .map(|held| json_from_document(&held.source));
    let Some(Value::Object(mut merged)) = held else {
        return bulk_item_error(
            index_name,
            Some(id),
            &Refusal {
                status: 404,
                error_type: DOCUMENT_MISSING_EXCEPTION,
                reason: format!("[{id}]: document missing"),
                index: Some(index_name.to_string()),
            },
        );
    };
    let Some(Value::Object(written)) = source.get("doc").cloned() else {
        return bulk_item_error(
            index_name,
            Some(id),
            &Refusal {
                status: 400,
                error_type: ILLEGAL_ARGUMENT_EXCEPTION,
                reason: "an update action carries the members to merge under [doc]".to_string(),
                index: Some(index_name.to_string()),
            },
        );
    };
    merged.extend(written);
    bulk_write(
        collection,
        index_name,
        Some(id),
        Value::Object(merged),
        Creates::Upsert,
    )
}

/// A bulk `delete` action.
fn bulk_delete(
    collection: &mut CollectionRecord,
    index_name: &str,
    id: Option<&str>,
) -> Value {
    let Some(id) = id else {
        return bulk_item_error(
            index_name,
            None,
            &Refusal {
                status: 400,
                error_type: ILLEGAL_ARGUMENT_EXCEPTION,
                reason: "a delete action names the document it deletes".to_string(),
                index: Some(index_name.to_string()),
            },
        );
    };
    let Some(index) = collection.indexes.get_mut(index_name) else {
        return bulk_item_error(index_name, Some(id), &Refusal::index_not_found(index_name));
    };
    let removed = index.documents.remove(id);
    let seq_no = index.take_seq_no();
    let version = removed
        .as_ref()
        .map_or(1, |held| held.version.saturating_add(1));
    json!({
        "_index": index_name,
        "_id": id,
        "_version": version,
        "result": if removed.is_some() { "deleted" } else { "not_found" },
        "_shards": shards(),
        "_seq_no": seq_no,
        "_primary_term": PRIMARY_TERM,
        "status": if removed.is_some() { 200 } else { 404 },
    })
}

/// One bulk item reporting a refused action.
fn bulk_item_error(index_name: &str, id: Option<&str>, refusal: &Refusal) -> Value {
    json!({
        "_index": index_name,
        "_id": id,
        "status": refusal.status,
        "error": refusal.item(),
    })
}
