// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateEnclaveCertificateIamRole` and the read that reports its
//! writes back, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=<Operation>`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rule under test is that this surface reads each ARN member of a request BY FORM —
//! `CertificateArn` as an ACM certificate ARN, `RoleArn` as an IAM role ARN — refusing any
//! other value with the code that says which of the two it will not take, and that an
//! accepted pair is STORED: the association is reported back with the bundle location
//! derived from the pair (the object key is `role_arn/certificate_arn`) and the one bucket
//! and encryption key the certificate's bundle was stored under, up to the documented
//! sixteen roles per certificate.
//!
//! Every table row is a distinct input along what that generalises over: the partition, the
//! Region and the account of each ARN, the form and the length of the certificate id, the
//! path and the characters of the role name, how many roles one certificate holds, and —
//! for the refusals — which member cannot be served and in which way.

use ec2::state::{
    uuid_suffixed_id_is_well_formed, ComAmazonawsEc2Amazonec2State as Ec2State,
    MAX_ENCLAVE_CERTIFICATE_IAM_ROLES,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for: the Region the bucket a certificate's bundle
/// is uploaded to is named for.
const REGION: &str = "us-east-1";

/// The ec2Query action under test.
const ASSOCIATE: &str = "AssociateEnclaveCertificateIamRole";

/// The read that reports what the association wrote.
const GET: &str = "GetAssociatedEnclaveCertificateIamRoles";

/// The code the surface refuses a value that is not an ACM certificate ARN with, and the
/// message it carries.
const CERTIFICATE_REFUSAL: (&str, &str) = (
    "InvalidCertificateArn.Malformed",
    "The request must contain a valid certificate arn",
);

/// The code the surface refuses a value that is not an IAM role ARN with, and its message.
const ROLE_REFUSAL: (&str, &str) = (
    "InvalidRoleArn.Malformed",
    "The request must contain a valid role arn",
);

/// A certificate ARN the suite associates when the row under test is about something else.
const A_CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:123456789012:certificate/0f2c1b7e-7f1a-4a0b-9f4d-5b8c9d0e1f23";

/// A role ARN the suite associates when the row under test is about something else.
const A_ROLE: &str = "arn:aws:iam::123456789012:role/enclave-certificate-role";

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One association of `role_arn` with `certificate_arn`, as its status and body.
async fn associate(
    router: &MultiServiceRouter,
    certificate_arn: &str,
    role_arn: &str,
) -> (u16, String) {
    call(
        router,
        ASSOCIATE,
        &[
            param("CertificateArn", certificate_arn),
            param("RoleArn", role_arn),
        ],
    )
    .await
}

/// The report of the roles `certificate_arn` holds, as its status and body.
async fn get(router: &MultiServiceRouter, certificate_arn: &str) -> (u16, String) {
    call(router, GET, &[param("CertificateArn", certificate_arn)]).await
}

/// The three members an association answers: the bucket, the object key and the KMS key id.
fn answered_bundle(case: &str, body: &str) -> (String, String, String) {
    let value = |name: &str| {
        element(body, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
    };
    (
        value("certificateS3BucketName"),
        value("certificateS3ObjectKey"),
        value("encryptionKmsKeyId"),
    )
}

/// Every role `GetAssociatedEnclaveCertificateIamRoles` reports for a certificate, as
/// (`associatedRoleArn`, `certificateS3BucketName`, `certificateS3ObjectKey`,
/// `encryptionKmsKeyId`) — read through the document so one item's members are never mixed
/// with another's.
fn reported_roles(case: &str, body: &str) -> Vec<(String, String, String, String)> {
    let parsed = document(case, body);
    let set = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "associatedRoleSet")
        .unwrap_or_else(|| panic!("case {case:?}: no <associatedRoleSet> in {body}"));
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(|item| {
            (
                child_text(item, "associatedRoleArn").unwrap_or_default(),
                child_text(item, "certificateS3BucketName").unwrap_or_default(),
                child_text(item, "certificateS3ObjectKey").unwrap_or_default(),
                child_text(item, "encryptionKmsKeyId").unwrap_or_default(),
            )
        })
        .collect()
}

/// The role ARNs the STORE holds for `certificate_arn` — what the "nothing was written"
/// assertions read.
fn stored_roles(engine: &engine::InMemoryEngine, certificate_arn: &str) -> Vec<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .enclave_certificate_iam_roles
        .get(certificate_arn)
        .map(|record| record.role_arns.iter().cloned().collect())
        .unwrap_or_default()
}

/// How many certificates the store holds an association for, whichever role they are with.
fn stored_certificates(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .enclave_certificate_iam_roles
        .len()
}

/// The bucket name an association reports in [`REGION`]: the fixed prefix, the Region and
/// the account the store belongs to.
fn expected_bucket() -> String {
    format!(
        "aws-ec2-enclave-certificate-{REGION}-{account}",
        account = engine::emulator_account_id()
    )
}

/// Is `value` shaped like a KMS key id — a bare UUID?
///
/// Read through the crate's own reader of that form
/// (`crate::state::uuid_suffixed_id_is_well_formed`, with the empty prefix), so the test
/// checks the id against the same rule the mint produces it under.
fn is_uuid(value: &str) -> bool {
    uuid_suffixed_id_is_well_formed(value, "")
}

/// A certificate ARN of exactly `characters` characters: the shortest well-formed one, with
/// its certificate id padded out. What the published length bound on an ACM ARN is tested
/// on.
fn certificate_arn_of_length(characters: usize) -> String {
    let stem = "arn:aws:acm:us-east-1:123456789012:certificate/";
    let padding = characters.saturating_sub(stem.len());
    format!("{stem}{}", "a".repeat(padding))
}

// ---------------------------------------------------------------------------
// What one association writes, and what reports it back
// ---------------------------------------------------------------------------

/// One row of the association table: the certificate and the role it names.
struct Row {
    case: &'static str,
    certificate_arn: String,
    role_arn: String,
}

/// The pairs the suite associates. Each row differs from the others along what the rule
/// generalises over — the partition, the Region and the account of either ARN, the form and
/// the length of the certificate id, and the path and the characters of the role name — so
/// no row is another one renamed.
///
/// The certificate forms come from ACM's own `Arn` shape, whose `@pattern` and `@length`
/// every `CertificateArn` member of that model carries
/// (`smithy_query(query="shape",shape="Arn",model_path="services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json")`),
/// and the role forms from the ARN `IamInstanceProfileSpecification$Arn`'s sibling members
/// document for an IAM role.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a UUID certificate id and a plain role name",
            certificate_arn: A_CERTIFICATE.to_string(),
            role_arn: A_ROLE.to_string(),
        },
        Row {
            case: "another partition, another Region and a role under a path",
            certificate_arn:
                "arn:aws-us-gov:acm:us-gov-west-1:000000000001:certificate/9a8b7c6d5e4f"
                    .to_string(),
            role_arn: "arn:aws-us-gov:iam::000000000001:role/enclaves/team/AcmRole".to_string(),
        },
        Row {
            case: "a one-digit account, a short certificate id and the punctuation a role \
                   name admits",
            certificate_arn: "arn:aws-cn:acm:cn-north-1:1:certificate/c".to_string(),
            role_arn: "arn:aws-cn:iam::1:role/role.name+with=punctuation@ok".to_string(),
        },
        Row {
            case: "a certificate ARN at the published maximum length",
            certificate_arn: certificate_arn_of_length(2048),
            role_arn: "arn:aws:iam::123456789012:role/another-enclave-role".to_string(),
        },
    ]
}

/// Every associated pair is stored, answered with the bundle the certificate's roles share
/// and the object key derived from the pair, and reported back by
/// `GetAssociatedEnclaveCertificateIamRoles` exactly as the association answered it. A
/// certificate nothing has been associated with reports no roles.
#[tokio::test]
async fn every_associated_role_is_reported_for_its_certificate() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, body) = get(&router, A_CERTIFICATE).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200, got {status} with body {body}",
        "a certificate nothing is associated with"
    );
    assert!(
        reported_roles("a certificate nothing is associated with", &body).is_empty(),
        "case {:?}: expected no roles, got {:?}",
        "a certificate nothing is associated with",
        reported_roles("a certificate nothing is associated with", &body)
    );

    for row in rows() {
        let case = row.case;
        let (status, body) = associate(&router, &row.certificate_arn, &row.role_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {ASSOCIATE}, got {status} with body {body}"
        );
        let (bucket, key, kms_key_id) = answered_bundle(case, &body);
        assert_eq!(
            bucket,
            expected_bucket(),
            "case {case:?}: expected the bucket {:?}, got {bucket:?}",
            expected_bucket()
        );
        assert_eq!(
            key,
            format!("{}/{}", row.role_arn, row.certificate_arn),
            "case {case:?}: expected the object key role_arn/certificate_arn, got {key:?}"
        );
        assert!(
            is_uuid(&kms_key_id),
            "case {case:?}: expected a UUID KMS key id, got {kms_key_id:?}"
        );

        assert_eq!(
            stored_roles(&engine, &row.certificate_arn),
            vec![row.role_arn.clone()],
            "case {case:?}: expected the store to hold exactly the associated role, got {:?}",
            stored_roles(&engine, &row.certificate_arn)
        );

        let (status, body) = get(&router, &row.certificate_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {GET}, got {status} with body {body}"
        );
        assert_eq!(
            reported_roles(case, &body),
            vec![(
                row.role_arn.clone(),
                bucket.clone(),
                key.clone(),
                kms_key_id.clone()
            )],
            "case {case:?}: expected {GET} to report the association as the associate \
             answered it, got {:?}",
            reported_roles(case, &body)
        );
    }
}

/// The roles of ONE certificate share that certificate's bucket and encryption key, are
/// reported in role-ARN order whatever order they were associated in, and a repeat of an
/// association in force neither adds a second role nor reports a different bundle. Two
/// certificates get encryption keys of their own.
#[tokio::test]
async fn one_certificate_reports_every_role_it_holds_under_one_bundle() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "three roles on one certificate";
    // Associated out of order, so the order reported is the store's and not the request's.
    let roles = [
        "arn:aws:iam::123456789012:role/third-role",
        "arn:aws:iam::123456789012:role/first-role",
        "arn:aws:iam::123456789012:role/second/path/role",
    ];
    let mut answered = Vec::new();
    for role_arn in roles {
        let (status, body) = associate(&router, A_CERTIFICATE, role_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 associating {role_arn}, got {status} with \
             body {body}"
        );
        answered.push(answered_bundle(case, &body));
    }
    let first = answered.first().expect("one row per associated role").clone();
    for (bucket, _, kms_key_id) in &answered {
        assert_eq!(
            (bucket.as_str(), kms_key_id.as_str()),
            (first.0.as_str(), first.2.as_str()),
            "case {case:?}: expected every role of one certificate to report one bucket and \
             one encryption key, got {bucket:?} and {kms_key_id:?}"
        );
    }

    // A repeat of an association already in force: the same answer, and still three roles.
    let (status, body) = associate(&router, A_CERTIFICATE, roles[0]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 repeating an association, got {status} with body \
         {body}"
    );
    assert_eq!(
        answered_bundle(case, &body),
        answered[0],
        "case {case:?}: expected a repeated association to answer what it answered first, \
         got {:?}",
        answered_bundle(case, &body)
    );

    let (status, body) = get(&router, A_CERTIFICATE).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {GET}, got {status} with body {body}"
    );
    let reported: Vec<String> = reported_roles(case, &body)
        .into_iter()
        .map(|(role_arn, _, _, _)| role_arn)
        .collect();
    let mut expected: Vec<String> = roles.iter().map(|role| (*role).to_string()).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected every role once, in role-ARN order, got {reported:?}"
    );
    assert_eq!(
        stored_roles(&engine, A_CERTIFICATE).len(),
        roles.len(),
        "case {case:?}: expected the store to hold one record per role, got {:?}",
        stored_roles(&engine, A_CERTIFICATE)
    );

    // A second certificate is a bundle of its own.
    let other_certificate =
        "arn:aws:acm:us-east-1:123456789012:certificate/11112222-3333-4444-5555-666677778888";
    let (status, body) = associate(&router, other_certificate, A_ROLE).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for a second certificate, got {status} with body \
         {body}"
    );
    let (_, _, other_kms_key_id) = answered_bundle(case, &body);
    assert_ne!(
        other_kms_key_id, first.2,
        "case {case:?}: expected a second certificate to report an encryption key of its \
         own, got {other_kms_key_id:?}"
    );
    assert_eq!(
        stored_certificates(&engine),
        2,
        "case {case:?}: expected the store to hold two certificates, got {}",
        stored_certificates(&engine)
    );
}

/// "You can associate up to 16 IAM roles with an ACM certificate"
/// (`AssociateEnclaveCertificateIamRoleRequest$RoleArn`): the sixteenth role is taken, the
/// seventeenth is refused and writes nothing, and a repeat of one of the sixteen still
/// answers the association in force.
#[tokio::test]
async fn the_role_past_the_documented_maximum_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "one role past the documented maximum";
    for index in 0..MAX_ENCLAVE_CERTIFICATE_IAM_ROLES {
        let role_arn = format!("arn:aws:iam::123456789012:role/enclave-role-{index}");
        let (status, body) = associate(&router, A_CERTIFICATE, &role_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for role {index}, got {status} with body {body}"
        );
    }
    let (status, body) = associate(
        &router,
        A_CERTIFICATE,
        "arn:aws:iam::123456789012:role/one-role-too-many",
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceLimitExceeded",
        "case {case:?}: expected ResourceLimitExceeded, got {:?} in {body}",
        error_code(&body)
    );
    assert_eq!(
        stored_roles(&engine, A_CERTIFICATE).len(),
        MAX_ENCLAVE_CERTIFICATE_IAM_ROLES,
        "case {case:?}: expected the refusal to write nothing, got {} roles",
        stored_roles(&engine, A_CERTIFICATE).len()
    );

    let (status, body) = associate(
        &router,
        A_CERTIFICATE,
        "arn:aws:iam::123456789012:role/enclave-role-0",
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a role already associated to be answered, got {status} with \
         body {body}"
    );
}

// ---------------------------------------------------------------------------
// What the surface will not take
// ---------------------------------------------------------------------------

/// The values that are not ACM certificate ARNs. Each is a different way of not being one:
/// absent, not an ARN at all, another service's ARN, another ACM resource type, no
/// certificate id, an empty Region, an account that is not digits, a multi-segment resource
/// id, a seventh ARN field, and one character past the published maximum length.
fn malformed_certificate_arns() -> Vec<(&'static str, String)> {
    vec![
        ("an empty value", String::new()),
        ("not an ARN at all", "a-certificate".to_string()),
        (
            "another service's ARN",
            "arn:aws:iam::123456789012:role/enclave-certificate-role".to_string(),
        ),
        (
            "another ACM resource type",
            "arn:aws:acm:us-east-1:123456789012:acme-endpoint/one".to_string(),
        ),
        (
            "no certificate id",
            "arn:aws:acm:us-east-1:123456789012:certificate/".to_string(),
        ),
        (
            "an empty Region",
            "arn:aws:acm::123456789012:certificate/one".to_string(),
        ),
        (
            "an account that is not digits",
            "arn:aws:acm:us-east-1:12a456789012:certificate/one".to_string(),
        ),
        (
            "a multi-segment resource id",
            "arn:aws:acm:us-east-1:123456789012:certificate/one/two".to_string(),
        ),
        (
            "a seventh ARN field",
            "arn:aws:acm:us-east-1:123456789012:certificate/one:extra".to_string(),
        ),
        (
            "one character past the published maximum length",
            certificate_arn_of_length(2049),
        ),
    ]
}

/// A certificate ARN this surface will not read is refused with the one code that says so,
/// writes nothing, and is refused the same way by the report as by the association.
#[tokio::test]
async fn a_value_that_is_not_a_certificate_arn_is_refused_by_both_operations() {
    let (router, engine) = super::ec2_query::emulator();
    let (expected_code, expected_message) = CERTIFICATE_REFUSAL;
    for (case, certificate_arn) in malformed_certificate_arns() {
        for action in [ASSOCIATE, GET] {
            let (status, body) = if action == ASSOCIATE {
                associate(&router, &certificate_arn, A_ROLE).await
            } else {
                get(&router, &certificate_arn).await
            };
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} on {action}: expected {expected_code}, got {:?} in {body}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?} on {action}: expected {expected_message:?}, got {:?}",
                error_message(&body)
            );
        }
        assert_eq!(
            stored_certificates(&engine),
            0,
            "case {case:?}: expected the refusal to write nothing, got {} certificates",
            stored_certificates(&engine)
        );
    }
}

/// The values that are not IAM role ARNs: absent, not an ARN, the certificate's own ARN, an
/// instance profile, the account's root user, an IAM user, a role ARN carrying a Region, an
/// account that is not digits, no role name, and a path-and-name past the bound IAM's own
/// quotas put on one.
fn malformed_role_arns() -> Vec<(&'static str, String)> {
    vec![
        ("an empty value", String::new()),
        ("not an ARN at all", "enclave-certificate-role".to_string()),
        ("the certificate's own ARN", A_CERTIFICATE.to_string()),
        (
            "an instance profile rather than a role",
            "arn:aws:iam::123456789012:instance-profile/enclave".to_string(),
        ),
        (
            "the account's root user",
            "arn:aws:iam::123456789012:root".to_string(),
        ),
        (
            "an IAM user rather than a role",
            "arn:aws:iam::123456789012:user/enclave".to_string(),
        ),
        (
            "a role ARN carrying a Region",
            "arn:aws:iam:us-east-1:123456789012:role/enclave".to_string(),
        ),
        (
            "an account that is not digits",
            "arn:aws:iam::12a456789012:role/enclave".to_string(),
        ),
        (
            "no role name",
            "arn:aws:iam::123456789012:role/".to_string(),
        ),
        (
            "a path and name past the bound IAM quotas put on one",
            format!(
                "arn:aws:iam::123456789012:role/{}",
                "r".repeat(512 + 64 + 1)
            ),
        ),
    ]
}

/// A role ARN this surface will not read is refused with the code that says it is the ROLE
/// the request got wrong, and writes nothing.
#[tokio::test]
async fn a_value_that_is_not_a_role_arn_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let (expected_code, expected_message) = ROLE_REFUSAL;
    for (case, role_arn) in malformed_role_arns() {
        let (status, body) = associate(&router, A_CERTIFICATE, &role_arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {:?} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            stored_certificates(&engine),
            0,
            "case {case:?}: expected the refusal to write nothing, got {} certificates",
            stored_certificates(&engine)
        );
    }
}

/// A dry run of either operation reports the permission outcome every caller of this
/// emulator gets and writes nothing — the request that would have succeeded leaves no
/// association behind.
#[tokio::test]
async fn a_dry_run_reports_the_permission_outcome_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let dry_runs: Vec<(&str, &str, Vec<Param>)> = vec![
        (
            "a dry run of the association",
            ASSOCIATE,
            vec![
                param("CertificateArn", A_CERTIFICATE),
                param("RoleArn", A_ROLE),
                param("DryRun", "true"),
            ],
        ),
        (
            "a dry run of the report",
            GET,
            vec![
                param("CertificateArn", A_CERTIFICATE),
                param("DryRun", "true"),
            ],
        ),
    ];
    for (case, action, params) in dry_runs {
        let (status, body) = call(&router, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {:?} in {body}",
            error_code(&body)
        );
        assert_eq!(
            stored_certificates(&engine),
            0,
            "case {case:?}: expected the dry run to write nothing, got {} certificates",
            stored_certificates(&engine)
        );
    }
}

/// The report reads the store rather than the request: a role associated through the service
/// is reported, and nothing a report does changes what a later one sees.
#[tokio::test]
async fn the_report_is_a_pure_read_of_the_store() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "repeated reports of one association";
    let (status, body) = associate(&router, A_CERTIFICATE, A_ROLE).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let answered = answered_bundle(case, &body);

    let mut reports = Vec::new();
    for _ in 0..3 {
        let (status, body) = get(&router, A_CERTIFICATE).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {GET}, got {status} with body {body}"
        );
        reports.push(reported_roles(case, &body));
    }
    let expected = vec![(
        A_ROLE.to_string(),
        answered.0.clone(),
        answered.1.clone(),
        answered.2.clone(),
    )];
    for report in &reports {
        assert_eq!(
            report, &expected,
            "case {case:?}: expected every report to read the one stored association, got \
             {report:?}"
        );
    }
    assert_eq!(
        stored_roles(&engine, A_CERTIFICATE),
        vec![A_ROLE.to_string()],
        "case {case:?}: expected the reports to leave the store as it was, got {:?}",
        stored_roles(&engine, A_CERTIFICATE)
    );
}

/// The ec2Query envelope of a successful association: the three members it answers, read as
/// direct children of the response element, plus the `requestId` every EC2 response carries.
#[tokio::test]
async fn the_association_answers_its_three_members_in_the_ec2_query_envelope() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the association envelope";
    let (status, body) = associate(&router, A_CERTIFICATE, A_ROLE).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let root = parsed.root_element();
    assert_eq!(
        root.tag_name().name(),
        format!("{ASSOCIATE}Response"),
        "case {case:?}: expected a <{ASSOCIATE}Response> root, got {body}"
    );
    for name in [
        "requestId",
        "certificateS3BucketName",
        "certificateS3ObjectKey",
        "encryptionKmsKeyId",
    ] {
        let value = child_text(root, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"));
        assert!(
            !value.is_empty(),
            "case {case:?}: expected <{name}> to carry a value, got {value:?}"
        );
    }
    // The report's envelope carries the set, with one item per associated role.
    let (status, body) = get(&router, A_CERTIFICATE).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {GET}, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    assert_eq!(
        set_items(parsed.root_element(), "associatedRoleSet").len(),
        1,
        "case {case:?}: expected one <associatedRoleSet> item, got {body}"
    );
}
