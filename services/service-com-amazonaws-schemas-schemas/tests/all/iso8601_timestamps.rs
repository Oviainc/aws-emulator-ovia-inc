// SPDX-License-Identifier: CC0-1.0

//! The wire form of this service's timestamps.
//!
//! RULE under test: a response member whose modelled target carries
//! `@timestampFormat("date-time")` is rendered as an RFC 3339 `date-time` STRING — a
//! JSON string, never a JSON number — wherever in a response it is answered: at the top
//! level of an operation's output, inside a summary list, or inside a list nested under
//! one. Every timestamp member of this service targets `__timestampIso8601`
//! (`smithy_query(query="shape", shape="__timestampIso8601")` reports
//! `{"type": "timestamp", "timestampFormat": "date-time"}`), so the rule covers all of
//! them; members of any other type keep their own JSON type.
//!
//! The rows differ along what the rule generalises over: the operation, the member name,
//! the depth the member sits at in the response, and the stored resource the timestamp
//! belongs to. The recorded fixture's `CreateSchema` response is one row of the first
//! table.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it.

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use service_com_amazonaws_schemas_schemas::ops::types::{
    CreateSchemaOutput, DescribeCodeBindingOutput, DescribeSchemaOutput, PutCodeBindingOutput,
    SchemaSummary, SearchSchemaVersionSummary, UpdateSchemaOutput,
};

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Emulator};

/// The registry every case below works in.
const REGISTRY: &str = "iso8601-registry";

/// One stored schema the wire table drives the timestamp-carrying operations for.
#[derive(Debug)]
struct Row {
    case: &'static str,
    schema: &'static str,
    /// A value of `com.amazonaws.schemas#Type`.
    schema_type: &'static str,
    content: &'static str,
    /// Whether the row also sends `Description` and `Tags` — the two optional members
    /// that sit beside the timestamps in the same response.
    with_metadata: bool,
    /// A value of `service_com_amazonaws_schemas_schemas::state::CODE_BINDING_LANGUAGES`,
    /// the language this row generates a code binding for.
    language: &'static str,
}

/// Rows differ along the schema `Type`, the form of `Content`, whether the optional
/// members are present, the shape of the schema name, and the binding language.
///
/// Each row's content declares the event it describes (`x-amazon-events-source` /
/// `x-amazon-events-detail-type`), which is what a registry indexes a schema under and
/// so what `SearchSchemas` — one of the operations whose timestamps this table collects —
/// selects on.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "openapi-minimal",
            schema: "demo-schema",
            schema_type: "OpenApi3",
            content: r##"{"openapi":"3.0.0","components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.timestamps","x-amazon-events-detail-type":"Demo Schema"}}}}"##,
            with_metadata: false,
            language: "Java8",
        },
        Row {
            case: "jsonschema-with-description-and-tags",
            schema: "order.placed_v1",
            schema_type: "JSONSchemaDraft4",
            content: r##"{"$schema":"http://json-schema.org/draft-04/schema#","type":"object","x-amazon-events-source":"my.timestamps","x-amazon-events-detail-type":"Order Placed"}"##,
            with_metadata: true,
            language: "Python36",
        },
        Row {
            case: "non-ascii-content",
            schema: "unicode-schema",
            schema_type: "OpenApi3",
            content: r##"{"openapi":"3.0.0","info":{"title":"caf\u00e9"},"components":{"schemas":{"AWSEvent":{"type":"object","x-amazon-events-source":"my.timestamps","x-amazon-events-detail-type":"Caf\u00e9 Served"}}}}"##,
            with_metadata: true,
            language: "TypeScript3",
        },
    ]
}

/// The `CreateSchema` body for a row.
fn create_body(row: &Row) -> Value {
    let mut body = json!({ "Content": row.content, "Type": row.schema_type });
    if row.with_metadata {
        body["Description"] = json!("a schema the timestamp table stores");
        body["tags"] = json!({ "team": "payments" });
    }
    body
}

/// The content an update stores as a second version, so `UpdateSchema` answers a row
/// whose `VersionCreatedDate` is NOT the schema's creation instant.
fn updated_content(row: &Row) -> String {
    match row.schema_type {
        "JSONSchemaDraft4" => json!({
            "$schema": "http://json-schema.org/draft-04/schema#",
            "type": "object",
            "title": row.case,
        })
        .to_string(),
        _ => json!({
            "openapi": "3.0.0",
            "info": {"title": row.case, "version": "2.0.0"},
        })
        .to_string(),
    }
}

/// `…/language/{Language}`, the URI both code-binding operations bind.
fn binding_uri(schema: &str, language: &str) -> String {
    format!("{}/language/{language}", schema_uri(REGISTRY, schema))
}

/// One timestamp value the wire table collected, labelled by the case it came from.
struct Observed {
    case: String,
    value: Value,
}

/// Collect `members` out of `body`, requiring each to be present.
fn collect(seen: &mut Vec<Observed>, case: &str, body: &Value, members: &[&str]) {
    for member in members {
        let value = body.get(member).unwrap_or_else(|| {
            panic!("case {case:?}: expected a {member:?} member in {body:?}")
        });
        seen.push(Observed {
            case: format!("{case}.{member}"),
            value: value.clone(),
        });
    }
}

/// Assert one observed value is an RFC 3339 `date-time` string in its canonical form.
fn assert_date_time(observed: &Observed) {
    let case = &observed.case;
    let rendered = observed.value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: a date-time member must be a JSON string, got {:?}",
            observed.value
        )
    });
    let instant = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case:?}: expected an RFC 3339 date-time, got {rendered:?} ({e})")
    });
    let recanonicalized = instant
        .fmt(Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {rendered:?} does not re-render ({e})"));
    assert_eq!(
        recanonicalized, rendered,
        "case {case:?}: expected the canonical date-time rendering {recanonicalized:?}, \
         got {rendered:?}"
    );
    // `date-time` is UTC with a `T` separator and a `Z` offset; epoch seconds carry
    // neither, which is exactly the difference this rule is about.
    assert!(
        rendered.contains('T') && rendered.ends_with('Z'),
        "case {case:?}: a date-time is UTC (`…T…Z`), got {rendered:?}"
    );
    // A stored instant is after the epoch and before any plausible clock skew ceiling;
    // a number read as a string would not be.
    assert!(
        instant.secs() > 1_600_000_000,
        "case {case:?}: {rendered:?} does not name a present-day instant, got {} seconds",
        instant.secs()
    );
}

/// RULE: every `date-time` member of every operation that answers one is an RFC 3339
/// string on the wire, at every depth of the response.
#[tokio::test]
async fn every_date_time_member_is_an_rfc3339_string() {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;

    let mut seen: Vec<Observed> = Vec::new();

    for row in rows() {
        let case = row.case;

        // CreateSchema — the operation the recorded fixture diverged on.
        let created = emulator
            .call("POST", &schema_uri(REGISTRY, row.schema), Some(create_body(&row)))
            .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from CreateSchema, got {} {:?}",
            created.status, created.body
        );
        collect(
            &mut seen,
            &format!("CreateSchema[{case}]"),
            &created.body,
            &["LastModified", "VersionCreatedDate"],
        );

        // DescribeSchema — the same two members, read back out of the store.
        let described = emulator
            .call("GET", &schema_uri(REGISTRY, row.schema), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        collect(
            &mut seen,
            &format!("DescribeSchema[{case}]"),
            &described.body,
            &["LastModified", "VersionCreatedDate"],
        );
        // The rendering is of the STORED instant, so the create and the read agree
        // string for string.
        for member in ["LastModified", "VersionCreatedDate"] {
            assert_eq!(
                described.body[member], created.body[member],
                "case {case:?}: {member} must render the instant the create stored, \
                 got {:?} then {:?}",
                created.body[member], described.body[member]
            );
        }

        // UpdateSchema — a second version, so `VersionCreatedDate` is a different
        // instant from the schema's creation.
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(REGISTRY, row.schema),
                Some(json!({ "Content": updated_content(&row), "Type": row.schema_type })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateSchema, got {} {:?}",
            updated.status, updated.body
        );
        collect(
            &mut seen,
            &format!("UpdateSchema[{case}]"),
            &updated.body,
            &["LastModified", "VersionCreatedDate"],
        );

        // PutCodeBinding / DescribeCodeBinding — a different resource, a different
        // pair of member names.
        let put = emulator
            .call("POST", &binding_uri(row.schema, row.language), None)
            .await;
        assert_eq!(
            put.status, 202,
            "case {case:?}: expected 202 from PutCodeBinding, got {} {:?}",
            put.status, put.body
        );
        collect(
            &mut seen,
            &format!("PutCodeBinding[{case}]"),
            &put.body,
            &["CreationDate", "LastModified"],
        );
        let binding = emulator
            .call("GET", &binding_uri(row.schema, row.language), None)
            .await;
        assert_eq!(
            binding.status, 200,
            "case {case:?}: expected 200 from DescribeCodeBinding, got {} {:?}",
            binding.status, binding.body
        );
        collect(
            &mut seen,
            &format!("DescribeCodeBinding[{case}]"),
            &binding.body,
            &["CreationDate", "LastModified"],
        );
    }

    // ListSchemas — the member sits one level down, inside a `SchemaSummary` list.
    let listed = emulator
        .call("GET", &format!("{}/schemas", registry_path()), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListSchemas, got {} {:?}",
        listed.status, listed.body
    );
    let summaries = listed.body["Schemas"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Schemas list from ListSchemas, got {:?}", listed.body));
    assert_eq!(
        summaries.len(),
        rows().len(),
        "ListSchemas must report every created schema, got {:?}",
        listed.body
    );
    for (index, summary) in summaries.iter().enumerate() {
        collect(
            &mut seen,
            &format!("ListSchemas.Schemas[{index}]"),
            summary,
            &["LastModified"],
        );
        // The complement of the rule: a member of another modelled type keeps that
        // type. `SchemaSummary$VersionCount` targets `__long`, so it stays a NUMBER.
        assert!(
            summary["VersionCount"].is_number(),
            "ListSchemas.Schemas[{index}]: VersionCount is modelled __long and must stay \
             a JSON number, got {:?}",
            summary["VersionCount"]
        );
        assert!(
            summary["SchemaName"].is_string(),
            "ListSchemas.Schemas[{index}]: SchemaName is modelled __string, got {:?}",
            summary["SchemaName"]
        );
    }

    // SearchSchemas — the member sits TWO levels down, in a list nested under a list.
    // A search reports the schemas the registry INDEXES, which is every row here:
    // each one's content declares the event it describes.
    let searched = emulator
        .call(
            "GET",
            &format!("{}/schemas/search?keywords=", registry_path()),
            None,
        )
        .await;
    assert_eq!(
        searched.status, 200,
        "expected 200 from SearchSchemas, got {} {:?}",
        searched.status, searched.body
    );
    let found = searched.body["Schemas"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "expected a Schemas list from SearchSchemas, got {:?}",
                searched.body
            )
        });
    assert_eq!(
        found.len(),
        rows().len(),
        "SearchSchemas with no keyword must report every schema, got {:?}",
        searched.body
    );
    let mut nested = 0usize;
    for (index, summary) in found.iter().enumerate() {
        let versions = summary["SchemaVersions"].as_array().unwrap_or_else(|| {
            panic!("expected a SchemaVersions list in {summary:?}")
        });
        assert!(
            !versions.is_empty(),
            "SearchSchemas.Schemas[{index}]: expected at least one version in {summary:?}"
        );
        for (position, version) in versions.iter().enumerate() {
            nested += 1;
            collect(
                &mut seen,
                &format!("SearchSchemas.Schemas[{index}].SchemaVersions[{position}]"),
                version,
                &["CreatedDate"],
            );
        }
    }
    // Each row was updated once, so every schema reports two versions: the nested list
    // really is exercised at more than one element.
    assert_eq!(
        nested,
        rows().len() * 2,
        "expected two searched versions per schema, got {nested} in {:?}",
        searched.body
    );

    // Ten per row from the five operations above (two members each), one per row from
    // `ListSchemas` and two per row from `SearchSchemas`: the table must have observed
    // them all before the assertions below mean anything.
    let expected = rows().len() * 13;
    assert_eq!(
        seen.len(),
        expected,
        "expected {expected} date-time values, collected {}: {:?}",
        seen.len(),
        seen.iter().map(|o| o.case.clone()).collect::<Vec<_>>()
    );

    for observed in &seen {
        assert_date_time(observed);
    }
}

/// `/v1/registries/name/{REGISTRY}`.
fn registry_path() -> String {
    format!("/v1/registries/name/{REGISTRY}")
}

/// Every shape of this service that carries a `__timestampIso8601` member, with the
/// member names the model gives it.
///
/// Source: `smithy_query(query="shape", shape="CreateSchemaResponse,
/// DescribeSchemaResponse,UpdateSchemaResponse,PutCodeBindingResponse,
/// DescribeCodeBindingResponse,SchemaSummary,SearchSchemaVersionSummary")` — the seven
/// shapes of this model with a timestamp member, each of which targets
/// `__timestampIso8601`.
fn timestamp_shapes() -> Vec<(&'static str, &'static Schema, &'static [&'static str])> {
    vec![
        (
            "CreateSchemaResponse",
            &CreateSchemaOutput::SCHEMA,
            &["LastModified", "VersionCreatedDate"],
        ),
        (
            "DescribeSchemaResponse",
            &DescribeSchemaOutput::SCHEMA,
            &["LastModified", "VersionCreatedDate"],
        ),
        (
            "UpdateSchemaResponse",
            &UpdateSchemaOutput::SCHEMA,
            &["LastModified", "VersionCreatedDate"],
        ),
        (
            "PutCodeBindingResponse",
            &PutCodeBindingOutput::SCHEMA,
            &["CreationDate", "LastModified"],
        ),
        (
            "DescribeCodeBindingResponse",
            &DescribeCodeBindingOutput::SCHEMA,
            &["CreationDate", "LastModified"],
        ),
        ("SchemaSummary", &SchemaSummary::SCHEMA, &["LastModified"]),
        (
            "SearchSchemaVersionSummary",
            &SearchSchemaVersionSummary::SCHEMA,
            &["CreatedDate"],
        ),
    ]
}

/// RULE, at the schema the codec reads: every timestamp member of every shape in the
/// catalog above reports `date-time`, and no other member reports a timestamp format at
/// all. This is the property the wire table above observes, pinned where the JSON codec
/// actually looks it up.
#[test]
fn every_modelled_timestamp_member_reports_the_date_time_format() {
    for (shape, schema, modelled) in timestamp_shapes() {
        let members = schema.members();
        let timestamps: Vec<&str> = members
            .iter()
            .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
            .map(|member| member.member_name().unwrap_or("<unnamed>"))
            .collect();
        assert_eq!(
            timestamps,
            modelled.to_vec(),
            "case {shape:?}: expected the modelled timestamp members {modelled:?}, got \
             {timestamps:?}"
        );

        for (position, member) in members.iter().enumerate() {
            let name = member.member_name().unwrap_or("<unnamed>");
            // The codec dispatches on the index, so it has to be the member's position.
            assert_eq!(
                member.member_index(),
                Some(position),
                "case {shape:?}.{name}: member_index must equal the member's position, \
                 got {:?}",
                member.member_index()
            );
            let format = member.timestamp_format().map(|carried| carried.format());
            if matches!(member.shape_type(), ShapeType::Timestamp) {
                assert_eq!(
                    format,
                    Some(TimestampFormat::DateTime),
                    "case {shape:?}.{name}: a `__timestampIso8601` member must report \
                     date-time, got {format:?}"
                );
            } else {
                assert_eq!(
                    format, None,
                    "case {shape:?}.{name}: only a timestamp member carries a timestamp \
                     format, got {format:?}"
                );
            }
        }
    }
}
