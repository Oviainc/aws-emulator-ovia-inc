// SPDX-License-Identifier: CC0-1.0

//! `AttachRolePolicy` and its reporting half `ListAttachedRolePolicies`, end to end
//! over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of an
//! attachment that was PERSISTED, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share.
//!
//! The suites are tables: each row differs along what the rule being asserted
//! generalises over (the role's path and name case, the policy's path and name, the
//! number of attachments, the page size, the path prefix, the form of a malformed
//! ARN), so no single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A trust policy with a single statement — the form `CreateRole`'s own modelled
/// example carries. Every role in this suite needs one to exist at all.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"mediaconvert.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// ARNs of AWS-managed policies, spanning the paths the managed-policy ARN grammar
/// admits: the account root, the `/service-role/` path AWS files the policies of a
/// service role under, and the `/job-function/` path. The names and paths are
/// entries of the AWS Managed Policy Reference; what the rule generalises over is
/// the ARN's SHAPE (path depth, name alphabet), which is why the rows differ in
/// path rather than only in name.
const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/AWSElementalMediaConvertFullAccess",
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
    "arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs",
    "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
];

/// How many managed policies one role may carry: AWS's "IAM and STS quotas" page
/// gives "Managed policies attached to an IAM role: 10", which is the quota
/// `crate::state::MAX_MANAGED_POLICIES_PER_ROLE` records.
const MAX_MANAGED_POLICIES_PER_ROLE: usize = 10;

/// Create a role at `path`, which every attachment case needs first.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?} at {path:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// One `AttachRolePolicy` call.
async fn attach(
    router: &MultiServiceRouter,
    role_name: &str,
    policy_arn: &str,
) -> (u16, String) {
    call(
        router,
        "AttachRolePolicy",
        &[("RoleName", role_name), ("PolicyArn", policy_arn)],
    )
    .await
}

/// One `ListAttachedRolePolicies` call, with any paging or filtering members the
/// row exercises.
async fn list(
    router: &MultiServiceRouter,
    role_name: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![("RoleName", role_name)];
    params.extend_from_slice(extra);
    call(router, "ListAttachedRolePolicies", &params).await
}

/// The `(PolicyName, PolicyArn)` pairs a list response reports, in document order.
fn reported(body: &str) -> Vec<(String, String)> {
    let names = elements(body, "PolicyName");
    let arns = elements(body, "PolicyArn");
    assert_eq!(
        names.len(),
        arns.len(),
        "every AttachedPolicy must carry both members: {body}"
    );
    names.into_iter().zip(arns).collect()
}

/// The friendly name a managed policy's ARN ends in, which is what a list reports
/// the attachment under.
fn policy_name_of(policy_arn: &str) -> &str {
    policy_arn
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(policy_arn)
}

/// The attachments `role_name` must report, in the order the service enumerates
/// them: ascending by ARN, which is the key the attachment set is ordered on.
fn expected_attachments(policy_arns: &[&str]) -> Vec<(String, String)> {
    let mut arns: Vec<&str> = policy_arns.to_vec();
    arns.sort_unstable();
    arns.dedup();
    arns.into_iter()
        .map(|arn| (policy_name_of(arn).to_string(), arn.to_string()))
        .collect()
}

/// Every attachment of `role_name`, walked over as many pages as it takes.
async fn walk_all(
    router: &MultiServiceRouter,
    role_name: &str,
    page_size: &str,
) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut extra = vec![("MaxItems", page_size)];
        if let Some(token) = &marker {
            extra.push(("Marker", token.as_str()));
        }
        let (status, body) = list(router, role_name, &extra).await;
        assert_eq!(
            status, 200,
            "case {role_name:?} at page size {page_size:?}: expected 200, got {status}: {body}"
        );
        seen.extend(reported(&body));
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            assert!(
                elements(&body, "Marker").is_empty(),
                "case {role_name:?}: a final page must carry no Marker: {body}"
            );
            return seen;
        }
    }
}

/// One role and the managed policies attached to it.
struct AttachCase {
    role_name: &'static str,
    path: &'static str,
    policies: &'static [&'static str],
}

/// Rows spanning the role path (root, one segment, nested), the role name's case
/// and alphabet, and the NUMBER of attachments (none, one, several) — the
/// dimensions the attachment rule generalises over.
const ATTACH_CASES: &[AttachCase] = &[
    AttachCase {
        role_name: "MediaConvertRole",
        path: "/",
        policies: &[AWS_MANAGED_POLICIES[1]],
    },
    AttachCase {
        role_name: "service_role.v2",
        path: "/eng/",
        policies: &[
            AWS_MANAGED_POLICIES[0],
            AWS_MANAGED_POLICIES[2],
            AWS_MANAGED_POLICIES[4],
        ],
    },
    AttachCase {
        role_name: "nested-role=1",
        path: "/eng/tools/",
        policies: AWS_MANAGED_POLICIES,
    },
    AttachCase {
        role_name: "roleWithNoPolicies",
        path: "/sales/",
        policies: &[],
    },
];

/// RULE: `AttachRolePolicy` records the managed policy its `PolicyArn` names on the
/// role its `RoleName` names, and `ListAttachedRolePolicies` reports exactly the
/// recorded set for that role — each attachment under the path and friendly name
/// its ARN carries — so one role's attachments never show up on another's.
#[tokio::test]
async fn attach_role_policy_persists_each_attachment_and_the_list_reports_it() {
    let router = router();
    for case in ATTACH_CASES {
        create_role(&router, case.role_name, case.path).await;
    }

    for case in ATTACH_CASES {
        for policy_arn in case.policies {
            let label = format!("{} <- {policy_arn}", case.role_name);
            let (status, body) = attach(&router, case.role_name, policy_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status}: {body}"
            );
        }
    }

    for case in ATTACH_CASES {
        let label = case.role_name;
        let (status, body) = list(&router, case.role_name, &[]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status}: {body}"
        );
        let expected = expected_attachments(case.policies);
        assert_eq!(
            reported(&body),
            expected,
            "case {label:?}: expected exactly the attached policies {expected:?}, got {:?}",
            reported(&body)
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {label:?}: a page holding every attachment is not truncated: {body}"
        );
    }
}

/// RULE: a managed policy is either attached to a role or not, so attaching one
/// that is already attached leaves the role's attachment set exactly as it was —
/// and IAM entity names are not distinguished by case, so the casing of the role
/// name a request uses does not pick out a different role.
#[tokio::test]
async fn attaching_the_same_policy_again_leaves_one_attachment() {
    let router = router();
    let role = "IdempotentRole";
    create_role(&router, role, "/eng/").await;

    // The rows differ in the casing the request names the role with, which is what
    // the case-folding rule generalises over.
    let spellings = [role, "idempotentrole", "IDEMPOTENTROLE", "IdEmPoTeNtRoLe"];
    for policy_arn in AWS_MANAGED_POLICIES {
        for spelling in spellings {
            let label = format!("{spelling} <- {policy_arn}");
            let (status, body) = attach(&router, spelling, policy_arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: a repeat attach succeeds, got {status}: {body}"
            );
        }
    }

    let (status, body) = list(&router, role, &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let expected = expected_attachments(AWS_MANAGED_POLICIES);
    assert_eq!(
        reported(&body),
        expected,
        "case {role:?}: expected each policy attached exactly once ({expected:?}), got {:?}",
        reported(&body)
    );
}

/// RULE: an operation naming a role that the account holds no role under
/// "referenced a resource entity that does not exist" — 404 `NoSuchEntity` — and
/// that is true of the reporting half as much as of the attach.
#[tokio::test]
async fn attaching_to_or_listing_an_unheld_role_is_refused() {
    let router = router();
    create_role(&router, "HeldRole", "/").await;

    // Rows spanning a name no role carries, a name that differs from a held one,
    // and the single-character and 64-character bounds of `roleNameType`.
    let longest = "R".repeat(64);
    let unheld = ["MissingRole", "HeldRole2", "h", longest.as_str()];
    for role_name in unheld {
        let (status, body) = attach(&router, role_name, AWS_MANAGED_POLICIES[0]).await;
        assert_eq!(
            status, 404,
            "case {role_name:?}: expected 404 for AttachRolePolicy, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {role_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );

        let (status, body) = list(&router, role_name, &[]).await;
        assert_eq!(
            status, 404,
            "case {role_name:?}: expected 404 for ListAttachedRolePolicies, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {role_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: `PolicyArn` must BE a managed policy's ARN —
/// `arn:<partition>:iam::<account>:policy/<path><name>`, with an empty region
/// segment because IAM is global — and a value that is not one is refused 400,
/// persisting nothing. IAM takes that form in TWO layers, and the row's value says
/// which one refuses it: a value that is not an ARN at all (no `arn:` frame and its
/// six colon-separated fields) is a value whose FORM is wrong for the member and
/// carries the code IAM answers that with (`ValidationError`), while an ARN that IS
/// one and names another service, another resource type or a body IAM's grammar does
/// not admit is the operation's own modelled refusal (`InvalidInput`). Both answer
/// IAM's one sentence for an unusable ARN.
#[tokio::test]
async fn attaching_an_arn_that_is_not_a_managed_policy_arn_is_refused() {
    let router = router();
    let role = "ArnValidationRole";
    create_role(&router, role, "/").await;
    assert_eq!(
        attach(&router, role, AWS_MANAGED_POLICIES[0]).await.0,
        200,
        "the fixture attachment must succeed"
    );

    // Each row is at least 20 characters, `arnType`'s minimum length, so the
    // handler — not the modelled length constraint — is what refuses it. The rows
    // break one part of the grammar each, and span both layers.
    let malformed = [
        ("this-is-not-an-arn-at-all", "ValidationError"),
        ("arn:aws:s3:::a-bucket-named-like-this", "InvalidInput"),
        ("arn:aws:iam::aws:role/NotAPolicyAtAll", "InvalidInput"),
        ("arn:aws:iam::aws:user/SomeUserNamedHere", "InvalidInput"),
        ("arn:aws:iam:us-east-1:aws:policy/RegionIsSet", "InvalidInput"),
        ("arn::iam::aws:policy/PartitionIsEmpty", "InvalidInput"),
        ("arn:aws:iam:::policy/AccountIsEmptyHere", "InvalidInput"),
        ("arn:aws:iam::aws:policy/service-role/", "InvalidInput"),
        ("arn:aws:iam::aws:policy//EmptySegment", "InvalidInput"),
        ("aws:iam::aws:policy/MissingArnPrefixHere", "ValidationError"),
    ];
    for (policy_arn, expected_code) in malformed {
        assert!(
            policy_arn.len() >= 20,
            "case {policy_arn:?}: a row must satisfy arnType's 20-character minimum"
        );
        let (status, body) = attach(&router, role, policy_arn).await;
        assert_eq!(
            status, 400,
            "case {policy_arn:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {policy_arn:?}: expected {expected_code}, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("ARN {policy_arn} is not valid."),
            "case {policy_arn:?}: expected IAM's unusable-ARN sentence, got {:?}",
            element(&body, "Message")
        );
    }

    // A refused attach writes nothing: the role still carries only the fixture.
    let (_, body) = list(&router, role, &[]).await;
    let expected = expected_attachments(&[AWS_MANAGED_POLICIES[0]]);
    assert_eq!(
        reported(&body),
        expected,
        "a refused attach must leave the attachment set at {expected:?}, got {:?}",
        reported(&body)
    );
}

/// RULE: a customer-managed policy exists only where the account holds it, so an
/// account-owned policy ARN naming no held policy refers to no entity — 404
/// `NoSuchEntity`.
#[tokio::test]
async fn attaching_a_customer_managed_policy_the_account_does_not_hold_is_refused() {
    let router = router();
    let role = "CustomerPolicyRole";
    create_role(&router, role, "/").await;

    // Rows spanning the policy's path depth and the owning account (this account
    // and another one), since neither holds a policy here.
    let unheld = [
        format!("arn:aws:iam::{}:policy/NeverCreated", account_id()),
        format!("arn:aws:iam::{}:policy/team/NeverCreated", account_id()),
        format!("arn:aws:iam::{}:policy/a/b/DeeplyPathed", account_id()),
        "arn:aws:iam::210987654321:policy/OtherAccountPolicy".to_string(),
    ];
    for policy_arn in &unheld {
        let (status, body) = attach(&router, role, policy_arn).await;
        assert_eq!(
            status, 404,
            "case {policy_arn:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {policy_arn:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }

    let (_, body) = list(&router, role, &[]).await;
    assert!(
        reported(&body).is_empty(),
        "a refused attach must persist nothing, got {:?}",
        reported(&body)
    );
}

/// RULE: a policy AWS files under the reserved service-linked role path is a
/// service role policy, which "can only be attached to the service-linked role for
/// that service" — 400 `PolicyNotAttachable` for any other role.
#[tokio::test]
async fn attaching_a_service_role_policy_to_an_ordinary_role_is_refused() {
    let router = router();
    let role = "OrdinaryRole";
    create_role(&router, role, "/").await;

    // Rows spanning the service the policy belongs to and the depth below the
    // reserved path.
    let service_role_policies = [
        "arn:aws:iam::aws:policy/aws-service-role/AWSLambdaRole",
        "arn:aws:iam::aws:policy/aws-service-role/AmazonEKSServiceRolePolicy",
        "arn:aws:iam::aws:policy/aws-service-role/elasticbeanstalk/ServicePolicy",
    ];
    for policy_arn in service_role_policies {
        let (status, body) = attach(&router, role, policy_arn).await;
        assert_eq!(
            status, 400,
            "case {policy_arn:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "PolicyNotAttachable",
            "case {policy_arn:?}: expected PolicyNotAttachable, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: a service-linked role is a protected resource only the service it is
/// linked to may modify, and attaching a managed policy modifies the role's
/// permission policy — so the attach is refused with 400 `UnmodifiableEntity`.
#[tokio::test]
async fn attaching_to_a_service_linked_role_is_refused() {
    let router = router();

    // Rows spanning the service the role is linked to.
    let principals = [
        "mediaconvert.amazonaws.com",
        "elasticbeanstalk.amazonaws.com",
        "autoscaling.amazonaws.com",
    ];
    for principal in principals {
        let (status, created) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: CreateServiceLinkedRole must succeed, got {status}: {created}"
        );
        let role_name = element(&created, "RoleName");
        // What the role carries before the refused attach: the policy its own service
        // attached when the role was created, which only that service controls.
        let (status, body) = list(&router, &role_name, &[]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: expected 200, got {status}: {body}"
        );
        let before = reported(&body);

        let (status, body) = attach(&router, &role_name, AWS_MANAGED_POLICIES[0]).await;
        assert_eq!(
            status, 400,
            "case {principal:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "UnmodifiableEntity",
            "case {principal:?}: expected UnmodifiableEntity, got {:?}",
            error_code(&body)
        );

        // The refusal left the role's attachment set exactly as it was, which the
        // reporting half of this pair is what shows.
        let (status, body) = list(&router, &role_name, &[]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            before,
            "case {principal:?}: a refused attach persists nothing, got {:?}",
            reported(&body)
        );
        assert!(
            !reported(&body)
                .iter()
                .any(|(_, arn)| arn == AWS_MANAGED_POLICIES[0]),
            "case {principal:?}: a refused attach persists nothing, got {:?}",
            reported(&body)
        );
    }
}

/// RULE: a role carries a bounded number of managed policies, so an attach that
/// would carry it past the quota is refused with 409 `LimitExceeded` while a repeat
/// attach of a policy already attached stays a no-op — it consumes no slot.
#[tokio::test]
async fn attaching_past_the_role_quota_is_refused() {
    let router = router();
    let role = "QuotaRole";
    create_role(&router, role, "/").await;

    // Distinct well-formed AWS-managed policy ARNs, one per slot the quota allows,
    // plus one more to take the role past it.
    let filler: Vec<String> = (1..=MAX_MANAGED_POLICIES_PER_ROLE + 1)
        .map(|index| format!("arn:aws:iam::aws:policy/QuotaFiller{index:02}"))
        .collect();

    for policy_arn in filler.iter().take(MAX_MANAGED_POLICIES_PER_ROLE) {
        let (status, body) = attach(&router, role, policy_arn).await;
        assert_eq!(
            status, 200,
            "case {policy_arn:?}: an attach inside the quota succeeds, got {status}: {body}"
        );
    }

    // Already attached: still a success, and still one attachment.
    let (status, body) = attach(&router, role, &filler[0]).await;
    assert_eq!(
        status, 200,
        "case {:?}: a repeat attach at the quota succeeds, got {status}: {body}",
        filler[0]
    );

    let past = &filler[MAX_MANAGED_POLICIES_PER_ROLE];
    let (status, body) = attach(&router, role, past).await;
    assert_eq!(
        status, 409,
        "case {past:?}: expected 409 past the quota, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "case {past:?}: expected LimitExceeded, got {:?}",
        error_code(&body)
    );

    let listed = walk_all(&router, role, "1000").await;
    let expected = expected_attachments(
        &filler
            .iter()
            .take(MAX_MANAGED_POLICIES_PER_ROLE)
            .map(String::as_str)
            .collect::<Vec<&str>>(),
    );
    assert_eq!(
        listed, expected,
        "expected the role to carry exactly the {MAX_MANAGED_POLICIES_PER_ROLE} \
         attachments the quota allows, got {listed:?}"
    );
}

/// RULE: `ListAttachedRolePolicies` honours `MaxItems` as a real page size and
/// hands out a `Marker` a follow-up resumes strictly after, so walking the pages
/// yields every attachment exactly once whatever the page size — and a read never
/// changes what a later caller sees.
#[tokio::test]
async fn list_attached_role_policies_pages_over_the_attachment_set() {
    let router = router();
    let role = "PagedRole";
    create_role(&router, role, "/").await;
    for policy_arn in AWS_MANAGED_POLICIES {
        assert_eq!(
            attach(&router, role, policy_arn).await.0,
            200,
            "case {policy_arn:?}: the fixture attach must succeed"
        );
    }
    let expected = expected_attachments(AWS_MANAGED_POLICIES);

    // Rows spanning page sizes below, at and above the attachment count, plus
    // `maxItemsType`'s published bounds (1..1000).
    for page_size in ["1", "2", "4", "5", "6", "100", "1000"] {
        let walked = walk_all(&router, role, page_size).await;
        assert_eq!(
            walked, expected,
            "case {page_size:?}: expected every attachment exactly once ({expected:?}), \
             got {walked:?}"
        );
    }

    // `maxItemsType` is range 1..1000: a value past a published bound is refused
    // before the handler runs, and one at a bound is not.
    for out_of_range in ["0", "1001"] {
        let (status, body) = list(&router, role, &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400 outside maxItemsType's range, got {status}: {body}"
        );
    }

    // A read leaves the set alone: the same request answers the same thing.
    let (_, first) = list(&router, role, &[]).await;
    let (_, again) = list(&router, role, &[]).await;
    assert_eq!(
        reported(&first),
        reported(&again),
        "a list must not change what a later list reports"
    );
}

/// RULE: `PathPrefix` filters the reported attachments to the policies whose path
/// begins with it, and defaults to `/` — listing all of them.
#[tokio::test]
async fn list_attached_role_policies_filters_by_path_prefix() {
    let router = router();
    let role = "FilteredRole";
    create_role(&router, role, "/").await;
    for policy_arn in AWS_MANAGED_POLICIES {
        assert_eq!(
            attach(&router, role, policy_arn).await.0,
            200,
            "case {policy_arn:?}: the fixture attach must succeed"
        );
    }

    // Rows spanning the prefix that matches everything, the two paths the fixture
    // set carries, and one no attached policy lives under.
    let prefixes = ["/", "/service-role/", "/job-function/", "/nope/"];
    for prefix in prefixes {
        let (status, body) = list(&router, role, &[("PathPrefix", prefix)]).await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200, got {status}: {body}"
        );
        let matching: Vec<&str> = AWS_MANAGED_POLICIES
            .iter()
            .copied()
            .filter(|arn| policy_path_of(arn).starts_with(prefix))
            .collect();
        let expected = expected_attachments(&matching);
        assert_eq!(
            reported(&body),
            expected,
            "case {prefix:?}: expected the policies under that path ({expected:?}), got {:?}",
            reported(&body)
        );
    }

    // The default is the root prefix, so an omitted `PathPrefix` lists them all.
    let (_, body) = list(&router, role, &[]).await;
    assert_eq!(
        reported(&body),
        expected_attachments(AWS_MANAGED_POLICIES),
        "an omitted PathPrefix lists every attachment, got {:?}",
        reported(&body)
    );
}

/// The path of a managed policy's ARN: everything between the `policy` resource
/// type and the friendly name, beginning and ending with `/`.
fn policy_path_of(policy_arn: &str) -> String {
    let resource = policy_arn
        .split_once(":policy/")
        .map(|(_, resource)| resource)
        .expect("a managed policy ARN carries a policy resource");
    match resource.rsplit_once('/') {
        Some((path_body, _)) => format!("/{path_body}/"),
        None => "/".to_string(),
    }
}

/// RULE: a `Marker` names a position in ONE role's attachment order, so a token
/// this service did not issue for that role — a foreign string, or one it issued
/// while paging another role — is refused with 400 `InvalidInput` rather than read
/// as "start from the beginning".
#[tokio::test]
async fn list_attached_role_policies_refuses_a_marker_it_did_not_issue() {
    let router = router();
    for role in ["MarkerRoleA", "MarkerRoleB"] {
        create_role(&router, role, "/").await;
        for policy_arn in AWS_MANAGED_POLICIES {
            assert_eq!(
                attach(&router, role, policy_arn).await.0,
                200,
                "case {role:?} <- {policy_arn:?}: the fixture attach must succeed"
            );
        }
    }

    // The marker role A hands out while paging its own attachments.
    let (_, first_page) = list(&router, "MarkerRoleA", &[("MaxItems", "1")]).await;
    assert_eq!(
        element(&first_page, "IsTruncated"),
        "true",
        "a one-item page of several attachments is truncated: {first_page}"
    );
    let marker = element(&first_page, "Marker");

    // It resumes role A strictly after the first attachment …
    let (status, resumed) = list(
        &router,
        "MarkerRoleA",
        &[("Marker", marker.as_str()), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {resumed}");
    let expected: Vec<(String, String)> = expected_attachments(AWS_MANAGED_POLICIES)
        .into_iter()
        .skip(1)
        .collect();
    assert_eq!(
        reported(&resumed),
        expected,
        "a marker resumes strictly after the position it names ({expected:?}), got {:?}",
        reported(&resumed)
    );

    // … and names no position in role B's order, nor does any token the service
    // never issued.
    let foreign = [
        marker.as_str(),
        "not-a-marker",
        "aWFtOnVzZXJzOmFsaWNl",
        "////",
    ];
    for token in foreign {
        let (status, body) = list(&router, "MarkerRoleB", &[("Marker", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {token:?}: expected InvalidInput, got {:?}",
            error_code(&body)
        );
    }
}
