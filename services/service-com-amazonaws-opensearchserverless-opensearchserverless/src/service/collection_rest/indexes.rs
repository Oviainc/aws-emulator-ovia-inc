// SPDX-License-Identifier: CC0-1.0

//! An INDEX of a collection, over the collection's own REST endpoint: the create a client
//! must make before a vector knowledge base can name the index, the read of its settings
//! and mappings, the existence check, the mapping write and the delete.
//!
//! The record written and read here is the SAME `IndexRecord` the control plane's
//! `CreateIndex` writes and `GetIndex` reads, held on the collection
//! ([`crate::state::IndexRecord`]), so an index created over REST is an index the control
//! plane reports and the other way round — there is one index store per collection, not
//! one per plane.

use super::*;

/// Serve a request addressed to one index: `PUT` creates it, `GET` reports it, `HEAD`
/// answers whether it is there, `DELETE` removes it.
pub(super) fn index_request(
    collection: &mut CollectionRecord,
    request: &Request<'_>,
    index_name: &str,
) -> RtResponse {
    match request.method {
        "PUT" => create(collection, request, index_name),
        "GET" => read(collection, index_name),
        "HEAD" => head(collection, index_name),
        "DELETE" => delete(collection, index_name),
        _ => wrong_method(request, "DELETE, GET, HEAD, PUT"),
    }
}

/// `PUT /{index}` — create the index, with the settings and mappings the body carries.
///
/// The body is stored as the index's `indexSchema`, which is the member the control plane
/// reports for the same index: a `{"settings": …, "mappings": …}` document is exactly what
/// that member holds, so the two planes describe one index.
fn create(
    collection: &mut CollectionRecord,
    request: &Request<'_>,
    index_name: &str,
) -> RtResponse {
    if let Some(refusal) = refused_index_name(index_name) {
        return refusal;
    }
    let schema = match index_body(request) {
        Ok(schema) => schema,
        Err(refusal) => return *refusal,
    };
    if collection.indexes.contains_key(index_name) {
        let uuid = index_uuid(collection, index_name);
        return indexed_error(
            400,
            RESOURCE_ALREADY_EXISTS_EXCEPTION,
            &format!("index [{index_name}/{uuid}] already exists"),
            index_name,
            Some(&uuid),
        );
    }
    if let Some(refusal) = refused_for_index_count(collection) {
        return refusal;
    }
    collection.indexes.insert(
        index_name.to_string(),
        state::IndexRecord::new(schema, state::now_epoch_millis()),
    );
    ok(&json!({
        "acknowledged": true,
        "shards_acknowledged": true,
        "index": index_name,
    }))
}

/// The index document a create body carries: the whole body, which an index create writes
/// as the index's schema. An empty body creates an index with no settings and no mappings
/// of its own, which is what `PUT /{index}` with no body does.
fn index_body(request: &Request<'_>) -> Result<Option<::aws_smithy_types::Document>, Box<RtResponse>> {
    let Some(body) = request.json()? else {
        return Ok(None);
    };
    if !body.is_object() {
        return Err(Box::new(error(
            400,
            PARSE_EXCEPTION,
            "failed to parse: expected the index body to be an object",
        )));
    }
    document_within(&body, state::MAX_INDEX_SCHEMA_NODES, "the index body").map(Some)
}

/// The refusal for an index name the cluster will not take, or `None` for one it will.
///
/// The DECISION is [`state::validate_index_name`], the one place this crate holds the rule
/// `CreateIndexRequest$indexName` documents ("Index names must be lowercase and can't begin
/// with underscores (_) or hyphens (-)"); only the WORDING is this plane's, because the
/// REST endpoint states it as the engine does rather than as the modelled
/// `ValidationException`.
fn refused_index_name(index_name: &str) -> Option<RtResponse> {
    if state::validate_index_name(index_name).is_ok() {
        return None;
    }
    let reason = if index_name.chars().any(char::is_uppercase) {
        format!("Invalid index name [{index_name}], must be lowercase")
    } else {
        format!("Invalid index name [{index_name}], must not start with '_' or '-'")
    };
    Some(indexed_error(
        400,
        INVALID_INDEX_NAME_EXCEPTION,
        &reason,
        index_name,
        None,
    ))
}

/// The refusal for a collection already holding every index this emulator stores for one,
/// or `None` while there is room for another.
///
/// EMULATOR RESOURCE GUARD, not an AWS limit: see
/// [`state::MAX_INDEXES_PER_COLLECTION`]. It is the same bound the control plane's
/// `CreateIndex` enforces, so the two planes cannot disagree about how many indexes a
/// collection holds.
pub(super) fn refused_for_index_count(collection: &CollectionRecord) -> Option<RtResponse> {
    (collection.indexes.len() >= state::MAX_INDEXES_PER_COLLECTION).then(|| {
        error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "no more than {} indexes can exist in one collection",
                state::MAX_INDEXES_PER_COLLECTION
            ),
        )
    })
}

/// `GET /{index}` — the index's aliases, mappings and settings, keyed by its own name.
fn read(collection: &CollectionRecord, index_name: &str) -> RtResponse {
    let Some(index) = collection.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    ok(&json!({
        index_name: {
            "aliases": json!({}),
            "mappings": mappings(index),
            "settings": settings(collection, index_name, index),
        }
    }))
}

/// `HEAD /{index}` — whether the collection holds the index, with no body either way.
fn head(collection: &CollectionRecord, index_name: &str) -> RtResponse {
    if collection.indexes.contains_key(index_name) {
        return status_only(200);
    }
    status_only(404)
}

/// `DELETE /{index}` — remove the index and the documents it holds.
fn delete(collection: &mut CollectionRecord, index_name: &str) -> RtResponse {
    if collection.indexes.remove(index_name).is_none() {
        return index_not_found(index_name);
    }
    ok(&json!({ "acknowledged": true }))
}

/// `GET|PUT|POST /{index}/_mapping` — the index's field mappings, and the write that adds
/// to them.
pub(super) fn mapping_request(
    collection: &mut CollectionRecord,
    request: &Request<'_>,
    index_name: &str,
) -> RtResponse {
    match request.method {
        "GET" => {
            let Some(index) = collection.indexes.get(index_name) else {
                return index_not_found(index_name);
            };
            ok(&json!({ index_name: { "mappings": mappings(index) } }))
        }
        "PUT" | "POST" => write_mapping(collection, request, index_name),
        _ => wrong_method(request, "GET, POST, PUT"),
    }
}

/// `PUT /{index}/_mapping` — merge the mapping the body carries into the index's own.
///
/// A mapping write ADDS to what the index holds: the members the body names replace the
/// ones of the same name and every other stays, which is what makes a second write of one
/// field leave the rest of the mapping alone.
fn write_mapping(
    collection: &mut CollectionRecord,
    request: &Request<'_>,
    index_name: &str,
) -> RtResponse {
    let body = match request.json() {
        Ok(Some(body)) => body,
        Ok(None) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: a mapping write carries a mapping body",
            )
        }
        Err(refusal) => return *refusal,
    };
    let Value::Object(added) = body else {
        return error(
            400,
            PARSE_EXCEPTION,
            "failed to parse: expected the mapping body to be an object",
        );
    };
    if !collection.indexes.contains_key(index_name) {
        return index_not_found(index_name);
    }
    let mut schema = schema_value(collection, index_name);
    let held = schema
        .as_object_mut()
        .and_then(|members| {
            members
                .entry("mappings")
                .or_insert_with(|| json!({}))
                .as_object_mut()
        });
    let Some(held) = held else {
        return error(
            400,
            ILLEGAL_ARGUMENT_EXCEPTION,
            &format!("the mappings of index [{index_name}] are not an object"),
        );
    };
    for (name, member) in added {
        match (held.get_mut(&name), member) {
            // `properties` is a map of fields, so a write of one field adds to the fields
            // the index already maps rather than replacing all of them.
            (Some(Value::Object(fields)), Value::Object(written)) if name == "properties" => {
                fields.extend(written);
            }
            (_, member) => {
                held.insert(name, member);
            }
        }
    }
    let written = match document_within(&schema, state::MAX_INDEX_SCHEMA_NODES, "the mapping body") {
        Ok(written) => written,
        Err(refusal) => return *refusal,
    };
    match collection.indexes.get_mut(index_name) {
        Some(index) => index.index_schema = Some(written),
        // Unreachable: the index was found above and this call holds the store.
        None => return index_not_found(index_name),
    }
    ok(&json!({ "acknowledged": true }))
}

/// `GET /{index}/_settings` — the index's settings, keyed by its own name.
pub(super) fn settings_request(
    collection: &CollectionRecord,
    request: &Request<'_>,
    index_name: &str,
) -> RtResponse {
    if request.method != "GET" {
        return wrong_method(request, "GET");
    }
    let Some(index) = collection.indexes.get(index_name) else {
        return index_not_found(index_name);
    };
    ok(&json!({
        index_name: { "settings": settings(collection, index_name, index) }
    }))
}

/// The index's whole stored schema as JSON — `{}` for an index created without one.
fn schema_value(collection: &CollectionRecord, index_name: &str) -> Value {
    collection
        .indexes
        .get(index_name)
        .and_then(|index| index.index_schema.as_ref())
        .map_or_else(|| json!({}), json_from_document)
}

/// The index's field mappings: the `mappings` member of the schema its create carried, or
/// `{}` for an index whose create carried none.
pub(super) fn mappings(index: &state::IndexRecord) -> Value {
    index
        .index_schema
        .as_ref()
        .map(json_from_document)
        .and_then(|schema| schema.get("mappings").cloned())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// The index's settings: the `settings` member of the schema its create carried, plus the
/// three the SERVICE owns — the name the index was created under, the instant it was
/// created at, and its uuid.
///
/// Every one of them is read off the stored record, so two reads of one index report one
/// set of settings.
fn settings(
    collection: &CollectionRecord,
    index_name: &str,
    index: &state::IndexRecord,
) -> Value {
    let mut settings = index
        .index_schema
        .as_ref()
        .map(json_from_document)
        .and_then(|schema| schema.get("settings").cloned())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let Some(members) = settings.as_object_mut() else {
        return settings;
    };
    let service_owned = json!({
        "provided_name": index_name,
        "creation_date": index.created_date.to_string(),
        "uuid": index_uuid(collection, index_name),
    });
    // A caller that wrote a non-object at `index` has its own value reported back as it
    // was stored: the service-owned settings cannot be merged into it.
    if let Some(held) = members
        .entry("index")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        if let Value::Object(service_owned) = service_owned {
            held.extend(service_owned);
        }
    }
    settings
}
