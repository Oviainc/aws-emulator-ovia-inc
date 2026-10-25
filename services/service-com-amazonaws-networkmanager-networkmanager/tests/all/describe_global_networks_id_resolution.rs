// SPDX-License-Identifier: CC0-1.0

//! `DescribeGlobalNetworks` resolving the ids its request names, driven over HTTP.
//!
//! The rule under test: where an id-list member is the only reference to a resource a
//! reporting request carries, the operation RESOLVES each id it names against stored
//! state — an id naming nothing is the modelled `ResourceNotFoundException` (HTTP 404)
//! identifying that id and its resource type, never a narrower page. A listing whose ids
//! are a plain filter models no not-found at all (`smithy_query
//! operation="ListAttachments,ListPeerings"` carries no `ResourceNotFoundException`),
//! which is what tells the two apart; `DescribeGlobalNetworks` binds no path resource
//! (`http = GET /global-networks`) and models the 404, so `GlobalNetworkIds` is the
//! member that resolves.
//!
//! The table spans the class the rule generalises over: how many ids the request names,
//! how many of them resolve, WHERE the unresolvable one sits in the list, what shape it
//! has (an id of this kind never minted, an id of another kind of this service, a string
//! that is no id at all), and whether it named a global network that has since been
//! deleted. A second table varies the page size and the position of the unresolvable id
//! together, since resolution happens before any page is cut, and a third case pins that
//! same precedence against an unusable page token.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use crate::global_network_lifecycle::{error_code, ids_of, Emulator, ListingCase};
use crate::resource_not_found_message::{assert_not_found_answer, GLOBAL_NETWORK};

/// An id of the minted shape (`global-network-<17 lowercase hex>`) that names nothing:
/// the leading `f` puts it above anything the monotonic id counter can mint.
const NEVER_MINTED: &str = "global-network-f0000000000000001";
/// A second such id, so a row can name two unresolvable ids and pin WHICH one is reported.
const NEVER_MINTED_TOO: &str = "global-network-f0000000000000002";
/// An id of another resource kind of this service: a well-formed id that names no GLOBAL
/// NETWORK, which the member's own constraints cannot catch (`GlobalNetworkId` is
/// `length 0..50, pattern ^[\s\S]*$` — `smithy_query shape="GlobalNetworkIdList"`).
const OTHER_KIND: &str = "site-f0000000000000003";
/// A string that is no id of this service at all; the member accepts it, so the answer is
/// still decided by resolution against state.
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

/// Assert `uri` answers the modelled not-found for `unknown`.
///
/// The 404, the modelled code, the constant message and the identifying detail in
/// `ResourceId` / `ResourceType` are the shared assertion every not-found of this service
/// is held to (`crate::resource_not_found_message`); what a refused LISTING owes on top is
/// that it carries no page at all.
async fn assert_not_found(emulator: &Emulator, case: &str, uri: &str, unknown: &str) {
    let body = assert_not_found_answer(
        emulator,
        case,
        "GET",
        uri,
        None,
        unknown,
        GLOBAL_NETWORK,
    )
    .await;
    assert!(
        body["GlobalNetworks"].is_null(),
        "case {case:?}: a refused request must carry no page, got {body}"
    );
}

#[tokio::test]
async fn describe_global_networks_resolves_every_id_its_request_names() {
    let emulator = Emulator::new();
    let first = emulator.create_global_network("first").await;
    let second = emulator.create_global_network("second").await;
    let third = emulator.create_global_network("third").await;
    let mut every = vec![first.clone(), second.clone(), third.clone()];
    every.sort();

    // A global network that existed and was deleted: its id is well-formed and named a
    // real resource, so only state — not the shape of the string — can decide the answer.
    let gone = emulator.create_global_network("to be deleted").await;
    let (status, body) = emulator.delete(&format!("/global-networks/{gone}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    // `Ok(page)` is the page the request must answer, in key order; `Err(id)` is the
    // not-found refusal identifying `id`.
    let rows: Vec<ListingCase> = vec![
        ("no ids named at all", vec![], Ok(every.clone())),
        ("one id that resolves", vec![second.clone()], Ok(vec![second.clone()])),
        ("every id, all resolving", every.clone(), Ok(every.clone())),
        (
            "one id repeated",
            vec![third.clone(), third.clone()],
            Ok(vec![third.clone()]),
        ),
        (
            "one id that was never minted",
            vec![NEVER_MINTED.to_string()],
            Err(NEVER_MINTED.to_string()),
        ),
        (
            "an unresolvable id after a resolving one",
            vec![first.clone(), NEVER_MINTED.to_string()],
            Err(NEVER_MINTED.to_string()),
        ),
        (
            "an unresolvable id before a resolving one",
            vec![NEVER_MINTED.to_string(), first.clone()],
            Err(NEVER_MINTED.to_string()),
        ),
        (
            "an unresolvable id between two resolving ones",
            vec![first.clone(), NEVER_MINTED.to_string(), second.clone()],
            Err(NEVER_MINTED.to_string()),
        ),
        (
            "two unresolvable ids: the first in request order is reported",
            vec![
                NEVER_MINTED_TOO.to_string(),
                first.clone(),
                NEVER_MINTED.to_string(),
            ],
            Err(NEVER_MINTED_TOO.to_string()),
        ),
        (
            "an id of another resource kind of this service",
            vec![OTHER_KIND.to_string()],
            Err(OTHER_KIND.to_string()),
        ),
        (
            "a string that is no id of this service",
            vec![NOT_AN_ID.to_string()],
            Err(NOT_AN_ID.to_string()),
        ),
        (
            "the id of a deleted global network",
            vec![gone.clone()],
            Err(gone.clone()),
        ),
        (
            "a deleted id mixed with a live one",
            vec![first.clone(), gone.clone()],
            Err(gone.clone()),
        ),
    ];

    for (case, ids, want) in rows {
        let uri = uri_for(&ids, None);
        match want {
            Ok(page) => {
                let (status, body) = emulator.get(&uri).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status} with {body}"
                );
                assert_eq!(
                    ids_of(&body, "GlobalNetworks", "GlobalNetworkId"),
                    page,
                    "case {case:?}: expected exactly {page:?}, got {body}"
                );
            }
            Err(unknown) => assert_not_found(&emulator, case, &uri, &unknown).await,
        }
    }

    // A refused request changed nothing a later caller sees: the three live networks are
    // still listed, and the deleted one is still gone.
    let (status, body) = emulator.get("/global-networks").await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {body}",
        "unfiltered read after the refusals"
    );
    assert_eq!(
        ids_of(&body, "GlobalNetworks", "GlobalNetworkId"),
        every,
        "case {:?}: expected exactly {every:?}, got {body}",
        "unfiltered read after the refusals"
    );
}

#[tokio::test]
async fn an_unresolvable_id_is_refused_wherever_it_sits_and_whatever_the_page_size() {
    let emulator = Emulator::new();
    // Enough networks that a small page cannot carry them all, so a row can put the
    // unresolvable id past where the page would end.
    let mut known: Vec<String> = Vec::new();
    for index in 0..4 {
        known.push(
            emulator
                .create_global_network(&format!("network {index}"))
                .await,
        );
    }

    // Every position the one unresolvable id can take among `count` resolving ids, at
    // every page size from one to past the list: resolution precedes paging, so the
    // answer is the same 404 in all of them.
    for count in 0..=known.len() {
        for position in 0..=count {
            for max_results in [None, Some(1), Some(2), Some(5)] {
                let mut ids: Vec<String> = known[..count].to_vec();
                ids.insert(position, NEVER_MINTED.to_string());
                let case = format!(
                    "{count} resolving ids, unresolvable at {position}, maxResults={max_results:?}"
                );
                let uri = uri_for(&ids, max_results);
                assert_not_found(&emulator, &case, &uri, NEVER_MINTED).await;
            }
        }
    }

    // The same requests with the unresolvable id dropped are answered as pages, so the
    // refusals above are the id and not the request's shape.
    for count in 1..=known.len() {
        let mut expected: Vec<String> = known[..count].to_vec();
        expected.sort();
        let uri = uri_for(&known[..count], None);
        let (status, body) = emulator.get(&uri).await;
        let case = format!("{count} resolving ids, none unresolvable");
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            ids_of(&body, "GlobalNetworks", "GlobalNetworkId"),
            expected,
            "case {case:?}: expected exactly {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn an_unresolvable_id_is_refused_before_a_page_token_this_service_did_not_mint() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("precedence").await;
    // A token of a form this service never mints, as `global_network_lifecycle` pins for
    // the token refusal itself.
    let bad_token = "zzzz";

    // The resource the request addresses is resolved before the rest of the request is
    // judged (the order the operations that bind their resource on the path use), so an
    // unresolvable id answers the not-found even when the token is unusable too.
    assert_not_found(
        &emulator,
        "an unresolvable id carried with an unmintable token",
        &format!("/global-networks?globalNetworkIds={NEVER_MINTED}&nextToken={bad_token}"),
        NEVER_MINTED,
    )
    .await;

    // The same token with an id that DOES resolve is the token refusal, so the answer
    // above is the precedence and not the token being ignored.
    let (status, body) = emulator
        .get(&format!(
            "/global-networks?globalNetworkIds={network}&nextToken={bad_token}"
        ))
        .await;
    let case = "a resolving id carried with an unmintable token";
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
}

#[tokio::test]
async fn a_resolving_id_stops_resolving_once_its_global_network_is_deleted() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("round trip").await;

    // Before the delete the very same request is a page of one.
    let uri = uri_for(std::slice::from_ref(&network), None);
    let (status, body) = emulator.get(&uri).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected 200, got {status} with {body}",
        "described before the delete"
    );
    assert_eq!(
        ids_of(&body, "GlobalNetworks", "GlobalNetworkId"),
        vec![network.clone()],
        "case {:?}: expected exactly the created network, got {body}",
        "described before the delete"
    );

    let (status, body) = emulator.delete(&format!("/global-networks/{network}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    // After it, the same request resolves nothing and is refused.
    assert_not_found(&emulator, "described after the delete", &uri, &network).await;
}
