// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `ListAttachPoints`, driven over the wire through the shared
//! `crate::emulator` harness (Interconnect and Direct Connect on one engine, as the
//! assembled emulator mounts them).
//!
//! Every assertion here is about a RULE, checked over a table built from the LIVE
//! seeded Environment catalog rather than from a copy of it: a row differs in the
//! environment, the provider kind, how many attach points that environment offers,
//! the page size, and the shape of the identifier the caller sends.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::emulator::{text, Emulator, ATTACH_POINT_TYPE};

/// `MaxResults` is modelled `@range(min: 1, max: 100)`, so these are the published
/// bounds a page size may take, and 0 / 101 are the first values past them.
const MAX_RESULTS_BOUNDS: [i64; 2] = [1, 100];
const MAX_RESULTS_PAST_BOUNDS: [i64; 2] = [0, 101];

/// `DirectConnectGatewayAttachPoint`: length exactly 36 and
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
fn is_dxgw_identifier(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    value.len() == 36
        && groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

// ---------------------------------------------------------------------------
// The attach points an Environment offers
// ---------------------------------------------------------------------------

/// RULE: `ListAttachPoints` reports the attach points stored for the Environment the
/// caller names, each with every member `AttachPointDescriptor` requires, and reading
/// it changes nothing a later caller sees.
#[tokio::test]
async fn every_environment_reports_descriptors_the_model_allows() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    assert!(
        !catalog.is_empty(),
        "case \"catalog\": expected the seeded Environment catalog to be non-empty, got {catalog:?}"
    );

    // One row per environment: they differ in the provider kind, the location and
    // how many attach points the environment offers.
    for environment in &catalog {
        let id = text(environment, "environmentId");
        let listed = emulator.attach_points(&id).await;
        assert!(
            !listed.is_empty(),
            "case {id:?}: expected the environment's attach points, got {listed:?}"
        );

        // The largest page a caller may ask for is the last one: `nextToken` comes back
        // only when results remain, so at `MaxResults`' published ceiling there is none
        // and the single page is the whole listing.
        let one_page = emulator
            .ic_ok(
                "ListAttachPoints",
                json!({ "environmentId": id, "maxResults": MAX_RESULTS_BOUNDS[1] }),
            )
            .await;
        assert!(
            one_page["nextToken"].is_null(),
            "case {id:?}: expected the largest page to be the last one, got {one_page:?}"
        );
        assert_eq!(
            one_page["attachPoints"].as_array().cloned().unwrap_or_default(),
            listed,
            "case {id:?}: expected the largest page to carry the whole listing, got {:?}",
            one_page["attachPoints"]
        );

        let mut identifiers = BTreeSet::new();
        for descriptor in &listed {
            assert_eq!(
                text(descriptor, "type"),
                ATTACH_POINT_TYPE,
                "case {id:?}: expected the only modelled AttachPointType, got {:?}",
                descriptor["type"]
            );
            let identifier = text(descriptor, "identifier");
            assert!(
                is_dxgw_identifier(&identifier),
                "case {id:?}: expected a DirectConnectGateway UUID identifier, got {identifier:?}"
            );
            assert!(
                !text(descriptor, "name").is_empty(),
                "case {id:?}: expected a descriptive name, got {:?}",
                descriptor["name"]
            );
            assert!(
                identifiers.insert(identifier.clone()),
                "case {id:?}: expected distinct identifiers, got {identifier:?} twice"
            );
        }

        // A read must not change what a later caller sees: the second call reports
        // byte-identical descriptors in the same order.
        let again = emulator.attach_points(&id).await;
        assert_eq!(
            again, listed,
            "case {id:?}: expected a repeated read to report the same attach points, got {again:?}"
        );
    }

    // Two different environments are two different sets of attach points: the
    // listing is keyed by the environment the caller names, not shared.
    let mut per_environment: Vec<(String, Vec<String>)> = Vec::new();
    for environment in &catalog {
        let id = text(environment, "environmentId");
        let identifiers: Vec<String> = emulator
            .attach_points(&id)
            .await
            .iter()
            .map(|descriptor| text(descriptor, "identifier"))
            .collect();
        per_environment.push((id, identifiers));
    }
    for (left_id, left) in &per_environment {
        for (right_id, right) in &per_environment {
            if left_id == right_id {
                continue;
            }
            let shared: Vec<&String> = left.iter().filter(|id| right.contains(id)).collect();
            assert!(
                shared.is_empty(),
                "case {left_id:?}/{right_id:?}: expected each environment's own attach points, got {shared:?} in both"
            );
        }
    }
}

/// RULE: a paged walk, at any page size, yields exactly the unpaged listing in the
/// same order, and a `nextToken` comes back exactly when results remain.
#[tokio::test]
async fn attach_point_pages_walk_each_environment_without_repeats_or_gaps() {
    let emulator = Emulator::new();

    for environment in emulator.environments().await {
        let id = text(&environment, "environmentId");
        let all = emulator.attach_points(&id).await;

        // Rows differ in page size: every size from one item to one past the whole
        // listing, so both the "exactly fits" and the "one page only" cases are
        // covered however many attach points the environment offers.
        for page_size in 1..=(all.len() + 1) {
            let mut walked: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = Map::new();
                request.insert("environmentId".to_string(), json!(id));
                request.insert("maxResults".to_string(), json!(page_size));
                if let Some(token) = &token {
                    request.insert("nextToken".to_string(), json!(token));
                }
                let listed = emulator
                    .ic_ok("ListAttachPoints", Value::Object(request))
                    .await;
                let page = listed["attachPoints"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    page.len() <= page_size,
                    "case {id:?}/page size {page_size}: expected at most {page_size} per page, got {}",
                    page.len()
                );
                walked.extend(page);
                token = listed["nextToken"].as_str().map(str::to_string);
                pages += 1;
                assert!(
                    pages <= all.len() + 2,
                    "case {id:?}/page size {page_size}: expected the walk to terminate, got {pages} pages"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                walked, all,
                "case {id:?}/page size {page_size}: expected the paged walk to equal the unpaged listing, got {walked:?}"
            );
        }

        // Each published `MaxResults` bound is accepted; the first value past either
        // bound is refused by the modelled range rather than silently clamped.
        for max_results in MAX_RESULTS_BOUNDS {
            let listed = emulator
                .ic_ok(
                    "ListAttachPoints",
                    json!({ "environmentId": id, "maxResults": max_results }),
                )
                .await;
            let page = listed["attachPoints"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !page.is_empty() && page.len() as i64 <= max_results,
                "case {id:?}/maxResults {max_results}: expected a page of 1..={max_results}, got {}",
                page.len()
            );
        }
        for max_results in MAX_RESULTS_PAST_BOUNDS {
            let (status, body) = emulator
                .ic(
                    "ListAttachPoints",
                    json!({ "environmentId": id, "maxResults": max_results }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {id:?}/maxResults {max_results}: expected HTTP 400 for a page size outside the modelled range, got {status} with {body:?}"
            );
        }

        // A cursor this operation did not issue is refused, not answered with an
        // empty page: a hand-made token, a bare identifier, the other list
        // operations' cursors, and this operation's own tag with no key.
        for token in [
            "not-a-cursor",
            id.as_str(),
            "env:env-csp-azure-us-east-1",
            "conn:mcc-00000000",
            "ap:",
        ] {
            let (status, body) = emulator
                .ic(
                    "ListAttachPoints",
                    json!({ "environmentId": id, "nextToken": token }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {id:?}/token {token:?}: expected HTTP 400 for a cursor ListAttachPoints never issued, got {status} with {body:?}"
            );
            assert!(
                text(&body, "__type").contains("InterconnectValidationException"),
                "case {id:?}/token {token:?}: expected InterconnectValidationException, got {body:?}"
            );
        }
    }
}

/// RULE: attach points are listed FOR an Environment, so an `environmentId` the
/// catalog does not hold is refused — `InterconnectClientException` (HTTP 400), the
/// same classification `GetEnvironment` gives that id — never an empty page.
#[tokio::test]
async fn unknown_environment_is_refused_rather_than_answered_empty() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let known = text(&catalog[0], "environmentId");

    // Rows differ in the SHAPE of the absent id: unrelated, a prefix of a real one, a
    // real one with a suffix, a case variant, and the longest id the member allows
    // (`EnvironmentId` is `@length(min: 1, max: 64)`).
    let mut cases: Vec<String> = vec![
        "env-does-not-exist".to_string(),
        known[..known.len() - 1].to_string(),
        format!("{known}-extra"),
        known.to_ascii_uppercase(),
        "e".repeat(64),
        "e".to_string(),
    ];
    cases.retain(|candidate| *candidate != known);

    for case in cases {
        let (status, body) = emulator
            .ic("ListAttachPoints", json!({ "environmentId": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an environment the catalog does not hold, got {status} with {body:?}"
        );
        assert!(
            text(&body, "__type").contains("InterconnectClientException"),
            "case {case:?}: expected InterconnectClientException, got {body:?}"
        );
        // And the same id is still refused by GetEnvironment, so the two reads agree.
        let (get_status, get_body) = emulator
            .ic("GetEnvironment", json!({ "id": case }))
            .await;
        assert_eq!(
            get_status, 400,
            "case {case:?}: expected GetEnvironment to agree the environment is absent, got {get_status} with {get_body:?}"
        );
    }

    // An id outside the member's length is refused by the modelled constraint
    // (`@length(min: 1, max: 64)`), one past each published bound.
    for case in ["".to_string(), "e".repeat(65)] {
        let (status, body) = emulator
            .ic("ListAttachPoints", json!({ "environmentId": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for an environmentId outside the modelled length, got {status} with {body:?}",
            case.len()
        );
    }
}

/// RULE: every attach point `ListAttachPoints` reports for an Environment is usable as
/// that Environment's `CreateConnection$attachPoint`, and the stored connection reports
/// the attach point it was created with — so the listing and the connection agree.
#[tokio::test]
async fn a_listed_attach_point_creates_a_connection_that_reports_it_back() {
    let emulator = Emulator::new();

    let mut rows = 0;
    for environment in emulator.environments().await {
        let id = text(&environment, "environmentId");
        // `Environment$bandwidths.available` is "the list of currently available
        // bandwidths": an environment offering none (the `unavailable` state) takes no
        // new connections, so there is nothing to round-trip there.
        let available: Vec<String> = environment["bandwidths"]["available"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|bandwidth| bandwidth.as_str().unwrap_or_default().to_string())
            .collect();
        if available.is_empty() {
            continue;
        }

        // One row per (attach point, bandwidth) pair the environment offers.
        for (index, descriptor) in emulator.attach_points(&id).await.iter().enumerate() {
            let identifier = text(descriptor, "identifier");
            let bandwidth = &available[index % available.len()];
            let attach_point = json!({ "directConnectGateway": identifier });
            let created = emulator
                .ic_ok(
                    "CreateConnection",
                    json!({
                        "environmentId": id,
                        "bandwidth": bandwidth,
                        "attachPoint": attach_point,
                    }),
                )
                .await;
            let connection = &created["connection"];
            let case = format!("{id} / {identifier} / {bandwidth}");
            assert_eq!(
                connection["attachPoint"], attach_point,
                "case {case:?}: expected the listed attach point on the created connection, got {:?}",
                connection["attachPoint"]
            );

            // Round trip: the reader reports the same attach point the listing gave.
            let fetched = emulator
                .ic_ok(
                    "GetConnection",
                    json!({ "identifier": text(connection, "id") }),
                )
                .await;
            assert_eq!(
                fetched["connection"]["attachPoint"], attach_point,
                "case {case:?}: expected GetConnection to report the same attach point, got {:?}",
                fetched["connection"]["attachPoint"]
            );

            // And the connection is found by filtering on that attach point, so one
            // record backs the listing, the get and the filter.
            let filtered = emulator
                .ic_ok(
                    "ListConnections",
                    json!({ "environmentId": id, "attachPoint": attach_point }),
                )
                .await;
            let ids: Vec<String> = filtered["connections"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|summary| text(summary, "id"))
                .collect();
            assert!(
                ids.contains(&text(connection, "id")),
                "case {case:?}: expected the connection among the ones filtered on its attach point, got {ids:?}"
            );
            rows += 1;
        }

        // Listing attach points is @readonly: creating connections against them does
        // not add, remove or reorder what the Environment offers.
        let after = emulator.attach_points(&id).await;
        assert_eq!(
            after.len(),
            emulator.attach_points(&id).await.len(),
            "case {id:?}: expected the attach-point listing to be unchanged by creates, got {after:?}"
        );
    }
    assert!(
        rows >= 2,
        "case \"coverage\": expected the table to span at least two attach points, got {rows}"
    );
}
