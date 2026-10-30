// SPDX-License-Identifier: CC0-1.0

//! The canonical-spelling half of this service's pagination-token rule, driven over the
//! wire it really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test:
//!
//! > a paginated list honours a `NextToken` only when it is byte for byte the token this
//! > service's encoder would mint for the resume point it carries; any OTHER spelling of
//! > the same payload is a token the service never issued, and is refused exactly as a
//! > token that carries no resume point at all is — HTTP 400 `ValidationException`
//! > naming `NextToken`.
//!
//! The rows span what that rule generalises over: the operation the token is presented
//! to (both lists of this service that mint one — `ListWorkspaceInstances`, whose resume
//! point is a stored record, and `ListInstanceTypes`, whose resume point is a catalogued
//! instance type), the page size the token was minted at, the re-spelling applied to it
//! (every digit upper-cased, every other digit upper-cased, only the first case-bearing
//! digit, only the last), and a token fabricated from an upper-case encoding of a resume
//! point that really exists rather than derived from a minted one. The request bodies the
//! token travels in are varied too, so the refusal cannot turn on a page size or filter.
//!
//! Nothing here passes against text written in this file: each refusal is compared whole
//! against the one the SAME operation gives a token that is not this service's encoding
//! at all, and the canonical spelling of every token is asserted to still serve its page.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::next_token_rule_sharing::requests;
use crate::workspace_instance_lifecycle::{call, create, error_code, minimal_create, ok, router};

/// The lists of this service that mint a `NextToken`, with the items member each one
/// reports (`OPERATIONS.md`; `ListRegions` is excluded because it mints none).
const MINTING: &[(&str, &str)] = &[
    ("ListWorkspaceInstances", "WorkspaceInstances"),
    ("ListInstanceTypes", "InstanceTypes"),
];

/// How many instances `ListWorkspaceInstances` pages over here: enough that page sizes
/// 1 and 2 both truncate and mint a token.
const STORED_INSTANCES: usize = 4;

/// Re-spellings of one minted token that carry the SAME payload: every one of these is a
/// spelling this service's encoder cannot produce, so none of them is a token it issued.
/// Rows that come out identical to the minted token (a token with no case-bearing digit
/// in the position a row touches) are dropped — they are the same token, not a
/// re-spelling of it.
fn respellings(token: &str) -> Vec<(String, String)> {
    let mut rows = vec![
        ("every digit upper-cased".to_string(), token.to_ascii_uppercase()),
        (
            "every other digit upper-cased".to_string(),
            token
                .chars()
                .enumerate()
                .map(|(index, digit)| {
                    if index % 2 == 0 {
                        digit.to_ascii_uppercase()
                    } else {
                        digit
                    }
                })
                .collect(),
        ),
    ];
    for (label, position) in [
        ("only the first case-bearing digit", token.find(|c: char| c.is_ascii_alphabetic())),
        (
            "only the last case-bearing digit",
            token.rfind(|c: char| c.is_ascii_alphabetic()),
        ),
    ] {
        if let Some(position) = position {
            let mut spelled = token.to_string();
            spelled.replace_range(
                position..position + 1,
                &token[position..position + 1].to_ascii_uppercase(),
            );
            rows.push((label.to_string(), spelled));
        }
    }
    rows.retain(|(_, spelled)| spelled != token);
    rows
}

/// The refusal `operation` gives a token that is not this service's encoding at all —
/// the answer every token it did not issue has to get, read off the service rather than
/// written here.
async fn reference_refusal(router: &MultiServiceRouter, operation: &str) -> Value {
    let (status, payload) = call(router, operation, json!({ "NextToken": "not-a-token" })).await;
    assert_eq!(
        status, 400,
        "fixture on {operation}: a token this service did not issue is a 400, got {status} \
         with {payload}"
    );
    payload
}

/// Assert `payload` is this service's refusal of a token it did not issue, and is the
/// very same refusal `reference` carries.
fn assert_refused(case: &str, operation: &str, status: u16, payload: &Value, reference: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?} on {operation}: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ValidationException",
        "case {case:?} on {operation}: expected ValidationException, got {payload}"
    );
    assert!(
        payload
            .get("FieldList")
            .and_then(Value::as_array)
            .is_some_and(|fields| fields
                .iter()
                .any(|field| field.get("Name").and_then(Value::as_str) == Some("NextToken"))),
        "case {case:?} on {operation}: the refusal must name the NextToken member, got {payload}"
    );
    assert_eq!(
        payload, reference,
        "case {case:?} on {operation}: expected the same refusal a token that is not this \
         service's encoding gets ({reference}), got {payload}"
    );
}

/// Seed what each list pages over, and answer the `NextToken` values that list mints at
/// the page sizes given — each a token the service really handed out.
async fn minted_tokens(
    router: &MultiServiceRouter,
    operation: &str,
    items: &str,
    page_sizes: &[u64],
) -> Vec<(u64, String)> {
    if operation == "ListWorkspaceInstances" {
        for _ in 0..STORED_INSTANCES {
            create(router, minimal_create()).await;
        }
    }
    let mut tokens = Vec::new();
    for &page_size in page_sizes {
        let payload = ok(router, operation, json!({ "MaxResults": page_size })).await;
        let reported = payload[items].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(
            reported, page_size as usize,
            "fixture on {operation}: expected a full page of {page_size} to page over, got \
             {payload}"
        );
        let token = payload
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "fixture on {operation}: a truncated page of {page_size} mints a NextToken, \
                     got {payload}"
                )
            })
            .to_string();
        tokens.push((page_size, token));
    }
    tokens
}

#[tokio::test]
async fn a_re_spelt_token_is_refused_by_every_list_that_mints_one() {
    for (operation, items) in MINTING {
        let router = router();
        let reference = reference_refusal(&router, operation).await;
        let tokens = minted_tokens(&router, operation, items, &[1, 2]).await;

        for (page_size, token) in &tokens {
            // The control: the spelling the service minted is still honoured, so the
            // rule refuses the re-spellings and nothing else.
            let served = ok(
                &router,
                operation,
                json!({ "NextToken": token, "MaxResults": page_size }),
            )
            .await;
            assert!(
                served[*items].as_array().is_some_and(|page| !page.is_empty()),
                "case \"the minted spelling at MaxResults={page_size}\" on {operation}: expected \
                 a page from the resume point, got {served}"
            );

            let rows = respellings(token);
            assert!(
                !rows.is_empty(),
                "fixture on {operation}: the token minted at MaxResults={page_size} ({token:?}) \
                 carries no case-bearing digit, so this table would assert nothing"
            );
            for (label, spelled) in rows {
                for body in requests(operation, &spelled) {
                    let case = format!("{label} of the token minted at MaxResults={page_size}, in {body}");
                    let (status, payload) = call(&router, operation, body.clone()).await;
                    assert_refused(&case, operation, status, &payload, &reference);
                }
            }
        }
    }
}

#[tokio::test]
async fn an_upper_case_encoding_of_a_resume_point_that_exists_is_refused() {
    // The payload here is a resume point the list really can name — a stored record's id,
    // a catalogued instance type — so only the SPELLING makes the token one this service
    // never issued.
    let router = router();
    let id = create(&router, minimal_create()).await;
    let catalog = ok(&router, "ListInstanceTypes", json!({})).await;
    let instance_type = catalog["InstanceTypes"][0]["InstanceType"]
        .as_str()
        .unwrap_or_else(|| panic!("fixture: the catalog reports no InstanceType: {catalog}"))
        .to_string();

    for (operation, resume_point) in [
        ("ListWorkspaceInstances", id),
        ("ListInstanceTypes", instance_type),
    ] {
        let reference = reference_refusal(&router, operation).await;
        // `hex::encode` is the encoder's own output, lower-case; `hex::encode_upper` is
        // the same payload in a spelling it cannot emit.
        let canonical = hex::encode(&resume_point);
        let fabricated = hex::encode_upper(&resume_point);
        assert_ne!(
            canonical, fabricated,
            "fixture on {operation}: {resume_point:?} must have a case-bearing hex digit"
        );
        // The canonical spelling of this very payload resolves, so the refusal below is
        // about the spelling and not about the resume point: the page it serves starts AT
        // the resume point the token carries, which is the behaviour the re-spelling must
        // not be able to reach.
        let items = MINTING
            .iter()
            .find_map(|(name, items)| (name == &operation).then_some(*items))
            .unwrap_or_else(|| panic!("{operation} is one of the lists that mint a token"));
        let served = ok(&router, operation, json!({ "NextToken": canonical })).await;
        let first = served[items]
            .as_array()
            .and_then(|page| page.first().cloned())
            .unwrap_or_else(|| {
                panic!(
                    "case \"the canonical spelling of {resume_point:?}\" on {operation}: expected \
                     a page from the resume point, got {served}"
                )
            });
        let reported = ["WorkspaceInstanceId", "InstanceType"]
            .iter()
            .find_map(|member| first.get(member).and_then(Value::as_str))
            .unwrap_or_else(|| {
                panic!(
                    "case \"the canonical spelling of {resume_point:?}\" on {operation}: the page's \
                     first entry names no identifier: {first}"
                )
            });
        assert_eq!(
            reported, resume_point,
            "case \"the canonical spelling of {resume_point:?}\" on {operation}: expected the \
             page to resume at {resume_point:?}, got {served}"
        );

        for body in requests(operation, &fabricated) {
            let case = format!("an upper-case encoding of {resume_point:?}, in {body}");
            let (status, payload) = call(&router, operation, body.clone()).await;
            assert_refused(&case, operation, status, &payload, &reference);
        }
    }
}
