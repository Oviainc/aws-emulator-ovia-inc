// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what this service's error BODIES carry.
//!
//! Every error shape this service models has two required members, `Code` ("The
//! error code.") and `Message`, and they say different things: the exception SHAPE
//! name travels in the `x-amzn-errortype` header, while `Code` carries the service's
//! own code for the CONDITION that was raised — which is what a client branching on
//! the SDK's error-code accessor reads. The cases below assert that over the whole
//! error surface of the service, one row per condition, not only the operations an
//! issue happened to cite.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! HTTP harness is the crate's one harness, imported from `registry_schema_lifecycle`.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, AWS_MANAGED_REGISTRY, CODE_CONFLICT, CODE_FORBIDDEN,
    CODE_MALFORMED_REQUEST, CODE_RESOURCE_ALREADY_EXIST, CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{create_registry, registry_uri, schema_uri, Answer, Emulator};

/// A registry the cases below read and write.
const REGISTRY: &str = "error-codes-registry";
/// A schema the registry above holds, so a case can ask for something *inside* an
/// existing schema and still be refused.
const SCHEMA: &str = "order.placed";
/// A registry that holds a resource-based policy, for the precondition cases.
const POLICY_REGISTRY: &str = "error-codes-policy-registry";
/// An OpenAPI 3 document, which is what `Content` must be.
const CONTENT: &str = r#"{"openapi":"3.0.0","info":{"version":"1.0.0","title":"t"},"paths":{}}"#;

/// One refusal: the request that provokes it and the whole envelope it must answer.
#[derive(Debug)]
struct Case {
    /// The condition this row is about, named in every assertion.
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    /// The exception shape, which the model's `httpError` ties to `status`.
    shape: &'static str,
    /// The service's error code for the condition — the `Code` member.
    code: &'static str,
}

/// Seed the store the table runs against: a registry holding one schema, and a
/// second registry holding a policy. Returns the policy's current revision id.
async fn seed(emulator: &Emulator) -> String {
    create_registry(emulator, REGISTRY).await;
    let created = emulator
        .call(
            "POST",
            &schema_uri(REGISTRY, SCHEMA),
            Some(json!({"Content": CONTENT, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "seeding a schema must succeed, got {} {:?}",
        created.status, created.body
    );

    create_registry(emulator, POLICY_REGISTRY).await;
    let policy = emulator
        .call(
            "PUT",
            &format!("/v1/policy?registryName={POLICY_REGISTRY}"),
            Some(json!({"Policy": r#"{"Version":"2012-10-17","Statement":[]}"#})),
        )
        .await;
    assert_eq!(
        policy.status, 200,
        "seeding a policy must succeed, got {} {:?}",
        policy.status, policy.body
    );
    policy.str_at("RevisionId")
}

/// The table: one row per refusal this service can answer with, spanning every
/// modelled error shape it raises (404 `NotFoundException`, 409 `ConflictException`,
/// 412 `PreconditionFailedException`, 400 `BadRequestException`, 403
/// `ForbiddenException`), every operation that raises one, and — within a shape —
/// each distinct condition.
///
/// Every row is a refusal, so no row changes the store and the table can run against
/// one seeded emulator.
fn cases(stale_revision: &str, current_revision: &str) -> Vec<Case> {
    // The service-owned registry and one of its schemas, read from the catalogue the
    // store seeds rather than written out here.
    let managed_registry = AWS_MANAGED_REGISTRY;
    let managed_schema = aws_managed_schemas()
        .first()
        .expect("the catalogue holds schemas")
        .name
        .as_str();
    let absent = "no-such-registry";
    let absent_schema = "no-such-schema";

    vec![
        // ---- 404: there is no such resource ------------------------------------
        Case {
            case: "DescribeRegistry on a registry nothing created",
            method: "GET",
            uri: registry_uri(absent),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "DeleteRegistry on a registry nothing created",
            method: "DELETE",
            uri: registry_uri(absent),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        // (`UpdateRegistry` is still scaffolded, so its refusal is the 501 the
        // scaffold channel answers with and it is not a row here.)
        Case {
            case: "ListSchemas of a registry nothing created",
            method: "GET",
            uri: format!("{}/schemas", registry_uri(absent)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "SearchSchemas in a registry nothing created",
            method: "GET",
            uri: format!("{}/schemas/search?keywords=order", registry_uri(absent)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "DescribeSchema on a schema the registry does not hold",
            method: "GET",
            uri: schema_uri(REGISTRY, absent_schema),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "DescribeSchema on a version the schema does not hold",
            method: "GET",
            uri: format!("{}?schemaVersion=99", schema_uri(REGISTRY, SCHEMA)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "ListSchemaVersions of a schema the registry does not hold",
            method: "GET",
            uri: format!("{}/versions", schema_uri(REGISTRY, absent_schema)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "UpdateSchema on a schema the registry does not hold",
            method: "PUT",
            uri: schema_uri(REGISTRY, absent_schema),
            body: Some(json!({"Content": CONTENT, "Type": "OpenApi3"})),
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "DeleteSchema on a schema the registry does not hold",
            method: "DELETE",
            uri: schema_uri(REGISTRY, absent_schema),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "ExportSchema of a version the schema does not hold",
            method: "GET",
            uri: format!(
                "{}/export?type=JSONSchemaDraft4&schemaVersion=99",
                schema_uri(REGISTRY, SCHEMA)
            ),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "DescribeCodeBinding for a language no binding was put for",
            method: "GET",
            uri: format!("{}/language/Java8", schema_uri(REGISTRY, SCHEMA)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "GetCodeBindingSource for a language no binding was put for",
            method: "GET",
            uri: format!("{}/language/Java8/source", schema_uri(REGISTRY, SCHEMA)),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "GetResourcePolicy for a registry that holds no policy",
            method: "GET",
            uri: format!("/v1/policy?registryName={REGISTRY}"),
            body: None,
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        Case {
            case: "PutResourcePolicy for a registry nothing created",
            method: "PUT",
            uri: format!("/v1/policy?registryName={absent}"),
            body: Some(json!({"Policy": "{}"})),
            status: 404,
            shape: "NotFoundException",
            code: CODE_RESOURCE_NOT_FOUND,
        },
        // ---- the resource already exists: one condition code, one conflict shape
        // ---- whichever resource was asked for -----------------------------------
        Case {
            case: "CreateRegistry for a registry that already exists",
            method: "POST",
            uri: registry_uri(REGISTRY),
            body: Some(json!({})),
            status: 409,
            shape: "ConflictException",
            code: CODE_RESOURCE_ALREADY_EXIST,
        },
        Case {
            // A resource that is already there is a conflict with the stored state,
            // not a malformed request, so it takes the service's conflict shape under
            // the same already-exists condition code as every other duplicate create.
            case: "CreateSchema for a schema that already exists",
            method: "POST",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Content": CONTENT, "Type": "OpenApi3"})),
            status: 409,
            shape: "ConflictException",
            code: CODE_RESOURCE_ALREADY_EXIST,
        },
        // ---- 409: a conflict that is not a duplicate resource ------------------
        Case {
            case: "DeleteRegistry of a registry that still holds schemas",
            method: "DELETE",
            uri: registry_uri(REGISTRY),
            body: None,
            status: 409,
            shape: "ConflictException",
            code: CODE_CONFLICT,
        },
        // ---- 412: the request's precondition is not the stored state -----------
        Case {
            case: "PutResourcePolicy with a RevisionId that is not the current one",
            method: "PUT",
            uri: format!("/v1/policy?registryName={POLICY_REGISTRY}"),
            body: Some(json!({"Policy": "{}", "RevisionId": stale_revision})),
            status: 412,
            shape: "PreconditionFailedException",
            code: CODE_CONFLICT,
        },
        Case {
            case: "PutResourcePolicy on a registry whose policy is at another revision",
            method: "PUT",
            uri: format!("/v1/policy?registryName={POLICY_REGISTRY}"),
            body: Some(json!({"Policy": "{}", "RevisionId": format!("{current_revision}x")})),
            status: 412,
            shape: "PreconditionFailedException",
            code: CODE_CONFLICT,
        },
        // ---- 400: the request is not one the service will act on ---------------
        Case {
            case: "GetDiscoveredSchema with an event that is not JSON",
            method: "POST",
            uri: "/v1/discover".to_string(),
            body: Some(json!({"Events": ["not json at all"], "Type": "OpenApi3"})),
            status: 400,
            shape: "BadRequestException",
            code: CODE_MALFORMED_REQUEST,
        },
        Case {
            case: "GetDiscoveredSchema with an event that is not a JSON object",
            method: "POST",
            uri: "/v1/discover".to_string(),
            body: Some(json!({"Events": ["[1,2,3]"], "Type": "OpenApi3"})),
            status: 400,
            shape: "BadRequestException",
            code: CODE_MALFORMED_REQUEST,
        },
        Case {
            case: "UpdateSchema with Content that is not a schema document",
            method: "PUT",
            uri: schema_uri(REGISTRY, SCHEMA),
            body: Some(json!({"Content": "not json at all", "Type": "OpenApi3"})),
            status: 400,
            shape: "BadRequestException",
            code: CODE_MALFORMED_REQUEST,
        },
        Case {
            case: "GetResourcePolicy that names no registry",
            method: "GET",
            uri: "/v1/policy".to_string(),
            body: None,
            status: 400,
            shape: "BadRequestException",
            code: CODE_MALFORMED_REQUEST,
        },
        Case {
            case: "PutCodeBinding for a language this service generates no bindings for",
            method: "POST",
            uri: format!("{}/language/Cobol", schema_uri(REGISTRY, SCHEMA)),
            body: None,
            status: 400,
            shape: "BadRequestException",
            code: CODE_MALFORMED_REQUEST,
        },
        Case {
            case: "ListRegistries with a Scope that is neither Local nor AWS",
            method: "GET",
            uri: "/v1/registries?scope=Everything".to_string(),
            body: None,
            status: 400,
            shape: "BadRequestException",
            code: CODE_MALFORMED_REQUEST,
        },
        Case {
            case: "ListRegistries with a NextToken this service did not mint",
            method: "GET",
            uri: "/v1/registries?nextToken=not-a-token".to_string(),
            body: None,
            status: 400,
            shape: "BadRequestException",
            code: CODE_MALFORMED_REQUEST,
        },
        // ---- 403: AWS owns the resource, so a caller may not write it ----------
        Case {
            case: "CreateSchema in the service-owned registry",
            method: "POST",
            uri: schema_uri(managed_registry, "mine"),
            body: Some(json!({"Content": CONTENT, "Type": "OpenApi3"})),
            status: 403,
            shape: "ForbiddenException",
            code: CODE_FORBIDDEN,
        },
        Case {
            case: "DeleteRegistry of the service-owned registry",
            method: "DELETE",
            uri: registry_uri(managed_registry),
            body: None,
            status: 403,
            shape: "ForbiddenException",
            code: CODE_FORBIDDEN,
        },
        Case {
            case: "UpdateSchema of a schema the service-owned registry holds",
            method: "PUT",
            uri: schema_uri(managed_registry, managed_schema),
            body: Some(json!({"Content": CONTENT, "Type": "OpenApi3"})),
            status: 403,
            shape: "ForbiddenException",
            code: CODE_FORBIDDEN,
        },
    ]
}

/// The exception shape name a response carries, as restJson1 renders it in the body.
fn body_shape(answer: &Answer) -> String {
    answer.error_code()
}

/// RULE: in every error this service answers with, the required `Code` member carries
/// the service's error code for the CONDITION raised — one code per condition, shared
/// by every operation that raises it — while the exception SHAPE name travels in the
/// `x-amzn-errortype` header (and the restJson1 `__type` body member). The two are
/// never the same string.
#[tokio::test]
async fn an_error_body_carries_the_condition_code_and_the_header_carries_the_shape() {
    let emulator = Emulator::new();
    let revision = seed(&emulator).await;
    let stale = "ffffffff-0000-4000-8000-ffffffffffff";

    for case in cases(stale, &revision) {
        let answer = emulator
            .call(case.method, &case.uri, case.body.clone())
            .await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {}, got {} {:?}",
            case.case, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_type(),
            case.shape,
            "case {:?}: the exception shape name travels in x-amzn-errortype, got {:?}",
            case.case,
            answer.header("x-amzn-errortype")
        );
        assert_eq!(
            body_shape(&answer),
            case.shape,
            "case {:?}: expected the restJson1 __type {}, got {:?}",
            case.case,
            case.shape,
            answer.body
        );
        let code = answer.str_at("Code");
        assert_eq!(
            code, case.code,
            "case {:?}: expected Code {:?}, got {:?}",
            case.case, case.code, code
        );
        assert_ne!(
            code, case.shape,
            "case {:?}: Code must carry the service's error code for the condition, \
             never the exception shape name",
            case.case
        );
        // `Message` is required too, and it is never the code: a client shows it to a
        // human.
        let message = answer.str_at("Message");
        assert!(
            !message.is_empty() && message != code,
            "case {:?}: expected a human-readable Message beside the code, got {message:?}",
            case.case
        );
    }
}

/// RULE: one not-found code serves every not-found condition, whatever resource is
/// missing — the `Message` is what says which. Asserted by reading the code of a
/// missing registry, schema, version, code binding and policy through one request
/// each and demanding they agree.
#[tokio::test]
async fn every_not_found_condition_answers_with_the_same_code() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let mut seen: Vec<(&str, String, String)> = Vec::new();
    for (case, method, uri) in [
        ("a missing registry", "GET", registry_uri("nowhere")),
        (
            "a missing schema",
            "GET",
            schema_uri(REGISTRY, "nowhere"),
        ),
        (
            "a missing schema version",
            "GET",
            format!("{}?schemaVersion=42", schema_uri(REGISTRY, SCHEMA)),
        ),
        (
            "a missing code binding",
            "GET",
            format!("{}/language/TypeScript3", schema_uri(REGISTRY, SCHEMA)),
        ),
        (
            "a missing resource policy",
            "GET",
            format!("/v1/policy?registryName={REGISTRY}"),
        ),
    ] {
        let answer = emulator.call(method, &uri, None).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        seen.push((case, answer.str_at("Code"), answer.str_at("Message")));
    }

    for (case, code, message) in &seen {
        assert_eq!(
            code, &CODE_RESOURCE_NOT_FOUND,
            "case {case:?}: every not-found condition answers with one code, got {code:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: the Message is what says WHICH resource is missing, got {message:?}"
        );
    }
    // And the messages do differ: the code is shared, the message is not.
    let distinct: std::collections::BTreeSet<&String> =
        seen.iter().map(|(_, _, message)| message).collect();
    assert_eq!(
        distinct.len(),
        seen.len(),
        "each not-found condition says which resource is missing, so no two \
         messages repeat; got {distinct:?}"
    );
}

/// RULE: `GetDiscoveredSchema`'s `Events` is a list, so a refusal of one event is
/// built from the request and names the INDEX of the event at fault — which is what
/// tells a caller sending a batch which of its events to fix.
#[tokio::test]
async fn a_refused_event_names_its_index_in_the_request() {
    let emulator = Emulator::new();
    let good = r#"{"id":"1"}"#;

    // Rows: which position in the batch is at fault (first, middle, last, and the
    // only one), crossed with the two ways an event is unreadable — text that is not
    // JSON, and JSON that is not an object.
    for (case, bad) in [
        // The empty event is not a row: `Events`' member carries a @length
        // constraint, so the boundary validator refuses it before a handler runs.
        ("not JSON at all", "}{"),
        ("a JSON array", "[1,2]"),
        ("a JSON number", "7"),
        ("the null literal", "null"),
    ] {
        for batch in 1..=3usize {
            for index in 0..batch {
                let events: Vec<String> = (0..batch)
                    .map(|position| {
                        if position == index {
                            bad.to_string()
                        } else {
                            good.to_string()
                        }
                    })
                    .collect();
                let answer = emulator
                    .call(
                        "POST",
                        "/v1/discover",
                        Some(json!({"Events": events, "Type": "OpenApi3"})),
                    )
                    .await;
                let row = format!("{case} at index {index} of {batch}");
                assert_eq!(
                    answer.status, 400,
                    "case {row:?}: expected 400, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.str_at("Code"),
                    CODE_MALFORMED_REQUEST,
                    "case {row:?}: expected Code {CODE_MALFORMED_REQUEST}, got {:?}",
                    answer.body
                );
                let message = answer.str_at("Message");
                assert!(
                    message.contains(&format!("[{index}]")),
                    "case {row:?}: the refusal must name the index of the event at fault, \
                     got {message:?}"
                );
                assert!(
                    !message.contains(good),
                    "case {row:?}: the refusal must not echo the caller's events back, \
                     got {message:?}"
                );
            }
        }
    }
}
