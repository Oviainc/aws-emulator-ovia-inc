// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REGIONAL scope of this service, driven over the wire
//! through the same front door as [`crate::site_and_outpost_lifecycle`] (whose
//! harness this suite reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: every
//! resource of this service belongs to the region of the endpoint it was created
//! through — the region its own ARN names — and only that region's endpoint serves
//! it. Every other region answers as if the resource had never been created: it is
//! absent from that region's lists, and a call naming it answers the modelled
//! `NotFoundException` (404), whichever identifier form names it and whether the call
//! reads it, changes it or deletes it.
//!
//! The rows differ along the region the call is addressed to, how many resources a
//! region holds (none, one, several), the resource kind (site, Outpost, quote), the
//! identifier form (bare id and full ARN), and the operation — every route of the
//! implemented slice that names a stored resource.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::capacity_task_lifecycle::pool;
use crate::outpost_assets::{asset, record};
use crate::quote_lifecycle::request as quote_request;
use crate::site_and_outpost_lifecycle::{address, ids, segment, text, Outposts, HOME_REGION};

/// The region codes every case below is addressed to.
///
/// Source: the AWS region codes this emulator's callers sign for — the rows span a
/// single-word direction (`us-east-1`, `eu-central-1`), a compound one
/// (`ap-southeast-2`), a southern-hemisphere region (`sa-east-1`) and a second
/// region of one geography (`eu-west-1`), so rows differ by a word and by a number
/// alike. Nothing about the rule depends on the form: what it depends on is that the
/// codes differ. Every row is a region of the `aws` partition, because which
/// partition prefix an ARN carries is a different rule from this one and no case
/// here asserts it.
const REGIONS: [&str; 5] = [
    "us-east-1",
    "eu-central-1",
    "ap-southeast-2",
    "sa-east-1",
    "eu-west-1",
];

/// How many resources each region of [`REGIONS`] is given, as
/// `(region, sites, Outposts on the first site)`.
///
/// The counts span none at all, one, and several, so a region that leaked another's
/// inventory would be caught either way round: a region holding nothing must list
/// nothing even while other regions hold plenty.
const INVENTORY: [(&str, usize, usize); 5] = [
    ("us-east-1", 1, 2),
    ("eu-central-1", 2, 1),
    ("ap-southeast-2", 3, 3),
    ("sa-east-1", 1, 0),
    ("eu-west-1", 0, 0),
];

/// The account id every generated emulator presents, which an ARN's fifth segment
/// carries.
fn account_id() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN prefix a resource of `kind` created in `region` must carry, from the
/// modelled `SiteArn` / `OutpostArn` patterns
/// (`^arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:(site|outpost)/…$`) — the region
/// segment is the region the create was addressed to.
fn arn_prefix(region: &str, kind: &str) -> String {
    format!("arn:aws:outposts:{region}:{}:{kind}/", account_id())
}

/// Every `region` of [`REGIONS`] that is not `home`: the endpoints that must answer
/// as if a resource of `home` had never been created.
fn other_regions(home: &str) -> Vec<&'static str> {
    REGIONS
        .into_iter()
        .filter(|region| *region != home)
        .collect()
}

/// One call that names a stored resource, as `method` plus a path with the
/// identifier already in it.
struct Call {
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

impl Call {
    fn new(case: String, method: &'static str, path: String, body: Option<Value>) -> Self {
        Self {
            case,
            method,
            path,
            body,
        }
    }
}

/// The resources one region's endpoint created, each by both identifier forms, plus
/// the capacity task started on the Outpost, the order placed on a second Outpost of
/// the same site, the hardware asset recorded on the first and the installation
/// connection started for that asset.
struct Created {
    site_id: String,
    site_arn: String,
    outpost_id: String,
    outpost_arn: String,
    /// The second Outpost of the same site, which the order is placed on: an
    /// outstanding order BLOCKS its Outpost's decommission, and the first Outpost is
    /// what the decommission case acts on.
    order_outpost_id: String,
    quote_id: String,
    capacity_task_id: String,
    order_id: String,
    asset_id: String,
    connection_id: String,
}

impl Created {
    /// The two identifier forms the modelled `@httpLabel` patterns accept for the
    /// site: the bare id and the full ARN.
    fn site_forms(&self) -> [&str; 2] {
        [&self.site_id, &self.site_arn]
    }

    /// The same, for the Outpost.
    fn outpost_forms(&self) -> [&str; 2] {
        [&self.outpost_id, &self.outpost_arn]
    }
}

/// A `WireGuardPublicKey` of the modelled form (`^[a-zA-Z0-9/+]{43}=$`), which
/// `StartConnection` requires for the client end of the tunnel.
const CLIENT_PUBLIC_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// The `AssetId` recorded on `region`'s Outpost, of the modelled `^(\w+)$` form —
/// one per region, so no two regions name the same unit of hardware.
fn asset_id_in(region: &str) -> String {
    format!("asset{}", region.replace('-', ""))
}

/// Create, through `region`'s endpoint, one of every resource the implemented slice
/// persists: a site with both addresses, an Outpost on it, a quote associated with
/// that Outpost, a capacity task on it, an order placed on it, and — for the hardware
/// asset recorded on it — an installation connection.
async fn create_everything(outposts: &Outposts, region: &str) -> Created {
    let site = outposts
        .create_site_in(
            region,
            &json!({
                "Name": format!("{region} site"),
                "OperatingAddress": address("US", "WA", "Seattle"),
                "ShippingAddress": address("US", "WA", "Kirkland"),
            }),
        )
        .await;
    let site_id = text(&site, "SiteId");
    let outpost = outposts
        .create_outpost_in(
            region,
            &json!({ "Name": format!("{region} outpost"), "SiteId": site_id }),
        )
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    let quote = outposts
        .ok_in(
            region,
            "POST",
            "/quotes",
            Some(&quote_request(
                "US",
                json!([]),
                json!({ "OutpostIdentifier": outpost_id }),
            )),
        )
        .await["Quote"]
        .clone();
    let task = outposts
        .ok_in(
            region,
            "POST",
            &format!("/outposts/{}/capacity", segment(&outpost_id)),
            Some(&json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
        )
        .await;
    // An asset is PHYSICAL hardware that arrives with a fulfilled order, and the
    // operations an order arrives through are still scaffold stubs, so it is
    // recorded on the stored Outpost the way `crate::outpost_assets` records one.
    // `COMPUTE` is a value of the modelled `AssetType` enum
    // (`ops::types::ASSET_TYPE_VALUES`).
    let asset_id = asset_id_in(region);
    record(
        outposts,
        &outpost_id,
        &[asset(&asset_id, "rack-1", Some("COMPUTE"), None, None)],
        &format!("{region} hardware"),
    );
    let connection = outposts
        .ok_in(
            region,
            "POST",
            "/connections",
            Some(&json!({
                "AssetId": asset_id,
                "ClientPublicKey": CLIENT_PUBLIC_KEY,
                "NetworkInterfaceDeviceIndex": 0,
            })),
        )
        .await;
    // An order on a SECOND Outpost of the same site: an outstanding order blocks
    // its own Outpost's decommission, and the first Outpost is the one the
    // decommission case acts on. The order is built from a line item of its own
    // rather than from the quote above, so that quote stays orderable.
    let order_outpost = outposts
        .create_outpost_in(
            region,
            &json!({ "Name": format!("{region} order outpost"), "SiteId": site_id }),
        )
        .await;
    let order = outposts
        .ok_in(
            region,
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": text(&order_outpost, "OutpostId"),
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": "OR-1A2B3C4", "Quantity": 1 }],
            })),
        )
        .await["Order"]
        .clone();
    Created {
        site_id: text(&site, "SiteId"),
        site_arn: text(&site, "SiteArn"),
        outpost_id: text(&outpost, "OutpostId"),
        outpost_arn: text(&outpost, "OutpostArn"),
        order_outpost_id: text(&order_outpost, "OutpostId"),
        quote_id: text(&quote, "QuoteId"),
        capacity_task_id: text(&task, "CapacityTaskId"),
        order_id: text(&order, "OrderId"),
        asset_id,
        connection_id: text(&connection, "ConnectionId"),
    }
}

/// Every route of the implemented slice that names one of `created`'s resources,
/// each by every identifier form that route accepts.
fn calls_naming(created: &Created) -> Vec<Call> {
    let mut calls = Vec::new();
    for site in created.site_forms() {
        let at = |suffix: &str| format!("/sites/{}{suffix}", segment(site));
        calls.push(Call::new(
            format!("GetSite {site}"),
            "GET",
            at(""),
            None,
        ));
        calls.push(Call::new(
            format!("GetSiteAddress {site}"),
            "GET",
            at("/address?AddressType=OPERATING_ADDRESS"),
            None,
        ));
        calls.push(Call::new(
            format!("UpdateSite {site}"),
            "PATCH",
            at(""),
            Some(json!({ "Name": "renamed from another region" })),
        ));
        calls.push(Call::new(
            format!("UpdateSiteAddress {site}"),
            "PUT",
            at("/address"),
            Some(json!({
                "AddressType": "SHIPPING_ADDRESS",
                "Address": address("US", "OR", "Portland"),
            })),
        ));
        calls.push(Call::new(
            format!("UpdateSiteRackPhysicalProperties {site}"),
            "PATCH",
            at("/rackPhysicalProperties"),
            Some(json!({ "PowerDrawKva": "POWER_10_KVA" })),
        ));
        calls.push(Call::new(
            format!("DeleteSite {site}"),
            "DELETE",
            at(""),
            None,
        ));
        calls.push(Call::new(
            format!("CreateOutpost on {site}"),
            "POST",
            "/outposts".to_string(),
            Some(json!({ "Name": "outpost on a foreign site", "SiteId": site })),
        ));
    }
    for outpost in created.outpost_forms() {
        let at = |suffix: &str| format!("/outposts/{}{suffix}", segment(outpost));
        calls.push(Call::new(
            format!("GetOutpost {outpost}"),
            "GET",
            at(""),
            None,
        ));
        calls.push(Call::new(
            format!("UpdateOutpost {outpost}"),
            "PATCH",
            at(""),
            Some(json!({ "Name": "renamed from another region" })),
        ));
        calls.push(Call::new(
            format!("GetPrivateConnectivityConfig {outpost}"),
            "GET",
            at("/privateConnectivity"),
            None,
        ));
        calls.push(Call::new(
            format!("CreatePrivateConnectivityConfig {outpost}"),
            "POST",
            at("/privateConnectivity"),
            Some(json!({ "VpcInformationList": [{ "VpcId": "vpc-0000000000001" }] })),
        ));
        calls.push(Call::new(
            format!("ListAssets {outpost}"),
            "GET",
            at("/assets"),
            None,
        ));
        calls.push(Call::new(
            format!("ListAssetInstances {outpost}"),
            "GET",
            at("/assetInstances"),
            None,
        ));
        calls.push(Call::new(
            format!("GetOutpostSupportedInstanceTypes {outpost}"),
            "GET",
            at("/supportedInstanceTypes"),
            None,
        ));
        calls.push(Call::new(
            format!("GetCapacityTask {outpost}"),
            "GET",
            at(&format!(
                "/capacity/{}",
                segment(&created.capacity_task_id)
            )),
            None,
        ));
        calls.push(Call::new(
            format!("ListBlockingInstancesForCapacityTask {outpost}"),
            "GET",
            at(&format!(
                "/capacity/{}/blockingInstances",
                segment(&created.capacity_task_id)
            )),
            None,
        ));
        calls.push(Call::new(
            format!("StartCapacityTask {outpost}"),
            "POST",
            at("/capacity"),
            Some(json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
        ));
        calls.push(Call::new(
            format!("StartOutpostDecommission {outpost}"),
            "POST",
            at("/decommission"),
            Some(json!({ "ValidateOnly": false })),
        ));
        calls.push(Call::new(
            format!("DeleteOutpost {outpost}"),
            "DELETE",
            at(""),
            None,
        ));
        // `GetRenewalPricing`'s route is the singular `/outpost/{…}`.
        calls.push(Call::new(
            format!("GetRenewalPricing {outpost}"),
            "GET",
            format!("/outpost/{}/renewal-pricing", segment(outpost)),
            None,
        ));
        calls.push(Call::new(
            format!("CreateQuote for {outpost}"),
            "POST",
            "/quotes".to_string(),
            Some(quote_request(
                "US",
                json!([]),
                json!({ "OutpostIdentifier": outpost }),
            )),
        ));
        calls.push(Call::new(
            format!("CreateOrder on {outpost}"),
            "POST",
            "/orders".to_string(),
            Some(json!({
                "OutpostIdentifier": outpost,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": [{ "CatalogItemId": "OR-ABCDEFG", "Quantity": 1 }],
            })),
        ));
    }
    calls.push(Call::new(
        format!("GetQuote {}", created.quote_id),
        "GET",
        format!("/quotes/{}", segment(&created.quote_id)),
        None,
    ));
    calls.push(Call::new(
        format!("GetConnection {}", created.connection_id),
        "GET",
        format!("/connections/{}", segment(&created.connection_id)),
        None,
    ));
    calls.push(Call::new(
        format!("GetOrder {}", created.order_id),
        "GET",
        format!("/orders/{}", segment(&created.order_id)),
        None,
    ));
    calls.push(Call::new(
        format!("StartConnection for asset {}", created.asset_id),
        "POST",
        "/connections".to_string(),
        Some(json!({
            "AssetId": created.asset_id,
            "ClientPublicKey": CLIENT_PUBLIC_KEY,
            "NetworkInterfaceDeviceIndex": 1,
        })),
    ));
    // The tagging routes name their resource by ARN alone (`ResourceArn`), so each
    // taggable kind appears once.
    for arn in [&created.site_arn, &created.outpost_arn] {
        calls.push(Call::new(
            format!("ListTagsForResource {arn}"),
            "GET",
            format!("/tags/{}", segment(arn)),
            None,
        ));
        calls.push(Call::new(
            format!("TagResource {arn}"),
            "POST",
            format!("/tags/{}", segment(arn)),
            Some(json!({ "Tags": { "Env": "from another region" } })),
        ));
    }
    calls
}

/// The rule's list half: each regional endpoint lists exactly the sites and Outposts
/// created through it, and every ARN it reports names that same region.
#[tokio::test]
async fn each_regional_endpoint_lists_only_its_own_inventory() {
    let outposts = Outposts::new();

    // What each region was given, by region: the ids its own lists must report and
    // no other region's may.
    let mut expected: Vec<(&str, BTreeSet<String>, BTreeSet<String>)> = Vec::new();
    for (region, sites, outposts_on_first_site) in INVENTORY {
        let mut site_ids: BTreeSet<String> = BTreeSet::new();
        let mut outpost_ids: BTreeSet<String> = BTreeSet::new();
        for index in 0..sites {
            let site = outposts
                .create_site_in(region, &json!({ "Name": format!("{region} site {index}") }))
                .await;
            let site_id = text(&site, "SiteId");
            assert!(
                site_id.starts_with("os-") && text(&site, "SiteArn").starts_with(&arn_prefix(region, "site")),
                "case {region}: a site created in {region} carries that region's ARN, got {site:?}"
            );
            if index == 0 {
                for ordinal in 0..outposts_on_first_site {
                    let outpost = outposts
                        .create_outpost_in(
                            region,
                            &json!({
                                "Name": format!("{region} outpost {ordinal}"),
                                "SiteId": site_id,
                            }),
                        )
                        .await;
                    assert!(
                        text(&outpost, "OutpostArn")
                            .starts_with(&arn_prefix(region, "outpost")),
                        "case {region}: an Outpost created in {region} carries that \
                         region's ARN, got {outpost:?}"
                    );
                    outpost_ids.insert(text(&outpost, "OutpostId"));
                }
            }
            site_ids.insert(site_id);
        }
        expected.push((region, site_ids, outpost_ids));
    }

    for (region, site_ids, outpost_ids) in &expected {
        let listed_sites: BTreeSet<String> =
            ids(&outposts.list_sites_in(region, "").await, "Sites")
                .into_iter()
                .collect();
        assert_eq!(
            &listed_sites, site_ids,
            "case {region}: ListSites in {region} reports exactly that region's sites, \
             got {listed_sites:?}"
        );
        let listed_outposts: BTreeSet<String> =
            ids(&outposts.list_outposts_in(region, "").await, "Outposts")
                .into_iter()
                .collect();
        assert_eq!(
            &listed_outposts, outpost_ids,
            "case {region}: ListOutposts in {region} reports exactly that region's \
             Outposts, got {listed_outposts:?}"
        );
        // Every ARN a list reports names the region the list was addressed to, so a
        // caller can never be handed a cross-region ARN.
        for (list, member, kind) in [
            (
                outposts.list_sites_in(region, "").await,
                "Sites",
                "site",
            ),
            (
                outposts.list_outposts_in(region, "").await,
                "Outposts",
                "outpost",
            ),
        ] {
            let prefix = arn_prefix(region, kind);
            for item in list.get(member).and_then(Value::as_array).into_iter().flatten() {
                let arn = match kind {
                    "site" => text(item, "SiteArn"),
                    _ => text(item, "OutpostArn"),
                };
                assert!(
                    arn.starts_with(&prefix),
                    "case {region}: every {member} entry {region} reports carries an ARN \
                     of {region}, got {arn:?}"
                );
            }
        }
    }
}

/// The rule's single-resource half, over every route of the implemented slice that
/// names a stored resource: the region that created the resource serves the call, and
/// every other region answers 404 `NotFoundException` with the identifier echoed as
/// supplied — and changes nothing, so the resource is still there afterwards.
#[tokio::test]
async fn a_resource_is_served_only_by_the_region_that_created_it() {
    let outposts = Outposts::new();
    let created = create_everything(&outposts, HOME_REGION).await;
    // A second region's resources, so the store holds an inventory on both sides and
    // neither region's answer can be "the store is empty".
    let elsewhere = other_regions(HOME_REGION)[0];
    let abroad = create_everything(&outposts, elsewhere).await;

    for (home, created) in [(HOME_REGION, &created), (elsewhere, &abroad)] {
        for call in calls_naming(created) {
            for region in other_regions(home) {
                let reply = outposts
                    .call_in(region, call.method, &call.path, call.body.as_ref())
                    .await;
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (404, "NotFoundException"),
                    "case {:?} created in {home}, called in {region}: another region's \
                     endpoint must answer as if the resource did not exist, got {} {} {:?}",
                    call.case,
                    reply.status,
                    reply.error_code,
                    reply.body
                );
            }
        }
    }

    // Nothing the foreign calls asked for happened: every resource is still exactly
    // as its own region's endpoint created it.
    for (home, created) in [(HOME_REGION, &created), (elsewhere, &abroad)] {
        let site = outposts
            .ok_in(
                home,
                "GET",
                &format!("/sites/{}", segment(&created.site_id)),
                None,
            )
            .await["Site"]
            .clone();
        assert_eq!(
            text(&site, "Name"),
            format!("{home} site"),
            "case {home}: a foreign UpdateSite must not have renamed the site, got {site:?}"
        );
        let outpost = outposts.get_outpost_in(home, &created.outpost_id).await;
        assert_eq!(
            text(&outpost, "Name"),
            format!("{home} outpost"),
            "case {home}: a foreign UpdateOutpost must not have renamed the Outpost, \
             got {outpost:?}"
        );
        for (kind, arn) in [("site", &created.site_arn), ("outpost", &created.outpost_arn)] {
            let tags = outposts
                .ok_in(home, "GET", &format!("/tags/{}", segment(arn)), None)
                .await["Tags"]
                .clone();
            assert_eq!(
                tags,
                json!({}),
                "case {home}: a foreign TagResource must not have tagged the {kind}, \
                 got {tags:?}"
            );
        }
        let config = outposts
            .ok_in(
                home,
                "GET",
                &format!("/outposts/{}/privateConnectivity", segment(&created.outpost_id)),
                None,
            )
            .await;
        assert_eq!(
            config.get("PrivateConnectivityConfig"),
            None,
            "case {home}: a foreign CreatePrivateConnectivityConfig must not have \
             configured the Outpost, got {config:?}"
        );
        let shipping = outposts
            .ok_in(
                home,
                "GET",
                &format!(
                    "/sites/{}/address?AddressType=SHIPPING_ADDRESS",
                    segment(&created.site_id)
                ),
                None,
            )
            .await["Address"]
            .clone();
        assert_eq!(
            text(&shipping, "City"),
            "Kirkland",
            "case {home}: a foreign UpdateSiteAddress must not have moved the shipping \
             address, got {shipping:?}"
        );
        // The quote and the capacity task are still reachable through their own
        // region, and the decommission a foreign call asked for never started: the
        // first one this region makes is the one that starts it.
        let quote = outposts
            .ok_in(
                home,
                "GET",
                &format!("/quotes/{}", segment(&created.quote_id)),
                None,
            )
            .await["Quote"]
            .clone();
        assert_eq!(
            text(&quote, "QuoteId"),
            created.quote_id,
            "case {home}: the quote is still readable through its own region, got {quote:?}"
        );
        let task = outposts
            .ok_in(
                home,
                "GET",
                &format!(
                    "/outposts/{}/capacity/{}",
                    segment(&created.outpost_id),
                    segment(&created.capacity_task_id)
                ),
                None,
            )
            .await;
        assert_eq!(
            text(&task, "CapacityTaskId"),
            created.capacity_task_id,
            "case {home}: the capacity task is still readable through its own region, \
             got {task:?}"
        );
        let connection = outposts
            .ok_in(
                home,
                "GET",
                &format!("/connections/{}", segment(&created.connection_id)),
                None,
            )
            .await;
        assert_eq!(
            text(&connection, "ConnectionId"),
            created.connection_id,
            "case {home}: the connection is still readable through its own region, \
             got {connection:?}"
        );
        let order = outposts
            .ok_in(
                home,
                "GET",
                &format!("/orders/{}", segment(&created.order_id)),
                None,
            )
            .await["Order"]
            .clone();
        assert_eq!(
            text(&order, "OrderId"),
            created.order_id,
            "case {home}: the order is still readable through its own region, \
             got {order:?}"
        );
        let decommission = outposts
            .ok_in(
                home,
                "POST",
                &format!("/outposts/{}/decommission", segment(&created.outpost_id)),
                Some(&json!({ "ValidateOnly": false })),
            )
            .await;
        assert_eq!(
            text(&decommission, "Status"),
            "REQUESTED",
            "case {home}: no foreign call can have started the decommission, so this \
             one starts it, got {decommission:?}"
        );
    }
}

/// The other side of the rule: a region that holds nothing of its own is not short of
/// anything — a create addressed to it works, and from then on its endpoint serves
/// exactly what it created, while the region that held the resources before is
/// unchanged.
#[tokio::test]
async fn a_region_serves_what_it_creates_even_while_another_region_holds_more() {
    let outposts = Outposts::new();
    let home = create_everything(&outposts, HOME_REGION).await;

    for region in other_regions(HOME_REGION) {
        // Before: this region's endpoint holds nothing at all.
        for (member, list) in [
            ("Sites", outposts.list_sites_in(region, "").await),
            ("Outposts", outposts.list_outposts_in(region, "").await),
        ] {
            let listed = ids(&list, member);
            assert!(
                listed.is_empty(),
                "case {region}: a region nothing was created in lists no {member}, \
                 got {listed:?}"
            );
        }

        // After: exactly what this region created, by both identifier forms.
        let created = create_everything(&outposts, region).await;
        assert_eq!(
            ids(&outposts.list_sites_in(region, "").await, "Sites"),
            vec![created.site_id.clone()],
            "case {region}: the region lists the site it created"
        );
        assert_eq!(
            ids(&outposts.list_outposts_in(region, "").await, "Outposts")
                .into_iter()
                .collect::<BTreeSet<String>>(),
            BTreeSet::from([
                created.outpost_id.clone(),
                created.order_outpost_id.clone()
            ]),
            "case {region}: the region lists exactly the Outposts it created"
        );
        for identifier in created.outpost_forms() {
            let read = outposts.get_outpost_in(region, identifier).await;
            assert_eq!(
                text(&read, "OutpostId"),
                created.outpost_id,
                "case {region}: GetOutpost on {identifier:?} answers the Outpost this \
                 region created"
            );
            assert!(
                text(&read, "OutpostArn").starts_with(&arn_prefix(region, "outpost")),
                "case {region}: the Outpost's ARN names its own region, got {read:?}"
            );
        }
    }

    // The region that started with an inventory still has exactly it.
    assert_eq!(
        ids(&outposts.list_outposts_in(HOME_REGION, "").await, "Outposts")
            .into_iter()
            .collect::<BTreeSet<String>>(),
        BTreeSet::from([home.outpost_id.clone(), home.order_outpost_id.clone()]),
        "every other region's creates leave {HOME_REGION}'s own inventory alone"
    );
}

/// An ANONYMOUS call carries no credential scope to read a region from, so it is
/// served by the one region this service falls back to — the same one whichever call
/// it is, so an unsigned create and an unsigned read see one inventory.
#[tokio::test]
async fn an_unsigned_call_is_served_by_the_fallback_region() {
    let outposts = Outposts::new();
    let fallback = service_com_amazonaws_outposts_outpostsolafservice::state::DEFAULT_REGION;

    let site = outposts
        .call_unsigned("POST", "/sites", Some(&json!({ "Name": "unsigned site" })))
        .await;
    assert_eq!(
        site.status, 200,
        "an unsigned create is accepted, got {} {:?}",
        site.status, site.body
    );
    let site_id = text(&site.body["Site"], "SiteId");
    assert!(
        text(&site.body["Site"], "SiteArn").starts_with(&arn_prefix(fallback, "site")),
        "an unsigned create mints an ARN of the fallback region, got {:?}",
        site.body
    );

    // The same unsigned endpoint reads it back, and so does the region its ARN names.
    let unsigned_list = outposts.call_unsigned("GET", "/sites", None).await;
    assert_eq!(
        ids(&unsigned_list.body, "Sites"),
        vec![site_id.clone()],
        "an unsigned list reports what an unsigned create persisted, got {:?}",
        unsigned_list.body
    );
    assert_eq!(
        ids(&outposts.list_sites_in(fallback, "").await, "Sites"),
        vec![site_id.clone()],
        "the fallback region's own endpoint reports it too"
    );

    // Every other region sees nothing of it.
    for region in other_regions(fallback) {
        let listed = ids(&outposts.list_sites_in(region, "").await, "Sites");
        assert!(
            listed.is_empty(),
            "case {region}: an unsigned create belongs to {fallback} alone, got {listed:?}"
        );
        let reply = outposts
            .call_in(
                region,
                "GET",
                &format!("/sites/{}", segment(&site_id)),
                None,
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {region}: a site of {fallback} is not found in {region}, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}
