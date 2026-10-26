// SPDX-License-Identifier: CC0-1.0

//! How the GENERIC resource-tagging operations word their certificate-ARN refusal, over
//! the wire (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! RULE: each generic tagging operation serves every ACM resource type except the
//! certificate resource type (`smithy_query(query="doc",
//! shape="com.amazonaws.acm#TagResource")`: "Use this action for all ACM resource types
//! except the certificate resource type. For certificate resources, use
//! AddTagsToCertificate instead" — and the same sentence pair on `UntagResource` and
//! `ListTagsForResource`, each naming its own certificate counterpart), so a certificate
//! ARN draws HTTP 400 `ValidationException` whose message states, in three fixed
//! sentences, the CALLED operation's supported resource types, that the resource the
//! request named is a certificate, and the certificate-specific operation to call
//! instead. Only three parts vary: the called operation, the `ResourceArn` the request
//! submitted, and that counterpart operation.
//!
//! [`crate::resource_tagging`] already pins the status, the code and the refusal's
//! effect (nothing is written, the certificate keeps the tags its own API wrote); this
//! suite pins the MESSAGE TEXT, which is a requirement of the refusal beside them.
//!
//! The table spans what the rule generalises over rather than the one request a trace
//! recorded: all three generic operations, a certificate stored by each of the two
//! creation paths (`RequestCertificate` and `ImportCertificate`), a certificate ARN with
//! no record behind it at all, and the ARN forms the member's `@pattern` admits — another
//! region, another account, another partition, an empty region segment and a
//! certificate id that is not a UUID. A positive control keeps the non-certificate path
//! served.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the path the emulator serves
//! real traffic on.

use serde_json::{json, Value};

use crate::certificate_import::{generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, error_code, error_message, request_certificate, router, ACCOUNT, REGION,
};
use crate::resource_tagging::{create_endpoint, stored_tags};

/// One ARN row: what it exercises and the `ResourceArn` the request submits.
type ArnCase = (String, String);

/// One generic operation: its name, the request members beside `ResourceArn`, and the
/// certificate-specific operation its refusal must name — the counterpart each
/// operation's own documentation points at.
type OperationCase = (&'static str, Value, &'static str);

/// AWS's wording for the refusal, with the three parts the rule leaves variable filled
/// in: the called operation, the submitted ARN and the certificate counterpart.
fn expected_message(operation: &str, resource_arn: &str, certificate_operation: &str) -> String {
    format!(
        "{operation} only supports non-certificate resource types. Resource {resource_arn} is a \
         certificate resource. Use {certificate_operation} API instead."
    )
}

/// The three generic tagging operations, with the certificate counterpart each one's
/// documentation names.
fn operation_cases() -> Vec<OperationCase> {
    vec![
        (
            "TagResource",
            json!({ "Tags": [{ "Key": "env", "Value": "via-generic" }] }),
            "AddTagsToCertificate",
        ),
        (
            "UntagResource",
            json!({ "TagKeys": ["env"] }),
            "RemoveTagsFromCertificate",
        ),
        ("ListTagsForResource", json!({}), "ListTagsForCertificate"),
    ]
}

/// The ARN rows the rule is asserted over: two stored certificates and the ARN forms the
/// `ResourceArn` member's `@pattern`
/// (`^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`, from
/// `smithy_query(query="operation", operation="TagResource")`) admits beside them — the
/// refusal is about the resource TYPE the ARN carries, so it cannot depend on there
/// being a record, on the region, the account or the partition, or on the id's shape.
fn arn_cases(requested: &str, imported: &str) -> Vec<ArnCase> {
    let id = "11111111-2222-3333-4444-555555555555";
    vec![
        (
            "a requested certificate's own ARN".to_string(),
            requested.to_string(),
        ),
        (
            "an imported certificate's own ARN".to_string(),
            imported.to_string(),
        ),
        (
            "a certificate ARN with no record behind it".to_string(),
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/{id}"),
        ),
        (
            "a certificate ARN in another region".to_string(),
            format!("arn:aws:acm:eu-central-1:{ACCOUNT}:certificate/{id}"),
        ),
        (
            "a certificate ARN in another account".to_string(),
            format!("arn:aws:acm:{REGION}:210987654321:certificate/{id}"),
        ),
        (
            "a certificate ARN in another partition".to_string(),
            format!("arn:aws-us-gov:acm:us-gov-west-1:{ACCOUNT}:certificate/{id}"),
        ),
        (
            "a certificate ARN with an empty region segment".to_string(),
            format!("arn:aws:acm::{ACCOUNT}:certificate/{id}"),
        ),
        (
            "a certificate ARN whose id is not a UUID".to_string(),
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/Not.A-UUID_at+all"),
        ),
    ]
}

/// RULE: a certificate ARN handed to a generic tagging operation is refused with HTTP
/// 400 `ValidationException` whose message states the called operation's supported
/// resource types, that the named resource is a certificate, and the
/// certificate-specific operation to use instead.
#[tokio::test]
async fn a_certificate_arn_is_refused_in_acms_own_wording() {
    let router = router();
    let requested = request_certificate(
        &router,
        &json!({ "DomainName": "refusal-text.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let generated = generate(&Blueprint {
        common_name: Some("refusal-text-imported.example.com"),
        ..Blueprint::default()
    });
    let imported = import(
        &router,
        "an imported certificate",
        &import_body(&generated),
    )
    .await;

    // The certificate path the refusals point at really does serve the write, so the
    // guidance in the message names an operation that works.
    let (status, response) = call(
        &router,
        "AddTagsToCertificate",
        &json!({ "CertificateArn": requested, "Tags": [{ "Key": "owner", "Value": "cert-api" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the certificate tagging API: expected 200, got {status} {response}"
    );

    for (what, arn) in arn_cases(&requested, &imported) {
        for (operation, members, certificate_operation) in operation_cases() {
            let mut body = json!({ "ResourceArn": arn });
            for (member, value) in members.as_object().expect("the members are an object") {
                body[member] = value.clone();
            }
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ValidationException"),
                "case {what:?} on {operation}: expected 400 ValidationException, got {status} \
                 {response}"
            );
            let stated = error_message(&response);
            let expected = expected_message(operation, &arn, certificate_operation);
            assert_eq!(
                stated, expected,
                "case {what:?} on {operation}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    // The refusals changed nothing: the certificate still carries exactly what its own
    // tagging API wrote.
    let (status, response) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": requested }),
    )
    .await;
    assert_eq!(
        (status, &response["Tags"]),
        (200, &json!([{ "Key": "owner", "Value": "cert-api" }])),
        "expected the certificate to keep only the tag AddTagsToCertificate wrote, got {status} \
         {response}"
    );
}

/// RULE (the other side of the same gate): a NON-certificate ACM resource ARN is served
/// by the generic operations, so the refusal wording above is reached only by the
/// certificate resource type.
#[tokio::test]
async fn a_non_certificate_arn_is_still_served() {
    let router = router();
    let case = "the non-certificate control";
    let endpoint = create_endpoint(&router, case).await;

    let (status, response) = call(
        &router,
        "TagResource",
        &json!({ "ResourceArn": endpoint, "Tags": [{ "Key": "env", "Value": "control" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TagResource on an acme-endpoint ARN must answer 200, got {status} \
         {response}"
    );
    let stored = stored_tags(&router, case, &endpoint).await;
    assert_eq!(
        stored,
        vec![("env".to_string(), "control".to_string())],
        "case {case:?}: expected the control tag to be readable back, got {stored:?}"
    );
}
