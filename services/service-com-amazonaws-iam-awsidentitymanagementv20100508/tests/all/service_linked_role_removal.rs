// SPDX-License-Identifier: CC0-1.0

//! What a submitted service-linked role deletion LEAVES, and what such a role carries
//! the moment it is created, end to end over the awsQuery wire.
//!
//! Two rules, both about the one record a service-linked role is:
//!
//! 1. once `DeleteServiceLinkedRole` has named a role, no read of the account reports a
//!    role under that name until a role is created under it again — whether the record
//!    that resolved it was the account's own or the one the owning service keeps
//!    provisioned under a name AWS publishes. A client that waits for a service-linked
//!    role to disappear before recreating it (the documented pattern, since the name is
//!    fixed per service principal) depends on that convergence.
//! 2. "The service controls the attached policies" (`CreateServiceLinkedRole`), so the
//!    role comes into existence carrying the policy AWS files for it under the path it
//!    reserves for service-linked roles.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: whether
//! AWS publishes a role name for the service principal, whether the account created the
//! role or only the owning service keeps it provisioned, the principal's label count,
//! whether a `CustomSuffix` is supplied, and whether any policy is paired with the
//! service at all. The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and reads a
//! request the same way.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The whole `NoSuchEntity` message a submit for a name the account holds no
/// service-linked role of its own under is refused with: one fixed sentence naming
/// nothing of the request, as `tests/all/service_linked_role_deletion.rs` reads it too.
const NO_SERVICE_ROLE_TO_DELETE: &str = "Cannot find the service role to delete.";

/// One role whose deletion is submitted.
struct DeletionCase {
    /// The service the role is linked to.
    service_principal: &'static str,
    /// The `CustomSuffix` the create carries, if any.
    custom_suffix: Option<&'static str>,
    /// Whether the ACCOUNT creates the role, or only the owning service keeps it
    /// provisioned under the name AWS publishes.
    created_by_the_account: bool,
    /// The role name the request addresses — written out rather than recomputed, so the
    /// table pins the naming instead of restating it.
    role_name: &'static str,
}

/// Rows spanning a role the account created and one only the owning service keeps
/// provisioned, published and derived names, two- and three-label principals, and both
/// presence and absence of a `CustomSuffix`.
const DELETION_CASES: &[DeletionCase] = &[
    // The role of the service the recorded lifecycle polls for: created by the account.
    DeletionCase {
        service_principal: "elasticloadbalancing.amazonaws.com",
        custom_suffix: None,
        created_by_the_account: true,
        role_name: "AWSServiceRoleForElasticLoadBalancing",
    },
    // The same published name, with no record of the account's behind it: only the
    // owning service keeps it provisioned.
    DeletionCase {
        service_principal: "elasticloadbalancing.amazonaws.com",
        custom_suffix: None,
        created_by_the_account: false,
        role_name: "AWSServiceRoleForElasticLoadBalancing",
    },
    // A published name carrying a `CustomSuffix`, which no service publishes.
    DeletionCase {
        service_principal: "autoscaling.amazonaws.com",
        custom_suffix: Some("debug"),
        created_by_the_account: true,
        role_name: "AWSServiceRoleForAutoScaling_debug",
    },
    // A principal AWS publishes no name for, so the name is the derived one.
    DeletionCase {
        service_principal: "networkmonitor.amazonaws.com",
        custom_suffix: None,
        created_by_the_account: true,
        role_name: "AWSServiceRoleForNetworkmonitor",
    },
    // A three-label principal whose published name names the service rather than the
    // principal's first label, provisioned rather than created.
    DeletionCase {
        service_principal: "ops.apigateway.amazonaws.com",
        custom_suffix: None,
        created_by_the_account: false,
        role_name: "AWSServiceRoleForAPIGateway",
    },
    // A role every account's support tooling reads, provisioned rather than created.
    DeletionCase {
        service_principal: "support.amazonaws.com",
        custom_suffix: None,
        created_by_the_account: false,
        role_name: "AWSServiceRoleForSupport",
    },
];

/// Create the row's service-linked role, answering the name the service gave it.
async fn create(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &DeletionCase,
) -> String {
    let mut params = vec![("AWSServiceName", case.service_principal)];
    if let Some(suffix) = case.custom_suffix {
        params.push(("CustomSuffix", suffix));
    }
    let (status, body) = call(router, "CreateServiceLinkedRole", &params).await;
    let label = case.role_name;
    assert_eq!(
        status, 200,
        "case {label:?}: the fixture create must succeed, got {status}: {body}"
    );
    element(&body, "RoleName")
}

/// The status and body of a `GetRole` for `role_name`.
async fn get_role(
    router: &smithy_server_runtime::MultiServiceRouter,
    role_name: &str,
) -> (u16, String) {
    call(router, "GetRole", &[("RoleName", role_name)]).await
}

/// Assert `GetRole` reports no role under `role_name`: 404 `NoSuchEntity` with the one
/// sentence this service words a missing role with, the role name substituted.
async fn assert_role_is_gone(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    role_name: &str,
) {
    let (status, body) = get_role(router, role_name).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the deleted role to be gone, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "case {case:?}: expected NoSuchEntity, got {:?}",
        error_code(&body)
    );
    let expected = format!("The role with name {role_name} cannot be found.");
    assert_eq!(
        element(&body, "Message"),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        element(&body, "Message")
    );
}

/// RULE: a `DeleteServiceLinkedRole` submit is the account giving the named
/// service-linked role up, so from then on NO read reports a role under that name —
/// neither the account's own record nor the one the owning service keeps provisioned
/// under a published name — until a role is created under that name again, which brings
/// it back.
#[tokio::test]
async fn a_submitted_deletion_leaves_no_role_under_that_name() {
    for case in DELETION_CASES {
        // A router of its own per row, so one row's deletion never explains another's.
        let router = router();
        let label = case.role_name;

        if case.created_by_the_account {
            let created = create(&router, case).await;
            assert_eq!(
                created, case.role_name,
                "case {label:?}: expected the create to name the role, got {created:?}"
            );
        }

        // The role is reported before the deletion, whichever record resolves it.
        let (status, body) = get_role(&router, case.role_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the role must be reported before the deletion, got \
             {status}: {body}"
        );

        // The submit: accepted for a role the account holds its own record of, and
        // refused for a name it holds none under — which is the account asking for that
        // service-linked role to be gone either way.
        let (status, body) = call(
            &router,
            "DeleteServiceLinkedRole",
            &[("RoleName", case.role_name)],
        )
        .await;
        if case.created_by_the_account {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the deletion to be submitted, got {status}: {body}"
            );
            let deletion_task_id = element(&body, "DeletionTaskId");
            let (status, status_body) = call(
                &router,
                "GetServiceLinkedRoleDeletionStatus",
                &[("DeletionTaskId", deletion_task_id.as_str())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the task to be reported, got {status}: {status_body}"
            );
            assert_eq!(
                element(&status_body, "Status"),
                state::DELETION_TASK_STATUS_IN_PROGRESS,
                "case {label:?}: expected the deletion the clock owes to be in flight, got {:?}",
                element(&status_body, "Status")
            );
            // The account's own listing no longer carries it.
            let (status, listed) = call(&router, "ListRoles", &[]).await;
            assert_eq!(
                status, 200,
                "case {label:?}: ListRoles must succeed, got {status}: {listed}"
            );
            assert!(
                !elements(&listed, "RoleName").contains(&case.role_name.to_string()),
                "case {label:?}: expected the role to be out of the account's roles, got {:?}",
                elements(&listed, "RoleName")
            );
        } else {
            assert_eq!(
                status, 404,
                "case {label:?}: expected the submit to find no role of the account's, got \
                 {status}: {body}"
            );
            assert_eq!(
                element(&body, "Message"),
                NO_SERVICE_ROLE_TO_DELETE,
                "case {label:?}: expected {NO_SERVICE_ROLE_TO_DELETE:?}, got {:?}",
                element(&body, "Message")
            );
        }

        // Twice, because a client POLLS for the role to be gone: the answer is the same
        // every read, so the poll converges on the first one.
        assert_role_is_gone(&router, label, case.role_name).await;
        assert_role_is_gone(&router, label, case.role_name).await;

        // And the name is free again: a role created under it is reported, so the
        // deletion gave the name up rather than banning it.
        let recreated = create(&router, case).await;
        assert_eq!(
            recreated, case.role_name,
            "case {label:?}: expected the second create to name the same role, got \
             {recreated:?}"
        );
        let (status, body) = get_role(&router, case.role_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the re-created role to be reported, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "RoleName"),
            case.role_name,
            "case {label:?}: expected the role reported under its own name, got {:?}",
            element(&body, "RoleName")
        );
    }
}

/// RULE: a deletion is about ONE role, so giving up the service-linked role of one
/// service leaves every other role of the account — and every other service's role —
/// exactly where it was.
#[tokio::test]
async fn a_deletion_leaves_every_other_role_alone() {
    let router = router();

    // A plain role of the account's, and two service-linked ones.
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "Deployer"),
            (
                "AssumeRolePolicyDocument",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#,
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture create must succeed, got {status}: {body}"
    );
    for principal in ["elasticloadbalancing.amazonaws.com", "ecs.amazonaws.com"] {
        let (status, body) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the fixture create must succeed, got {status}: {body}"
        );
    }

    let (status, body) = call(
        &router,
        "DeleteServiceLinkedRole",
        &[("RoleName", "AWSServiceRoleForElasticLoadBalancing")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the deletion to be submitted, got {status}: {body}"
    );

    // Rows: the plain role, the other service's role, and a published name nothing has
    // asked to delete.
    for untouched in [
        "Deployer",
        "AWSServiceRoleForECS",
        "AWSServiceRoleForTrustedAdvisor",
    ] {
        let (status, body) = get_role(&router, untouched).await;
        assert_eq!(
            status, 200,
            "case {untouched:?}: another role's deletion must leave it reported, got \
             {status}: {body}"
        );
    }
    assert_role_is_gone(
        &router,
        "AWSServiceRoleForElasticLoadBalancing",
        "AWSServiceRoleForElasticLoadBalancing",
    )
    .await;
}

/// RULE: the account's authorization details report the roles the account HOLDS, one
/// repeated `member` entry per role, each built from that role's own record — so the
/// entries a report carries follow the account's roles and nothing else.
#[tokio::test]
async fn the_reported_role_details_are_one_member_per_role_the_account_holds() {
    let router = router();
    let trust_policy = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

    /// The role names the authorization details report, in the order they are reported.
    async fn reported_role_names(
        router: &smithy_server_runtime::MultiServiceRouter,
        case: &str,
    ) -> Vec<String> {
        let (status, body) = call(
            router,
            "GetAccountAuthorizationDetails",
            &[("Filter.member.1", "Role"), ("MaxItems", "1000")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        elements(&body, "RoleName")
    }

    /// The names in the order the account enumerates its roles: IAM holds a role under
    /// its case-folded name ("names are not distinguished by case"), and that key order
    /// is the order a report pages over them in.
    fn in_account_order(names: &[String]) -> Vec<String> {
        let mut ordered = names.to_vec();
        ordered.sort_by_key(|name| name.to_lowercase());
        ordered
    }

    // Rows: an account holding no role, then one, two and three — the cardinalities the
    // report's entry count has to follow, including the single-role account a repeated
    // member list is easiest to get wrong on.
    let mut expected: Vec<String> = Vec::new();
    assert_eq!(
        reported_role_names(&router, "no roles").await,
        expected,
        "expected an account holding no roles to report none"
    );

    for role_name in ["Deployer", "Auditor"] {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name),
                ("AssumeRolePolicyDocument", trust_policy),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the fixture create must succeed, got {status}: {body}"
        );
        expected.push(role_name.to_string());
        expected = in_account_order(&expected);
        let reported = reported_role_names(&router, role_name).await;
        assert_eq!(
            reported, expected,
            "case {role_name:?}: expected one entry per role held, got {reported:?}"
        );
    }

    // A service-linked role is a role of the account too, so it is reported beside the
    // others — and its entry carries the policy its service attached.
    let (status, created) = call(
        &router,
        "CreateServiceLinkedRole",
        &[("AWSServiceName", "elasticloadbalancing.amazonaws.com")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture create must succeed, got {status}: {created}"
    );
    expected.push(element(&created, "RoleName"));
    expected = in_account_order(&expected);
    let reported = reported_role_names(&router, "with a service-linked role").await;
    assert_eq!(
        reported, expected,
        "expected one entry per role held, got {reported:?}"
    );

    // And a role the account gives up leaves the report with one entry fewer.
    let (status, body) = call(
        &router,
        "DeleteServiceLinkedRole",
        &[("RoleName", "AWSServiceRoleForElasticLoadBalancing")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the deletion to be submitted, got {status}: {body}"
    );
    expected.retain(|role_name| role_name != "AWSServiceRoleForElasticLoadBalancing");
    let reported = reported_role_names(&router, "after the deletion").await;
    assert_eq!(
        reported, expected,
        "expected the deleted role to be out of the report, got {reported:?}"
    );
}

/// One service and the policy it controls its service-linked role's permissions
/// through.
///
/// Written out by hand so this suite pins the pairing independently of the table the
/// service reads. Source: the "Using service-linked roles for <service>" page of each
/// service's own user guide, which names the role AND the policy AWS attaches to it —
/// the same documentation `src/state.rs`'s `SERVICE_LINKED_ROLE_POLICIES` cites. Rows
/// span a service whose policy is named for the service (Elastic Load Balancing,
/// Amazon ECS), one whose policy name carries a prefix the principal does not
/// (AWS Support), the Auto Scaling row the recorded suite reads, and two services AWS
/// publishes no policy name this emulator carries for.
const POLICY_ROWS: &[(&str, Option<&str>)] = &[
    (
        "elasticloadbalancing.amazonaws.com",
        Some("ElasticLoadBalancingServiceRolePolicy"),
    ),
    ("ecs.amazonaws.com", Some("AmazonECSServiceRolePolicy")),
    ("support.amazonaws.com", Some("AWSSupportServiceRolePolicy")),
    ("autoscaling.amazonaws.com", Some("AutoScalingServiceRolePolicy")),
    ("networkmonitor.amazonaws.com", None),
    ("mediaconvert.amazonaws.com", None),
];

/// RULE: "the service controls the attached policies" of a service-linked role, so the
/// role is created already carrying the policy AWS files for that service under the path
/// it reserves for service-linked roles — reported by `ListAttachedRolePolicies` under
/// that policy's name and ARN, and by the account's authorization details as the role's
/// attached managed policy.
#[tokio::test]
async fn a_service_linked_role_carries_the_policy_its_service_controls_it_through() {
    for (service_principal, expected_policy) in POLICY_ROWS {
        let router = router();
        let case = *service_principal;

        // The pairing this row writes out is the one the service reads.
        let carried = state::service_linked_role_policy(service_principal)
            .map(|policy| policy.policy_name);
        assert_eq!(
            carried, *expected_policy,
            "case {case:?}: expected the catalog to pair {expected_policy:?} with the \
             service, got {carried:?}"
        );

        let (status, created) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", service_principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture create must succeed, got {status}: {created}"
        );
        let role_name = element(&created, "RoleName");

        let (status, listed) = call(
            &router,
            "ListAttachedRolePolicies",
            &[("RoleName", role_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        let expected_names: Vec<String> = expected_policy
            .iter()
            .map(|policy_name| (*policy_name).to_string())
            .collect();
        // An AWS-managed policy's ARN ends in its path and friendly name, and the path
        // AWS reserves for a service-linked role's policy is `/aws-service-role/`.
        let expected_arns: Vec<String> = expected_policy
            .iter()
            .map(|policy_name| format!("arn:aws:iam::aws:policy/aws-service-role/{policy_name}"))
            .collect();
        assert_eq!(
            elements(&listed, "PolicyName"),
            expected_names,
            "case {case:?}: expected the service's own policy, got {:?}",
            elements(&listed, "PolicyName")
        );
        assert_eq!(
            elements(&listed, "PolicyArn"),
            expected_arns,
            "case {case:?}: expected the policy's own ARN, got {:?}",
            elements(&listed, "PolicyArn")
        );

        // The attachment names a policy a reader of it can report, and one AWS publishes
        // as attachable while reserving it for this role: a service-role policy "can only
        // be attached to the service-linked role for that service", which is the refusal
        // `tests/all/aws_managed_policy_attachability.rs` asserts of every identity's
        // attempt to take one.
        for policy_arn in &expected_arns {
            let (status, read) = call(&router, "GetPolicy", &[("PolicyArn", policy_arn)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the attached policy to resolve, got {status}: {read}"
            );
            assert_eq!(
                element(&read, "IsAttachable"),
                "true",
                "case {case:?}: expected the attachability AWS publishes for a service-role \
                 policy, got {:?}",
                element(&read, "IsAttachable")
            );
            assert_eq!(
                element(&read, "AttachmentCount"),
                "1",
                "case {case:?}: expected the role's attachment to be counted, got {:?}",
                element(&read, "AttachmentCount")
            );
        }

        // The same attachment is what the account's authorization details report for the
        // role.
        let (status, details) = call(
            &router,
            "GetAccountAuthorizationDetails",
            &[("Filter.member.1", "Role"), ("MaxItems", "1000")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {details}"
        );
        assert_eq!(
            elements(&details, "PolicyArn"),
            expected_arns,
            "case {case:?}: expected the role's attached managed policies to be reported, \
             got {:?}",
            elements(&details, "PolicyArn")
        );

        // And the attachment goes with the role: once the deletion is submitted there is
        // no role to report attachments for.
        let (status, body) = call(
            &router,
            "DeleteServiceLinkedRole",
            &[("RoleName", role_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deletion to be submitted, got {status}: {body}"
        );
        let (status, body) = call(
            &router,
            "ListAttachedRolePolicies",
            &[("RoleName", role_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted role's attachments to be gone, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: what the account GIVES UP is a name some service publishes a role under, so a
/// stream of submits for names no service publishes — which are refused, since the
/// account holds no service-linked role under them — leaves the account holding nothing
/// of them, and the record of given-up roles can never outgrow the published catalog.
#[tokio::test]
async fn submits_for_names_no_service_publishes_are_held_nowhere() {
    let (router, engine) = crate::user_lifecycle::router_with_engine();

    // Rows: names of several shapes no service publishes a role under — a bare word, a
    // near-miss of a published name, the bare service-provided prefix, and a run of
    // distinct generated names, which is what a caller repeating the call looks like.
    let mut names = vec![
        "nobody".to_string(),
        "AWSServiceRoleForElasticLoadBalancin".to_string(),
        "AWSServiceRoleFor".to_string(),
    ];
    names.extend((0..64).map(|index| format!("AWSServiceRoleForNothing{index:03}")));
    for role_name in &names {
        let (status, body) = call(
            &router,
            "DeleteServiceLinkedRole",
            &[("RoleName", role_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {role_name:?}: expected the submit to find no service role, got \
             {status}: {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            NO_SERVICE_ROLE_TO_DELETE,
            "case {role_name:?}: expected {NO_SERVICE_ROLE_TO_DELETE:?}, got {:?}",
            element(&body, "Message")
        );
    }

    let held = state::service_state(&*engine).expect("the IAM store");
    let store = held.lock();
    assert!(
        store.withdrawn_service_provisioned_roles.is_empty(),
        "a submit for a name no service publishes must be held nowhere, got {:?}",
        store
            .withdrawn_service_provisioned_roles
            .iter()
            .collect::<Vec<&String>>()
    );
    assert!(
        store.service_linked_role_deletions.is_empty(),
        "a refused submit records no deletion task, got {:?}",
        store
            .service_linked_role_deletions
            .keys()
            .collect::<Vec<&String>>()
    );
    assert!(
        store.withdrawn_service_provisioned_roles.len()
            <= state::PUBLISHED_SERVICE_LINKED_ROLE_NAMES.len(),
        "the record of given-up roles must stay inside the published catalog, got {}",
        store.withdrawn_service_provisioned_roles.len()
    );
}
