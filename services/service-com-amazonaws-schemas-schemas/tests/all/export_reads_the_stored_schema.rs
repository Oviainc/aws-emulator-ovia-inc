// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHAT `ExportSchema` serves and what it refuses: the schema
//! must be one the SERVICE wrote, and the `Type` must name the one language an export is
//! rendered into.
//!
//! RULES under test:
//!
//! * `Type` names the language the export is rendered INTO, and the service renders one —
//!   JSON Schema draft 4 — so any other value is refused 400 `BadRequestException` on the
//!   REQUEST, before the resource it names is resolved, with the sentence this service
//!   words a value outside a member's admitted set with;
//! * an export is the service's own rendering of a document the SERVICE authored (one AWS
//!   publishes, one schema discovery wrote), so a schema a caller authored has no export
//!   and asking for one is refused 403 `ForbiddenException`;
//! * where both hold, the export renders the document of the VERSION it resolves — the
//!   one the request names, or the latest the schema holds when it names none — and the
//!   registry, schema and version are resolved in that order, each step answering its own
//!   404.
//!
//! Every case drives the real wire path through the harness in
//! `registry_schema_lifecycle.rs`: the schemas are created over HTTP, exported over HTTP
//! and read back through `DescribeSchema`, so an export computed from anything but the
//! stored document would have to make a later read agree.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::TYPE_VALUES;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, ManagedSchema, AWS_MANAGED_REGISTRY, CODE_FORBIDDEN,
    CODE_MALFORMED_REQUEST, CODE_RESOURCE_NOT_FOUND, JSON_SCHEMA_DRAFT4_URI,
};

use crate::registry_schema_lifecycle::{create_registry, export_uri, schema_uri, Answer, Emulator};
use crate::resource_tags::percent_encode;

/// The schema language the export reads FROM. Source: `com.amazonaws.schemas#Type`,
/// re-exported as [`TYPE_VALUES`].
const OPEN_API_3: &str = "OpenApi3";

/// The one language an export is rendered INTO. Source: as [`OPEN_API_3`] for the
/// spelling; that this is the only export target is the rule under test.
const DRAFT4: &str = "JSONSchemaDraft4";

/// An OpenAPI 3 document with a single component and a title — the smallest document
/// the conversion would have something to render.
const ONE_COMPONENT: &str = concat!(
    r#"{"openapi":"3.0.0","info":{"title":"Order Placed"},"#,
    r#""components":{"schemas":{"AWSEvent":{"type":"object"}}}}"#
);

/// An OpenAPI 3 document whose components reference each other, directly and through
/// an array, and which carries a nullable member.
const NESTED_REFS: &str = concat!(
    r#"{"openapi":"3.0.0","info":{"title":"Order Placed published by my.orders"},"#,
    r#""components":{"schemas":{"#,
    r#""AWSEvent":{"type":"object","properties":{"#,
    r##""detail":{"$ref":"#/components/schemas/Detail"},"##,
    r##""resources":{"type":"array","items":{"$ref":"#/components/schemas/Resource"}}}},"##,
    r#""Detail":{"type":"object","properties":{"id":{"type":"string"},"#,
    r#""extra":{"type":"object","nullable":true}}},"#,
    r#""Resource":{"type":"string"}}}}"#
);

/// A JSON Schema draft-4 document, for a row stored in the language an export produces.
const ALREADY_DRAFT4: &str = concat!(
    r#"{"$schema":"http://json-schema.org/draft-04/schema#","title":"Order Placed","#,
    r#""type":"object","properties":{"id":{"type":"string"}}}"#
);

/// One schema a caller created: a row of the eligibility rule's table.
struct Authored {
    case: &'static str,
    registry: &'static str,
    schema: &'static str,
    /// A member of `TYPE_VALUES`: the language the document is STORED as.
    stored_as: &'static str,
    document: &'static str,
    /// How many versions the row's schema holds, so the refusal does not turn on the
    /// schema holding exactly one definition.
    versions: u32,
}

/// The schemas a caller authored, differing along what the eligibility rule generalises
/// over: the registry, the stored schema language, what the document carries, and how
/// many versions the schema holds.
const AUTHORED: &[Authored] = &[
    Authored {
        case: "a caller's own single-component OpenAPI 3 document",
        registry: "mine",
        schema: "order-placed",
        stored_as: OPEN_API_3,
        document: ONE_COMPONENT,
        versions: 1,
    },
    Authored {
        case: "a caller's own document whose components reference each other",
        registry: "mine",
        schema: "order-placed-detailed",
        stored_as: OPEN_API_3,
        document: NESTED_REFS,
        versions: 3,
    },
    Authored {
        case: "a second registry of the caller's",
        registry: "another-registry",
        schema: "shipment-delayed",
        stored_as: OPEN_API_3,
        document: ONE_COMPONENT,
        versions: 2,
    },
    Authored {
        case: "a caller's document stored in the language an export produces",
        registry: "mine",
        schema: "already-draft4",
        stored_as: DRAFT4,
        document: ALREADY_DRAFT4,
        versions: 1,
    },
];

/// Create one schema over the wire with `versions` definitions, asserting it was stored.
async fn create_schema(emulator: &Emulator, row: &Authored) {
    let created = emulator
        .call(
            "POST",
            &schema_uri(row.registry, row.schema),
            Some(json!({"Content": row.document, "Type": row.stored_as})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateSchema, got {} {:?}",
        row.case, created.status, created.body
    );
    for round in 2..=row.versions {
        let updated = emulator
            .call(
                "PUT",
                &schema_uri(row.registry, row.schema),
                Some(json!({"Content": row.document, "Type": row.stored_as})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 writing version {round}, got {} {:?}",
            row.case, updated.status, updated.body
        );
    }
}

/// Create every registry the authored rows live in, and every row in it.
async fn seed_authored(emulator: &Emulator) {
    for registry in ["mine", "another-registry"] {
        create_registry(emulator, registry).await;
    }
    for row in AUTHORED {
        create_schema(emulator, row).await;
    }
}

/// A member of the published catalogue holding more than one version, so a case can name
/// each of them. Read from the catalogue rather than written out here.
fn published_with_several_versions() -> ManagedSchema {
    aws_managed_schemas()
        .iter()
        .find(|entry| entry.version_count > 1)
        .expect("the catalogue publishes a schema at more than one version")
        .clone()
}

/// The service's sentence for a version the named schema does not hold.
fn no_such_version_sentence(schema: &str, version: &str) -> String {
    format!("Schema with name {schema} and version {version} does not exist.")
}

/// RULE: an export is served only for a schema the service itself wrote, so every schema
/// a caller authored is refused 403 `ForbiddenException` with the service's one sentence
/// for that refusal — whatever its registry, its stored language, its document or how
/// many versions it holds — while a schema AWS publishes exports.
#[tokio::test]
async fn only_a_schema_the_service_wrote_has_an_export() {
    let emulator = Emulator::new();
    seed_authored(&emulator).await;

    for row in AUTHORED {
        let case = row.case;
        // Driven with no version named and with each version named: the refusal is about
        // who wrote the schema, not about which definition is resolved.
        let mut queries: Vec<String> = vec![String::new()];
        queries.extend((1..=row.versions).map(|version| format!("&schemaVersion={version}")));
        for query in queries {
            let refused = emulator
                .call(
                    "GET",
                    &format!("{}{query}", export_uri(row.registry, row.schema, DRAFT4)),
                    None,
                )
                .await;
            assert_eq!(
                refused.status, 403,
                "case {case:?}{query}: expected 403, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ForbiddenException",
                "case {case:?}{query}: expected ForbiddenException, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Code"),
                CODE_FORBIDDEN,
                "case {case:?}{query}: expected the {CODE_FORBIDDEN} condition code, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Message"),
                "You cannot export non discovered or non aws managed schemas.",
                "case {case:?}{query}: wrong sentence for a schema the service did not \
                 write, got {:?}",
                refused.body
            );
        }

        // The refusal changed nothing: the schema is still served as the caller stored
        // it — an export is what it has not got, not the document.
        let described = emulator
            .call("GET", &schema_uri(row.registry, row.schema), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeSchema, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.str_at("Content"),
            row.document,
            "case {case:?}: expected the stored document, got {:?}",
            described.str_at("Content")
        );
    }

    // The positive control: a schema AWS publishes, in the same emulator, exports.
    let published = published_with_several_versions();
    let exported = emulator
        .call(
            "GET",
            &export_uri(AWS_MANAGED_REGISTRY, &published.name, DRAFT4),
            None,
        )
        .await;
    assert_eq!(
        exported.status, 200,
        "case {:?}: a published schema must export, got {} {:?}",
        published.name, exported.status, exported.body
    );
}

/// RULE: `Type` names the export TARGET, and the service renders one — so every other
/// value is refused 400 `BadRequestException`, naming the member it was checked at, the
/// value the request sent and the set the member admits; and it is decided on the REQUEST,
/// so a value outside the set is refused whether or not the schema it names exists.
#[tokio::test]
async fn the_only_export_target_is_json_schema_draft_4() {
    let emulator = Emulator::new();
    seed_authored(&emulator).await;
    let published = published_with_several_versions();

    // Rows: values differing from the one target along another dimension each — the other
    // language the model's `Type` enum holds (a language a schema is STORED as, which is
    // no export target), letter case, a neighbouring draft number, a truncation,
    // surrounding whitespace, quoting, two values at once, a language the service does not
    // store, and no value at all.
    let refused: &[(&str, &str)] = &[
        ("the other modelled schema language", OPEN_API_3),
        ("the target in another letter case", "jsonschemadraft4"),
        ("another draft of the same language", "JSONSchemaDraft3"),
        ("a prefix of the target", "JSONSchema"),
        ("the target with trailing whitespace", "JSONSchemaDraft4 "),
        ("the target in quotes", "\"JSONSchemaDraft4\""),
        ("both modelled values at once", "JSONSchemaDraft4,OpenApi3"),
        ("a schema language the service does not store", "Avro"),
        ("no value at all", ""),
    ];

    for (case, value) in refused {
        // Against a published schema (which an export does serve), a caller's own schema,
        // and a resource nothing holds: the value is refused on the request either way.
        for (what, uri) in [
            (
                "a published schema",
                export_uri(AWS_MANAGED_REGISTRY, &published.name, &percent_encode(value)),
            ),
            (
                "a schema the caller wrote",
                export_uri("mine", "order-placed", &percent_encode(value)),
            ),
            (
                "a registry nothing created",
                export_uri("no-such-registry", "no-such-schema", &percent_encode(value)),
            ),
        ] {
            let answer = emulator.call("GET", &uri, None).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}/{what}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {case:?}/{what}: expected BadRequestException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("Code"),
                CODE_MALFORMED_REQUEST,
                "case {case:?}/{what}: expected the {CODE_MALFORMED_REQUEST} condition \
                 code, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("Message"),
                format!(
                    "1 validation error detected: Value '{value}' at 'type' failed to \
                     satisfy constraint: Member must satisfy enum value set: [{DRAFT4}]"
                ),
                "case {case:?}/{what}: the refusal must name the value and the admitted \
                 set, got {:?}",
                answer.body
            );
        }
    }

    // The one value that is an export target is served, and the answer reports it.
    let served = emulator
        .call(
            "GET",
            &export_uri(AWS_MANAGED_REGISTRY, &published.name, DRAFT4),
            None,
        )
        .await;
    assert_eq!(
        served.status, 200,
        "the one export target must be served, got {} {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.str_at("Type"),
        DRAFT4,
        "the export reports the language it rendered, got {:?}",
        served.body
    );
    // The export target set is NARROWER than the set of languages a schema is stored as:
    // both of those are modelled `Type` values, and only one of them is a target.
    assert!(
        TYPE_VALUES.contains(&DRAFT4) && TYPE_VALUES.contains(&OPEN_API_3),
        "both languages must be modelled Type values, got {TYPE_VALUES:?}"
    );
}

/// RULE: the export renders the document of the VERSION it resolves — the one the request
/// names, the latest the schema holds when the member is absent, and the same for a member
/// sent with nothing in it — and a `SchemaVersion` that names no stored version is refused
/// 404 with the service's sentence for that, whatever spelling it takes.
#[tokio::test]
async fn the_export_renders_the_version_it_resolves() {
    let emulator = Emulator::new();
    let published = published_with_several_versions();
    let name = published.name.as_str();
    let latest = published.version_count.to_string();

    // Rows: every version the catalogue publishes for this schema, then the two ways of
    // naming none — the member absent and the member sent empty, which specifies no
    // version and so resolves the latest.
    let mut cases: Vec<(String, String, String)> = (1..=published.version_count)
        .map(|version| {
            (
                format!("version {version} named"),
                format!("&schemaVersion={version}"),
                version.to_string(),
            )
        })
        .collect();
    cases.push((
        "no version named".to_string(),
        String::new(),
        latest.clone(),
    ));
    cases.push((
        "an empty version".to_string(),
        "&schemaVersion=".to_string(),
        latest.clone(),
    ));

    let source: Value = serde_json::from_str(&published.content)
        .expect("a published document is JSON");
    for (case, query, expected) in &cases {
        let uri = format!(
            "{}{query}",
            export_uri(AWS_MANAGED_REGISTRY, name, DRAFT4)
        );
        let exported = emulator.call("GET", &uri, None).await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            exported.status, exported.body
        );
        assert_eq!(
            exported.str_at("SchemaVersion"),
            *expected,
            "case {case:?}: expected version {expected}, got {:?}",
            exported.body
        );
        assert_eq!(
            exported.str_at("SchemaName"),
            name,
            "case {case:?}: the export names the schema it read, got {:?}",
            exported.body
        );

        // The document it answers is computed from the version's stored content: draft-04
        // at the root and the source document's own title.
        let converted: Value = serde_json::from_str(&exported.str_at("Content"))
            .unwrap_or_else(|e| panic!("case {case:?}: the export must be JSON: {e}"));
        assert_eq!(
            converted["$schema"].as_str(),
            Some(JSON_SCHEMA_DRAFT4_URI),
            "case {case:?}: the export names draft-04, got {converted:?}"
        );
        assert_eq!(
            converted["title"], source["info"]["title"],
            "case {case:?}: the export carries the stored document's title, got {converted:?}"
        );

        // And the members it reports are the ones the reader reports for that version.
        let described = emulator
            .call(
                "GET",
                &format!("{}?schemaVersion={expected}", schema_uri(AWS_MANAGED_REGISTRY, name)),
                None,
            )
            .await;
        assert_eq!(
            exported.body.get("SchemaArn"),
            described.body.get("SchemaArn"),
            "case {case:?}: expected the ARN DescribeSchema reports ({:?}), got {:?}",
            described.body.get("SchemaArn"),
            exported.body.get("SchemaArn")
        );
        assert_eq!(
            described.str_at("Content"),
            published.content,
            "case {case:?}: the reader serves the stored document, got {:?}",
            described.str_at("Content")
        );
    }

    // A `SchemaVersion` that names no stored version: a number past the newest, one below
    // the first, and spellings that are no rendering of a version number at all — each
    // sent percent-encoded, so the handler reads exactly the spelling the row names.
    let past_newest = (published.version_count + 1).to_string();
    let unheld: &[(&str, &str)] = &[
        ("a number past the newest", past_newest.as_str()),
        ("a number below the first", "0"),
        ("a leading plus", "+1"),
        ("a leading zero", "01"),
        ("a leading space", " 1"),
        ("a decimal point", "1.0"),
        ("a word", "latest"),
        ("a number wider than a version counter", "18446744073709551616"),
        ("a full-width digit", "\u{ff11}"),
    ];
    for (case, label) in unheld {
        let uri = format!(
            "{}&schemaVersion={}",
            export_uri(AWS_MANAGED_REGISTRY, name, DRAFT4),
            percent_encode(label)
        );
        let refused = emulator.call("GET", &uri, None).await;
        assert_not_found(case, &refused, &no_such_version_sentence(name, label));
    }

    // The registry and the schema steps of the same resolution, each answering its own
    // sentence.
    for (case, uri, sentence) in [
        (
            "a registry nothing created",
            export_uri("no-such-registry", name, DRAFT4),
            "Registry with name no-such-registry does not exist.".to_string(),
        ),
        (
            "a schema the registry does not hold",
            export_uri(AWS_MANAGED_REGISTRY, "aws.nothing@Nothing", DRAFT4),
            "Schema with name aws.nothing@Nothing does not exist.".to_string(),
        ),
    ] {
        let refused = emulator.call("GET", &uri, None).await;
        assert_not_found(case, &refused, &sentence);
    }
}

/// Assert `answer` is the 404 `NotFoundException` the read resolution answers, worded
/// with `sentence`.
fn assert_not_found(case: &str, answer: &Answer, sentence: &str) {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected 404, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_RESOURCE_NOT_FOUND,
        "case {case:?}: expected the {CODE_RESOURCE_NOT_FOUND} condition code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Message"),
        sentence,
        "case {case:?}: wrong sentence for this step of the resolution, got {:?}",
        answer.body
    );
}

/// An export REPORTS: two exports of one schema render one document, and the resource it
/// read is unchanged by the reading.
#[tokio::test]
async fn exporting_changes_nothing_a_later_reader_sees() {
    let emulator = Emulator::new();
    let published = published_with_several_versions();
    let name = published.name.as_str();

    let before = emulator
        .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
        .await;
    let first = emulator
        .call("GET", &export_uri(AWS_MANAGED_REGISTRY, name, DRAFT4), None)
        .await;
    let second = emulator
        .call("GET", &export_uri(AWS_MANAGED_REGISTRY, name, DRAFT4), None)
        .await;
    assert_eq!(
        first.body, second.body,
        "two exports of one schema must answer one document, got {:?} and {:?}",
        first.body, second.body
    );
    let after = emulator
        .call("GET", &schema_uri(AWS_MANAGED_REGISTRY, name), None)
        .await;
    assert_eq!(
        after.body, before.body,
        "exporting must leave the schema exactly as it was"
    );
}
