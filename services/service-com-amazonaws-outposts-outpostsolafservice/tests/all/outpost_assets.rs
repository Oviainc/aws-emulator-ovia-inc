// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAssets`, driven over the wire through the same front
//! door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `ListAssets` reports the hardware assets RECORDED for the Outpost the
//!    identifier names, field for field, in the Outpost's own installation order —
//!    and nothing else: an Outpost with no installed hardware reports no asset, and a
//!    member no asset recorded is reported by no member.
//! 2. Each `*Filter` is a LIST of accepted values for ONE member of an asset
//!    (`AssetTypeFilter` on its `AssetType`, `HostIdFilter` and `StatusFilter` on the
//!    `HostId` and `State` its `ComputeAttributes` carry), so an asset matches a
//!    filter when its value for that member is one of the listed ones, several
//!    filters narrow rather than widen, and an absent filter matches everything.
//! 3. A page holds at most `MaxResults` assets and carries a `NextToken` that resumes
//!    exactly where it stopped, so paging a filtered or unfiltered list from the
//!    first token to the last yields every matching asset once, in that same order.
//! 4. Both forms the modelled `OutpostIdentifier` pattern
//!    `^(arn:…:outpost/)?op-[a-f0-9]{17}$` accepts name the same Outpost, and the
//!    list is a REPORT: repeating it answers the same thing and changes nothing. An
//!    identifier that names no Outpost (including one whose Outpost was deleted) is
//!    404 `NotFoundException` with the identifier echo of the value AS SUPPLIED; one
//!    that is no Outpost identifier at all, and a `NextToken` this service did not
//!    issue for `ListAssets`, are 400 `ValidationException`.
//!
//! Where the recorded assets come from: an asset is a PHYSICAL unit of hardware —
//! a compute or storage server, a powershelf, a switch — that Amazon Web Services
//! installs, so no call in this API creates one; the hardware an Outpost settles with
//! is its own configuration's, which [`crate::outpost_hardware_installation`] states
//! and exercises. Each row here RECORDS its assets on the stored Outpost
//! through the same store seam [`crate::quote_lifecycle`] ages a quote through — while
//! the Outpost is still provisioning, so the installation the clock owes it leaves the
//! recorded hardware alone — and then asks over the wire: what the list answers about
//! what is recorded is the whole of rules 1 to 3. The hardware below is the rows' own
//! data, spanning members and values no one configuration holds at once.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    AssetInfo, AssetInstanceTypeCapacity, AssetLocation, ComputeAttributes, ASSET_STATE_VALUES,
    ASSET_TYPE_VALUES,
};

use crate::site_and_outpost_lifecycle::{
    query_value, refusal, segment, settle, text, Outposts, Refusal, ABSENT_OUTPOST_ID,
    ABSENT_SITE_ID, OUTPOST_ARN_PREFIX,
};

/// The route `ListAssets` binds, for one identifier form and one query string.
fn route(identifier: &str, query: &str) -> String {
    format!("/outposts/{}/assets{query}", segment(identifier))
}

/// One recorded asset, spelled out.
///
/// `elevation` becomes the asset's `AssetLocation$RackElevation`; an asset whose
/// location was never recorded passes `None` and must be reported without one.
pub(crate) fn asset(
    id: &str,
    rack: &str,
    asset_type: Option<&str>,
    compute: Option<ComputeAttributes>,
    elevation: Option<f32>,
) -> AssetInfo {
    AssetInfo {
        asset_id: Some(id.to_string()),
        rack_id: Some(rack.to_string()),
        asset_type: asset_type.map(str::to_string),
        compute_attributes: compute,
        asset_location: elevation.map(|rack_elevation| AssetLocation {
            rack_elevation: Some(rack_elevation),
        }),
    }
}

/// The `ComputeAttributes` of one recorded asset. Every member is optional, so a row
/// can record a server whose host id, state, instance families, per-type capacities
/// or vCPU count were never recorded.
fn compute(
    host_id: Option<&str>,
    state: Option<&str>,
    families: Option<&[&str]>,
    capacities: Option<&[(&str, i32)]>,
    max_vcpus: Option<i32>,
) -> ComputeAttributes {
    ComputeAttributes {
        host_id: host_id.map(str::to_string),
        state: state.map(str::to_string),
        instance_families: families
            .map(|families| families.iter().map(|family| family.to_string()).collect()),
        instance_type_capacities: capacities.map(|capacities| {
            capacities
                .iter()
                .map(|(instance_type, count)| AssetInstanceTypeCapacity {
                    instance_type: instance_type.to_string(),
                    count: *count,
                })
                .collect()
        }),
        max_vcpus,
    }
}

/// The hardware every filter and paging row below is recorded against: seven assets
/// in installation order.
///
/// Between them they span what the rules generalise over. Every value of the modelled
/// `AssetType` enum appears once (the catalog is `ops::types::ASSET_TYPE_VALUES`, read
/// off the model's own enum), and every value of `AssetState` appears on one of the
/// servers (`ops::types::ASSET_STATE_VALUES`) — so the filter rows cover each value of
/// each catalog, not only the ones a report cited. Three assets carry no
/// `ComputeAttributes` at all, which is what a powershelf, a switch and a networking
/// asset are; one carries no `AssetLocation`; the rack elevations include both
/// published bounds of the modelled `RackElevation` (`range(0..99)`) and the per-type
/// capacities both bounds of `InstanceTypeCount` (`range(0..9999)`); and the assets sit
/// in two different racks.
fn installed() -> Vec<AssetInfo> {
    vec![
        asset(
            "1000000001",
            "rack-1",
            Some("COMPUTE"),
            Some(compute(
                Some("h-0000000000000001"),
                Some("ACTIVE"),
                Some(&["m5", "c5"]),
                Some(&[("m5.24xlarge", 1), ("m5.4xlarge", 6)]),
                Some(96),
            )),
            // The published minimum of `RackElevation`.
            Some(0.0),
        ),
        asset(
            "1000000002",
            "rack-1",
            Some("COMPUTE"),
            Some(compute(
                Some("h-0000000000000002"),
                Some("ISOLATED"),
                Some(&["m5"]),
                // The published minimum of `InstanceTypeCount`.
                Some(&[("m5.xlarge", 0)]),
                Some(96),
            )),
            Some(14.5),
        ),
        asset(
            "1000000003",
            "rack-1",
            Some("STORAGE"),
            Some(compute(
                Some("h-0000000000000003"),
                Some("INSTALLING"),
                None,
                None,
                None,
            )),
            // The published maximum of `RackElevation`.
            Some(99.0),
        ),
        asset("1000000004", "rack-1", Some("POWERSHELF"), None, Some(1.0)),
        asset("1000000005", "rack-1", Some("SWITCH"), None, None),
        asset("1000000006", "rack-2", Some("NETWORKING"), None, Some(42.0)),
        asset(
            "1000000007",
            "rack-2",
            Some("COMPUTE"),
            Some(compute(
                Some("h-0000000000000004"),
                Some("RETIRING"),
                Some(&["r5"]),
                // The published maximum of `InstanceTypeCount`.
                Some(&[("r5.24xlarge", 9999)]),
                Some(96),
            )),
            Some(20.0),
        ),
    ]
}

/// An Outpost to record hardware on, as its `OutpostId`. An Outpost belongs to a
/// site, so the site comes first — the same order a real workflow takes.
async fn an_outpost(outposts: &Outposts, name: &str) -> String {
    let site = outposts
        .create_site(&json!({ "Name": format!("{name}-site") }))
        .await;
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": text(&site, "SiteId") }))
        .await;
    text(&outpost, "OutpostId")
}

/// Record `assets` as the hardware installed on the stored Outpost. See this module's
/// header for why a row records them through the store.
pub(crate) fn record(outposts: &Outposts, outpost_id: &str, assets: &[AssetInfo], case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .outposts
        .get_mut(outpost_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted the Outpost"));
    record.assets = assets.to_vec();
}

/// The `Assets` a page reports, as a list.
fn reported(output: &Value, case: &str) -> Vec<Value> {
    output
        .get("Assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an Assets list, got {output}"))
}

/// The `AssetId`s a page reports, in the order it reported them.
fn reported_ids(output: &Value, case: &str) -> Vec<String> {
    reported(output, case)
        .iter()
        .map(|item| text(item, "AssetId"))
        .collect()
}

/// The `AssetId`s of the recorded assets a predicate selects, in installation order:
/// what the wire must answer for the filter that states the same selection.
fn ids_where(assets: &[AssetInfo], select: impl Fn(&AssetInfo) -> bool) -> Vec<String> {
    assets
        .iter()
        .filter(|asset| select(asset))
        .map(|asset| asset.asset_id.clone().unwrap_or_default())
        .collect()
}

/// The `State` an asset's compute attributes recorded, if any.
fn state(asset: &AssetInfo) -> Option<&str> {
    asset
        .compute_attributes
        .as_ref()
        .and_then(|compute| compute.state.as_deref())
}

/// The `HostId` an asset's compute attributes recorded, if any.
fn host_id(asset: &AssetInfo) -> Option<&str> {
    asset
        .compute_attributes
        .as_ref()
        .and_then(|compute| compute.host_id.as_deref())
}

/// Rule 1's field-for-field half: one reported asset against the one a row recorded.
fn assert_reported(actual: &Value, expected: &AssetInfo, case: &str) {
    for (member, value) in [
        ("AssetId", expected.asset_id.as_deref()),
        ("RackId", expected.rack_id.as_deref()),
        ("AssetType", expected.asset_type.as_deref()),
    ] {
        assert_eq!(
            actual.get(member).and_then(Value::as_str),
            value,
            "case {case:?}: expected {member} {value:?}, got {actual}"
        );
    }
    match &expected.asset_location {
        Some(location) => {
            let location_value = actual.get("AssetLocation").unwrap_or_else(|| {
                panic!("case {case:?}: expected AssetLocation, got {actual}")
            });
            let elevation = location_value.get("RackElevation").and_then(Value::as_f64);
            assert_eq!(
                elevation,
                location.rack_elevation.map(f64::from),
                "case {case:?}: expected RackElevation {:?}, got {location_value}",
                location.rack_elevation
            );
        }
        None => assert!(
            actual.get("AssetLocation").is_none(),
            "case {case:?}: an asset with no recorded location must report none, got {actual}"
        ),
    }
    match &expected.compute_attributes {
        Some(expected_compute) => {
            let compute_value = actual.get("ComputeAttributes").unwrap_or_else(|| {
                panic!("case {case:?}: expected ComputeAttributes, got {actual}")
            });
            for (member, value) in [
                ("HostId", expected_compute.host_id.as_deref()),
                ("State", expected_compute.state.as_deref()),
            ] {
                assert_eq!(
                    compute_value.get(member).and_then(Value::as_str),
                    value,
                    "case {case:?}: expected ComputeAttributes.{member} {value:?}, got \
                     {compute_value}"
                );
            }
            let vcpus = compute_value.get("MaxVcpus").and_then(Value::as_i64);
            assert_eq!(
                vcpus,
                expected_compute.max_vcpus.map(i64::from),
                "case {case:?}: expected MaxVcpus {:?}, got {compute_value}",
                expected_compute.max_vcpus
            );
            let families: Option<Vec<String>> = compute_value
                .get("InstanceFamilies")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().unwrap_or_default().to_string())
                        .collect()
                });
            let expected_families: Option<Vec<String>> = expected_compute.instance_families.clone();
            assert_eq!(
                families, expected_families,
                "case {case:?}: expected InstanceFamilies {expected_families:?}, got \
                 {compute_value}"
            );
            let capacities: Option<Vec<(String, i64)>> = compute_value
                .get("InstanceTypeCapacities")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            (
                                text(item, "InstanceType"),
                                // `Count` is modelled with a default of 0, so a
                                // reported capacity that omits it carries that
                                // default.
                                item.get("Count").and_then(Value::as_i64).unwrap_or(0),
                            )
                        })
                        .collect()
                });
            let expected_capacities: Option<Vec<(String, i64)>> = expected_compute
                .instance_type_capacities
                .as_ref()
                .map(|capacities| {
                    capacities
                        .iter()
                        .map(|capacity| {
                            (capacity.instance_type.clone(), i64::from(capacity.count))
                        })
                        .collect()
                });
            assert_eq!(
                capacities, expected_capacities,
                "case {case:?}: expected InstanceTypeCapacities {expected_capacities:?}, got \
                 {compute_value}"
            );
        }
        None => assert!(
            actual.get("ComputeAttributes").is_none(),
            "case {case:?}: an asset with no recorded compute attributes must report none, got \
             {actual}"
        ),
    }
}

/// Rules 1 + 4's identifier half: the list answers every asset recorded for the
/// Outpost, field for field and in installation order, through either identifier form.
///
/// The rows differ along everything the rule generalises over: how many assets are
/// recorded (none, one, two, seven), which members each one recorded (all of them,
/// only an id, no compute attributes, no location), and which asset type and state
/// each carries.
#[tokio::test]
async fn list_assets_reports_every_asset_recorded_for_the_outpost() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Vec<AssetInfo>)> = vec![
        ("no hardware installed", Vec::new()),
        (
            "one server, every member recorded",
            vec![asset(
                "2000000001",
                "rack-solo",
                Some("COMPUTE"),
                Some(compute(
                    Some("h-0000000000000009"),
                    Some("ACTIVE"),
                    Some(&["c5", "m5", "r5"]),
                    Some(&[("c5.xlarge", 12), ("r5.24xlarge", 2)]),
                    Some(48),
                )),
                Some(33.0),
            )],
        ),
        (
            "one asset that recorded nothing but its id",
            vec![AssetInfo {
                asset_id: Some("2000000002".to_string()),
                ..AssetInfo::default()
            }],
        ),
        (
            "two assets, one of them without compute attributes",
            vec![
                asset(
                    "2000000003",
                    "rack-pair",
                    Some("COMPUTE"),
                    Some(compute(
                        Some("h-000000000000000a"),
                        Some("INSTALLING"),
                        Some(&["m5"]),
                        Some(&[("m5.2xlarge", 3)]),
                        Some(96),
                    )),
                    Some(2.0),
                ),
                asset("2000000004", "rack-pair", Some("SWITCH"), None, Some(3.0)),
            ],
        ),
        (
            "every asset type and state the model declares",
            installed(),
        ),
    ];

    for (case, assets) in &cases {
        let outpost_id = an_outpost(&outposts, "assets").await;
        record(&outposts, &outpost_id, assets, case);

        for identifier in [
            outpost_id.clone(),
            format!("{OUTPOST_ARN_PREFIX}{outpost_id}"),
        ] {
            let case = &format!("{case} read by {identifier}");
            let reply = outposts.ok("GET", &route(&identifier, ""), None).await;
            let items = reported(&reply, case);
            assert_eq!(
                items.len(),
                assets.len(),
                "case {case:?}: expected {} asset(s), got {reply}",
                assets.len()
            );
            for (index, (actual, expected)) in items.iter().zip(assets).enumerate() {
                assert_reported(actual, expected, &format!("{case} (asset {index})"));
            }
            // A page that exhausts the list issues no token to resume from.
            assert!(
                reply.get("NextToken").is_none(),
                "case {case:?}: a page holding every asset issues no NextToken, got {reply}"
            );
        }
    }
}

/// Rule 2: every filter row, with the expectation computed from the recorded hardware
/// rather than spelled out — so each value of the modelled `AssetType` and `AssetState`
/// catalogs, and each recorded host id, is covered.
#[tokio::test]
async fn filters_narrow_to_the_assets_that_match_every_one_of_them() {
    let outposts = Outposts::new();
    let installed = installed();
    let outpost_id = an_outpost(&outposts, "asset-filters").await;
    record(&outposts, &outpost_id, &installed, "filters");

    let every_id = ids_where(&installed, |_| true);
    let mut cases: Vec<(String, String, Vec<String>)> = vec![
        ("no filter at all".to_string(), String::new(), every_id),
        (
            "two asset types, so the union of both".to_string(),
            "?AssetTypeFilter=COMPUTE&AssetTypeFilter=SWITCH".to_string(),
            ids_where(&installed, |asset| {
                matches!(asset.asset_type.as_deref(), Some("COMPUTE") | Some("SWITCH"))
            }),
        ),
        (
            "every asset type at once, the modelled five-value maximum".to_string(),
            format!(
                "?{}",
                ASSET_TYPE_VALUES
                    .iter()
                    .map(|value| format!("AssetTypeFilter={value}"))
                    .collect::<Vec<_>>()
                    .join("&")
            ),
            ids_where(&installed, |asset| asset.asset_type.is_some()),
        ),
        (
            "two states, so the union of both".to_string(),
            "?StatusFilter=ACTIVE&StatusFilter=RETIRING".to_string(),
            ids_where(&installed, |asset| {
                matches!(state(asset), Some("ACTIVE") | Some("RETIRING"))
            }),
        ),
        (
            "an asset type and a state together narrow rather than widen".to_string(),
            "?AssetTypeFilter=COMPUTE&StatusFilter=INSTALLING".to_string(),
            ids_where(&installed, |asset| {
                asset.asset_type.as_deref() == Some("COMPUTE") && state(asset) == Some("INSTALLING")
            }),
        ),
        (
            "an asset type and a host id that belong to different assets".to_string(),
            "?AssetTypeFilter=STORAGE&HostIdFilter=h-0000000000000001".to_string(),
            Vec::new(),
        ),
        (
            "a host id no asset carries".to_string(),
            "?HostIdFilter=h-ffffffffffffffff".to_string(),
            Vec::new(),
        ),
        (
            "two host ids, so the union of both".to_string(),
            "?HostIdFilter=h-0000000000000001&HostIdFilter=h-0000000000000004".to_string(),
            ids_where(&installed, |asset| {
                matches!(
                    host_id(asset),
                    Some("h-0000000000000001") | Some("h-0000000000000004")
                )
            }),
        ),
    ];
    // One row per value of each catalog the model declares, so no value goes
    // untested: an asset matches the filter exactly when it recorded that value, and
    // an asset that recorded no value for the filtered member matches none.
    for asset_type in ASSET_TYPE_VALUES {
        cases.push((
            format!("the one asset type {asset_type}"),
            format!("?AssetTypeFilter={asset_type}"),
            ids_where(&installed, |asset| {
                asset.asset_type.as_deref() == Some(*asset_type)
            }),
        ));
    }
    for asset_state in ASSET_STATE_VALUES {
        cases.push((
            format!("the one state {asset_state}"),
            format!("?StatusFilter={asset_state}"),
            ids_where(&installed, |asset| state(asset) == Some(*asset_state)),
        ));
    }
    for recorded in installed.iter().filter_map(host_id) {
        cases.push((
            format!("the one host id {recorded}"),
            format!("?HostIdFilter={}", query_value(recorded)),
            ids_where(&installed, |asset| host_id(asset) == Some(recorded)),
        ));
    }

    for (case, query, wanted) in &cases {
        let reply = outposts.ok("GET", &route(&outpost_id, query), None).await;
        assert_eq!(
            &reported_ids(&reply, case),
            wanted,
            "case {case:?} ({query}): ListAssets must answer only the matching assets, got \
             {reply}"
        );
    }
}

/// Rule 3: for EVERY page size, paging the list from the first `NextToken` to the last
/// yields exactly the recorded assets, once each, in installation order — unfiltered
/// and under a filter alike.
///
/// The page sizes span one asset per page, a size that leaves a short last page, an
/// exact divisor of the count, the count itself, and more than the count, so a token
/// that duplicated or skipped an asset at any boundary would show up.
#[tokio::test]
async fn paging_covers_every_recorded_asset_exactly_once() {
    let outposts = Outposts::new();
    let installed = installed();
    let outpost_id = an_outpost(&outposts, "asset-paging").await;
    record(&outposts, &outpost_id, &installed, "paging");

    let unfiltered = ids_where(&installed, |_| true);
    let compute_only = ids_where(&installed, |asset| {
        asset.asset_type.as_deref() == Some("COMPUTE")
    });
    let filters: Vec<(&str, &str, &Vec<String>)> = vec![
        ("no filter", "", &unfiltered),
        ("filtered to COMPUTE", "AssetTypeFilter=COMPUTE&", &compute_only),
    ];

    for (filter_case, filter, expected) in &filters {
        for size in [1usize, 2, 3, 7, 9] {
            let case = &format!("{filter_case}, page size {size}");
            let mut collected: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let query = match &token {
                    Some(token) => format!(
                        "?{filter}MaxResults={size}&NextToken={}",
                        query_value(token)
                    ),
                    None => format!("?{filter}MaxResults={size}"),
                };
                let page = outposts.ok("GET", &route(&outpost_id, &query), None).await;
                let page_ids = reported_ids(&page, case);
                assert!(
                    page_ids.len() <= size,
                    "case {case:?}: a page holds at most MaxResults assets, got {page}"
                );
                collected.extend(page_ids);
                pages += 1;
                assert!(
                    pages <= installed.len() + 1,
                    "case {case:?}: paging must terminate, got {collected:?}"
                );
                match page.get("NextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                &collected, *expected,
                "case {case:?}: paging must yield every matching asset once, in installation \
                 order"
            );
        }
    }
}

/// Rule 4's report half: listing an Outpost's assets changes nothing — not the assets
/// it answers, not the Outpost, not another Outpost's assets.
#[tokio::test]
async fn listing_assets_changes_nothing_a_later_call_sees() {
    let outposts = Outposts::new();
    let stocked = an_outpost(&outposts, "stocked").await;
    let empty = an_outpost(&outposts, "empty").await;
    let installed = installed();
    record(&outposts, &stocked, &installed, "stocked");

    // Past the Outpost lifecycle transition the clock owes a just-created Outpost, so
    // the Outpost snapshots below differ only if THIS read changed one.
    settle();
    let before = outposts.get_outpost(&stocked).await;

    let mut answers = Vec::new();
    for pass in ["first read", "second read", "third read"] {
        answers.push((pass, outposts.ok("GET", &route(&stocked, ""), None).await));
        // Another Outpost's hardware is its own: listing one never fills in another.
        let other = outposts.ok("GET", &route(&empty, ""), None).await;
        assert_eq!(
            other.get("Assets"),
            Some(&json!([])),
            "case {pass:?}: an Outpost with no installed hardware reports no asset, got {other}"
        );
    }
    let (_, first) = &answers[0];
    for (pass, answer) in &answers {
        assert_eq!(
            answer, first,
            "case {pass:?}: every listing of one Outpost's assets must answer the same"
        );
    }

    let after = outposts.get_outpost(&stocked).await;
    assert_eq!(
        after, before,
        "listing assets must not change the Outpost itself"
    );
    let held = outposts.store().lock().outposts[&stocked].assets.len();
    assert_eq!(
        held,
        installed.len(),
        "listing assets must leave the recorded hardware alone: expected {} held, got {held}",
        installed.len()
    );
}

/// Rule 4's refusal half: an identifier that names no Outpost is 404
/// `NotFoundException`; one that is no Outpost identifier at all, a `MaxResults`
/// outside the modelled range, and a `NextToken` this service did not issue for this
/// list are 400 `ValidationException`.
#[tokio::test]
async fn an_absent_outpost_is_not_found_and_bad_input_is_refused() {
    let outposts = Outposts::new();
    // An Outpost that existed, had hardware recorded on it, and was then deleted: its
    // assets went with it, so the route answers as it does for one that never was.
    let deleted = an_outpost(&outposts, "deleted").await;
    record(&outposts, &deleted, &installed(), "deleted");
    outposts
        .ok("DELETE", &format!("/outposts/{deleted}"), None)
        .await;

    // A token this service issued for ANOTHER list is not a boundary of this one.
    let outposts_page = outposts.ok("GET", "/outposts?MaxResults=1", None).await;
    let other_token = outposts_page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or("1##outposts")
        .to_string();
    let live = an_outpost(&outposts, "live").await;

    let cases: Vec<Refusal> = vec![
        refusal(
            "a well-formed Outpost id that names nothing",
            "GET",
            route(ABSENT_OUTPOST_ID, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "the ARN form of an Outpost that names nothing",
            "GET",
            route(&format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}"), ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "an Outpost that was deleted",
            "GET",
            route(&deleted, ""),
            404,
            "NotFoundException",
        ),
        refusal(
            "a label that is no identifier at all",
            "GET",
            route("not-an-outpost-id", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a site id",
            "GET",
            route(ABSENT_SITE_ID, ""),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id whose suffix is not hex",
            "GET",
            route("op-zzzzzzzzzzzzzzzzz", ""),
            400,
            "ValidationException",
        ),
        refusal(
            "a ListOutposts token replayed on ListAssets",
            "GET",
            route(&live, &format!("?NextToken={}", query_value(&other_token))),
            400,
            "ValidationException",
        ),
        refusal(
            "a token this service never issued at all",
            "GET",
            route(&live, "?NextToken=1%23%23nonsense"),
            400,
            "ValidationException",
        ),
        refusal(
            "MaxResults below the modelled minimum of 1",
            "GET",
            route(&live, "?MaxResults=0"),
            400,
            "ValidationException",
        ),
        refusal(
            "MaxResults above the modelled maximum of 1000",
            "GET",
            route(&live, "?MaxResults=1001"),
            400,
            "ValidationException",
        ),
    ];

    outposts.expect_refusals(&cases).await;
}

/// Rule 4's message half: a not-found refusal is the identifier ECHO
/// `outpostIdentifier=<value as supplied>` — the ARN form reads its ARN back, not the
/// bare id the service resolved it to.
#[tokio::test]
async fn a_not_found_outpost_echoes_the_identifier_the_request_supplied() {
    let outposts = Outposts::new();
    let absent_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let cases: Vec<(&str, &str)> = vec![("by bare id", ABSENT_OUTPOST_ID), ("by ARN", &absent_arn)];

    for (case, identifier) in &cases {
        let reply = outposts.call("GET", &route(identifier, ""), None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let expected = format!("outpostIdentifier={identifier}");
        let actual = reply
            .body
            .get("Message")
            .or_else(|| reply.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the identifier echo {expected:?}, got {actual:?}"
        );
    }
}
