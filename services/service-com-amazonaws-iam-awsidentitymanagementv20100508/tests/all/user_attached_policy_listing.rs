// SPDX-License-Identifier: CC0-1.0

//! `ListAttachedUserPolicies` — the reporting half of `AttachUserPolicy` — end to
//! end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of
//! attachments that were PERSISTED, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share.
//!
//! The suites are TABLES: each row differs along what the rule being asserted
//! generalises over (the user's path and name case, the policy's owner, path and
//! name, the number of attachments, the page size, the path prefix, the form of a
//! foreign marker), so no single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries. Every customer-managed fixture policy needs one.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement — the form `CreateRole`'s own modelled
/// example carries — needed by the one role this suite creates to show that a
/// role's attachments are not a user's.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"mediaconvert.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The customer-managed policies the fixture account holds: `(PolicyName, Path)`.
/// The paths span the account root, a one-segment path and a nested path, so a rule
/// about an attachment's reported path is exercised at more than one depth.
const CUSTOMER_POLICIES: &[(&str, &str)] = &[
    ("ReadItems", "/"),
    ("WriteItems", "/app/"),
    ("AuditItems", "/app/audit/"),
];

/// ARNs of AWS-managed policies — owned by AWS rather than by the account, so the
/// account holds no record of them. The rows span the paths the managed-policy ARN
/// grammar admits: the account root, the `/service-role/` path and the
/// `/job-function/` path. The names and paths are entries of the AWS Managed Policy
/// Reference; what the rule generalises over is the ARN's SHAPE (path depth, name
/// alphabet), which is why the rows differ in path rather than only in name.
const AWS_MANAGED_POLICIES: &[&str] = &[
    "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
    "arn:aws:iam::aws:policy/job-function/ViewOnlyAccess",
];

/// How many fixture policies there are: the account's own, then AWS's.
fn policy_count() -> usize {
    CUSTOMER_POLICIES.len() + AWS_MANAGED_POLICIES.len()
}

/// The ARN of the fixture policy at `index`: a customer-managed policy's ARN is
/// `arn:aws:iam::<account>:policy<path><name>`, and AWS's own are listed verbatim.
fn policy_arn(index: usize) -> String {
    match CUSTOMER_POLICIES.get(index) {
        Some((name, path)) => format!("arn:aws:iam::{}:policy{path}{name}", account_id()),
        None => AWS_MANAGED_POLICIES[index - CUSTOMER_POLICIES.len()].to_string(),
    }
}

/// Create a user at `path`, which every attachment case needs first.
async fn create_user(router: &MultiServiceRouter, user_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user_name:?} at {path:?}: CreateUser must succeed, got {status}: {body}"
    );
}

/// Create a customer-managed policy carrying [`POLICY_DOCUMENT`].
async fn create_policy(router: &MultiServiceRouter, policy_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("Path", path),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: CreatePolicy must succeed, got {status}: {body}"
    );
}

/// Create every fixture policy the account owns, so each one is attachable.
async fn create_customer_policies(router: &MultiServiceRouter) {
    for (name, path) in CUSTOMER_POLICIES {
        create_policy(router, name, path).await;
    }
}

/// One `AttachUserPolicy` call.
async fn attach(router: &MultiServiceRouter, user_name: &str, arn: &str) -> (u16, String) {
    call(
        router,
        "AttachUserPolicy",
        &[("UserName", user_name), ("PolicyArn", arn)],
    )
    .await
}

/// One `AttachUserPolicy` call that must succeed.
async fn attach_ok(router: &MultiServiceRouter, user_name: &str, arn: &str) {
    let (status, body) = attach(router, user_name, arn).await;
    assert_eq!(
        status, 200,
        "case {user_name:?} <- {arn:?}: the fixture attach must succeed, got {status}: {body}"
    );
}

/// One `ListAttachedUserPolicies` call, with any paging or filtering members the row
/// exercises.
async fn list(
    router: &MultiServiceRouter,
    user_name: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![("UserName", user_name)];
    params.extend_from_slice(extra);
    call(router, "ListAttachedUserPolicies", &params).await
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
fn policy_name_of(arn: &str) -> &str {
    arn.rsplit_once('/').map(|(_, name)| name).unwrap_or(arn)
}

/// The path of a managed policy's ARN: everything between the `policy` resource type
/// and the friendly name, beginning and ending with `/`.
fn policy_path_of(arn: &str) -> String {
    let resource = arn
        .split_once(":policy/")
        .map(|(_, resource)| resource)
        .expect("a managed policy ARN carries a policy resource");
    match resource.rsplit_once('/') {
        Some((path_body, _)) => format!("/{path_body}/"),
        None => "/".to_string(),
    }
}

/// The attachments a user carrying `arns` must report, in the order the service
/// enumerates them: ascending by ARN, which is the key the attachment set is ordered
/// on.
fn expected_attachments(arns: &[String]) -> Vec<(String, String)> {
    let mut sorted: Vec<String> = arns.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .into_iter()
        .map(|arn| (policy_name_of(&arn).to_string(), arn))
        .collect()
}

/// Every attachment of `user_name`, walked over as many pages as it takes.
async fn walk_all(
    router: &MultiServiceRouter,
    user_name: &str,
    page_size: &str,
) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut extra = vec![("MaxItems", page_size)];
        if let Some(token) = &marker {
            extra.push(("Marker", token.as_str()));
        }
        let (status, body) = list(router, user_name, &extra).await;
        assert_eq!(
            status, 200,
            "case {user_name:?} at page size {page_size:?}: expected 200, got {status}: {body}"
        );
        seen.extend(reported(&body));
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            assert!(
                elements(&body, "Marker").is_empty(),
                "case {user_name:?}: a final page must carry no Marker: {body}"
            );
            return seen;
        }
    }
}

/// One fixture user and the fixture policies attached to it, by [`policy_arn`] index.
struct ListCase {
    user_name: &'static str,
    path: &'static str,
    policies: &'static [usize],
}

/// Rows spanning the user path (root, one segment, nested), the user name's case and
/// alphabet, the NUMBER of attachments (none, one, several, all) and the OWNER of the
/// attached policy (this account, AWS) — the dimensions the listing rule generalises
/// over.
const LIST_CASES: &[ListCase] = &[
    ListCase {
        user_name: "alice",
        path: "/",
        policies: &[0],
    },
    ListCase {
        user_name: "Bob.9",
        path: "/app/",
        policies: &[0, 3, 5],
    },
    ListCase {
        user_name: "carol_7",
        path: "/app/audit/",
        policies: &[0, 1, 2, 3, 4, 5],
    },
    ListCase {
        user_name: "dave+x=1",
        path: "/sales/west/",
        policies: &[],
    },
    ListCase {
        user_name: "EVE@4",
        path: "/",
        policies: &[2, 4],
    },
];

/// The ARNs the case's indices name.
fn case_arns(case: &ListCase) -> Vec<String> {
    case.policies.iter().copied().map(policy_arn).collect()
}

/// Create every fixture user of [`LIST_CASES`] and attach its policies.
async fn seed_cases(router: &MultiServiceRouter) {
    create_customer_policies(router).await;
    for case in LIST_CASES {
        create_user(router, case.user_name, case.path).await;
        for arn in case_arns(case) {
            attach_ok(router, case.user_name, &arn).await;
        }
    }
}

/// RULE: `ListAttachedUserPolicies` reports exactly the managed policies recorded as
/// attached to the user its `UserName` names — each under the friendly name its ARN
/// ends in, ascending by ARN — so one principal's attachments never show up on
/// another's, and a user carrying none lists an empty set.
#[tokio::test]
async fn list_attached_user_policies_reports_exactly_that_users_attachments() {
    let router = router();
    seed_cases(&router).await;

    for case in LIST_CASES {
        let label = case.user_name;
        let (status, body) = list(&router, case.user_name, &[]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status}: {body}"
        );
        let expected = expected_attachments(&case_arns(case));
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

    // Every fixture policy exists, so the report is a statement about the
    // ATTACHMENTS rather than about which policies the account holds.
    assert_eq!(
        policy_count(),
        CUSTOMER_POLICIES.len() + AWS_MANAGED_POLICIES.len(),
        "the fixture tables must cover both policy owners"
    );
}

/// RULE: IAM entity names "are not distinguished by case", so the casing the request
/// spells the user with picks out the same user and reports the same attachments.
#[tokio::test]
async fn list_attached_user_policies_is_case_insensitive_in_the_user_name() {
    let router = router();
    create_customer_policies(&router).await;
    let user = "CaseFoldedUser";
    create_user(&router, user, "/eng/").await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for arn in &arns {
        attach_ok(&router, user, arn).await;
    }
    let expected = expected_attachments(&arns);

    // The rows differ in the casing the request names the user with, which is what
    // the case-folding rule generalises over.
    for spelling in [user, "casefoldeduser", "CASEFOLDEDUSER", "cAsEfOlDeDuSeR"] {
        let (status, body) = list(&router, spelling, &[]).await;
        assert_eq!(
            status, 200,
            "case {spelling:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            expected,
            "case {spelling:?}: expected {expected:?}, got {:?}",
            reported(&body)
        );
    }
}

/// RULE: a request naming a user the account holds no user under "referenced a
/// resource entity that does not exist" — 404 `NoSuchEntity`.
#[tokio::test]
async fn listing_the_attachments_of_an_unheld_user_is_refused() {
    let router = router();
    create_user(&router, "HeldUser", "/").await;

    // Rows spanning a name no user carries, a name that differs from a held one, and
    // the single-character and 64-character bounds of `userNameType`.
    let longest = "U".repeat(64);
    for user_name in ["MissingUser", "HeldUser2", "h", longest.as_str()] {
        let (status, body) = list(&router, user_name, &[]).await;
        assert_eq!(
            status, 404,
            "case {user_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {user_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: `ListAttachedUserPolicies` honours `MaxItems` as a real page size and hands
/// out a `Marker` a follow-up resumes strictly after, so walking the pages yields
/// every attachment exactly once whatever the page size — and a read never changes
/// what a later caller sees.
#[tokio::test]
async fn list_attached_user_policies_pages_over_the_attachment_set() {
    let router = router();
    create_customer_policies(&router).await;
    let user = "PagedUser";
    create_user(&router, user, "/").await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for arn in &arns {
        attach_ok(&router, user, arn).await;
    }
    let expected = expected_attachments(&arns);

    // Rows spanning page sizes below, at and above the attachment count, plus
    // `maxItemsType`'s published bounds (1..1000).
    for page_size in ["1", "2", "5", "6", "7", "100", "1000"] {
        let walked = walk_all(&router, user, page_size).await;
        assert_eq!(
            walked, expected,
            "case {page_size:?}: expected every attachment exactly once ({expected:?}), \
             got {walked:?}"
        );
    }

    // `maxItemsType` is range 1..1000: a value past a published bound is refused
    // before the handler runs, and one at a bound is not.
    for out_of_range in ["0", "1001"] {
        let (status, body) = list(&router, user, &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400 outside maxItemsType's range, got {status}: {body}"
        );
    }

    // A read leaves the set alone: the same request answers the same thing.
    let (_, first) = list(&router, user, &[]).await;
    let (_, again) = list(&router, user, &[]).await;
    assert_eq!(
        reported(&first),
        reported(&again),
        "a list must not change what a later list reports"
    );
}

/// RULE: `PathPrefix` filters the reported attachments to the policies whose path
/// begins with it, and defaults to `/` — listing all of them.
#[tokio::test]
async fn list_attached_user_policies_filters_by_path_prefix() {
    let router = router();
    create_customer_policies(&router).await;
    let user = "FilteredUser";
    create_user(&router, user, "/").await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for arn in &arns {
        attach_ok(&router, user, arn).await;
    }

    // Rows spanning the prefix that matches everything, each path the fixture set
    // carries, a prefix that selects a nested path only, and one no attached policy
    // lives under.
    let prefixes = [
        "/",
        "/app/",
        "/app/audit/",
        "/service-role/",
        "/job-function/",
        "/nope/",
    ];
    for prefix in prefixes {
        let (status, body) = list(&router, user, &[("PathPrefix", prefix)]).await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200, got {status}: {body}"
        );
        let matching: Vec<String> = arns
            .iter()
            .filter(|arn| policy_path_of(arn).starts_with(prefix))
            .cloned()
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
    let (_, body) = list(&router, user, &[]).await;
    assert_eq!(
        reported(&body),
        expected_attachments(&arns),
        "an omitted PathPrefix lists every attachment, got {:?}",
        reported(&body)
    );
}

/// RULE: a `Marker` names a position in ONE user's attachment order, so a token this
/// service did not issue for that user — a foreign string, one it issued while
/// paging another user, or one it issued while paging a ROLE's attachments — is
/// refused with 400 `InvalidInput` rather than read as "start from the beginning".
#[tokio::test]
async fn list_attached_user_policies_refuses_a_marker_it_did_not_issue() {
    let router = router();
    create_customer_policies(&router).await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for user in ["MarkerUserA", "MarkerUserB"] {
        create_user(&router, user, "/").await;
        for arn in &arns {
            attach_ok(&router, user, arn).await;
        }
    }

    // A role carrying the same attachments, so the token it hands out names the same
    // POSITION in a different principal's order.
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "MarkerRole"),
            ("Path", "/"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateRole must succeed, got {status}: {body}");
    for arn in &arns {
        let (status, body) = call(
            &router,
            "AttachRolePolicy",
            &[("RoleName", "MarkerRole"), ("PolicyArn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: the fixture role attach must succeed, got {status}: {body}"
        );
    }
    let (_, role_page) = call(
        &router,
        "ListAttachedRolePolicies",
        &[("RoleName", "MarkerRole"), ("MaxItems", "1")],
    )
    .await;
    let role_marker = element(&role_page, "Marker");

    // The marker user A hands out while paging its own attachments.
    let (_, first_page) = list(&router, "MarkerUserA", &[("MaxItems", "1")]).await;
    assert_eq!(
        element(&first_page, "IsTruncated"),
        "true",
        "a one-item page of several attachments is truncated: {first_page}"
    );
    let marker = element(&first_page, "Marker");

    // It resumes user A strictly after the first attachment …
    let (status, resumed) = list(
        &router,
        "MarkerUserA",
        &[("Marker", marker.as_str()), ("MaxItems", "1000")],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {resumed}");
    let expected: Vec<(String, String)> = expected_attachments(&arns).into_iter().skip(1).collect();
    assert_eq!(
        reported(&resumed),
        expected,
        "a marker resumes strictly after the position it names ({expected:?}), got {:?}",
        reported(&resumed)
    );

    // … and names no position in user B's order, nor does the role's token, nor any
    // string the service never issued.
    let foreign = [
        marker.as_str(),
        role_marker.as_str(),
        "not-a-marker",
        "aWFtOnVzZXJzOmFsaWNl",
        "////",
    ];
    for token in foreign {
        let (status, body) = list(&router, "MarkerUserB", &[("Marker", token)]).await;
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

/// RULE: a user's attachment set is the user's own, so attaching a policy to a role
/// or to another user leaves it alone — and the report of one principal is never the
/// report of another.
#[tokio::test]
async fn one_principals_attachments_are_not_anothers() {
    let router = router();
    create_customer_policies(&router).await;
    create_user(&router, "OwnerUser", "/").await;
    create_user(&router, "BystanderUser", "/").await;
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "BystanderRole"),
            ("Path", "/"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateRole must succeed, got {status}: {body}");

    let owned = policy_arn(0);
    attach_ok(&router, "OwnerUser", &owned).await;
    for index in 1..policy_count() {
        let arn = policy_arn(index);
        let (status, body) = call(
            &router,
            "AttachRolePolicy",
            &[("RoleName", "BystanderRole"), ("PolicyArn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: the fixture role attach must succeed, got {status}: {body}"
        );
    }

    let (_, body) = list(&router, "OwnerUser", &[]).await;
    let expected = expected_attachments(&[owned]);
    assert_eq!(
        reported(&body),
        expected,
        "case \"OwnerUser\": expected only its own attachment {expected:?}, got {:?}",
        reported(&body)
    );

    let (status, body) = list(&router, "BystanderUser", &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert!(
        reported(&body).is_empty(),
        "case \"BystanderUser\": a user with no attachment lists an empty set, got {:?}",
        reported(&body)
    );
}
