// SPDX-License-Identifier: CC0-1.0

//! How `DescribeGlobalNetworks` resolves the ids its request names, driven over HTTP.
//!
//! The rule under test: an id-list member bound as a query SELECTOR on a reporting
//! operation is intersected with stored state — the operation answers 200 with the
//! subset of resources whose ids it holds, and a value naming no stored record
//! contributes nothing instead of failing the read, so a filter naming only such values
//! is an EMPTY page, never a not-found. `GlobalNetworkIds` is such a member
//! (`smithy_query operation="DescribeGlobalNetworks"`: `GET /global-networks`, code 200,
//! `GlobalNetworkIds` bound as `@httpQuery("globalNetworkIds")`, output
//! `{GlobalNetworks, NextToken}`, `@paginated(items: GlobalNetworks)`), exactly like the
//! selectors the other listings of this service take (`SiteIds`, `LinkIds`, `DeviceIds`,
//! `ConnectionIds`, `TransitGatewayArns`). Only the resource a request ADDRESSES — a
//! required member bound on the path, as `GetSites` binds `GlobalNetworkId` — is resolved
//! and can answer the modelled `ResourceNotFoundException`.
//!
//! The table spans the class the rule generalises over: how many ids the request names,
//! how many of them name a stored network, WHERE the unmatched one sits in the list, what
//! shape it has (an id of this kind never minted, an id of another kind of this service, a
//! string that is no id at all), and whether it named a global network that has since been
//! deleted. A second table varies the page size and the position of the unmatched id
//! together, since selection happens before any page is cut; a third pins that an unusable
//! page token is still refused; a fourth drives the delete-completion probe the empty-page
//! answer is what makes possible.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use crate::global_network_lifecycle::{error_code, ids_of, Emulator, ListingCase};

/// An id of the minted shape (`global-network-<17 lowercase hex>`) that names nothing:
/// the leading `f` puts it above anything the monotonic id counter can mint.
const NEVER_MINTED: &str = "global-network-f0000000000000001";
/// A second such id, so a row can name two unmatched ids at once.
const NEVER_MINTED_TOO: &str = "global-network-f0000000000000002";
/// An id of another resource kind of this service: a well-formed id that names no GLOBAL
/// NETWORK, which the member's own constraints cannot catch (`GlobalNetworkId` is
/// `length 0..50, pattern ^[\s\S]*$` — `smithy_query shape="GlobalNetworkIdList"`).
const OTHER_KIND: &str = "site-f0000000000000003";
/// A string that is no id of this service at all; the member accepts it, so the answer is
/// still decided by the intersection with state.
const NOT_AN_ID: &str = "nothing-like-an-id";

/// The URI a request addresses: the listing plus one `globalNetworkIds` parameter per id
/// (the way a client binds a list to `@httpQuery`), and `maxResults` when a row sets one.
fn uri_for(ids: &[String], max_results: Option<usize>) -> String {
    let mut query: Vec<String> = ids
        .iter()
        .map(|id| format!("globalNetworkIds={id}"))
        .collect();
    if let Some(size) = max_results {
        query.push(format!("maxResults={size}"));
    }
    if query.is_empty() {
        return "/global-networks".to_string();
    }
    format!("/global-networks?{}", query.join("&"))
}

/// Ask `uri` for a page, assert it is a 200, and answer the ids it carries.
async fn page_ids(emulator: &Emulator, case: &str, uri: &str) -> Vec<String> {
    let (status, body) = emulator.get(uri).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {body}"
    );
    ids_of(&body, "GlobalNetworks", "GlobalNetworkId")
}

/// Every id `ids` selects, walked across pages of `max_results` items, so a row can assert
/// the whole selection whatever the page size is.
async fn every_selected_id(
    emulator: &Emulator,
    case: &str,
    ids: &[String],
    max_results: Option<usize>,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut uri = uri_for(ids, max_results);
        if let Some(next) = token.as_deref() {
            let separator = if uri.contains('?') { '&' } else { '?' };
            uri = format!("{uri}{separator}nextToken={next}");
        }
        let (status, body) = emulator.get(&uri).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let page = ids_of(&body, "GlobalNetworks", "GlobalNetworkId");
        if let Some(size) = max_results {
            assert!(
                page.len() <= size,
                "case {case:?}: a page may carry at most {size} items, got {body}"
            );
        }
        seen.extend(page);
        match body["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn describe_global_networks_answers_the_stored_subset_its_filter_names() {
    let emulator = Emulator::new();
    let first = emulator.create_global_network("first").await;
    let second = emulator.create_global_network("second").await;
    let third = emulator.create_global_network("third").await;
    let mut every = vec![first.clone(), second.clone(), third.clone()];
    every.sort();
    let mut first_and_second = vec![first.clone(), second.clone()];
    first_and_second.sort();

    // A global network that existed and was deleted: its id is well-formed and named a
    // real resource, so only state — not the shape of the string — can decide the answer.
    let gone = emulator.create_global_network("to be deleted").await;
    let (status, body) = emulator.delete(&format!("/global-networks/{gone}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    // Each row is the ids the request names and the page it must answer, in key order.
    let rows: Vec<ListingCase> = vec![
        ("no ids named at all", vec![], every.clone()),
        (
            "one id that names a stored network",
            vec![second.clone()],
            vec![second.clone()],
        ),
        ("every stored id", every.clone(), every.clone()),
        (
            "one id repeated",
            vec![third.clone(), third.clone()],
            vec![third.clone()],
        ),
        (
            "one id that was never minted",
            vec![NEVER_MINTED.to_string()],
            vec![],
        ),
        (
            "an unmatched id after a matching one",
            vec![first.clone(), NEVER_MINTED.to_string()],
            vec![first.clone()],
        ),
        (
            "an unmatched id before a matching one",
            vec![NEVER_MINTED.to_string(), first.clone()],
            vec![first.clone()],
        ),
        (
            "an unmatched id between two matching ones",
            vec![first.clone(), NEVER_MINTED.to_string(), second.clone()],
            first_and_second.clone(),
        ),
        (
            "two unmatched ids around a matching one",
            vec![
                NEVER_MINTED_TOO.to_string(),
                first.clone(),
                NEVER_MINTED.to_string(),
            ],
            vec![first.clone()],
        ),
        (
            "only unmatched ids",
            vec![NEVER_MINTED.to_string(), NEVER_MINTED_TOO.to_string()],
            vec![],
        ),
        (
            "an id of another resource kind of this service",
            vec![OTHER_KIND.to_string()],
            vec![],
        ),
        (
            "a string that is no id of this service",
            vec![NOT_AN_ID.to_string()],
            vec![],
        ),
        (
            "an id of another kind mixed with a matching one",
            vec![OTHER_KIND.to_string(), third.clone()],
            vec![third.clone()],
        ),
        (
            "the id of a deleted global network",
            vec![gone.clone()],
            vec![],
        ),
        (
            "a deleted id mixed with a live one",
            vec![first.clone(), gone.clone()],
            vec![first.clone()],
        ),
        (
            "a deleted id mixed with an id never minted",
            vec![gone.clone(), NEVER_MINTED.to_string()],
            vec![],
        ),
    ];

    for (case, ids, want) in rows {
        let answered = page_ids(&emulator, case, &uri_for(&ids, None)).await;
        assert_eq!(
            answered, want,
            "case {case:?}: expected exactly {want:?}, got {answered:?}"
        );
    }

    // The reads above changed nothing a later caller sees: the three live networks are
    // still listed, and the deleted one is still gone.
    let case = "unfiltered read after the filtered ones";
    let answered = page_ids(&emulator, case, "/global-networks").await;
    assert_eq!(
        answered, every,
        "case {case:?}: expected exactly {every:?}, got {answered:?}"
    );
}

#[tokio::test]
async fn an_id_that_names_nothing_is_dropped_wherever_it_sits_and_whatever_the_page_size() {
    let emulator = Emulator::new();
    // Enough networks that a small page cannot carry them all, so a row can put the
    // unmatched id past where the first page ends.
    let mut known: Vec<String> = Vec::new();
    for index in 0..4 {
        known.push(
            emulator
                .create_global_network(&format!("network {index}"))
                .await,
        );
    }

    // Every position the one unmatched id can take among `count` matching ids, at every
    // page size from one to past the list: selection precedes paging, so the answer is
    // always the matching subset and never a refusal.
    for count in 0..=known.len() {
        let mut expected: Vec<String> = known[..count].to_vec();
        expected.sort();
        for position in 0..=count {
            for max_results in [None, Some(1), Some(2), Some(5)] {
                let mut ids: Vec<String> = known[..count].to_vec();
                ids.insert(position, NEVER_MINTED.to_string());
                let case = format!(
                    "{count} matching ids, unmatched at {position}, maxResults={max_results:?}"
                );
                let answered = every_selected_id(&emulator, &case, &ids, max_results).await;
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected exactly {expected:?}, got {answered:?}"
                );
            }
        }
    }

    // The same requests with the unmatched id dropped answer the same pages, so the id
    // that names nothing is what the filter ignores — not the request's shape.
    for count in 1..=known.len() {
        let mut expected: Vec<String> = known[..count].to_vec();
        expected.sort();
        let case = format!("{count} matching ids, none unmatched");
        let answered = page_ids(&emulator, &case, &uri_for(&known[..count], None)).await;
        assert_eq!(
            answered, expected,
            "case {case:?}: expected exactly {expected:?}, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_this_service_did_not_mint_is_refused_whatever_the_filter_names() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("precedence").await;
    // A token of a form this service never mints, as `global_network_lifecycle` pins for
    // the token refusal itself.
    let bad_token = "zzzz";

    // The filter names no resource the request ADDRESSES, so nothing about it can turn an
    // unusable token into another answer: both filters are the same token refusal.
    for (case, filter) in [
        ("an unmatched id carried with an unmintable token", NEVER_MINTED.to_string()),
        ("a matching id carried with an unmintable token", network.clone()),
    ] {
        let (status, body) = emulator
            .get(&format!(
                "/global-networks?globalNetworkIds={filter}&nextToken={bad_token}"
            ))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["Fields"][0]["Name"], serde_json::json!("NextToken"),
            "case {case:?}: the refusal must name NextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn the_delete_completion_probe_keeps_answering_a_page_once_the_network_is_gone() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("round trip").await;

    // Before the delete the very same request is a page of one.
    let uri = uri_for(std::slice::from_ref(&network), None);
    let case = "described before the delete";
    let answered = page_ids(&emulator, case, &uri).await;
    assert_eq!(
        answered,
        vec![network.clone()],
        "case {case:?}: expected exactly the created network, got {answered:?}"
    );

    let (status, body) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    // After it the same request keeps answering 200 with an empty page — the probe a
    // caller polls to observe the delete — however many times it is repeated.
    for attempt in 0..3 {
        let case = format!("polled after the delete, attempt {attempt}");
        let answered = page_ids(&emulator, &case, &uri).await;
        assert!(
            answered.is_empty(),
            "case {case:?}: expected an empty page, got {answered:?}"
        );
    }

    // The resource is nonetheless gone for the operations that ADDRESS it: an update of
    // the deleted id is the modelled not-found, which is what tells the empty page apart
    // from the id still being live.
    let (status, body) = emulator
        .patch(
            &format!("/global-networks/{network}"),
            serde_json::json!({ "Description": "post-delete" }),
        )
        .await;
    let case = "updated after the delete";
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
}
