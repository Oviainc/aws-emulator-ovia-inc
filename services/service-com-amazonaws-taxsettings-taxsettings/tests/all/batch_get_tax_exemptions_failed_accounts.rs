// SPDX-License-Identifier: CC0-1.0

//! `BatchGetTaxExemptions` as a BATCH: what it answers for a request that mixes
//! accounts this service can report on with accounts it cannot.
//!
//! The rule under test: a batch read whose response models a per-account failure
//! list answers 200 and reports every account it could not get the resource for
//! in that list, while still reporting the data of the accounts named beside it
//! — one unreportable id costs the caller the one id, not the call.
//!
//! The rows span the class the rule generalises over: how many of the named
//! accounts are unreportable (none, one, two, and all five the published
//! `@length` bound of `accountIds` admits), where they sit in the list (first,
//! last, between two that report), whether one is named twice, and what the
//! reportable accounts hold (an exemption, only a tax registration, nothing at
//! all — the caller's own account).
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs` and
//! the submitted certificate in `tax_exemptions.rs`; nothing here builds a second
//! copy of either.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_exemptions::certificate;
use crate::tax_registration_lifecycle::{call, emulator, Answer};

/// Accounts this service holds a recorded exemption for.
const WITH_EXEMPTION: [&str; 2] = ["000000000011", "000000000012"];
/// An account this service holds a tax registration but no exemption for.
const WITH_REGISTRATION: &str = "000000000013";
/// Well-formed 12-digit ids this service holds nothing at all for, so it has no
/// tax exemptions to report for them.
const UNREPORTABLE: [&str; 5] = [
    "999999999991",
    "999999999992",
    "999999999993",
    "999999999994",
    "999999999995",
];

/// One row: the accounts a call names, the accounts the reported map must carry
/// (sorted) and the accounts `failedAccounts` must carry, in the order the
/// request first named them.
struct Row {
    case: &'static str,
    named: Vec<String>,
    reported: Vec<String>,
    failed: Vec<String>,
}

/// The account this emulator presents as the caller's own.
fn own_account() -> String {
    engine::emulator_account_id().to_string()
}

fn id(account: &str) -> String {
    account.to_string()
}

/// The account ids the reported map carries, sorted.
fn reported_accounts(answer: &Answer) -> Vec<String> {
    let mut accounts: Vec<String> = answer
        .get("taxExemptionDetailsMap")
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    accounts.sort();
    accounts
}

/// Make `account_id` an account this service holds an exemption for, by
/// submitting one over the wire.
async fn record_an_exemption(router: &MultiServiceRouter, account_id: &str) {
    let submitted = call(
        router,
        "PutTaxExemption",
        &json!({
            "accountIds": [account_id],
            "authority": { "country": "US" },
            // An entry of the catalog this service publishes
            // (`tests/all/tax_exemption_types.rs` pins its wire form).
            "exemptionType": "Reseller",
            "exemptionCertificate": certificate("exemption.pdf"),
        }),
    )
    .await;
    assert_eq!(
        (submitted.status, submitted.error_code.as_str()),
        (200, ""),
        "case \"setup\": the exemption for {account_id} must record, got {submitted:?}"
    );
}

/// The emulator with the two exempt accounts and the registered one in place.
async fn seeded_emulator() -> MultiServiceRouter {
    let router = emulator();
    for account_id in WITH_EXEMPTION {
        record_an_exemption(&router, account_id).await;
    }
    let stored = call(
        &router,
        "PutTaxRegistration",
        &json!({
            "accountId": WITH_REGISTRATION,
            "taxRegistrationEntry": {
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            },
        }),
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "case \"setup\": the registration must store, got {stored:?}"
    );
    router
}

/// RULE: the batch read reports each account it cannot get tax exemptions for in
/// the response's `failedAccounts` list on a 200, and reports the exemptions of
/// every account named beside it in `taxExemptionDetailsMap`.
#[tokio::test]
async fn an_unreportable_account_fails_on_its_own_row_not_the_whole_call() {
    let router = seeded_emulator().await;
    let [first_exempt, second_exempt] = WITH_EXEMPTION;
    let [u1, u2, u3, u4, u5] = UNREPORTABLE;

    let rows = vec![
        Row {
            case: "no unreportable account at all",
            named: vec![id(first_exempt), id(second_exempt), id(WITH_REGISTRATION)],
            reported: vec![id(first_exempt), id(second_exempt), id(WITH_REGISTRATION)],
            failed: vec![],
        },
        Row {
            case: "one unreportable account, named alone",
            named: vec![id(u1)],
            reported: vec![],
            failed: vec![id(u1)],
        },
        Row {
            case: "an unreportable account named last",
            named: vec![id(first_exempt), id(u1)],
            reported: vec![id(first_exempt)],
            failed: vec![id(u1)],
        },
        Row {
            case: "an unreportable account named first",
            named: vec![id(u2), id(first_exempt)],
            reported: vec![id(first_exempt)],
            failed: vec![id(u2)],
        },
        Row {
            case: "an unreportable account between two that report",
            named: vec![id(first_exempt), id(u3), id(second_exempt)],
            reported: vec![id(first_exempt), id(second_exempt)],
            failed: vec![id(u3)],
        },
        Row {
            // Five ids is the published `@length` bound of `accountIds`.
            case: "two unreportable accounts among five named",
            named: vec![
                id(first_exempt),
                id(u1),
                id(WITH_REGISTRATION),
                id(u4),
                id(second_exempt),
            ],
            reported: vec![id(first_exempt), id(second_exempt), id(WITH_REGISTRATION)],
            failed: vec![id(u1), id(u4)],
        },
        Row {
            case: "every one of the five named accounts unreportable",
            named: vec![id(u1), id(u2), id(u3), id(u4), id(u5)],
            reported: vec![],
            failed: vec![id(u1), id(u2), id(u3), id(u4), id(u5)],
        },
        Row {
            // `accountIds` is a "List of unique account identifiers": a repeated
            // id names ONE account, so it fails once.
            case: "an unreportable account named twice",
            named: vec![id(u5), id(first_exempt), id(u5)],
            reported: vec![id(first_exempt)],
            failed: vec![id(u5)],
        },
        Row {
            case: "an unreportable account beside the caller's own, which holds no exemption",
            named: vec![own_account(), id(u2)],
            reported: vec![own_account()],
            failed: vec![id(u2)],
        },
        Row {
            case: "an unreportable account beside one holding only a registration",
            named: vec![id(WITH_REGISTRATION), id(u3)],
            reported: vec![id(WITH_REGISTRATION)],
            failed: vec![id(u3)],
        },
    ];

    for row in &rows {
        let case = row.case;
        let answer = call(
            &router,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": row.named }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected 200 whatever the named accounts hold, got {answer:?}"
        );

        let mut expected = row.reported.clone();
        expected.sort();
        assert_eq!(
            reported_accounts(&answer),
            expected,
            "case {case:?}: expected the map to carry {expected:?}, got {:?}",
            answer.get("taxExemptionDetailsMap")
        );
        assert_eq!(
            answer.get("failedAccounts"),
            &json!(row.failed),
            "case {case:?}: expected the failed accounts {:?}, got {:?}",
            row.failed,
            answer.body
        );

        // Each account the map carries carries its own details — the inheritance
        // indicator every reported account has, and the exemptions of the
        // accounts that hold some — so a partial answer is a real one rather than
        // an empty placeholder.
        for account_id in &expected {
            let details = answer
                .get("taxExemptionDetailsMap")
                .get(account_id)
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                details.get("heritageObtainedDetails"),
                Some(&json!(false)),
                "case {case:?} ({account_id}): expected the inheritance indicator, got {details}"
            );
            let exemptions = details
                .get("taxExemptions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                !exemptions.is_empty(),
                WITH_EXEMPTION.contains(&account_id.as_str()),
                "case {case:?} ({account_id}): expected the exemptions this service holds for it, \
                 got {details}"
            );
        }
    }
}

/// RULE: reporting an account as failed is a report — it records nothing, so the
/// same request answers the same thing again and the listing still knows only
/// the accounts that were written.
#[tokio::test]
async fn a_failed_account_is_not_recorded_by_being_read() {
    let router = seeded_emulator().await;
    let [first_exempt, _] = WITH_EXEMPTION;
    let request = json!({ "accountIds": [UNREPORTABLE[0], first_exempt, UNREPORTABLE[1]] });

    let first = call(&router, "BatchGetTaxExemptions", &request).await;
    let second = call(&router, "BatchGetTaxExemptions", &request).await;
    assert_eq!(
        (first.status, first.error_code.as_str()),
        (200, ""),
        "case \"a first read\": expected 200, got {first:?}"
    );
    assert_eq!(
        second.body, first.body,
        "case \"a second read\": expected the same report, got {:?}",
        second.body
    );

    let listed = call(&router, "ListTaxExemptions", &json!({})).await;
    assert_eq!(
        (listed.status, listed.error_code.as_str()),
        (200, ""),
        "case \"the listing after the reads\": expected 200, got {listed:?}"
    );
    let mut listed_accounts: Vec<String> = listed
        .get("taxExemptionDetailsMap")
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    listed_accounts.sort();
    let mut expected: Vec<String> = WITH_EXEMPTION
        .iter()
        .map(|account| id(account))
        .chain(std::iter::once(id(WITH_REGISTRATION)))
        .chain(std::iter::once(own_account()))
        .collect();
    expected.sort();
    expected.dedup();
    assert_eq!(
        listed_accounts, expected,
        "case \"the listing after the reads\": expected the accounts this service answers for \
         {expected:?}, got {:?}",
        listed.body
    );
}
