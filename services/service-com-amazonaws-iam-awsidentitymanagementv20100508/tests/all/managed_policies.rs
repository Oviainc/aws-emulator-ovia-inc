// SPDX-License-Identifier: CC0-1.0

//! `CreatePolicy` and its reporting half `GetPolicy`, end to end over the awsQuery
//! wire, plus the created ARN's use as a role's permissions boundary.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is
//! the HTTP behaviour (status, awsQuery error code, XML members) of a managed policy
//! that was PERSISTED, never a handler called directly. The awsQuery harness is the
//! one `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The suites are tables: each row differs along what the rule being asserted
//! generalises over (the path, the optional members, the tag count, the case of a
//! name, the form of a policy document, the document's size against the quota), so
//! no single recorded request is the specification.

use std::collections::BTreeMap;
use std::sync::Arc;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags, router_on};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s
/// own documentation describes ("the JSON policy document that you want to use as
/// the content for the new policy").
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:GetCallerIdentity","Resource":"*"}]}"#;

/// A managed policy ARN is `arn:aws:iam::<account>:policy<path><name>` — IAM is
/// global, so the region segment is EMPTY.
fn expected_policy_arn(path: &str, policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy{path}{policy_name}", account_id())
}

/// `Policy$PolicyId` targets `idType` (length 16..128, pattern `^[\w]+$`) — the
/// modelled constraints on the unique id the service MINTS (never echoes).
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

/// A policy document whose size against the managed-policy quota is exactly
/// `nonspace` characters, padded with `whitespace` space characters that the quota
/// does not count (AWS: IAM "doesn't count whitespace when calculating the size of
/// a policy against this limitation").
fn document_of_size(nonspace: usize, whitespace: usize) -> String {
    let prefix = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:GetCallerIdentity","Resource":"*","Sid":""#;
    let suffix = r#""}]}"#;
    let fixed = prefix.len() + suffix.len();
    assert!(
        nonspace >= fixed,
        "a document cannot be shorter than its own envelope ({fixed} characters)"
    );
    let document = format!(
        "{prefix}{}{}{suffix}",
        "a".repeat(nonspace - fixed),
        " ".repeat(whitespace)
    );
    assert_eq!(
        state::policy_document_characters(&document),
        nonspace,
        "the fixture must measure exactly {nonspace} characters against the quota"
    );
    document
}

/// One `CreatePolicy` request, with the members the row exercises.
struct CreateCase {
    name: &'static str,
    policy_name: &'static str,
    path: Option<&'static str>,
    /// The path the policy must REPORT: the request's, or `/` when it named none.
    expected_path: &'static str,
    description: Option<&'static str>,
    tags: Vec<(String, String)>,
    document: String,
}

impl CreateCase {
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            param("PolicyName", self.policy_name),
            param("PolicyDocument", &self.document),
        ];
        if let Some(path) = self.path {
            params.push(param("Path", path));
        }
        if let Some(description) = self.description {
            params.push(param("Description", description));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }
}

/// `CreatePolicy` answers from the record it persisted, and `GetPolicy` reads that
/// same record back by ARN: one row per thing the rule generalises over — the path
/// (absent, root, one segment, nested), the name's case and alphabet, the optional
/// `Description`, the tag count (none, one, two, the `tagListType` bound of 50) and
/// the form of the policy document.
#[tokio::test]
async fn create_persists_every_member_and_get_policy_reads_it_back() {
    let fifty_tags: Vec<(String, String)> = (0..50)
        .map(|index| (format!("key-{index:02}"), format!("value-{index:02}")))
        .collect();

    let cases = [
        CreateCase {
            name: "no path, no description, no tags",
            policy_name: "boundary-1",
            path: None,
            expected_path: "/",
            description: None,
            tags: Vec::new(),
            document: POLICY_DOCUMENT.to_string(),
        },
        CreateCase {
            name: "the root path spelled out",
            policy_name: "boundary-2",
            path: Some("/"),
            expected_path: "/",
            description: Some("Grants nothing but a caller-identity read."),
            tags: vec![("team".to_string(), "storage".to_string())],
            document: POLICY_DOCUMENT.to_string(),
        },
        CreateCase {
            name: "a one-segment path",
            policy_name: "MixedCaseName",
            path: Some("/boundaries/"),
            expected_path: "/boundaries/",
            description: None,
            tags: vec![
                ("team".to_string(), "eng".to_string()),
                ("cost-center".to_string(), "42".to_string()),
            ],
            document: POLICY_DOCUMENT.to_string(),
        },
        CreateCase {
            name: "a nested path and every allowed name character",
            policy_name: "policy_with+every=allowed.char@-",
            path: Some("/team/eng/tools/"),
            expected_path: "/team/eng/tools/",
            description: Some("x"),
            tags: Vec::new(),
            document: POLICY_DOCUMENT.to_string(),
        },
        CreateCase {
            name: "a document with no Version and a single statement object",
            policy_name: "boundary-3",
            path: Some("/boundaries/nested/"),
            expected_path: "/boundaries/nested/",
            description: None,
            tags: Vec::new(),
            document: r#"{"Statement":{"Effect":"Deny","Action":"*","Resource":"*"}}"#.to_string(),
        },
        CreateCase {
            name: "a document carrying an Id and two statements",
            policy_name: "boundary-4",
            path: None,
            expected_path: "/",
            description: Some("Two statements, one with a condition."),
            tags: vec![("team".to_string(), "sales".to_string())],
            document: r#"{"Version":"2012-10-17","Id":"doc-1","Statement":[{"Effect":"Allow","Action":["sts:GetCallerIdentity"],"Resource":"*"},{"Effect":"Deny","Action":"workmailmessageflow:GetRawMessageContent","Resource":"*","Condition":{"Bool":{"aws:SecureTransport":"false"}}}]}"#.to_string(),
        },
        CreateCase {
            name: "the tagListType bound of fifty tags",
            policy_name: "boundary-5",
            path: Some("/boundaries/"),
            expected_path: "/boundaries/",
            description: None,
            tags: fifty_tags,
            document: POLICY_DOCUMENT.to_string(),
        },
    ];

    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();
    for case in &cases {
        let params = case.params();
        let (status, body) = call(&router, "CreatePolicy", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreatePolicy must succeed, got {status}: {body}",
            case.name
        );

        let expected_arn = expected_policy_arn(case.expected_path, case.policy_name);
        for (member, expected) in [
            ("PolicyName", case.policy_name.to_string()),
            ("Path", case.expected_path.to_string()),
            ("Arn", expected_arn.clone()),
            // "This operation creates a policy version with a version identifier of
            // v1 and sets v1 as the policy's default version."
            ("DefaultVersionId", "v1".to_string()),
            // A new policy is attached to nothing and bounds nobody.
            ("AttachmentCount", "0".to_string()),
            ("PermissionsBoundaryUsageCount", "0".to_string()),
            // A customer managed policy can be attached to a user, group or role.
            ("IsAttachable", "true".to_string()),
        ] {
            assert_eq!(
                element(&body, member),
                expected,
                "case {:?}: expected {member} {expected:?}, got {body}",
                case.name
            );
        }

        assert_eq!(
            optional(&body, "Description"),
            case.description.map(str::to_string),
            "case {:?}: Description must be reported exactly when the request carried \
             one, got {body}",
            case.name
        );
        assert_eq!(
            reported_tags(&body),
            case.tags.iter().cloned().collect::<BTreeMap<_, _>>(),
            "case {:?}: expected the tags the request carried, got {body}",
            case.name
        );

        let policy_id = element(&body, "PolicyId");
        assert_well_formed_policy_id(case.name, &policy_id);
        minted_ids.push(policy_id);

        // "When a policy has only one version, this field contains the date and time
        // when the policy was created" (`Policy$UpdateDate`).
        let create_date = element(&body, "CreateDate");
        assert!(
            !create_date.is_empty(),
            "case {:?}: CreateDate must be reported, got {body}",
            case.name
        );
        assert_eq!(
            element(&body, "UpdateDate"),
            create_date,
            "case {:?}: a one-version policy must report UpdateDate as its CreateDate, \
             got {body}",
            case.name
        );

        // The reporting half reads the SAME record back, addressed by the ARN the
        // create answered.
        let (get_status, got) = call(&router, "GetPolicy", &[("PolicyArn", &expected_arn)]).await;
        assert_eq!(
            get_status, 200,
            "case {:?}: GetPolicy on the created ARN must succeed, got {get_status}: {got}",
            case.name
        );
        for member in [
            "PolicyName",
            "PolicyId",
            "Path",
            "Arn",
            "DefaultVersionId",
            "AttachmentCount",
            "PermissionsBoundaryUsageCount",
            "IsAttachable",
            "Description",
            "CreateDate",
            "UpdateDate",
        ] {
            assert_eq!(
                optional(&got, member),
                optional(&body, member),
                "case {:?}: GetPolicy must report the same {member} as the create, got {got}",
                case.name
            );
        }
        assert_eq!(
            reported_tags(&got),
            reported_tags(&body),
            "case {:?}: GetPolicy must report the same tags as the create, got {got}",
            case.name
        );
    }

    minted_ids.sort();
    let distinct = minted_ids.len();
    minted_ids.dedup();
    assert_eq!(
        minted_ids.len(),
        distinct,
        "every policy must get its own PolicyId, got {minted_ids:?}"
    );
}

/// Policy names are unique within the account and NOT distinguished by case
/// (`CreatePolicy$PolicyName`: "IAM user, group, role, and policy names must be
/// unique within the account. Names are not distinguished by case."), so a second
/// create under any casing of a held name — even under a different path — is refused
/// and leaves the first policy untouched.
#[tokio::test]
async fn a_held_policy_name_is_refused_whatever_its_case() {
    let router = router();
    let (status, created) = call(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", "MyPolicy"),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(status, 200, "the first create must succeed, got {created}");
    let held_arn = element(&created, "Arn");
    let held_id = element(&created, "PolicyId");

    for (case, name, path) in [
        ("the same case", "MyPolicy", None),
        ("all lower case", "mypolicy", None),
        ("all upper case", "MYPOLICY", None),
        ("another case under another path", "myPolicy", Some("/other/")),
    ] {
        let mut params = vec![
            param("PolicyName", name),
            param("PolicyDocument", POLICY_DOCUMENT),
        ];
        if let Some(path) = path {
            params.push(param("Path", path));
        }
        let (status, body) = call(&router, "CreatePolicy", &borrowed(&params)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 for the held name {name:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    let (get_status, got) = call(&router, "GetPolicy", &[("PolicyArn", &held_arn)]).await;
    assert_eq!(
        get_status, 200,
        "the refused creates must leave the held policy readable, got {got}"
    );
    assert_eq!(
        element(&got, "PolicyId"),
        held_id,
        "the refused creates must not replace the held policy, got {got}"
    );
}

/// A managed policy's content must be an IAM policy document — a JSON object
/// carrying a `Statement` element, the rule `crate::state::is_policy_document`
/// holds for every operation in this crate that takes one — and anything else is
/// refused with the modelled `MalformedPolicyDocumentException`, whatever
/// `policyDocumentType`'s own pattern allows. One row per form the rule
/// generalises over.
#[tokio::test]
async fn a_policy_document_that_is_not_a_json_policy_is_refused() {
    let refused = [
        ("not JSON at all", "this is not a policy"),
        ("a JSON string", r#""Statement""#),
        ("a JSON array", r#"[{"Effect":"Allow"}]"#),
        ("a JSON number", "17"),
        ("a JSON null", "null"),
        ("an object with no Statement", r#"{"Version":"2012-10-17"}"#),
        (
            "an object whose Statement key is misspelled",
            r#"{"Version":"2012-10-17","Statements":[]}"#,
        ),
        ("truncated JSON", r#"{"Statement":[{"Effect":"Allow""#),
    ];

    let router = router();
    for (index, (case, document)) in refused.iter().enumerate() {
        let policy_name = format!("malformed-{index}");
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[("PolicyName", &policy_name), ("PolicyDocument", document)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {document:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "MalformedPolicyDocument",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        // A refused create persists nothing a later caller can read.
        let (get_status, got) = call(
            &router,
            "GetPolicy",
            &[("PolicyArn", &expected_policy_arn("/", &policy_name))],
        )
        .await;
        assert_eq!(
            get_status, 404,
            "case {case:?}: a refused create must persist nothing, got {get_status}: {got}"
        );
    }
}

/// A managed policy document is bounded at `state::MAX_MANAGED_POLICY_CHARACTERS`
/// characters, whitespace excluded: at the published bound a create succeeds, one
/// character past it is refused with the modelled `LimitExceededException`, and
/// whitespace does not count toward the measure however much of it the document
/// carries.
#[tokio::test]
async fn the_managed_policy_size_quota_refuses_one_character_too_many() {
    let bound = state::MAX_MANAGED_POLICY_CHARACTERS;
    for (case, nonspace, whitespace, expected_status) in [
        ("well under the bound", 200, 0, 200),
        ("one character short of the bound", bound - 1, 0, 200),
        ("exactly at the bound", bound, 0, 200),
        ("at the bound plus whitespace the quota ignores", bound, 4_096, 200),
        ("one character past the bound", bound + 1, 0, 409),
        ("far past the bound", bound * 2, 0, 409),
    ] {
        let router = router();
        let document = document_of_size(nonspace, whitespace);
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[("PolicyName", "sized"), ("PolicyDocument", &document)],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: with {nonspace} counted characters, expected HTTP \
             {expected_status}, got {status}: {body}"
        );
        let (get_status, got) = call(
            &router,
            "GetPolicy",
            &[("PolicyArn", &expected_policy_arn("/", "sized"))],
        )
        .await;
        if expected_status == 200 {
            assert_eq!(
                get_status, 200,
                "case {case:?}: the created policy must be readable, got {get_status}: {got}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            assert_eq!(
                get_status, 404,
                "case {case:?}: the refused create must persist nothing, got {got}"
            );
        }
    }
}

/// AWS reserves the `aws:` tag-key prefix, so a create naming one is refused with
/// the modelled `InvalidInputException` and persists nothing; a key that merely
/// CONTAINS those characters is fine.
#[tokio::test]
async fn a_reserved_tag_key_is_refused() {
    let router = router();
    for (index, (case, key)) in [
        ("lower case prefix", "aws:cost-center"),
        ("upper case prefix", "AWS:CostCenter"),
        ("mixed case prefix", "Aws:team"),
        ("the bare prefix", "aws:"),
    ]
    .iter()
    .enumerate()
    {
        let policy_name = format!("reserved-{index}");
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", &policy_name),
                ("PolicyDocument", POLICY_DOCUMENT),
                ("Tags.member.1.Key", "team"),
                ("Tags.member.1.Value", "storage"),
                ("Tags.member.2.Key", key),
                ("Tags.member.2.Value", "x"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for the reserved key {key:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        let (get_status, got) = call(
            &router,
            "GetPolicy",
            &[("PolicyArn", &expected_policy_arn("/", &policy_name))],
        )
        .await;
        assert_eq!(
            get_status, 404,
            "case {case:?}: a refused create must persist nothing, got {get_status}: {got}"
        );
    }

    for (index, (case, key)) in [
        ("the prefix in the middle", "team-aws:owner"),
        ("the prefix without its colon", "awsteam"),
        ("a value that carries the prefix", "team"),
    ]
    .iter()
    .enumerate()
    {
        let policy_name = format!("allowed-{index}");
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", &policy_name),
                ("PolicyDocument", POLICY_DOCUMENT),
                ("Tags.member.1.Key", key),
                ("Tags.member.1.Value", "aws:value"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the key {key:?} to be accepted, got {status}: {body}"
        );
    }
}

/// `GetPolicy` distinguishes a value it cannot use as a managed-policy ARN (HTTP 400,
/// under the code of the layer that took the form: `ValidationError` for a value that is
/// not an ARN at all, the modelled `InvalidInputException` for an ARN naming another
/// service or resource type) from a well-formed ARN the account does not hold (the
/// modelled `NoSuchEntityException`, HTTP 404). Every row is at least 20 characters, so
/// `arnType`'s own length constraint is satisfied and the handler is what answers.
#[tokio::test]
async fn get_policy_separates_an_invalid_arn_from_an_unheld_one() {
    let router = router();
    let (status, created) = call(
        &router,
        "CreatePolicy",
        &[("PolicyName", "held"), ("PolicyDocument", POLICY_DOCUMENT)],
    )
    .await;
    assert_eq!(status, 200, "the fixture create must succeed, got {created}");

    for (case, arn, expected_code) in [
        (
            "a bare string of the right length",
            "not-an-arn-at-all-xx",
            "ValidationError",
        ),
        (
            "a role ARN",
            &format!("arn:aws:iam::{}:role/some-role", account_id()),
            "InvalidInput",
        ),
        (
            "a user ARN",
            &format!("arn:aws:iam::{}:user/some-user", account_id()),
            "InvalidInput",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket-name",
            "InvalidInput",
        ),
        (
            "an IAM policy ARN carrying a region",
            &format!("arn:aws:iam:us-west-2:{}:policy/held", account_id()),
            "InvalidInput",
        ),
        (
            "a policy ARN with no partition",
            &format!("arn::iam::{}:policy/held", account_id()),
            "InvalidInput",
        ),
    ] {
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code}, got {body}"
        );
    }

    // An ARN naming no policy the ACCOUNT holds is the missing entity: every row is
    // in the account's own namespace, because the `arn:aws:iam::aws:policy/` namespace
    // is AWS's and a policy AWS owns is never reported as absent — that resolution is
    // asserted in `tests/all/aws_managed_policy_catalog.rs`.
    for (case, arn) in [
        ("another name at the root", expected_policy_arn("/", "absent")),
        (
            "the held name under another path",
            expected_policy_arn("/elsewhere/", "held"),
        ),
        (
            "a name under a nested path",
            expected_policy_arn("/a/b/c/", "absent"),
        ),
    ] {
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for the unheld ARN {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // A read changes nothing a later caller sees: the held policy still reads back
    // exactly as it was created. The envelope's per-request `RequestId` is the one
    // thing that legitimately differs between two responses, so the comparison is
    // over the reported `Policy` members.
    let held_arn = element(&created, "Arn");
    let (first_status, first) = call(&router, "GetPolicy", &[("PolicyArn", &held_arn)]).await;
    let (second_status, second) = call(&router, "GetPolicy", &[("PolicyArn", &held_arn)]).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "both reads of a held policy must succeed, got {first} then {second}"
    );
    for member in [
        "PolicyName",
        "PolicyId",
        "Path",
        "Arn",
        "DefaultVersionId",
        "AttachmentCount",
        "PermissionsBoundaryUsageCount",
        "IsAttachable",
        "Description",
        "CreateDate",
        "UpdateDate",
    ] {
        assert_eq!(
            optional(&first, member),
            optional(&second, member),
            "two reads of one policy must report the same {member}, got {first} then {second}"
        );
        assert_eq!(
            optional(&first, member),
            optional(&created, member),
            "a read must report the created {member} unchanged, got {first}"
        );
    }
}

/// The point of the ARN `CreatePolicy` answers: it is usable as a role's
/// `PermissionsBoundary`, so the bounded role reports the boundary the account
/// holds. One row per path the policy was created under, since the ARN the role
/// carries is the one the create minted.
#[tokio::test]
async fn a_created_policy_arn_is_usable_as_a_role_permissions_boundary() {
    let trust = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"workmail.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;
    let router = router();
    for (index, (case, path)) in [
        ("the root path", None),
        ("a one-segment path", Some("/boundaries/")),
        ("a nested path", Some("/team/eng/")),
    ]
    .iter()
    .enumerate()
    {
        let policy_name = format!("wmf-boundary-{index}");
        let mut params = vec![
            param("PolicyName", &policy_name),
            param("PolicyDocument", POLICY_DOCUMENT),
        ];
        if let Some(path) = path {
            params.push(param("Path", path));
        }
        let (status, created) = call(&router, "CreatePolicy", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePolicy must succeed, got {status}: {created}"
        );
        let boundary_arn = element(&created, "Arn");
        assert_eq!(
            boundary_arn,
            expected_policy_arn(path.unwrap_or("/"), &policy_name),
            "case {case:?}: expected the policy ARN the path and name give, got {created}"
        );

        let role_name = format!("wmf-bounded-{index}");
        let (role_status, role) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", &role_name),
                ("AssumeRolePolicyDocument", trust),
                ("PermissionsBoundary", &boundary_arn),
            ],
        )
        .await;
        assert_eq!(
            role_status, 200,
            "case {case:?}: the created policy ARN must be usable as a boundary, got \
             {role_status}: {role}"
        );

        let (get_status, got) = call(&router, "GetRole", &[("RoleName", &role_name)]).await;
        assert_eq!(
            get_status, 200,
            "case {case:?}: GetRole must report the bounded role, got {get_status}: {got}"
        );
        assert_eq!(
            optional(&got, "PermissionsBoundaryArn"),
            Some(boundary_arn.clone()),
            "case {case:?}: the role must report the boundary ARN the policy was \
             created with, got {got}"
        );
        assert_eq!(
            optional(&got, "PermissionsBoundaryType"),
            Some(state::PERMISSIONS_BOUNDARY_TYPE.to_string()),
            "case {case:?}: the role must report the one modelled boundary type, got {got}"
        );
    }
}

/// A created customer-managed policy is one the account HOLDS, which is what makes
/// it attachable: `AttachRolePolicy` records the ARN `CreatePolicy` answered, while
/// an ARN of the same form the account does not hold refers to no entity. The
/// policy store is the one record of which customer-managed policies exist, so the
/// create and the attach cannot disagree about it.
#[tokio::test]
async fn a_created_policy_is_attachable_and_an_unheld_one_is_not() {
    let trust = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"workmail.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;
    let router = router();
    let (role_status, role) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "attachable-role"),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(role_status, 200, "the fixture role must be created, got {role}");

    // One row per path the policy was created under, since the ARN the attach takes
    // is the one the create minted.
    let mut attached: Vec<String> = Vec::new();
    for (index, (case, path)) in [
        ("the root path", None),
        ("a one-segment path", Some("/boundaries/")),
        ("a nested path", Some("/team/eng/")),
    ]
    .iter()
    .enumerate()
    {
        let policy_name = format!("attachable-{index}");
        let mut params = vec![
            param("PolicyName", &policy_name),
            param("PolicyDocument", POLICY_DOCUMENT),
        ];
        if let Some(path) = path {
            params.push(param("Path", path));
        }
        let (status, created) = call(&router, "CreatePolicy", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePolicy must succeed, got {status}: {created}"
        );
        let policy_arn = element(&created, "Arn");

        let (attach_status, body) = call(
            &router,
            "AttachRolePolicy",
            &[("RoleName", "attachable-role"), ("PolicyArn", &policy_arn)],
        )
        .await;
        assert_eq!(
            attach_status, 200,
            "case {case:?}: the created policy must be attachable, got {attach_status}: {body}"
        );
        attached.push(policy_arn);
    }

    let (list_status, listed) = call(
        &router,
        "ListAttachedRolePolicies",
        &[("RoleName", "attachable-role")],
    )
    .await;
    assert_eq!(
        list_status, 200,
        "ListAttachedRolePolicies must succeed, got {list_status}: {listed}"
    );
    let mut expected = attached.clone();
    expected.sort();
    assert_eq!(
        elements(&listed, "PolicyArn"),
        expected,
        "the role must report exactly the created policies that were attached, got {listed}"
    );

    // An ARN of the same form the account does not hold refers to no entity.
    for (case, arn) in [
        ("another name at the root", expected_policy_arn("/", "never-created")),
        (
            "a held name under another path",
            expected_policy_arn("/elsewhere/", "attachable-0"),
        ),
        (
            "a held name in another case",
            expected_policy_arn("/", "ATTACHABLE-0"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "AttachRolePolicy",
            &[("RoleName", "attachable-role"), ("PolicyArn", &arn)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for the unheld policy {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }
}

/// One already-held managed policy, as the store holds it. Every member is filled
/// the way `CreatePolicy` fills it, so a seeded policy is indistinguishable from a
/// created one.
fn seeded_policy(policy_name: &str) -> state::ManagedPolicy {
    use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{
        Policy, PolicyVersion,
    };

    let created = state::now();
    state::ManagedPolicy {
        policy: Policy {
            policy_name: Some(policy_name.to_string()),
            policy_id: Some(format!("ANPASEEDED{policy_name}").replace('-', "_")),
            arn: Some(state::policy_arn("/", policy_name)),
            path: Some("/".to_string()),
            default_version_id: Some(state::FIRST_POLICY_VERSION_ID.to_string()),
            attachment_count: Some(0),
            permissions_boundary_usage_count: Some(0),
            is_attachable: true,
            description: None,
            create_date: Some(created),
            update_date: Some(created),
            tags: None,
        },
        versions: [(
            state::FIRST_POLICY_VERSION_ID.to_string(),
            PolicyVersion {
                document: Some(POLICY_DOCUMENT.to_string()),
                version_id: Some(state::FIRST_POLICY_VERSION_ID.to_string()),
                is_default_version: true,
                create_date: Some(created),
            },
        )]
        .into_iter()
        .collect(),
    }
}

/// An account holds a bounded number of customer managed policies: at the quota a
/// create is refused with the modelled `LimitExceededException`, and one short of it
/// still succeeds. The store is seeded directly, so the bound is exercised without
/// a thousand wire calls.
#[tokio::test]
async fn the_account_policy_quota_refuses_one_policy_too_many() {
    use service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

    for (case, seeded, expected_status) in [
        ("one short of the quota", state::MAX_POLICIES_PER_ACCOUNT - 1, 200),
        ("at the quota", state::MAX_POLICIES_PER_ACCOUNT, 409),
        ("past the quota", state::MAX_POLICIES_PER_ACCOUNT + 1, 409),
    ] {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        {
            let store = engine.state::<ComAmazonawsIamAwsidentitymanagementv20100508State>();
            let mut store = store.lock();
            for index in 0..seeded {
                let policy_name = format!("seeded-{index}");
                store
                    .policies
                    .insert(policy_name.to_ascii_lowercase(), seeded_policy(&policy_name));
            }
        }
        let router = router_on(engine);

        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", "one-more-policy"),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: with {seeded} policies held, expected HTTP {expected_status}, \
             got {status}: {body}"
        );
        if expected_status == 200 {
            assert_well_formed_policy_id(case, &element(&body, "PolicyId"));
        } else {
            assert_eq!(
                error_code(&body),
                "LimitExceeded",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            let (get_status, got) = call(
                &router,
                "GetPolicy",
                &[("PolicyArn", &expected_policy_arn("/", "one-more-policy"))],
            )
            .await;
            assert_eq!(
                get_status, 404,
                "case {case:?}: the refused create must persist nothing, got {got}"
            );
        }
    }
}
