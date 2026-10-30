// SPDX-License-Identifier: CC0-1.0

//! The Amazon Web Services Private CA resource types a template may DECLARE: every operation
//! that submits a template for deployment admits them, and a stack that declares them is
//! created.
//!
//! The rule under test, which two crates have to agree on (CloudFormation decides what a
//! template may declare; Amazon Web Services Private CA serves the calls a declared resource
//! of one of those types is created by), so the suite lives here rather than in either crate:
//!
//! **an operation that submits a template for deployment admits every resource type the "AWS
//! resource and property types reference" publishes for a service namespace** — so the
//! submission answers with the identifier of the thing it created (a stack id, a change-set
//! id) and the stack is recorded, and a resource of a type this emulator provisions is really
//! created in the owning service — **while a name that reference does not publish is still
//! refused**, with HTTP 400, the wire code `ValidationError` and the message `Template format
//! error: Unrecognized resource types: [<types>]`, and nothing is created for it.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the resource NAME inside one published namespace — every type the reference publishes
//!   under `AWS::ACMPCA::` is a row, so the reported ones are rows among their siblings
//!   rather than special cases, and `AWS::ACMPCA::Permission` — the fourth published type,
//!   which the reported template does not carry — is a row beside them;
//! * the OPERATION that submits the body — `CreateStack`, `CreateChangeSet` and
//!   `UpdateStack` are the three held to the catalogue;
//! * the FORM the body is written in — JSON and YAML;
//! * for the refusing half, the near misses of the same namespace — a plural, a wrong case,
//!   an invented resource, the service model's own shape name as the namespace and its
//!   signing name as the namespace — so admitting the published set is not admitting
//!   `AWS::ACMPCA::<anything>`.
//!
//! What this suite asserts nothing about is what each provisioned type's deployment then
//! PRODUCES: the CA's configuration, tags and outputs are
//! `cloudformation_provisions_acmpca_certificate_authorities.rs`, and the certification
//! request, the issued certificate and the activation that imports it are
//! `cloudformation_activates_acmpca_certificate_authorities.rs`. Here the deployed row is the
//! mixed stack those two do not carry: a CA beside a permission over it, the fourth published
//! type of the namespace.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_recognized, AWS_RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{form_encode, opt_element, pairs_of, physical_ids_of};

/// How long the suite goes on polling for an outcome that is already due, and how often: a
/// loaded machine can be slower than the emulator's own spans without the rule being any
/// different.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS: usize = 40;

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The Region every row deploys in: which Region a stack lives in is
/// `cloudformation_provisions_acmpca_certificate_authorities.rs`' own axis, so it is held
/// still here.
const REGION: &str = "us-east-1";

/// The `StackStatus`/`ResourceStatus` values the rule is stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The namespace Amazon Web Services Private CA's resource types are published under, which
/// is the `cloudFormationName` of its own service model.
const PRIVATE_CA_NAMESPACE: &str = "AWS::ACMPCA::";

/// Every resource type the "AWS resource and property types reference" publishes under
/// `AWS::ACMPCA::`, each with the operation of the owning service — present in this
/// checkout's `acmpca` model, `smithy_query(query = "list_operations")` — that deploying one
/// calls.
///
/// Source: that reference's AWS Private CA section. This is the expectation side of the
/// emulator's catalogue: the first test holds `AWS_RESOURCE_TYPES`' own `AWS::ACMPCA::`
/// group to exactly this set, so a missing entry (the reported defect, which refused every
/// template declaring one) and a stray one both fail.
const PUBLISHED: &[&str] = &[
    // IssueCertificate
    "AWS::ACMPCA::Certificate",
    // CreateCertificateAuthority
    "AWS::ACMPCA::CertificateAuthority",
    // ImportCertificateAuthorityCertificate
    "AWS::ACMPCA::CertificateAuthorityActivation",
    // CreatePermission
    "AWS::ACMPCA::Permission",
];

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
///
/// Request authentication is off, as in every other suite here; the in-process callback
/// CloudFormation makes signs its request either way (the credential scope is what routes
/// it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for [`REGION`].
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", REGION))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one Amazon Web Services Private CA operation over its awsJson1_1 wire, signed for
/// [`REGION`].
async fn acm_pca(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("acm-pca.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
        .header("authorization", authorization("acm-pca", REGION))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Poll the stack until it reports `expected`, or answer the last status it reported with the
/// `DescribeStacks` body that reported it.
async fn wait_for_status(
    router: &MultiServiceRouter,
    stack: &str,
    expected: &str,
) -> (String, String) {
    for _ in 0..POLLS {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "DescribeStacks {stack}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if reported == expected {
            return (reported, body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (_, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Whether `arn` is a certificate-authority ARN of [`REGION`] in the account this emulator
/// mints, as `DescribeCertificateAuthority`'s own `CertificateAuthorityArn` `@pattern`
/// requires.
fn is_well_formed_ca_arn(arn: &str) -> bool {
    let prefix = format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/");
    arn.strip_prefix(&prefix).is_some_and(|id| {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+=,.@-".contains(c))
    })
}

/// A template declaring one resource of `resource_type` under the logical id `Subject`, with
/// no properties: a published type may be declared with whatever properties its own reference
/// gives it, and what the admission rule is about is the TYPE.
fn one_resource_template(resource_type: &str) -> String {
    json!({ "Resources": { "Subject": { "Type": resource_type } } }).to_string()
}

/// The same template in YAML, the other form a body may be written in.
fn one_resource_yaml(resource_type: &str) -> String {
    ["Resources:", "  Subject:", &format!("    Type: {resource_type}"), ""].join("\n")
}

/// The stack name a one-resource stack for `resource_type` is submitted under: the type's
/// last segment, lower-cased, which is inside a stack name's own alphabet.
fn stack_name_for(prefix: &str, resource_type: &str) -> String {
    let resource = resource_type
        .rsplit("::")
        .next()
        .unwrap_or(resource_type)
        .to_ascii_lowercase();
    format!("{prefix}-{resource}")
}

/// The catalogue's own `AWS::ACMPCA::` group, in catalogue order.
fn catalogued_private_ca_types() -> Vec<&'static str> {
    AWS_RESOURCE_TYPES
        .iter()
        .copied()
        .filter(|entry| entry.starts_with(PRIVATE_CA_NAMESPACE))
        .collect()
}

/// The catalogue carries exactly the published Amazon Web Services Private CA namespace —
/// every type the reference publishes under it, and nothing else.
#[test]
fn the_catalogue_carries_the_published_private_ca_namespace() {
    let mut expected: Vec<&str> = PUBLISHED.to_vec();
    expected.sort_unstable();
    let mut actual = catalogued_private_ca_types();
    actual.sort_unstable();
    assert_eq!(
        actual, expected,
        "case {PRIVATE_CA_NAMESPACE:?}: expected the catalogue to carry exactly the published \
         types {expected:?}, got {actual:?}"
    );
    for resource_type in PUBLISHED {
        assert!(
            is_recognized(resource_type),
            "case {resource_type:?}: expected a published type to be recognized, it is not"
        );
    }
}

/// Every published type may be declared by each operation that submits a template for
/// deployment: `CreateStack` answers with the stack it created and the stack is recorded,
/// `CreateChangeSet` answers with the change set it created, in either body form.
#[tokio::test]
async fn every_published_private_ca_type_may_be_declared() {
    let router = build_router();

    for resource_type in PUBLISHED {
        for (form, body) in [
            ("JSON", one_resource_template(resource_type)),
            ("YAML", one_resource_yaml(resource_type)),
        ] {
            let case = format!("{resource_type} declared in {form}");
            let stack_name = stack_name_for(&format!("private-ca-{form}"), resource_type);

            let (status, answer) = cloudformation(
                &router,
                "CreateStack",
                &[("StackName", &stack_name), ("TemplateBody", &body)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateStack for a published resource \
                 type, got {status} with body {answer}"
            );
            let stack_id = opt_element(&answer, "StackId").unwrap_or_default();
            assert!(
                stack_id.contains(&format!(":stack/{stack_name}/")),
                "case {case:?}: expected CreateStack to answer with the stack's id, got \
                 {stack_id:?} in {answer}"
            );

            // The stack really is there: the reported defect created none at all.
            let (status, described) =
                cloudformation(&router, "DescribeStacks", &[("StackName", &stack_name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeStacks to report the created stack, got \
                 {status} with body {described}"
            );
            assert_eq!(
                opt_element(&described, "StackId").as_deref(),
                Some(stack_id.as_str()),
                "case {case:?}: expected the stack to be the one CreateStack answered with, got \
                 body {described}"
            );

            // The other operation that submits a body for a stack that does not exist yet is
            // held to the same catalogue, and admits the same type.
            let change_set_stack = format!("{stack_name}-change-set");
            let (status, answer) = cloudformation(
                &router,
                "CreateChangeSet",
                &[
                    ("StackName", &change_set_stack),
                    ("ChangeSetName", "a-change-set"),
                    ("ChangeSetType", "CREATE"),
                    ("TemplateBody", &body),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateChangeSet for a published resource \
                 type, got {status} with body {answer}"
            );
            assert!(
                opt_element(&answer, "Id").is_some_and(|id| id.contains("changeSet/")),
                "case {case:?}: expected CreateChangeSet to answer with the change set's id, got \
                 body {answer}"
            );
        }
    }
}

/// A stack declaring a private CA beside a permission over it deploys: the CA is really
/// created in Amazon Web Services Private CA and reported under the ARN that service minted,
/// both resources are recorded under the types the template declared, and `UpdateStack`
/// admits a template declaring one more of the same published types.
#[tokio::test]
async fn a_stack_deploys_a_private_ca_beside_a_declared_only_permission() {
    let router = build_router();
    let case = "a root CA with a permission over it";
    let stack_name = "private-ca-with-a-permission";

    // `AWS::ACMPCA::Permission`'s own published properties: the CA the permission is over,
    // the service principal it is granted to and the actions it covers (the resource
    // reference's AWS::ACMPCA::Permission page).
    let permission = |principal: &str| {
        json!({
            "Type": "AWS::ACMPCA::Permission",
            "Properties": {
                "CertificateAuthorityArn": { "Ref": "Ca" },
                "Principal": principal,
                "Actions": ["IssueCertificate", "GetCertificate", "ListPermissions"]
            }
        })
    };
    let certificate_authority = json!({
        "Type": "AWS::ACMPCA::CertificateAuthority",
        "Properties": {
            "Type": "ROOT",
            "KeyAlgorithm": "RSA_2048",
            "SigningAlgorithm": "SHA256WITHRSA",
            "Subject": { "CommonName": "root.example-widgets.internal" }
        }
    });
    let mut resources = Map::new();
    resources.insert("Ca".to_string(), certificate_authority.clone());
    resources.insert(
        "AcmPermission".to_string(),
        permission("acm.amazonaws.com"),
    );
    let template = json!({ "Resources": Value::Object(resources) }).to_string();

    let (status, answer) = cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {answer}"
    );

    let (stack_status, described) = wait_for_status(&router, stack_name, CREATE_COMPLETE).await;
    assert_eq!(
        stack_status, CREATE_COMPLETE,
        "case {case:?}: expected the stack to complete, got {stack_status:?} ({described})"
    );

    let (status, reported) =
        cloudformation(&router, "DescribeStackResources", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status} with body \
         {reported}"
    );
    let identifiers = physical_ids_of(&reported);
    let declared_types = pairs_of(&reported, "LogicalResourceId", "ResourceType");
    assert_eq!(
        declared_types.get("Ca").map(String::as_str),
        Some("AWS::ACMPCA::CertificateAuthority"),
        "case {case:?}: expected the CA to be recorded under its declared type, got \
         {declared_types:?}"
    );
    assert_eq!(
        declared_types.get("AcmPermission").map(String::as_str),
        Some("AWS::ACMPCA::Permission"),
        "case {case:?}: expected the permission to be recorded under its declared type, got \
         {declared_types:?}"
    );
    let ca_arn = identifiers.get("Ca").cloned().unwrap_or_default();
    assert!(
        is_well_formed_ca_arn(&ca_arn),
        "case {case:?}: expected the CA to be reported under the ARN that service minted, got \
         {ca_arn:?} in {reported}"
    );
    // `CreatePermission` mints no identifier of its own (its output is empty), so what a
    // permission reports is a physical resource id of CloudFormation's own making — what
    // matters to a client is that the resource carries one.
    let permission_id = identifiers.get("AcmPermission").cloned().unwrap_or_default();
    assert!(
        !permission_id.is_empty(),
        "case {case:?}: expected the permission to carry a physical resource id, got \
         {permission_id:?} in {reported}"
    );

    // The owning service resolves the CA the stack created.
    let (status, authority) = acm_pca(
        &router,
        "DescribeCertificateAuthority",
        &json!({ "CertificateAuthorityArn": ca_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeCertificateAuthority to resolve {ca_arn:?}, got \
         {status} {authority:?}"
    );
    assert_eq!(
        authority
            .get("CertificateAuthority")
            .and_then(|ca| ca.get("Arn"))
            .and_then(Value::as_str),
        Some(ca_arn.as_str()),
        "case {case:?}: expected the CA to report its own ARN, got {authority:?}"
    );

    // `UpdateStack` is held to the same catalogue: a body declaring one more resource of a
    // published type of this namespace is admitted and deployed.
    let mut updated = Map::new();
    updated.insert("Ca".to_string(), certificate_authority);
    updated.insert(
        "AcmPermission".to_string(),
        permission("acm.amazonaws.com"),
    );
    updated.insert(
        "SecondPermission".to_string(),
        permission("ecs.amazonaws.com"),
    );
    let updated = json!({ "Resources": Value::Object(updated) }).to_string();
    let (status, answer) = cloudformation(
        &router,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &updated)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateStack to admit a template declaring the same published \
         types, got {status} with body {answer}"
    );
    let (stack_status, described) = wait_for_status(&router, stack_name, UPDATE_COMPLETE).await;
    assert_eq!(
        stack_status, UPDATE_COMPLETE,
        "case {case:?}: expected the update to complete, got {stack_status:?} ({described})"
    );
    let (status, reported) =
        cloudformation(&router, "DescribeStackResources", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackResources to answer after the update, got \
         {status} with body {reported}"
    );
    let identifiers = physical_ids_of(&reported);
    assert_eq!(
        identifiers.get("Ca").map(String::as_str),
        Some(ca_arn.as_str()),
        "case {case:?}: expected the CA's identity to survive the update, got {identifiers:?}"
    );
    assert!(
        identifiers.contains_key("SecondPermission"),
        "case {case:?}: expected the added permission to be recorded, got {identifiers:?}"
    );
}

/// One refusing row: a name of the same namespace the reference does not publish.
struct NearMiss {
    case: &'static str,
    /// The name part of the stack and change-set names the row submits under.
    name: &'static str,
    /// The unrecognised types the row declares, beside a published one.
    declared: &'static [&'static str],
    /// Whether the row writes its body in YAML rather than JSON.
    yaml: bool,
}

/// The near misses of the admitted namespace: admitting the published set must not admit
/// `AWS::ACMPCA::<anything>`.
const NEAR_MISSES: &[NearMiss] = &[
    NearMiss {
        case: "a published type, pluralised",
        name: "private-ca-plural",
        declared: &["AWS::ACMPCA::CertificateAuthorities"],
        yaml: false,
    },
    NearMiss {
        case: "a published type in the wrong case",
        name: "private-ca-wrong-case",
        declared: &["AWS::ACMPCA::certificateauthority"],
        yaml: false,
    },
    NearMiss {
        case: "a resource this namespace does not publish",
        name: "private-ca-invented",
        declared: &["AWS::ACMPCA::CertificateAuthorityAudit"],
        yaml: false,
    },
    NearMiss {
        case: "the service model's own shape name as the namespace",
        name: "private-ca-model-namespace",
        declared: &["AWS::ACMPrivateCA::CertificateAuthority"],
        yaml: false,
    },
    NearMiss {
        case: "the service's signing name as the namespace, in YAML",
        name: "private-ca-signing-name",
        declared: &["AWS::ACM-PCA::CertificateAuthority"],
        yaml: true,
    },
    NearMiss {
        case: "two unpublished names beside published ones",
        name: "private-ca-mixed",
        declared: &[
            "AWS::ACMPCA::CertificateAuthorityDeactivation",
            "AWS::ACMPCA::CertificateRevocation",
        ],
        yaml: false,
    },
];

/// The body a near-miss row submits: a published type of the namespace beside the
/// unpublished ones, so the refusal has something recognised to leave out.
fn near_miss_body(row: &NearMiss) -> String {
    if row.yaml {
        let mut lines = vec![
            "Resources:".to_string(),
            "  Permitted:".to_string(),
            "    Type: AWS::ACMPCA::Permission".to_string(),
        ];
        for (index, resource_type) in row.declared.iter().enumerate() {
            lines.push(format!("  Unknown{index}:"));
            lines.push(format!("    Type: {resource_type}"));
        }
        lines.push(String::new());
        return lines.join("\n");
    }
    let mut resources = Map::new();
    resources.insert(
        "Permitted".to_string(),
        json!({ "Type": "AWS::ACMPCA::Permission" }),
    );
    for (index, resource_type) in row.declared.iter().enumerate() {
        resources.insert(format!("Unknown{index}"), json!({ "Type": resource_type }));
    }
    json!({ "Resources": Value::Object(resources) }).to_string()
}

/// A name the reference does not publish is refused by every operation that submits a
/// template for deployment, in the recorded wording, and nothing is created for it.
#[tokio::test]
async fn a_name_the_private_ca_namespace_does_not_publish_is_refused() {
    let router = build_router();

    // A stack an `UpdateStack` row can be submitted against: the catalogue is applied to the
    // submitted body, so the stack it names must be one that exists for the row to be about
    // the body at all.
    let deployed = "private-ca-refusals-base";
    let (status, answer) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", deployed),
            (
                "TemplateBody",
                &one_resource_template("AWS::ACMPCA::Permission"),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the base stack\": expected HTTP 200 from CreateStack, got {status} with body \
         {answer}"
    );
    let (stack_status, described) = wait_for_status(&router, deployed, CREATE_COMPLETE).await;
    assert_eq!(
        stack_status, CREATE_COMPLETE,
        "case \"the base stack\": expected it to complete, got {stack_status:?} ({described})"
    );

    for row in NEAR_MISSES {
        let case = row.case;
        let body = near_miss_body(row);
        let change_set_stack = format!("{}-change-set", row.name);
        let submissions: [(&str, Vec<(&str, &str)>); 3] = [
            (
                "CreateStack",
                vec![("StackName", row.name), ("TemplateBody", &body)],
            ),
            (
                "CreateChangeSet",
                vec![
                    ("StackName", change_set_stack.as_str()),
                    ("ChangeSetName", "a-change-set"),
                    ("ChangeSetType", "CREATE"),
                    ("TemplateBody", &body),
                ],
            ),
            (
                "UpdateStack",
                vec![("StackName", deployed), ("TemplateBody", &body)],
            ),
        ];

        for (action, members) in submissions {
            let (status, answer) = cloudformation(&router, action, &members).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from {action} for an unpublished resource \
                 type, got {status} with body {answer}"
            );
            assert_eq!(
                opt_element(&answer, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected {action} to answer the wire code ValidationError, got \
                 body {answer}"
            );
            let message = opt_element(&answer, "Message").unwrap_or_default();
            // One unpublished type fixes the whole message; a row declaring more than one
            // fixes which types it names, in whatever order the body's resources were read.
            if let [resource_type] = row.declared {
                assert_eq!(
                    message,
                    format!("Template format error: Unrecognized resource types: [{resource_type}]"),
                    "case {case:?}: expected {action} to word the refusal as the service does, \
                     got {message:?}"
                );
            } else {
                assert!(
                    message.starts_with("Template format error: Unrecognized resource types: [")
                        && message.ends_with(']'),
                    "case {case:?}: expected {action} to word the refusal as the service does, \
                     got {message:?}"
                );
                for resource_type in row.declared {
                    assert!(
                        message.contains(resource_type),
                        "case {case:?}: expected the refusal from {action} to name \
                         {resource_type:?}, got {message:?}"
                    );
                }
            }
            assert!(
                !message.contains("AWS::ACMPCA::Permission"),
                "case {case:?}: expected the refusal from {action} not to name the published \
                 type declared beside them, got {message:?}"
            );
        }

        // Nothing was created for a refused body: neither the stack the create named nor the
        // one the change set named exists, and the stack the update named keeps what it had.
        for stack in [row.name, change_set_stack.as_str()] {
            let (status, answer) =
                cloudformation(&router, "DescribeStacks", &[("StackName", stack)]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected no stack {stack:?} to exist, DescribeStacks answered \
                 {status} with body {answer}"
            );
        }
        let (status, reported) =
            cloudformation(&router, "DescribeStackResources", &[("StackName", deployed)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated stack to still answer, got {status} with body \
             {reported}"
        );
        let identifiers = physical_ids_of(&reported);
        assert_eq!(
            identifiers.keys().cloned().collect::<Vec<String>>(),
            vec!["Subject".to_string()],
            "case {case:?}: expected the refused update to leave the stack's resources alone, \
             got {identifiers:?}"
        );
    }
}
