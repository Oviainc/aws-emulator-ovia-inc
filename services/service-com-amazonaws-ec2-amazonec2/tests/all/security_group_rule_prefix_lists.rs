// SPDX-License-Identifier: CC0-1.0

//! The PREFIX LIST a security group rule grants, driven over `ec2Query` (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test: "the prefix list IDs" (`IpPermission$PrefixListIds`) a rule grants
//! are resolved against the prefix lists the account can see — the lists it created and the
//! ones the Region publishes for its services, which is what `DescribeManagedPrefixLists`
//! reports ("describes your managed prefix lists and any Amazon Web Services-managed prefix
//! lists") — so a well-formed id that names none of them is refused with that resource's own
//! not-found class and no rule is stored, while an id that names one of them is granted. A
//! rule can only ever grant the ranges its prefix list holds, so an id resolving to nothing
//! grants nothing.
//!
//! The rows differ along what the rule generalises over: the DIRECTION the rule is authorized
//! in (both `AuthorizeSecurityGroupIngress` and `AuthorizeSecurityGroupEgress` resolve one),
//! which kind of visible list is granted (one this account created, of either address family,
//! and each list the Region publishes), and which kind of unknown id is refused (one of the
//! length the service mints, one of the length a published list carries, and one whose
//! Region publishes it to no account at all).

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{PREFIX_LIST_ADDRESS_FAMILY_IPV4, PREFIX_LIST_ADDRESS_FAMILY_IPV6};

use super::ec2_query::{element, elements, emulator, error_code, error_message};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// The two operations that authorize a rule, and the member each reports a granted prefix
/// list on. "Adds the specified inbound (ingress) rules" / "outbound (egress) rules to a
/// security group", and a rule grants "exactly one of the following sources" /
/// "destinations".
const DIRECTIONS: [(&str, &str); 2] = [
    ("AuthorizeSecurityGroupIngress", "false"),
    ("AuthorizeSecurityGroupEgress", "true"),
];

/// A security group of the account's default VPC, by its minted id.
async fn create_group(case: &str, router: &MultiServiceRouter, group_name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateSecurityGroup",
        &[
            param("GroupName", group_name),
            param("GroupDescription", "the group whose prefix lists are read"),
        ],
    )
    .await;
    member(case, &body, "groupId")
}

/// A managed prefix list of `family` this account creates, by its minted id.
async fn create_prefix_list(
    case: &str,
    router: &MultiServiceRouter,
    name: &str,
    family: &str,
    cidr: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateManagedPrefixList",
        &[
            param("PrefixListName", name),
            param("AddressFamily", family),
            param("MaxEntries", "5"),
            param("Entry.1.Cidr", cidr),
        ],
    )
    .await;
    member(case, &body, "prefixListId")
}

/// The form fields of one permission granting the prefix list `prefix_list_id`.
fn permission_params(prefix_list_id: &str) -> Vec<Param> {
    vec![
        param("IpPermissions.1.IpProtocol", "tcp"),
        param("IpPermissions.1.FromPort", "443"),
        param("IpPermissions.1.ToPort", "443"),
        param(
            "IpPermissions.1.PrefixListIds.1.PrefixListId",
            prefix_list_id,
        ),
    ]
}

/// A rule granting a prefix list the account can see is stored and reported with that list,
/// on both sides of the group — the account's own lists, of either address family, and the
/// ones the Region publishes.
#[tokio::test]
async fn a_rule_may_grant_any_prefix_list_the_account_can_see() {
    for (action, is_egress) in DIRECTIONS {
        let (router, _engine) = emulator();
        let case = action;

        // The lists this account can see: two it creates itself, and every one the Region
        // publishes for its services.
        let mut visible: Vec<(String, String)> = vec![
            (
                "an IPv4 list this account created".to_string(),
                create_prefix_list(
                    case,
                    &router,
                    "office-v4",
                    PREFIX_LIST_ADDRESS_FAMILY_IPV4,
                    "203.0.113.0/24",
                )
                .await,
            ),
            (
                "an IPv6 list this account created".to_string(),
                create_prefix_list(
                    case,
                    &router,
                    "office-v6",
                    PREFIX_LIST_ADDRESS_FAMILY_IPV6,
                    "2001:db8::/32",
                )
                .await,
            ),
        ];
        for published in ec2::state::service_prefix_lists_in_region(REGION) {
            visible.push((
                format!("the published list {}", published.prefix_list_name),
                published.prefix_list_id,
            ));
        }

        for (row, prefix_list_id) in visible {
            let group_id = create_group(case, &router, &format!("granting-{prefix_list_id}")).await;
            let mut params = vec![param("GroupId", &group_id)];
            params.extend(permission_params(&prefix_list_id));
            let body = ok(&row, &router, action, &params).await;
            assert_eq!(
                element(&body, "return").as_deref(),
                Some("true"),
                "case {row:?}: expected <return>true</return> from {action}, got {body}"
            );
            let granted = elements(&body, "prefixListId");
            assert_eq!(
                granted,
                vec![prefix_list_id.clone()],
                "case {row:?}: expected the rule to grant {prefix_list_id:?}, got {body}"
            );
            assert_eq!(
                elements(&body, "isEgress"),
                vec![is_egress.to_string()],
                "case {row:?}: expected the rule on the {action} side, got {body}"
            );

            // The stored rule is what the group reports, so the grant is persisted rather
            // than assembled for the reply.
            let described = ok(
                &row,
                &router,
                "DescribeSecurityGroupRules",
                &[param("Filter.1.Name", "group-id"), param("Filter.1.Value.1", &group_id)],
            )
            .await;
            assert!(
                elements(&described, "prefixListId").contains(&prefix_list_id),
                "case {row:?}: expected the stored rule to carry {prefix_list_id:?}, got \
                 {described}"
            );
        }
    }
}

/// A rule naming a well-formed prefix list id the account cannot see is refused with the
/// prefix list's own not-found class, on both sides of the group, and nothing is stored.
#[tokio::test]
async fn a_rule_naming_a_prefix_list_the_account_cannot_see_is_refused() {
    // (case, the id the request names) — the length the service mints for a list of its own,
    // the length a published list carries, and a list of ANOTHER Region, which this Region
    // publishes to nobody.
    let unknown: [(&str, String); 3] = [
        (
            "an id of the length a created list carries",
            "pl-00000000000000bad".to_string(),
        ),
        (
            "an id of the length a published list carries",
            "pl-0a1b2c3d".to_string(),
        ),
        (
            "a list another Region publishes",
            ec2::state::service_prefix_lists_in_region("eu-west-1")
                .first()
                .map(|published| published.prefix_list_id.clone())
                .expect("every Region publishes a prefix list"),
        ),
    ];

    for (action, _) in DIRECTIONS {
        let (router, _engine) = emulator();
        for (row, prefix_list_id) in &unknown {
            let case = format!("{action}: {row}");
            let group_id = create_group(&case, &router, &format!("refused-{row}")).await;
            let mut params = vec![param("GroupId", &group_id)];
            params.extend(permission_params(prefix_list_id));
            let (status, body) = call(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code,
                ec2::state::INVALID_PREFIX_LIST_ID_NOT_FOUND,
                "case {case:?}: expected the prefix list's not-found code, got {code:?} in \
                 {body}"
            );
            assert!(
                body.contains(prefix_list_id.as_str()),
                "case {case:?}: expected the refusal to name {prefix_list_id:?}, got {body}"
            );
            // The sentence EC2 words this class with, whose only variable part is the id the
            // request sent.
            let expected = format!("The prefix list ID '{prefix_list_id}' does not exist");
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );

            // Nothing was stored: the group holds no rule at all beyond the outbound rule
            // every new group comes with.
            let described = ok(
                &case,
                &router,
                "DescribeSecurityGroupRules",
                &[param("Filter.1.Name", "group-id"), param("Filter.1.Value.1", &group_id)],
            )
            .await;
            assert!(
                elements(&described, "prefixListId").is_empty(),
                "case {case:?}: expected no rule granting a prefix list, got {described}"
            );
        }
    }
}
