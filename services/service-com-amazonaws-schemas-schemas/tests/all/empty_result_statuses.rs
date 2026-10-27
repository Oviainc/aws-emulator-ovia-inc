// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH STATUS an operation of this service that answers no
//! body answers with.
//!
//! RULE under test: the success status is a fact of the OPERATION, carried on its own
//! `SUCCESS_STATUS` and never inferred from the response having no members to send —
//! this service removes a resource with a bodiless `204`, while every method served on
//! its resource-policy endpoint answers `200`, the removal of the policy included, so a
//! caller that branches on the status (or reads the body of a `200`) takes the same path
//! on all three of them. Whatever the status, an answer with no body states its length
//! with `content-length: 0`.
//!
//! The rule is asserted over a TABLE spanning the class rather than over the one
//! operation a recording happened to cover: every implemented operation of this service
//! whose output shape is `Unit` is a row
//! (`smithy_query(query="digest",model_path="services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json")`
//! names them), reached on the resource kind it names — a registry, a schema, one
//! version of a schema, a discoverer, a tag set and a resource policy.
//!
//! Every case but the last drives the real wire path in process: a signed `restJson1`
//! request through `MultiServiceRouter::dispatch`, exactly as the production router
//! serves it. The last case serves the router on a real socket, where the status line
//! and the framing header are written by the HTTP layer. The HTTP harness is the
//! crate's one harness, imported from `registry_schema_lifecycle`; the policy route
//! comes from `resource_policy`, the tag routes and the create helpers from
//! `resource_tags`, and the socket exchange from `empty_response_framing`.

use serde_json::json;

use crate::empty_response_framing::raw_request;
use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_uri, Answer, Emulator,
};
use crate::resource_policy::policy_uri;
use crate::resource_tags::{
    create_tagged, discoverer_uri, percent_encode, source_arn, tags, tags_uri, Kind,
    DISCOVERERS_URI, CONTENT,
};

/// The length an answer with no body states.
const EMPTY_BODY_LENGTH: &str = "0";

/// A resource-based policy document, used wherever a row needs one attached.
const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// One row: an implemented operation of this service whose output shape is `Unit`, so
/// its success carries no body at all.
#[derive(Debug, Clone, Copy)]
enum NoOutput {
    /// `DELETE /v1/registries/name/{RegistryName}` — remove a registry.
    DeleteRegistry,
    /// `DELETE /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}` — remove a
    /// schema.
    DeleteSchema,
    /// `DELETE …/schemas/name/{SchemaName}/version/{SchemaVersion}` — remove one
    /// version of a schema.
    DeleteSchemaVersion,
    /// `DELETE /v1/discoverers/id/{DiscovererId}` — remove a discoverer.
    DeleteDiscoverer,
    /// `DELETE /tags/{ResourceArn}?tagKeys=…` — remove tags from a resource.
    UntagResource,
    /// `DELETE /v1/policy?registryName=…` — remove a registry's resource-based policy.
    DeleteResourcePolicy,
}

impl NoOutput {
    /// Every row of the table.
    const ALL: [Self; 6] = [
        Self::DeleteRegistry,
        Self::DeleteSchema,
        Self::DeleteSchemaVersion,
        Self::DeleteDiscoverer,
        Self::UntagResource,
        Self::DeleteResourcePolicy,
    ];

    /// The operation name, for the case label every assertion carries.
    fn case(self) -> &'static str {
        match self {
            Self::DeleteRegistry => "DeleteRegistry",
            Self::DeleteSchema => "DeleteSchema",
            Self::DeleteSchemaVersion => "DeleteSchemaVersion",
            Self::DeleteDiscoverer => "DeleteDiscoverer",
            Self::UntagResource => "UntagResource",
            Self::DeleteResourcePolicy => "DeleteResourcePolicy",
        }
    }

    /// The status this operation answers on success.
    fn status(self) -> u16 {
        match self {
            // Removing a resource is answered with no content at all.
            Self::DeleteRegistry
            | Self::DeleteSchema
            | Self::DeleteSchemaVersion
            | Self::DeleteDiscoverer
            | Self::UntagResource => 204,
            // The resource-policy endpoint answers 200 for every method it serves.
            Self::DeleteResourcePolicy => 200,
        }
    }

    /// Create what this row removes, then remove it, answering what the removal
    /// answered. `label` keeps one row's resources apart from another's.
    async fn seed_and_send(self, emulator: &Emulator, label: &str) -> Answer {
        match self {
            Self::DeleteRegistry => {
                let registry = format!("registry-{label}");
                create_registry(emulator, &registry).await;
                emulator.call("DELETE", &registry_uri(&registry), None).await
            }
            Self::DeleteSchema => {
                let registry = format!("registry-{label}");
                create_registry(emulator, &registry).await;
                let uri = schema_uri(&registry, &format!("schema-{label}"));
                create_schema(emulator, label, &uri).await;
                emulator.call("DELETE", &uri, None).await
            }
            Self::DeleteSchemaVersion => {
                let registry = format!("registry-{label}");
                create_registry(emulator, &registry).await;
                let uri = schema_uri(&registry, &format!("schema-{label}"));
                let created = create_schema(emulator, label, &uri).await;
                // A second version, so the removal takes one version of a schema that
                // keeps on existing.
                let updated = emulator
                    .call(
                        "PUT",
                        &uri,
                        Some(json!({"Content": CONTENT, "Type": "JSONSchemaDraft4"})),
                    )
                    .await;
                assert_eq!(
                    updated.status, 200,
                    "case {label:?}: expected 200 from UpdateSchema, got {} {:?}",
                    updated.status, updated.body
                );
                emulator
                    .call(
                        "DELETE",
                        &format!("{uri}/version/{}", created.str_at("SchemaVersion")),
                        None,
                    )
                    .await
            }
            Self::DeleteDiscoverer => {
                let created = emulator
                    .call(
                        "POST",
                        DISCOVERERS_URI,
                        Some(json!({"SourceArn": source_arn(&format!("bus-{label}"))})),
                    )
                    .await;
                assert_eq!(
                    created.status, 201,
                    "case {label:?}: expected 201 from CreateDiscoverer, got {} {:?}",
                    created.status, created.body
                );
                emulator
                    .call(
                        "DELETE",
                        &discoverer_uri(&created.str_at("DiscovererId")),
                        None,
                    )
                    .await
            }
            Self::UntagResource => {
                let created = create_tagged(
                    emulator,
                    label,
                    Kind::Registry,
                    &format!("tagged-{label}"),
                    &tags(&[("team", "payments"), ("env", "test")]),
                )
                .await;
                emulator
                    .call(
                        "DELETE",
                        &format!(
                            "{}?tagKeys={}",
                            tags_uri(&created.arn),
                            percent_encode("env")
                        ),
                        None,
                    )
                    .await
            }
            Self::DeleteResourcePolicy => {
                let registry = format!("registry-{label}");
                create_registry(emulator, &registry).await;
                let attached = emulator
                    .call(
                        "PUT",
                        &policy_uri(Some(&registry)),
                        Some(json!({ "Policy": POLICY })),
                    )
                    .await;
                assert_eq!(
                    attached.status, 200,
                    "case {label:?}: expected 200 from PutResourcePolicy, got {} {:?}",
                    attached.status, attached.body
                );
                emulator
                    .call("DELETE", &policy_uri(Some(&registry)), None)
                    .await
            }
        }
    }
}

/// Create one schema at `uri`, asserting the create succeeded.
async fn create_schema(emulator: &Emulator, label: &str, uri: &str) -> Answer {
    let created = emulator
        .call(
            "POST",
            uri,
            Some(json!({"Content": CONTENT, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {label:?}: expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );
    created
}

/// Assert `answer` is a success that carries no body, at `expected` status, framed with
/// the length it states.
fn assert_bodiless(case: &str, answer: &Answer, expected: u16) {
    assert_eq!(
        answer.status, expected,
        "case {case}: expected {expected}, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.raw.is_empty(),
        "case {case}: this answer carries no body, got {:?}",
        String::from_utf8_lossy(&answer.raw)
    );
    assert_eq!(
        answer.header("content-length"),
        Some(EMPTY_BODY_LENGTH),
        "case {case}: an answer with no body must state its length, got {:?}",
        answer.header("content-length")
    );
}

/// RULE: every operation whose output shape is `Unit` answers its OWN success status —
/// not one inferred from the empty body — with no body and `content-length: 0`.
#[tokio::test]
async fn every_operation_with_no_output_answers_its_own_status() {
    for (row, operation) in NoOutput::ALL.into_iter().enumerate() {
        // One store per row, so a row's answer cannot depend on another row's state.
        let emulator = Emulator::new();
        let label = format!("row-{row}");
        let answer = operation.seed_and_send(&emulator, &label).await;
        assert_bodiless(operation.case(), &answer, operation.status());
    }
}

/// One policy document a row of the policy-endpoint table writes, reads and removes.
#[derive(Debug)]
struct PolicyCase {
    case: &'static str,
    document: String,
}

/// The table: rows differ along what the status rule generalises over — the size,
/// nesting, member count and whitespace of the document the endpoint is carrying when
/// each of its three methods is called. The status is the same for all of them, which
/// is the point: it is a property of the endpoint's methods and not of the payload.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "the smallest JSON object there is",
            document: "{}".to_string(),
        },
        PolicyCase {
            case: "a single-statement policy",
            document: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowDescribe",
                    "Effect": "Allow",
                    "Principal": { "AWS": "000000000000" },
                    "Action": ["schemas:DescribeRegistry"],
                    "Resource": "*"
                }]
            })
            .to_string(),
        },
        PolicyCase {
            case: "pretty-printed, with newlines and tabs between tokens",
            document: "{\n\t\"Version\" : \"2012-10-17\",\n\t\"Statement\" : [ ]\n}".to_string(),
        },
        PolicyCase {
            case: "a document of several kilobytes",
            document: format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Principal":"*","Action":"schemas:*","Resource":"*","Sid":"{}"}}]}}"#,
                "x".repeat(8_000)
            ),
        },
    ]
}

/// RULE: the resource-policy endpoint answers 200 for every method it serves — the
/// read and the write, which carry a body, and the removal, which carries none — and
/// the removal really removed the policy, so the 200 reports a deletion rather than
/// being an empty answer to nothing.
#[tokio::test]
async fn every_method_on_the_policy_endpoint_answers_two_hundred() {
    for (row, policy) in policy_cases().into_iter().enumerate() {
        let emulator = Emulator::new();
        let registry = format!("policy-registry-{row}");
        create_registry(&emulator, &registry).await;

        // The write and the read: a 200 that carries a body, framed from that body.
        for (method, body) in [
            ("PUT", Some(json!({ "Policy": policy.document }))),
            ("GET", None),
        ] {
            let answer = emulator
                .call(method, &policy_uri(Some(&registry)), body)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {:?} ({method} /v1/policy): expected 200, got {} {:?}",
                policy.case, answer.status, answer.body
            );
            assert!(
                !answer.raw.is_empty(),
                "case {:?} ({method} /v1/policy): this answer carries a body",
                policy.case
            );
        }

        // And the removal: the same 200, with no body and its length stated.
        let removed = emulator
            .call("DELETE", &policy_uri(Some(&registry)), None)
            .await;
        assert_bodiless(
            &format!("{:?}/DeleteResourcePolicy", policy.case),
            &removed,
            200,
        );

        // The policy is gone, and a second removal has nothing left to remove.
        for (case, answer) in [
            (
                "GetResourcePolicy after the removal",
                emulator.call("GET", &policy_uri(Some(&registry)), None).await,
            ),
            (
                "a second DeleteResourcePolicy",
                emulator
                    .call("DELETE", &policy_uri(Some(&registry)), None)
                    .await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {:?} ({case}): expected 404 once the policy is gone, got {} {:?}",
                policy.case, answer.status, answer.body
            );
        }

        // The registry itself survived: only its policy was removed.
        let described = emulator.call("GET", &registry_uri(&registry), None).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: removing a policy must not remove the registry, got {} {:?}",
            policy.case, described.status, described.body
        );
    }
}

/// What the bodiless 200 looks like on the WIRE: a status line a client's HTTP stack
/// reads as a response that MAY carry a body, the stated `content-length: 0`, and no
/// body. Unlike a `204` — where the HTTP/1.1 writer below this crate drops the framing
/// header, as `empty_response_framing.rs` records — a `200` keeps it, so this is the
/// framing a client really sees.
#[tokio::test(flavor = "multi_thread")]
async fn the_bodiless_two_hundred_is_framed_on_the_wire() {
    let emulator = Emulator::new();
    let registry = "wire-policy-registry";
    create_registry(&emulator, registry).await;
    let attached = emulator
        .call(
            "PUT",
            &policy_uri(Some(registry)),
            Some(json!({ "Policy": POLICY })),
        )
        .await;
    assert_eq!(
        attached.status, 200,
        "expected 200 from PutResourcePolicy, got {} {:?}",
        attached.status, attached.body
    );

    // The same router, serving the same store, on a loopback port.
    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(emulator.router()).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let path = policy_uri(Some(registry));
    let answer = tokio::task::spawn_blocking(move || raw_request(port, "DELETE", &path))
        .await
        .expect("the socket exchange runs to completion");
    assert!(
        answer.starts_with("HTTP/1.1 200"),
        "case DeleteResourcePolicy: expected a 200 on the wire, got {answer:?}"
    );
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answer:?}"));
    assert!(
        body.is_empty(),
        "case DeleteResourcePolicy: this answer carries no body, got {body:?}"
    );
    assert!(
        head.to_ascii_lowercase().contains("content-length: 0"),
        "case DeleteResourcePolicy: an answer with no body must state its length, got \
         {head:?}"
    );

    // And the policy really is gone, so the 200 reported a deletion.
    let gone = emulator.call("GET", &policy_uri(Some(registry)), None).await;
    assert_eq!(
        gone.status, 404,
        "after the wire removal the policy must be gone, got {} {:?}",
        gone.status, gone.body
    );
}
