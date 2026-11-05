// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetManagedPrefixListAssociations`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, and the Region in that scope is the Region the service answers for, so
//! every call here carries one.
//!
//! The rules under test:
//!
//! 1. The operation reports THE RESOURCES THAT REFERENCE the one prefix list the request
//!    names, computed from what the service holds rather than from the request: a security
//!    group whose rule grants the prefix list, a route table holding a route whose destination
//!    is the prefix list, and a transit gateway route table holding a prefix list reference to
//!    it. Each row carries the referencing resource's id and the account that owns THAT
//!    resource — the owner its own describe reports — and a resource is reported ONCE however
//!    many references it holds. The table's rows differ along what that generalises over: the
//!    kind of referencing resource, the direction of a security group rule, how many resources
//!    reference the list, and whether the list is one the account created or one the Region
//!    publishes.
//! 2. A reference to ANOTHER prefix list is not an association of this one, and a list nothing
//!    references reports an empty set rather than a refusal.
//! 3. A page is `MaxResults` resources long and carries a `NextToken` only while a resource
//!    follows it, so walking the tokens yields every referencing resource exactly once in that
//!    one order, whatever page size is asked for.
//! 4. A request this operation cannot serve is REFUSED with EC2's code for that class and
//!    reports no association: an id that is not a prefix list id, an id nothing the caller can
//!    see carries, a pagination token this service did not mint or minted for another list,
//!    and a page size outside the range the model publishes.
//! 5. Reporting associations is a READ: a `DryRun` answers 412 `DryRunOperation`, and asking
//!    twice — before and after a dry run — reports the same associations, with the referencing
//!    resources left exactly as they were.

use ec2::state::PREFIX_LIST_ADDRESS_FAMILY_IPV4;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, member as envelope_member};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

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

/// One element of a response body, or a panic naming the case.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// One prefix list of the account, carrying one entry, as the id the create minted.
async fn create_prefix_list(case: &str, router: &MultiServiceRouter, name: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateManagedPrefixList",
        &[
            param("PrefixListName", name),
            param("AddressFamily", PREFIX_LIST_ADDRESS_FAMILY_IPV4),
            param("MaxEntries", "4"),
            param("Entry.1.Cidr", "10.0.0.0/8"),
        ],
    )
    .await;
    member(case, &body, "prefixListId")
}

/// The owner the resource's OWN describe reports: the element carrying `id_element` is the
/// resource, and `ownerId` beside it is the account that owns it.
///
/// Read off the document tree rather than by substring, so an `ownerId` nested inside a
/// referenced group or an attachment is never read as the resource's own.
fn owner_beside(case: &str, body: &str, id_element: &str) -> String {
    let parsed = document(case, body);
    let resource = parsed
        .descendants()
        .find(|node| node.is_element() && child_text(*node, id_element).is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no element carrying <{id_element}> in {body}"));
    child_text(resource, "ownerId").unwrap_or_else(|| {
        panic!("case {case:?}: no <ownerId> beside <{id_element}> in {body}")
    })
}

/// One kind of reference a resource can hold to a prefix list — what one row of the table
/// varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// An INBOUND security group rule whose grant names the prefix list.
    Ingress,
    /// An OUTBOUND security group rule whose grant names the prefix list.
    Egress,
    /// One group granting the prefix list in BOTH directions: two rules, one resource.
    BothDirections,
    /// A route of a VPC route table whose destination is the prefix list.
    Route,
    /// A prefix list reference of a transit gateway route table.
    TransitGatewayRoute,
}

/// Grant `prefix_list_id` on `group_id` through `action`, the authorize of one direction.
async fn grant(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    group_id: &str,
    prefix_list_id: &str,
) {
    ok(
        case,
        router,
        action,
        &[
            param("GroupId", group_id),
            param("IpPermissions.1.IpProtocol", "tcp"),
            param("IpPermissions.1.FromPort", "443"),
            param("IpPermissions.1.ToPort", "443"),
            param("IpPermissions.1.PrefixListIds.1.PrefixListId", prefix_list_id),
        ],
    )
    .await;
}

/// Give `prefix_list_id` one more referencing resource of the kind `reference` names, and
/// answer that resource's id and the owner ITS OWN describe reports for it.
///
/// `index` keeps the names and address blocks of the resources one case creates apart.
async fn add_reference(
    case: &str,
    router: &MultiServiceRouter,
    index: usize,
    prefix_list_id: &str,
    reference: Reference,
) -> (String, String) {
    match reference {
        Reference::Ingress | Reference::Egress | Reference::BothDirections => {
            let body = ok(
                case,
                router,
                "CreateSecurityGroup",
                &[
                    param("GroupName", &format!("association-{index}")),
                    param("GroupDescription", "a group that references a prefix list"),
                ],
            )
            .await;
            let group_id = member(case, &body, "groupId");
            let actions: &[&str] = match reference {
                Reference::Ingress => &["AuthorizeSecurityGroupIngress"],
                Reference::Egress => &["AuthorizeSecurityGroupEgress"],
                _ => &[
                    "AuthorizeSecurityGroupIngress",
                    "AuthorizeSecurityGroupEgress",
                ],
            };
            for action in actions {
                grant(case, router, action, &group_id, prefix_list_id).await;
            }
            let described = ok(
                case,
                router,
                "DescribeSecurityGroups",
                &[param("GroupId.1", &group_id)],
            )
            .await;
            let owner = owner_beside(case, &described, "groupId");
            (group_id, owner)
        }
        Reference::Route => {
            // A route points one destination at one target of its table's own VPC, so the
            // table gets a VPC and an internet gateway attached to it before the route.
            let vpc = ok(
                case,
                router,
                "CreateVpc",
                // One block per resource of the case, inside the one octet a `/16` of
                // `10.0.0.0/8` varies in.
                &[param("CidrBlock", &format!("10.{}.0.0/16", index % 256))],
            )
            .await;
            let vpc_id = member(case, &vpc, "vpcId");
            let gateway = ok(case, router, "CreateInternetGateway", &[]).await;
            let internet_gateway_id = member(case, &gateway, "internetGatewayId");
            ok(
                case,
                router,
                "AttachInternetGateway",
                &[
                    param("InternetGatewayId", &internet_gateway_id),
                    param("VpcId", &vpc_id),
                ],
            )
            .await;
            let table = ok(case, router, "CreateRouteTable", &[param("VpcId", &vpc_id)]).await;
            let route_table_id = member(case, &table, "routeTableId");
            ok(
                case,
                router,
                "CreateRoute",
                &[
                    param("RouteTableId", &route_table_id),
                    param("DestinationPrefixListId", prefix_list_id),
                    param("GatewayId", &internet_gateway_id),
                ],
            )
            .await;
            let described = ok(
                case,
                router,
                "DescribeRouteTables",
                &[param("RouteTableId.1", &route_table_id)],
            )
            .await;
            let owner = owner_beside(case, &described, "routeTableId");
            (route_table_id, owner)
        }
        Reference::TransitGatewayRoute => {
            // "We create a default transit gateway route table and use it as the default
            // association route table" (`CreateTransitGateway`), which is the table the
            // reference goes in.
            let gateway = ok(case, router, "CreateTransitGateway", &[]).await;
            let route_table_id = member(case, &gateway, "associationDefaultRouteTableId");
            let transit_gateway_id = member(case, &gateway, "transitGatewayId");
            ok(
                case,
                router,
                "CreateTransitGatewayPrefixListReference",
                &[
                    param("TransitGatewayRouteTableId", &route_table_id),
                    param("PrefixListId", prefix_list_id),
                    param("Blackhole", "true"),
                ],
            )
            .await;
            // A transit gateway route table carries no owner of its own, so the owner of the
            // GATEWAY it belongs to is what a row for it must carry — read off a describe
            // narrowed to THAT gateway, so the owner asserted is the owner of this table's
            // gateway rather than of whichever gateway the store reports first.
            let described = ok(
                case,
                router,
                "DescribeTransitGateways",
                &[param("TransitGatewayIds.1", &transit_gateway_id)],
            )
            .await;
            let owner = owner_beside(case, &described, "transitGatewayId");
            (route_table_id, owner)
        }
    }
}

/// The associations a successful answer reported, as `(resourceId, resourceOwner)` in the
/// order it reported them.
///
/// Read off the document tree rather than by substring, so an owner is paired with the
/// resource it belongs to rather than with the row at the same position in a flat list.
fn reported_associations(case: &str, body: &str) -> Vec<(String, Option<String>)> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "prefixListAssociationSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "resourceId").unwrap_or_default(),
                child_text(item, "resourceOwner"),
            )
        })
        .collect()
}

/// The associations a successful answer reported, having first pinned the status.
async fn associations_of(
    case: &str,
    router: &MultiServiceRouter,
    params: &[Param],
) -> Vec<(String, Option<String>)> {
    let body = ok(case, router, "GetManagedPrefixListAssociations", params).await;
    reported_associations(case, &body)
}

/// The rows expected of a report, in the ascending resource-id order it answers in.
fn expected_rows(resources: &[(String, String)]) -> Vec<(String, Option<String>)> {
    let mut rows: Vec<(String, Option<String>)> = resources
        .iter()
        .map(|(resource_id, owner)| (resource_id.clone(), Some(owner.clone())))
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

/// One row of the association table: the case name and the references the list is given, one
/// resource per entry.
struct Case {
    name: &'static str,
    references: Vec<Reference>,
}

/// The rows the table is written against: no reference at all, each kind of referencing
/// resource on its own, one group referencing in both directions (two rules, one row), and
/// every kind together.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a prefix list nothing references",
            references: Vec::new(),
        },
        Case {
            name: "one inbound security group rule",
            references: vec![Reference::Ingress],
        },
        Case {
            name: "one outbound security group rule",
            references: vec![Reference::Egress],
        },
        Case {
            name: "one group granting the list in both directions",
            references: vec![Reference::BothDirections],
        },
        Case {
            name: "one route of a VPC route table",
            references: vec![Reference::Route],
        },
        Case {
            name: "one transit gateway route table reference",
            references: vec![Reference::TransitGatewayRoute],
        },
        Case {
            name: "two security groups and a route table",
            references: vec![Reference::Ingress, Reference::Egress, Reference::Route],
        },
        Case {
            name: "every kind of reference at once",
            references: vec![
                Reference::BothDirections,
                Reference::Route,
                Reference::TransitGatewayRoute,
            ],
        },
    ]
}

/// Rules 1, 2 and 5: the reported set is exactly the resources that reference the list, each
/// with its own owner, one row per resource; a reference to another list is not one of them;
/// and reading twice reports the same set.
#[tokio::test]
async fn the_reported_associations_are_the_resources_that_reference_the_list() {
    for case in cases() {
        let (router, _engine) = super::ec2_query::emulator();
        let name = case.name;
        let prefix_list_id = create_prefix_list(name, &router, "the-list-under-test").await;

        // A second list, referenced by every kind of resource: nothing it holds may appear in
        // the report of the list under test.
        let decoy = create_prefix_list(name, &router, "another-list").await;
        let mut decoy_resources = Vec::new();
        for (index, reference) in [
            Reference::BothDirections,
            Reference::Route,
            Reference::TransitGatewayRoute,
        ]
        .into_iter()
        .enumerate()
        {
            decoy_resources
                .push(add_reference(name, &router, 900 + index, &decoy, reference).await);
        }

        let mut expected = Vec::new();
        for (index, reference) in case.references.iter().copied().enumerate() {
            expected.push(add_reference(name, &router, index, &prefix_list_id, reference).await);
        }

        let params = vec![param("PrefixListId", &prefix_list_id)];
        let reported = associations_of(name, &router, &params).await;
        assert_eq!(
            reported,
            expected_rows(&expected),
            "case {name:?}: expected one row per referencing resource in resource-id order, \
             got {reported:?}"
        );
        for (resource_id, _) in &decoy_resources {
            assert!(
                !reported.iter().any(|(reported_id, _)| reported_id == resource_id),
                "case {name:?}: {resource_id} references another prefix list and must not be \
                 reported, got {reported:?}"
            );
        }

        // A read: asking again reports exactly the same set.
        let again = associations_of(name, &router, &params).await;
        assert_eq!(
            again, reported,
            "case {name:?}: a second read must report the same associations, got {again:?} \
             after {reported:?}"
        );

        // Every row of the case fits one page, so the answer names no further page.
        let body = ok(name, &router, "GetManagedPrefixListAssociations", &params).await;
        assert_eq!(
            envelope_member(&body, "nextToken"),
            None,
            "case {name:?}: a whole association set in one page must carry no nextToken, got \
             {body}"
        );

        // The decoy's own report is its own resources, so the two reports are of two lists
        // rather than of the store.
        let decoy_reported = associations_of(name, &router, &[param("PrefixListId", &decoy)]).await;
        assert_eq!(
            decoy_reported,
            expected_rows(&decoy_resources),
            "case {name:?}: expected the other list's own referencing resources, got \
             {decoy_reported:?}"
        );
    }
}

/// Rule 1: the same report serves a prefix list the Region PUBLISHES for one of its services —
/// a security group rule may grant one, and the published list is then reported as referenced
/// by that group.
#[tokio::test]
async fn a_published_prefix_list_reports_the_resources_that_reference_it() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "a published prefix list";

    // The Region's published lists, as `DescribePrefixLists` reports them — the operation that
    // owns that derivation, so this suite does not spell a catalog of its own.
    let published = ok(CASE, &router, "DescribePrefixLists", &[]).await;
    let published_id = member(CASE, &published, "prefixListId");

    let before = associations_of(CASE, &router, &[param("PrefixListId", &published_id)]).await;
    assert!(
        before.is_empty(),
        "case {CASE:?}: nothing references {published_id} yet, got {before:?}"
    );

    let (group_id, owner) =
        add_reference(CASE, &router, 1, &published_id, Reference::Ingress).await;
    let reported = associations_of(CASE, &router, &[param("PrefixListId", &published_id)]).await;
    assert_eq!(
        reported,
        vec![(group_id.clone(), Some(owner))],
        "case {CASE:?}: expected the group that grants {published_id} to be reported, got \
         {reported:?}"
    );
}

/// The associations a walk of the whole report yields with `max_results` as its page size, and
/// how many calls the walk took.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    prefix_list_id: &str,
    max_results: Option<usize>,
) -> (Vec<(String, Option<String>)>, usize) {
    let mut collected: Vec<(String, Option<String>)> = Vec::new();
    let mut token: Option<String> = None;
    let mut calls = 0usize;
    loop {
        let mut params = vec![param("PrefixListId", prefix_list_id)];
        if let Some(limit) = max_results {
            params.push(param("MaxResults", &limit.to_string()));
        }
        if let Some(carried) = &token {
            params.push(param("NextToken", carried));
        }
        let (status, body) = call(router, "GetManagedPrefixListAssociations", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {calls} expected HTTP 200, got {status} with body {body}"
        );
        calls += 1;
        let page = reported_associations(case, &body);
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit,
                "case {case:?}: page {calls} carries {} rows, above the page size of {limit}: \
                 {body}",
                page.len()
            );
        }
        collected.extend(page);
        token = envelope_member(&body, "nextToken");
        if token.is_none() {
            return (collected, calls);
        }
        assert!(
            calls < 32,
            "case {case:?}: the walk did not end after {calls} pages"
        );
    }
}

/// Rule 3: a page is at most `MaxResults` rows long and names a further page only while one
/// follows, so a walk yields every referencing resource once, in the one order, whatever page
/// size it asks for.
#[tokio::test]
async fn paging_yields_every_association_once_whatever_the_page_size() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "paging";

    let prefix_list_id = create_prefix_list(CASE, &router, "paged-list").await;
    // More referencing resources than the smallest page the model admits, so a page size at
    // that bound takes more than one call.
    let mut resources = Vec::new();
    for index in 0..7 {
        resources.push(add_reference(CASE, &router, index, &prefix_list_id, Reference::Ingress).await);
    }
    let whole = associations_of(CASE, &router, &[param("PrefixListId", &prefix_list_id)]).await;
    assert_eq!(
        whole,
        expected_rows(&resources),
        "case {CASE:?}: expected every referencing group in one answer, got {whole:?}"
    );

    // The page sizes the model's `GetManagedPrefixListAssociationsMaxResults` range admits: at
    // both published bounds (5 and 255), inside it, exactly the number of rows, and no page
    // size at all.
    for max_results in [None, Some(5), Some(6), Some(7), Some(100), Some(255)] {
        let case = format!("{CASE} with MaxResults {max_results:?}");
        let (walked, calls) = walk_pages(&router, &case, &prefix_list_id, max_results).await;
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the whole association set in the one order, got {walked:?}"
        );
        // A page size of N takes ceil(len / N) calls, and the last of them carries no token:
        // the walk above only returns once a page named no further page.
        let expected_calls = match max_results {
            None => 1,
            Some(limit) => whole.len().div_ceil(limit),
        };
        assert_eq!(
            calls, expected_calls,
            "case {case:?}: expected {expected_calls} call(s) to walk {} rows, took {calls}",
            whole.len()
        );
    }

    // A page size outside the range the model publishes (5..255) is refused rather than served
    // as the nearest page size inside it.
    for max_results in ["4", "256"] {
        let case = format!("{CASE} with MaxResults {max_results}");
        let (status, body) = call(
            &router,
            "GetManagedPrefixListAssociations",
            &[
                param("PrefixListId", &prefix_list_id),
                param("MaxResults", max_results),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
}

/// Rule 4: a pagination token is only good for the report that minted it — a token this
/// service did not mint, and one minted while paging another prefix list, are both refused.
#[tokio::test]
async fn a_token_is_only_good_for_the_list_it_was_minted_for() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "a token minted for another list";

    let first = create_prefix_list(CASE, &router, "token-source").await;
    let second = create_prefix_list(CASE, &router, "token-target").await;
    for index in 0..6 {
        add_reference(CASE, &router, index, &first, Reference::Ingress).await;
        add_reference(CASE, &router, 100 + index, &second, Reference::Ingress).await;
    }

    let body = ok(
        CASE,
        &router,
        "GetManagedPrefixListAssociations",
        &[param("PrefixListId", &first), param("MaxResults", "5")],
    )
    .await;
    let token = envelope_member(&body, "nextToken")
        .unwrap_or_else(|| panic!("case {CASE:?}: the first page must name a next page: {body}"));

    for (case, prefix_list_id, carried) in [
        (
            "a token this service did not mint",
            first.as_str(),
            "bm90LWEtdG9rZW4=".to_string(),
        ),
        (
            "a token minted while paging another list",
            second.as_str(),
            token.clone(),
        ),
    ] {
        let (status, body) = call(
            &router,
            "GetManagedPrefixListAssociations",
            &[
                param("PrefixListId", prefix_list_id),
                param("NextToken", &carried),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected the pagination-token code, got {body}"
        );
    }
}

/// One row of the refusal table: the case, the fields it sends, the status and the code.
type RefusalCase = (String, Vec<Param>, u16, &'static str);

/// Rules 4 and 5: a request this operation cannot serve is refused with EC2's code for that
/// class, naming the value it refused, and reports no association — and the refusals leave the
/// report of the list exactly as it was.
#[tokio::test]
async fn a_request_outside_what_the_operation_serves_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    const CASE: &str = "refusals";

    let prefix_list_id = create_prefix_list(CASE, &router, "refusals").await;
    let referencing = add_reference(CASE, &router, 1, &prefix_list_id, Reference::Route).await;
    let before = associations_of(CASE, &router, &[param("PrefixListId", &prefix_list_id)]).await;

    let rows: Vec<RefusalCase> = vec![
        (
            "a value that is no resource id at all".to_string(),
            vec![param("PrefixListId", "the-office-list")],
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "an id of another kind of resource".to_string(),
            vec![param("PrefixListId", "vpc-0123456789abcdef0")],
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "a prefix list id whose suffix is not hexadecimal".to_string(),
            vec![param("PrefixListId", "pl-zzzzzzzz")],
            400,
            "InvalidPrefixListId.Malformed",
        ),
        (
            "a well-formed short id nothing the caller can see carries".to_string(),
            vec![param("PrefixListId", "pl-0a1b2c3d")],
            400,
            "InvalidPrefixListID.NotFound",
        ),
        (
            "a well-formed long id nothing the caller can see carries".to_string(),
            vec![param("PrefixListId", "pl-00112233445566778")],
            400,
            "InvalidPrefixListID.NotFound",
        ),
        (
            "a dry run".to_string(),
            vec![
                param("PrefixListId", &prefix_list_id),
                param("DryRun", "true"),
            ],
            DRY_RUN_STATUS,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in &rows {
        let (status, body) = call(&router, "GetManagedPrefixListAssociations", params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected the code {expected_code}, got {body}"
        );
        assert!(
            reported_associations(case, &body).is_empty(),
            "case {case:?}: a refusal must report no association, got {body}"
        );
        // The message is built from the REQUEST: the prefix list id it refused is the one the
        // request sent.
        if *expected_status == 400 {
            for (_, refused) in params.iter().filter(|(name, _)| name == "PrefixListId") {
                assert!(
                    error_message(&body).contains(refused.as_str()),
                    "case {case:?}: expected the message to name {refused:?}, got {body}"
                );
            }
        }
    }

    // Rule 5: nothing the refusals touched moved — the report is the same set it was, and the
    // referencing resource still holds its route.
    let after = associations_of(CASE, &router, &[param("PrefixListId", &prefix_list_id)]).await;
    assert_eq!(
        after, before,
        "case {CASE:?}: expected the same associations after the refusals, got {after:?} after \
         {before:?}"
    );
    assert_eq!(
        after,
        expected_rows(std::slice::from_ref(&referencing)),
        "case {CASE:?}: expected the one referencing route table, got {after:?}"
    );
}
