// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what REMOVING an ACM resource requires of the caller, and for how
//! this service words a tag call addressed to a resource it no longer holds. Every
//! assertion goes over the real wire path (an awsJson1_1 request through the multi-service
//! router into the mounted service), so what is asserted is what a client sees.
//!
//! RULE A: every operation of this service that removes a resource and models
//! `AccessDeniedException` (HTTP 403) evaluates the identity-based IAM policies in force
//! on the principal the request's credentials name against `acm:<OperationName>` on the
//! ARN the request addresses BEFORE it removes anything — so a request no statement allows,
//! or one an explicit `Deny` covers, is answered HTTP 403 in IAM's own wording and the
//! resource is left exactly as it was, while the same call under a principal a matching
//! `Allow` covers, or under the account's own credential, performs the removal, after which
//! the resource's own `Describe` reports it missing.
//!
//! RULE B: the `ResourceNotFoundException` a type-agnostic tag operation answers for an ARN
//! this account holds no resource for names the resource TYPE the ARN itself carries, in
//! the one sentence this service words a missing resource with.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME shared
//! engine — the stores those services' own handlers write — so the documents under test are
//! the ones `iam:PutRolePolicy` stores and the principal is the one
//! `sts:GetCallerIdentity` answers for the key the request signed with. The harness is
//! shared with [`crate::acme_authorization`], which pins the same authorization rule over
//! the reads, lists, creates and updates of this service; this suite is about the
//! DESTRUCTIVE half of it, where a request served to a principal AWS would refuse destroys
//! a resource.
//!
//! The tables span what the rules generalise over: for RULE A the resource a removal
//! destroys (a certificate, an ACME endpoint, and the two sub-resources whose ARNs are
//! paths under an endpoint's) crossed with the documents in force on the principal (none at
//! all, every other removal of this service, an explicit deny beside an allow of
//! everything, an allow of exactly the action, an allow of the whole service); for RULE B
//! the resource type a deleted resource's ARN carries crossed with the three operations
//! that address a resource by type-agnostic ARN.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_authorization::{
    allow, call_as, given_role_policy, given_session, router_with_identity, session_arn, Refusal,
    ACCOUNT_KEY_ID, SESSION_KEY_ID,
};
use crate::acme_directory_endpoint::activate_endpoints_now;
use crate::acme_domain_validation_lifecycle::{
    create_validation, endpoint_body, settle_prevalidations_now,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::{binding_body, binding_cases, create_binding};
use crate::certificate_lifecycle::{
    call, error_code, error_message, hold_pending_validations, request_certificate,
};
use crate::resource_tagging::{expect, stored_tags, tag_resource};

/// One removal operation of this service, as a row of the RULE A table.
#[derive(Debug, Clone, Copy)]
struct Removal {
    /// The resource this removal destroys — what the row exercises.
    case: &'static str,
    /// The operation's own name, which is also the action IAM judges it as (`acm:<name>`).
    operation: &'static str,
    /// The input member the ARN of that resource travels in, which is also the member its
    /// `Describe` reads it back by and the member of the record that carries it.
    arn_member: &'static str,
    /// The operation that reads the resource back.
    describe: &'static str,
    /// The member of that answer which carries the record.
    record: &'static str,
}

/// Every operation of this service that removes a resource and models the refusal — one
/// per resource type a removal reaches: a certificate, an ACME endpoint, and the two
/// sub-resources an endpoint holds, whose ARNs extend its own resource path.
///
/// The operations and their modelled errors come from the service's model
/// (`smithy_query(query="digest")`): each of the four declares `AccessDeniedException`
/// with `httpError` 403.
fn removals() -> Vec<Removal> {
    vec![
        Removal {
            case: "a certificate",
            operation: "DeleteCertificate",
            arn_member: "CertificateArn",
            describe: "DescribeCertificate",
            record: "Certificate",
        },
        Removal {
            case: "an ACME endpoint",
            operation: "DeleteAcmeEndpoint",
            arn_member: "AcmeEndpointArn",
            describe: "DescribeAcmeEndpoint",
            record: "AcmeEndpoint",
        },
        Removal {
            case: "an external account binding, a sub-resource of an endpoint",
            operation: "DeleteAcmeExternalAccountBinding",
            arn_member: "AcmeExternalAccountBindingArn",
            describe: "DescribeAcmeExternalAccountBinding",
            record: "ExternalAccountBinding",
        },
        Removal {
            case: "a domain validation, a sub-resource of an endpoint",
            operation: "DeleteAcmeDomainValidation",
            arn_member: "AcmeDomainValidationArn",
            describe: "DescribeAcmeDomainValidation",
            record: "AcmeDomainValidation",
        },
    ]
}

/// The request body addressing `arn` through the input member `member`.
fn addressing(member: &str, arn: &str) -> Value {
    let mut body = json!({});
    body[member] = json!(arn);
    body
}

/// One ACME endpoint, created with the account's own credential and carried to `ACTIVE` by
/// making the activation it is owed fall due — the status an endpoint a caller has waited
/// for stands in, and a terminal one, so no transition can change the record mid-test.
async fn given_endpoint(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
) -> String {
    let arn = create_endpoint(router, case, &endpoint_body()).await;
    activate_endpoints_now(engine);
    arn
}

/// A resource of the kind `row` removes, created with the account's own credential and
/// settled in a status the clock owes nothing beyond, so the record a refused removal is
/// compared against cannot change under the test.
///
/// `nth` keeps the fixtures of one row distinct: ACM resolves a repeated certificate
/// request to the certificate it already made, so each row's resources differ in the name
/// they are requested for.
async fn given_resource(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    row: &Removal,
    nth: usize,
) -> String {
    let case = format!("{} #{nth}", row.case);
    let domain = format!("acme-{nth}.example.com");
    match row.operation {
        "DeleteCertificate" => {
            let arn = request_certificate(
                router,
                &json!({ "DomainName": domain, "ValidationMethod": "DNS" }),
            )
            .await;
            hold_pending_validations(engine);
            arn
        }
        "DeleteAcmeEndpoint" => given_endpoint(router, engine, &case).await,
        "DeleteAcmeExternalAccountBinding" => {
            let endpoint_arn = given_endpoint(router, engine, &case).await;
            // The first binding case carries no expiration, so the stored record is owed
            // no later change of its own.
            let binding = create_binding(
                router,
                &case,
                &binding_body(&binding_cases()[0], &endpoint_arn),
            )
            .await;
            binding["AcmeExternalAccountBindingArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the create reported no binding ARN in {binding}")
                })
                .to_string()
        }
        "DeleteAcmeDomainValidation" => {
            let endpoint_arn = given_endpoint(router, engine, &case).await;
            let arn = create_validation(
                router,
                &case,
                &json!({
                    "AcmeEndpointArn": endpoint_arn,
                    "DomainName": domain,
                    "PrevalidationOptions": { "DnsPrevalidation": {} },
                }),
            )
            .await;
            settle_prevalidations_now(engine);
            arn
        }
        other => panic!("case {other:?}: this suite builds no fixture for that operation"),
    }
}

/// `(status, body)` of `row`'s own `Describe` for `arn`, read with the account's own
/// credential — so what a removal did or did not do is read back through a DIFFERENT
/// operation than the one under test.
async fn describe(router: &MultiServiceRouter, row: &Removal, arn: &str) -> (u16, Value) {
    call(router, row.describe, &addressing(row.arn_member, arn)).await
}

/// The record `row`'s `Describe` reports for `arn`, asserted to be served and to name the
/// ARN it was read by.
async fn record(router: &MultiServiceRouter, case: &str, row: &Removal, arn: &str) -> Value {
    let (status, response) = describe(router, row, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {} must report {arn}, got {status} {response}",
        row.describe
    );
    let record = response[row.record].clone();
    assert_eq!(
        record[row.arn_member].as_str(),
        Some(arn),
        "case {case:?}: {} must report the record of {arn}, got {response}",
        row.describe
    );
    record
}

/// Assert `row`'s own `Describe` no longer reports `arn`: the resource is gone, which that
/// operation answers with the missing-resource verdict it models.
async fn assert_gone(router: &MultiServiceRouter, case: &str, row: &Removal, arn: &str) {
    let (status, response) = describe(router, row, arn).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: after the removal {} must report {arn} missing, got {status} {response}",
        row.describe
    );
}

/// RULE A: a removal is judged against its OWN action before it destroys anything, so a
/// session whose role carries no policy — or every other removal of this service, or an
/// explicit deny — is refused with HTTP 403 in IAM's wording and the resource is still
/// there, unchanged; the account's own credential still removes it, and a session a
/// matching `Allow` covers removes one of its own.
#[tokio::test]
async fn a_removal_no_policy_allows_is_refused_and_destroys_nothing() {
    for row in removals() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        let action = format!("acm:{}", row.operation);
        let others: Vec<Value> = removals()
            .iter()
            .filter(|other| other.operation != row.operation)
            .map(|other| json!(format!("acm:{}", other.operation)))
            .collect();

        // Every refusal is judged on ONE resource: a refused removal destroys nothing, so
        // the same record has to answer all of them.
        let kept = given_resource(&router, &engine, &row, 0).await;
        let before = record(&router, row.case, &row, &kept).await;

        // (what the row exercises, the document in force on the role, the refusal IAM
        // reaches)
        let refusals: Vec<(&str, Option<Value>, Refusal)> = vec![
            ("no policy at all", None, Refusal::DefaultDeny),
            (
                "an allow of every other removal of this service, but not this one",
                Some(allow(json!(others))),
                Refusal::DefaultDeny,
            ),
            (
                "an explicit deny of this action beside an allow of everything",
                Some(json!({
                    "Statement": [
                        { "Effect": "Allow", "Action": "acm:*", "Resource": "*" },
                        { "Effect": "Deny", "Action": action.clone(), "Resource": "*" },
                    ]
                })),
                Refusal::ExplicitDeny,
            ),
        ];

        for (document_case, document, refusal) in refusals {
            let case = format!("{} on {} with {document_case}", row.operation, row.case);
            given_role_policy(&engine, document.as_ref());
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                row.operation,
                &addressing(row.arn_member, &kept),
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected HTTP 403, got {status} {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "AccessDeniedException",
                "case {case:?}: expected an AccessDeniedException, got {answer}"
            );
            assert_eq!(
                error_message(&answer),
                format!(
                    "User: {} is not authorized to perform: {action} on resource: {kept} {}",
                    session_arn(),
                    refusal.reason(&action)
                ),
                "case {case:?}: expected IAM's refusal wording, got {answer}"
            );
            let still = record(&router, &case, &row, &kept).await;
            assert_eq!(
                still, before,
                "case {case:?}: a refused removal must leave the resource as it was, got {still}"
            );
        }

        // The account's own credential is not bounded by the role's policies: the removal
        // the session was refused is served for it, and the resource is then gone.
        let case = format!(
            "{} on {} with the account's own credential",
            row.operation, row.case
        );
        given_role_policy(&engine, None);
        let (status, answer) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            row.operation,
            &addressing(row.arn_member, &kept),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the account's own credential must be served, got {status} {answer}"
        );
        assert_gone(&router, &case, &row, &kept).await;

        // A session a matching `Allow` covers performs the removal itself — each on a
        // resource of its own, the one above being already gone.
        let grants: Vec<(&str, Value)> = vec![
            ("an allow of exactly this action", allow(json!(action))),
            (
                "an allow of every action of this service",
                allow(json!("acm:*")),
            ),
        ];
        for (nth, (document_case, document)) in grants.into_iter().enumerate() {
            let case = format!("{} on {} with {document_case}", row.operation, row.case);
            let arn = given_resource(&router, &engine, &row, nth + 1).await;
            record(&router, &case, &row, &arn).await;
            given_role_policy(&engine, Some(&document));
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                row.operation,
                &addressing(row.arn_member, &arn),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a granted removal must be served, got {status} {answer}"
            );
            assert_gone(&router, &case, &row, &arn).await;
        }
    }
}

/// One tagged resource of the RULE B table: the resource type token its ARN carries, the
/// operation that removes it, and the member that removal addresses it by.
struct Tagged {
    case: &'static str,
    resource_type: &'static str,
    delete: &'static str,
    arn_member: &'static str,
    arn: String,
}

/// RULE B: once a resource is deleted, every operation that addresses an ACM resource by
/// type-agnostic ARN reports it missing with the resource TYPE read out of that ARN — while
/// the same operations act on the live resource, which is what makes the two answers
/// distinguishable.
#[tokio::test]
async fn tag_calls_for_a_deleted_resource_name_its_type_from_the_arn() {
    let (router, engine) = router_with_identity();

    // The non-certificate resources this service persists: an ACME endpoint, and the two
    // sub-resources whose ARNs extend its resource path.
    let endpoint_arn = given_endpoint(&router, &engine, "an ACME endpoint").await;
    let binding = create_binding(
        &router,
        "an external account binding",
        &binding_body(&binding_cases()[0], &endpoint_arn),
    )
    .await;
    let binding_arn = binding["AcmeExternalAccountBindingArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the create reported no binding ARN in {binding}"))
        .to_string();
    let validation_arn = create_validation(
        &router,
        "a domain validation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "tagged.example.com",
            "PrevalidationOptions": { "DnsPrevalidation": {} },
        }),
    )
    .await;
    settle_prevalidations_now(&engine);

    // Each row's type token is the one that ARN shape's modelled `@pattern` spells
    // (`smithy_query(query="shape", shape="com.amazonaws.acm#AcmeDomainValidationArn")` and
    // its siblings). The endpoint comes LAST, because removing it removes the
    // sub-resources that hang under it.
    let rows: Vec<Tagged> = vec![
        Tagged {
            case: "an external account binding",
            resource_type: "acme-external-account-binding",
            delete: "DeleteAcmeExternalAccountBinding",
            arn_member: "AcmeExternalAccountBindingArn",
            arn: binding_arn,
        },
        Tagged {
            case: "a domain validation",
            resource_type: "acme-domain-validation",
            delete: "DeleteAcmeDomainValidation",
            arn_member: "AcmeDomainValidationArn",
            arn: validation_arn,
        },
        Tagged {
            case: "an ACME endpoint",
            resource_type: "acme-endpoint",
            delete: "DeleteAcmeEndpoint",
            arn_member: "AcmeEndpointArn",
            arn: endpoint_arn,
        },
    ];

    // The members each type-agnostic operation needs beside `ResourceArn`.
    let operations: &[(&str, Value)] = &[
        ("ListTagsForResource", json!({})),
        (
            "TagResource",
            json!({ "Tags": [{ "Key": "env", "Value": "dev" }] }),
        ),
        ("UntagResource", json!({ "TagKeys": ["env"] })),
    ];

    for row in &rows {
        let case = row.case;
        let arn = &row.arn;

        // The positive leg: while the resource is there, the same three operations write
        // and read its tags.
        tag_resource(
            &router,
            case,
            arn,
            &json!([
                { "Key": "env", "Value": "dev" },
                { "Key": "owner", "Value": "platform" },
            ]),
        )
        .await;
        let tagged = stored_tags(&router, case, arn).await;
        assert_eq!(
            tagged,
            expect(&[("env", "dev"), ("owner", "platform")]),
            "case {case:?}: the live resource must carry the tags written to it, got {tagged:?}"
        );
        let (status, answer) = call(
            &router,
            "UntagResource",
            &json!({ "ResourceArn": arn, "TagKeys": ["env"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: untagging the live resource must be served, got {status} {answer}"
        );
        let remaining = stored_tags(&router, case, arn).await;
        assert_eq!(
            remaining,
            expect(&[("owner", "platform")]),
            "case {case:?}: untagging must drop only the key it named, got {remaining:?}"
        );

        // Remove the resource through its own operation, then address it by the same ARN.
        let (status, answer) = call(&router, row.delete, &addressing(row.arn_member, arn)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {} must be served, got {status} {answer}",
            row.delete
        );

        for (operation, extra) in operations {
            let mut body = extra.clone();
            body["ResourceArn"] = json!(arn);
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} ({operation}): expected the missing-resource refusal, got \
                 {status} {answer}"
            );
            assert_eq!(
                error_message(&answer),
                format!("Could not find {} {arn}.", row.resource_type),
                "case {case:?} ({operation}): expected the ARN's own resource type to be \
                 named, got {answer}"
            );
        }
    }
}
