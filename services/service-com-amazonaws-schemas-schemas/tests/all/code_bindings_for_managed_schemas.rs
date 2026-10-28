// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for code bindings generated from the schemas of a
//! service-owned registry.
//!
//! RULE under test: a code binding is code generated FROM a schema version for the
//! caller that asked for it, so starting one changes nothing a registry holds — the
//! code-binding operations are therefore served for every schema a caller may READ,
//! the published AWS event catalogue included, while every call that would change what
//! a registry holds (its own metadata, a schema, a schema version, a tag, its policy)
//! stays refused with HTTP 403 `ForbiddenException` for a service-owned registry.
//!
//! The refusal side of that rule is the table in
//! [`crate::registry_write_resolution`]; this suite is the other side of it — the one
//! call on a service-owned schema that is SERVED, and the proof that being served
//! leaves the AWS-owned schema byte-for-byte as it was. The two refusals are asserted
//! here too, per row, so the contrast is a single case rather than two suites that
//! have to be read together.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it, on the
//! harness in [`crate::registry_schema_lifecycle`]. Nothing here touches the state
//! struct directly, so a handler that quietly rewrote the catalogue would still have to
//! make the reads that follow agree.

use std::time::Instant;

use serde_json::json;
use service_com_amazonaws_schemas_schemas::ops::types::CODE_GENERATION_STATUS_VALUES;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, managed_version_count, AWS_MANAGED_REGISTRIES, AWS_MANAGED_REGISTRY,
    CODE_FORBIDDEN, CODE_GENERATION_DURATION, CODE_GENERATION_IN_PROGRESS,
};

use crate::registry_schema_lifecycle::{
    listed_versions, schema_uri, versions_uri, Answer, Emulator,
};
use crate::registry_write_resolution::service_owned_sentence;

/// One code binding this suite generates from a schema of the AWS-owned catalogue.
#[derive(Debug)]
struct ManagedBindingCase {
    case: &'static str,
    /// A schema of the service-owned catalogue, by its published name.
    schema: &'static str,
    /// One of the languages the service generates bindings for.
    language: &'static str,
    /// The `schemaVersion` query member, absent for "the latest version".
    requested_version: Option<&'static str>,
}

/// The table: rows differ along what the rule generalises over — which schema of the
/// catalogue the binding is generated from (its publishing source, whether the event
/// type is the API-call event every source carries or a service-specific one, and
/// whether its document is flat or nested), the language asked for, how many versions
/// the published schema has, and whether the request names the version explicitly or
/// leaves it at the latest.
///
/// Source: `state::AWS_EVENT_SOURCES` and `state::AWS_SERVICE_EVENT_TYPES` for the
/// names, `state::MANAGED_VERSION_COUNTS` (read through
/// `state::managed_version_count`) for the entries published at more than one version,
/// and `state::CODE_BINDING_LANGUAGES` for the languages. Each name is checked against
/// the catalogue below, so a row can never pass by naming a schema that is not there.
/// The first row is the entry the issue cited; every other row is one it did not.
fn managed_binding_cases() -> Vec<ManagedBindingCase> {
    vec![
        ManagedBindingCase {
            case: "multi-version-entry-at-its-latest-version",
            schema: "aws.a4b@RoomStateChange",
            language: "Java8",
            requested_version: None,
        },
        ManagedBindingCase {
            case: "multi-version-entry-at-its-earliest-version",
            schema: "aws.a4b@RoomStateChange",
            language: "Python36",
            requested_version: Some("1"),
        },
        ManagedBindingCase {
            case: "multi-version-entry-at-a-middle-version",
            schema: "aws.acm@ACMCertificateAvailable",
            language: "Go1",
            requested_version: Some("2"),
        },
        ManagedBindingCase {
            case: "api-call-event-of-a-cloud-trail-source",
            schema: "aws.a4b@AWSAPICallviaCloudTrail",
            language: "TypeScript3",
            requested_version: None,
        },
        ManagedBindingCase {
            case: "service-event-with-a-nested-document",
            schema: "aws.s3@ObjectCreated",
            language: "Go1",
            requested_version: Some("1"),
        },
        ManagedBindingCase {
            case: "service-event-with-an-empty-detail",
            schema: "aws.events@ScheduledEvent",
            language: "Python36",
            requested_version: None,
        },
        ManagedBindingCase {
            case: "service-event-with-several-referenced-components",
            schema: "aws.health@AWSHealthEvent",
            language: "TypeScript3",
            requested_version: None,
        },
    ]
}

impl ManagedBindingCase {
    /// The version number the binding this row generates must report: the one the
    /// request named, else the latest version the catalogue publishes for the schema.
    fn expected_version(&self) -> String {
        match self.requested_version {
            Some(version) => version.to_string(),
            None => managed_version_count(self.schema).to_string(),
        }
    }

    /// Fail the row unless the catalogue really publishes the schema it names.
    fn assert_catalogued(&self) {
        assert!(
            aws_managed_schemas()
                .iter()
                .any(|entry| entry.name == self.schema),
            "case {:?}: the catalogue must publish {:?} for this row to mean anything",
            self.case,
            self.schema
        );
    }
}

/// `PutCodeBinding` / `DescribeCodeBinding` both live at this URI, with the optional
/// `schemaVersion` query member both of them bind.
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
        "case {case:?}: {status:?} is not one of the modelled CodeGenerationStatus values \
         {CODE_GENERATION_STATUS_VALUES:?}, in {:?}",
        answer.body
    );
    status
}

/// Describe one schema, asserting the read succeeded, and answer what it reported.
async fn describe_schema(emulator: &Emulator, registry: &str, schema: &str, case: &str) -> Answer {
    let described = emulator
        .call("GET", &schema_uri(registry, schema), None)
        .await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: a caller may read a service-owned schema, got {} {:?}",
        described.status, described.body
    );
    described
}

/// RULE: code generation is SERVED for a schema of a service-owned registry — the put
/// starts the binding, a describe of the same key reports the stored binding, and the
/// AWS-owned schema it was generated from is left exactly as the catalogue published
/// it.
#[tokio::test]
async fn code_bindings_are_generated_from_the_schemas_of_a_service_owned_registry() {
    let emulator = Emulator::new();

    for case in managed_binding_cases() {
        case.assert_catalogued();
        let label = case.case;
        let uri = binding_uri(
            AWS_MANAGED_REGISTRY,
            case.schema,
            case.language,
            case.requested_version,
        );

        // What the catalogue published, before anything is generated from it.
        let schema_before = describe_schema(&emulator, AWS_MANAGED_REGISTRY, case.schema, label).await;
        let versions_before = emulator
            .call("GET", &versions_uri(AWS_MANAGED_REGISTRY, case.schema), None)
            .await;
        assert_eq!(
            versions_before.status, 200,
            "case {label:?}: expected 200 from ListSchemaVersions, got {} {:?}",
            versions_before.status, versions_before.body
        );

        // Nothing is generated until a put asks for it, here as anywhere else.
        let absent = emulator.call("GET", &uri, None).await;
        assert_eq!(
            absent.status, 404,
            "case {label:?}: no binding exists before the put, got {} {:?}",
            absent.status, absent.body
        );

        let started = Instant::now();
        let put = emulator.call("POST", &uri, None).await;
        assert_eq!(
            put.status, 202,
            "case {label:?}: expected 202 from PutCodeBinding on a service-owned schema, \
             got {} {:?}",
            put.status, put.body
        );
        assert_eq!(
            modelled_status(&put, label),
            CODE_GENERATION_IN_PROGRESS,
            "case {label:?}: the put that starts generation reports it in progress, got {:?}",
            put.body
        );
        assert_eq!(
            put.str_at("SchemaVersion"),
            case.expected_version(),
            "case {label:?}: expected the resolved version {:?}, got {:?}",
            case.expected_version(),
            put.body
        );

        // The binding the put created is the one a later read reports.
        let described = emulator.call("GET", &uri, None).await;
        assert_eq!(
            described.status, 200,
            "case {label:?}: expected 200 from DescribeCodeBinding, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.value_at("CreationDate"),
            put.value_at("CreationDate"),
            "case {label:?}: the describe must report the binding the put stored, got {:?}",
            described.body
        );
        assert_eq!(
            described.str_at("SchemaVersion"),
            case.expected_version(),
            "case {label:?}: the describe must report the version the put resolved, got {:?}",
            described.body
        );
        let status = modelled_status(&described, label);
        if started.elapsed() < CODE_GENERATION_DURATION {
            assert_eq!(
                status, CODE_GENERATION_IN_PROGRESS,
                "case {label:?}: within the generation span the binding is still in progress, \
                 got {:?}",
                described.body
            );
        }

        // And the AWS-owned schema the code was generated FROM is untouched: the same
        // document, type, description, version and timestamps the catalogue published.
        let schema_after = describe_schema(&emulator, AWS_MANAGED_REGISTRY, case.schema, label).await;
        assert_eq!(
            schema_after.body, schema_before.body,
            "case {label:?}: generating a binding must leave the published schema alone, \
             got {:?} after {:?}",
            schema_after.body, schema_before.body
        );
        let versions_after = emulator
            .call("GET", &versions_uri(AWS_MANAGED_REGISTRY, case.schema), None)
            .await;
        assert_eq!(
            listed_versions(&versions_after),
            listed_versions(&versions_before),
            "case {label:?}: generating a binding must add and remove no version, got {:?}",
            versions_after.body
        );
    }
}

/// RULE, the other side of it: being served a code binding changes nothing about who
/// may write to the registry — every call that would change what the service-owned
/// registry HOLDS is still refused 403 `ForbiddenException` with the service's one
/// sentence for that refusal, for the very schema a binding was just generated from,
/// and the binding and the schema both survive the refusals.
#[tokio::test]
async fn a_generated_binding_does_not_make_the_catalogue_writable() {
    for registry in AWS_MANAGED_REGISTRIES {
        for case in managed_binding_cases() {
            case.assert_catalogued();
            let label = format!("{}/{registry}", case.case);
            let uri = binding_uri(registry, case.schema, case.language, case.requested_version);
            let emulator = Emulator::new();

            let put = emulator.call("POST", &uri, None).await;
            assert_eq!(
                put.status, 202,
                "case {label:?}: expected 202 from PutCodeBinding, got {} {:?}",
                put.status, put.body
            );

            // Rows: the writes that change what the registry holds, on the same schema
            // and the same registry — the schema's content, the schema itself, one of
            // its versions, and the registry's own description.
            let document = json!({
                "openapi": "3.0.0",
                "info": {"title": "written by a caller", "version": "1.0.0"},
                "paths": {},
            })
            .to_string();
            let content_writes = [
                (
                    "UpdateSchema",
                    "PUT",
                    schema_uri(registry, case.schema),
                    Some(json!({"Content": document, "Type": "OpenApi3"})),
                ),
                (
                    "DeleteSchema",
                    "DELETE",
                    schema_uri(registry, case.schema),
                    None,
                ),
                (
                    "DeleteSchemaVersion",
                    "DELETE",
                    format!(
                        "{}/version/{}",
                        schema_uri(registry, case.schema),
                        case.expected_version()
                    ),
                    None,
                ),
                (
                    "UpdateRegistry",
                    "PUT",
                    format!("/v1/registries/name/{registry}"),
                    Some(json!({"Description": "rewritten by a caller"})),
                ),
            ];
            for (operation, method, write_uri, body) in content_writes {
                let refused = emulator.call(method, &write_uri, body).await;
                assert_eq!(
                    refused.status, 403,
                    "case {label:?}/{operation}: a write to the registry's content is still \
                     refused, got {} {:?}",
                    refused.status, refused.body
                );
                assert_eq!(
                    refused.error_code(),
                    "ForbiddenException",
                    "case {label:?}/{operation}: expected ForbiddenException, got {:?}",
                    refused.body
                );
                assert_eq!(
                    refused.str_at("Code"),
                    CODE_FORBIDDEN,
                    "case {label:?}/{operation}: expected the {CODE_FORBIDDEN} condition code, \
                     got {:?}",
                    refused.body
                );
                assert_eq!(
                    refused.str_at("Message"),
                    service_owned_sentence(registry),
                    "case {label:?}/{operation}: wrong sentence for a write to a service-owned \
                     registry, got {:?}",
                    refused.body
                );
            }

            // The refusals took nothing with them: the schema is still readable and the
            // binding generated from it is still reported.
            describe_schema(&emulator, registry, case.schema, &label).await;
            let still_there = emulator.call("GET", &uri, None).await;
            assert_eq!(
                still_there.status, 200,
                "case {label:?}: the generated binding must survive the refusals, got {} {:?}",
                still_there.status, still_there.body
            );
            assert_eq!(
                still_there.value_at("CreationDate"),
                put.value_at("CreationDate"),
                "case {label:?}: the binding must still report its stored creation date, got {:?}",
                still_there.body
            );
        }
    }
}

/// RULE: a binding generated from a service-owned schema is keyed like any other — by
/// that schema, that version and that language — so a put for one key leaves every
/// other key of the catalogue as absent as before.
#[tokio::test]
async fn a_binding_on_a_managed_schema_is_keyed_like_any_other() {
    let emulator = Emulator::new();
    let generated = ManagedBindingCase {
        case: "keyed-binding",
        schema: "aws.s3@ObjectCreated",
        language: "Java8",
        requested_version: None,
    };
    generated.assert_catalogued();
    let put = emulator
        .call(
            "POST",
            &binding_uri(AWS_MANAGED_REGISTRY, generated.schema, generated.language, None),
            None,
        )
        .await;
    assert_eq!(
        put.status, 202,
        "expected 202 from PutCodeBinding, got {} {:?}",
        put.status, put.body
    );

    // Rows: keys that differ from the one the put created in exactly one component —
    // the language, the schema, and the version.
    for (case, schema, language, version) in [
        ("other-language", generated.schema, "Go1", None),
        ("other-schema", "aws.ec2@EC2InstanceStateChangeNotification", "Java8", None),
        (
            "other-schema-of-a-multi-version-entry",
            "aws.a4b@RoomStateChange",
            "Java8",
            None,
        ),
        ("version-the-schema-does-not-hold", generated.schema, "Java8", Some("9")),
    ] {
        let answer = emulator
            .call(
                "GET",
                &binding_uri(AWS_MANAGED_REGISTRY, schema, language, version),
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
}
