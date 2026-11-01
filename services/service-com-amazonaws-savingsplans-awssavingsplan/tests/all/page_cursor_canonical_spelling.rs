// SPDX-License-Identifier: CC0-1.0

//! A cursor is opaque, so the only tokens a paginated operation of this service
//! resumes a walk at are the ones it minted itself.
//!
//! The sibling suite (`shared_page_cursor_rule`) pins the refusal for a token
//! some OTHER paginator minted and for a token that carries no offset at all.
//! This one pins the narrower family that sits between those: a payload whose
//! page kind and offset are both the operation's own, spelled in a way the mint
//! never writes. Every such token must be refused exactly as a foreign one is,
//! and the ONE canonical spelling of the same offset must still be served.
//!
//! The rows span what the rule generalises over: which of the four paginated
//! operations serves the request, which offset of its walk the payload names,
//! and how the payload differs from the minted one (a leading `+`, one or
//! several leading zeros, trailing text after the offset, and the same bytes
//! re-encoded in upper-case hex). Everything goes through the same
//! `MultiServiceRouter` the emulator boots, so what is pinned is the HTTP
//! behaviour.

use serde_json::json;
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR, THREE_YEARS};
use crate::shared_page_cursor_rule::{assert_refused, request, PAGINATED};

/// How many rows of each walk the canonical cursors are checked against: enough
/// that an offset past the first page is exercised too.
const HEAD: usize = 3;

/// Three plans, bought from three different offerings, so the plan search has
/// more rows than `HEAD`. Returns the id of the first.
async fn three_plans(emulator: &Emulator) -> String {
    let mut ids: Vec<String> = Vec::new();
    for (plan_type, payment_option, duration, commitment) in [
        ("EC2Instance", state::PAYMENT_NO_UPFRONT, ONE_YEAR, "0.1"),
        ("Database", state::PAYMENT_ALL_UPFRONT, THREE_YEARS, "0.5"),
        ("Compute", state::PAYMENT_NO_UPFRONT, THREE_YEARS, "0.2"),
    ] {
        let offering = emulator.offering(plan_type, payment_option, duration).await;
        ids.push(
            emulator
                .purchase(json!({
                    "savingsPlanOfferingId": text(&offering, "offeringId"),
                    "commitment": commitment,
                }))
                .await,
        );
    }
    ids.swap_remove(0)
}

/// Payloads that name `kind` and `offset` yet differ from the single token
/// `state::encode_page_token` mints for that pair, as `(label, token)`.
///
/// The mint writes `hex::encode(format!("{kind}:{offset}"))`: lower-case hex of
/// a `usize` rendered in plain decimal digits. So a sign, any leading zero, any
/// trailing byte and upper-case hex are all spellings it can never hand out.
fn non_canonical(kind: &str, offset: usize) -> Vec<(String, String)> {
    let minted = state::encode_page_token(kind, offset);
    vec![
        (
            "a leading plus on the offset".to_string(),
            hex::encode(format!("{kind}:+{offset}")),
        ),
        (
            "one leading zero on the offset".to_string(),
            hex::encode(format!("{kind}:0{offset}")),
        ),
        (
            "three leading zeros on the offset".to_string(),
            hex::encode(format!("{kind}:000{offset}")),
        ),
        (
            "a plus and leading zeros on the offset".to_string(),
            hex::encode(format!("{kind}:+0{offset}")),
        ),
        (
            "a space after the offset".to_string(),
            hex::encode(format!("{kind}:{offset} ")),
        ),
        (
            "another offset appended".to_string(),
            hex::encode(format!("{kind}:{offset}:{offset}")),
        ),
        (
            "the minted payload re-encoded in upper-case hex".to_string(),
            minted.to_uppercase(),
        ),
    ]
}

/// For every paginated operation and every offset of its walk, the one minted
/// spelling resumes the page and every other spelling of the same offset is the
/// shared refusal.
#[tokio::test]
async fn only_the_minted_spelling_of_a_cursor_resumes_a_walk() {
    let emulator = Emulator::new();
    let savings_plan_id = three_plans(&emulator).await;

    for (operation, kind, results) in PAGINATED {
        // The head of the walk, read without a cursor: the rows each canonical
        // offset must resume at.
        let case = format!("{operation} with no cursor");
        let head = emulator
            .call(
                operation,
                request(operation, &savings_plan_id, HEAD as i64),
            )
            .await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            head.status, head.body
        );
        let rows = head.list(results).to_vec();
        assert_eq!(
            rows.len(),
            HEAD,
            "case {case:?}: expected {HEAD} rows to walk, got {}",
            rows.len()
        );

        for (offset, row) in rows.iter().enumerate() {
            // The minted token for this offset is served, and serves the row
            // that offset names.
            let minted = state::encode_page_token(kind, offset);
            let case = format!("{operation} resumed by the minted cursor for offset {offset}");
            let mut body = request(operation, &savings_plan_id, 1);
            body["nextToken"] = json!(minted);
            let answer = emulator.call(operation, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 for {minted:?}, got {} {:?}",
                answer.status, answer.body
            );
            let served = answer.list(results).to_vec();
            assert_eq!(
                served.as_slice(),
                &rows[offset..=offset],
                "case {case:?}: expected the row at offset {offset} ({row:?}), got {served:?}"
            );

            // Every other spelling of that same offset is refused, so no page
            // is ever served off a cursor this operation did not hand out.
            for (label, token) in non_canonical(kind, offset) {
                let case = format!("{operation} handed offset {offset} with {label}");
                assert_ne!(
                    token, minted,
                    "case {case:?}: the forged token must differ from the minted one"
                );
                let mut body = request(operation, &savings_plan_id, 1);
                body["nextToken"] = json!(token);
                let answer = emulator.call(operation, body).await;
                assert_refused(&case, &answer);
            }
        }
    }
}
