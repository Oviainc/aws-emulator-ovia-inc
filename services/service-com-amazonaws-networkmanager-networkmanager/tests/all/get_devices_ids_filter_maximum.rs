// SPDX-License-Identifier: CC0-1.0

//! The published maximum on `GetDevices$DeviceIds`, judged on the NUMBER OF VALUES the
//! request carries, driven over HTTP.
//!
//! The rule under test: a list-valued filter member whose documentation publishes a
//! maximum number of values is refused — with the modelled `ValidationException`
//! (HTTP 400) naming that member, and no page at all — as soon as the request carries more
//! than that many values, and the count is of the values the list carries, whatever those
//! values name and whatever else the request asks for. `DeviceIds` is "One or more device
//! IDs. The maximum is 10." (`smithy_query query="doc" shape="GetDevicesRequest"
//! member="DeviceIds"`) and the modelled `DeviceIdList` expresses the bound nowhere — it
//! carries a length on its member STRING only (`smithy_query shape=DeviceIdList`) — so the
//! refusal is the handler's to answer rather than the pre-handler constraint walk's.
//!
//! What the sibling suites of this class already cover, and this one does not repeat:
//! `crate::filter_id_list_maximum` spans every id-list filter of this service by COUNT
//! alone over the fixture's own ids, `crate::site_ids_filter_maximum` crosses `SiteIds`
//! with the rest of the query string, and `crate::link_ids_filter_maximum` crosses
//! `LinkIds` and `DeviceIds` with the value selectors their listings take, over stored ids
//! and ids naming nothing. This suite covers what none of them does: ids that DO name
//! stored devices but of a DIFFERENT global network, and a list mixing ids that resolve
//! with ids that do not — so the bound is shown to count the values the request carries
//! rather than the devices they resolve to inside the listing; the bound against the rest
//! of `GetDevices`' query string (a page size, and a page token this service never issued),
//! which fixes the order the two refusals are decided in; and a filter AT the bound
//! followed across every page it answers.
//!
//! The lengths, the bound and the refusal are the shared ones `crate::filter_maximum_table`
//! states — reused, not restated — so this suite cannot disagree with its siblings about
//! any of them.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 route, the
//! repeated-query binding of the list and the error envelope are exercised as a client hits
//! them. Nothing reaches into `crate::state`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::json;

use crate::device_lifecycle::create_device_ok;
use crate::filter_maximum_table::{
    assert_filter_refusal, unknown_ids, EXTRAS, FIXTURE_COUNT, MAX_FILTER_VALUES, ROWS,
};
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::resource_tagging::escape;

/// One row of the provenance table: what the ids the filter carries name.
struct Provenance {
    /// The row's name, which every assertion of it reports.
    name: &'static str,
    /// The values this row draws the lengths of [`ROWS`] from.
    values: Vec<String>,
}

/// The fixture: two global networks, each holding [`FIXTURE_COUNT`] devices, and the
/// provenance rows over them.
struct Fixture {
    /// The global network the listing under test addresses.
    network: String,
    /// Its devices, in the order `GetDevices` answers them.
    devices: Vec<String>,
    rows: Vec<Provenance>,
}

async fn fixture(emulator: &Emulator) -> Fixture {
    let network = emulator.create_global_network("devices under test").await;
    let other = emulator.create_global_network("another global network").await;

    let mut devices: Vec<String> = Vec::new();
    let mut elsewhere: Vec<String> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        devices.push(
            create_device_ok(
                emulator,
                &network,
                json!({ "Description": format!("device {index}") }),
            )
            .await,
        );
        elsewhere.push(
            create_device_ok(
                emulator,
                &other,
                json!({ "Description": format!("other device {index}") }),
            )
            .await,
        );
    }

    let unknown = unknown_ids("device");

    // A mix: the listed network's own ids and ids that name nothing, alternating, so no
    // prefix of it is all of one kind.
    let mixed: Vec<String> = (0..FIXTURE_COUNT)
        .map(|index| {
            if index % 2 == 0 {
                devices[index / 2 % devices.len()].clone()
            } else {
                unknown[index / 2 % unknown.len()].clone()
            }
        })
        .collect();

    let rows = vec![
        Provenance {
            name: "devices of another global network",
            values: elsewhere,
        },
        Provenance {
            name: "stored ids mixed with ids naming nothing",
            values: mixed,
        },
    ];

    Fixture {
        network,
        devices,
        rows,
    }
}

/// The devices URI of `network`, carrying one `deviceIds` parameter per value plus `extra`
/// (already a query fragment, so it travels unescaped).
fn uri_for(network: &str, values: &[String], extra: &str) -> String {
    let mut query: Vec<String> = values
        .iter()
        .map(|id| format!("deviceIds={}", escape(id)))
        .collect();
    if !extra.is_empty() {
        query.push(extra.to_string());
    }
    let base = format!("/global-networks/{network}/devices");
    if query.is_empty() {
        base
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

#[tokio::test]
async fn the_device_ids_filter_counts_the_values_the_request_carries_whatever_they_name() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for row in &fixture.rows {
        for length in &ROWS {
            let case = format!("{}/{}", row.name, length.name);
            let values = length.values(&row.values);
            let (status, body) = emulator.get(&uri_for(&fixture.network, &values, "")).await;

            if values.len() > MAX_FILTER_VALUES {
                // Past the published maximum the request is refused whatever its values
                // name — the bound belongs to the member, not to what the ids resolve to.
                assert_filter_refusal(&case, status, &body, "DeviceIds", "Devices");
                continue;
            }

            // At or inside the maximum the listing runs and selects the global network's
            // devices the filter names — none of another network's, and only the resolving
            // half of a mixed list. An absent filter matches every device of the network.
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} with {body}"
            );
            let expected: Vec<String> = if values.is_empty() {
                fixture.devices.clone()
            } else {
                fixture
                    .devices
                    .iter()
                    .filter(|id| values.contains(id))
                    .cloned()
                    .collect()
            };
            assert_eq!(
                ids_of(&body, "Devices", "DeviceId"),
                expected,
                "case {case}: expected exactly {expected:?}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_device_ids_maximum_is_judged_ahead_of_the_rest_of_the_query_string() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for extra in &EXTRAS {
        // One past the maximum: the refusal names the filter member, whatever page size or
        // page token travels with it — so nothing of the listing, not even the token walk,
        // runs ahead of the bound.
        let case = format!("one past the maximum with {}", extra.name);
        let values = &fixture.devices[..=MAX_FILTER_VALUES];
        let (status, body) = emulator
            .get(&uri_for(&fixture.network, values, extra.query))
            .await;
        assert_filter_refusal(&case, status, &body, "DeviceIds", "Devices");

        // The same request one value shorter is INSIDE the bound, so the rest of the query
        // string is what decides it: a forged page token is refused on its own member, and
        // a page size answers a page no longer than it asked for. That is the proof the
        // refusal above is the LENGTH and nothing else.
        let case = format!("at the maximum with {}", extra.name);
        let values = &fixture.devices[..MAX_FILTER_VALUES];
        let (status, body) = emulator
            .get(&uri_for(&fixture.network, values, extra.query))
            .await;
        if extra.forged_token {
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected ValidationException, got {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"],
                json!("NextToken"),
                "case {case}: the refusal must name NextToken, got {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let expected: Vec<String> = match extra.limit {
            Some(limit) => values.iter().take(limit).cloned().collect(),
            None => values.to_vec(),
        };
        assert_eq!(
            ids_of(&body, "Devices", "DeviceId"),
            expected,
            "case {case}: expected exactly {expected:?}, got {body}"
        );
    }

    // Nothing any refused request carried changed what a later caller sees.
    let (status, body) = emulator
        .get(&format!("/global-networks/{}/devices", fixture.network))
        .await;
    assert_eq!(
        status, 200,
        "case unfiltered read after the refusals: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_of(&body, "Devices", "DeviceId"),
        fixture.devices,
        "case unfiltered read after the refusals: expected every created device, got {body}"
    );
}

#[tokio::test]
async fn a_device_ids_filter_at_the_maximum_is_honoured_across_every_page() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    // A filter AT the bound is served in full, however small the pages it is read in: the
    // bound caps the request, not the answer.
    const PAGE_SIZE: usize = 3;
    let values = &fixture.devices[..MAX_FILTER_VALUES];
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=MAX_FILTER_VALUES {
        let case = format!("page {page} of a filter at the maximum");
        let extra = match &token {
            Some(value) => format!("maxResults={PAGE_SIZE}&nextToken={value}"),
            None => format!("maxResults={PAGE_SIZE}"),
        };
        let (status, body) = emulator
            .get(&uri_for(&fixture.network, values, &extra))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let listed = ids_of(&body, "Devices", "DeviceId");
        assert!(
            !listed.is_empty() && listed.len() <= PAGE_SIZE,
            "case {case}: expected 1..={PAGE_SIZE} devices, got {listed:?}"
        );
        seen.extend(listed);
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        token, None,
        "case paging a filter at the maximum: the listing never ran out of pages, got {seen:?}"
    );
    assert_eq!(
        seen,
        values.to_vec(),
        "case paging a filter at the maximum: expected exactly {values:?}, got {seen:?}"
    );
}
