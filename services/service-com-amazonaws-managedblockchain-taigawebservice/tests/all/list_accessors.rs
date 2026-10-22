// SPDX-License-Identifier: CC0-1.0

//! `ListAccessors`, end to end over HTTP: the page it answers with is cut from the
//! accessors this account's store holds, every summary member comes from the record
//! `CreateAccessor` persisted (the same record `GetAccessor` answers from), the
//! `NetworkType` filter selects on each record's stored network type, and the listing
//! pages through `NextToken` without repeating or skipping an accessor.
//!
//! The tables below span the classes the rules generalise over: every `NetworkType` the
//! model declares plus its omission, every value `AccessorStatus` declares, page sizes
//! from one to past the item count plus the published `MaxResults` bounds and one past
//! each, and a `NextToken` this service never minted.
//!
//! Only `CreateAccessor` mints an accessor today (`DeleteAccessor` is still a scaffold),
//! so the rows that need a status no create produces write them into this service's own
//! store — the same store the handler reads — and then assert over the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_managedblockchain_taigawebservice::state::{
    ComAmazonawsManagedblockchainTaigawebserviceState as ServiceState, StoredAccessor,
    ACCESSOR_MAX_RESULTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, is_date_time, is_resource_id, new_router, router_and_engine, ACCOUNT, REGION,
};
use crate::network_and_node_lifecycle::Answer;

/// The only `AccessorType` the model declares (smithy_query: shape AccessorType =
/// BILLING_TOKEN).
const BILLING_TOKEN: &str = "BILLING_TOKEN";

/// Every value `AccessorNetworkType` declares (smithy_query: shape AccessorNetworkType).
const NETWORK_TYPES: &[&str] = &[
    "ETHEREUM_GOERLI",
    "ETHEREUM_MAINNET",
    "ETHEREUM_MAINNET_AND_GOERLI",
    "POLYGON_MAINNET",
    "POLYGON_MUMBAI",
];

/// Every value `AccessorStatus` declares (smithy_query: shape AccessorStatus).
const STATUSES: &[&str] = &["AVAILABLE", "PENDING_DELETION", "DELETED"];

/// The members `AccessorSummary` declares (smithy_query: shape AccessorSummary), sorted.
/// Notably NOT `BillingToken` or `Tags`: the summary is a narrower projection than
/// `Accessor`.
const SUMMARY_MEMBERS: &[&str] = &["Arn", "CreationDate", "Id", "NetworkType", "Status", "Type"];

/// Create one accessor over the wire and return its id.
async fn create_accessor(
    router: &MultiServiceRouter,
    token: &str,
    network_type: Option<&str>,
) -> String {
    let mut body = json!({
        "ClientRequestToken": token,
        "AccessorType": BILLING_TOKEN,
    });
    if let Some(network_type) = network_type {
        body["NetworkType"] = Value::String(network_type.to_string());
    }
    let answer = call(router, "POST", "/accessors", Some(body)).await;
    assert_eq!(
        answer.status, 200,
        "case {token:?}: expected CreateAccessor 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.text(&["AccessorId"])
}

/// One `ListAccessors` call: `query` is the already-encoded query string (empty for none).
async fn list_accessors(router: &MultiServiceRouter, query: &str) -> Answer {
    let path = if query.is_empty() {
        "/accessors".to_string()
    } else {
        format!("/accessors?{query}")
    };
    call(router, "GET", &path, None).await
}

/// The `Id` of every accessor the answer reports, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["Accessors"]
        .as_array()
        .map(|accessors| {
            accessors
                .iter()
                .map(|accessor| accessor["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One seeded accessor: what this row varies from the record `CreateAccessor` stored.
#[derive(Debug, Clone, Copy)]
struct Seed {
    /// An `AccessorStatus` value.
    status: &'static str,
    /// An `AccessorNetworkType` value.
    network_type: &'static str,
}

/// Write `seeds` into this service's store and return their ids, in the order the store
/// holds them.
///
/// Each seeded record is a copy of the one `CreateAccessor` stored, with only the row's
/// status and network type changed — so a case tests the handler, not a hand-built record
/// no create could have produced.
fn seed_accessors(engine: &engine::InMemoryEngine, seeds: &[Seed]) -> Vec<String> {
    let state = engine.state::<ServiceState>();
    let mut guard = state.lock();
    let template = guard
        .accessors
        .values()
        .next()
        .expect("CreateAccessor stored the template accessor")
        .clone();
    let now = SystemTime::now();
    let mut ids = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        // Zero-padded, so the id ordering the store pages on is the row ordering, and
        // within `ResourceIdString`'s `@length(1, 32)`.
        let id = format!("ac-SEED{index:022}");
        let accessor = StoredAccessor {
            id: id.clone(),
            status: seed.status.to_string(),
            network_type: seed.network_type.to_string(),
            creation_date: now,
            arn: format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:accessors/{id}"),
            ..template.clone()
        };
        guard.accessors.insert(id.clone(), accessor);
        ids.push(id);
    }
    ids
}

/// `ListAccessors` reports every stored accessor, each summary projected from the record
/// the create persisted — the same record `GetAccessor` answers from — and repeating the
/// read changes nothing a later caller sees.
#[tokio::test]
async fn list_accessors_answers_from_the_stored_accessors() {
    let router = new_router();

    // An empty store is an empty page, not a refusal and not a token.
    let answer = list_accessors(&router, "").await;
    assert_eq!(
        (answer.status, listed_ids(&answer).len()),
        (200, 0),
        "case an empty store: expected 200 and no accessors, got {} {:?}",
        answer.status,
        answer.body
    );
    assert!(
        answer.body["NextToken"].is_null(),
        "case an empty store: expected no NextToken, got {:?}",
        answer.body
    );

    // Rows: every `NetworkType` the model declares, plus one request that names none —
    // AWS's `CreateAccessor` reference states `ETHEREUM_MAINNET_AND_GOERLI` is applied
    // when the action does not set a `networkType`.
    let mut cases: Vec<(String, Option<&str>, &str)> = NETWORK_TYPES
        .iter()
        .map(|network_type| {
            (
                format!("token-{network_type}"),
                Some(*network_type),
                *network_type,
            )
        })
        .collect();
    cases.push((
        "token-default".to_string(),
        None,
        "ETHEREUM_MAINNET_AND_GOERLI",
    ));

    let mut created: Vec<(String, &str)> = Vec::new();
    for (token, network_type, expected_network_type) in &cases {
        let id = create_accessor(&router, token, *network_type).await;
        created.push((id, expected_network_type));
    }

    let answer = list_accessors(&router, &format!("maxResults={ACCESSOR_MAX_RESULTS}")).await;
    assert_eq!(
        answer.status, 200,
        "case every accessor: expected ListAccessors 200, got {} {:?}",
        answer.status, answer.body
    );
    let mut expected: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
    expected.sort();
    let mut listed = listed_ids(&answer);
    listed.sort();
    assert_eq!(
        listed, expected,
        "case every accessor: expected every created accessor exactly once, got {:?}",
        answer.body
    );
    assert!(
        answer.body["NextToken"].is_null(),
        "case every accessor: expected the only page to carry no NextToken, got {:?}",
        answer.body
    );

    for (id, expected_network_type) in &created {
        let summary = answer.body["Accessors"]
            .as_array()
            .and_then(|accessors| {
                accessors
                    .iter()
                    .find(|accessor| accessor["Id"].as_str() == Some(id.as_str()))
            })
            .cloned()
            .unwrap_or(Value::Null);
        assert!(
            is_resource_id(id, "ac"),
            "case {id}: expected an `ac-`-prefixed resource id, got {id:?}"
        );
        assert_eq!(
            summary["Type"].as_str(),
            Some(BILLING_TOKEN),
            "case {id}: expected the stored accessor type, got {summary:?}"
        );
        // `AccessorStatus` declares no creating state, so a created accessor is ready at
        // once and the clock owes it no transition.
        assert_eq!(
            summary["Status"].as_str(),
            Some("AVAILABLE"),
            "case {id}: expected the stored status, got {summary:?}"
        );
        assert_eq!(
            summary["NetworkType"].as_str(),
            Some(*expected_network_type),
            "case {id}: expected the stored NetworkType {expected_network_type}, got {summary:?}"
        );
        assert_eq!(
            summary["Arn"].as_str(),
            Some(format!("arn:aws:managedblockchain:{REGION}:{ACCOUNT}:accessors/{id}").as_str()),
            "case {id}: expected the stored accessor ARN, got {summary:?}"
        );
        assert!(
            is_date_time(&summary["CreationDate"]),
            "case {id}: expected an ISO-8601 CreationDate, got {summary:?}"
        );

        let mut keys: Vec<String> = summary
            .as_object()
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        assert_eq!(
            keys,
            SUMMARY_MEMBERS
                .iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<String>>(),
            "case {id}: expected exactly the AccessorSummary members the model declares, \
             got {summary:?}"
        );

        // The listing and `GetAccessor` project the SAME record.
        let read = call(&router, "GET", &format!("/accessors/{id}"), None).await;
        for field in SUMMARY_MEMBERS {
            assert_eq!(
                summary[*field], read.body["Accessor"][*field],
                "case {id}: expected ListAccessors' {field} to match GetAccessor's, got \
                 {summary:?} and {:?}",
                read.body
            );
        }
    }

    // A read reports; it does not change what a later caller sees.
    let again = list_accessors(&router, &format!("maxResults={ACCESSOR_MAX_RESULTS}")).await;
    assert_eq!(
        again.body, answer.body,
        "case a repeated listing: expected an identical answer, got {:?} then {:?}",
        answer.body, again.body
    );
}

/// The `NetworkType` filter selects on each record's stored network type, and an omitted
/// one selects every accessor — over rows spanning every `AccessorNetworkType` and every
/// `AccessorStatus` the model declares.
#[tokio::test]
async fn the_network_type_filter_selects_on_the_stored_value() {
    let (router, engine) = router_and_engine();
    let created = create_accessor(&router, "filter-template", Some("ETHEREUM_MAINNET")).await;

    // One row per (status, network type) pair, so every declared value of both enums is
    // stored and the filter is asserted independently of the status.
    let seeds: Vec<Seed> = STATUSES
        .iter()
        .flat_map(|status| {
            NETWORK_TYPES.iter().map(move |network_type| Seed {
                status,
                network_type,
            })
        })
        .collect();
    let seeded = seed_accessors(&engine, &seeds);

    let mut everything: Vec<String> = seeded.clone();
    everything.push(created.clone());
    everything.sort();

    let answer = list_accessors(&router, &format!("maxResults={ACCESSOR_MAX_RESULTS}")).await;
    let mut listed = listed_ids(&answer);
    listed.sort();
    assert_eq!(
        listed, everything,
        "case no filter: expected every stored accessor, got {:?}",
        answer.body
    );

    for network_type in NETWORK_TYPES {
        let answer = list_accessors(
            &router,
            &format!("networkType={network_type}&maxResults={ACCESSOR_MAX_RESULTS}"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case networkType={network_type}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        let mut expected: Vec<String> = seeds
            .iter()
            .enumerate()
            .filter(|(_, seed)| seed.network_type == *network_type)
            .map(|(index, _)| seeded[index].clone())
            .collect();
        if *network_type == "ETHEREUM_MAINNET" {
            expected.push(created.clone());
        }
        expected.sort();
        let mut listed = listed_ids(&answer);
        listed.sort();
        assert_eq!(
            listed, expected,
            "case networkType={network_type}: expected only the accessors stored with that \
             network type, got {:?}",
            answer.body
        );
        for accessor in answer.body["Accessors"].as_array().unwrap_or(&Vec::new()) {
            assert_eq!(
                accessor["NetworkType"].as_str(),
                Some(*network_type),
                "case networkType={network_type}: expected every reported accessor to carry it, \
                 got {accessor:?}"
            );
        }
    }

    // Each stored status is reported from its record: nothing the listing does moves one.
    for status in STATUSES {
        let answer = list_accessors(&router, &format!("maxResults={ACCESSOR_MAX_RESULTS}")).await;
        let reported: Vec<String> = answer.body["Accessors"]
            .as_array()
            .map(|accessors| {
                accessors
                    .iter()
                    .filter(|accessor| accessor["Status"].as_str() == Some(*status))
                    .map(|accessor| accessor["Id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let mut expected: Vec<String> = seeds
            .iter()
            .enumerate()
            .filter(|(_, seed)| seed.status == *status)
            .map(|(index, _)| seeded[index].clone())
            .collect();
        if *status == "AVAILABLE" {
            expected.push(created.clone());
        }
        expected.sort();
        let mut reported = reported;
        reported.sort();
        assert_eq!(
            reported, expected,
            "case status {status}: expected the accessors stored in that status, got {:?}",
            answer.body
        );
    }
}

/// The listing pages by `MaxResults` through `NextToken`: walking the pages reports every
/// accessor exactly once, and the last page carries no token.
#[tokio::test]
async fn list_accessors_pages_through_every_accessor() {
    let router = new_router();

    let mut everything: Vec<String> = Vec::new();
    for index in 0..6 {
        everything.push(
            create_accessor(
                &router,
                &format!("page-token-{index}"),
                Some(NETWORK_TYPES[index % NETWORK_TYPES.len()]),
            )
            .await,
        );
    }
    everything.sort();

    // Rows: a page per accessor, sizes either side of the item count, the published
    // `MaxResults` maximum (`AccessorListMaxResults`'s `@range(1, 50)`) and no
    // `MaxResults` at all.
    let cases: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(4),
        Some(everything.len() as i32 - 1),
        Some(everything.len() as i32),
        Some(everything.len() as i32 + 1),
        Some(ACCESSOR_MAX_RESULTS),
        None,
    ];

    for max_results in cases {
        let case = format!("maxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = match max_results {
                Some(limit) => format!("maxResults={limit}"),
                None => String::new(),
            };
            if let Some(token) = &next_token {
                if !query.is_empty() {
                    query.push('&');
                }
                query.push_str(&format!("nextToken={token}"));
            }
            let answer = list_accessors(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case} page {pages}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            let ids = listed_ids(&answer);
            let limit = max_results.unwrap_or(ACCESSOR_MAX_RESULTS) as usize;
            assert!(
                ids.len() <= limit && !ids.is_empty(),
                "case {case} page {pages}: expected 1..={limit} accessors, got {:?}",
                answer.body
            );
            seen.extend(ids);
            next_token = answer.body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= everything.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case {case}: expected no accessor on two pages, got {seen:?}"
        );
        assert_eq!(
            sorted, everything,
            "case {case}: expected every stored accessor across the pages, got {seen:?}"
        );
    }

    // A token this service never issued cannot resume a page.
    for forged in ["not-a-token", "6162636465", "ZZZZ"] {
        let answer = list_accessors(&router, &format!("nextToken={forged}")).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "InvalidRequestException"),
            "case nextToken={forged}: expected 400 InvalidRequestException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // One past each published `MaxResults` bound is refused at the boundary, before the
    // handler runs. Only the status is asserted: which shape the boundary validator
    // refuses with is not this operation's to choose.
    for out_of_range in [0, ACCESSOR_MAX_RESULTS + 1] {
        let answer = list_accessors(&router, &format!("maxResults={out_of_range}")).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={out_of_range}: expected a refusal, got {} {:?}",
            answer.status, answer.body
        );
    }

    // And the refusals changed nothing: every accessor still lists.
    let answer = list_accessors(&router, &format!("maxResults={ACCESSOR_MAX_RESULTS}")).await;
    let mut listed = listed_ids(&answer);
    listed.sort();
    assert_eq!(
        listed, everything,
        "case after the refusals: expected every stored accessor, got {:?}",
        answer.body
    );
}
