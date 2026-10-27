// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE selection rule every list of this service applies to
//! its `*Filter` members, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rule, stated once: a filter is a LIST of ACCEPTED VALUES for the member it
//! names, so a record matches it when its value for that member is one of them; a
//! filter the caller did not send (or sent empty) narrows nothing; a record that
//! recorded no value at all for a filtered member matches none of the accepted
//! values, so a filter on that member narrows it out; and several filters narrow
//! rather than widen. The five list operations that take filters —
//! `ListOutposts`, `ListSites`, `ListAssets`, `ListAssetInstances` and
//! `ListCapacityTasks` — select by that one rule
//! (`state::filter_accepts`), so this suite drives it through EACH of them and
//! asserts the same rows of the same table against every one: whatever one list
//! answers for a filter, the others answer for theirs.
//!
//! The one form of a filter the wire cannot carry is the EMPTY list — a query
//! parameter always carries a value, and the length-constrained filters are modelled
//! `length(min: 1)` — so the "narrows nothing" arm for it is asserted against the
//! shared rule itself, in [`the_shared_rule_covers_the_filter_forms_no_query_string_can_carry`].
//!
//! Where the filtered records come from: hardware and instances are recorded on the
//! stored Outpost through the same store seams [`crate::outpost_assets`] and
//! [`crate::blocking_instances`] record them through, and a capacity task in a status
//! no implemented call produces is recorded the way
//! [`crate::capacity_task_listing`] records one — nothing in this API creates any of
//! them.

use std::collections::VecDeque;

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    ASSET_STATE_VALUES, ASSET_TYPE_VALUES, A_W_S_SERVICE_NAME_VALUES,
    CAPACITY_TASK_STATUS_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    filter_accepts, CapacityTaskRecord, CAPACITY_TASK_COMPLETED,
};

use crate::blocking_instances::{instance, instance_id, record as record_instances, OTHER_ACCOUNT};
use crate::outpost_assets::{asset, compute, record as record_assets};
use crate::site_and_outpost_lifecycle::{address, install_hardware, segment, text, Outposts};

/// What one list under test is reached and read by: the operation, the route the
/// model binds it to, the output member its page arrives in, and the member that
/// identifies one record of that page.
struct Listing {
    operation: &'static str,
    route: String,
    page_member: &'static str,
    id_member: &'static str,
}

/// One row of the table: the case it states, the query string its filters travel in,
/// and the identifiers the list must report, in the order it reports records.
struct Row {
    case: &'static str,
    query: String,
    expected: Vec<String>,
}

fn row(case: &'static str, query: String, expected: Vec<String>) -> Row {
    Row {
        case,
        query,
        expected,
    }
}

/// The identifiers one page reports, in the order it reported them.
fn ids(output: &Value, listing: &Listing, case: &str) -> Vec<String> {
    output
        .get(listing.page_member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} answers a {} list, got {output}",
                listing.operation, listing.page_member
            )
        })
        .iter()
        .map(|item| text(item, listing.id_member))
        .collect()
}

/// Drive every row of one list's table over the wire and require exactly the records
/// the row names, in order.
async fn assert_selects(outposts: &Outposts, listing: &Listing, rows: &[Row]) {
    for row in rows {
        let output = outposts
            .ok("GET", &format!("{}{}", listing.route, row.query), None)
            .await;
        let listed = ids(&output, listing, row.case);
        assert_eq!(
            listed, row.expected,
            "case {:?} ({} GET {}{}): expected {:?}, got {:?}",
            row.case, listing.operation, listing.route, row.query, row.expected, listed
        );
    }
}

/// `ListOutposts`, whose `AvailabilityZoneFilter` / `AvailabilityZoneIdFilter` /
/// `LifeCycleStatusFilter` select on the Outpost's own members.
async fn outpost_list_selects_by_the_rule() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "filter-rule-site" }))
        .await;
    let site_id = text(&site, "SiteId");

    // Three zones of the region these requests are addressed to (an Outpost can only
    // be placed in a zone of its own region), and one Outpost placed in no zone at
    // all: the record with no value for the filtered member.
    let zones = [
        ("us-east-1a", "use1-az1"),
        ("us-east-1b", "use1-az2"),
        ("us-east-1c", "use1-az3"),
    ];
    let mut placed: Vec<String> = Vec::new();
    for (zone, zone_id) in zones {
        let outpost = outposts
            .create_outpost(&json!({
                "Name": format!("filter-rule-{zone}"),
                "SiteId": site_id,
                "AvailabilityZone": zone,
                "AvailabilityZoneId": zone_id,
            }))
            .await;
        placed.push(text(&outpost, "OutpostId"));
    }
    let zoneless = text(
        &outposts
            .create_outpost(&json!({
                "Name": "filter-rule-zoneless",
                "SiteId": site_id,
            }))
            .await,
        "OutpostId",
    );
    // An Outpost carries the provisioning status until hardware ordered for it has
    // been installed, so the status rows are exact only once each Outpost above holds
    // the status the row names: hardware is ordered and installed for every one of
    // them here, and the row for the provisioning status is the one no record matches.
    for outpost_id in placed.iter().chain(std::iter::once(&zoneless)) {
        install_hardware(&outposts, outpost_id).await;
    }

    let (first, second, third) = (
        placed[0].clone(),
        placed[1].clone(),
        placed[2].clone(),
    );
    let every = vec![
        first.clone(),
        second.clone(),
        third.clone(),
        zoneless.clone(),
    ];
    let rows = vec![
        row("no filter at all", String::new(), every.clone()),
        row(
            "one accepted value",
            "?AvailabilityZoneFilter=us-east-1a".to_string(),
            vec![first.clone()],
        ),
        row(
            "two accepted values",
            "?AvailabilityZoneFilter=us-east-1a&AvailabilityZoneFilter=us-east-1c".to_string(),
            vec![first.clone(), third.clone()],
        ),
        row(
            "every value any record carries, which still narrows out the one carrying none",
            "?AvailabilityZoneFilter=us-east-1a&AvailabilityZoneFilter=us-east-1b\
             &AvailabilityZoneFilter=us-east-1c"
                .to_string(),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        row(
            "a value no record carries",
            "?AvailabilityZoneFilter=us-west-2a".to_string(),
            Vec::new(),
        ),
        row(
            "the other identity of the same zone",
            "?AvailabilityZoneIdFilter=use1-az2".to_string(),
            vec![second.clone()],
        ),
        row(
            "two filters that agree narrow to their overlap",
            "?AvailabilityZoneFilter=us-east-1a&AvailabilityZoneIdFilter=use1-az1".to_string(),
            vec![first.clone()],
        ),
        row(
            "two filters that overlap in nothing",
            "?AvailabilityZoneFilter=us-east-1a&AvailabilityZoneIdFilter=use1-az2".to_string(),
            Vec::new(),
        ),
        row(
            "the status every Outpost whose ordered hardware is installed carries",
            "?LifeCycleStatusFilter=ACTIVE".to_string(),
            every.clone(),
        ),
        row(
            "a status no installed Outpost carries",
            "?LifeCycleStatusFilter=PENDING".to_string(),
            Vec::new(),
        ),
    ];

    assert_selects(
        &outposts,
        &Listing {
            operation: "ListOutposts",
            route: "/outposts".to_string(),
            page_member: "Outposts",
            id_member: "OutpostId",
        },
        &rows,
    )
    .await;
}

/// `ListSites`, whose three `OperatingAddress*Filter` members select on the site's
/// operating-address projection.
async fn site_list_selects_by_the_rule() {
    let outposts = Outposts::new();
    // Three sites whose operating addresses differ in country, state and city, and
    // one site with no operating address at all: the record with no value for the
    // filtered members.
    let rows_of_sites: Vec<(&str, Option<Value>)> = vec![
        ("seattle", Some(address("US", "WA", "Seattle"))),
        ("dublin", Some(address("IE", "Leinster", "Dublin"))),
        ("tokyo", Some(address("JP", "Tokyo", "Tokyo"))),
        ("addressless", None),
    ];
    let mut created: Vec<String> = Vec::new();
    for (name, operating) in rows_of_sites {
        let mut request = json!({ "Name": format!("filter-rule-{name}") });
        if let Some(operating) = operating {
            request["OperatingAddress"] = operating;
        }
        created.push(text(&outposts.create_site(&request).await, "SiteId"));
    }
    let (seattle, dublin, tokyo) = (
        created[0].clone(),
        created[1].clone(),
        created[2].clone(),
    );

    let rows = vec![
        row("no filter at all", String::new(), created.clone()),
        row(
            "one accepted value",
            "?OperatingAddressCityFilter=Seattle".to_string(),
            vec![seattle.clone()],
        ),
        row(
            "two accepted values",
            "?OperatingAddressCityFilter=Seattle&OperatingAddressCityFilter=Dublin".to_string(),
            vec![seattle.clone(), dublin.clone()],
        ),
        row(
            "every value any record carries, which still narrows out the one carrying none",
            "?OperatingAddressCityFilter=Seattle&OperatingAddressCityFilter=Dublin\
             &OperatingAddressCityFilter=Tokyo"
                .to_string(),
            vec![seattle.clone(), dublin.clone(), tokyo.clone()],
        ),
        row(
            "a value no record carries",
            "?OperatingAddressCityFilter=Bangalore".to_string(),
            Vec::new(),
        ),
        row(
            "another member's filter",
            "?OperatingAddressCountryCodeFilter=JP".to_string(),
            vec![tokyo.clone()],
        ),
        row(
            "a third member's filter",
            "?OperatingAddressStateOrRegionFilter=WA".to_string(),
            vec![seattle.clone()],
        ),
        row(
            "two filters that agree narrow to their overlap",
            "?OperatingAddressCountryCodeFilter=US&OperatingAddressCityFilter=Seattle".to_string(),
            vec![seattle.clone()],
        ),
        row(
            "two filters that overlap in nothing",
            "?OperatingAddressCountryCodeFilter=US&OperatingAddressCityFilter=Dublin".to_string(),
            Vec::new(),
        ),
    ];

    assert_selects(
        &outposts,
        &Listing {
            operation: "ListSites",
            route: "/sites".to_string(),
            page_member: "Sites",
            id_member: "SiteId",
        },
        &rows,
    )
    .await;
}

/// `ListAssets`, whose `AssetTypeFilter` selects on the asset's own `AssetType` and
/// whose `HostIdFilter` / `StatusFilter` select on the `ComputeAttributes` a compute
/// asset carries.
async fn asset_list_selects_by_the_rule() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("filter-rule-assets").await;

    // The catalogs the values below come from are the model's own enums,
    // `ops::types::ASSET_TYPE_VALUES` and `ops::types::ASSET_STATE_VALUES`; the two
    // states used here are entries of `AssetState` the issue this suite was written
    // for did not cite.
    let (active, isolated) = (ASSET_STATE_VALUES[0], ASSET_STATE_VALUES[2]);
    let (first_host, second_host) = ("h-0000000000000001", "h-0000000000000002");
    let recorded = vec![
        asset(
            "1000000001",
            "rack-1",
            Some(ASSET_TYPE_VALUES[0]),
            Some(compute(Some(first_host), Some(active), None, None, None)),
            Some(1.0),
        ),
        asset(
            "1000000002",
            "rack-1",
            Some(ASSET_TYPE_VALUES[0]),
            Some(compute(Some(second_host), Some(isolated), None, None, None)),
            Some(2.0),
        ),
        // A storage server and a powershelf carry no compute attributes at all, so
        // they recorded no value for either member those two filters select on.
        asset("1000000003", "rack-1", Some(ASSET_TYPE_VALUES[3]), None, None),
        asset("1000000004", "rack-2", Some(ASSET_TYPE_VALUES[2]), None, None),
    ];
    record_assets(&outposts, &outpost_id, &recorded, "the filter rule");
    let every: Vec<String> = recorded
        .iter()
        .map(|asset| asset.asset_id.clone().unwrap_or_default())
        .collect();
    let (first, second, third, fourth) = (
        every[0].clone(),
        every[1].clone(),
        every[2].clone(),
        every[3].clone(),
    );

    let rows = vec![
        row("no filter at all", String::new(), every.clone()),
        row(
            "one accepted value",
            format!("?AssetTypeFilter={}", ASSET_TYPE_VALUES[0]),
            vec![first.clone(), second.clone()],
        ),
        row(
            "two accepted values",
            format!(
                "?AssetTypeFilter={}&AssetTypeFilter={}",
                ASSET_TYPE_VALUES[3], ASSET_TYPE_VALUES[2]
            ),
            vec![third.clone(), fourth.clone()],
        ),
        row(
            "a value no record carries",
            format!("?AssetTypeFilter={}", ASSET_TYPE_VALUES[4]),
            Vec::new(),
        ),
        row(
            "a filter whose member two records carry no value for",
            format!("?StatusFilter={active}"),
            vec![first.clone()],
        ),
        row(
            "both values that member carries, which still narrows out the records carrying none",
            format!("?StatusFilter={active}&StatusFilter={isolated}"),
            vec![first.clone(), second.clone()],
        ),
        row(
            "a third member's filter",
            format!("?HostIdFilter={second_host}"),
            vec![second.clone()],
        ),
        row(
            "two filters that agree narrow to their overlap",
            format!(
                "?AssetTypeFilter={}&StatusFilter={isolated}",
                ASSET_TYPE_VALUES[0]
            ),
            vec![second.clone()],
        ),
        row(
            "two filters that overlap in nothing",
            format!(
                "?AssetTypeFilter={}&StatusFilter={active}",
                ASSET_TYPE_VALUES[3]
            ),
            Vec::new(),
        ),
    ];

    assert_selects(
        &outposts,
        &Listing {
            operation: "ListAssets",
            route: format!("/outposts/{}/assets", segment(&outpost_id)),
            page_member: "Assets",
            id_member: "AssetId",
        },
        &rows,
    )
    .await;
}

/// `ListAssetInstances`, whose four filters select on four members of one recorded
/// instance.
async fn asset_instance_list_selects_by_the_rule() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("filter-rule-instances").await;

    // The owning-service values are entries of the model's own `AWSServiceName` enum
    // (`ops::types::A_W_S_SERVICE_NAME_VALUES`); the last entry of it is the one no
    // recorded instance below carries.
    let (ec2, rds, unused_service) = (
        A_W_S_SERVICE_NAME_VALUES[1],
        A_W_S_SERVICE_NAME_VALUES[5],
        A_W_S_SERVICE_NAME_VALUES[6],
    );
    let own = ::engine::emulator_account_id();
    let recorded = vec![
        instance(
            &instance_id(1),
            Some("m5.4xlarge"),
            Some("1000000001"),
            Some(own),
            None,
        ),
        instance(
            &instance_id(2),
            Some("c5.2xlarge"),
            Some("1000000001"),
            Some(OTHER_ACCOUNT),
            Some(ec2),
        ),
        instance(
            &instance_id(3),
            Some("m5.4xlarge"),
            Some("1000000002"),
            Some(own),
            Some(rds),
        ),
        // An instance that recorded nothing but its id: no value for any of the four
        // filtered members.
        instance(&instance_id(4), None, None, None, None),
    ];
    record_instances(&outposts, &outpost_id, &recorded, "the filter rule");
    let every: Vec<String> = recorded
        .iter()
        .map(|instance| instance.instance_id.clone().unwrap_or_default())
        .collect();
    let (first, second, third) = (every[0].clone(), every[1].clone(), every[2].clone());

    let rows = vec![
        row("no filter at all", String::new(), every.clone()),
        row(
            "one accepted value",
            "?InstanceTypeFilter=c5.2xlarge".to_string(),
            vec![second.clone()],
        ),
        row(
            "two accepted values",
            "?InstanceTypeFilter=m5.4xlarge&InstanceTypeFilter=c5.2xlarge".to_string(),
            vec![first.clone(), second.clone(), third.clone()],
        ),
        row(
            "a value no record carries",
            "?InstanceTypeFilter=t3.micro".to_string(),
            Vec::new(),
        ),
        row(
            "another member's filter",
            format!("?AccountIdFilter={OTHER_ACCOUNT}"),
            vec![second.clone()],
        ),
        row(
            "a third member's filter",
            "?AssetIdFilter=1000000001".to_string(),
            vec![first.clone(), second.clone()],
        ),
        row(
            "a fourth member's filter, whose member two records carry no value for",
            format!("?AwsServiceFilter={ec2}&AwsServiceFilter={rds}"),
            vec![second.clone(), third.clone()],
        ),
        row(
            "a value of that same catalog no record carries",
            format!("?AwsServiceFilter={unused_service}"),
            Vec::new(),
        ),
        row(
            "two filters that agree narrow to their overlap",
            format!("?InstanceTypeFilter=m5.4xlarge&AccountIdFilter={own}"),
            vec![first.clone(), third.clone()],
        ),
        row(
            "two filters that overlap in nothing",
            format!("?InstanceTypeFilter=c5.2xlarge&AccountIdFilter={own}"),
            Vec::new(),
        ),
    ];

    assert_selects(
        &outposts,
        &Listing {
            operation: "ListAssetInstances",
            route: format!("/outposts/{}/assetInstances", segment(&outpost_id)),
            page_member: "AssetInstances",
            id_member: "InstanceId",
        },
        &rows,
    )
    .await;
}

/// Record a capacity task of `outpost_id` in `status` through the store, and answer
/// the `CapacityTaskId` it is keyed by.
///
/// The modelled `CapacityTaskStatus` declares seven values and the implemented
/// operations produce three of them, so a row that needs a task in one of the others
/// records it through the store — the seam a case uses for state no call of this API
/// creates. The record takes the store's next creation sequence, so a recorded task
/// takes its place in the one creation order the list is reported in, and owes the
/// clock nothing: the status it is recorded in is the status it holds.
fn record_task(outposts: &Outposts, outpost_id: &str, status: &str, index: usize) -> String {
    let now = std::time::SystemTime::now();
    let store = outposts.store();
    let mut store = store.lock();
    let sequence = store.created;
    store.created += 1;
    let capacity_task_id = format!("cap-{:017x}", index + 1);
    store.capacity_tasks.insert(
        capacity_task_id.clone(),
        CapacityTaskRecord {
            sequence,
            capacity_task_id: capacity_task_id.clone(),
            outpost_id: outpost_id.to_string(),
            order_id: None,
            asset_id: None,
            requested_instance_pools: Vec::new(),
            instances_to_exclude: None,
            dry_run: false,
            task_action_on_blocking_instances: None,
            status: status.to_string(),
            failed: None,
            created_at: now,
            last_modified_at: now,
            completed_at: (status == CAPACITY_TASK_COMPLETED).then_some(now),
            pending: VecDeque::new(),
        },
    );
    capacity_task_id
}

/// `ListCapacityTasks`, whose `CapacityTaskStatusFilter` is its one list filter.
///
/// A task's status is a member every task carries, so this list has no record with no
/// value for the filtered member; what it does have is the second filter
/// (`OutpostIdentifierFilter`, a single identifier rather than a list), which the rows
/// below narrow with alongside the list filter.
async fn capacity_task_list_selects_by_the_rule() {
    let outposts = Outposts::new();
    let (first_outpost, _) = outposts.site_and_outpost("filter-rule-tasks-first").await;
    let (second_outpost, _) = outposts.site_and_outpost("filter-rule-tasks-second").await;

    // Four statuses of the model's own `CapacityTaskStatus` enum
    // (`ops::types::CAPACITY_TASK_STATUS_VALUES`), spread across the two Outposts so
    // no row can pass by selecting everything — and the fifth entry of that same
    // catalog is the value no recorded task carries.
    let held: Vec<(String, &str)> = CAPACITY_TASK_STATUS_VALUES[..4]
        .iter()
        .enumerate()
        .map(|(index, status)| {
            let outpost_id = match index % 2 {
                0 => &first_outpost,
                _ => &second_outpost,
            };
            (record_task(&outposts, outpost_id, status, index), *status)
        })
        .collect();
    let unused_status = CAPACITY_TASK_STATUS_VALUES[4];
    let select = |statuses: &[&str]| -> Vec<String> {
        held.iter()
            .filter(|(_, status)| statuses.contains(status))
            .map(|(capacity_task_id, _)| capacity_task_id.clone())
            .collect()
    };
    let every: Vec<String> = held
        .iter()
        .map(|(capacity_task_id, _)| capacity_task_id.clone())
        .collect();
    let statuses: Vec<&str> = held.iter().map(|(_, status)| *status).collect();

    let rows = vec![
        row("no filter at all", String::new(), every.clone()),
        row(
            "one accepted value",
            format!("?CapacityTaskStatusFilter={}", statuses[0]),
            select(&statuses[..1]),
        ),
        row(
            "two accepted values",
            format!(
                "?CapacityTaskStatusFilter={}&CapacityTaskStatusFilter={}",
                statuses[0], statuses[2]
            ),
            select(&[statuses[0], statuses[2]]),
        ),
        row(
            "every value any record carries",
            format!(
                "?{}",
                statuses
                    .iter()
                    .map(|status| format!("CapacityTaskStatusFilter={status}"))
                    .collect::<Vec<String>>()
                    .join("&")
            ),
            every.clone(),
        ),
        row(
            "a value of that same catalog no record carries",
            format!("?CapacityTaskStatusFilter={unused_status}"),
            Vec::new(),
        ),
        row(
            "two filters that agree narrow to their overlap",
            format!(
                "?OutpostIdentifierFilter={first_outpost}&CapacityTaskStatusFilter={}",
                statuses[0]
            ),
            select(&statuses[..1]),
        ),
        row(
            "two filters that overlap in nothing",
            format!(
                "?OutpostIdentifierFilter={second_outpost}&CapacityTaskStatusFilter={}",
                statuses[0]
            ),
            Vec::new(),
        ),
    ];

    assert_selects(
        &outposts,
        &Listing {
            operation: "ListCapacityTasks",
            route: "/capacity/tasks".to_string(),
            page_member: "CapacityTasks",
            id_member: "CapacityTaskId",
        },
        &rows,
    )
    .await;
}

/// The one rule, driven through an operation from each of the five lists that take
/// filters: every list answers the same table of cases for its own filters.
#[tokio::test]
async fn every_list_selects_its_filtered_records_by_the_one_rule() {
    outpost_list_selects_by_the_rule().await;
    site_list_selects_by_the_rule().await;
    asset_list_selects_by_the_rule().await;
    asset_instance_list_selects_by_the_rule().await;
    capacity_task_list_selects_by_the_rule().await;
}

/// The arms of the rule no query string can carry: a filter the request did not
/// send at all, and one sent EMPTY. Both narrow nothing, whether or not the record
/// carries a value for the member — which is what keeps a list's filters from
/// widening it.
#[test]
fn the_shared_rule_covers_the_filter_forms_no_query_string_can_carry() {
    /// One row of the rule's own table: the case it states, the filter the request
    /// carried, the value the record holds for the filtered member, and whether the
    /// rule accepts it.
    struct Case<'a> {
        case: &'static str,
        filter: Option<&'a [String]>,
        value: Option<&'static str>,
        expected: bool,
    }

    let accepted: Vec<String> = ASSET_STATE_VALUES[..2]
        .iter()
        .map(|state| state.to_string())
        .collect();
    fn case<'a>(
        case: &'static str,
        filter: Option<&'a [String]>,
        value: Option<&'static str>,
        expected: bool,
    ) -> Case<'a> {
        Case {
            case,
            filter,
            value,
            expected,
        }
    }
    let absent: Option<&[String]> = None;
    let empty: Option<&[String]> = Some(&[]);
    let cases = vec![
        case(
            "no filter, a record with a value",
            absent,
            Some(ASSET_STATE_VALUES[0]),
            true,
        ),
        case("no filter, a record with no value", absent, None, true),
        case(
            "an empty filter, a record with a value",
            empty,
            Some(ASSET_STATE_VALUES[0]),
            true,
        ),
        case("an empty filter, a record with no value", empty, None, true),
        case(
            "one accepted value the record carries",
            Some(&accepted[..1]),
            Some(ASSET_STATE_VALUES[0]),
            true,
        ),
        case(
            "one accepted value the record does not carry",
            Some(&accepted[..1]),
            Some(ASSET_STATE_VALUES[1]),
            false,
        ),
        case(
            "several accepted values, the first",
            Some(&accepted[..]),
            Some(ASSET_STATE_VALUES[0]),
            true,
        ),
        case(
            "several accepted values, the last",
            Some(&accepted[..]),
            Some(ASSET_STATE_VALUES[1]),
            true,
        ),
        case(
            "several accepted values, none of them the record's",
            Some(&accepted[..]),
            Some(ASSET_STATE_VALUES[3]),
            false,
        ),
        case(
            "a filter on a member the record carries no value for",
            Some(&accepted[..]),
            None,
            false,
        ),
    ];

    for row in cases {
        let actual = filter_accepts(row.filter, row.value);
        assert_eq!(
            actual, row.expected,
            "case {:?}: filter {:?} against value {:?} must answer {}, got {actual}",
            row.case, row.filter, row.value, row.expected
        );
    }
}
