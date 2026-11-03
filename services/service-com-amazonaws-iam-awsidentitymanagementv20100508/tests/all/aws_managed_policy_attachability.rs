// SPDX-License-Identifier: CC0-1.0

//! What AWS's own managed-policy catalog offers an account, and what may be attached from
//! it — `ListPolicies` with `Scope=AWS`, cross-read through `GetPolicy`, and enforced by
//! `AttachRolePolicy`, `AttachUserPolicy`, `AttachGroupPolicy`,
//! `PutRolePermissionsBoundary` and `PutUserPermissionsBoundary`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket sees.
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share.
//!
//! Two rules are asserted:
//!
//! 1. the catalog an account enumerates carries the long-standing AWS managed policies a
//!    client resolves BY NAME, each at the path, ARN and attachability AWS publishes for
//!    it;
//! 2. `Policy$IsAttachable` is a report of whether that policy may be attached to an
//!    identity: a policy reported as not attachable is one AWS reserves, so it is refused
//!    by every operation that would attach it or make it an identity's permissions
//!    boundary (400 `PolicyNotAttachable`, IAM's fixed sentence for that reservation
//!    naming the identity kind the call addressed, nothing persisted), and one reported as
//!    attachable is accepted by all of them.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! policy's path (the account root, `/service-role/`, the path reserved for
//! service-linked roles), whether the catalog carries the entry at all, which identity
//! kind the policy is being attached to, and which operation names it.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::role_lifecycle::{borrowed, optional, param};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// A trust policy with a single statement — every role in this suite needs one to exist
/// at all.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The AWS managed policies a client is expected to resolve by name, as `(path, name)`.
///
/// SOURCE: the AWS Managed Policy Reference's account-wide and job-function policies, and
/// its `/service-role/` policies — the names CDK, CloudFormation and the SDK helpers look
/// up when they are asked for "the AWS managed policy called X". The rows deliberately go
/// past the handful any one fixture cites, and span the path depth as well as the name.
const WELL_KNOWN_POLICIES: &[(&str, &str)] = &[
    ("/", "AdministratorAccess"),
    ("/", "PowerUserAccess"),
    ("/", "ReadOnlyAccess"),
    ("/", "IAMFullAccess"),
    ("/", "IAMReadOnlyAccess"),
    ("/", "AmazonS3FullAccess"),
    ("/service-role/", "AWSLambdaBasicExecutionRole"),
    ("/service-role/", "AmazonECSTaskExecutionRolePolicy"),
];

/// One policy as the `Scope=AWS` enumeration reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogPolicy {
    arn: String,
    policy_name: String,
    path: String,
    is_attachable: String,
}

/// Every policy `ListPolicies` reports for `Scope=AWS`, walked over ALL its pages by
/// following the `Marker` the service hands out, with a page size small enough that the
/// walk really pages.
async fn aws_scope_catalog(router: &MultiServiceRouter) -> Vec<CatalogPolicy> {
    let mut reported: Vec<CatalogPolicy> = Vec::new();
    let mut marker: Option<String> = None;
    // The catalog is finite, so the walk must terminate; the cap only keeps a broken
    // `Marker` from looping forever.
    for page in 0..1_000 {
        let mut params = vec![param("Scope", "AWS"), param("MaxItems", "3")];
        if let Some(token) = marker.as_deref() {
            params.push(param("Marker", token));
        }
        let (status, body) = call(router, "ListPolicies", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "page {page}: ListPolicies(Scope=AWS) must succeed, got {status}: {body}"
        );
        let arns = elements(&body, "Arn");
        let names = elements(&body, "PolicyName");
        let paths = elements(&body, "Path");
        let attachable = elements(&body, "IsAttachable");
        for (label, found) in [
            ("PolicyName", names.len()),
            ("Path", paths.len()),
            ("IsAttachable", attachable.len()),
        ] {
            assert_eq!(
                found,
                arns.len(),
                "page {page}: every reported policy must carry one <{label}>, got {found} \
                 for {} policies: {body}",
                arns.len()
            );
        }
        for index in 0..arns.len() {
            reported.push(CatalogPolicy {
                arn: arns[index].clone(),
                policy_name: names[index].clone(),
                path: paths[index].clone(),
                is_attachable: attachable[index].clone(),
            });
        }
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            marker = None;
            break;
        }
    }
    assert!(
        marker.is_none(),
        "the Scope=AWS walk must terminate, got a Marker that never ran out"
    );
    reported
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

/// Create a user at the account root, which every attachment case needs first.
async fn create_user(router: &MultiServiceRouter, user_name: &str) {
    let (status, body) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: CreateUser must succeed, got {status}: {body}"
    );
}

/// Create a group at the account root, which every attachment case needs first.
async fn create_group(router: &MultiServiceRouter, group_name: &str) {
    let (status, body) = call(router, "CreateGroup", &[("GroupName", group_name)]).await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: CreateGroup must succeed, got {status}: {body}"
    );
}

/// The operations that put an AWS managed policy to use on an identity: the three
/// identity kinds a policy can be attached to, and the two entity kinds a policy can
/// bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Use {
    AttachRole,
    AttachUser,
    AttachGroup,
    BoundRole,
    BoundUser,
}

impl Use {
    /// Every one of them, so a table spans the whole class.
    const ALL: &'static [Use] = &[
        Use::AttachRole,
        Use::AttachUser,
        Use::AttachGroup,
        Use::BoundRole,
        Use::BoundUser,
    ];

    fn action(self) -> &'static str {
        match self {
            Use::AttachRole => "AttachRolePolicy",
            Use::AttachUser => "AttachUserPolicy",
            Use::AttachGroup => "AttachGroupPolicy",
            Use::BoundRole => "PutRolePermissionsBoundary",
            Use::BoundUser => "PutUserPermissionsBoundary",
        }
    }

    /// The kind of IAM identity IAM names in the refusal of a policy it reserves — the
    /// identity this use would have put the policy on.
    fn identity_kind(self) -> &'static str {
        match self {
            Use::AttachRole | Use::BoundRole => "role",
            Use::AttachUser | Use::BoundUser => "user",
            Use::AttachGroup => "group",
        }
    }

    /// The request this use makes of `identity` with `policy_arn`.
    fn params(self, identity: &str, policy_arn: &str) -> Vec<(String, String)> {
        match self {
            Use::AttachRole => vec![param("RoleName", identity), param("PolicyArn", policy_arn)],
            Use::AttachUser => vec![param("UserName", identity), param("PolicyArn", policy_arn)],
            Use::AttachGroup => {
                vec![param("GroupName", identity), param("PolicyArn", policy_arn)]
            }
            Use::BoundRole => vec![
                param("RoleName", identity),
                param("PermissionsBoundary", policy_arn),
            ],
            Use::BoundUser => vec![
                param("UserName", identity),
                param("PermissionsBoundary", policy_arn),
            ],
        }
    }
}

/// Make one of those calls.
async fn put_to_use(
    router: &MultiServiceRouter,
    which: Use,
    identity: &str,
    policy_arn: &str,
) -> (u16, String) {
    let params = which.params(identity, policy_arn);
    call(router, which.action(), &borrowed(&params)).await
}

/// The boundary ARN the entity reports, or `None` when it carries none.
async fn reported_boundary(
    router: &MultiServiceRouter,
    which: Use,
    identity: &str,
) -> Option<String> {
    let (action, member) = match which {
        Use::BoundRole => ("GetRole", "RoleName"),
        _ => ("GetUser", "UserName"),
    };
    let (status, body) = call(router, action, &[(member, identity)]).await;
    assert_eq!(
        status, 200,
        "case {identity:?}: {action} must succeed, got {status}: {body}"
    );
    optional(&body, "PermissionsBoundaryArn")
}

/// The managed policy ARNs attached to a role or a user.
async fn attached_arns(router: &MultiServiceRouter, which: Use, identity: &str) -> Vec<String> {
    let (action, member) = match which {
        Use::AttachRole => ("ListAttachedRolePolicies", "RoleName"),
        _ => ("ListAttachedUserPolicies", "UserName"),
    };
    let (status, body) = call(router, action, &[(member, identity)]).await;
    assert_eq!(
        status, 200,
        "case {identity:?}: {action} must succeed, got {status}: {body}"
    );
    elements(&body, "PolicyArn")
}

/// Whether `policy_arn` reports `group_name` among the entities it is attached to.
///
/// A group's attachments are read back through the reporting half of the attachment
/// (`ListEntitiesForPolicy`), because `ListAttachedGroupPolicies` is still a scaffolded
/// 501 in this emulator and belongs to its own stub issue.
async fn group_holds(router: &MultiServiceRouter, policy_arn: &str, group_name: &str) -> bool {
    let (status, body) = call(
        router,
        "ListEntitiesForPolicy",
        &[("PolicyArn", policy_arn), ("EntityFilter", "Group")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_arn:?}: ListEntitiesForPolicy must succeed, got {status}: {body}"
    );
    elements(&body, "GroupName")
        .iter()
        .any(|name| name == group_name)
}

/// Whatever the use recorded for `identity`: the boundary it carries, or the policy it
/// now holds attached — empty when the request recorded nothing.
async fn recorded_use(
    router: &MultiServiceRouter,
    which: Use,
    identity: &str,
    policy_arn: &str,
) -> Vec<String> {
    match which {
        Use::AttachRole | Use::AttachUser => attached_arns(router, which, identity).await,
        Use::AttachGroup => group_holds(router, policy_arn, identity)
            .await
            .then(|| policy_arn.to_string())
            .into_iter()
            .collect(),
        Use::BoundRole | Use::BoundUser => reported_boundary(router, which, identity)
            .await
            .into_iter()
            .collect(),
    }
}

/// Create the identity a row acts on.
async fn create_identity(router: &MultiServiceRouter, which: Use, identity: &str) {
    match which {
        Use::AttachRole | Use::BoundRole => create_role(router, identity).await,
        Use::AttachUser | Use::BoundUser => create_user(router, identity).await,
        Use::AttachGroup => create_group(router, identity).await,
    }
}

/// The catalog entries whose `IsAttachable` is false — the service-linked-role policies.
fn non_attachable_entries() -> Vec<&'static state::AwsManagedPolicy> {
    state::AWS_MANAGED_POLICIES
        .iter()
        .filter(|entry| !entry.is_attachable())
        .collect()
}

/// The catalog entries whose `IsAttachable` is true.
fn attachable_entries() -> Vec<&'static state::AwsManagedPolicy> {
    state::AWS_MANAGED_POLICIES
        .iter()
        .filter(|entry| entry.is_attachable())
        .collect()
}

/// RULE: the catalog an account enumerates carries the long-standing AWS managed policies
/// a client resolves by name, each at the ARN `arn:aws:iam::aws:policy<path><name>` AWS
/// publishes, and the same record its own `GetPolicy` read reports.
#[tokio::test]
async fn the_enumerated_catalog_carries_the_well_known_aws_managed_policies() {
    let router = router();
    let reported = aws_scope_catalog(&router).await;

    for (path, policy_name) in WELL_KNOWN_POLICIES {
        let arn = format!("arn:aws:iam::aws:policy{path}{policy_name}");
        let case = format!("{policy_name} at {path}");
        let listed = reported.iter().find(|policy| &policy.policy_name == policy_name);
        let Some(listed) = listed else {
            panic!(
                "case {case:?}: the Scope=AWS catalog must report {policy_name:?}, got {:?}",
                reported
                    .iter()
                    .map(|policy| policy.policy_name.as_str())
                    .collect::<Vec<&str>>()
            );
        };
        assert_eq!(
            listed.arn, arn,
            "case {case:?}: expected the published ARN, got {:?}",
            listed.arn
        );
        assert_eq!(
            listed.path, *path,
            "case {case:?}: expected the published path, got {:?}",
            listed.path
        );

        // The listing and the single-policy read tell one story about the policy, so a
        // client that resolved the name can read it back by ARN.
        let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", arn.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetPolicy must serve the listed policy, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "PolicyName"),
            *policy_name,
            "case {case:?}: GetPolicy must report the name the listing reported: {body}"
        );
        assert_eq!(
            element(&body, "IsAttachable"),
            listed.is_attachable,
            "case {case:?}: the two readers must report one IsAttachable: {body}"
        );
        assert_eq!(
            listed.is_attachable, "true",
            "case {case:?}: a published job-function or service-role policy is attachable"
        );
    }
}

/// RULE: `Policy$IsAttachable` is reported per policy, so the enumerated catalog offers
/// BOTH kinds — the policies published to be attached, and the ones AWS files under the
/// path it reserves for service-linked roles, which belong to that role alone.
#[tokio::test]
async fn the_enumerated_catalog_reports_both_kinds_of_policy() {
    let router = router();
    let reported = aws_scope_catalog(&router).await;
    assert!(
        reported.len() >= 2,
        "the catalog must carry more than one policy for this table to span anything, got \
         {reported:?}"
    );

    let attachable: Vec<&CatalogPolicy> = reported
        .iter()
        .filter(|policy| policy.is_attachable == "true")
        .collect();
    let not_attachable: Vec<&CatalogPolicy> = reported
        .iter()
        .filter(|policy| policy.is_attachable == "false")
        .collect();
    assert!(
        !attachable.is_empty(),
        "the catalog must offer policies an identity can attach, got {reported:?}"
    );
    assert!(
        !not_attachable.is_empty(),
        "the catalog must offer at least one policy that is NOT attachable, got {reported:?}"
    );

    // Every row's flag follows the published rule, so a client can pre-screen an ARN on
    // the path as well as on the flag, and the two never disagree.
    for policy in &reported {
        let expected = (!policy.path.starts_with("/aws-service-role/")).to_string();
        assert_eq!(
            policy.is_attachable, expected,
            "case {:?}: a policy under the service-linked-role path is not attachable and \
             every other one is, got IsAttachable {:?}",
            policy.arn, policy.is_attachable
        );
    }
}

/// RULE: a managed policy reported as NOT attachable cannot be attached to an identity or
/// used to set one's permissions boundary — 400 `PolicyNotAttachable` with IAM's fixed
/// text, and nothing is recorded on the identity.
#[tokio::test]
async fn a_policy_that_is_not_attachable_is_refused_by_every_use() {
    let router = router();

    // Rows: every non-attachable catalog entry, plus ARNs in that same namespace the
    // catalog does not carry — the refusal is about the policy's attachability, not about
    // which entries this emulator happens to hold.
    let mut policies: Vec<(String, String)> = non_attachable_entries()
        .iter()
        .map(|entry| (format!("the catalog entry {}", entry.policy_name), entry.arn()))
        .collect();
    assert!(
        !policies.is_empty(),
        "the catalog must carry a non-attachable policy for this suite to span anything"
    );
    for arn in [
        "arn:aws:iam::aws:policy/aws-service-role/AWSLambdaRole",
        "arn:aws:iam::aws:policy/aws-service-role/elasticbeanstalk/ServicePolicy",
    ] {
        policies.push((format!("an uncarried {arn}"), arn.to_string()));
    }

    for (row, (case, policy_arn)) in policies.iter().enumerate() {
        for (index, which) in Use::ALL.iter().enumerate() {
            let identity = format!("refused-{row}-{index}");
            create_identity(&router, *which, &identity).await;

            let (status, body) = put_to_use(&router, *which, &identity, policy_arn).await;
            let action = which.action();
            assert_eq!(
                status, 400,
                "case {case:?} through {action}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "PolicyNotAttachable",
                "case {case:?} through {action}: expected the modelled awsQuery code, got \
                 {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                format!(
                    "Cannot attach AWS reserved policy to an IAM {}.",
                    which.identity_kind()
                ),
                "case {case:?} through {action}: expected IAM's fixed text, got {:?}",
                element(&body, "Message")
            );

            // The refusal recorded nothing: the identity carries neither the boundary nor
            // the attachment the request asked for.
            let recorded = recorded_use(&router, *which, &identity, policy_arn).await;
            assert!(
                recorded.is_empty(),
                "case {case:?} through {action}: a refused request must record nothing, got \
                 {recorded:?}"
            );
        }
    }
}

/// RULE: the refusal is about the policy's attachability and nothing else, so every
/// policy the catalog reports as attachable IS accepted by the same operations, and
/// the identity reports it afterwards.
#[tokio::test]
async fn a_policy_that_is_attachable_is_accepted_by_every_use() {
    let router = router();

    // Rows: catalog entries at the account root and under `/service-role/` — the two
    // paths an attachable AWS managed policy is published under.
    let entries = attachable_entries();
    let root = entries
        .iter()
        .find(|entry| entry.path == "/")
        .expect("a catalog entry at the account root");
    let service_role = entries
        .iter()
        .find(|entry| entry.path == "/service-role/")
        .expect("a catalog entry under /service-role/");

    for (index, entry) in [root, service_role].iter().enumerate() {
        let policy_arn = entry.arn();
        let case = policy_arn.clone();
        for (position, which) in Use::ALL.iter().enumerate() {
            let identity = format!("accepted-{index}-{position}");
            create_identity(&router, *which, &identity).await;

            let (status, body) = put_to_use(&router, *which, &identity, &policy_arn).await;
            let action = which.action();
            assert_eq!(
                status, 200,
                "case {case:?} through {action}: expected HTTP 200, got {status}: {body}"
            );
            let recorded = recorded_use(&router, *which, &identity, &policy_arn).await;
            assert_eq!(
                recorded,
                vec![policy_arn.clone()],
                "case {case:?} through {action}: the identity must report the policy it was \
                 given, got {recorded:?}"
            );
        }
    }
}
