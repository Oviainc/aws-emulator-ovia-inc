// SPDX-License-Identifier: CC0-1.0

//! The ONE de-duplication of an `accountIds` list, driven end to end through an
//! operation of every caller that used to carry its own copy of it.
//!
//! RULE: the accounts an `accountIds` list addresses are the ids it names, each
//! once, in the order each first appears — so every operation that walks such a
//! list acts on an addressed account exactly once, and a response it builds while
//! walking it (the per-account rows of a batch's `errors` list) reports those
//! accounts in the order the request named them. The rule lives in one place,
//! `state::distinct_account_ids`, beside `state::resolve_account_id`, the one
//! place an addressed account is normalised.
//!
//! The rows span what the rule generalises over: a list with no repeat at all,
//! repeats adjacent, separated and interleaved, a list filled by a single id, how
//! many times an id repeats, and how many distinct accounts the list names. Every
//! row names the accounts in an order that is NOT their sorted order, so a copy
//! that walked a sorted set instead of the request fails them.
//!
//! The four callers are reached through one operation each —
//! `BatchDeleteTaxRegistration` and `BatchPutTaxRegistration` (which report per
//! account, so they pin the ORDER too), `BatchGetTaxExemptions` (which reports a
//! map keyed by account) and `PutTaxExemption` (whose writes go through
//! `state::store_exemptions`).
//!
//! The harness (`emulator_with_engine`, `call`, `hold_validations_open`) lives in
//! `tax_registration_lifecycle.rs`, the registration entry in
//! `batch_tax_registration.rs` and the submitted certificate in
//! `tax_exemptions.rs`; nothing here builds a second copy of any of them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    distinct_account_ids, ComAmazonawsTaxsettingsTaxsettingsState,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::batch_tax_registration::entries;
use crate::tax_exemptions::certificate;
use crate::tax_registration_lifecycle::{
    call, emulator_with_engine, hold_validations_open, Answer,
};

/// Five distinct 12-digit account ids the rows draw their lists from.
const A: &str = "000000000001";
const B: &str = "000000000002";
const C: &str = "000000000003";
const D: &str = "000000000004";
const E: &str = "000000000005";

/// One row: the list a request names, and the accounts it addresses — each named
/// id once, in the order it first appears.
struct Row {
    case: &'static str,
    named: &'static [&'static str],
    addressed: &'static [&'static str],
}

/// The rows, each naming at most five ids: the published `@length` of
/// `accountIds` on `BatchDeleteTaxRegistration` and `BatchGetTaxExemptions` is
/// 1..=5 (`smithy_query(query="operation",
/// operation="BatchDeleteTaxRegistration,BatchGetTaxExemptions")`), which the
/// 1..=550 of `BatchPutTaxRegistration` and `PutTaxExemption` also admits.
const ROWS: &[Row] = &[
    Row {
        case: "a single id, nothing repeated",
        named: &[C],
        addressed: &[C],
    },
    Row {
        case: "distinct ids, named out of sorted order",
        named: &[C, A, E, B],
        addressed: &[C, A, E, B],
    },
    Row {
        case: "an adjacent repeat ahead of a smaller id",
        named: &[B, B, A],
        addressed: &[B, A],
    },
    Row {
        case: "a repeat separated by another account",
        named: &[D, A, D],
        addressed: &[D, A],
    },
    Row {
        case: "one id filling the whole list",
        named: &[E, E, E, E, E],
        addressed: &[E],
    },
    Row {
        case: "two ids interleaved ahead of a third",
        named: &[D, B, D, B, A],
        addressed: &[D, B, A],
    },
    Row {
        case: "repeats at both ends of the list",
        named: &[C, A, A, A, C],
        addressed: &[C, A],
    },
];

/// The `accountId`s a batch response's `errors` list reports, in order.
fn failed_accounts(answer: &Answer) -> Vec<String> {
    answer
        .get("errors")
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| error.get("accountId").and_then(Value::as_str))
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The row's addressed accounts as owned strings, for comparing against a
/// response.
fn addressed_of(row: &Row) -> Vec<String> {
    row.addressed
        .iter()
        .map(|account_id| (*account_id).to_string())
        .collect()
}

/// The accounts this service holds a registration for, and the accounts it holds
/// exemption records for with how many each holds — read from the state the
/// handlers share.
fn stored(engine: &Arc<engine::InMemoryEngine>) -> (Vec<String>, Vec<(String, usize)>) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let store = state.lock();
    (
        store.registrations.keys().cloned().collect(),
        store
            .exemptions
            .iter()
            .map(|(account_id, held)| (account_id.clone(), held.len()))
            .collect(),
    )
}

/// Store the registration of each of `accounts`, one single-account write each.
async fn register(router: &MultiServiceRouter, accounts: &[&str]) {
    let entry = &entries()[0].1;
    for account_id in accounts {
        let put = call(
            router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "setup: registering {account_id} must succeed, got {put:?}"
        );
    }
}

/// RULE: the helper answers each named id once, in the order it first appears.
#[test]
fn the_shared_rule_addresses_each_named_id_once_in_first_seen_order() {
    for row in ROWS {
        let case = row.case;
        let named: Vec<String> = row
            .named
            .iter()
            .map(|account_id| (*account_id).to_string())
            .collect();
        let addressed = distinct_account_ids(&named);
        assert_eq!(
            addressed.as_slice(),
            row.addressed,
            "case {case:?}: expected {:?} for {:?}, got {addressed:?}",
            row.addressed,
            row.named
        );
    }
}

/// RULE, through `BatchDeleteTaxRegistration`: the deletion is attempted once per
/// addressed account, in request order — so against a store holding none of them
/// the `errors` list is exactly the addressed accounts, in that order, never a
/// repeated id reported twice.
#[tokio::test]
async fn the_batch_delete_addresses_each_account_once_in_request_order() {
    for row in ROWS {
        let case = row.case;
        let (router, engine) = emulator_with_engine();
        let answer = call(
            &router,
            "BatchDeleteTaxRegistration",
            &json!({ "accountIds": row.named }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: the batch delete must succeed, got {answer:?}"
        );
        let reported = failed_accounts(&answer);
        assert_eq!(
            reported,
            addressed_of(row),
            "case {case:?}: expected one row per addressed account in request order, got {:?}",
            answer.body
        );
        let (registrations, _) = stored(&engine);
        assert!(
            registrations.is_empty(),
            "case {case:?}: a refused delete stores nothing, got {registrations:?}"
        );
    }
}

/// RULE, through `BatchPutTaxRegistration`: the write is performed once per
/// addressed account, in request order — so a store whose records are all
/// outstanding reports exactly the addressed accounts in that order, and an empty
/// store ends up holding one record per addressed account and no error row.
#[tokio::test]
async fn the_batch_write_addresses_each_account_once_in_request_order() {
    let entry = &entries()[0].1;
    for row in ROWS {
        let case = row.case;
        let addressed = addressed_of(row);

        // Every addressed account already holds a TRN whose validation is still
        // outstanding, which the write refuses — one row per addressed account,
        // in the order the request named them.
        let (router, engine) = emulator_with_engine();
        register(&router, row.addressed).await;
        hold_validations_open(&engine);
        let refused = call(
            &router,
            "BatchPutTaxRegistration",
            &json!({ "accountIds": row.named, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (200, ""),
            "case {case:?}: the batch write must succeed, got {refused:?}"
        );
        assert_eq!(
            failed_accounts(&refused),
            addressed,
            "case {case:?}: expected one row per addressed account in request order, got {:?}",
            refused.body
        );

        // The same list against an empty store: one record per addressed
        // account, nothing reported as failed.
        let (fresh_router, fresh_engine) = emulator_with_engine();
        let written = call(
            &fresh_router,
            "BatchPutTaxRegistration",
            &json!({ "accountIds": row.named, "taxRegistrationEntry": entry }),
        )
        .await;
        assert_eq!(
            (written.status, written.error_code.as_str()),
            (200, ""),
            "case {case:?}: the batch write must succeed, got {written:?}"
        );
        assert_eq!(
            failed_accounts(&written),
            Vec::<String>::new(),
            "case {case:?}: no addressed account may be reported as failed, got {:?}",
            written.body
        );
        let (registrations, _) = stored(&fresh_engine);
        let mut expected = addressed.clone();
        expected.sort();
        assert_eq!(
            registrations, expected,
            "case {case:?}: expected one registration per addressed account, got \
             {registrations:?}"
        );
    }
}

/// RULE, through `PutTaxExemption` (whose writes go through
/// `state::store_exemptions`) and `BatchGetTaxExemptions`: the submission records
/// one exemption per addressed account and the read reports each addressed
/// account once.
#[tokio::test]
async fn the_exemption_write_and_read_address_each_account_once() {
    for row in ROWS {
        let case = row.case;
        let mut addressed = addressed_of(row);
        addressed.sort();

        let (router, engine) = emulator_with_engine();
        let submitted = call(
            &router,
            "PutTaxExemption",
            &json!({
                "accountIds": row.named,
                "authority": { "country": "US" },
                // An entry of the catalog this service publishes, named by its
                // displayName (`tests/all/tax_exemption_types.rs` pins the
                // catalog's wire form).
                "exemptionType": "501(c)(3)",
                "exemptionCertificate": certificate("exemption.pdf"),
            }),
        )
        .await;
        assert_eq!(
            (submitted.status, submitted.error_code.as_str()),
            (200, ""),
            "case {case:?}: the exemption submission must succeed, got {submitted:?}"
        );
        let (_, held) = stored(&engine);
        let expected_records: Vec<(String, usize)> = addressed
            .iter()
            .map(|account_id| (account_id.clone(), 1))
            .collect();
        assert_eq!(
            held, expected_records,
            "case {case:?}: expected exactly one exemption record per addressed account, got \
             {held:?}"
        );

        let read = call(
            &router,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": row.named }),
        )
        .await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (200, ""),
            "case {case:?}: the exemption read must succeed, got {read:?}"
        );
        let mut reported: Vec<String> = read
            .get("taxExemptionDetailsMap")
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        reported.sort();
        assert_eq!(
            reported, addressed,
            "case {case:?}: expected each addressed account reported once, got {:?}",
            read.body
        );
        assert_eq!(
            read.get("failedAccounts").as_array().map(Vec::len),
            Some(0),
            "case {case:?}: no addressed account failed, got {:?}",
            read.body
        );
    }
}
