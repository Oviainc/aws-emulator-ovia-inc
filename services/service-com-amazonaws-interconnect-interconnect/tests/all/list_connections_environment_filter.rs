// SPDX-License-Identifier: CC0-1.0

//! The `ListConnections` `environmentId` filter, driven over the wire the way a client
//! drives it: `POST /` with `X-Amz-Target: Interconnect.<Operation>` on `awsJson1_0`,
//! through the same `MultiServiceRouter::dispatch` the socket front door uses.
//!
//! The RULE under test: this member FILTERS the results — "Filter the results to only
//! include Connection objects on the given Environment"
//! (`smithy_query(query="operation",operation="ListConnections")`) — so it selects on a
//! member each stored connection carries and is never resolved in the Environment
//! catalog: a value no connection of the called Region carries answers HTTP 200 with an
//! empty page, which is what the service is recorded answering a filtered listing that
//! selects nothing, and never a refusal. The `environmentId` of an operation whose
//! SUBJECT is an Environment (`GetEnvironment`, `ListAttachPoints`, `CreateConnection`)
//! is the other half of the same rule: there the id names the resource the call is about,
//! so it IS resolved and an id the called Region's catalog does not hold is refused with
//! the 400 `InterconnectClientException`. Both halves are asserted here, over the same
//! ids, so the two cannot drift apart.
//!
//! Each assertion is checked over a TABLE whose rows differ along what the rule
//! generalises over: the shape of the id (including an id of another Region's catalog and
//! one this Region offers but holds no connection on), the other filters the same request
//! carries, whether the store holds connections at all, and the Region the request is
//! signed for.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use service_com_amazonaws_interconnect_interconnect::state::environment_not_found_message;

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, remote_account_for, text,
    Emulator, DEFAULT_SIGNING_REGION,
};

/// The Regions the rows are driven through, so no answer below depends on the harness
/// default Region: the default one, one that also creates connections, and one that
/// creates none at all.
const REGIONS: [&str; 3] = [DEFAULT_SIGNING_REGION, "eu-west-1", "ap-southeast-2"];

/// The Regions the tables create connections in; `REGIONS[2]` deliberately creates none,
/// so its own catalog's Environment is an OFFERED id that selects nothing.
const CREATING_REGIONS: [&str; 2] = [REGIONS[0], REGIONS[1]];

/// How many bandwidths of an Environment a row creates on, so a filtered listing
/// carries more than one connection without the suite creating the whole catalog.
const BANDWIDTHS_PER_ENVIRONMENT: usize = 2;

/// The error code this service answers an `environmentId` that names the resource a call
/// is ABOUT and resolves to no Environment with.
const CLIENT_CODE: &str = "InterconnectClientException";

/// The error code every fault in the REQUEST itself is answered with.
const VALIDATION_CODE: &str = "ValidationException";

/// `EnvironmentId` is `@length(min: 1, max: 64)`
/// (`smithy_query(query="operation",operation="ListConnections")`), so these are the
/// shortest and the longest value the member can carry at all.
const ENVIRONMENT_ID_LENGTH: (usize, usize) = (1, 64);

/// A `DirectConnectGatewayAttachPoint` inside the member's `@pattern` that names no
/// gateway of the account: what the create rows below carry, so what refuses them is the
/// Environment and nothing else.
const UNOWNED_GATEWAY_UUID: &str = "12345678-1234-1234-1234-123456789012";

/// `ListConnections` filtered on `environment_id` through `region`'s endpoint, with
/// `extra` members written over the request, as a `(status, body)` pair.
async fn list_filtered(
    emulator: &Emulator,
    region: &str,
    environment_id: &str,
    extra: &Value,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(environment_id));
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request.insert(member, value);
    }
    emulator
        .ic_in(region, "ListConnections", Value::Object(request))
        .await
}

/// The ids of every connection `region`'s endpoint holds, walking the pages so the
/// answer is the whole listing however it is windowed.
async fn stored_ids(emulator: &Emulator, region: &str) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = Map::new();
        request.insert("maxResults".to_string(), json!(100));
        if let Some(token) = &token {
            request.insert("nextToken".to_string(), json!(token));
        }
        let listed = emulator
            .ic_ok_in(region, "ListConnections", Value::Object(request))
            .await;
        for summary in listed["connections"].as_array().cloned().unwrap_or_default() {
            all.insert(text(&summary, "id"));
        }
        token = listed["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return all;
        }
    }
}

/// The `environmentId`s `region`'s endpoint offers.
async fn offered_in(emulator: &Emulator, region: &str) -> Vec<String> {
    emulator
        .environments_in(region)
        .await
        .iter()
        .map(|environment| text(environment, "environmentId"))
        .collect()
}

/// The ids no connection of `region` can ever carry, each labelled by the SHAPE it takes:
/// unrelated, a prefix of an offered one, an offered one with a suffix, a case variant of
/// one, the two ends of the member's own `@length`, and — the row that is a real
/// Environment somewhere — an id of ANOTHER Region's catalog, which only that Region's
/// connections carry.
async fn ids_no_connection_carries(emulator: &Emulator, region: &str) -> Vec<(String, String)> {
    let offered = offered_in(emulator, region).await;
    let known = offered
        .first()
        .cloned()
        .expect("the Region's endpoint offers a catalog");

    let mut rows = vec![
        (
            "an id unrelated to any offered one",
            "env-does-not-exist".to_string(),
        ),
        (
            "the shortest id the member admits",
            "e".repeat(ENVIRONMENT_ID_LENGTH.0),
        ),
        (
            "the longest id the member admits",
            "e".repeat(ENVIRONMENT_ID_LENGTH.1),
        ),
        ("an offered id with a suffix", format!("{known}-extra")),
        (
            "a case variant of an offered id",
            known.to_ascii_uppercase(),
        ),
    ];
    if known.chars().count() > 1 {
        rows.push((
            "a prefix of an offered id",
            known[..known.len() - 1].to_string(),
        ));
    }

    // An Environment of another Region's catalog: a published id that this endpoint's
    // connections cannot carry, because a connection is created on an Environment of the
    // Region it lands in.
    for other in REGIONS.iter().filter(|other| **other != region) {
        for elsewhere in offered_in(emulator, other).await {
            if !offered.contains(&elsewhere) {
                rows.push(("an offered id of another Region's catalog", elsewhere));
                break;
            }
        }
    }

    rows.retain(|(_, candidate)| !offered.contains(candidate));
    assert!(
        rows.len() >= 6,
        "case {region:?}: expected the table to span the shapes of an id nothing carries, got {rows:?}"
    );
    rows.into_iter()
        .map(|(label, id)| (label.to_string(), id))
        .collect()
}

/// The other filters the same request carries, so the answer is shown not to depend on
/// this filter being the only one: none, each of the three remaining filters, a page size,
/// and all of them at once.
fn companion_filters() -> Vec<(&'static str, Value)> {
    vec![
        ("no other filter", json!({})),
        ("a state filter", json!({ "state": "available" })),
        (
            "a provider filter",
            json!({ "provider": { "cloudServiceProvider": "azure" } }),
        ),
        (
            "an attachPoint filter",
            json!({ "attachPoint": { "directConnectGateway": UNOWNED_GATEWAY_UUID } }),
        ),
        ("a page size", json!({ "maxResults": 1 })),
        (
            "every other filter at once",
            json!({
                "state": "requested",
                "provider": { "cloudServiceProvider": "azure" },
                "attachPoint": { "directConnectGateway": UNOWNED_GATEWAY_UUID },
                "maxResults": 100,
            }),
        ),
    ]
}

/// Assert `(status, body)` is a served listing carrying no connection at all.
fn assert_empty_page(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected the filtered listing to be served, got {status} with {body:?}"
    );
    assert_eq!(
        body["connections"].as_array().map(Vec::len).unwrap_or(0),
        0,
        "case {case:?}: expected no connection to match, got {:?}",
        body["connections"]
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: expected no cursor on a page nothing matched, got {:?}",
        body["nextToken"]
    );
}

/// Assert `(status, body)` is the refusal an operation whose SUBJECT is an Environment
/// gives an id the called Region's catalog does not hold.
fn assert_environment_refused(case: &str, environment_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for an environmentId this endpoint does not offer, got {status} with {body:?}"
    );
    assert!(
        code(body).contains(CLIENT_CODE),
        "case {case:?}: expected {CLIENT_CODE}, got {:?}",
        code(body)
    );
    assert_eq!(
        message(body),
        environment_not_found_message(environment_id),
        "case {case:?}: expected the unresolvable-environment wording, got {:?}",
        message(body)
    );
}

/// Create `BANDWIDTHS_PER_ENVIRONMENT` connections on every Environment the creating
/// Regions offer, answering, per Region, the connection ids created on each
/// `environmentId`.
async fn create_everywhere(
    emulator: &Emulator,
) -> BTreeMap<String, BTreeMap<String, BTreeSet<String>>> {
    let attach_point = emulator.attach_point().await;
    let mut created: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for region in CREATING_REGIONS {
        let catalog = emulator.environments_in(region).await;
        for environment in creatable_environments(&catalog) {
            let environment_id = text(&environment, "environmentId");
            for bandwidth in available_bandwidths(&environment)
                .into_iter()
                .take(BANDWIDTHS_PER_ENVIRONMENT)
            {
                let mut request = Map::new();
                request.insert("environmentId".to_string(), json!(environment_id));
                request.insert("bandwidth".to_string(), json!(bandwidth));
                request.insert(
                    "attachPoint".to_string(),
                    json!({ "directConnectGateway": attach_point }),
                );
                request.insert(
                    "description".to_string(),
                    json!(format!("filter row {bandwidth}")),
                );
                if let Some(remote_account) = remote_account_for(&environment) {
                    request.insert("remoteAccount".to_string(), remote_account);
                }
                let answered = emulator
                    .ic_ok_in(region, "CreateConnection", Value::Object(request))
                    .await;
                created
                    .entry(region.to_string())
                    .or_default()
                    .entry(environment_id.clone())
                    .or_default()
                    .insert(text(&answered["connection"], "id"));
            }
        }
    }
    assert_eq!(
        created.len(),
        CREATING_REGIONS.len(),
        "setup: expected every creating Region to have placed a connection, got {created:?}"
    );
    created
}

/// RULE: the `environmentId` FILTER selects on the member each stored connection carries,
/// so a value no connection of the called Region carries is answered with HTTP 200 and an
/// empty page — whatever shape the value takes, whatever else the request filters on, and
/// whether or not the endpoint holds connections.
#[tokio::test]
async fn an_environment_filter_no_connection_carries_answers_an_empty_page() {
    let emulator = Emulator::new();

    // With nothing stored at all, every id selects nothing — the offered ones included,
    // which is the answer "nothing of yours is on that Environment".
    for region in REGIONS {
        let mut rows = ids_no_connection_carries(&emulator, region).await;
        for offered in offered_in(&emulator, region).await {
            rows.push(("an offered id of this Region's catalog".to_string(), offered));
        }
        for (shape, id) in rows {
            for (filter, extra) in companion_filters() {
                let case = format!("{region} / {shape} ({id}) / {filter} / nothing stored");
                let (status, body) = list_filtered(&emulator, region, &id, &extra).await;
                assert_empty_page(&case, status, &body);
            }
        }
    }

    // And with the store holding connections on the Environments the catalogs offer: a
    // value no connection of THIS Region carries still selects nothing, so the empty page
    // is about the selection and not about the store being empty.
    let created = create_everywhere(&emulator).await;
    for region in REGIONS {
        for (shape, id) in ids_no_connection_carries(&emulator, region).await {
            for (filter, extra) in companion_filters() {
                let case = format!("{region} / {shape} ({id}) / {filter} / connections stored");
                let (status, body) = list_filtered(&emulator, region, &id, &extra).await;
                assert_empty_page(&case, status, &body);
            }
        }
    }

    // The listings changed nothing a later caller sees: every Region still holds exactly
    // the connections it created.
    for region in REGIONS {
        let expected: BTreeSet<String> = created
            .get(region)
            .map(|by_environment| by_environment.values().flatten().cloned().collect())
            .unwrap_or_default();
        let held = stored_ids(&emulator, region).await;
        assert_eq!(
            held, expected,
            "case {region:?}: expected the filtered listings to have changed nothing, got {held:?}"
        );
    }
}

/// RULE, the other half: where an `environmentId` names the Environment the call is
/// ABOUT, it is resolved in the called Region's catalog and an id that catalog does not
/// hold is refused with the 400 `InterconnectClientException` — so the empty page above is
/// the filter's answer, not this service declining to judge an Environment at all.
#[tokio::test]
async fn the_same_id_is_refused_where_it_names_the_environment_the_call_is_about() {
    let emulator = Emulator::new();

    for region in REGIONS {
        let offered = offered_in(&emulator, region).await;
        for (shape, id) in ids_no_connection_carries(&emulator, region).await {
            // The rows that are not in this Region's catalog at all are the ones the
            // Environment-subject operations refuse; an id of another Region's catalog is
            // among them, since a catalog is a function of its own Region.
            assert!(
                !offered.contains(&id),
                "case {region:?} / {shape}: expected {id:?} to be outside this Region's catalog"
            );
            for (operation, request) in [
                ("GetEnvironment", json!({ "id": id })),
                ("ListAttachPoints", json!({ "environmentId": id })),
                (
                    "CreateConnection",
                    json!({
                        "environmentId": id,
                        "bandwidth": "1Gbps",
                        "attachPoint": { "directConnectGateway": UNOWNED_GATEWAY_UUID },
                    }),
                ),
            ] {
                let case = format!("{region} / {shape} ({id}) / {operation}");
                let (status, body) = emulator.ic_in(region, operation, request).await;
                assert_environment_refused(&case, &id, status, &body);
            }

            // The same id, in the same Region, on the listing whose member only filters:
            // served, with an empty page.
            let case = format!("{region} / {shape} ({id}) / ListConnections");
            let (status, body) = list_filtered(&emulator, region, &id, &json!({})).await;
            assert_empty_page(&case, status, &body);
        }
    }
}

/// RULE: a value the filter DOES select on narrows the listing to exactly the connections
/// carrying it, inside the called Region's own set.
#[tokio::test]
async fn a_matching_environment_filter_narrows_the_listing_within_the_called_region() {
    let emulator = Emulator::new();
    let created = create_everywhere(&emulator).await;

    for region in REGIONS {
        let by_environment = created.get(region).cloned().unwrap_or_default();
        for environment_id in offered_in(&emulator, region).await {
            let case = format!("{region} / {environment_id}");
            let listed = emulator
                .ic_ok_in(
                    region,
                    "ListConnections",
                    json!({ "environmentId": environment_id, "maxResults": 100 }),
                )
                .await;
            let summaries = listed["connections"].as_array().cloned().unwrap_or_default();
            let got: BTreeSet<String> = summaries
                .iter()
                .map(|summary| text(summary, "id"))
                .collect();
            let want = by_environment
                .get(&environment_id)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                got, want,
                "case {case:?}: expected exactly this Environment's connections of this Region, got {got:?}"
            );

            // Every summary the filter returned carries the filtered value, so the
            // narrowing is on the stored member and not on the page boundary.
            for summary in &summaries {
                assert_eq!(
                    text(summary, "environmentId"),
                    environment_id,
                    "case {case:?}: expected every summary to carry the filtered Environment, got {summary:?}"
                );
            }

            // The same value with a state nothing is in selects nothing: the filters are
            // conjunctive, so an empty page here is about the pair and not about either
            // member being ignored.
            let (status, body) = list_filtered(
                &emulator,
                region,
                &environment_id,
                &json!({ "state": "failed" }),
            )
            .await;
            assert_empty_page(&format!("{case} / state failed"), status, &body);
        }
    }
}

/// RULE: a fault the REQUEST carries is still refused — the filter being permissive about
/// a value nothing carries is not a blanket 200 for every listing.
#[tokio::test]
async fn a_request_fault_is_still_refused_on_the_filtered_listing() {
    let emulator = Emulator::new();
    let unmatched = "env-does-not-exist";

    // Rows differ in the KIND of fault: a cursor this operation never issued, a
    // `maxResults` outside `MaxResults`' `@range(1, 100)`, and a union member outside its
    // own modelled constraint in each of the two unions this input carries.
    let faults: Vec<(&str, Value)> = vec![
        (
            "a cursor this operation never issued",
            json!({ "nextToken": "not-a-cursor" }),
        ),
        (
            "a maxResults above the modelled range",
            json!({ "maxResults": 101 }),
        ),
        (
            "an attachPoint member value its union cannot hold",
            json!({ "attachPoint": { "directConnectGateway": "abc" } }),
        ),
        (
            "a provider member longer than its modelled length",
            json!({ "provider": { "cloudServiceProvider": "p".repeat(33) } }),
        ),
    ];

    for region in REGIONS {
        for (fault, extra) in &faults {
            let case = format!("{region} / {fault}");
            let (status, body) = list_filtered(&emulator, region, unmatched, extra).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert!(
                code(&body).contains(VALIDATION_CODE),
                "case {case:?}: expected the request's own fault to be reported, got {:?} with {body:?}",
                code(&body)
            );
        }
    }
}
