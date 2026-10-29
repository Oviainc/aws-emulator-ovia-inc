// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH type a schema reads as once some of its versions
//! have been deleted, and for the `UpdateSchema` guard that is decided by it.
//!
//! `Type` names the type of the schema CONTENT, so the type a schema carries at any
//! instant is the type of the newest definition it still HOLDS — the same version a
//! version-less read resolves to. A definition a `DeleteSchemaVersion` took out is
//! reported by nothing afterwards, so the type it was written with decides nothing
//! afterwards either: not what a read reports, not what a new version written without
//! a `Type` is stored under, and not whether an `UpdateSchema` that sends `Type`
//! without `Content` is asking to relabel stored content.
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`; nothing here touches the state struct directly.

use serde_json::json;

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Answer, Emulator};
use crate::schema_version_deletion::version_uri;

/// The two schema types this service models.
///
/// Source: the `Type` enum of `UpdateSchemaInput`
/// (`smithy_query(query="shape",shape="Type")`: `JSONSchemaDraft4 | OpenApi3`), which
/// the crate also exposes as `ops::types::TYPE_VALUES`.
const JSON_SCHEMA_DRAFT4: &str = "JSONSchemaDraft4";
const OPEN_API_3: &str = "OpenApi3";

/// The other modelled type — the one a request can name to ask for a relabel.
fn other_type(schema_type: &str) -> &'static str {
    match schema_type {
        OPEN_API_3 => JSON_SCHEMA_DRAFT4,
        JSON_SCHEMA_DRAFT4 => OPEN_API_3,
        other => panic!("unknown schema type {other:?}"),
    }
}

/// A document of `schema_type`, marked so two versions never share their bytes.
fn document(schema_type: &str, marker: &str) -> String {
    match schema_type {
        OPEN_API_3 => json!({
            "openapi": "3.0.0",
            "info": {"title": marker, "version": "1.0.0"},
            "paths": {},
        })
        .to_string(),
        JSON_SCHEMA_DRAFT4 => json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "object",
            "title": marker,
        })
        .to_string(),
        other => panic!("unknown schema type {other:?}"),
    }
}

/// One case: the types the schema's versions are written with, in order, and the
/// version numbers deleted afterwards (in the order given).
struct Case {
    case: &'static str,
    /// The type of version 1, 2, … n — version 1 comes from `CreateSchema`, the rest
    /// from `UpdateSchema`.
    written: &'static [&'static str],
    /// The version numbers a `DeleteSchemaVersion` then takes out.
    deleted: &'static [u64],
    /// The type the schema must read as afterwards: the type of the newest version it
    /// still holds.
    expected: &'static str,
}

/// Build the schema the case describes: create it, write every later version, then
/// delete the versions it names.
async fn build(emulator: &Emulator, registry: &str, schema: &str, case: &Case) {
    create_registry(emulator, registry).await;
    let mut written = case.written.iter().enumerate();
    let (_, first) = written.next().expect("every case writes version 1");
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": document(first, "v1"), "Type": first})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateSchema, got {} {:?}",
        case.case, created.status, created.body
    );

    for (index, schema_type) in written {
        let number = index + 1;
        let marker = format!("v{number}");
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Content": document(schema_type, &marker), "Type": schema_type})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 writing version {number}, got {} {:?}",
            case.case, updated.status, updated.body
        );
        assert_eq!(
            updated.str_at("SchemaVersion"),
            number.to_string(),
            "case {:?}: expected version {number}, got {:?}",
            case.case,
            updated.body
        );
    }

    for version in case.deleted {
        let deleted = emulator
            .call(
                "DELETE",
                &version_uri(registry, schema, &version.to_string()),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected 204 deleting version {version}, got {} {:?}",
            case.case, deleted.status, deleted.body
        );
    }
}

/// Read the schema back (no `SchemaVersion`: the latest the schema still holds).
async fn describe(emulator: &Emulator, registry: &str, schema: &str) -> Answer {
    emulator
        .call("GET", &schema_uri(registry, schema), None)
        .await
}

/// Rows differing along what the rule generalises over: which types the versions were
/// written with, how many there are, which of them are deleted and whether the newest
/// is among them. Each row's `expected` is the type of the newest version left.
const CASES: &[Case] = &[
    Case {
        case: "newest-deleted-leaves-the-older-type",
        written: &[OPEN_API_3, JSON_SCHEMA_DRAFT4],
        deleted: &[2],
        expected: OPEN_API_3,
    },
    Case {
        case: "newest-deleted-leaves-the-older-type-inverted",
        written: &[JSON_SCHEMA_DRAFT4, OPEN_API_3],
        deleted: &[2],
        expected: JSON_SCHEMA_DRAFT4,
    },
    Case {
        case: "two-newest-deleted",
        written: &[OPEN_API_3, JSON_SCHEMA_DRAFT4, JSON_SCHEMA_DRAFT4],
        deleted: &[3, 2],
        expected: OPEN_API_3,
    },
    Case {
        case: "alternating-types-newest-two-deleted",
        written: &[OPEN_API_3, JSON_SCHEMA_DRAFT4, OPEN_API_3, OPEN_API_3],
        deleted: &[4, 3],
        expected: JSON_SCHEMA_DRAFT4,
    },
    Case {
        case: "older-deleted-newest-type-stands",
        written: &[OPEN_API_3, JSON_SCHEMA_DRAFT4],
        deleted: &[1],
        expected: JSON_SCHEMA_DRAFT4,
    },
    Case {
        case: "middle-deleted-newest-type-stands",
        written: &[OPEN_API_3, OPEN_API_3, JSON_SCHEMA_DRAFT4],
        deleted: &[2],
        expected: JSON_SCHEMA_DRAFT4,
    },
    Case {
        case: "nothing-deleted-single-version",
        written: &[JSON_SCHEMA_DRAFT4],
        deleted: &[],
        expected: JSON_SCHEMA_DRAFT4,
    },
    Case {
        case: "nothing-deleted-mixed-versions",
        written: &[JSON_SCHEMA_DRAFT4, OPEN_API_3],
        deleted: &[],
        expected: OPEN_API_3,
    },
];

/// RULE: the type a schema carries is the type of the newest definition it still
/// holds, so that is what a version-less read reports — whatever type a definition a
/// delete took out was written with.
#[tokio::test]
async fn a_read_reports_the_type_of_the_newest_definition_the_schema_still_holds() {
    for case in CASES {
        let emulator = Emulator::new();
        let registry = "mixed-types";
        let schema = "order.placed";
        build(&emulator, registry, schema, case).await;

        let described = describe(&emulator, registry, schema).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeSchema, got {} {:?}",
            case.case, described.status, described.body
        );
        assert_eq!(
            described.str_at("Type"),
            case.expected,
            "case {:?}: expected the type of the newest stored definition, got {:?}",
            case.case,
            described.body
        );
    }
}

/// RULE: `Type` without `Content` asks to relabel stored content, so it is refused
/// only when it names a type OTHER than the one the schema's remaining definition
/// carries; a request naming that type asks for no change to it and is served, and its
/// answer reports the same type.
#[tokio::test]
async fn an_update_naming_the_type_the_schema_still_carries_is_served() {
    for case in CASES {
        let emulator = Emulator::new();
        let registry = "mixed-types";
        let schema = "order.placed";
        build(&emulator, registry, schema, case).await;

        let description = "no change to the type";
        let served = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Type": case.expected, "Description": description})),
            )
            .await;
        assert_eq!(
            served.status, 200,
            "case {:?}: a request naming the type the schema carries asks for no change \
             to it, so it is served; got {} {:?}",
            case.case, served.status, served.body
        );
        assert_eq!(
            served.str_at("Type"),
            case.expected,
            "case {:?}: the answer reports the type the request named, got {:?}",
            case.case,
            served.body
        );
        assert_eq!(
            served.str_at("Description"),
            description,
            "case {:?}: the description the request sent is what the schema now carries, \
             got {:?}",
            case.case,
            served.body
        );
        assert_eq!(
            describe(&emulator, registry, schema).await.str_at("Type"),
            case.expected,
            "case {:?}: a served metadata update does not relabel the definition",
            case.case
        );
    }
}

/// RULE (the other side of it): a `Type` that names the OTHER type is a relabel of
/// stored content, which only a request that also rewrites the content can ask for —
/// refused HTTP 400 `BadRequestException`, storing nothing.
#[tokio::test]
async fn an_update_naming_the_other_type_without_content_is_refused() {
    for case in CASES {
        let emulator = Emulator::new();
        let registry = "mixed-types";
        let schema = "order.placed";
        build(&emulator, registry, schema, case).await;

        let relabel = other_type(case.expected);
        let refused = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Type": relabel, "Description": "a relabel"})),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected 400 for a relabel to {relabel:?} without Content, got {} {:?}",
            case.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "BadRequestException",
            "case {:?}: expected the wire code BadRequestException, got {:?}",
            case.case,
            refused.body
        );
        assert!(
            refused.str_at("Message").contains("Type"),
            "case {:?}: the refusal must name the member it is about, got {:?}",
            case.case,
            refused.body
        );

        // Nothing was stored: neither the type nor the description the refused
        // request carried.
        let described = describe(&emulator, registry, schema).await;
        assert_eq!(
            described.str_at("Type"),
            case.expected,
            "case {:?}: a refused update must not relabel the definition, got {:?}",
            case.case,
            described.body
        );
        assert!(
            described.body.get("Description").is_none_or(|value| value
                .as_str()
                .is_none_or(|text| text != "a relabel")),
            "case {:?}: a refused update must store no description, got {:?}",
            case.case,
            described.body
        );
    }
}

/// RULE: a request that sends `Content` and no `Type` leaves the type unchanged, so
/// the version it mints is stored under the type the schema reads as — never under one
/// only a deleted definition ever carried.
#[tokio::test]
async fn a_version_written_without_a_type_is_stored_under_the_type_the_schema_carries() {
    for case in CASES {
        let emulator = Emulator::new();
        let registry = "mixed-types";
        let schema = "order.placed";
        build(&emulator, registry, schema, case).await;

        let minted = emulator
            .call(
                "PUT",
                &schema_uri(registry, schema),
                Some(json!({"Content": document(case.expected, "typeless")})),
            )
            .await;
        assert_eq!(
            minted.status, 200,
            "case {:?}: expected 200 from a typeless UpdateSchema, got {} {:?}",
            case.case, minted.status, minted.body
        );
        assert_eq!(
            minted.str_at("Type"),
            case.expected,
            "case {:?}: the minted version carries the type the schema read as, got {:?}",
            case.case,
            minted.body
        );

        let number = minted.str_at("SchemaVersion");
        let described = emulator
            .call(
                "GET",
                &format!("{}?schemaVersion={number}", schema_uri(registry, schema)),
                None,
            )
            .await;
        assert_eq!(
            described.str_at("Type"),
            case.expected,
            "case {:?}: version {number} reads back under the type it was stored with, got {:?}",
            case.case,
            described.body
        );
    }
}
