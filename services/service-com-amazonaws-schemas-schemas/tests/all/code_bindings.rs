// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `Language` the code-binding operations take.
//!
//! RULE under test: `PutCodeBinding` and `DescribeCodeBinding` generate and report
//! bindings only for the languages the service generates code for — the alternation
//! `state::CODE_BINDING_LANGUAGES` holds — and refuse any other `Language` with HTTP
//! 400 `BadRequestException`, naming the parameter and the pattern it must match.
//! That refusal is a property of the REQUEST, so it does not depend on the registry,
//! the schema or the version existing, and it persists nothing.
//!
//! The lifecycle of a binding once it is started lives in the sibling suite
//! [`super::code_binding_lifecycle`]; this suite is about which `Language` values get
//! that far. Both drive the real wire path through `MultiServiceRouter::dispatch` on
//! the harness in [`super::registry_schema_lifecycle`].

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    code_binding_language_pattern, CODE_BINDING_LANGUAGES, CODE_BINDING_LANGUAGE_PARAMETER,
    CODE_GENERATION_IN_PROGRESS,
};

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Emulator};

/// The registry every case below works in.
const REGISTRY: &str = "code-binding-language-registry";

/// The schema every case below generates bindings from.
const SCHEMA: &str = "demo-schema";

/// `PutCodeBinding` / `DescribeCodeBinding` both live at this URI.
fn binding_uri(registry: &str, schema: &str, language: &str) -> String {
    format!("{}/language/{language}", schema_uri(registry, schema))
}

/// The same URI with an explicit `SchemaVersion`.
fn binding_uri_at_version(
    registry: &str,
    schema: &str,
    language: &str,
    version: &str,
) -> String {
    format!(
        "{}?schemaVersion={version}",
        binding_uri(registry, schema, language)
    )
}

/// An emulator holding one registry with one schema — what a code binding is
/// generated from.
async fn emulator_with_schema() -> Emulator {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(REGISTRY, SCHEMA),
            Some(json!({ "Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    emulator
}

/// RULE: every language the service generates for is accepted — a put starts the
/// binding and a describe of the same key reports it — whichever language it is.
#[tokio::test]
async fn every_catalogued_language_is_generated_and_reads_back() {
    let emulator = emulator_with_schema().await;

    // Rows: the whole catalog, so a language added to it cannot go untried.
    for language in CODE_BINDING_LANGUAGES {
        let uri = binding_uri(REGISTRY, SCHEMA, language);

        let before = emulator.call("GET", &uri, None).await;
        assert_eq!(
            before.status, 404,
            "case {language:?}: nothing is generated until a put asks for it, got {} {:?}",
            before.status, before.body
        );

        let put = emulator.call("POST", &uri, None).await;
        assert_eq!(
            put.status, 202,
            "case {language:?}: expected 202 from PutCodeBinding, got {} {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.str_at("Status"),
            CODE_GENERATION_IN_PROGRESS,
            "case {language:?}: the put that starts generation reports it in progress, \
             got {:?}",
            put.body
        );

        let described = emulator.call("GET", &uri, None).await;
        assert_eq!(
            described.status, 200,
            "case {language:?}: expected 200 from DescribeCodeBinding, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.value_at("CreationDate"),
            put.value_at("CreationDate"),
            "case {language:?}: the describe must report the binding the put stored, \
             got {:?}",
            described.body
        );
    }
}

/// The catalog is the service's own constraint, stated as the pattern its refusal
/// quotes: this pins both, so neither can drift without a test saying so.
#[test]
fn the_catalogued_languages_are_the_pattern_the_service_publishes() {
    let covered: BTreeSet<&str> = CODE_BINDING_LANGUAGES.iter().copied().collect();
    assert_eq!(
        covered.len(),
        CODE_BINDING_LANGUAGES.len(),
        "the language catalog must not repeat a language, got {CODE_BINDING_LANGUAGES:?}"
    );
    // The languages the service's own validation pattern admits; see
    // `service_com_amazonaws_schemas_schemas::state::CODE_BINDING_LANGUAGES` for the
    // source of this list.
    for expected in ["Java8", "Python36", "TypeScript3", "Go1"] {
        assert!(
            covered.contains(expected),
            "case {expected:?}: the catalog must hold every language this service \
             generates bindings for, got {CODE_BINDING_LANGUAGES:?}"
        );
    }
    assert_eq!(
        code_binding_language_pattern(),
        "^(Java8|TypeScript3|Python36|Go1)$",
        "the pattern the refusal quotes is the catalog's alternation, got {}",
        code_binding_language_pattern()
    );
}

/// RULE: a `Language` outside the catalog is refused with HTTP 400
/// `BadRequestException` whose message names the parameter and the pattern — by both
/// code-binding operations, whether or not the registry, schema and version it names
/// exist, and without persisting anything.
#[tokio::test]
async fn a_language_outside_the_catalog_is_refused() {
    let emulator = emulator_with_schema().await;

    // Rows: a language with no code-binding support at all, a bare language family
    // without the version suffix the identifier carries, a supported language at an
    // unsupported version, the wrong case of a supported one, and a supported one with
    // surrounding space — every near miss of the anchored pattern.
    let unsupported = [
        "Cobol85",
        "Ruby31",
        "Rust1",
        "Csharp",
        "Java",
        "Python",
        "TypeScript",
        "Go",
        "Java11",
        "Python27",
        "java8",
        "JAVA8",
        "typescript3",
        // Percent-encoded space on either side: the label is a supported identifier
        // with surrounding whitespace, which the anchored pattern does not admit
        // whether or not the router decodes the escape before the handler sees it.
        "Java8%20",
        "%20Java8",
    ];

    for language in unsupported {
        // Against the existing schema, against a schema and a registry that do not
        // exist, and against a version the schema does not hold: the language is
        // refused first in every one of them.
        let targets = [
            (
                "existing schema",
                binding_uri(REGISTRY, SCHEMA, language),
            ),
            (
                "absent schema",
                binding_uri(REGISTRY, "ghost-schema", language),
            ),
            (
                "absent registry",
                binding_uri("ghost-registry", SCHEMA, language),
            ),
            (
                "absent schema version",
                binding_uri_at_version(REGISTRY, SCHEMA, language, "7"),
            ),
        ];
        for (target, uri) in targets {
            for (operation, method) in [("PutCodeBinding", "POST"), ("DescribeCodeBinding", "GET")]
            {
                let case = format!("{language:?} / {target} / {operation}");
                let answer = emulator.call(method, &uri, None).await;
                assert_eq!(
                    answer.status, 400,
                    "case {case}: an unsupported Language is a 400, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "BadRequestException",
                    "case {case}: expected BadRequestException, got {:?}",
                    answer.body
                );
                let message = answer
                    .body
                    .get("Message")
                    .or_else(|| answer.body.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // The refusal states the constraint that was checked: the parameter
                // and the pattern an acceptable value matches, both derived from the
                // catalog rather than written out here. The literal text that pattern
                // renders to is pinned once, by
                // `the_catalogued_languages_are_the_pattern_the_service_publishes`,
                // so this equality is not a tautology against the implementation.
                assert_eq!(
                    message,
                    format!(
                        "Parameter {CODE_BINDING_LANGUAGE_PARAMETER} doesn't match pattern {}",
                        code_binding_language_pattern()
                    ),
                    "case {case}: the refusal must name the parameter and the pattern \
                     it must match, got {message:?}"
                );
            }
        }
    }

    // And none of those refusals persisted anything: every catalogued language still
    // has no binding to describe.
    for language in CODE_BINDING_LANGUAGES {
        let answer = emulator
            .call("GET", &binding_uri(REGISTRY, SCHEMA, language), None)
            .await;
        assert_eq!(
            answer.status, 404,
            "case {language:?}: a refused request must persist nothing, got {} {:?}",
            answer.status, answer.body
        );
    }
}
