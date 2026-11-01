// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SEQUENCES a client drives through a schema's lifetime,
//! where one call's answer decides what the next call may do.
//!
//! RULES under test, each driven as a whole sequence rather than as one call:
//!
//! * a schema always holds at least one definition, so a `DeleteSchemaVersion` walking a
//!   schema's versions is served for every version but the last and refused 409
//!   `ConflictException` for that one, leaving the definition readable;
//! * a version a delete removed is gone for every later call that names it — the read of
//!   it and the re-delete of it take the one 404 `NotFoundException` the service words for
//!   a version a schema does not hold, built from the schema name and the version alone;
//! * an export is served only for a schema the SERVICE wrote, and only INTO the one
//!   language it renders, so a caller's own schema is refused on the `Type` it asks for
//!   before it is refused on who wrote it;
//! * a schema a caller created enters the registry's keyword index a short span after it
//!   is written — nothing of it is reachable by keyword before then, and from the catch-up
//!   on a word of its document and a word of its name both select it — so a
//!   discovery-by-keyword flow that polls finds it.
//!
//! Every case drives the real wire path through the crate's one harness in
//! `registry_schema_lifecycle.rs`; nothing here touches the state struct directly.

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{
    CODE_CONFLICT, CODE_FORBIDDEN, CODE_MALFORMED_REQUEST, CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{
    create_registry, export_uri, schema_names, schema_uri, wait_for_keyword_index, Answer,
    Emulator,
};
use crate::schema_version_deletion::{stored_versions, version_uri};

/// The one language an export is rendered into. Source: `com.amazonaws.schemas#Type`.
const DRAFT4: &str = "JSONSchemaDraft4";

/// The other modelled schema language — one a schema is STORED as, and no export target.
const OPEN_API_3: &str = "OpenApi3";

/// An OpenAPI 3 document carrying `marker` as its title and as a property name, so a
/// search has a word that occurs in this document and in no other.
fn document(marker: &str) -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"title": marker, "version": "1.0.0"},
        "paths": {},
        "components": {"schemas": {"Event": {
            "type": "object",
            "properties": {marker: {"type": "string"}},
        }}},
    })
    .to_string()
}

/// Create a registry holding one schema with `count` versions, the `n`th titled
/// `<marker>-<n>`; answers the version labels the service minted, in order.
async fn schema_with_versions(
    emulator: &Emulator,
    registry: &str,
    schema: &str,
    marker: &str,
    count: u32,
) -> Vec<String> {
    create_registry(emulator, registry).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": document(&format!("{marker}-1")), "Type": OPEN_API_3})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {schema:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    let mut minted = vec![created.str_at("SchemaVersion")];
    for round in 2..=count {
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({
                    "Content": document(&format!("{marker}-{round}")),
                    "Type": OPEN_API_3,
                })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {schema:?}/v{round}: expected 200 from UpdateSchema, got {} {:?}",
            updated.status, updated.body
        );
        minted.push(updated.str_at("SchemaVersion"));
    }
    minted
}

/// The sentence the service words a version the named schema does not hold with: built
/// from the request's `SchemaName` and `SchemaVersion`, and from those alone.
fn no_such_version_sentence(schema: &str, version: &str) -> String {
    format!("Schema with name {schema} and version {version} does not exist.")
}

/// Assert `answer` is the 404 `NotFoundException` for a version the schema does not hold.
fn assert_version_not_found(case: &str, schema: &str, version: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected 404, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NotFoundException",
        "case {case}: expected NotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_RESOURCE_NOT_FOUND,
        "case {case}: expected the {CODE_RESOURCE_NOT_FOUND} condition code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Message"),
        no_such_version_sentence(schema, version),
        "case {case}: wrong sentence for a version the schema does not hold, got {:?}",
        answer.body
    );
}

/// RULE: walking a schema's versions with `DeleteSchemaVersion` takes every version but
/// the last; the one left is refused 409 `ConflictException`, and after the refusal the
/// schema still reads back at that version. Each version the walk removed answers the one
/// not-found sentence for a read of it and for a re-delete of it.
#[tokio::test]
async fn deleting_a_schemas_versions_in_turn_stops_at_its_last_definition() {
    // Rows: how many definitions the schema holds, and in which order the walk takes
    // them — oldest first, newest first — so neither the count nor the order decides
    // which call is refused.
    let cases: [(&str, u32, bool); 6] = [
        ("one version, oldest first", 1, true),
        ("two versions, oldest first", 2, true),
        ("two versions, newest first", 2, false),
        ("three versions, oldest first", 3, true),
        ("three versions, newest first", 3, false),
        ("five versions, newest first", 5, false),
    ];

    for (case, count, oldest_first) in cases {
        let emulator = Emulator::new();
        let registry = "version-walk";
        let schema = "order.placed";
        let minted = schema_with_versions(&emulator, registry, schema, "walk", count).await;
        let mut order: Vec<String> = minted.clone();
        if !oldest_first {
            order.reverse();
        }

        let mut removed: Vec<String> = Vec::new();
        for (index, version) in order.iter().enumerate() {
            let answer = emulator
                .call("DELETE", &version_uri(registry, schema, version), None)
                .await;
            let last = index + 1 == order.len();
            if last {
                // The definition the schema cannot be emptied of.
                assert_eq!(
                    answer.status, 409,
                    "case {case}/v{version}: expected 409 for the last definition, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ConflictException",
                    "case {case}/v{version}: expected ConflictException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.str_at("Code"),
                    CODE_CONFLICT,
                    "case {case}/v{version}: expected the {CODE_CONFLICT} condition code, \
                     got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.str_at("Message"),
                    "Cannot delete all versions of a Schema. To delete a Schema use the \
                     DeleteSchema API.",
                    "case {case}/v{version}: wrong sentence for the last definition, got {:?}",
                    answer.body
                );
                // It is still there, and still what a read of the schema answers.
                let described = emulator
                    .call(
                        "GET",
                        &format!("{}?schemaVersion={version}", schema_uri(registry, schema)),
                        None,
                    )
                    .await;
                assert_eq!(
                    described.status, 200,
                    "case {case}/v{version}: the refused delete must leave it readable, \
                     got {} {:?}",
                    described.status, described.body
                );
                let latest = emulator
                    .call("GET", &schema_uri(registry, schema), None)
                    .await;
                assert_eq!(
                    latest.str_at("SchemaVersion"),
                    *version,
                    "case {case}: the surviving definition is what a read of the latest \
                     answers, got {:?}",
                    latest.body
                );
                continue;
            }

            assert_eq!(
                answer.status, 204,
                "case {case}/v{version}: expected 204, got {} {:?}",
                answer.status, answer.body
            );
            removed.push(version.clone());

            // Every version the walk has removed so far answers the one not-found
            // sentence — for a read of it and for a re-delete of it.
            for gone in &removed {
                let read = emulator
                    .call(
                        "GET",
                        &format!("{}?schemaVersion={gone}", schema_uri(registry, schema)),
                        None,
                    )
                    .await;
                assert_version_not_found(
                    &format!("{case}/DescribeSchema of v{gone}"),
                    schema,
                    gone,
                    &read,
                );
                let again = emulator
                    .call("DELETE", &version_uri(registry, schema, gone), None)
                    .await;
                assert_version_not_found(
                    &format!("{case}/re-delete of v{gone}"),
                    schema,
                    gone,
                    &again,
                );
            }
        }

        // Exactly one definition is left: the last one the walk reached.
        let held = stored_versions(&emulator, registry, schema).await;
        assert_eq!(
            held.len(),
            1,
            "case {case}: the walk must leave exactly one definition, got {held:?}"
        );
    }
}

/// RULE: a caller's own schema has no export, and the `Type` it is asked for is checked
/// first — so a request naming a language that is no export target is refused 400 whatever
/// schema it names, and one naming the target is refused 403 for a schema the service did
/// not write. Both refusals leave the schema exactly as the caller stored it.
#[tokio::test]
async fn exporting_a_callers_own_schema_is_refused_on_the_type_then_on_the_schema() {
    // Rows: the registry, the schema's name and how many versions it holds — none of
    // which the two preconditions turn on.
    let cases: [(&str, &str, &str, u32); 3] = [
        ("one version", "export-seq-a", "order.placed", 1),
        ("two versions", "export-seq-b", "order.amended", 2),
        ("four versions", "export-seq-c", "shipment.delayed", 4),
    ];

    for (case, registry, schema, count) in cases {
        let emulator = Emulator::new();
        schema_with_versions(&emulator, registry, schema, "export", count).await;

        // The `Type` gate first: a language the schema is STORED as is no export target.
        let on_type = emulator
            .call("GET", &export_uri(registry, schema, OPEN_API_3), None)
            .await;
        assert_eq!(
            on_type.status, 400,
            "case {case}: expected 400 for a Type that is no export target, got {} {:?}",
            on_type.status, on_type.body
        );
        assert_eq!(
            on_type.str_at("Code"),
            CODE_MALFORMED_REQUEST,
            "case {case}: expected the {CODE_MALFORMED_REQUEST} condition code, got {:?}",
            on_type.body
        );
        assert_eq!(
            on_type.str_at("Message"),
            format!(
                "1 validation error detected: Value '{OPEN_API_3}' at 'type' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{DRAFT4}]"
            ),
            "case {case}: the refusal must name the value and the admitted set, got {:?}",
            on_type.body
        );

        // Then the eligibility gate, reached only once the `Type` is the one target.
        let on_schema = emulator
            .call("GET", &export_uri(registry, schema, DRAFT4), None)
            .await;
        assert_eq!(
            on_schema.status, 403,
            "case {case}: expected 403 for a schema the service did not write, got {} {:?}",
            on_schema.status, on_schema.body
        );
        assert_eq!(
            on_schema.str_at("Code"),
            CODE_FORBIDDEN,
            "case {case}: expected the {CODE_FORBIDDEN} condition code, got {:?}",
            on_schema.body
        );
        assert_eq!(
            on_schema.str_at("Message"),
            "You cannot export non discovered or non aws managed schemas.",
            "case {case}: wrong sentence for a schema the service did not write, got {:?}",
            on_schema.body
        );

        // Neither refusal touched the schema: its newest definition reads back with the
        // bytes it was written with.
        let described = emulator
            .call("GET", &schema_uri(registry, schema), None)
            .await;
        assert_eq!(
            described.str_at("Content"),
            document(&format!("export-{count}")),
            "case {case}: the refusals must leave the stored document alone, got {:?}",
            described.str_at("Content")
        );
    }
}

/// RULE: a schema a caller created enters the registry's keyword index a short span after
/// it is written — so a search made straight after the create reports nothing for it, and
/// from the catch-up on a word of its stored document, its own name and a word of that
/// name all select it. A search that names no keyword consults no index, so it reports the
/// schema throughout.
#[tokio::test]
async fn a_schema_becomes_searchable_a_short_span_after_it_is_created() {
    // Rows: the marker that occurs in the document (and in nothing else), crossed with a
    // schema name that shares a word with it and one that does not.
    let cases: [(&str, &str, &str); 4] = [
        ("a generated marker", "quokka7f3a1c", "fruit-apple-7f3a1c"),
        ("a marker also in the name", "basket", "catalog-basket-v3"),
        ("a long marker", "verylongpropertyname", "plain-schema"),
        ("a numeric marker", "x29384756", "numbered-29384756"),
    ];

    for (case, marker, name) in cases {
        let emulator = Emulator::new();
        let registry = "searchable-on-create";
        create_registry(&emulator, registry).await;
        let created = emulator
            .call(
                "POST",
                &schema_uri(registry, name),
                Some(json!({"Content": document(marker), "Type": OPEN_API_3})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );

        let name_word = name
            .split('-')
            .next()
            .unwrap_or_else(|| panic!("case {case}: a name carries a word"));
        let keywords = [marker, name, name_word];

        // Straight after the create, nothing of the schema is in the index yet. Asserted
        // on ONE keyword, so this half of the case is the two calls that follow the
        // create and cannot be overtaken by the catch-up.
        let found = searched(&emulator, registry, marker).await;
        assert!(
            found.is_empty(),
            "case {case}/{marker:?}: the index has not caught up yet, got {found:?}"
        );
        // A search that names no keyword consults no index, so it reports the schema even
        // then — the schema is stored, just not indexed.
        assert_eq!(
            searched(&emulator, registry, "").await,
            vec![name.to_string()],
            "case {case}: a search naming no keyword must report the stored schema"
        );

        wait_for_keyword_index();

        // From the catch-up on, every one of those keywords selects it.
        for keyword in keywords {
            let found = searched(&emulator, registry, keyword).await;
            assert_eq!(
                found,
                vec![name.to_string()],
                "case {case}/{keyword:?}: expected the stored schema, got {found:?}"
            );
        }

        // And a keyword nothing about it carries still selects nothing.
        let found = searched(&emulator, registry, "wombat").await;
        assert!(
            found.is_empty(),
            "case {case}: a keyword nothing carries must select nothing, got {found:?}"
        );
    }
}

/// The schema names a search of `registry` for `keywords` reports, asserting 200.
async fn searched(emulator: &Emulator, registry: &str, keywords: &str) -> Vec<String> {
    let answer = emulator
        .call(
            "GET",
            &format!("/v1/registries/name/{registry}/schemas/search?keywords={keywords}"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {keywords:?}: expected 200 from SearchSchemas, got {} {:?}",
        answer.status, answer.body
    );
    schema_names(&answer)
}
