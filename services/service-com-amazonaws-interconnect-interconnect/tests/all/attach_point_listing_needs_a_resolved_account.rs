// SPDX-License-Identifier: CC0-1.0

//! `ListAttachPoints` when this emulator cannot read the account's attach points at all,
//! driven over the wire through the shared `crate::emulator` harness.
//!
//! RULE: an answer this emulator could not resolve from the state it is derived from is
//! not published as a resolved one — a `@required` output list is never answered empty
//! or short, which would ASSERT that those are all the records that exist; the operation
//! refuses with its modelled server-side exception instead, the same class every other
//! reader of the same unreadable state refuses with.
//!
//! The rows differ along what the rule generalises over: the Region the request is
//! signed for, the page size it asks for (both published bounds of `MaxResults` and no
//! page size at all), whether it carries a page cursor, and how many gateways the
//! account actually holds — none, one and several — because none of that is knowable
//! through a front door that cannot read them.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::emulator::{code, identifiers, message, text, Emulator};

/// The Regions the rows sign for, so the rule is checked across the endpoint addressed
/// as well as across the request: each Region's catalog publishes its own Environment.
const REGIONS: [&str; 3] = ["us-east-1", "eu-west-1", "ap-northeast-1"];

/// The page sizes the rows ask for: no `maxResults` at all, and each published bound of
/// `MaxResults` (`smithy_query(query="shape",shape="MaxResults")`: `@range(min: 1, max:
/// 100)`).
const PAGE_SIZES: [Option<i64>; 3] = [None, Some(1), Some(100)];

/// The gateways the account comes to hold as the sweeps below progress: it holds none at
/// first, then one, then several, so no assertion can be keyed to an account whose
/// listing would have been empty anyway.
const GATEWAY_ROWS: [(&str, Option<i64>); 3] = [
    ("ic-unreadable-listing-a", None),
    ("ic unreadable listing b", Some(64_512)),
    ("ic-unreadable-listing-c", Some(4_200_000_000)),
];

/// How a listing that could not resolve the account's attach points is worded — written
/// out here rather than imported, so this suite is a second witness to the sentence the
/// service answers with.
const ATTACH_POINTS_UNLISTABLE: &str =
    "the attach points of this account could not be resolved, so the attach points valid for \
     the requested environment could not be listed";

/// One `ListAttachPoints` request, as a `(status, body)` pair so a refusal is readable.
async fn list(
    emulator: &Emulator,
    region: &str,
    environment_id: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("environmentId".to_string(), json!(environment_id));
    if let Some(max_results) = max_results {
        request.insert("maxResults".to_string(), json!(max_results));
    }
    if let Some(token) = next_token {
        request.insert("nextToken".to_string(), json!(token));
    }
    emulator
        .ic_in(region, "ListAttachPoints", Value::Object(request))
        .await
}

/// The `attachPoints` of a listing body.
fn page(body: &Value) -> Vec<Value> {
    body["attachPoints"].as_array().cloned().unwrap_or_default()
}

/// A page cursor the FULLY MOUNTED door issued for this environment, so a row can send a
/// token the service really minted — or `None` while the account holds too few attach
/// points for a first page of one to leave any behind.
async fn issued_cursor(mounted: &Emulator, region: &str, environment_id: &str) -> Option<String> {
    let (status, body) = list(mounted, region, environment_id, Some(1), None).await;
    assert_eq!(
        status, 200,
        "setup {region:?}/{environment_id:?}: expected the mounted door to serve a first page, got {status} with {body:?}"
    );
    body["nextToken"].as_str().map(str::to_string)
}

// ---------------------------------------------------------------------------
// An account whose attach points cannot be read at all
// ---------------------------------------------------------------------------

/// RULE: `ListAttachPoints` answers the modelled `InterconnectServerException` (500)
/// when the account's attach points cannot be resolved, whatever the request asked for
/// and whatever the account holds, and publishes no page at all — because
/// `ListAttachPointsResponse$attachPoints` is `@required`, so an empty or short page
/// would claim those are all the attach points the account has. The fully mounted door
/// over the SAME state serves the listing, so what the refusal reports is the unreadable
/// peer rather than the request.
#[tokio::test]
async fn an_unresolvable_account_refuses_the_listing_instead_of_claiming_it_holds_none() {
    // Two front doors over ONE store: the fully mounted emulator, which owns the
    // account's Direct Connect gateways, and one with Interconnect alone behind it,
    // which can reach no service holding them.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mounted = Emulator::on_engine(Arc::clone(&engine));
    let degraded = Emulator::interconnect_only(engine);

    let mut owned: BTreeSet<String> = BTreeSet::new();
    // Sweep once per account holding: none, the first gateway, then the rest.
    for created in [0usize, 1, GATEWAY_ROWS.len()] {
        while owned.len() < created {
            let row = GATEWAY_ROWS[owned.len()];
            owned.extend(mounted.ready_gateways(&[row]).await);
        }

        for region in REGIONS {
            for environment in mounted.environments_in(region).await {
                let environment_id = text(&environment, "environmentId");

                // Positive control: the same state, read through the door that CAN
                // resolve the account's gateways, is served as a listing.
                let (status, served) = list(&mounted, region, &environment_id, None, None).await;
                assert_eq!(
                    status, 200,
                    "case {region:?}/{environment_id:?} holding {created}: expected the mounted door to serve the listing, got {status} with {served:?}"
                );
                assert_eq!(
                    identifiers(&page(&served)),
                    owned,
                    "case {region:?}/{environment_id:?} holding {created}: expected the account's gateways to be the listing, got {:?}",
                    page(&served)
                );

                let cursor = issued_cursor(&mounted, region, &environment_id).await;
                for max_results in PAGE_SIZES {
                    for token in [None, cursor.as_deref()] {
                        let case = format!(
                            "{region}/{environment_id} holding {created}, maxResults {max_results:?}, nextToken {token:?}"
                        );
                        let (status, body) =
                            list(&degraded, region, &environment_id, max_results, token).await;
                        assert_eq!(
                            status, 500,
                            "case {case:?}: expected the modelled 500 when the account's attach points cannot be resolved, got {status} with {body:?}"
                        );
                        assert!(
                            code(&body).contains("InterconnectServerException"),
                            "case {case:?}: expected InterconnectServerException, got {:?}",
                            code(&body)
                        );
                        assert_eq!(
                            message(&body),
                            ATTACH_POINTS_UNLISTABLE,
                            "case {case:?}: expected the unresolvable-account wording, got {:?}",
                            message(&body)
                        );
                        assert!(
                            page(&body).is_empty() && body["nextToken"].is_null(),
                            "case {case:?}: expected a refusal to publish no page at all, got {body:?}"
                        );
                        assert!(
                            !message(&body).contains(&environment_id)
                                && owned.iter().all(|gateway| !message(&body).contains(gateway)),
                            "case {case:?}: expected the refusal to name nothing it could not read, got {:?}",
                            message(&body)
                        );
                    }
                }
            }
        }
    }

    // Nothing the degraded door was asked changed what the mounted one reports: the
    // listing is a read.
    for region in REGIONS {
        for environment in mounted.environments_in(region).await {
            let environment_id = text(&environment, "environmentId");
            let (status, served) = list(&mounted, region, &environment_id, None, None).await;
            assert_eq!(
                status, 200,
                "case {region:?}/{environment_id:?}: expected the mounted listing to survive the refusals, got {status} with {served:?}"
            );
            assert_eq!(
                identifiers(&page(&served)),
                owned,
                "case {region:?}/{environment_id:?}: expected the account's gateways to be unchanged, got {:?}",
                page(&served)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The refusal is about the state that could not be read, not about the request
// ---------------------------------------------------------------------------

/// RULE: the unresolvable refusal is reached only for the state this emulator could not
/// read — a request this service can judge from its OWN state is still refused as the
/// caller's fault behind the same degraded door: an `environmentId` no catalog of that
/// Region holds is the 400 every environment-resolving operation gives, and a
/// `nextToken` this operation did not issue is the modelled validation 400.
#[tokio::test]
async fn a_request_the_service_can_judge_itself_is_still_the_callers_fault() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mounted = Emulator::on_engine(Arc::clone(&engine));
    let degraded = Emulator::interconnect_only(engine);

    let region = REGIONS[0];
    let environment_id = text(
        mounted
            .environments_in(region)
            .await
            .first()
            .expect("the catalog publishes an environment"),
        "environmentId",
    );
    let other_region_id = text(
        mounted
            .environments_in(REGIONS[1])
            .await
            .first()
            .expect("the catalog publishes an environment"),
        "environmentId",
    );

    // Rows differ in WHICH part of the request this service can judge without reading
    // the account's gateways at all.
    let rows: Vec<(&str, String, Option<&str>, &str)> = vec![
        (
            "an environment id no catalog holds",
            "mce-aws-nobody-nowhere".to_string(),
            None,
            "InterconnectClientException",
        ),
        (
            "an environment id of another Region's catalog",
            other_region_id,
            None,
            "InterconnectClientException",
        ),
        (
            "a nextToken this operation did not issue",
            environment_id.clone(),
            Some("not-a-token-this-service-minted"),
            "InterconnectValidationException",
        ),
    ];

    for (case, environment, token, expected) in rows {
        let (status, body) = list(&degraded, region, &environment, None, token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the caller's own fault to stay a 400 behind the degraded door, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(expected),
            "case {case:?}: expected {expected}, got {:?}",
            code(&body)
        );
    }

    // The same requests get the same answers through the fully mounted door, so what
    // decides them is the request and not which services are behind the door.
    for (case, environment, token) in [
        (
            "an environment id no catalog holds",
            "mce-aws-nobody-nowhere".to_string(),
            None,
        ),
        (
            "a nextToken this operation did not issue",
            environment_id,
            Some("not-a-token-this-service-minted"),
        ),
    ] {
        let (status, body) = list(&mounted, region, &environment, None, token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the mounted door to refuse the same request the same way, got {status} with {body:?}"
        );
    }
}
