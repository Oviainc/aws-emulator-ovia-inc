// SPDX-License-Identifier: CC0-1.0

//! The pagination-token rule `ListWorkspaceInstances` follows, over the wire this
//! service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<OperationName>`, the Region in the SigV4
//! credential scope).
//!
//! The rule: a paginated list honours a `NextToken` only as a resume point the
//! service itself issued — the token must resolve to a member of the collection THIS
//! request pages (a WorkSpace Instance held by the Region the request is signed for)
//! — and a token that resolves to no such member is refused with HTTP 400
//! `ValidationException` naming `NextToken`, never served as a page.
//!
//! The tables below span what the rule generalises over, rather than one fabricated
//! token: the resolvable side is asserted for EVERY stored id (a token built for the
//! first, a middle and the last record) and for every token the service itself mints
//! across page sizes 1..=count; the unresolvable side spans a decoded resume point
//! sorting above, below and strictly between the stored ids, well-shaped ids the
//! service never minted, values that are not ids at all, encodings that do not decode
//! — and a resume point another Region's list issued.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{
    call, call_in_region, create, error_code, list_all_ids, minimal_create, ok, router,
    DEFAULT_TEST_REGION,
};

/// How many instances every table here pages over: enough that a page size of 1, 2 or
/// 3 all truncate and mint a token, and that a resume point has records both before
/// and after it.
const STORED_INSTANCES: usize = 5;

/// `STORED_INSTANCES` instances in the default Region, as the ids the service minted
/// (reported in the order the listing pages them).
async fn stored_ids(router: &MultiServiceRouter) -> Vec<String> {
    for _ in 0..STORED_INSTANCES {
        create(router, minimal_create()).await;
    }
    let ids = list_all_ids(router, json!({})).await;
    assert_eq!(
        ids.len(),
        STORED_INSTANCES,
        "fixture: expected {STORED_INSTANCES} instances to be reported, got {ids:?}"
    );
    ids
}

/// The ids one page of `ListWorkspaceInstances` reports, with the page's `NextToken`.
async fn page(router: &MultiServiceRouter, request: Value) -> (Vec<String>, Option<String>) {
    let payload = ok(router, "ListWorkspaceInstances", request.clone()).await;
    let ids = payload["WorkspaceInstances"]
        .as_array()
        .unwrap_or_else(|| panic!("case {request}: no WorkspaceInstances in {payload}"))
        .iter()
        .map(|instance| {
            instance["WorkspaceInstanceId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let next = payload
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (ids, next)
}

/// Assert `payload` is this service's refusal of a token it did not issue: the modelled
/// `ValidationException`, HTTP 400, naming the `NextToken` member.
fn assert_token_refused(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {payload}"
    );
    let names_member = payload
        .get("FieldList")
        .and_then(Value::as_array)
        .is_some_and(|fields| {
            fields
                .iter()
                .any(|field| field.get("Name").and_then(Value::as_str) == Some("NextToken"))
        });
    assert!(
        names_member,
        "case {case:?}: the refusal must name the NextToken member, got {payload}"
    );
}

#[tokio::test]
async fn a_token_resumes_at_the_record_it_resolves_to() {
    let router = router();
    let ids = stored_ids(&router).await;

    // Every stored id is a resume point this listing can name, so a token carrying it
    // resumes exactly there — the first record, each middle one, and the last.
    for (position, id) in ids.iter().enumerate() {
        let token = hex::encode(id);
        let (reported, _) = page(
            &router,
            json!({ "NextToken": token, "MaxResults": STORED_INSTANCES as i64 }),
        )
        .await;
        assert_eq!(
            reported,
            ids[position..].to_vec(),
            "case resume at position {position} ({id:?}): expected the tail from {id:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn every_token_this_service_mints_resolves_and_pages_the_whole_list_once() {
    let router = router();
    let ids = stored_ids(&router).await;

    // Page sizes that all truncate, plus one that does not: a token is only minted
    // where the list is longer than the page.
    for page_size in 1..=STORED_INSTANCES + 1 {
        let mut request = json!({ "MaxResults": page_size as i64 });
        let mut walked: Vec<String> = Vec::new();
        loop {
            let (reported, next) = page(&router, request.clone()).await;
            walked.extend(reported);
            let Some(token) = next else { break };
            // The token the service minted carries a resume point this listing holds:
            // it decodes to one of the stored ids, and the record it names is the next
            // one reported.
            let decoded = String::from_utf8(
                hex::decode(&token).unwrap_or_else(|e| {
                    panic!("case MaxResults={page_size}: NextToken {token:?} is not this service's encoding ({e})")
                }),
            )
            .unwrap_or_else(|e| {
                panic!("case MaxResults={page_size}: NextToken {token:?} does not carry text ({e})")
            });
            assert!(
                ids.contains(&decoded),
                "case MaxResults={page_size}: expected the minted token to name a stored \
                 instance, got {decoded:?}"
            );
            assert_eq!(
                Some(&decoded),
                ids.get(walked.len()),
                "case MaxResults={page_size}: expected the token to resume at the record one \
                 past the page, got {decoded:?}"
            );
            request["NextToken"] = json!(token);
        }
        assert_eq!(
            walked, ids,
            "case MaxResults={page_size}: paging must report every instance exactly once, got \
             {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_token_that_resolves_to_no_record_of_this_region_is_refused() {
    let router = router();
    let ids = stored_ids(&router).await;
    let first = ids.first().expect("the fixture stores instances").clone();
    let last = ids.last().expect("the fixture stores instances").clone();

    // Each row is a `NextToken` no page of this list ever handed out. The first group
    // decodes to a well-shaped `WorkspaceInstanceId`
    // (`smithy_query(query="shape",shape="WorkspaceInstanceId")`: `len 15..70`,
    // `^wsinst-[0-9a-zA-Z]{8,63}$`) that names no stored record, placed above, below
    // and strictly between the ids the store holds; the rest do not even decode to an
    // id.
    let rows: Vec<(String, String)> = vec![
        (
            "a well-shaped id sorting above every stored id".to_string(),
            hex::encode("wsinst-ffffffffffffffff"),
        ),
        (
            "a well-shaped id sorting below every stored id".to_string(),
            hex::encode("wsinst-00000000"),
        ),
        (
            "a well-shaped id sorting strictly between two stored ids".to_string(),
            // One character longer than a stored id and sharing its prefix, so it
            // sorts after that record and before the next one — and names neither.
            hex::encode(format!("{first}0")),
        ),
        (
            "a well-shaped id just past the last stored one".to_string(),
            hex::encode(format!("{last}0")),
        ),
        (
            "a well-shaped id in another character case".to_string(),
            hex::encode("wsinst-AbCdEf01234"),
        ),
        (
            "the longest well-shaped id".to_string(),
            hex::encode(format!("wsinst-{}", "z".repeat(63))),
        ),
        (
            "a stored id with one character dropped".to_string(),
            hex::encode(&first[..first.len() - 1]),
        ),
        (
            "another resource kind's id".to_string(),
            hex::encode("vol-0123456789"),
        ),
        (
            "text that is not an id".to_string(),
            hex::encode("zzzzzzzzzzzzzz"),
        ),
        (
            "a stored id passed through raw, not encoded".to_string(),
            first.clone(),
        ),
        (
            "a token re-encoded over this service's encoding".to_string(),
            hex::encode(hex::encode(&first)),
        ),
        ("not an encoding at all".to_string(), "not-a-token".to_string()),
        ("odd-length hex".to_string(), "abc".to_string()),
        ("hex that is not text".to_string(), "ff".to_string()),
    ];

    for (case, token) in &rows {
        // The refusal does not depend on the page size asked for, nor on a filter
        // narrowing the list: a token the service did not issue is a bad request.
        for request in [
            json!({ "NextToken": token }),
            json!({ "NextToken": token, "MaxResults": 1 }),
            json!({ "NextToken": token, "MaxResults": STORED_INSTANCES as i64 }),
            json!({ "NextToken": token, "ProvisionStates": ["ALLOCATING", "ALLOCATED"] }),
        ] {
            let (status, payload) = call(&router, "ListWorkspaceInstances", request.clone()).await;
            assert_token_refused(&format!("{case} ({token:?}) with {request}"), status, &payload);
        }
    }

    // A refused request changes nothing: the list still reports every instance.
    assert_eq!(
        list_all_ids(&router, json!({})).await,
        ids,
        "case after the refusals: expected the stored instances to be reported unchanged"
    );
}

#[tokio::test]
async fn a_resume_point_is_scoped_to_the_region_whose_list_issued_it() {
    // A token names a record of the Region it was issued for, so presenting it to
    // another Region's endpoint resolves to nothing there — whether or not that Region
    // holds instances of its own.
    let router = router();
    let regions = ["us-east-1", "eu-west-3", "ap-northeast-2"];
    let mut ids_by_region: Vec<(&str, Vec<String>)> = Vec::new();
    for region in regions {
        let mut ids = Vec::new();
        for _ in 0..2 {
            let (status, payload) = call_in_region(
                &router,
                region,
                "CreateWorkspaceInstance",
                minimal_create(),
            )
            .await;
            assert_eq!(
                status, 200,
                "fixture {region}: expected the create to answer HTTP 200, got {status} with \
                 {payload}"
            );
            ids.push(
                payload["WorkspaceInstanceId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("fixture {region}: no id in {payload}"))
                    .to_string(),
            );
        }
        ids_by_region.push((region, ids));
    }

    for (issued_in, ids) in &ids_by_region {
        for id in ids {
            let token = hex::encode(id);
            // In the Region that holds the record, the resume point resolves.
            let (status, payload) = call_in_region(
                &router,
                issued_in,
                "ListWorkspaceInstances",
                json!({ "NextToken": token }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {id:?} in its own Region {issued_in}: expected HTTP 200, got {status} \
                 with {payload}"
            );
            assert_eq!(
                payload["WorkspaceInstances"][0]["WorkspaceInstanceId"].as_str(),
                Some(id.as_str()),
                "case {id:?} in its own Region {issued_in}: expected the page to resume at it, \
                 got {payload}"
            );

            for (foreign, _) in ids_by_region.iter().filter(|(other, _)| other != issued_in) {
                let (status, payload) = call_in_region(
                    &router,
                    foreign,
                    "ListWorkspaceInstances",
                    json!({ "NextToken": token }),
                )
                .await;
                assert_token_refused(
                    &format!("{id:?} issued in {issued_in}, presented in {foreign}"),
                    status,
                    &payload,
                );
            }
        }
    }

    // The Regions still report exactly their own instances after the refusals.
    for (region, ids) in &ids_by_region {
        let mut request = json!({});
        let mut reported: Vec<String> = Vec::new();
        loop {
            let (status, payload) =
                call_in_region(&router, region, "ListWorkspaceInstances", request.clone()).await;
            assert_eq!(
                status, 200,
                "case {region} after the refusals: expected HTTP 200, got {status} with {payload}"
            );
            for instance in payload["WorkspaceInstances"].as_array().unwrap_or_else(|| {
                panic!("case {region} after the refusals: no WorkspaceInstances in {payload}")
            }) {
                reported.push(
                    instance["WorkspaceInstanceId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match payload.get("NextToken").and_then(Value::as_str) {
                Some(token) => request["NextToken"] = json!(token),
                None => break,
            }
        }
        assert_eq!(
            &reported, ids,
            "case {region} after the refusals: expected its own instances, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_token_resolves_against_the_region_it_is_presented_to_even_when_it_holds_nothing() {
    // The Region a request is signed for holds no instances at all, so there is no
    // resume point it can name: a token is refused rather than answered with the empty
    // page the Region would report without one.
    let router = router();
    let id = create(&router, minimal_create()).await;
    let empty_region = "sa-east-1";
    assert_ne!(
        empty_region, DEFAULT_TEST_REGION,
        "fixture: the empty Region must not be the one the instance was created in"
    );

    let (status, payload) = call_in_region(
        &router,
        empty_region,
        "ListWorkspaceInstances",
        json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the Region without a token: expected HTTP 200, got {status} with {payload}"
    );
    assert_eq!(
        payload["WorkspaceInstances"].as_array().map(Vec::len),
        Some(0),
        "case the Region without a token: expected an empty page, got {payload}"
    );

    let (status, payload) = call_in_region(
        &router,
        empty_region,
        "ListWorkspaceInstances",
        json!({ "NextToken": hex::encode(&id) }),
    )
    .await;
    assert_token_refused(
        &format!("{id:?} presented to the empty Region {empty_region}"),
        status,
        &payload,
    );
}
