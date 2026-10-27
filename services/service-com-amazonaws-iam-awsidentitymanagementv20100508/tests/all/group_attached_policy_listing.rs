// SPDX-License-Identifier: CC0-1.0

//! `ListAttachedGroupPolicies` — the reporting half of `AttachGroupPolicy` — end to
//! end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of
//! attachments that were PERSISTED, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share; what `AttachGroupPolicy` itself refuses is asserted by
//! `tests/all/group_attached_policies.rs`.
//!
//! The suites are TABLES: each row differs along what the rule being asserted
//! generalises over (the group's path and name case, the policy's owner, path and
//! name, the number of attachments, the page size, the path prefix, the form of a
//! foreign marker), so no single recorded request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries. Every customer-managed fixture policy needs one.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

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

/// Create a group at `path`, answering the ARN the account holds it under, which the
/// readers addressed by principal ARN need.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?} at {path:?}: CreateGroup must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a customer-managed policy carrying `document`, answering its ARN.
async fn create_policy_with(
    router: &MultiServiceRouter,
    policy_name: &str,
    path: &str,
    document: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("Path", path),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?} at {path:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create every fixture policy the account owns, so each one is attachable.
async fn create_customer_policies(router: &MultiServiceRouter) {
    for (name, path) in CUSTOMER_POLICIES {
        create_policy_with(router, name, path, POLICY_DOCUMENT).await;
    }
}

/// One `AttachGroupPolicy` call that must succeed, which is how every row of this
/// suite gets the state it reports on.
async fn attach_ok(router: &MultiServiceRouter, group_name: &str, policy_arn: &str) {
    let (status, body) = call(
        router,
        "AttachGroupPolicy",
        &[("GroupName", group_name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?} <- {policy_arn:?}: the fixture attach must succeed, got \
         {status}: {body}"
    );
}

/// One `ListAttachedGroupPolicies` call, with any paging or filtering members the row
/// exercises.
async fn list(
    router: &MultiServiceRouter,
    group_name: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![("GroupName", group_name)];
    params.extend_from_slice(extra);
    call(router, "ListAttachedGroupPolicies", &params).await
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

/// The attachments a group carrying `arns` must report, in the order the service
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

/// Every attachment of `group_name`, walked over as many pages as it takes.
async fn walk_all(
    router: &MultiServiceRouter,
    group_name: &str,
    page_size: &str,
) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut extra = vec![("MaxItems", page_size)];
        if let Some(token) = &marker {
            extra.push(("Marker", token.as_str()));
        }
        let (status, body) = list(router, group_name, &extra).await;
        assert_eq!(
            status, 200,
            "case {group_name:?} at page size {page_size:?}: expected 200, got {status}: {body}"
        );
        seen.extend(reported(&body));
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            assert!(
                elements(&body, "Marker").is_empty(),
                "case {group_name:?}: a final page must carry no Marker: {body}"
            );
            return seen;
        }
    }
}

/// One fixture group and the fixture policies attached to it, by [`policy_arn`] index.
struct ListCase {
    group_name: &'static str,
    path: &'static str,
    policies: &'static [usize],
}

/// Rows spanning the group path (root, one segment, nested), the group name's case and
/// alphabet, the NUMBER of attachments (none, one, several, all) and the OWNER of the
/// attached policy (this account, AWS) — the dimensions the listing rule generalises
/// over.
const LIST_CASES: &[ListCase] = &[
    ListCase {
        group_name: "readers",
        path: "/",
        policies: &[0],
    },
    ListCase {
        group_name: "Writers.9",
        path: "/app/",
        policies: &[0, 3, 5],
    },
    ListCase {
        group_name: "auditors_7",
        path: "/app/audit/",
        policies: &[0, 1, 2, 3, 4, 5],
    },
    ListCase {
        group_name: "idle+x=1",
        path: "/sales/west/",
        policies: &[],
    },
    ListCase {
        group_name: "OPS@4",
        path: "/",
        policies: &[2, 4],
    },
];

/// The ARNs the case's indices name.
fn case_arns(case: &ListCase) -> Vec<String> {
    case.policies.iter().copied().map(policy_arn).collect()
}

/// Create every fixture group of [`LIST_CASES`] and attach its policies.
async fn seed_cases(router: &MultiServiceRouter) {
    create_customer_policies(router).await;
    for case in LIST_CASES {
        create_group(router, case.group_name, case.path).await;
        for arn in case_arns(case) {
            attach_ok(router, case.group_name, &arn).await;
        }
    }
}

/// RULE: `ListAttachedGroupPolicies` reports exactly the managed policies recorded as
/// attached to the group its `GroupName` names — each under the friendly name its ARN
/// ends in, ascending by ARN — so one group's attachments never show up on another's,
/// and a group carrying none lists an empty set.
#[tokio::test]
async fn list_attached_group_policies_reports_exactly_that_groups_attachments() {
    let router = router();
    seed_cases(&router).await;

    for case in LIST_CASES {
        let label = case.group_name;
        let (status, body) = list(&router, case.group_name, &[]).await;
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
/// spells the group with picks out the same group and reports the same attachments.
#[tokio::test]
async fn list_attached_group_policies_is_case_insensitive_in_the_group_name() {
    let router = router();
    create_customer_policies(&router).await;
    let group = "CaseFoldedGroup";
    create_group(&router, group, "/eng/").await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for arn in &arns {
        attach_ok(&router, group, arn).await;
    }
    let expected = expected_attachments(&arns);

    // The rows differ in the casing the request names the group with, which is what
    // the case-folding rule generalises over.
    for spelling in [group, "casefoldedgroup", "CASEFOLDEDGROUP", "cAsEfOlDeDgRoUp"] {
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

/// RULE: a request naming a group the account holds no group under "referenced a
/// resource entity that does not exist" — 404 `NoSuchEntity`.
#[tokio::test]
async fn listing_the_attachments_of_an_unheld_group_is_refused() {
    let router = router();
    create_group(&router, "HeldGroup", "/").await;

    // Rows spanning a name no group carries, a name that differs from a held one, and
    // the single-character and 128-character bounds of `groupNameType` (len 1..128).
    let longest = "G".repeat(128);
    for group_name in ["MissingGroup", "HeldGroup2", "g", longest.as_str()] {
        let (status, body) = list(&router, group_name, &[]).await;
        assert_eq!(
            status, 404,
            "case {group_name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {group_name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: `ListAttachedGroupPolicies` honours `MaxItems` as a real page size and hands
/// out a `Marker` a follow-up resumes strictly after, so walking the pages yields
/// every attachment exactly once whatever the page size — and a read never changes
/// what a later caller sees.
#[tokio::test]
async fn list_attached_group_policies_pages_over_the_attachment_set() {
    let router = router();
    create_customer_policies(&router).await;
    let group = "PagedGroup";
    create_group(&router, group, "/").await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for arn in &arns {
        attach_ok(&router, group, arn).await;
    }
    let expected = expected_attachments(&arns);

    // Rows spanning page sizes below, at and above the attachment count, plus
    // `maxItemsType`'s published bounds (1..1000).
    for page_size in ["1", "2", "5", "6", "7", "100", "1000"] {
        let walked = walk_all(&router, group, page_size).await;
        assert_eq!(
            walked, expected,
            "case {page_size:?}: expected every attachment exactly once ({expected:?}), \
             got {walked:?}"
        );
    }

    // `maxItemsType` is range 1..1000: a value past a published bound is refused
    // before the handler runs, and one at a bound is not.
    for out_of_range in ["0", "1001"] {
        let (status, body) = list(&router, group, &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400 outside maxItemsType's range, got {status}: {body}"
        );
    }

    // A read leaves the set alone: the same request answers the same thing.
    let (_, first) = list(&router, group, &[]).await;
    let (_, again) = list(&router, group, &[]).await;
    assert_eq!(
        reported(&first),
        reported(&again),
        "a list must not change what a later list reports"
    );
}

/// RULE: `PathPrefix` filters the reported attachments to the policies whose path
/// begins with it, and defaults to `/` — listing all of them.
#[tokio::test]
async fn list_attached_group_policies_filters_by_path_prefix() {
    let router = router();
    create_customer_policies(&router).await;
    let group = "FilteredGroup";
    create_group(&router, group, "/").await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for arn in &arns {
        attach_ok(&router, group, arn).await;
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
        let (status, body) = list(&router, group, &[("PathPrefix", prefix)]).await;
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
    let (_, body) = list(&router, group, &[]).await;
    assert_eq!(
        reported(&body),
        expected_attachments(&arns),
        "an omitted PathPrefix lists every attachment, got {:?}",
        reported(&body)
    );
}

/// RULE: a `Marker` names a position in ONE group's attachment order, so a token this
/// service did not issue for that group — a foreign string, one it issued while
/// paging another group, or one it issued while paging a USER's attachments — is
/// refused with 400 `InvalidInput` rather than read as "start from the beginning".
#[tokio::test]
async fn list_attached_group_policies_refuses_a_marker_it_did_not_issue() {
    let router = router();
    create_customer_policies(&router).await;
    let arns: Vec<String> = (0..policy_count()).map(policy_arn).collect();
    for group in ["MarkerGroupA", "MarkerGroupB"] {
        create_group(&router, group, "/").await;
        for arn in &arns {
            attach_ok(&router, group, arn).await;
        }
    }

    // A user carrying the same attachments, so the token it hands out names the same
    // POSITION in a different principal's order.
    let (status, body) = call(
        &router,
        "CreateUser",
        &[("UserName", "MarkerUser"), ("Path", "/")],
    )
    .await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    for arn in &arns {
        let (status, body) = call(
            &router,
            "AttachUserPolicy",
            &[("UserName", "MarkerUser"), ("PolicyArn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: the fixture user attach must succeed, got {status}: {body}"
        );
    }
    let (_, user_page) = call(
        &router,
        "ListAttachedUserPolicies",
        &[("UserName", "MarkerUser"), ("MaxItems", "1")],
    )
    .await;
    let user_marker = element(&user_page, "Marker");

    // The marker group A hands out while paging its own attachments.
    let (_, first_page) = list(&router, "MarkerGroupA", &[("MaxItems", "1")]).await;
    assert_eq!(
        element(&first_page, "IsTruncated"),
        "true",
        "a one-item page of several attachments is truncated: {first_page}"
    );
    let marker = element(&first_page, "Marker");

    // It resumes group A strictly after the first attachment …
    let (status, resumed) = list(
        &router,
        "MarkerGroupA",
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

    // … and names no position in group B's order, nor does the user's token, nor any
    // string the service never issued.
    let foreign = [
        marker.as_str(),
        user_marker.as_str(),
        "not-a-marker",
        "aWFtOmdyb3VwczpyZWFkZXJz",
        "////",
    ];
    for token in foreign {
        let (status, body) = list(&router, "MarkerGroupB", &[("Marker", token)]).await;
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

/// RULE: a group's attachment set is ONE record, so the group's own list and the
/// reader of the identity policies in force on that group — the context keys those
/// documents reference — answer from the same record.
#[tokio::test]
async fn the_group_list_and_the_principal_policy_reader_answer_from_one_record() {
    let router = router();

    // Rows spanning how many attached policies carry a condition (none, one, two) and
    // how many keys each one tests. The key names are AWS global condition context
    // keys (IAM User Guide, "AWS global condition context keys"); what is under test
    // is which keys the attached documents REFERENCE, not which keys exist.
    let documents: &[(&str, &str, &[&str])] = &[
        (
            "GroupSourceIpBound",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*","Condition":{"IpAddress":{"aws:SourceIp":"10.0.0.0/8"}}}]}"#,
            &["aws:SourceIp"],
        ),
        (
            "GroupMfaAndTimeBound",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:*","Resource":"*","Condition":{"Bool":{"aws:MultiFactorAuthPresent":"false"},"DateGreaterThan":{"aws:CurrentTime":"2026-01-01T00:00:00Z"}}}]}"#,
            &["aws:MultiFactorAuthPresent", "aws:CurrentTime"],
        ),
    ];

    let group_arn = create_group(&router, "ContextKeyGroup", "/eng/").await;

    // Before any attachment: the group lists nothing and its policies reference no
    // key at all.
    let (status, body) = list(&router, "ContextKeyGroup", &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert!(
        reported(&body).is_empty(),
        "case \"no attachment\": expected an empty list, got {:?}",
        reported(&body)
    );
    let (status, body) = call(
        &router,
        "GetContextKeysForPrincipalPolicy",
        &[("PolicySourceArn", &group_arn)],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert!(
        elements(&body, "member").is_empty(),
        "case \"no attachment\": expected no context key, got {:?}",
        elements(&body, "member")
    );

    let mut attached: Vec<String> = Vec::new();
    let mut expected_keys: Vec<String> = Vec::new();
    for (policy_name, document, keys) in documents {
        let arn = create_policy_with(&router, policy_name, "/eng/", document).await;
        attach_ok(&router, "ContextKeyGroup", &arn).await;
        attached.push(arn);
        expected_keys.extend(keys.iter().map(|key| (*key).to_string()));
        expected_keys.sort();

        let (status, body) = list(&router, "ContextKeyGroup", &[]).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            expected_attachments(&attached),
            "case {policy_name:?}: expected the attachments {:?}, got {:?}",
            expected_attachments(&attached),
            reported(&body)
        );

        let (status, body) = call(
            &router,
            "GetContextKeysForPrincipalPolicy",
            &[("PolicySourceArn", &group_arn)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected 200, got {status}: {body}"
        );
        let mut found = elements(&body, "member");
        found.sort();
        assert_eq!(
            found, expected_keys,
            "case {policy_name:?}: expected the context keys {expected_keys:?}, got {found:?}"
        );
    }
}
