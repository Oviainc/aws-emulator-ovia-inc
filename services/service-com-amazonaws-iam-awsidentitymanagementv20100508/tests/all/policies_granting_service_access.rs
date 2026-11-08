// SPDX-License-Identifier: CC0-1.0

//! The policies that grant an IAM identity access to a service, end to end over the
//! awsQuery wire.
//!
//! `ListPoliciesGrantingServiceAccess` reports, for each service namespace the
//! request names, the identity-based policies in force on the identity its `Arn`
//! names that ALLOW at least one action of that service. Every policy it reads here
//! was PERSISTED by an earlier call (`PutUserPolicy`, `PutRolePolicy`,
//! `PutGroupPolicy`, `CreatePolicy` + `AttachUserPolicy`/`AttachRolePolicy`), so each
//! assertion is about what a client on the socket sees after a round trip through the
//! store. The awsQuery harness (router, call, element, elements, error_code) is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.
//!
//! The tests are TABLES whose rows differ along what the rule generalises over: the
//! form of the action element (a literal action, a wildcard run in the action, a
//! wildcard inside the namespace, the whole-element wildcard, a list of actions,
//! `NotAction`), the effect, the form of `Statement`, the case the namespace is
//! spelled in, which kind of policy carries the grant (inline on a user, a role or a
//! group, attached managed), how many namespaces one request asks about and how many
//! policies one identity carries — plus the statements that decide a SIGNED call,
//! since IAM authorizes its own API before reporting anything.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, error_code, router,
};

/// A trust policy every role this suite creates needs. It grants `sts:AssumeRole`,
/// which is what makes it evidence for the rule: a trust policy is a RESOURCE-based
/// policy — it says who may assume the role, not what the role may do — so it grants
/// the role access to nothing and must never be reported.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The namespace [`TRUST_POLICY`] would be reported under if it were ever read as an
/// identity-based policy.
const TRUST_POLICY_NAMESPACE: &str = "sts";

/// The sentence the refusal of an ARN naming no held identity carries: AWS words it
/// with the ARN the request supplied, which is the resource this operation is
/// addressed by.
fn arn_does_not_exist(arn: &str) -> String {
    format!("ARN {arn} does not exist.")
}

/// The `PolicyType` value of a policy embedded in the identity.
const INLINE: &str = "INLINE";

/// The `PolicyType` value of a managed policy attached to the identity.
const MANAGED: &str = "MANAGED";

/// Create a user, answering the ARN the account holds it under.
async fn create_user(router: &MultiServiceRouter, user_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: CreateUser must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a role, answering the ARN the account holds it under.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) -> String {
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
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a group, answering the ARN the account holds it under.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: CreateGroup must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Embed `document` in an identity under `policy_name`, through the put that owns
/// the identity's kind.
async fn put_policy(
    router: &MultiServiceRouter,
    action: &str,
    name_parameter: &str,
    entity_name: &str,
    policy_name: &str,
    document: &str,
) {
    let (status, body) = call(
        router,
        action,
        &[
            (name_parameter, entity_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: {action} must succeed, got {status}: {body}"
    );
}

/// Create a managed policy carrying `document`, answering its ARN.
async fn create_managed_policy(
    router: &MultiServiceRouter,
    policy_name: &str,
    document: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[("PolicyName", policy_name), ("PolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Attach the managed policy `policy_arn` to an identity, through the attach that
/// owns the identity's kind.
async fn attach_policy(
    router: &MultiServiceRouter,
    action: &str,
    name_parameter: &str,
    entity_name: &str,
    policy_arn: &str,
) {
    let (status, body) = call(
        router,
        action,
        &[(name_parameter, entity_name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {entity_name:?}: {action} must succeed, got {status}: {body}"
    );
}

/// One `ListPoliciesGrantingServiceAccess` call: the identity's ARN, the namespaces
/// asked about, and an optional `Marker`.
async fn granting(
    router: &MultiServiceRouter,
    arn: &str,
    namespaces: &[&str],
    marker: Option<&str>,
) -> (u16, String) {
    let names: Vec<String> = (1..=namespaces.len())
        .map(|position| format!("ServiceNamespaces.member.{position}"))
        .collect();
    let mut params: Vec<(&str, &str)> = vec![("Arn", arn)];
    for (name, namespace) in names.iter().zip(namespaces) {
        params.push((name.as_str(), namespace));
    }
    if let Some(marker) = marker {
        params.push(("Marker", marker));
    }
    call(router, "ListPoliciesGrantingServiceAccess", &params).await
}

/// The text of the FIRST `<tag>` element in `chunk`, if it carries one.
///
/// The response nests a list inside a list, which the suite's shared `elements`
/// helper reads only as leaves, so each entry and each policy is cut out of the body
/// by its first member (`ServiceNamespace`, `PolicyName`) and its remaining members
/// are read from that slice.
fn first(chunk: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = chunk.find(&open)? + open.len();
    let end = chunk[start..].find(&close)?;
    Some(chunk[start..start + end].to_string())
}

/// The reported entries: one per namespace, in document order, each with the
/// policies reported for it rendered as the members the report must carry.
fn reported(body: &str) -> Vec<(String, Vec<String>)> {
    body.split("<ServiceNamespace>")
        .skip(1)
        .map(|chunk| {
            let (namespace, rest) = chunk
                .split_once("</ServiceNamespace>")
                .unwrap_or_else(|| panic!("<ServiceNamespace> is never closed in {body}"));
            let policies = rest
                .split("<PolicyName>")
                .skip(1)
                .map(|policy| {
                    let (name, members) = policy
                        .split_once("</PolicyName>")
                        .unwrap_or_else(|| panic!("<PolicyName> is never closed in {body}"));
                    let kind = first(members, "PolicyType")
                        .unwrap_or_else(|| panic!("a reported policy carries no type: {body}"));
                    match kind.as_str() {
                        MANAGED => format!(
                            "{MANAGED} {name} {}",
                            first(members, "PolicyArn").unwrap_or_default()
                        ),
                        _ => format!(
                            "{kind} {name} {}/{}",
                            first(members, "EntityType").unwrap_or_default(),
                            first(members, "EntityName").unwrap_or_default()
                        ),
                    }
                })
                .collect();
            (namespace.to_string(), policies)
        })
        .collect()
}

/// How one inline policy must be reported.
fn inline(policy_name: &str, entity_type: &str, entity_name: &str) -> String {
    format!("{INLINE} {policy_name} {entity_type}/{entity_name}")
}

/// How one attached managed policy must be reported.
fn managed(policy_name: &str, policy_arn: &str) -> String {
    format!("{MANAGED} {policy_name} {policy_arn}")
}

/// One row: the document a user embeds, and whether each namespace asked about must
/// report it.
struct Case {
    case: &'static str,
    document: &'static str,
    expected: &'static [(&'static str, bool)],
}

/// Rows spanning the forms an action element takes in the IAM policy grammar — a
/// literal action, a wildcard run in the action part, a wildcard inside the
/// namespace, the whole-element wildcard, a list of actions, `NotAction` excluding a
/// whole namespace and `NotAction` excluding one action — beside the effect, the form
/// of `Statement`, and a statement that is about no action at all.
///
/// The namespaces are service prefixes the IAM User Guide publishes ("Actions,
/// resources, and condition keys for AWS services"); the rule under test is which
/// namespace an action element REACHES, not which services exist, so nothing here
/// depends on that catalog.
const CASES: &[Case] = &[
    Case {
        case: "a literal action",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("s3", true), ("ec2", false)],
    },
    Case {
        case: "every action of one service",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#,
        expected: &[("s3", true), ("s3outposts", false), ("ec2", false)],
    },
    Case {
        case: "a wildcard run in the action part",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"ec2:Describe*","Resource":"*"}]}"#,
        expected: &[("ec2", true), ("s3", false)],
    },
    Case {
        case: "the whole-element wildcard",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
        expected: &[("s3", true), ("ec2", true), ("a4b", true)],
    },
    Case {
        case: "a wildcard inside the namespace",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"dynamo*:GetItem","Resource":"*"}]}"#,
        expected: &[("dynamodb", true), ("s3", false)],
    },
    Case {
        case: "a namespace spelled in another case",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"S3:GetObject","Resource":"*"}]}"#,
        expected: &[("s3", true)],
    },
    Case {
        case: "a list of actions in two namespaces",
        document: r#"{"Statement":[{"Effect":"Allow","Action":["iam:ListUsers","kms:Decrypt"],"Resource":"*"}]}"#,
        expected: &[("iam", true), ("kms", true), ("s3", false)],
    },
    Case {
        case: "a statement scoped to one resource",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/*"}]}"#,
        expected: &[("s3", true)],
    },
    Case {
        case: "a Deny and nothing else",
        document: r#"{"Statement":[{"Effect":"Deny","Action":"s3:*","Resource":"*"}]}"#,
        expected: &[("s3", false)],
    },
    Case {
        case: "an Allow beside a Deny of the same actions",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"},{"Effect":"Deny","Action":"s3:*","Resource":"*"}]}"#,
        expected: &[("s3", true)],
    },
    Case {
        case: "two statements, each about one namespace",
        document: r#"{"Statement":[{"Effect":"Allow","Action":"sqs:SendMessage","Resource":"*"},{"Effect":"Allow","Action":"sns:Publish","Resource":"*"}]}"#,
        expected: &[("sqs", true), ("sns", true), ("s3", false)],
    },
    Case {
        case: "a bare statement object rather than a list",
        document: r#"{"Statement":{"Effect":"Allow","Action":"lambda:InvokeFunction","Resource":"*"}}"#,
        expected: &[("lambda", true), ("s3", false)],
    },
    Case {
        case: "NotAction excluding a whole namespace",
        document: r#"{"Statement":[{"Effect":"Allow","NotAction":"iam:*","Resource":"*"}]}"#,
        expected: &[("s3", true), ("iam", false)],
    },
    Case {
        case: "NotAction excluding one action of a namespace",
        document: r#"{"Statement":[{"Effect":"Allow","NotAction":"s3:GetObject","Resource":"*"}]}"#,
        expected: &[("s3", true), ("ec2", true)],
    },
    Case {
        case: "a statement about no action at all",
        document: r#"{"Statement":[{"Effect":"Allow","Resource":"*"}]}"#,
        expected: &[("s3", false)],
    },
    Case {
        case: "a policy carrying no statement of its own",
        document: r#"{"Statement":[]}"#,
        expected: &[("s3", false), ("ec2", false)],
    },
];

/// A policy is reported for a namespace exactly when one of its statements allows an
/// action of that service, and every namespace the request asks about is answered —
/// with an empty list when nothing grants it.
#[tokio::test]
async fn a_policy_is_reported_for_each_namespace_it_allows_an_action_of() {
    let router = router();
    for (index, case) in CASES.iter().enumerate() {
        let Case {
            case,
            document,
            expected,
        } = case;
        let user_name = format!("grant{index}");
        let arn = create_user(&router, &user_name, "/eng/").await;
        put_policy(
            &router,
            "PutUserPolicy",
            "UserName",
            &user_name,
            "Embedded",
            document,
        )
        .await;

        let namespaces: Vec<&str> = expected.iter().map(|(namespace, _)| *namespace).collect();
        let (status, body) = granting(&router, &arn, &namespaces, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read must succeed, got {status}: {body}"
        );
        let expected_entries: Vec<(String, Vec<String>)> = expected
            .iter()
            .map(|(namespace, granted)| {
                let policies = if *granted {
                    vec![inline("Embedded", "USER", &user_name)]
                } else {
                    Vec::new()
                };
                ((*namespace).to_string(), policies)
            })
            .collect();
        assert_eq!(
            reported(&body),
            expected_entries,
            "case {case:?}: expected {expected_entries:?}, got {body}"
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {case:?}: one page carries every namespace asked about, got {body}"
        );
    }
}

/// Every identity-based policy in force on an identity is a candidate: the policies
/// embedded in it and the default version of each managed policy attached to it, for
/// a user, a role and a group alike — and a role's trust policy is not one of them.
#[tokio::test]
async fn the_report_spans_every_policy_in_force_on_the_identity() {
    let router = router();

    // A user carrying one inline policy and one attached managed policy, each about
    // a different namespace, plus a second inline policy about one of the same ones:
    // so an entry reporting several policies is exercised as well as one reporting a
    // single policy.
    let user_arn = create_user(&router, "Granted", "/sales/").await;
    put_policy(
        &router,
        "PutUserPolicy",
        "UserName",
        "Granted",
        "UserInline",
        r#"{"Statement":[{"Effect":"Allow","Action":"s3:ListBucket","Resource":"*"}]}"#,
    )
    .await;
    put_policy(
        &router,
        "PutUserPolicy",
        "UserName",
        "Granted",
        "UserInlineToo",
        r#"{"Statement":[{"Effect":"Allow","Action":"s3:PutObject","Resource":"*"}]}"#,
    )
    .await;
    let user_managed_arn = create_managed_policy(
        &router,
        "UserManaged",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"dynamodb:*","Resource":"*"}]}"#,
    )
    .await;
    attach_policy(
        &router,
        "AttachUserPolicy",
        "UserName",
        "Granted",
        &user_managed_arn,
    )
    .await;

    let (status, body) = granting(&router, &user_arn, &["s3", "dynamodb", "ec2"], None).await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![
            (
                "s3".to_string(),
                vec![
                    inline("UserInline", "USER", "Granted"),
                    inline("UserInlineToo", "USER", "Granted"),
                ]
            ),
            (
                "dynamodb".to_string(),
                vec![managed("UserManaged", &user_managed_arn)]
            ),
            ("ec2".to_string(), Vec::new()),
        ],
        "expected the user's inline and attached policies, got {body}"
    );

    // A role: its inline policy and its attached managed policy are reported, its
    // trust policy is not.
    let role_arn = create_role(&router, "GrantedRole", "/").await;
    put_policy(
        &router,
        "PutRolePolicy",
        "RoleName",
        "GrantedRole",
        "RoleInline",
        r#"{"Statement":[{"Effect":"Allow","Action":"kms:Decrypt","Resource":"*"}]}"#,
    )
    .await;
    let role_managed_arn = create_managed_policy(
        &router,
        "RoleManaged",
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sqs:*","Resource":"*"}]}"#,
    )
    .await;
    attach_policy(
        &router,
        "AttachRolePolicy",
        "RoleName",
        "GrantedRole",
        &role_managed_arn,
    )
    .await;

    let (status, body) = granting(
        &router,
        &role_arn,
        &["kms", "sqs", TRUST_POLICY_NAMESPACE],
        None,
    )
    .await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![
            (
                "kms".to_string(),
                vec![inline("RoleInline", "ROLE", "GrantedRole")]
            ),
            (
                "sqs".to_string(),
                vec![managed("RoleManaged", &role_managed_arn)]
            ),
            (TRUST_POLICY_NAMESPACE.to_string(), Vec::new()),
        ],
        "the trust policy grants the role nothing, got {body}"
    );

    // A group: its inline policy is reported under the group's own entity type.
    let group_arn = create_group(&router, "GrantedGroup", "/eng/").await;
    put_policy(
        &router,
        "PutGroupPolicy",
        "GroupName",
        "GrantedGroup",
        "GroupInline",
        r#"{"Statement":[{"Effect":"Allow","Action":"logs:PutLogEvents","Resource":"*"}]}"#,
    )
    .await;
    let (status, body) = granting(&router, &group_arn, &["logs", "s3"], None).await;
    assert_eq!(status, 200, "the read must succeed, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![
            (
                "logs".to_string(),
                vec![inline("GroupInline", "GROUP", "GrantedGroup")]
            ),
            ("s3".to_string(), Vec::new()),
        ],
        "expected the group's inline policy, got {body}"
    );

    // A read reports, so a second identical read answers the same thing and what it
    // read is still there to read.
    let (status, repeated) = granting(&router, &user_arn, &["s3", "dynamodb", "ec2"], None).await;
    assert_eq!(status, 200, "the repeated read must succeed, got {repeated}");
    let (_, first_read) = granting(&router, &user_arn, &["s3", "dynamodb", "ec2"], None).await;
    assert_eq!(
        reported(&repeated),
        reported(&first_read),
        "a read must not change what a later read sees, got {repeated}"
    );
    let (status, listed) = call(
        &router,
        "ListAttachedUserPolicies",
        &[("UserName", "Granted")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the attachment must survive the read, got {status}: {listed}"
    );
    assert_eq!(
        element(&listed, "PolicyName"),
        "UserManaged",
        "the read must leave the attachment as it was, got {listed}"
    );
}

/// A request asking about more namespaces than one page carries is reported over
/// pages, each resumable with the `Marker` the one before it reported.
#[tokio::test]
async fn the_namespaces_are_reported_over_resumable_pages() {
    // The number of items an IAM list operation reports when a request supplies no
    // `MaxItems`, which is what bounds a page here: this operation takes no
    // `MaxItems` of its own.
    const PAGE: usize = 100;
    // Rows: one page exactly, one more than a page, and the largest request
    // `serviceNamespaceListType` admits (len 1..200).
    let counts = [PAGE, PAGE + 1, 200];

    let router = router();
    let arn = create_user(&router, "Paged", "/").await;
    put_policy(
        &router,
        "PutUserPolicy",
        "UserName",
        "Paged",
        "Everything",
        r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
    )
    .await;

    for count in counts {
        // The first `count` service prefixes AWS publishes, which is what a request may
        // ask about: a namespace has to name a service, so the request is built from the
        // catalog this service reads them against rather than from invented words.
        let owned: Vec<String> = state::REPORTED_SERVICES
            .iter()
            .take(count)
            .map(|service| service.service_namespace.to_string())
            .collect();
        assert_eq!(
            owned.len(),
            count,
            "case {count}: the catalog must publish at least {count} namespaces, got {}",
            owned.len()
        );
        let namespaces: Vec<&str> = owned.iter().map(String::as_str).collect();

        let (status, body) = granting(&router, &arn, &namespaces, None).await;
        assert_eq!(
            status, 200,
            "case {count}: the first page must succeed, got {status}: {body}"
        );
        let first_page = reported(&body);
        let expected_first = count.min(PAGE);
        assert_eq!(
            first_page.len(),
            expected_first,
            "case {count}: expected {expected_first} entries on the first page, got {body}"
        );
        assert_eq!(
            first_page
                .iter()
                .map(|(namespace, _)| namespace.as_str())
                .collect::<Vec<&str>>(),
            namespaces[..expected_first],
            "case {count}: the entries must follow the request's order, got {body}"
        );
        assert!(
            first_page
                .iter()
                .all(|(_, policies)| policies == &[inline("Everything", "USER", "Paged")]),
            "case {count}: a policy allowing every action grants every namespace, got {body}"
        );

        let truncated = count > PAGE;
        assert_eq!(
            element(&body, "IsTruncated"),
            truncated.to_string(),
            "case {count}: expected IsTruncated {truncated}, got {body}"
        );
        if !truncated {
            assert!(
                first(&body, "Marker").is_none(),
                "case {count}: a page that is not truncated names no marker, got {body}"
            );
            continue;
        }

        let marker = element(&body, "Marker");
        let (status, body) = granting(&router, &arn, &namespaces, Some(&marker)).await;
        assert_eq!(
            status, 200,
            "case {count}: the second page must succeed, got {status}: {body}"
        );
        let second_page = reported(&body);
        assert_eq!(
            second_page
                .iter()
                .map(|(namespace, _)| namespace.as_str())
                .collect::<Vec<&str>>(),
            namespaces[PAGE..],
            "case {count}: the second page must resume after the first, got {body}"
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {count}: the last page must not be truncated, got {body}"
        );
    }

    // A marker this service did not issue is refused rather than read as the start of
    // the list. Rows: not base64 at all, base64 of a foreign payload, and a marker
    // issued for another collection.
    create_user(&router, "PagedToo", "/").await;
    let (_, listed) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    let foreign = element(&listed, "Marker");
    for (case, marker) in [
        ("not a token this service issued", "not-a-marker"),
        ("base64 of a foreign payload", "aWFtOnBvbGljaWVzOjA="),
        ("a marker issued for another collection", foreign.as_str()),
    ] {
        let (status, body) = granting(&router, &arn, &["s3"], Some(marker)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {marker}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }
}

/// IAM authorizes its own API: a call signed by an IAM user is decided against that
/// user's identity-based policies before anything is reported, on the identity the
/// request addresses, and a refusal is HTTP 403 `AccessDenied`.
#[tokio::test]
async fn the_signed_call_is_decided_against_the_callers_own_policies() {
    /// The IAM action this operation is authorized under.
    const ACTION: &str = "iam:ListPoliciesGrantingServiceAccess";

    let router = router();
    let subject = create_user(&router, "Subject", "/eng/").await;
    let other = format!("arn:aws:iam::{}:user/Elsewhere", account_id());

    // Rows spanning the forms a statement about this call takes: an Allow naming the
    // action and the identity asked about, an Allow naming every action, an Allow
    // scoped to ANOTHER identity, an Allow beside a Deny of the same call, a Deny
    // scoped to another identity, and a caller carrying no statement about the call
    // at all. `None` is "served"; `Some(reason)` is the refusal that must be worded
    // with it.
    let allow_everything = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#.to_string();
    let allow_this = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{ACTION}","Resource":"{subject}"}}]}}"#
    );
    let allow_other = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"{ACTION}","Resource":"{other}"}}]}}"#
    );
    let deny_this = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Action":"{ACTION}","Resource":"{subject}"}}]}}"#
    );
    let deny_other = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny","Action":"{ACTION}","Resource":"{other}"}}]}}"#
    );
    let no_allow = format!("because no identity-based policy allows the {ACTION} action");
    let explicit_deny = "with an explicit deny in an identity-based policy".to_string();
    let rows: Vec<(&str, Vec<String>, Option<String>)> = vec![
        (
            "an Allow naming the action and the identity",
            vec![allow_this.clone()],
            None,
        ),
        (
            "an Allow naming every action",
            vec![allow_everything.clone()],
            None,
        ),
        (
            "an Allow scoped to another identity",
            vec![allow_other],
            Some(no_allow.clone()),
        ),
        (
            "an Allow beside a Deny of the same call",
            vec![allow_everything.clone(), deny_this],
            Some(explicit_deny),
        ),
        (
            "a Deny scoped to another identity",
            vec![allow_everything, deny_other],
            None,
        ),
        ("no statement about the call at all", Vec::new(), Some(no_allow)),
    ];

    for (row, (case, policies, refusal)) in rows.into_iter().enumerate() {
        let caller = format!("caller{row}");
        create_user(&router, &caller, "/").await;
        for (position, document) in policies.iter().enumerate() {
            put_policy(
                &router,
                "PutUserPolicy",
                "UserName",
                &caller,
                &format!("policy-{position}"),
                document,
            )
            .await;
        }
        let (status, key) = call(&router, "CreateAccessKey", &[("UserName", &caller)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
        );
        let access_key_id = element(&key, "AccessKeyId");

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListPoliciesGrantingServiceAccess",
            &[("Arn", subject.as_str()), ("ServiceNamespaces.member.1", "s3")],
        )
        .await;
        match refusal {
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected the call to be served, got {status}: {body}"
            ),
            Some(reason) => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {body}"
                );
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{caller}", account_id()),
                    ACTION,
                    &subject,
                    &reason,
                );
            }
        }
    }
}

/// An ARN that names no identity the account holds is refused as a missing entity,
/// and one that is not an identity's ARN at all as invalid input.
#[tokio::test]
async fn an_arn_naming_no_held_identity_is_refused() {
    let router = router();
    let held = create_user(&router, "HeldUser", "/eng/").await;
    create_role(&router, "HeldRole", "/").await;
    let account = account_id();

    // Rows: a user never created, a role never created, a group never created, a held
    // user's name under another path, a held user's name in another account, and a
    // held role's name carrying the user resource type.
    let absent = [
        (
            "a user nobody created",
            format!("arn:aws:iam::{account}:user/ghost"),
        ),
        (
            "a role nobody created",
            format!("arn:aws:iam::{account}:role/eng/ghost"),
        ),
        (
            "a group nobody created",
            format!("arn:aws:iam::{account}:group/Developers"),
        ),
        (
            "a held user under another path",
            format!("arn:aws:iam::{account}:user/HeldUser"),
        ),
        (
            "a held user in another account",
            "arn:aws:iam::000000000001:user/eng/HeldUser".to_string(),
        ),
        (
            "a held role under the user resource type",
            format!("arn:aws:iam::{account}:user/HeldRole"),
        ),
    ];
    for (case, arn) in absent {
        let (status, body) = granting(&router, &arn, &["s3"], None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for {arn}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            arn_does_not_exist(&arn),
            "case {case:?}: expected the sentence naming the ARN, got {body}"
        );
    }

    // Rows spanning the ways an ARN can fail to name an identity: not an ARN at all,
    // another service's resource, IAM resources of other types, an IAM ARN carrying a
    // region, one carrying no account, and one whose name is empty. Every row is at
    // least the 20 characters `arnType` requires, so the boundary validator passes it
    // to the handler.
    let invalid = [
        ("not an ARN at all", "a-string-of-twenty-c".to_string()),
        (
            "another service's resource",
            "arn:aws:s3:::my-bucket-name/key".to_string(),
        ),
        (
            "an IAM managed policy",
            format!("arn:aws:iam::{account}:policy/ReadOnly"),
        ),
        (
            "an IAM instance profile",
            format!("arn:aws:iam::{account}:instance-profile/Build"),
        ),
        (
            "an IAM ARN carrying a region",
            format!("arn:aws:iam:us-east-1:{account}:user/alice"),
        ),
        (
            "an IAM ARN carrying no account",
            "arn:aws:iam:::user/alice-with-a-long-name".to_string(),
        ),
        (
            "a user ARN with an empty name",
            format!("arn:aws:iam::{account}:user/"),
        ),
    ];
    for (case, candidate) in invalid {
        let (status, body) = granting(&router, &candidate, &["s3"], None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {candidate}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // The held user is still read, so the refusals above are about the ARN rather
    // than about the store.
    let (status, body) = granting(&router, &held, &["s3"], None).await;
    assert_eq!(
        status, 200,
        "the held identity must still be read, got {status}: {body}"
    );
    assert_eq!(
        reported(&body),
        vec![("s3".to_string(), Vec::new())],
        "a user carrying no policy grants nothing, got {body}"
    );
}
