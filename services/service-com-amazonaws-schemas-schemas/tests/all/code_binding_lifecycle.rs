// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the code-binding lifecycle.
//!
//! RULE under test: a code binding is a resource keyed by (registry, schema, schema
//! version, language) that only `PutCodeBinding` brings into existence; it is reported
//! `CREATE_IN_PROGRESS` from the put until the generation span the service gives it has
//! elapsed and `CREATE_COMPLETE` from then on, and `DescribeCodeBinding` reports that
//! stored binding — answering `NotFoundException` (HTTP 404) for every key no put has
//! created, and changing nothing by being called.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. Nothing
//! here touches the state struct directly, so a handler that answered from its input
//! instead of from the store would still have to make a later read agree.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::ops::types::CODE_GENERATION_STATUS_VALUES;
use service_com_amazonaws_schemas_schemas::state::{
    CODE_GENERATION_COMPLETE, CODE_GENERATION_DURATION, CODE_GENERATION_IN_PROGRESS,
};

// The harness this crate's suites share: one emulator, reached over HTTP.
use crate::registry_schema_lifecycle::{create_registry, schema_uri, Answer, Emulator};

/// The registry every case below creates its schemas in.
const REGISTRY: &str = "code-binding-registry";

/// Create a schema to generate bindings from, asserting it succeeded.
async fn create_schema(emulator: &Emulator, schema: &str) {
    let created = emulator
        .call(
            "POST",
            &schema_uri(REGISTRY, schema),
            Some(json!({ "Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {schema:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
}

/// `/v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/language/{Language}`,
/// with the optional `schemaVersion` query member both code-binding operations bind.
fn binding_uri(registry: &str, schema: &str, language: &str, version: Option<&str>) -> String {
    let base = format!("{}/language/{language}", schema_uri(registry, schema));
    match version {
        Some(version) => format!("{base}?schemaVersion={version}"),
        None => base,
    }
}

/// The `Status` an answer reports, checked against the values the model admits.
fn modelled_status(answer: &Answer, case: &str) -> String {
    let status = answer.str_at("Status");
    assert!(
        CODE_GENERATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case}: {status:?} is not one of the modelled CodeGenerationStatus values \
         {CODE_GENERATION_STATUS_VALUES:?}, in {:?}",
        answer.body
    );
    status
}

/// One (schema, language, requested version) key the table drives the lifecycle for.
#[derive(Debug)]
struct BindingCase {
    case: &'static str,
    schema: &'static str,
    language: &'static str,
    /// The `schemaVersion` query member, absent for "the latest version".
    requested_version: Option<&'static str>,
    /// The version number the binding must report, in decimal.
    reported_version: &'static str,
}

/// The table: rows differ along what the rule generalises over — the language asked
/// for, whether the schema version is named explicitly or left to the latest, and the
/// shape of the schema name.
///
/// `Language` is modelled as an unconstrained `__string`
/// (`smithy_query(query="operation",operation="PutCodeBinding")` shows
/// `Language: __string, httpLabel, required`), but the SERVICE constrains it to the
/// languages it generates bindings for — see the sibling suite
/// [`crate::code_bindings`] and
/// `service_com_amazonaws_schemas_schemas::state::CODE_BINDING_LANGUAGES` — so every
/// row here names one of those, and a value outside them belongs to the refusal
/// table in that suite.
fn binding_cases() -> Vec<BindingCase> {
    vec![
        BindingCase {
            case: "latest-version-implicit",
            schema: "demo-schema",
            language: "Java8",
            requested_version: None,
            reported_version: "1",
        },
        BindingCase {
            case: "version-named-explicitly",
            schema: "demo-schema",
            language: "Python36",
            requested_version: Some("1"),
            reported_version: "1",
        },
        BindingCase {
            case: "second-language-on-the-same-schema-version",
            schema: "demo-schema",
            language: "TypeScript3",
            requested_version: None,
            reported_version: "1",
        },
        BindingCase {
            case: "dotted-schema-name",
            schema: "order.placed_v1",
            language: "Go1",
            requested_version: Some("1"),
            reported_version: "1",
        },
        BindingCase {
            case: "second-language-on-a-dotted-schema-name",
            schema: "order.placed_v1",
            language: "Python36",
            requested_version: None,
            reported_version: "1",
        },
    ]
}

/// The schemas the table's rows generate bindings from.
const SCHEMAS: [&str; 2] = ["demo-schema", "order.placed_v1"];

/// An emulator with the registry and both schemas already created.
async fn emulator_with_schemas() -> Emulator {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;
    for schema in SCHEMAS {
        create_schema(&emulator, schema).await;
    }
    emulator
}

/// RULE: only a put creates a binding, so describing a key no put has created is a
/// 404 `NotFoundException` — whether the registry, the schema, the version or the
/// language is the part that does not exist.
#[tokio::test]
async fn describing_a_binding_no_put_created_is_not_found() {
    let emulator = emulator_with_schemas().await;

    // Rows: each part of the key in turn is one nothing has created — including a
    // `schemaVersion` that is not a version number at all, and a language for which
    // the schema that DOES exist has no binding.
    for (case, registry, schema, language, version) in [
        (
            "registry-never-created",
            "no-such-registry",
            "demo-schema",
            "Java8",
            None,
        ),
        (
            "schema-never-created",
            REGISTRY,
            "no-such-schema",
            "Java8",
            None,
        ),
        (
            "version-the-schema-does-not-hold",
            REGISTRY,
            "demo-schema",
            "Java8",
            Some("2"),
        ),
        (
            "version-zero",
            REGISTRY,
            "demo-schema",
            "Java8",
            Some("0"),
        ),
        (
            "version-that-is-not-a-number",
            REGISTRY,
            "demo-schema",
            "Java8",
            Some("v1"),
        ),
        (
            "no-binding-for-this-language-yet",
            REGISTRY,
            "demo-schema",
            "Java8",
            None,
        ),
        (
            "no-binding-for-this-language-at-an-explicit-version",
            REGISTRY,
            "order.placed_v1",
            "Go1",
            Some("1"),
        ),
    ] {
        let answer = emulator
            .call("GET", &binding_uri(registry, schema, language, version), None)
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 from DescribeCodeBinding, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            answer.body
        );
    }
}

/// RULE: the put creates the binding and reports it as generation in progress, and a
/// describe of the same key reports the SAME stored binding.
#[tokio::test]
async fn a_put_creates_a_binding_that_describe_reports_identically() {
    let emulator = emulator_with_schemas().await;

    for case in binding_cases() {
        let uri = binding_uri(
            REGISTRY,
            case.schema,
            case.language,
            case.requested_version,
        );
        let started = Instant::now();
        let put = emulator.call("POST", &uri, None).await;
        assert_eq!(
            put.status, 202,
            "case {:?}: expected 202 from PutCodeBinding, got {} {:?}",
            case.case, put.status, put.body
        );
        assert_eq!(
            modelled_status(&put, case.case),
            CODE_GENERATION_IN_PROGRESS,
            "case {:?}: the put that starts generation reports it in progress, got {:?}",
            case.case,
            put.body
        );
        assert_eq!(
            put.str_at("SchemaVersion"),
            case.reported_version,
            "case {:?}: expected the resolved version {:?}, got {:?}",
            case.case,
            case.reported_version,
            put.body["SchemaVersion"]
        );
        // A binding is created once, so at the put both timestamps are the creation
        // instant.
        assert_eq!(
            put.value_at("LastModified"),
            put.value_at("CreationDate"),
            "case {:?}: a just-created binding was last modified when it was created, got {:?}",
            case.case,
            put.body
        );

        let described = emulator.call("GET", &uri, None).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected 200 from DescribeCodeBinding, got {} {:?}",
            case.case, described.status, described.body
        );
        assert_eq!(
            described.value_at("CreationDate"),
            put.value_at("CreationDate"),
            "case {:?}: describe must report the stored creation date, got {:?}",
            case.case,
            described.body
        );
        assert_eq!(
            described.str_at("SchemaVersion"),
            case.reported_version,
            "case {:?}: describe must report the version the put resolved, got {:?}",
            case.case,
            described.body
        );
        let status = modelled_status(&described, case.case);
        // Generation completes on the clock, so the status is only pinned while the
        // span has provably not elapsed yet.
        if started.elapsed() < CODE_GENERATION_DURATION {
            assert_eq!(
                status, CODE_GENERATION_IN_PROGRESS,
                "case {:?}: within the generation span the binding is still in progress, got {:?}",
                case.case, described.body
            );
        }
    }
}

/// RULE: a binding's generation completes on the CLOCK — the status moves once the
/// span has elapsed, whoever asks and however often, and a read never changes what a
/// later read sees.
#[tokio::test]
async fn a_binding_completes_on_the_clock_and_reads_do_not_change_it() {
    let emulator = emulator_with_schemas().await;

    for case in binding_cases() {
        let uri = binding_uri(
            REGISTRY,
            case.schema,
            case.language,
            case.requested_version,
        );
        let put = emulator.call("POST", &uri, None).await;
        assert_eq!(
            put.status, 202,
            "case {:?}: expected 202 from PutCodeBinding, got {} {:?}",
            case.case, put.status, put.body
        );

        // Nothing polls generation to completion: the due instant was fixed at the
        // put, so waiting past it is all that is needed.
        tokio::time::sleep(CODE_GENERATION_DURATION + Duration::from_millis(50)).await;

        let first = emulator.call("GET", &uri, None).await;
        assert_eq!(
            modelled_status(&first, case.case),
            CODE_GENERATION_COMPLETE,
            "case {:?}: past the generation span the binding is complete, got {:?}",
            case.case,
            first.body
        );
        let second = emulator.call("GET", &uri, None).await;
        assert_eq!(
            second.body, first.body,
            "case {:?}: two describes in a row must report the same binding, got {:?} then {:?}",
            case.case, first.body, second.body
        );
        assert_eq!(
            first.value_at("CreationDate"),
            put.value_at("CreationDate"),
            "case {:?}: completing generation must not move the creation date, got {:?}",
            case.case,
            first.body
        );
    }
}

/// RULE: the binding for one version and language is ONE resource — a repeat put
/// reports the generation already under way (or already finished) rather than starting
/// a second one, so the creation date never moves and a completed binding never falls
/// back to in progress.
#[tokio::test]
async fn a_repeat_put_does_not_restart_generation() {
    let emulator = emulator_with_schemas().await;
    let case = "repeat-put";
    let uri = binding_uri(REGISTRY, "demo-schema", "Java8", None);

    let first = emulator.call("POST", &uri, None).await;
    assert_eq!(
        first.status, 202,
        "case {case:?}: expected 202 from PutCodeBinding, got {} {:?}",
        first.status, first.body
    );
    let immediate_repeat = emulator.call("POST", &uri, None).await;
    assert_eq!(
        immediate_repeat.value_at("CreationDate"),
        first.value_at("CreationDate"),
        "case {case:?}: a repeat put must report the first put's creation date, got {:?}",
        immediate_repeat.body
    );

    tokio::time::sleep(CODE_GENERATION_DURATION + Duration::from_millis(50)).await;

    let after_completion = emulator.call("POST", &uri, None).await;
    assert_eq!(
        after_completion.status, 202,
        "case {case:?}: expected 202 from the repeat PutCodeBinding, got {} {:?}",
        after_completion.status, after_completion.body
    );
    assert_eq!(
        modelled_status(&after_completion, case),
        CODE_GENERATION_COMPLETE,
        "case {case:?}: a put against a completed binding reports it complete, got {:?}",
        after_completion.body
    );
    assert_eq!(
        after_completion.value_at("CreationDate"),
        first.value_at("CreationDate"),
        "case {case:?}: no put after the first moves the creation date, got {:?}",
        after_completion.body
    );
    let described = emulator.call("GET", &uri, None).await;
    assert_eq!(
        described.body, after_completion.body,
        "case {case:?}: describe and put report the same stored binding, got {:?} then {:?}",
        after_completion.body, described.body
    );
}

/// RULE: a binding is keyed by schema version AND language, so a put for one key
/// leaves every other key as absent as before — and the key a request with no
/// `schemaVersion` names is the latest version, which is the same key as naming that
/// version explicitly.
#[tokio::test]
async fn a_binding_is_keyed_by_version_and_language() {
    let emulator = emulator_with_schemas().await;
    let schema = "demo-schema";
    let put = emulator
        .call("POST", &binding_uri(REGISTRY, schema, "Java8", None), None)
        .await;
    assert_eq!(
        put.status, 202,
        "expected 202 from PutCodeBinding, got {} {:?}",
        put.status, put.body
    );

    // The same key, named the other way: the schema holds one version, so "latest"
    // and version 1 are the same binding.
    let by_explicit_version = emulator
        .call(
            "GET",
            &binding_uri(REGISTRY, schema, "Java8", Some("1")),
            None,
        )
        .await;
    assert_eq!(
        by_explicit_version.status, 200,
        "case \"explicit-version-names-the-same-binding\": expected 200, got {} {:?}",
        by_explicit_version.status, by_explicit_version.body
    );
    assert_eq!(
        by_explicit_version.value_at("CreationDate"),
        put.value_at("CreationDate"),
        "case \"explicit-version-names-the-same-binding\": expected the stored binding, got {:?}",
        by_explicit_version.body
    );

    // Rows: keys that differ from the one the put created in exactly one component.
    for (case, other_schema, language, version) in [
        ("other-language", schema, "Python36", None),
        ("other-language-explicit-version", schema, "Go1", Some("1")),
        ("other-schema", "order.placed_v1", "Java8", None),
        ("other-version", schema, "Java8", Some("3")),
    ] {
        let answer = emulator
            .call(
                "GET",
                &binding_uri(REGISTRY, other_schema, language, version),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a key no put created must still be absent, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            answer.body
        );
    }

    // And the one key that WAS created is still there, unaffected by the reads above.
    let described = emulator
        .call("GET", &binding_uri(REGISTRY, schema, "Java8", None), None)
        .await;
    assert_eq!(
        described.status, 200,
        "the created binding must survive reads of absent keys, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.value_at("CreationDate"),
        put.value_at("CreationDate"),
        "the created binding must still report its stored creation date, got {:?}",
        described.body
    );
}

/// `PutCodeBinding` against a registry, schema or version nothing created is a 404 as
/// well: the put creates the BINDING, never the schema it binds.
#[tokio::test]
async fn a_put_against_a_missing_schema_or_version_is_not_found() {
    let emulator = emulator_with_schemas().await;

    for (case, registry, schema, language, version) in [
        (
            "registry-never-created",
            "no-such-registry",
            "demo-schema",
            "Java8",
            None,
        ),
        (
            "schema-never-created",
            REGISTRY,
            "no-such-schema",
            "Java8",
            None,
        ),
        (
            "version-the-schema-does-not-hold",
            REGISTRY,
            "demo-schema",
            "Java8",
            Some("7"),
        ),
        (
            "version-that-is-not-a-number",
            REGISTRY,
            "demo-schema",
            "Java8",
            Some("latest"),
        ),
    ] {
        let uri = binding_uri(registry, schema, language, version);
        let answer = emulator.call("POST", &uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 from PutCodeBinding, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {:?}",
            answer.body
        );
        // And the refusal persisted nothing: the describe of that key is still a 404.
        let described = emulator.call("GET", &uri, None).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused put must persist no binding, got {} {:?}",
            described.status, described.body
        );
    }
}

/// The reported `SchemaVersion` is the stored version number in decimal, so a client
/// can feed it straight back as `schemaVersion` and reach the same binding.
#[tokio::test]
async fn the_reported_version_round_trips_as_a_request_member() {
    let emulator = emulator_with_schemas().await;

    for (case, schema, language) in [
        ("java-on-demo-schema", "demo-schema", "Java8"),
        ("typescript-on-dotted-schema", "order.placed_v1", "TypeScript3"),
    ] {
        let put = emulator
            .call("POST", &binding_uri(REGISTRY, schema, language, None), None)
            .await;
        let reported: Value = put.value_at("SchemaVersion").clone();
        let version = put.str_at("SchemaVersion");
        let described = emulator
            .call(
                "GET",
                &binding_uri(REGISTRY, schema, language, Some(&version)),
                None,
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the reported SchemaVersion {version:?} must name the same binding, \
             got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.value_at("SchemaVersion"),
            &reported,
            "case {case:?}: expected SchemaVersion {reported:?}, got {:?}",
            described.body
        );
    }
}
