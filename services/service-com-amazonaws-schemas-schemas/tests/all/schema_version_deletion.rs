// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for deleting ONE version of a schema.
//!
//! RULE under test: deleting a schema version removes exactly the version named and
//! nothing else — the schema, its metadata and every other version it holds stay
//! readable with the bytes they were written with; a read of "the latest" answers the
//! newest version that survives; a version number a delete frees is never minted
//! again; a request that would leave the schema with NO version is refused 409
//! `ConflictException`, so only `DeleteSchema` takes a schema's final definition; and a
//! request naming a version the schema does not hold (or a schema or registry that does
//! not exist) is refused as not found, having deleted nothing. A service-owned
//! registry's content is AWS's, so a delete inside one is refused as forbidden.
//!
//! The rows below vary along what that rule generalises over — the NUMBER of versions
//! a schema holds, WHICH of them is deleted (the first, a middle one, the newest), how
//! many are deleted in turn, how the schema came to hold just one, and the FORM of a
//! `SchemaVersion` that names no stored version — rather than along the name of any one
//! schema.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. Nothing
//! here touches the state struct directly, so a handler that answered from its input
//! rather than from the store would still have to make a later read agree.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, AWS_MANAGED_REGISTRY, CODE_CONFLICT, CODE_FORBIDDEN,
    CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{
    create_registry, listed_versions, schema_uri, versions_uri, Answer, Emulator,
};

/// The version-scoped URI `DeleteSchemaVersion` is bound to.
///
/// Shared with the sibling suite in `schema_version_label_spellings`, so both spell the
/// route the same way.
pub(crate) fn version_uri(registry: &str, schema: &str, version: &str) -> String {
    format!("{}/version/{version}", schema_uri(registry, schema))
}

/// The `DescribeSchema` URI of one version of one schema.
fn describe_uri(registry: &str, schema: &str, version: &str) -> String {
    format!("{}?schemaVersion={version}", schema_uri(registry, schema))
}

/// A JSON schema document, titled so each version's content is its own.
fn document(marker: &str) -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"title": marker, "version": "1.0.0"},
        "paths": {},
    })
    .to_string()
}

/// The content the `n`th version of every schema below is written with.
fn content_of(version: u32) -> String {
    document(&format!("version-{version}"))
}

/// Create a registry holding one schema with `count` versions, the `n`th carrying
/// `content_of(n)`; answers the version numbers the service minted, in order.
async fn schema_with_versions(
    emulator: &Emulator,
    registry: &str,
    schema: &str,
    count: u32,
) -> Vec<String> {
    create_registry(emulator, registry).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": content_of(1), "Type": "OpenApi3"})),
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
                Some(json!({"Content": content_of(round), "Type": "OpenApi3"})),
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

/// The version numbers `ListSchemaVersions` reports for one schema, read through the
/// shared harness's reducer.
///
/// Shared with the sibling suite in `schema_version_label_spellings`, so no two suites
/// can disagree about which versions a schema still holds.
pub(crate) async fn stored_versions(
    emulator: &Emulator,
    registry: &str,
    schema: &str,
) -> BTreeSet<String> {
    let listed = emulator
        .call("GET", &versions_uri(registry, schema), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListSchemaVersions, got {} {:?}",
        listed.status, listed.body
    );
    listed_versions(&listed)
}

/// Assert `answer` is the empty 204 a delete answers with.
fn assert_deleted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 204,
        "case {case}: expected 204 from DeleteSchemaVersion, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.raw.is_empty(),
        "case {case}: a 204 carries no body, got {:?}",
        String::from_utf8_lossy(&answer.raw)
    );
}

/// Assert `answer` is the modelled 404 for a version, schema or registry that is not
/// there.
fn assert_not_found(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected 404, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type(),
        "NotFoundException",
        "case {case}: expected NotFoundException, got {:?}",
        answer.error_type()
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_RESOURCE_NOT_FOUND,
        "case {case}: expected {CODE_RESOURCE_NOT_FOUND}, got {:?}",
        answer.str_at("Code")
    );
}

/// Assert `answer` is the modelled 409 for a delete that would leave the schema with no
/// version at all.
fn assert_last_version_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 409,
        "case {case}: expected 409, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type(),
        "ConflictException",
        "case {case}: expected ConflictException, got {:?}",
        answer.error_type()
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_CONFLICT,
        "case {case}: expected {CODE_CONFLICT}, got {:?}",
        answer.str_at("Code")
    );
    assert_eq!(
        answer.str_at("Message"),
        "Cannot delete all versions of a Schema. To delete a Schema use the DeleteSchema API.",
        "case {case}: wrong sentence for a delete of the schema's last version, got {:?}",
        answer.body
    );
}

/// RULE: a delete takes exactly the named version. Every other version of the schema
/// is still readable with the bytes it was written with, the schema itself survives,
/// and a read of the latest version answers the newest one that remains.
#[tokio::test]
async fn deleting_a_version_leaves_the_schema_and_its_other_versions() {
    // Rows: the count of stored versions crossed with WHICH of them goes — the first, a
    // middle one, the newest, and one of a longer run.
    let cases: [(&str, u32, u32); 5] = [
        ("the first of two", 2, 1),
        ("the newest of two", 2, 2),
        ("the first of three", 3, 1),
        ("the middle of three", 3, 2),
        ("the newest of five", 5, 5),
    ];

    for (case, count, deleted) in cases {
        let emulator = Emulator::new();
        let registry = "version-deletes";
        let schema = "order.placed";
        let minted = schema_with_versions(&emulator, registry, schema, count).await;
        let target = minted
            .get((deleted - 1) as usize)
            .unwrap_or_else(|| panic!("case {case}: version {deleted} was minted"))
            .clone();

        let answer = emulator
            .call("DELETE", &version_uri(registry, schema, &target), None)
            .await;
        assert_deleted(case, &answer);

        // Exactly the other versions remain, and each still carries its own content.
        let survivors: BTreeSet<String> = minted
            .iter()
            .filter(|version| **version != target)
            .cloned()
            .collect();
        assert_eq!(
            stored_versions(&emulator, registry, schema).await,
            survivors,
            "case {case}: only the deleted version goes"
        );
        for (index, version) in minted.iter().enumerate() {
            let described = emulator
                .call("GET", &describe_uri(registry, schema, version), None)
                .await;
            if *version == target {
                assert_not_found(&format!("{case}/DescribeSchema of v{version}"), &described);
                continue;
            }
            assert_eq!(
                described.status, 200,
                "case {case}: v{version} must survive, got {} {:?}",
                described.status, described.body
            );
            assert_eq!(
                described.str_at("Content"),
                content_of((index + 1) as u32),
                "case {case}: v{version} keeps the bytes it was written with"
            );
        }

        // A read of "the latest" answers the newest version that survives; the schema
        // always holds one, because the delete of its last definition is refused.
        let latest = emulator.call("GET", &schema_uri(registry, schema), None).await;
        let newest = survivors
            .iter()
            .next_back()
            .unwrap_or_else(|| panic!("case {case}: the schema must still hold a version"));
        assert_eq!(
            latest.status, 200,
            "case {case}: expected 200 from DescribeSchema, got {} {:?}",
            latest.status, latest.body
        );
        assert_eq!(
            latest.str_at("SchemaVersion"),
            *newest,
            "case {case}: the latest version is the newest that survives, got {:?}",
            latest.str_at("SchemaVersion")
        );

        // Deleting the same version again finds nothing to delete.
        let again = emulator
            .call("DELETE", &version_uri(registry, schema, &target), None)
            .await;
        assert_not_found(&format!("{case}/the second delete"), &again);
    }
}

/// RULE: a version number a delete freed is never minted again — the next definition
/// takes the next number after every version the schema has EVER held.
#[tokio::test]
async fn a_freed_version_number_is_never_minted_again() {
    // Rows: which of three stored versions are deleted before the next definition is
    // written — the newest, the oldest, a pair, and every one but the last (which a
    // delete cannot take).
    let cases: [(&str, &[u32]); 4] = [
        ("the newest", &[3]),
        ("the oldest", &[1]),
        ("the newest and the oldest", &[3, 1]),
        ("every version but the last", &[1, 2]),
    ];

    for (case, deleted) in cases {
        let emulator = Emulator::new();
        let registry = "version-numbering";
        let schema = "order.shipped";
        schema_with_versions(&emulator, registry, schema, 3).await;
        for version in deleted {
            let answer = emulator
                .call(
                    "DELETE",
                    &version_uri(registry, schema, &version.to_string()),
                    None,
                )
                .await;
            assert_deleted(&format!("{case}/v{version}"), &answer);
        }

        let updated = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Content": content_of(4), "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateSchema, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.str_at("SchemaVersion"),
            "4",
            "case {case}: the next version follows every version ever minted, got {:?}",
            updated.str_at("SchemaVersion")
        );
        // And it is the one a read of the latest answers with.
        let described = emulator.call("GET", &schema_uri(registry, schema), None).await;
        assert_eq!(
            described.str_at("SchemaVersion"),
            "4",
            "case {case}: the newly minted version is the latest, got {:?}",
            described.str_at("SchemaVersion")
        );
        assert_eq!(
            described.str_at("Content"),
            content_of(4),
            "case {case}: the newly minted version carries its own content"
        );
    }
}

/// RULE: a `SchemaVersion` that names no stored version is refused as not found and
/// deletes nothing — whatever form the value takes.
#[tokio::test]
async fn a_version_the_schema_does_not_hold_is_refused_and_deletes_nothing() {
    let emulator = Emulator::new();
    let registry = "version-refusals";
    let schema = "order.cancelled";
    let minted: BTreeSet<String> = schema_with_versions(&emulator, registry, schema, 2)
        .await
        .into_iter()
        .collect();

    // Rows: the forms a `SchemaVersion` can take that name no stored version — a
    // number below the first one, numbers past the newest (including one past the
    // widest number a version counter can reach), and values that are not decimal
    // version numbers at all.
    let versions = [
        "0",
        "3",
        "99",
        "18446744073709551616",
        "abc",
        "-1",
        "1.0",
        "%201",
        "1a",
        "latest",
    ];
    for version in versions {
        let refused = emulator
            .call("DELETE", &version_uri(registry, schema, version), None)
            .await;
        assert_not_found(&format!("SchemaVersion={version:?}"), &refused);
        assert_eq!(
            stored_versions(&emulator, registry, schema).await,
            minted,
            "case SchemaVersion={version:?}: a refused delete removes nothing"
        );
    }

    // The same refusal when the schema or the registry is the part that is not there.
    for (case, uri) in [
        (
            "a schema the registry does not hold",
            version_uri(registry, "no-such-schema", "1"),
        ),
        (
            "a registry nothing created",
            version_uri("no-such-registry", schema, "1"),
        ),
    ] {
        let refused = emulator.call("DELETE", &uri, None).await;
        assert_not_found(case, &refused);
    }
    assert_eq!(
        stored_versions(&emulator, registry, schema).await,
        minted,
        "every refusal above left the schema's versions as they were"
    );
}

/// RULE: the contents of a service-owned registry are AWS's, so a caller cannot delete
/// a version of one of its schemas; the version is still there afterwards.
#[tokio::test]
async fn a_version_of_a_service_owned_schema_cannot_be_deleted() {
    let emulator = Emulator::new();
    // Source: `crate::state::aws_managed_schemas` — the seeded catalogue, rather than
    // a schema name written out here. Several rows, so the refusal is not pinned to
    // one published schema.
    let published: Vec<String> = aws_managed_schemas()
        .iter()
        .map(|schema| schema.name.clone())
        .take(3)
        .collect();
    assert!(
        !published.is_empty(),
        "the service-owned catalogue must publish schemas to delete from"
    );

    for name in published {
        let before = stored_versions(&emulator, AWS_MANAGED_REGISTRY, &name).await;
        for version in &before {
            let refused = emulator
                .call(
                    "DELETE",
                    &version_uri(AWS_MANAGED_REGISTRY, &name, version),
                    None,
                )
                .await;
            assert_eq!(
                refused.status, 403,
                "case {name:?}/v{version}: expected 403, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type(),
                "ForbiddenException",
                "case {name:?}/v{version}: expected ForbiddenException, got {:?}",
                refused.error_type()
            );
            assert_eq!(
                refused.str_at("Code"),
                CODE_FORBIDDEN,
                "case {name:?}/v{version}: expected {CODE_FORBIDDEN}, got {:?}",
                refused.str_at("Code")
            );
        }
        assert_eq!(
            stored_versions(&emulator, AWS_MANAGED_REGISTRY, &name).await,
            before,
            "case {name:?}: a refused delete leaves the published versions as they were"
        );
    }
}

/// RULE: a response with no body states its length, which the 204 this delete answers
/// with is one of. (The framing rule itself, and what the HTTP writer does with it on a
/// socket, are `empty_response_framing.rs`'s; this row is the version delete's.)
#[tokio::test]
async fn the_delete_answers_with_no_body_and_states_its_length() {
    let emulator = Emulator::new();
    let registry = "version-framing";
    let schema = "order.returned";
    let minted = schema_with_versions(&emulator, registry, schema, 3).await;
    // Every version but the last the schema holds: a delete that would empty it is
    // refused, and a refusal is not what this row is about.
    for version in &minted[..minted.len() - 1] {
        let answer = emulator
            .call("DELETE", &version_uri(registry, schema, version), None)
            .await;
        assert_deleted(&format!("v{version}"), &answer);
        assert_eq!(
            answer.header("content-length"),
            Some("0"),
            "case v{version}: an answer with no body must state its length, got {:?}",
            answer.header("content-length")
        );
    }
    // Every delete landed: the schema holds only the version no delete may take.
    assert_eq!(
        stored_versions(&emulator, registry, schema).await,
        BTreeSet::from([minted[minted.len() - 1].clone()]),
        "every delete above landed"
    );
}

/// What `ListSchemaVersions` reports about a schema is read from the store, so a
/// deleted version is gone from the schema's `VersionCount` too.
#[tokio::test]
async fn the_schema_listing_counts_only_the_versions_that_remain() {
    let emulator = Emulator::new();
    let registry = "version-counts";
    let schema = "order.updated";
    let minted = schema_with_versions(&emulator, registry, schema, 4).await;

    let mut expected = minted.len();
    for version in &minted {
        let answer = emulator
            .call("DELETE", &version_uri(registry, schema, version), None)
            .await;
        if expected == 1 {
            // The last definition the schema holds: refused, so the count does not move.
            assert_last_version_refused(&format!("v{version}"), &answer);
        } else {
            assert_deleted(&format!("v{version}"), &answer);
            expected -= 1;
        }
        let listed = emulator
            .call(
                "GET",
                &format!("/v1/registries/name/{registry}/schemas"),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "expected 200 from ListSchemas, got {} {:?}",
            listed.status, listed.body
        );
        let summary = listed.body["Schemas"]
            .as_array()
            .and_then(|schemas| schemas.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            summary["VersionCount"].as_u64(),
            Some(expected as u64),
            "case v{version}: VersionCount counts the versions that remain, got {summary:?}"
        );
    }
}

/// RULE: a delete takes everything that described only the version it removed — the
/// idempotency record of the call that minted it included. So a retry carrying that
/// same `ClientTokenId` has no minted version left to answer with and writes a new
/// one, past every version the schema ever held, rather than resurrecting the deleted
/// definition.
#[tokio::test]
async fn a_delete_takes_the_idempotency_record_of_the_version_it_removes() {
    // Rows: how many retried mints are deleted in turn — the record must go with each
    // one, so a loop of {mint with a fresh token, delete it} never answers a deleted
    // version and never holds more than the schema's versions.
    for (case, rounds) in [("one mint", 1_u32), ("four mints", 4)] {
        let emulator = Emulator::new();
        let registry = "version-idempotency";
        let schema = "order.reconciled";
        schema_with_versions(&emulator, registry, schema, 1).await;

        for round in 1..=rounds {
            let token = format!("token-{round}");
            let minted = emulator
                .call(
                    "PUT",
                    &schema_uri(registry, schema),
                    Some(json!({
                        "Content": content_of(round + 1),
                        "Type": "OpenApi3",
                        "ClientTokenId": token,
                    })),
                )
                .await;
            assert_eq!(
                minted.status, 200,
                "case {case}/{token}: expected 200 from UpdateSchema, got {} {:?}",
                minted.status, minted.body
            );
            let version = minted.str_at("SchemaVersion");

            // The retry before the delete is the idempotent one: same token, same
            // version, no second definition.
            let retried = emulator
                .call(
                    "PUT",
                    &schema_uri(registry, schema),
                    Some(json!({
                        "Content": content_of(round + 1),
                        "Type": "OpenApi3",
                        "ClientTokenId": token,
                    })),
                )
                .await;
            assert_eq!(
                retried.str_at("SchemaVersion"),
                version,
                "case {case}/{token}: a retry answers the version its first attempt minted"
            );

            let deleted = emulator
                .call("DELETE", &version_uri(registry, schema, &version), None)
                .await;
            assert_deleted(&format!("{case}/{token}"), &deleted);

            // The same token again: its version is gone, so a NEW one is minted.
            let after = emulator
                .call(
                    "PUT",
                    &schema_uri(registry, schema),
                    Some(json!({
                        "Content": content_of(round + 1),
                        "Type": "OpenApi3",
                        "ClientTokenId": token,
                    })),
                )
                .await;
            let reminted = after.str_at("SchemaVersion");
            assert_ne!(
                reminted, version,
                "case {case}/{token}: the deleted version must not come back, got {reminted:?}"
            );
            assert!(
                stored_versions(&emulator, registry, schema)
                    .await
                    .contains(&reminted),
                "case {case}/{token}: the re-minted version is the one the schema holds"
            );

            // Clean the re-mint away so the next round starts from one version again.
            let deleted = emulator
                .call("DELETE", &version_uri(registry, schema, &reminted), None)
                .await;
            assert_deleted(&format!("{case}/{token}/re-mint"), &deleted);
            assert_eq!(
                stored_versions(&emulator, registry, schema).await,
                BTreeSet::from(["1".to_string()]),
                "case {case}/{token}: only the first version is left between rounds"
            );
        }
    }
}
/// RULE: a schema always holds at least one definition, so a `DeleteSchemaVersion` of
/// the only version a schema still holds is refused 409 `ConflictException` and removes
/// nothing — whichever version that is and however the schema came to hold just one.
/// `DeleteSchema` is the one call that takes a schema's final definition, and it takes
/// the schema with it.
#[tokio::test]
async fn the_only_version_a_schema_holds_cannot_be_deleted() {
    // Rows: how the schema came to hold exactly one version, and WHICH version number
    // that one is — the first one minted, a middle one, the newest of a run — so the
    // refusal is not keyed to a version number or to a freshly created schema.
    let cases: [(&str, u32, &[u32]); 5] = [
        ("created with one version", 1, &[]),
        ("two versions, the newest deleted", 2, &[2]),
        ("two versions, the first deleted", 2, &[1]),
        ("four versions, all but the second deleted", 4, &[1, 3, 4]),
        ("four versions, all but the newest deleted", 4, &[1, 2, 3]),
    ];

    for (case, count, deleted) in cases {
        let emulator = Emulator::new();
        let registry = "last-version-guard";
        let schema = "order.archived";
        let minted = schema_with_versions(&emulator, registry, schema, count).await;
        for version in deleted {
            let removed = emulator
                .call(
                    "DELETE",
                    &version_uri(registry, schema, &version.to_string()),
                    None,
                )
                .await;
            assert_deleted(&format!("{case}/v{version}"), &removed);
        }
        let held = stored_versions(&emulator, registry, schema).await;
        assert_eq!(
            held.len(),
            1,
            "case {case}: the schema must be left holding exactly one version, got {held:?}"
        );
        let last = held
            .iter()
            .next()
            .unwrap_or_else(|| panic!("case {case}: one version is held"))
            .clone();

        // The refusal, twice over: it is the state of the schema that refuses, not the
        // first attempt.
        for attempt in 1..=2 {
            let refused = emulator
                .call("DELETE", &version_uri(registry, schema, &last), None)
                .await;
            assert_last_version_refused(&format!("{case}/attempt {attempt}"), &refused);
            assert_eq!(
                stored_versions(&emulator, registry, schema).await,
                BTreeSet::from([last.clone()]),
                "case {case}: the refused delete must remove nothing"
            );
        }

        // The definition is still readable, and still the one the latest read answers.
        let described = emulator
            .call("GET", &describe_uri(registry, schema, &last), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: v{last} must still be readable, got {} {:?}",
            described.status, described.body
        );
        let index: usize = last
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("case {case}: a version label renders in decimal"));
        assert_eq!(
            described.str_at("Content"),
            content_of(index as u32),
            "case {case}: v{last} keeps the bytes it was written with"
        );
        assert_eq!(
            emulator
                .call("GET", &schema_uri(registry, schema), None)
                .await
                .str_at("SchemaVersion"),
            last,
            "case {case}: the surviving version is what a read of the latest answers"
        );
        assert_eq!(
            minted.len(),
            count as usize,
            "case {case}: the seeding minted {count} versions"
        );

        // `DeleteSchema` is the call that does remove it, schema and all.
        let deleted_schema = emulator
            .call("DELETE", &schema_uri(registry, schema), None)
            .await;
        assert_eq!(
            deleted_schema.status, 204,
            "case {case}: expected 204 from DeleteSchema, got {} {:?}",
            deleted_schema.status, deleted_schema.body
        );
        let gone = emulator
            .call("GET", &schema_uri(registry, schema), None)
            .await;
        assert_not_found(&format!("{case}/after DeleteSchema"), &gone);
    }
}
