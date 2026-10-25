// SPDX-License-Identifier: CC0-1.0

//! What a repeated account id means in a TaxSettings `accountIds` list.
//!
//! Every operation of this service that takes `accountIds` documents it as a
//! "List of unique account identifiers", so an id named more than once names ONE
//! account and ONE action on it. The rule under test: such a request answers
//! exactly as the same request with the repeats collapsed does — the same
//! response and the same state afterwards — so no per-account row of the
//! response reports an action this very call performed.
//!
//! The rows span the class the rule generalises over: how many times an id is
//! repeated (twice up to the published maximum count), where the repeats sit in
//! the list (adjacent, separated, interleaved), how many distinct accounts the
//! list names, and what those accounts already hold — crossed with every
//! operation of this service whose input carries an `accountIds` list
//! (`BatchDeleteTaxRegistration`, `BatchPutTaxRegistration`,
//! `BatchGetTaxExemptions`, `PutTaxExemption`).
//!
//! The harness (`emulator`, `emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs`, the registration entries in
//! `batch_tax_registration.rs` and the submitted certificate in
//! `tax_exemptions.rs`; nothing here builds a second copy of any of them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::ComAmazonawsTaxsettingsTaxsettingsState;

use crate::batch_tax_registration::entries;
use crate::tax_exemptions::certificate;
use crate::tax_registration_lifecycle::{
    call, emulator, emulator_with_engine, make_validations_due, Answer,
};

/// Three distinct 12-digit account ids the rows draw their lists from.
const A: &str = "000000000001";
const B: &str = "000000000002";
const C: &str = "000000000003";

/// One row: the list a request names, and which of those accounts already hold
/// the resource the operation acts on.
struct Row {
    case: &'static str,
    named: &'static [&'static str],
    held: &'static [&'static str],
}

/// The rows, each naming at most five accounts — the published maximum of
/// `accountIds` on `BatchDeleteTaxRegistration` and `BatchGetTaxExemptions`
/// (`smithy_query(query="operation", operation="BatchDeleteTaxRegistration")`
/// shows `@length` 1..=5), which `PutTaxExemption` and
/// `BatchPutTaxRegistration` (1..=550) also admit.
const ROWS: &[Row] = &[
    Row {
        case: "one account named twice, which holds the resource",
        named: &[A, A],
        held: &[A],
    },
    Row {
        case: "one account named twice, holding nothing",
        named: &[A, A],
        held: &[],
    },
    Row {
        case: "a repeat separated by another account",
        named: &[A, B, A],
        held: &[A, B],
    },
    Row {
        case: "a repeat separated by an account holding nothing",
        named: &[A, B, A],
        held: &[A],
    },
    Row {
        case: "one account filling the whole list",
        named: &[A, A, A, A, A],
        held: &[A],
    },
    Row {
        case: "two accounts interleaved beside a third",
        named: &[A, B, A, B, C],
        held: &[A, C],
    },
    Row {
        case: "adjacent repeats of three accounts holding nothing",
        named: &[A, A, B, B, C],
        held: &[],
    },
];

/// The accounts `named` names, each once, in the order they first appear — the
/// list the rule says the request is equivalent to.
fn collapsed(named: &[&str]) -> Vec<String> {
    let mut distinct: Vec<String> = Vec::new();
    for account_id in named {
        if !distinct.iter().any(|seen| seen == account_id) {
            distinct.push((*account_id).to_string());
        }
    }
    distinct
}

/// The `accountId`s the `errors` list of a batch response reports, in order.
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

/// The accounts this service holds a tax registration for, and how many
/// exemption records it holds in total — read from the state the handlers share.
fn stored(engine: &Arc<engine::InMemoryEngine>) -> (Vec<String>, usize) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let store = state.lock();
    (
        store.registrations.keys().cloned().collect(),
        store.stored_exemption_count(),
    )
}

/// Store the registration of each account of `accounts`.
async fn register(router: &smithy_server_runtime::MultiServiceRouter, accounts: &[&str]) {
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

/// RULE: a batch delete removes the registration of each account its list names
/// once, so an id named more than once names one deletion and is reported at
/// most once — never as a "not found" failure of a deletion this same call
/// performed — and the call answers exactly as the same request with the repeats
/// collapsed does.
#[tokio::test]
async fn a_repeated_account_id_names_one_deletion() {
    for row in ROWS {
        let case = row.case;
        let distinct = collapsed(row.named);
        // Only the accounts that held nothing may be reported, each once, in the
        // order they first appear in the request.
        let expected: Vec<String> = distinct
            .iter()
            .filter(|account_id| !row.held.contains(&account_id.as_str()))
            .cloned()
            .collect();

        let (router, engine) = emulator_with_engine();
        register(&router, row.held).await;
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
        assert_eq!(
            failed_accounts(&answer),
            expected,
            "case {case:?}: exactly the accounts holding no registration must be reported, got \
             {:?}",
            answer.body
        );

        // Every account the list named now holds nothing: the deletion the
        // repeats share did happen.
        for account_id in &distinct {
            let read = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": account_id }),
            )
            .await;
            assert_eq!(
                read.status, 404,
                "case {case:?}: {account_id} must hold nothing after the delete, got {read:?}"
            );
        }
        let (held_after, _) = stored(&engine);
        assert!(
            held_after.is_empty(),
            "case {case:?}: the delete must leave no registration behind, got {held_after:?}"
        );

        // The same request with the repeats collapsed answers identically.
        let collapsed_router = emulator();
        register(&collapsed_router, row.held).await;
        let collapsed_answer = call(
            &collapsed_router,
            "BatchDeleteTaxRegistration",
            &json!({ "accountIds": &distinct }),
        )
        .await;
        assert_eq!(
            (answer.status, &answer.body),
            (collapsed_answer.status, &collapsed_answer.body),
            "case {case:?}: expected the answer of {distinct:?}, got {:?} for {:?}",
            answer.body,
            row.named
        );
    }
}

/// RULE (the same rule, over the sibling operations): a repeated id names one
/// account and one action in every operation of this service whose input carries
/// an `accountIds` list, so the batch write stores one registration per distinct
/// account and reports no failure, the exemption read reports each distinct
/// account once, and the exemption write records one exemption per distinct
/// account.
#[tokio::test]
async fn a_repeated_account_id_names_one_account_in_the_sibling_batch_operations() {
    let entry = &entries()[0].1;
    for row in ROWS {
        let case = row.case;
        let distinct = collapsed(row.named);

        // `BatchPutTaxRegistration`: one registration per distinct account, no
        // error row, whatever the repeats.
        let (router, engine) = emulator_with_engine();
        register(&router, row.held).await;
        // A registration whose validation the clock has already settled, so the
        // accounts that already hold one are not refused for a pending TRN:
        // what this row varies is the repeats, not the pending state
        // (`tests/all/pending_trn_conflict.rs` covers that refusal).
        make_validations_due(&engine);
        let written = call(
            &router,
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
            "case {case:?}: no account may be reported as failed, got {:?}",
            written.body
        );
        let (held_after, _) = stored(&engine);
        let mut expected_registrations = distinct.clone();
        expected_registrations.sort();
        assert_eq!(
            held_after, expected_registrations,
            "case {case:?}: expected one registration per distinct account, got {held_after:?}"
        );

        // `PutTaxExemption`: one exemption record per distinct account, and one
        // support case for the one submission.
        let (exemption_router, exemption_engine) = emulator_with_engine();
        let submitted = call(
            &exemption_router,
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
        let (_, records) = stored(&exemption_engine);
        assert_eq!(
            records,
            distinct.len(),
            "case {case:?}: expected one exemption record per distinct account ({}), got \
             {records}",
            distinct.len()
        );

        // `BatchGetTaxExemptions`: each distinct account reported once. The read
        // names at most the five accounts the published bound admits.
        let read = call(
            &exemption_router,
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
        let mut expected = distinct.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected each distinct account reported once, got {:?}",
            read.body
        );
        assert_eq!(
            read.get("failedAccounts").as_array().map(Vec::len),
            Some(0),
            "case {case:?}: no named account failed, got {:?}",
            read.body
        );
    }
}
