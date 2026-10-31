// SPDX-License-Identifier: CC0-1.0

//! The batch that WRITES against an invoice of an agreement, driven on a store
//! no other call has read, end to end over HTTP.
//!
//! Every call here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: a call that resolves the agreement and the invoice its
//! input names reaches them as the clock has already left the store, so whether
//! a write is served never depends on an unrelated call having looked first. An
//! agreement reaches its own stored end date at that instant whether or not
//! anybody calls, and `EndTimeBehaviorType` `RENEW` is "a new agreement is
//! created from the accepted terms of this agreement"
//! (`smithy_query(query="shape", shape="EndTimeBehavior")`) — so from that date
//! on the renewal is an agreement of the store and the invoices its acceptance
//! billed are invoices of the store, which is what an entry of
//! `BatchCreateBillingAdjustmentRequest` names ("each entry specifies an
//! invoice and the adjustment details",
//! `smithy_query(query="operation", operation="BatchCreateBillingAdjustmentRequest")`).
//! Settling admits the records the clock owed and nothing else, so an id no end
//! date could have created is still reported on the per-entry `errors` channel.
//!
//! The rows differ along what the rule generalises over: which contract length
//! the acceptor purchased, how many whole lengths have passed since it was
//! signed (one, two, as many as the renewal term allows), which link of the
//! chain the entry adjusts, and how many entries one batch carries.
//!
//! The clock cannot be wound forward 30 days inside a test, so the agreement's
//! own stored window is wound BACK instead: the harness that does it
//! (`aged_rows`, `aged_store`, `chain_after_an_unrelated_read`) is the one
//! `unsettled_store_single_agreement_reads` owns, the wire harness (`describe`,
//! `invoices_of`) is `agreement_lifecycle`'s, and the batch (`entry`, `get`,
//! `submit`) is `billing_adjustments`'.

use serde_json::Value;

use crate::agreement_lifecycle::{describe, invoices_of, Answer};
use crate::billing_adjustments::{entry, get, submit};
use crate::unsettled_store_single_agreement_reads::{
    aged_rows, aged_store, chain_after_an_unrelated_read,
};

/// The per-entry error code the batch reports for an entry naming a resource
/// the store holds nothing under (`state::BILLING_ADJUSTMENT_ERROR_NOT_FOUND`),
/// spelled out here rather than imported so a handler reporting another one
/// fails these rows instead of agreeing with them.
const NOT_FOUND_CODE: &str = "RESOURCE_NOT_FOUND_EXCEPTION";

/// The status a request this batch stores is submitted in
/// (`BillingAdjustmentStatus`, `smithy_query(query="shape",
/// shape="BillingAdjustmentStatus")`).
const PENDING: &str = "PENDING";

/// An `agreementId` within `AgreementId`'s `^[A-Za-z0-9_/-]+$` that names no
/// agreement any end date of these stores could have created.
const UNKNOWN_AGREEMENT: &str = "agmt-ffffffffffffffff";

/// An `originalInvoiceId` no invoice of these stores carries, so nothing the
/// clock owed can resolve it.
const UNKNOWN_INVOICE: &str = "inv-ffffffffffffffff";

/// The amount each entry asks to credit, well within what a prepaid invoice of
/// these rows bills.
const AMOUNT: &str = "1";

/// `BillingAdjustmentRequestId`'s `^ba-[a-zA-Z0-9]+$`, checked here rather than
/// through the crate's own code so a wrong minter cannot make this test pass.
fn is_billing_adjustment_id(value: &str) -> bool {
    value
        .strip_prefix("ba-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The `errors` channel of a batch answer.
fn batch_errors(answer: &Answer) -> Vec<Value> {
    answer.body["errors"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("errors is a required member, got {:?}", answer.body))
}

/// The `billingAdjustmentRequestId` of every item the batch created, in the
/// order it answered them.
fn created_ids(answer: &Answer) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("items is a required member, got {:?}", answer.body))
        .iter()
        .map(|item| {
            item["billingAdjustmentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert a batch was served and created one request per entry, answering the
/// ids it created.
fn assert_created(case: &str, entries: usize, answer: &Answer) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the batch must be served, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        batch_errors(answer).is_empty(),
        "case {case:?}: every entry names an invoice this store holds, got {:?}",
        answer.body
    );
    let ids = created_ids(answer);
    assert_eq!(
        ids.len(),
        entries,
        "case {case:?}: expected {entries} created request(s), got {ids:?}"
    );
    for id in &ids {
        assert!(
            is_billing_adjustment_id(id),
            "case {case:?}: expected a minted billing adjustment id, got {id:?}"
        );
    }
    ids
}

/// The entry an adjustment of `invoice` is submitted as, under a token of its
/// own.
fn adjustment(agreement: &str, invoice: &str, token: &str) -> Value {
    entry(agreement, invoice, AMOUNT, token)
}

/// An entry naming the invoice of a renewal the clock owes is CREATED by a
/// batch that is the first call its store sees, and the request it stored is the
/// one a read of it reports — over every contract length and chain depth these
/// rows cover.
#[tokio::test]
async fn the_batch_adjusts_the_invoice_of_a_renewal_the_clock_owes_without_an_earlier_call() {
    for (index, row) in aged_rows().into_iter().enumerate() {
        let case = row.case;
        let proposal = format!("ap-unsettledadjust{index}");

        // A store of the row an unrelated call read first, which says which
        // agreements the clock owes this chain and which invoices each of them
        // was billed.
        let (probe, source) = aged_store(&row, &proposal).await;
        let chain = chain_after_an_unrelated_read(&probe, &source).await;
        assert_eq!(
            chain.len(),
            row.renewals + 1,
            "case {case:?}: expected the source and {} renewal(s), got {chain:?}",
            row.renewals
        );

        // Every link of the chain, so the entry adjusts the first renewal, the
        // renewal of that renewal, and so on.
        let mut adjusted: Vec<(String, String)> = Vec::new();
        for agreement in &chain {
            let invoices = invoices_of(&probe, agreement).await;
            let invoice = invoices.first().cloned().unwrap_or_else(|| {
                panic!("case {case:?}: {agreement} is billed its prepaid charge, got {invoices:?}")
            });
            adjusted.push((agreement.clone(), invoice));
        }

        for (link, (agreement, invoice)) in adjusted.iter().enumerate() {
            let token = format!("tok-unsettled{index}-{link}");

            // A store of its own, built identically, where the batch is the
            // FIRST call it sees: ids are minted from the store's own counter,
            // so the identical setup mints the identical ones.
            let (unsettled, same) = aged_store(&row, &proposal).await;
            assert_eq!(
                same, source,
                "case {case:?}: the identical setup must mint the identical agreement id"
            );
            let first = submit(&unsettled, vec![adjustment(agreement, invoice, &token)]).await;
            let ids = assert_created(case, 1, &first);
            let created = ids[0].clone();

            // The record the entry stored, read back through the operation that
            // reports one: it is an adjustment OF that invoice of that
            // agreement, not of whatever the batch was handed.
            let read = get(&unsettled, agreement, &created).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the created request {created} must be readable, got {} with {:?}",
                read.status, read.body
            );
            for (member, expected) in [
                ("/billingAdjustmentRequestId", created.as_str()),
                ("/agreementId", agreement.as_str()),
                ("/originalInvoiceId", invoice.as_str()),
                ("/adjustmentAmount", AMOUNT),
                ("/status", PENDING),
            ] {
                assert_eq!(
                    read.str_at(member),
                    expected,
                    "case {case:?}: expected {member} {expected:?}, got {:?}",
                    read.body
                );
            }

            // The identical batch on a store an unrelated read settled first:
            // the same outcome, so the write never depended on that read.
            let (settled, same) = aged_store(&row, &proposal).await;
            assert_eq!(
                same, source,
                "case {case:?}: the identical setup must mint the identical agreement id"
            );
            let described = describe(&settled, &source).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the unrelated read must succeed, got {} with {:?}",
                described.status, described.body
            );
            let after = submit(&settled, vec![adjustment(agreement, invoice, &token)]).await;
            assert_created(case, 1, &after);
            assert_eq!(
                after.body, first.body,
                "case {case:?}: expected the batch to answer what the settled store answers \
                 {:?}, got {:?}",
                after.body, first.body
            );
        }
    }
}

/// The same rule where one batch carries SEVERAL entries against an invoice of
/// a renewal the clock owes: every entry is created, each under its own token
/// and with an id of its own.
#[tokio::test]
async fn every_entry_of_one_batch_reaches_the_invoice_of_a_renewal_the_clock_owes() {
    let row = aged_rows()
        .into_iter()
        .find(|row| row.renewals > 1)
        .expect("the rows cover a chain the clock owes more than one renewal");
    let case = row.case;
    let proposal = "ap-unsettledadjustbatch";

    let (probe, source) = aged_store(&row, proposal).await;
    let chain = chain_after_an_unrelated_read(&probe, &source).await;
    let renewal = chain
        .last()
        .cloned()
        .expect("the chain holds at least the source");
    let invoices = invoices_of(&probe, &renewal).await;
    let invoice = invoices
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the renewal {renewal} is billed its prepaid charge"));

    let (unsettled, same) = aged_store(&row, proposal).await;
    assert_eq!(
        same, source,
        "case {case:?}: the identical setup must mint the identical agreement id"
    );
    let entries: Vec<Value> = (0..3)
        .map(|index| adjustment(&renewal, &invoice, &format!("tok-unsettledbatch-{index}")))
        .collect();
    let answer = submit(&unsettled, entries).await;
    let ids = assert_created(case, 3, &answer);
    let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "case {case:?}: each entry stores a request of its own, got {ids:?}"
    );
    for id in &ids {
        let read = get(&unsettled, &renewal, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created request {id} must be readable, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.str_at("/originalInvoiceId"),
            invoice,
            "case {case:?}: expected the request to adjust {invoice}, got {:?}",
            read.body
        );
    }
}

/// The complement: settling the clock admits the agreements and invoices it
/// owed and nothing else, so an entry naming an id no end date could have
/// created is still reported on the batch's per-entry `errors` channel and
/// stores nothing.
#[tokio::test]
async fn an_entry_naming_what_no_end_date_created_is_still_reported_not_found() {
    let row = aged_rows()
        .into_iter()
        .next()
        .expect("the rows cover a chain the clock owes a renewal");
    let proposal = "ap-unsettledadjustunknown";

    let (probe, source) = aged_store(&row, proposal).await;
    let chain = chain_after_an_unrelated_read(&probe, &source).await;
    let renewal = chain
        .last()
        .cloned()
        .expect("the chain holds at least the source");
    let invoices = invoices_of(&probe, &renewal).await;
    let invoice = invoices
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the renewal {renewal} is billed its prepaid charge"));

    // The rows: an agreement no end date created, and an invoice no agreement
    // of the store was billed — each on a store the batch is the first call of.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an agreement no end date created",
            UNKNOWN_AGREEMENT.to_string(),
            invoice.clone(),
        ),
        (
            "an invoice of a renewal the clock owes that nothing issued",
            renewal.clone(),
            UNKNOWN_INVOICE.to_string(),
        ),
    ];

    for (case, agreement, named_invoice) in rows {
        let (unsettled, same) = aged_store(&row, proposal).await;
        assert_eq!(
            same, source,
            "case {case:?}: the identical setup must mint the identical agreement id"
        );
        let answer = submit(
            &unsettled,
            vec![adjustment(&agreement, &named_invoice, "tok-unsettled-unknown")],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a batch whose entry names nothing is still answered, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            created_ids(&answer).is_empty(),
            "case {case:?}: the entry names nothing to adjust, so nothing is created, got {:?}",
            answer.body
        );
        let errors = batch_errors(&answer);
        assert_eq!(
            errors.len(),
            1,
            "case {case:?}: expected the one entry reported on the errors channel, got {:?}",
            answer.body
        );
        assert_eq!(
            errors[0]["code"].as_str().unwrap_or_default(),
            NOT_FOUND_CODE,
            "case {case:?}: expected the code {NOT_FOUND_CODE}, got {:?}",
            answer.body
        );
        assert_eq!(
            errors[0]["clientToken"].as_str().unwrap_or_default(),
            "tok-unsettled-unknown",
            "case {case:?}: the error is reported under the entry's own token, got {:?}",
            answer.body
        );
    }
}
