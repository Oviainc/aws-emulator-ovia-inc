// SPDX-License-Identifier: CC0-1.0

//! The NAME a service-linked role carries, end to end over the awsQuery wire.
//!
//! `CreateServiceLinkedRole$AWSServiceName` is only the lookup key: "service
//! principals are unique and case-sensitive. To find the exact service principal
//! for your service-linked role, see Amazon Web Services services that work with
//! IAM" — the role's own name is the one AWS assigns the service, and nothing in
//! the IAM model carries it. So the name the create files the role under, and the
//! name `GetRole` resolves, is that assigned name rather than any transformation
//! of the principal's labels.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! shape of the principal's first label (one word, two words joined by a hyphen, a
//! label carrying the service's API version, an acronym), the shape of the assigned
//! name (the service's name in mixed case, an "Amazon"/"AWS"-prefixed name, a name
//! naming a feature), and whether AWS assigns the service a name at all. The
//! awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    MAX_ROLE_NAME_CHARACTERS, PUBLISHED_SERVICE_LINKED_ROLE_NAMES,
};

use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// Principals whose first label is NOT the label AWS puts in the role name, with the
/// name AWS assigns.
///
/// Source: the "Using service-linked roles for <service>" page of each service's own
/// user guide, the same documentation `src/state.rs`'s
/// `PUBLISHED_SERVICE_LINKED_ROLE_NAMES` cites. Rows span a two-word hyphenated
/// label whose name is the service's own name in mixed case (Migration Hub Strategy
/// Recommendations and Migration Hub Orchestrator, reached through the
/// `migrationhub-strategy` and `migrationhub-orchestrator` endpoint labels), a
/// one-word hyphenated label (Compute Optimizer), a label carrying the service's API
/// version (`resource-explorer-2`), a hyphenated label whose name is prefixed with
/// `Amazon` and names the feature it serves (EKS on Fargate), a label whose words
/// are re-capitalised (CodeStar Notifications), and an acronym the name spells
/// differently (AWS SSM for SAP).
const ASSIGNED_NAMES: &[(&str, &str)] = &[
    (
        "migrationhub-strategy.amazonaws.com",
        "AWSServiceRoleForMigrationHubStrategy",
    ),
    (
        "migrationhub-orchestrator.amazonaws.com",
        "AWSServiceRoleForMigrationHubOrchestrator",
    ),
    ("compute-optimizer.amazonaws.com", "AWSServiceRoleForComputeOptimizer"),
    ("resource-explorer-2.amazonaws.com", "AWSServiceRoleForResourceExplorer"),
    ("eks-fargate.amazonaws.com", "AWSServiceRoleForAmazonEKSForFargate"),
    (
        "codestar-notifications.amazonaws.com",
        "AWSServiceRoleForCodeStarNotifications",
    ),
    ("ssm-sap.amazonaws.com", "AWSServiceRoleForAWSSSMForSAP"),
];

/// The name a MECHANICAL transformation of the principal would produce: the
/// service-provided prefix followed by the principal's first label with only its
/// initial letter upper-cased — the fallback
/// `src/ops/create_service_linked_role.rs` documents for a principal AWS assigns no
/// name to.
///
/// Written out here so each row can assert that this name is NOT what addresses the
/// role when AWS assigns the service one of its own.
fn mechanically_derived_name(service_principal: &str) -> String {
    let label = service_principal
        .split('.')
        .next()
        .unwrap_or(service_principal);
    let mut name = String::from("AWSServiceRoleFor");
    let mut characters = label.chars();
    if let Some(first) = characters.next() {
        name.extend(first.to_uppercase());
        name.push_str(characters.as_str());
    }
    name
}

/// RULE: a service-linked role is addressed by the role name AWS ASSIGNS the service
/// its principal names, so `CreateServiceLinkedRole` files the role under that name
/// and reports it there, `GetRole` on that name answers 200 with that same role, and
/// a name a transformation of the principal's labels produces instead addresses no
/// role (`NoSuchEntity`, HTTP 404).
#[tokio::test]
async fn the_role_is_filed_and_read_under_the_name_aws_assigns_the_service() {
    for (service_principal, assigned_name) in ASSIGNED_NAMES {
        let case = *service_principal;
        // Each row gets its own account: a second create of one role name is a
        // duplicate, and the rows are independent services.
        let router = router();

        // The service keeps its role provisioned, so the assigned name resolves
        // before the account asks for one of its own — while the mechanically
        // derived name resolves nothing at all.
        let (status, body) = call(&router, "GetRole", &[("RoleName", assigned_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRole on {assigned_name:?} to answer 200, \
             got {status}: {body}"
        );
        let derived = mechanically_derived_name(service_principal);
        assert_ne!(
            derived, *assigned_name,
            "case {case:?}: this row is only meaningful where the assigned name differs \
             from the derived one, got {derived:?}"
        );
        let (status, body) = call(&router, "GetRole", &[("RoleName", derived.as_str())]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetRole on the derived {derived:?} to answer 404, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity for {derived:?}, got {:?}",
            error_code(&body)
        );

        // The create files the account's own record under the assigned name.
        let (status, created) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", service_principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status}: {created}"
        );
        assert_eq!(
            element(&created, "RoleName"),
            *assigned_name,
            "case {case:?}: expected the assigned role name {assigned_name:?}, got {:?}",
            element(&created, "RoleName")
        );
        let expected_path = format!("/aws-service-role/{service_principal}/");
        assert_eq!(
            element(&created, "Path"),
            expected_path,
            "case {case:?}: expected the service-linked role path {expected_path:?}, got {:?}",
            element(&created, "Path")
        );
        let expected_arn = format!(
            "arn:aws:iam::{}:role{expected_path}{assigned_name}",
            account_id()
        );
        assert_eq!(
            element(&created, "Arn"),
            expected_arn,
            "case {case:?}: expected the ARN {expected_arn:?}, got {:?}",
            element(&created, "Arn")
        );

        // And the read resolves THAT record, which is the round trip a client's
        // documented bootstrap makes: create the role, then confirm it by the name
        // AWS publishes.
        let (status, fetched) = call(&router, "GetRole", &[("RoleName", assigned_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRole 200 after the create, got {status}: {fetched}"
        );
        assert_eq!(
            element(&fetched, "RoleId"),
            element(&created, "RoleId"),
            "case {case:?}: expected the created role to be the one read back, got {:?}",
            element(&fetched, "RoleId")
        );
        assert_eq!(
            element(&fetched, "RoleName"),
            *assigned_name,
            "case {case:?}: expected the role reported under {assigned_name:?}, got {:?}",
            element(&fetched, "RoleName")
        );

        // The derived name is still nobody's: the create did not file a second
        // record under a name no service assigns.
        let (status, body) = call(&router, "GetRole", &[("RoleName", derived.as_str())]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the create filed a role under the derived {derived:?}, \
             got {status}: {body}"
        );
    }
}

/// RULE: a role name addresses ONE role, and `GetRole`'s `RoleName` is a
/// `roleNameType` — len 1..64, pattern `^[\w+=,.@-]+$`
/// (`smithy_query(query="operation", operation="GetRole")`) — so every name the
/// catalog assigns is a name a request can carry, no two services are assigned one
/// name, and no service is assigned two.
#[tokio::test]
async fn each_service_is_assigned_exactly_one_addressable_role_name() {
    let mut seen_names: Vec<String> = Vec::new();
    let mut seen_principals: Vec<String> = Vec::new();

    // Rows: every catalog entry, so the invariant holds over the whole class rather
    // than over the rows this suite writes out.
    for (service_principal, assigned_name) in PUBLISHED_SERVICE_LINKED_ROLE_NAMES {
        let case = *assigned_name;
        let length = assigned_name.chars().count();
        assert!(
            (1..=MAX_ROLE_NAME_CHARACTERS).contains(&length),
            "case {case:?}: expected a name of 1..{MAX_ROLE_NAME_CHARACTERS} characters, \
             got {length}"
        );
        assert!(
            assigned_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_+=,.@-".contains(c)),
            "case {case:?}: expected a name matching ^[\\w+=,.@-]+$, got {assigned_name:?}"
        );

        // IAM names "are not distinguished by case" (`CreateRole$RoleName`), so
        // uniqueness is case-folded; the principal is the catalog's lookup key, and
        // `published_service_linked_role_name` matches it the same way.
        let name_key = assigned_name.to_ascii_lowercase();
        assert!(
            !seen_names.contains(&name_key),
            "case {case:?}: expected one service per assigned role name, got the repeat \
             {assigned_name:?}"
        );
        seen_names.push(name_key);
        let principal_key = service_principal.to_ascii_lowercase();
        assert!(
            !seen_principals.contains(&principal_key),
            "case {case:?}: expected one assigned role name per service, got the repeat \
             {service_principal:?}"
        );
        seen_principals.push(principal_key);
    }

    // Every row this suite writes out by hand is a row of the catalog the service
    // reads, so the two cannot drift apart.
    for (service_principal, assigned_name) in ASSIGNED_NAMES {
        assert!(
            PUBLISHED_SERVICE_LINKED_ROLE_NAMES
                .iter()
                .any(|(principal, name)| principal == service_principal && name == assigned_name),
            "case {assigned_name:?}: expected the catalog to assign it to {service_principal:?}"
        );
    }
}

/// RULE: the assigned name is the SERVICE's, not this emulator's, so a principal AWS
/// assigns no name to is all the fallback can cover: the create files the role under
/// the derived name, `GetRole` resolves it there, and a mixed-case name AWS might
/// have assigned it addresses no role.
#[tokio::test]
async fn a_service_aws_assigns_no_name_to_falls_back_to_the_derived_name() {
    // Rows: a one-word label, a hyphenated two-word label, and a label carrying a
    // digit — none of them in the catalog, each paired with a name a reader might
    // guess AWS had assigned. Each guess differs from the derived name by more than
    // ASCII case, since IAM names "are not distinguished by case"
    // (`CreateRole$RoleName`) and a differently-cased spelling is the SAME name.
    let unassigned: &[(&str, &str)] = &[
        (
            "networkmonitor.amazonaws.com",
            "AWSServiceRoleForCloudWatchNetworkMonitor",
        ),
        ("vendor-widget.amazonaws.com", "AWSServiceRoleForVendorWidget"),
        ("widget-2.amazonaws.com", "AWSServiceRoleForWidget2"),
    ];

    for (service_principal, guessed_name) in unassigned {
        let case = *service_principal;
        assert!(
            PUBLISHED_SERVICE_LINKED_ROLE_NAMES
                .iter()
                .all(|(principal, _)| principal != service_principal),
            "case {case:?}: this row is only meaningful for a principal the catalog does \
             not carry"
        );
        let router = router();
        let derived = mechanically_derived_name(service_principal);

        let (status, created) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", service_principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to answer 200, got {status}: {created}"
        );
        assert_eq!(
            element(&created, "RoleName"),
            derived,
            "case {case:?}: expected the derived name {derived:?}, got {:?}",
            element(&created, "RoleName")
        );

        let (status, fetched) = call(&router, "GetRole", &[("RoleName", derived.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRole on {derived:?} to answer 200, got {status}: {fetched}"
        );
        assert_eq!(
            element(&fetched, "RoleId"),
            element(&created, "RoleId"),
            "case {case:?}: expected the created role to be the one read back, got {:?}",
            element(&fetched, "RoleId")
        );

        let (status, body) = call(&router, "GetRole", &[("RoleName", guessed_name)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetRole on the unassigned {guessed_name:?} to answer 404, \
             got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected NoSuchEntity for {guessed_name:?}, got {:?}",
            error_code(&body)
        );
    }
}
