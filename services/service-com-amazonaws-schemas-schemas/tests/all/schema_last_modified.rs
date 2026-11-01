// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a schema's `LastModified`.
//!
//! RULE under test: `LastModified` is "the date and time that schema was modified"
//! (`smithy_query(query="doc", shape="DescribeSchemaResponse", member="LastModified")`,
//! the same doc on `SchemaSummary.LastModified` and `UpdateSchemaResponse.LastModified`),
//! so every call that WRITES to a schema reports the instant of that write — a definition
//! minted, a definition removed, or the schema's own metadata replaced — while a call that
//! changes nothing (a read, a refused write, a request that asks for no change) leaves the
//! member exactly where it was. Every reader of the member answers that one stored
//! instant.
//!
//! The rows differ along what the rule generalises over: WHICH kind of write the call is,
//! which members the request carries, how many versions the schema holds, which of them a
//! delete takes, and which operation the member is read back through. A write's instant is
//! pinned against the wall clock either side of the call rather than against a recorded
//! value, so a handler that answered a fixed timestamp would fail.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. Nothing
//! here touches the state struct directly, so a handler that answered from its input
//! rather than from the store would still have to make a later read agree.

use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use crate::registry_schema_lifecycle::{
    create_registry, export_uri, registry_uri, schema_uri, versions_uri, Answer, Emulator,
};

/// The registry every case below works in.
const REGISTRY: &str = "last-modified";

/// The schema every case below writes to.
const SCHEMA: &str = "order.placed";

/// `GET /v1/registries/name/{registry}/schemas` — the listing that reports a summary of
/// every schema a registry holds, versionless ones included.
fn listing_uri(registry: &str) -> String {
    format!("{}/schemas", registry_uri(registry))
}

/// The version-scoped URI `DeleteSchemaVersion` is bound to.
fn version_uri(registry: &str, schema: &str, version: &str) -> String {
    format!("{}/version/{version}", schema_uri(registry, schema))
}

/// A JSON schema document, titled so each version's content is its own. It declares
/// the event it describes and carries component schemas, so what the keyword index and a
/// conversion read is all there.
fn content_of(version: u32) -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"title": format!("version-{version}"), "version": "1.0.0"},
        "paths": {},
        "components": {"schemas": {
            "AWSEvent": {
                "type": "object",
                "x-amazon-events-source": "my.orders",
                "x-amazon-events-detail-type": "Order Placed",
                "properties": {"detail": {"$ref": "#/components/schemas/Detail"}},
            },
            "Detail": {"type": "object", "properties": {"id": {"type": "string"}}},
        }},
    })
    .to_string()
}

/// Nanoseconds since the epoch on the test's own clock — the same clock the handlers
/// read, so a write's instant falls between a reading taken before the call and one
/// taken after it.
fn clock_nanos() -> i128 {
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the test clock is after the epoch");
    i128::try_from(since_epoch.as_nanos()).expect("the epoch nanoseconds fit an i128")
}

/// The instant a timestamp member carries, in nanoseconds since the epoch.
///
/// What these cases own is the VALUE, not its rendering: the wire FORM of a `date-time`
/// member is pinned on its own in `tests/all/iso8601_timestamps.rs`.
fn instant_nanos(value: &Value, case: &str) -> i128 {
    let rendered = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a timestamp string, got {value:?}"));
    let instant = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case}: {rendered:?} is not an ISO-8601 instant ({e})")
    });
    i128::from(instant.secs()) * 1_000_000_000 + i128::from(instant.subsec_nanos())
}

/// One schema's summary in a `ListSchemas` answer.
fn summary_of<'a>(answer: &'a Answer, name: &str, case: &str) -> &'a Value {
    answer.body["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Schemas array in {:?}", answer.body))
        .iter()
        .find(|summary| summary["SchemaName"].as_str() == Some(name))
        .unwrap_or_else(|| {
            panic!(
                "case {case}: the listing must carry a summary of {name:?}, got {:?}",
                answer.body
            )
        })
}

/// The `LastModified` the store holds for one schema, read back through EVERY operation
/// that reports it and asserted to be the one instant.
///
/// `ListSchemas` is the reader that answers for a schema holding no definition at all, so
/// it is the one the value comes from; `DescribeSchema` is asserted to agree wherever it
/// has a definition to answer about.
async fn stored_last_modified(emulator: &Emulator, case: &str) -> i128 {
    let listed = emulator.call("GET", &listing_uri(REGISTRY), None).await;
    assert_eq!(
        listed.status, 200,
        "case {case}: expected 200 from ListSchemas, got {} {:?}",
        listed.status, listed.body
    );
    let summary = summary_of(&listed, SCHEMA, case);
    let listed_instant = instant_nanos(&summary["LastModified"], &format!("{case}/ListSchemas"));

    let described = emulator
        .call("GET", &schema_uri(REGISTRY, SCHEMA), None)
        .await;
    if described.status == 200 {
        let described_instant =
            instant_nanos(described.value_at("LastModified"), &format!("{case}/DescribeSchema"));
        assert_eq!(
            described_instant, listed_instant,
            "case {case}: DescribeSchema and ListSchemas must report the one stored \
             instant, got {described_instant} and {listed_instant}"
        );
    }
    listed_instant
}

/// Assert `reported` is the instant of a call the clock bracketed by `before`/`after`.
fn assert_written_at(case: &str, reported: i128, before: i128, after: i128) {
    assert!(
        reported >= before && reported <= after,
        "case {case}: expected LastModified at the instant of the write \
         ({before}..={after}), got {reported}"
    );
}

/// One call the table drives, and what the rule says it does to `LastModified`.
#[derive(Debug)]
struct Step {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    /// Whether the call WRITES to the schema, so the member must become its instant.
    writes: bool,
}

/// RULE: every call that writes to a schema sets `LastModified` to the instant of that
/// write, and every call that changes nothing leaves it alone.
#[tokio::test]
async fn every_write_to_a_schema_moves_last_modified_and_nothing_else_does() {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;

    // Rows: the kinds of call a schema can receive — the three kinds of write (a
    // definition minted, the metadata replaced, both at once, a definition removed), the
    // reads, the requests that ask for no change, and the refusals.
    let steps = vec![
        Step {
            case: "CreateSchema writes the first definition",
            method: "POST",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Content": content_of(1), "Type": "OpenApi3"})),
            status: 201,
            writes: true,
        },
        Step {
            case: "DescribeSchema reads",
            method: "GET",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: None,
            status: 200,
            writes: false,
        },
        Step {
            case: "ListSchemaVersions reads",
            method: "GET",
            uri: versions_uri(REGISTRY, SCHEMA),
            body: None,
            status: 200,
            writes: false,
        },
        Step {
            case: "a refused ExportSchema (the caller wrote this schema)",
            method: "GET",
            uri: export_uri(REGISTRY, SCHEMA, "JSONSchemaDraft4"),
            body: None,
            status: 403,
            writes: false,
        },
        Step {
            case: "UpdateSchema mints a second definition",
            method: "PUT",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Content": content_of(2), "Type": "OpenApi3"})),
            status: 200,
            writes: true,
        },
        Step {
            case: "UpdateSchema replaces only the description",
            method: "PUT",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Description": "metadata, no definition"})),
            status: 200,
            writes: true,
        },
        Step {
            case: "UpdateSchema writes a definition and the description at once",
            method: "PUT",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Content": content_of(3), "Description": "both at once"})),
            status: 200,
            writes: true,
        },
        Step {
            case: "UpdateSchema asking for no change",
            method: "PUT",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({})),
            status: 200,
            writes: false,
        },
        Step {
            case: "a refused UpdateSchema (Type without Content)",
            method: "PUT",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Type": "JSONSchemaDraft4"})),
            status: 400,
            writes: false,
        },
        Step {
            case: "DeleteSchemaVersion takes the newest definition",
            method: "DELETE",
            uri: version_uri(REGISTRY, SCHEMA, "3"),
            body: None,
            status: 204,
            writes: true,
        },
        Step {
            case: "a refused DeleteSchemaVersion (the version is already gone)",
            method: "DELETE",
            uri: version_uri(REGISTRY, SCHEMA, "3"),
            body: None,
            status: 404,
            writes: false,
        },
        Step {
            case: "a refused DeleteSchemaVersion (not a version number at all)",
            method: "DELETE",
            uri: version_uri(REGISTRY, SCHEMA, "latest"),
            body: None,
            status: 404,
            writes: false,
        },
        Step {
            case: "DeleteSchemaVersion takes the oldest definition",
            method: "DELETE",
            uri: version_uri(REGISTRY, SCHEMA, "1"),
            body: None,
            status: 204,
            writes: true,
        },
        Step {
            case: "a refused DeleteSchemaVersion (the schema's last definition)",
            method: "DELETE",
            uri: version_uri(REGISTRY, SCHEMA, "2"),
            body: None,
            status: 409,
            writes: false,
        },
        Step {
            case: "ListSchemas reads the schema",
            method: "GET",
            uri: listing_uri(REGISTRY),
            body: None,
            status: 200,
            writes: false,
        },
        Step {
            case: "UpdateSchema mints a further definition",
            method: "PUT",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Content": content_of(4), "Type": "OpenApi3"})),
            status: 200,
            writes: true,
        },
    ];

    let mut previous: Option<i128> = None;
    for step in steps {
        let case = step.case;
        let before = clock_nanos();
        let answer = emulator
            .call(step.method, &step.uri, step.body.clone())
            .await;
        let after = clock_nanos();
        assert_eq!(
            answer.status, step.status,
            "case {case}: expected {}, got {} {:?}",
            step.status, answer.status, answer.body
        );

        let reported = stored_last_modified(&emulator, case).await;
        if step.writes {
            assert_written_at(case, reported, before, after);
            // An operation that answers the member reports the instant it just wrote.
            if answer.body.get("LastModified").is_some() {
                assert_eq!(
                    instant_nanos(answer.value_at("LastModified"), case),
                    reported,
                    "case {case}: the response must carry the instant the store now holds"
                );
            }
        } else if let Some(previous) = previous {
            assert_eq!(
                reported, previous,
                "case {case}: a call that changes nothing must leave LastModified at \
                 {previous}, got {reported}"
            );
        }
        if let Some(previous) = previous {
            assert!(
                reported >= previous,
                "case {case}: LastModified never goes backwards; was {previous}, got {reported}"
            );
        }
        previous = Some(reported);
    }
}

/// RULE (the delete's own row of the rule above, over a SET of schemas): removing a
/// definition is a write to the schema, so it sets `LastModified` to the instant of the
/// delete — however many versions the schema holds and whichever of them goes — and every
/// reader of the member answers the new instant. (A delete that would take the schema's
/// LAST definition is refused and so writes nothing; that refusal is pinned in
/// `tests/all/schema_version_deletion.rs`.)
#[tokio::test]
async fn deleting_a_version_moves_last_modified_whichever_version_goes() {
    // Rows: the count of stored versions crossed with WHICH of them is deleted — the
    // first, a middle one, the newest, and one of a longer run.
    let cases: [(&str, u32, u32); 5] = [
        ("the first of two", 2, 1),
        ("the newest of two", 2, 2),
        ("the first of three", 3, 1),
        ("the middle of three", 3, 2),
        ("the newest of five", 5, 5),
    ];

    for (case, count, deleted) in cases {
        let emulator = Emulator::new();
        create_registry(&emulator, REGISTRY).await;
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, SCHEMA),
                Some(json!({"Content": content_of(1), "Type": "OpenApi3"})),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
        for round in 2..=count {
            let updated = emulator
                .call(
                    "PUT",
                    &schema_uri(REGISTRY, SCHEMA),
                    Some(json!({"Content": content_of(round), "Type": "OpenApi3"})),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case}/v{round}: expected 200 from UpdateSchema, got {} {:?}",
                updated.status, updated.body
            );
        }

        let before_delete = stored_last_modified(&emulator, case).await;
        let before = clock_nanos();
        let answer = emulator
            .call(
                "DELETE",
                &version_uri(REGISTRY, SCHEMA, &deleted.to_string()),
                None,
            )
            .await;
        let after = clock_nanos();
        assert_eq!(
            answer.status, 204,
            "case {case}: expected 204 from DeleteSchemaVersion, got {} {:?}",
            answer.status, answer.body
        );

        let reported = stored_last_modified(&emulator, case).await;
        assert_written_at(case, reported, before, after);
        assert!(
            reported > before_delete,
            "case {case}: the delete changed the schema, so its instant must be later \
             than {before_delete}, got {reported}"
        );
        // And a read of it afterwards reports the same instant, twice over.
        assert_eq!(
            stored_last_modified(&emulator, case).await,
            reported,
            "case {case}: reading the schema must not move its instant"
        );
    }
}
