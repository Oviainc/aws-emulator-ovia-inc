// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeIpamResourceDiscoveryAssociations`, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * a describe REPORTS the resource discovery associations the store holds, every member as
//!   the write that minted them stored it — which for a default association is `CreateIpam`
//!   ("when you create an IPAM, a default resource discovery is created for your IPAM and it's
//!   associated with your IPAM", `IpamResourceDiscoveryAssociation$IsDefault`) and for any
//!   other `AssociateIpamResourceDiscovery` — narrowed to the ids the request names and to
//!   every filter it carries (joined with AND, each filter's own values with OR);
//! * a filter name the operation does not resolve is refused rather than read as "no results",
//!   an id that is not an association id at all is refused with its `Malformed` code and a
//!   well-formed one the account holds no association for with its `NotFound` code, and a
//!   `DryRun` reports the permission outcome without reading;
//! * the report is paged in a stable `IpamResourceDiscoveryAssociationId` order: at most
//!   `MaxResults` items with a `NextToken` naming where the next page starts, and no token once
//!   "there are no more results to return"
//!   (`DescribeIpamResourceDiscoveryAssociationsResult$NextToken`);
//! * the read is a REPORT: it applies only the state move the clock already owed the
//!   association (`associate-in-progress` -> `associate-complete`) and changes nothing else a
//!   later caller sees.
//!
//! Each table row is a distinct input along what those generalise over: whether the association
//! is the IPAM's default one or one a caller made, how many tags it carries, which KIND of
//! member a filter compares (an id, an ARN, a Region, an account, a boolean, an enum, a tag),
//! how many values that filter names, how many ids the request names, how big a page it asks
//! for, and which member of a refused request is at fault.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::ops::describe_ipam_resource_discovery_associations::{
    INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_MALFORMED,
    INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_NOT_FOUND,
    IPAM_RESOURCE_DISCOVERY_STATUS_NOT_FOUND,
};
use ec2::state::{
    INVALID_PAGINATION_TOKEN, INVALID_PARAMETER_VALUE,
    IPAM_ASSOCIATION_STATE_ASSOCIATE_COMPLETE, IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS,
    IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX, IPAM_RESOURCE_DISCOVERY_STATUS_ACTIVE,
    IPAM_STATE_CREATE_COMPLETE, RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region and the
/// Region every association of it reports.
const REGION: &str = "us-east-1";

/// The action under test.
const ACTION: &str = "DescribeIpamResourceDiscoveryAssociations";

/// One row of the joined-filter table: the case, the filter entries the request carries, and
/// the association ids they select together.
type JoinedFilterCase<'row> = (&'row str, Vec<(&'row str, Vec<&'row str>)>, Vec<&'row str>);

/// Longer than the span the clock owes an `associate-in-progress` association, so a read after
/// this much has certainly passed the due instant.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `DescribeIpamResourceDiscoveryAssociations` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    ok(case, router, ACTION, params).await
}

/// The IPAM a row creates through `CreateIpam`: what a request names it by, the ARN every
/// association of it reports, and the default resource discovery and association it came up
/// with.
struct CreatedIpam {
    /// The `IpamId` an associate request names.
    id: String,
    /// "The IPAM ARN" (`IpamResourceDiscoveryAssociation$IpamArn`).
    arn: String,
    /// The resource discovery `CreateIpam` created for the IPAM.
    default_discovery: String,
    /// The association `CreateIpam` made between the two above.
    default_association: String,
}

/// Create one IPAM through the operation that owns it, and answer what it is named by.
async fn create_ipam(case: &str, router: &MultiServiceRouter, params: &[Param]) -> CreatedIpam {
    let body = ok(case, router, "CreateIpam", params).await;
    let parsed = document(case, &body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    let read = |name: &str| -> String {
        child_text(ipam, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the IPAM in {body}"))
    };
    CreatedIpam {
        id: read("ipamId"),
        arn: read("ipamArn"),
        default_discovery: read("defaultResourceDiscoveryId"),
        default_association: read("defaultResourceDiscoveryAssociationId"),
    }
}

/// The form fields for one `ipam-resource-discovery-association` tag specification.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param(
        "TagSpecification.1.ResourceType",
        RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION,
    )];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// Associate `discovery_id` with `ipam_id` through the operation that owns that link, and
/// answer the `IpamResourceDiscoveryAssociationId` it minted.
async fn associate(
    case: &str,
    router: &MultiServiceRouter,
    ipam_id: &str,
    discovery_id: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut params = vec![
        param("IpamId", ipam_id),
        param("IpamResourceDiscoveryId", discovery_id),
    ];
    params.extend(tag_params(tags));
    let body = ok(case, router, "AssociateIpamResourceDiscovery", &params).await;
    let parsed = document(case, &body);
    let node = parsed
        .descendants()
        .find(|node| {
            node.is_element() && child(*node, "ipamResourceDiscoveryAssociationId").is_some()
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: no association in {body}")
        });
    child_text(node, "ipamResourceDiscoveryAssociationId")
        .unwrap_or_else(|| panic!("case {case:?}: no association id in {body}"))
}

/// One resource discovery association as a response reports it: every member read off the wire,
/// with the tag list flattened into comparable values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedAssociation {
    owner_id: Option<String>,
    association_id: Option<String>,
    association_arn: Option<String>,
    discovery_id: Option<String>,
    ipam_id: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    is_default: Option<String>,
    discovery_status: Option<String>,
    state: Option<String>,
    /// The `tagSet`, sorted: a tag set has no order of its own, but which tags it holds is part
    /// of the rule.
    tags: Vec<(String, String)>,
}

/// Every association the envelope's `ipamResourceDiscoveryAssociationSet` carries, in the order
/// the response reported them.
fn reported(case: &str, body: &str) -> Vec<ReportedAssociation> {
    let parsed = document(case, body);
    set_items(
        parsed.root_element(),
        "ipamResourceDiscoveryAssociationSet",
    )
    .into_iter()
    .map(|item| {
        let mut tags: Vec<(String, String)> = set_items(item, "tagSet")
            .into_iter()
            .map(|tag| {
                (
                    child_text(tag, "key").unwrap_or_default(),
                    child_text(tag, "value").unwrap_or_default(),
                )
            })
            .collect();
        tags.sort();
        ReportedAssociation {
            owner_id: child_text(item, "ownerId"),
            association_id: child_text(item, "ipamResourceDiscoveryAssociationId"),
            association_arn: child_text(item, "ipamResourceDiscoveryAssociationArn"),
            discovery_id: child_text(item, "ipamResourceDiscoveryId"),
            ipam_id: child_text(item, "ipamId"),
            ipam_arn: child_text(item, "ipamArn"),
            ipam_region: child_text(item, "ipamRegion"),
            is_default: child_text(item, "isDefault"),
            discovery_status: child_text(item, "resourceDiscoveryStatus"),
            state: child_text(item, "state"),
            tags,
        }
    })
    .collect()
}

/// The one association a response narrowed to a single id carries.
fn reported_one(case: &str, body: &str) -> ReportedAssociation {
    let all = reported(case, body);
    assert_eq!(
        all.len(),
        1,
        "case {case:?}: expected exactly one association, got {body}"
    );
    all.into_iter().next().expect("one association")
}

/// The association ids a response reported, as a set — what a narrowing assertion compares.
fn reported_ids(case: &str, body: &str) -> BTreeSet<String> {
    reported(case, body)
        .into_iter()
        .filter_map(|association| association.association_id)
        .collect()
}

/// The ARN of the association `association_id` names, as the store derives it from that id.
fn association_arn(association_id: &str) -> String {
    engine::emulator_arn(
        "ec2",
        "",
        &format!("{RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY_ASSOCIATION}/{association_id}"),
    )
}

// ---------------------------------------------------------------------------
// What the describe reports
// ---------------------------------------------------------------------------

/// The rule: `DescribeIpamResourceDiscoveryAssociations` reports the associations the store
/// holds, every member as the write that minted them stored it — the account that owns the
/// discovery, the id, the ARN derived from that id, the two resources the association links,
/// the linked IPAM's own ARN and home Region, whether it is that IPAM's default association,
/// the lifecycle state the write left it in, and the tags the request named — plus the one
/// member a report DERIVES rather than reads off the record, the status of the discovery it
/// links, and nothing the store does not hold.
///
/// The rows differ along what that generalises over: whether the association is the one
/// `CreateIpam` came up with or one a caller asked for, and how many tags it carries (none, one,
/// three).
#[tokio::test]
async fn describe_reports_every_stored_association_as_the_write_stored_it() {
    let (router, _engine) = emulator();
    let account = engine::emulator_account_id().to_string();
    let mut every_id: BTreeSet<String> = BTreeSet::new();

    // The default association `CreateIpam` made: the IPAM's own discovery, linked to itself.
    // Read straight back, so that discovery's own creation has not completed and its results
    // cannot be read yet.
    let case = "the default association of a created IPAM";
    let first = create_ipam(case, &router, &[]).await;
    every_id.insert(first.default_association.clone());
    let body = describe_ok(
        case,
        &router,
        &[param(
            "IpamResourceDiscoveryAssociationId.1",
            &first.default_association,
        )],
    )
    .await;
    assert!(
        is_long_id(
            IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX,
            &first.default_association
        ),
        "case {case:?}: expected an id of the form \
         {IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX}-<17 hex digits>, got {:?}",
        first.default_association
    );
    let expected = ReportedAssociation {
        owner_id: Some(account.clone()),
        association_id: Some(first.default_association.clone()),
        association_arn: Some(association_arn(&first.default_association)),
        discovery_id: Some(first.default_discovery.clone()),
        ipam_id: Some(first.id.clone()),
        ipam_arn: Some(first.arn.clone()),
        ipam_region: Some(REGION.to_string()),
        is_default: Some("true".to_string()),
        discovery_status: Some(IPAM_RESOURCE_DISCOVERY_STATUS_NOT_FOUND.to_string()),
        state: Some(IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS.to_string()),
        tags: Vec::new(),
    };
    assert_eq!(
        reported_one(case, &body),
        expected,
        "case {case:?}: expected the association {expected:?}, got {body}"
    );

    // (case, the tags the associate names)
    let rows: [(&str, Vec<(&str, &str)>); 3] = [
        ("an association with no tag at all", Vec::new()),
        (
            "an association with one tag",
            vec![("Name", "discovery-alpha")],
        ),
        (
            "an association with three tags",
            vec![("Name", "discovery-beta"), ("Team", "network"), ("Env", "prod")],
        ),
    ];
    for (case, tags) in &rows {
        // One IPAM-and-discovery pair holds at most one association, so each row links a
        // discovery and an IPAM of its own: the first IPAM's own default discovery, associated
        // with a second IPAM, both created by this row so the read below is a read straight
        // back.
        let source = create_ipam(case, &router, &[]).await;
        let target = create_ipam(case, &router, &[]).await;
        every_id.insert(source.default_association.clone());
        every_id.insert(target.default_association.clone());
        let association_id =
            associate(case, &router, &target.id, &source.default_discovery, tags).await;
        every_id.insert(association_id.clone());

        let body = describe_ok(
            case,
            &router,
            &[param("IpamResourceDiscoveryAssociationId.1", &association_id)],
        )
        .await;
        let mut expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected_tags.sort();
        let expected = ReportedAssociation {
            owner_id: Some(account.clone()),
            association_id: Some(association_id.clone()),
            association_arn: Some(association_arn(&association_id)),
            discovery_id: Some(source.default_discovery.clone()),
            ipam_id: Some(target.id.clone()),
            ipam_arn: Some(target.arn.clone()),
            ipam_region: Some(REGION.to_string()),
            // Only the association an IPAM came up with is that IPAM's default one.
            is_default: Some("false".to_string()),
            discovery_status: Some(IPAM_RESOURCE_DISCOVERY_STATUS_NOT_FOUND.to_string()),
            state: Some(IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS.to_string()),
            tags: expected_tags,
        };
        assert_eq!(
            reported_one(case, &body),
            expected,
            "case {case:?}: expected the association {expected:?}, got {body}"
        );
    }

    // Every association the writes minted, and nothing else, with no narrowing at all.
    let body = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        reported_ids("the whole store", &body),
        every_id,
        "the describe reports the associations that were made and nothing else, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Narrowing
// ---------------------------------------------------------------------------

/// The rule: a describe narrows its report to the ids the request names and to every filter it
/// carries — the filters joined with an AND, each filter's own values with an OR, each value
/// matched with EC2's wildcards against the member the filter name selects.
///
/// The rows differ along the KIND of member compared (its own id and ARN, the linked IPAM's id,
/// ARN and Region, the linked discovery's id, the owning account, a boolean, an enum, a tag),
/// how many values the filter names, and whether the value is exact or a wildcard.
#[tokio::test]
async fn describe_narrows_by_ids_and_by_every_filter() {
    let (router, _engine) = emulator();
    let account = engine::emulator_account_id().to_string();

    let source = create_ipam("the source IPAM", &router, &[]).await;
    let target = create_ipam("the target IPAM", &router, &[]).await;
    // Three associations: each IPAM's own default one, and one a caller made between the two,
    // which is the only one carrying tags.
    let made = associate(
        "the caller-made association",
        &router,
        &target.id,
        &source.default_discovery,
        &[("Name", "shared"), ("Team", "network")],
    )
    .await;
    let every: BTreeSet<String> = [
        source.default_association.clone(),
        target.default_association.clone(),
        made.clone(),
    ]
    .into_iter()
    .collect();
    // Past every due instant, so the lifecycle state and the discovery status the filters below
    // compare are the settled ones and stay settled for the whole table.
    tokio::time::sleep(past_the_transition()).await;

    // The ids the request names, which narrow the report without any filter at all.
    let by_ids: [(&str, Vec<&str>, Vec<&str>); 3] = [
        ("one id", vec![&made], vec![&made]),
        (
            "two ids",
            vec![&source.default_association, &made],
            vec![&source.default_association, &made],
        ),
        ("the same id twice", vec![&made, &made], vec![&made]),
    ];
    for (case, ids, expected) in &by_ids {
        let params: Vec<Param> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                param(
                    &format!("IpamResourceDiscoveryAssociationId.{}", index + 1),
                    id,
                )
            })
            .collect();
        let body = describe_ok(case, &router, &params).await;
        let wanted: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids(case, &body),
            wanted,
            "case {case:?}: expected the ids to select {wanted:?}, got {body}"
        );
    }

    let made_arn = association_arn(&made);
    // (case, filter name, filter values, the association ids it selects)
    let by_filter: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        (
            "by its own id",
            "ipam-resource-discovery-association-id",
            vec![&made],
            vec![&made],
        ),
        (
            "by two of its own ids, joined with OR",
            "ipam-resource-discovery-association-id",
            vec![&made, &target.default_association],
            vec![&made, &target.default_association],
        ),
        (
            "by an id no association has",
            "ipam-resource-discovery-association-id",
            vec!["ipam-res-disco-assoc-00000000000000000"],
            Vec::new(),
        ),
        (
            "by its own ARN",
            "ipam-resource-discovery-association-arn",
            vec![&made_arn],
            vec![&made],
        ),
        (
            "by the linked IPAM",
            "ipam-id",
            vec![&target.id],
            vec![&target.default_association, &made],
        ),
        (
            "by the linked IPAM's ARN",
            "ipam-arn",
            vec![&source.arn],
            vec![&source.default_association],
        ),
        (
            "by the IPAM home Region, which every association of this account shares",
            "ipam-region",
            vec![REGION],
            vec![&source.default_association, &target.default_association, &made],
        ),
        (
            "by a Region no IPAM is in",
            "ipam-region",
            vec!["eu-west-1"],
            Vec::new(),
        ),
        (
            "by the linked resource discovery, which two associations share",
            "ipam-resource-discovery-id",
            vec![&source.default_discovery],
            vec![&source.default_association, &made],
        ),
        (
            "by the owning account",
            "owner-id",
            vec![&account],
            vec![&source.default_association, &target.default_association, &made],
        ),
        (
            "by being an IPAM's default association",
            "is-default",
            vec!["true"],
            vec![&source.default_association, &target.default_association],
        ),
        (
            "by not being a default association",
            "is-default",
            vec!["false"],
            vec![&made],
        ),
        (
            // "`active` - Connection or permissions required to read the results of the resource
            // discovery are intact" (`$ResourceDiscoveryStatus`), which holds of every discovery
            // this store holds.
            "by the resource discovery status",
            "resource-discovery-status",
            vec![IPAM_RESOURCE_DISCOVERY_STATUS_ACTIVE],
            vec![&source.default_association, &target.default_association, &made],
        ),
        (
            "by a resource discovery status no association is in",
            "resource-discovery-status",
            vec!["not-found"],
            Vec::new(),
        ),
        (
            "by a lifecycle state every association being made is in",
            "state",
            vec!["associate-*"],
            vec![&source.default_association, &target.default_association, &made],
        ),
        (
            "by a state no association is in",
            "state",
            vec!["disassociate-complete"],
            Vec::new(),
        ),
        ("by a tag key", "tag-key", vec!["Team"], vec![&made]),
        (
            "by any tag key at all, which only a tagged association has",
            "tag-key",
            vec!["*"],
            vec![&made],
        ),
        ("by a tag value", "tag-value", vec!["shared"], vec![&made]),
        ("by a named tag", "tag:Name", vec!["shared"], vec![&made]),
        (
            "by a named tag whose value no association carries",
            "tag:Name",
            vec!["unshared"],
            Vec::new(),
        ),
    ];
    for (case, name, values, expected) in &by_filter {
        let value_refs: Vec<&str> = values.to_vec();
        let body = describe_ok(case, &router, &filters(&[(name, &value_refs)])).await;
        let wanted: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids(case, &body),
            wanted,
            "case {case:?}: expected the filter {name:?} over {values:?} to select {wanted:?}, \
             got {body}"
        );
    }

    // (case, filter entries, the ids they select together)
    let joined: Vec<JoinedFilterCase> = vec![
        (
            "two filters that both hold",
            vec![
                ("ipam-resource-discovery-id", vec![&source.default_discovery]),
                ("owner-id", vec![&account]),
            ],
            vec![&source.default_association, &made],
        ),
        (
            "two filters only one association satisfies",
            vec![
                ("ipam-resource-discovery-id", vec![&source.default_discovery]),
                ("is-default", vec!["false"]),
            ],
            vec![&made],
        ),
        (
            "two filters no association satisfies at once",
            vec![
                ("ipam-id", vec![&source.id]),
                ("is-default", vec!["false"]),
            ],
            Vec::new(),
        ),
        (
            "three filters that all hold",
            vec![
                ("owner-id", vec![&account]),
                ("ipam-region", vec![REGION]),
                ("ipam-resource-discovery-association-id", vec![&made]),
            ],
            vec![&made],
        ),
    ];
    for (case, entries, expected) in &joined {
        let fields: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let body = describe_ok(case, &router, &filters(&fields)).await;
        let wanted: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids(case, &body),
            wanted,
            "case {case:?}: expected the filters to select {wanted:?} with an AND, got {body}"
        );
    }

    // An id narrows what the filters would otherwise have selected: the two meet with an AND.
    let case = "an id beside a filter";
    let body = describe_ok(case, &router, &{
        let mut params = vec![param(
            "IpamResourceDiscoveryAssociationId.1",
            &source.default_association,
        )];
        params.extend(filters(&[("is-default", &["false"])]));
        params
    })
    .await;
    assert_eq!(
        reported_ids(case, &body),
        BTreeSet::new(),
        "case {case:?}: an id and a filter narrow together, got {body}"
    );

    // None of the narrowing changed what the store reports.
    let body = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        reported_ids("the whole store", &body),
        every,
        "a narrowed describe changes nothing, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// The rule: the report is paged in a stable `IpamResourceDiscoveryAssociationId` order — at
/// most `MaxResults` items, a `NextToken` naming where the next page starts, and no token once
/// there are no more results to return — so a walk of the pages reports every association
/// exactly once.
///
/// The rows are page sizes: the model's smallest (`IpamMaxResults` has a minimum of 5), one that
/// divides the store unevenly, and one at the store's size.
#[tokio::test]
async fn describe_pages_in_a_stable_id_order() {
    let (router, _engine) = emulator();
    let source = create_ipam("the source IPAM", &router, &[]).await;
    let mut created: BTreeSet<String> = BTreeSet::from([source.default_association.clone()]);
    // Three more IPAMs, each contributing its own default association and one caller-made
    // association with the source IPAM's discovery: seven records in all.
    for nth in 0..3 {
        let case = format!("IPAM {nth}");
        let target = create_ipam(&case, &router, &[]).await;
        created.insert(target.default_association.clone());
        created.insert(
            associate(&case, &router, &target.id, &source.default_discovery, &[]).await,
        );
    }
    assert_eq!(
        created.len(),
        7,
        "the store holds seven associations before paging, got {created:?}"
    );

    for page_size in [5usize, 6, 7] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = elements(&body, "ipamResourceDiscoveryAssociationId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} associations, got {body}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 4,
                "case {case:?}: the paging walk must terminate, got {seen:?}"
            );
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages come in IpamResourceDiscoveryAssociationId order, \
             got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "case {case:?}: paging reports every association exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            created.len(),
            "case {case:?}: no association is reported twice, got {seen:?}"
        );
    }

    // A page that holds everything left carries no token at all.
    let body = describe_ok("one page over the whole store", &router, &[]).await;
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "no token once there are no more results to return, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The rule: a request this operation's contract does not admit is refused with the code that
/// names what was wrong — this resource's `Malformed` code for a value that is not an
/// association id at all, its `NotFound` code for a well-formed id the account holds no
/// association for, `InvalidParameterValue` for a filter name the operation does not resolve or
/// for more of any request-controlled factor than it will do, `InvalidPaginationToken` for a
/// page token it did not mint, and `DryRunOperation` for a permission check, which reads
/// nothing.
#[tokio::test]
async fn describe_refuses_what_the_contract_does_not_admit() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the one IPAM the store holds", &router, &[]).await;
    let held = ipam.default_association.clone();

    let many_filters = |count: usize| -> Vec<Param> {
        (0..count)
            .flat_map(|index| {
                vec![
                    param(
                        &format!("Filter.{}.Name", index + 1),
                        "ipam-resource-discovery-association-id",
                    ),
                    param(&format!("Filter.{}.Value.1", index + 1), &held),
                ]
            })
            .collect()
    };
    let many_values = |count: usize| -> Vec<Param> {
        let mut params = vec![param(
            "Filter.1.Name",
            "ipam-resource-discovery-association-id",
        )];
        // Synthesised ids for every position but the last, which carries the id the store really
        // minted — so a list AT the cap exercises the cap and still selects the association.
        params.extend((0..count.saturating_sub(1)).map(|index| {
            param(
                &format!("Filter.1.Value.{}", index + 1),
                &format!(
                    "{IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_PREFIX}-{:017x}",
                    index + 1
                ),
            )
        }));
        params.push(param(&format!("Filter.1.Value.{count}"), &held));
        params
    };
    let many_ids = |count: usize| -> Vec<Param> {
        (0..count)
            .map(|index| {
                param(
                    &format!("IpamResourceDiscoveryAssociationId.{}", index + 1),
                    &held,
                )
            })
            .collect()
    };

    // (case, params, expected status, expected code). A refused request is a 400 except for the
    // permission check, which EC2 answers 412 `DryRunOperation`.
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "an id that is not an association id at all",
            vec![param(
                "IpamResourceDiscoveryAssociationId.1",
                "not-an-association-id",
            )],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_MALFORMED,
        ),
        (
            // A resource DISCOVERY id is an id of another resource, however much of the prefix it
            // shares.
            "an id of another resource of the same family",
            vec![param(
                "IpamResourceDiscoveryAssociationId.1",
                "ipam-res-disco-0123456789abcdef0",
            )],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_MALFORMED,
        ),
        (
            "a malformed id beside a well-formed one",
            vec![
                param("IpamResourceDiscoveryAssociationId.1", &held),
                param(
                    "IpamResourceDiscoveryAssociationId.2",
                    "ipam-res-disco-assoc-not-hexadecimal",
                ),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_MALFORMED,
        ),
        (
            "a well-formed id the account holds no association for",
            vec![param(
                "IpamResourceDiscoveryAssociationId.1",
                "ipam-res-disco-assoc-00000000000000000",
            )],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_NOT_FOUND,
        ),
        (
            "an unknown id beside a known one",
            vec![
                param("IpamResourceDiscoveryAssociationId.1", &held),
                param(
                    "IpamResourceDiscoveryAssociationId.2",
                    "ipam-res-disco-assoc-00000000000000000",
                ),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_NOT_FOUND,
        ),
        (
            "a filter name the operation does not resolve",
            vec![
                param("Filter.1.Name", "association-colour"),
                param("Filter.1.Value.1", "blue"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            // A filter of the DISCOVERY describe is not a filter of this one: the names are the
            // members of the resource THIS operation reports.
            "a filter name another describe of this family resolves",
            vec![
                param("Filter.1.Name", "ipam-resource-discovery-region"),
                param("Filter.1.Value.1", REGION),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            // "Filter names are case-sensitive" (`Filter$Name`).
            "a filter name spelled in another case",
            vec![
                param("Filter.1.Name", "Is-Default"),
                param("Filter.1.Value.1", "true"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag filter naming no key",
            vec![
                param("Filter.1.Name", "tag:"),
                param("Filter.1.Value.1", "anything"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a page token this service did not mint",
            vec![param("NextToken", "not-a-token-this-service-minted")],
            400,
            INVALID_PAGINATION_TOKEN,
        ),
        (
            "one filter past the cap",
            many_filters(ec2::state::MAX_FILTERS + 1),
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "one filter value past the cap",
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a filter value one byte past the cap",
            vec![
                param("Filter.1.Name", "ipam-resource-discovery-association-id"),
                param(
                    "Filter.1.Value.1",
                    &"d".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
                ),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "one id past the cap",
            many_ids(ec2::state::MAX_RESOURCE_IDS + 1),
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a permission check",
            vec![param("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            // The form of the ids is a fact about the REQUEST, so it is answered before the
            // permission check rather than after it.
            "a permission check beside a malformed id",
            vec![
                param("DryRun", "true"),
                param(
                    "IpamResourceDiscoveryAssociationId.1",
                    "not-an-association-id",
                ),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ASSOCIATION_ID_MALFORMED,
        ),
        (
            // …and so is a filter name, so a dry run that would have been refused for one is
            // refused with that reason instead of reporting that it would have succeeded.
            "a permission check beside an unresolvable filter",
            vec![
                param("DryRun", "true"),
                param("Filter.1.Name", "association-colour"),
                param("Filter.1.Value.1", "blue"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
    ];
    for (case, params, status, code) in &cases {
        let (answered, body) = invoke(&router, ACTION, params).await;
        assert_eq!(
            answered, *status,
            "case {case:?}: expected HTTP {status}, got {answered} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code}, got {body}"
        );
    }

    // A request AT each cap is served rather than refused, so the caps are not refusing requests
    // they should serve.
    let at_the_caps: [(&str, Vec<Param>); 3] = [
        ("filters at the cap", many_filters(ec2::state::MAX_FILTERS)),
        (
            "filter values at the cap",
            many_values(ec2::state::MAX_FILTER_VALUES),
        ),
        ("ids at the cap", many_ids(ec2::state::MAX_RESOURCE_IDS)),
    ];
    for (case, params) in &at_the_caps {
        let body = describe_ok(case, &router, params).await;
        assert_eq!(
            reported_ids(case, &body),
            [held.clone()].into_iter().collect::<BTreeSet<_>>(),
            "case {case:?}: a request at the cap still selects the association, got {body}"
        );
    }

    // None of the refusals changed what the store reports.
    let body = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        reported_ids("the whole store", &body),
        [held].into_iter().collect::<BTreeSet<_>>(),
        "a refused describe changes nothing, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The read is a report
// ---------------------------------------------------------------------------

/// The rule: a describe is a REPORT — it applies the state move the clock already owed the
/// association and nothing else, so a caller reading straight back still sees the transient
/// state, reading twice does not move it, and one reading after the due instant sees the settled
/// one; and the status it carries for the discovery that association links is derived at the
/// instant of the read, so it says the discovery's results can be read exactly when that
/// discovery's own creation has completed.
#[tokio::test]
async fn describe_reports_the_state_the_clock_owes() {
    let case = "the clock completes the association";
    let (router, _engine) = emulator();
    let source = create_ipam(case, &router, &[]).await;
    let target = create_ipam(case, &router, &[]).await;
    let association =
        associate(case, &router, &target.id, &source.default_discovery, &[]).await;
    let narrowed = [param("IpamResourceDiscoveryAssociationId.1", &association)];

    // Straight back, before the due instant: still being made, and the discovery it links is
    // still being created, so its results cannot be read. Reading it twice is still the
    // transient state — a read does not move it.
    for attempt in 0..2 {
        let body = describe_ok(case, &router, &narrowed).await;
        let reported = reported_one(case, &body);
        assert_eq!(
            reported.state.as_deref(),
            Some(IPAM_ASSOCIATION_STATE_ASSOCIATE_IN_PROGRESS),
            "case {case:?}: read {attempt} before the due instant sees the transient state, \
             got {body}"
        );
        assert_eq!(
            reported.discovery_status.as_deref(),
            Some(IPAM_RESOURCE_DISCOVERY_STATUS_NOT_FOUND),
            "case {case:?}: read {attempt} before the due instant reports no readable \
             discovery, got {body}"
        );
    }

    tokio::time::sleep(past_the_transition()).await;
    let settled = describe_ok(case, &router, &narrowed).await;
    let after = reported_one(case, &settled);
    assert_eq!(
        after.state.as_deref(),
        Some(IPAM_ASSOCIATION_STATE_ASSOCIATE_COMPLETE),
        "case {case:?}: a read past the due instant sees the settled state, got {settled}"
    );
    assert_eq!(
        after.discovery_status.as_deref(),
        Some(IPAM_RESOURCE_DISCOVERY_STATUS_ACTIVE),
        "case {case:?}: a read past the due instant reports the discovery's results as \
         readable, got {settled}"
    );
    // The status the report derives agrees with what the discovery's own describe says about it:
    // `active` exactly while that discovery is `create-complete`.
    let discoveries = ok(
        case,
        &router,
        "DescribeIpamResourceDiscoveries",
        &[param("IpamResourceDiscoveryId.1", &source.default_discovery)],
    )
    .await;
    assert_eq!(
        element(&discoveries, "state").as_deref(),
        Some(IPAM_STATE_CREATE_COMPLETE),
        "case {case:?}: the discovery an active association links is create-complete, \
         got {discoveries}"
    );

    // Nothing else about the association moved with the state, and a further read reports it
    // identically.
    let again = describe_ok(case, &router, &narrowed).await;
    assert_eq!(
        reported_one(case, &again),
        after,
        "case {case:?}: a second read past the due instant reports the same association, \
         got {again}"
    );
}
