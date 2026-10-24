// SPDX-License-Identifier: CC0-1.0

//! Reads of a managed policy by ARN — `GetPolicy` and `GetPolicyVersion` — over the
//! awsQuery wire, for the policies AWS owns as well as the ones the account creates.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share.
//!
//! The rule being asserted: a read addressed by `PolicyArn` resolves the ARN against
//! the account's own policies AND AWS's catalog of AWS-managed policies (the
//! `arn:aws:iam::aws:policy/` namespace, which exists in every account), reports metadata
//! computed from state — `AttachmentCount` is the number of entities attached to the
//! policy at that instant — and reserves `NoSuchEntity` for an ARN naming a policy the
//! ACCOUNT does not hold, never for a policy AWS owns.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: the
//! policy's owner (AWS or the account), the ARN's path depth, the catalog entry (every
//! one of them, not only the entries a fixture cited), the number of attachments, and
//! the form of an ARN that names nothing.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::optional;
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A trust policy with a single statement — every role an attachment case needs one
/// to exist at all.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The document a policy this ACCOUNT creates carries, so every rule asserted of an
/// AWS-managed policy is asserted of a customer-managed one too.
const CUSTOMER_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// One `GetPolicy` call.
async fn get_policy(router: &MultiServiceRouter, policy_arn: &str) -> (u16, String) {
    call(router, "GetPolicy", &[("PolicyArn", policy_arn)]).await
}

/// One `GetPolicyVersion` call.
async fn get_policy_version(
    router: &MultiServiceRouter,
    policy_arn: &str,
    version_id: &str,
) -> (u16, String) {
    call(
        router,
        "GetPolicyVersion",
        &[("PolicyArn", policy_arn), ("VersionId", version_id)],
    )
    .await
}

/// Create a role at the account root, which every attachment case needs first.
async fn create_role(router: &MultiServiceRouter, role_name: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// Create a policy in the ACCOUNT, answering the ARN it reported.
async fn create_customer_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    document: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Attach `policy_arn` to `role_name`, which must succeed.
async fn attach(router: &MultiServiceRouter, role_name: &str, policy_arn: &str) {
    let (status, body) = call(
        router,
        "AttachRolePolicy",
        &[("RoleName", role_name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?} on {role_name:?}: AttachRolePolicy must succeed, got {status}: {body}"
    );
}

/// Detach `policy_arn` from `role_name`, which must succeed.
async fn detach(router: &MultiServiceRouter, role_name: &str, policy_arn: &str) {
    let (status, body) = call(
        router,
        "DetachRolePolicy",
        &[("RoleName", role_name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?} on {role_name:?}: DetachRolePolicy must succeed, got {status}: {body}"
    );
}

/// The `AttachmentCount` `GetPolicy` reports for `policy_arn`.
async fn reported_attachment_count(router: &MultiServiceRouter, policy_arn: &str) -> i64 {
    let (status, body) = get_policy(router, policy_arn).await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: GetPolicy must succeed, got {status}: {body}"
    );
    let reported = element(&body, "AttachmentCount");
    reported.parse().unwrap_or_else(|_| {
        panic!("case {policy_arn:?}: AttachmentCount must be an integer, got {reported:?}")
    })
}

/// `Policy$PolicyId` targets `idType` (length 16..128, pattern `^[\w]+$`) — the
/// modelled constraints on the unique id the service reports for a policy.
fn assert_well_formed_policy_id(case: &str, policy_id: &str) {
    assert!(
        policy_id.len() >= 16 && policy_id.len() <= 128,
        "case {case:?}: PolicyId must be 16..128 characters, got {policy_id:?}"
    );
    assert!(
        policy_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "case {case:?}: PolicyId must match ^[\\w]+$, got {policy_id:?}"
    );
}

/// Whether `version_id` is a policy version identifier: `policyVersionIdType`'s
/// pattern `^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`
/// (`smithy_query(query="operation", operation="GetPolicyVersion")`).
fn is_policy_version_id(version_id: &str) -> bool {
    let Some(digits) = version_id.strip_prefix('v') else {
        return false;
    };
    let number = digits.split('.').next().unwrap_or_default();
    !number.starts_with('0')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
        && digits
            .split_once('.')
            .is_none_or(|(_, rest)| rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// EVERY entry of the AWS-managed policy catalog is readable, and the version its
/// `DefaultVersionId` names serves its document: one row per catalog entry, so the
/// rule is asserted on the entries no fixture cited as well as on the three the
/// recorded suite attached.
#[tokio::test]
async fn every_catalog_policy_is_readable_and_serves_its_default_version() {
    let router = router();
    assert!(
        state::AWS_MANAGED_POLICIES.len() >= 2,
        "the catalog must carry more than one policy for this table to span anything, \
         got {}",
        state::AWS_MANAGED_POLICIES.len()
    );

    for entry in state::AWS_MANAGED_POLICIES {
        let arn = entry.arn();
        let case = arn.clone();

        let (status, body) = get_policy(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicy on an AWS-managed policy must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "Arn"),
            arn,
            "case {case:?}: the reported Arn must be the ARN asked for: {body}"
        );
        assert_eq!(
            element(&body, "PolicyName"),
            entry.policy_name,
            "case {case:?}: the reported PolicyName must be the catalog's: {body}"
        );
        assert_eq!(
            element(&body, "Path"),
            entry.path,
            "case {case:?}: the reported Path must be the catalog's: {body}"
        );
        assert_eq!(
            element(&body, "IsAttachable"),
            "true",
            "case {case:?}: an AWS-managed policy is published to be attached: {body}"
        );
        assert_eq!(
            element(&body, "AttachmentCount"),
            "0",
            "case {case:?}: nothing is attached in this account yet: {body}"
        );
        assert_eq!(
            element(&body, "PermissionsBoundaryUsageCount"),
            "0",
            "case {case:?}: nothing uses it as a permissions boundary yet: {body}"
        );
        let policy_id = element(&body, "PolicyId");
        assert_well_formed_policy_id(&case, &policy_id);
        assert_eq!(
            policy_id, entry.policy_id,
            "case {case:?}: the reported PolicyId must be the policy's stable id: {body}"
        );
        assert!(
            !element(&body, "CreateDate").is_empty(),
            "case {case:?}: GetPolicy must report a CreateDate: {body}"
        );
        assert_eq!(
            optional(&body, "Description"),
            None,
            "case {case:?}: the catalog carries no AWS description prose: {body}"
        );

        let default_version = element(&body, "DefaultVersionId");
        assert!(
            is_policy_version_id(&default_version),
            "case {case:?}: DefaultVersionId must be a policy version id, got \
             {default_version:?}"
        );

        let (status, body) = get_policy_version(&router, &arn, &default_version).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicyVersion must serve the default version \
             {default_version:?}, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "VersionId"),
            default_version,
            "case {case:?}: the reported VersionId must be the one asked for: {body}"
        );
        assert_eq!(
            element(&body, "IsDefaultVersion"),
            "true",
            "case {case:?}: the version GetPolicy named is the default one: {body}"
        );
        let document = element(&body, "Document");
        assert_eq!(
            document, entry.document,
            "case {case:?}: GetPolicyVersion must serve the catalog's document"
        );
        assert!(
            state::is_policy_document(&document),
            "case {case:?}: the served document must be an IAM policy document, got \
             {document:?}"
        );
    }
}

/// Every catalog entry is a well-formed managed policy, distinct from the others:
/// the table is the catalog itself, so an entry added later is checked too.
///
/// The constraints are the modelled ones (`policyPathType`, `policyNameType`,
/// `arnType`, `idType`, from `smithy_query(query="shape", shape="Policy")`) plus the
/// managed-policy size quota this crate records.
#[test]
fn catalog_entries_are_well_formed_and_distinct() {
    let mut arns: Vec<String> = Vec::new();
    let mut policy_ids: Vec<&str> = Vec::new();

    for entry in state::AWS_MANAGED_POLICIES {
        let arn = entry.arn();
        let case = arn.clone();

        assert!(
            entry.path.starts_with('/') && entry.path.ends_with('/'),
            "case {case:?}: policyPathType begins and ends with '/', got {:?}",
            entry.path
        );
        assert!(
            entry
                .path
                .trim_matches('/')
                .split('/')
                .all(|segment| !segment.is_empty() || entry.path == "/"),
            "case {case:?}: no path segment may be empty, got {:?}",
            entry.path
        );
        assert!(
            !entry.policy_name.is_empty() && entry.policy_name.len() <= 128,
            "case {case:?}: policyNameType is 1..128 characters, got {:?}",
            entry.policy_name
        );
        assert!(
            entry
                .policy_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_+=,.@-".contains(c)),
            "case {case:?}: policyNameType matches ^[\\w+=,.@-]+$, got {:?}",
            entry.policy_name
        );
        assert!(
            arn.len() >= 20 && arn.len() <= 2048,
            "case {case:?}: arnType is 20..2048 characters, got {}",
            arn.len()
        );
        assert!(
            state::is_policy_document(entry.document),
            "case {case:?}: the document must be an IAM policy document, got {:?}",
            entry.document
        );
        assert!(
            state::managed_policy_characters(entry.document)
                <= state::MAX_MANAGED_POLICY_CHARACTERS,
            "case {case:?}: the document must fit the managed-policy quota of {} \
             characters, got {}",
            state::MAX_MANAGED_POLICY_CHARACTERS,
            state::managed_policy_characters(entry.document)
        );
        assert!(
            state::is_aws_managed_policy_arn(&arn),
            "case {case:?}: a catalog entry's ARN is in AWS's own namespace"
        );
        assert_eq!(
            state::aws_managed_policy(&arn),
            Some(entry),
            "case {case:?}: the catalog must resolve its own entry by ARN"
        );
        assert_well_formed_policy_id(&case, entry.policy_id);
        // The two id spaces cannot collide: a catalog id carries `_`, which is not in
        // the base32 alphabet an id MINTED for an account-created policy is drawn from
        // (`state`'s `unique_id`), so no catalog id can ever be handed out by
        // `next_policy_id`.
        assert!(
            entry.policy_id.contains('_'),
            "case {case:?}: a catalog PolicyId must carry the `_` that keeps it out of \
             the minted id space, got {:?}",
            entry.policy_id
        );

        if let Some(previous) = arns.last() {
            assert!(
                previous < &arn,
                "the catalog is ordered by ARN and carries each policy once, but \
                 {previous:?} is not before {arn:?}"
            );
        }
        assert!(
            !policy_ids.contains(&entry.policy_id),
            "case {case:?}: a PolicyId must identify ONE policy, but {:?} repeats",
            entry.policy_id
        );
        policy_ids.push(entry.policy_id);
        arns.push(arn);
    }
}

/// `AttachmentCount` is the number of entities attached to the policy NOW, for a
/// policy AWS owns and for one the account created alike: one row per owner and per
/// ARN shape, walked up as attachments are added and back down as they are removed.
#[tokio::test]
async fn attachment_count_reports_the_entities_attached_now() {
    let router = router();
    let customer_arn = create_customer_policy(&router, "AuditReadOnly", CUSTOMER_DOCUMENT).await;

    let cases: Vec<(&str, String)> = vec![
        (
            "AWS-managed at the account root",
            "arn:aws:iam::aws:policy/AmazonS3FullAccess".to_string(),
        ),
        (
            "AWS-managed under /service-role/",
            "arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole".to_string(),
        ),
        (
            "AWS-managed, the policy the recorded suite read",
            "arn:aws:iam::aws:policy/AWSLakeFormationDataAdmin".to_string(),
        ),
        ("customer-managed", customer_arn),
    ];

    for (index, (case, arn)) in cases.iter().enumerate() {
        // Each row attaches to roles of its own, so one row's attachments are never
        // counted toward another's.
        let roles: Vec<String> = (0..3).map(|n| format!("counted-{index}-{n}")).collect();
        for role in &roles {
            create_role(&router, role).await;
        }

        assert_eq!(
            reported_attachment_count(&router, arn).await,
            0,
            "case {case:?}: a policy nothing is attached to is attached to nothing"
        );

        for (attached, role) in roles.iter().enumerate() {
            attach(&router, role, arn).await;
            let expected = i64::try_from(attached + 1).expect("a small count");
            assert_eq!(
                reported_attachment_count(&router, arn).await,
                expected,
                "case {case:?}: after {expected} attachments the count must be {expected}"
            );
        }

        // "A policy is either attached to a role or it is not": attaching one that is
        // already attached leaves the count where it is.
        attach(&router, &roles[0], arn).await;
        assert_eq!(
            reported_attachment_count(&router, arn).await,
            3,
            "case {case:?}: a repeat attach is the same attachment, so the count holds"
        );

        for (removed, role) in roles.iter().enumerate() {
            detach(&router, role, arn).await;
            let expected = i64::try_from(roles.len() - removed - 1).expect("a small count");
            assert_eq!(
                reported_attachment_count(&router, arn).await,
                expected,
                "case {case:?}: after a detach the count must fall to {expected}"
            );
        }
    }
}

/// One ARN that names no policy this emulator can report, and the answer it must get.
struct MissCase {
    name: &'static str,
    arn: String,
    status: u16,
    code: &'static str,
}

/// An ARN naming no readable policy is refused by its KIND: an account ARN the
/// account does not hold is the missing entity `NoSuchEntity` is for; a value that is
/// not a managed-policy ARN at all is an invalid input; and an ARN in AWS's own
/// namespace whose catalog entry this emulator does not carry is NOT reported as
/// absent — it answers 501, because AWS does serve that policy.
#[tokio::test]
async fn an_arn_outside_the_catalog_is_refused_by_its_kind() {
    let router = router();
    let account = account_id();
    let cases = vec![
        MissCase {
            name: "an account policy the account never created",
            arn: format!("arn:aws:iam::{account}:policy/NeverCreated"),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an account policy under a nested path, never created",
            arn: format!("arn:aws:iam::{account}:policy/eng/tools/NeverCreated"),
            status: 404,
            code: "NoSuchEntity",
        },
        MissCase {
            name: "an AWS-managed policy under /job-function/ the catalog does not carry",
            arn: "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess".to_string(),
            status: 501,
            code: "NotImplemented",
        },
        MissCase {
            name: "an AWS-managed policy at the root the catalog does not carry",
            arn: "arn:aws:iam::aws:policy/AmazonDynamoDBFullAccess".to_string(),
            status: 501,
            code: "NotImplemented",
        },
        MissCase {
            name: "an ARN of another service",
            arn: "arn:aws:s3:::a-bucket-named-here".to_string(),
            status: 400,
            code: "InvalidInput",
        },
        MissCase {
            name: "an IAM ARN naming a role rather than a policy",
            arn: format!("arn:aws:iam::{account}:role/a-role-not-a-policy"),
            status: 400,
            code: "InvalidInput",
        },
        MissCase {
            name: "a managed-policy ARN carrying a region, which IAM's never do",
            arn: "arn:aws:iam:us-east-1:aws:policy/AdministratorAccess".to_string(),
            status: 400,
            code: "InvalidInput",
        },
        MissCase {
            name: "a managed-policy ARN with an empty path segment",
            arn: "arn:aws:iam::aws:policy//AdministratorAccess".to_string(),
            status: 400,
            code: "InvalidInput",
        },
    ];

    for case in &cases {
        let name = case.name;
        let (status, body) = get_policy(&router, &case.arn).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: GetPolicy({:?}) must answer {}, got {status}: {body}",
            case.arn, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: GetPolicy({:?}) must answer the error code {:?}: {body}",
            case.arn,
            case.code
        );

        // The same ARN resolution answers the document read, so it refuses the same
        // way — a reader must not be able to tell two different stories about one ARN.
        let (status, body) = get_policy_version(&router, &case.arn, "v1").await;
        assert_eq!(
            status, case.status,
            "case {name:?}: GetPolicyVersion({:?}) must answer {}, got {status}: {body}",
            case.arn, case.status
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {name:?}: GetPolicyVersion({:?}) must answer the error code {:?}: {body}",
            case.arn,
            case.code
        );
    }
}

/// `GetPolicyVersion` serves exactly the versions the resolved policy carries: the
/// one a `CreatePolicy` minted, with the document it was created from, and the one
/// the catalog carries — and refuses every other version id as a missing entity,
/// whoever owns the policy.
#[tokio::test]
async fn get_policy_version_serves_only_the_versions_a_policy_carries() {
    let router = router();
    let customer_arn = create_customer_policy(&router, "ReportReader", CUSTOMER_DOCUMENT).await;

    // The round trip: the document a create stored is the document the reader serves.
    let (status, body) = get_policy_version(&router, &customer_arn, "v1").await;
    assert_eq!(
        status, 200,
        "a created policy's first version must be readable, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Document"),
        CUSTOMER_DOCUMENT,
        "the version must serve the document CreatePolicy stored"
    );
    assert_eq!(
        element(&body, "IsDefaultVersion"),
        "true",
        "CreatePolicy \"sets v1 as the policy's default version\": {body}"
    );

    let owners: Vec<(&str, String)> = vec![
        ("customer-managed", customer_arn),
        (
            "AWS-managed",
            "arn:aws:iam::aws:policy/AdministratorAccess".to_string(),
        ),
    ];
    // Version ids the model admits (`^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`) that no policy
    // in this account carries: the next one, a far one, and the dotted form.
    for (owner, arn) in &owners {
        for version in ["v2", "v3", "v10", "v1.0"] {
            let (status, body) = get_policy_version(&router, arn, version).await;
            assert_eq!(
                status, 404,
                "case {owner:?} {version:?}: a version the policy does not carry must \
                 answer 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {owner:?} {version:?}: the refusal must be NoSuchEntity: {body}"
            );
        }
    }
}
