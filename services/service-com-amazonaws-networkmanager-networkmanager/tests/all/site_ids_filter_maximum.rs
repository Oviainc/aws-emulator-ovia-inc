// SPDX-License-Identifier: CC0-1.0

//! The documented maximum on `GetSites$SiteIds`, judged on the REQUEST and nothing else,
//! driven over HTTP.
//!
//! The rule under test: a list-valued filter member whose documentation publishes a
//! maximum number of values is refused — with the modelled `ValidationException`
//! (HTTP 400) naming that member, and no page at all — as soon as the request carries more
//! than that many values, whatever else the query string carries and whether or not the
//! values name anything stored. `SiteIds` is "One or more site IDs. The maximum is 10."
//! (`smithy_query query="doc" shape="GetSitesRequest" member="SiteIds"`), and the modelled
//! `SiteIdList` expresses the bound nowhere — it carries a length on its member STRING
//! only (`smithy_query shape=SiteIdList`) — so the refusal is the handler's to answer.
//!
//! `crate::filter_maximum_table` states the bound, the length rows, the extra parameters
//! and the refusal once for every suite of this class; `crate::filter_id_list_maximum`
//! spans the filters of the five listings whose pages this service keys by resource id.
//! This suite covers what that table does not:
//!
//! * the bound is judged on the request ALONE, so the rest of the query string cannot
//!   lift it (a page size, including the largest one the model allows) and cannot be
//!   answered ahead of it (a page token this service never issued is a refusal of
//!   `NextToken`, but only for a filter INSIDE the bound: past it the answer names
//!   `SiteIds`);
//! * values that name nothing stored are bounded exactly as stored ids are, so the
//!   refusal does not depend on the listing resolving anything;
//! * the same published maximum on `GetTransitGatewayRegistrations$TransitGatewayArns`
//!   ("The Amazon Resource Names (ARNs) of one or more transit gateways. The maximum is
//!   10.", `smithy_query query="doc" shape="GetTransitGatewayRegistrationsRequest"
//!   member="TransitGatewayArns"`), a filter of a listing this service keys by ARN rather
//!   than by resource id — the rule is the member's published maximum, not the shape of
//!   the values it carries.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! query bindings and the error envelopes are exercised as a client hits them. Nothing
//! reaches into `crate::state`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::json;

use crate::filter_maximum_table::{
    assert_filter_refusal, unknown_ids, Extra, EXTRAS, FIXTURE_COUNT, KINDS, MAX_FILTER_VALUES,
    ROWS,
};
use crate::global_network_lifecycle::{error_code, ids_of, Emulator};
use crate::resource_tagging::escape;
use crate::transit_gateway_peering::{own_account, transit_gateway_arn};

/// The URI a request addresses: the listing, one `siteIds` parameter per filter value the
/// way a client binds a list to `@httpQuery`, then the row's other parameter.
fn sites_uri(network: &str, values: &[String], extra: &Extra) -> String {
    let mut query: Vec<String> = values
        .iter()
        .map(|id| format!("siteIds={}", escape(id)))
        .collect();
    if !extra.query.is_empty() {
        query.push(extra.query.to_string());
    }
    let base = format!("/global-networks/{network}/sites");
    if query.is_empty() {
        base
    } else {
        format!("{base}?{}", query.join("&"))
    }
}

#[tokio::test]
async fn the_site_ids_filter_maximum_is_judged_on_the_request_alone() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("site filter bound").await;
    let mut stored: Vec<String> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        stored.push(
            emulator
                .create_site(&network, &format!("site {index}"))
                .await,
        );
    }
    let unknown = unknown_ids("site");

    for kind in &KINDS {
        let available = if kind.unknown { &unknown } else { &stored };
        for row in &ROWS {
            for extra in &EXTRAS {
                let case = format!("{}/{}/{}", kind.name, row.name, extra.name);
                let values = row.values(available);
                let (status, body) = emulator.get(&sites_uri(&network, &values, extra)).await;

                if values.len() > MAX_FILTER_VALUES {
                    // Past the published maximum the length is the answer, ahead of every
                    // other thing the query string could be refused for.
                    assert_filter_refusal(&case, status, &body, "SiteIds", "Sites");
                    continue;
                }

                if extra.forged_token {
                    // Inside the bound the token is reached, and the refusal names it —
                    // which is what shows the refusal above is the LENGTH.
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

                // At or inside the bound the listing is computed: an absent filter matches
                // every site of the global network, a filter matches the sites it names
                // (none, for ids that name nothing), and a page size bounds the page.
                assert_eq!(
                    status, 200,
                    "case {case}: expected 200, got {status} with {body}"
                );
                let mut expected: Vec<String> = if values.is_empty() {
                    stored.clone()
                } else if kind.unknown {
                    Vec::new()
                } else {
                    values.clone()
                };
                let full = expected.len();
                if let Some(limit) = extra.limit {
                    expected.truncate(limit);
                }
                assert_eq!(
                    ids_of(&body, "Sites", "SiteId"),
                    expected,
                    "case {case}: expected exactly {expected:?}, got {body}"
                );
                assert_eq!(
                    body["NextToken"].is_string(),
                    expected.len() < full,
                    "case {case}: a token must be carried only when sites remain, got {body}"
                );
            }
        }
    }

    // Nothing any refused request carried changed what a later caller sees.
    let (status, body) = emulator
        .get(&format!("/global-networks/{network}/sites"))
        .await;
    assert_eq!(
        status, 200,
        "case unfiltered read after the refusals: expected 200, got {status} with {body}"
    );
    assert_eq!(
        ids_of(&body, "Sites", "SiteId"),
        stored,
        "case unfiltered read after the refusals: expected every created site, got {body}"
    );
}

#[tokio::test]
async fn the_registration_listing_filter_takes_the_same_published_maximum() {
    let emulator = Emulator::new();
    let network = emulator
        .create_global_network("registration filter bound")
        .await;

    // The ids are zero-padded to the widest published width, so the ARNs sort in the order
    // they were registered and the page order is the fixture order.
    let mut stored: Vec<String> = Vec::new();
    for index in 0..FIXTURE_COUNT {
        let arn = transit_gateway_arn("aws", "us-west-2", &own_account(), index + 1, 17);
        let (status, body) = emulator
            .post(
                &format!("/global-networks/{network}/transit-gateway-registrations"),
                json!({ "TransitGatewayArn": arn }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case register {arn}: expected 200, got {status} with {body}"
        );
        stored.push(arn);
    }

    for row in &ROWS {
        let case = row.name;
        let values = row.values(&stored);
        let query: Vec<String> = values
            .iter()
            .map(|arn| format!("transitGatewayArns={}", escape(arn)))
            .collect();
        let base = format!("/global-networks/{network}/transit-gateway-registrations");
        let uri = if query.is_empty() {
            base
        } else {
            format!("{base}?{}", query.join("&"))
        };
        let (status, body) = emulator.get(&uri).await;

        if values.len() > MAX_FILTER_VALUES {
            assert_filter_refusal(
                case,
                status,
                &body,
                "TransitGatewayArns",
                "TransitGatewayRegistrations",
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        let expected: Vec<String> = if values.is_empty() {
            stored.clone()
        } else {
            values.clone()
        };
        assert_eq!(
            ids_of(&body, "TransitGatewayRegistrations", "TransitGatewayArn"),
            expected,
            "case {case}: expected exactly {expected:?}, got {body}"
        );
    }
}
