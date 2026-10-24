// SPDX-License-Identifier: CC0-1.0

//! Target domain DELETION in the SecurityAgent emulator, end to end over the wire:
//! `DeleteTargetDomain` followed by `BatchGetTargetDomains` (the registration/read round
//! trip itself lives in `target_domain_registration.rs`).
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), so nothing here can pass against a handler that answers a canned
//! body: what a delete did must come back out of the shared store through
//! `BatchGetTargetDomains` before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: the
//! rows differ along the `DomainVerificationMethod` the deleted domain was registered
//! with, WHICH of several registered domains is deleted, and the COMPOSITION of the batch
//! the read asks for (one id, a mix of live and deleted ids, a repeated id, ids in either
//! order).

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    VERIFICATION_METHOD_DNS_TXT, VERIFICATION_METHOD_HTTP_ROUTE, VERIFICATION_METHOD_PRIVATE_VPC,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok};

/// Every `DomainVerificationMethod` the model admits
/// (`smithy_query(query="shape",shape="DomainVerificationMethod")`), so the deletion
/// cases span the registration forms a stored record can have.
fn verification_methods() -> [&'static str; 3] {
    [
        VERIFICATION_METHOD_DNS_TXT,
        VERIFICATION_METHOD_HTTP_ROUTE,
        VERIFICATION_METHOD_PRIVATE_VPC,
    ]
}

/// `CreateTargetDomain`, returning the minted `targetDomainId`.
async fn register(
    router: &MultiServiceRouter,
    domain: &str,
    method: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": method}),
        case,
    )
    .await;
    member(&created, "targetDomainId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateTargetDomain minted no targetDomainId: {created}")
    })
}

/// `BatchGetTargetDomains` for `ids`: the `targetDomainId` of every record it found, in
/// the order reported, and every id it reported as not found.
async fn batch_get(
    router: &MultiServiceRouter,
    ids: &[String],
    case: &str,
) -> (Vec<String>, Vec<String>) {
    let payload = ok(
        router,
        "BatchGetTargetDomains",
        &json!({"targetDomainIds": ids}),
        case,
    )
    .await;
    let found = payload
        .get("targetDomains")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no targetDomains list: {payload}"))
        .iter()
        .map(|domain| {
            member(domain, "targetDomainId")
                .unwrap_or_else(|| panic!("case {case:?}: a record carries no id: {domain}"))
        })
        .collect();
    let not_found = payload
        .get("notFound")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no notFound list: {payload}"))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a notFound entry is not a string: {id}"))
                .to_string()
        })
        .collect();
    (found, not_found)
}

/// `DeleteTargetDomain`, requiring HTTP 200 and the deleted id in the answer.
async fn delete(router: &MultiServiceRouter, id: &str, case: &str) {
    let answer = ok(
        router,
        "DeleteTargetDomain",
        &json!({"targetDomainId": id}),
        case,
    )
    .await;
    assert_eq!(
        member(&answer, "targetDomainId").as_deref(),
        Some(id),
        "case {case:?}: expected the delete to report the deleted id {id:?}, got {answer}"
    );
}

/// RULE: once a target domain is deleted its id is unknown to the account, so a batch
/// read reports it in `notFound` and carries no record for it, while every id the account
/// still holds is answered from its stored record.
#[tokio::test]
async fn a_deleted_target_domain_moves_from_the_records_to_not_found() {
    // The rows differ in which of the registered domains is deleted and with which
    // verification method it was registered.
    for victim in 0..verification_methods().len() {
        let router = emulator();
        let mut ids = Vec::new();
        for (index, method) in verification_methods().into_iter().enumerate() {
            let case = format!("victim {victim}, domain {index} ({method})");
            ids.push(register(&router, &format!("d{index}.example.com"), method, &case).await);
        }
        let doomed = ids[victim].clone();
        let surviving: Vec<String> = ids
            .iter()
            .filter(|id| **id != doomed)
            .cloned()
            .collect();
        let case = format!("deleting domain {victim} of {}", ids.len());

        // Before the delete every id is a record.
        let (found, not_found) = batch_get(&router, &ids, &case).await;
        assert_eq!(
            found, ids,
            "case {case:?}: expected every registered domain to be found, got {found:?}"
        );
        assert!(
            not_found.is_empty(),
            "case {case:?}: nothing is missing yet, got {not_found:?}"
        );

        delete(&router, &doomed, &case).await;

        // The batch compositions the read is asked in: the deleted id alone, the whole
        // set, the set in reverse, the deleted id repeated, and the survivors alone.
        let mut reversed = ids.clone();
        reversed.reverse();
        let compositions: Vec<(String, Vec<String>)> = vec![
            (format!("{case}: the deleted id alone"), vec![doomed.clone()]),
            (format!("{case}: every id"), ids.clone()),
            (format!("{case}: every id, reversed"), reversed),
            (
                format!("{case}: the deleted id twice"),
                vec![doomed.clone(), doomed.clone()],
            ),
            (format!("{case}: the survivors alone"), surviving.clone()),
        ];

        for (composition_case, asked) in compositions {
            let (found, not_found) = batch_get(&router, &asked, &composition_case).await;
            // Every id lands in exactly one of the two lists, at the position it was
            // asked at.
            let expected_found: Vec<String> = asked
                .iter()
                .filter(|id| **id != doomed)
                .cloned()
                .collect();
            let expected_missing: Vec<String> = asked
                .iter()
                .filter(|id| **id == doomed)
                .cloned()
                .collect();
            assert_eq!(
                found, expected_found,
                "case {composition_case:?}: expected the records {expected_found:?}, got \
                 {found:?}"
            );
            assert_eq!(
                not_found, expected_missing,
                "case {composition_case:?}: expected {expected_missing:?} in notFound, \
                 got {not_found:?}"
            );
        }
    }
}

/// RULE: a delete the service cannot act on changes nothing — an id the account does not
/// hold (including one it has already deleted) is HTTP 404 `ResourceNotFoundException`,
/// and an id that could never name a record is HTTP 400 `ValidationException`; the
/// domains the account does hold stay readable throughout.
#[tokio::test]
async fn a_delete_the_service_cannot_act_on_is_refused_and_removes_nothing() {
    let router = emulator();
    // Several registrations, so the id the refusal rows mutate carries a letter as well
    // as digits and the upper-cased row really is a different id.
    let mut kept = String::new();
    for index in 0..10 {
        let case = format!("stored domain {index}");
        kept = register(
            &router,
            &format!("kept{index}.example.com"),
            VERIFICATION_METHOD_DNS_TXT,
            &case,
        )
        .await;
    }
    assert!(
        kept.chars().any(|c| c.is_ascii_alphabetic()),
        "expected a minted targetDomainId carrying a letter, got {kept:?}"
    );
    let gone = register(
        &router,
        "gone.example.com",
        VERIFICATION_METHOD_HTTP_ROUTE,
        "the domain deleted up front",
    )
    .await;
    delete(&router, &gone, "the domain deleted up front").await;

    // (case, the id the delete names, the status and code due)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an id already deleted",
            gone.clone(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a never-minted id",
            "ffffffffffffffffffffffffffffffff".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored id upper-cased",
            kept.to_uppercase(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored id with a suffix",
            format!("{kept}0"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored id with its last character dropped",
            kept[..kept.len() - 1].to_string(),
            404,
            "ResourceNotFoundException",
        ),
        ("an empty id", String::new(), 400, "ValidationException"),
    ];

    for (case, target_domain_id, status, code) in rows {
        assert_ne!(
            target_domain_id, kept,
            "case {case:?}: the row must not name the domain that is kept"
        );
        let answer = call(
            &router,
            "DeleteTargetDomain",
            &json!({"targetDomainId": target_domain_id}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {case:?}: expected {status} {code} for {target_domain_id:?}, got {} \
             ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );

        // The domain the account holds is still a record, and the deleted one is still
        // gone.
        let (found, not_found) = batch_get(&router, &[kept.clone(), gone.clone()], case).await;
        assert_eq!(
            found,
            vec![kept.clone()],
            "case {case:?}: expected {kept:?} to still be a record, got {found:?}"
        );
        assert_eq!(
            not_found,
            vec![gone.clone()],
            "case {case:?}: expected {gone:?} to still be missing, got {not_found:?}"
        );
    }
}
