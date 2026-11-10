// SPDX-License-Identifier: CC0-1.0

//! The pending-TRN rule, over both writers of a tax registration.
//!
//! RULE (the model documents it on `PutTaxRegistration` and, word for word, on
//! `BatchPutTaxRegistration`): "You can't set a TRN if there's a pending TRN.
//! You'll need to delete the pending TRN first." So a write aimed at an account
//! whose stored registration is still in the non-terminal `Pending` status is
//! REFUSED and that record is left exactly as it was, on whichever channel the
//! writer renders a conflict on — a 409 `ConflictException` response for the
//! single-account write, a per-account row of the batch response's `errors` list
//! (with every other account the batch named still stored).
//!
//! The tables span the classes the rule generalises over: both writers, on each
//! side of the setup and of the refused write; every value of the model's
//! `TaxRegistrationStatus` enum, so that the refusal is pinned to `Pending`
//! alone; several entries; the document shapes a stored record can carry, whose
//! reported access tokens the refused write must leave naming the documents the
//! stored record's own write minted them for; and, for the batch, the mix of
//! pending, settled and unregistered accounts one request can name, over several
//! counts.
//!
//! The harness (`emulator_with_engine`, `call`, `make_validations_due`) lives in
//! `tax_registration_lifecycle.rs`, the writer fan-out (`WRITERS`, `write`) and
//! the certificate document (`document`) in `country_rules.rs` and the status
//! catalog in `fidelity_rules.rs`; nothing here builds a second one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, StoredTaxRegistration, ERROR_CODE_GENERAL_CONFLICT,
    PENDING_REGISTRATION_CONFLICT_MESSAGE, STATUS_PENDING, STATUS_VERIFIED,
};

use crate::country_rules::{document, write, WRITERS};
use crate::fidelity_rules::REGISTRATION_STATUSES;
use crate::tax_registration_lifecycle::{call, emulator_with_engine, make_validations_due, Answer};

/// The batch writer, whose conflict channel is a row of `errors` rather than a
/// response status.
const BATCH_WRITER: &str = "BatchPutTaxRegistration";

/// The entries a case stores and then tries to overwrite: `(case, stored,
/// submitted)`.
///
/// They differ in registration type, sector, country and whether they carry the
/// country block that country requires — none of which the rule depends on, so
/// one row submits the very entry that is already stored: the refusal is about
/// the STATE of the account's record, not about what the new entry says.
fn entry_pairs() -> Vec<(&'static str, Value, Value)> {
    let vat = json!({
        "registrationId": "DE136695976",
        "registrationType": "VAT",
        "legalName": "Acme GmbH",
        "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
    });
    let gst = json!({
        "registrationId": "29ABCDE1234F1Z5",
        "registrationType": "GST",
        "legalName": "Acme India Private Limited",
        "sector": "Business",
        "certifiedEmailId": "billing.team+trn@example.com",
        "legalAddress": {
            "addressLine1": "1 Residency Road",
            "city": "Bengaluru",
            "stateOrRegion": "Karnataka",
            "postalCode": "560025",
            "countryCode": "IN",
        },
    });
    let kenyan = json!({
        "registrationId": "P123456789K",
        "registrationType": "VAT",
        "legalName": "Acme Kenya Limited",
        "legalAddress": {
            "addressLine1": "Kenyatta Ave 1",
            "city": "Nairobi",
            "postalCode": "00100",
            "countryCode": "KE",
        },
        "additionalTaxInformation": {
            "kenyaAdditionalInfo": { "personType": "Legal Person" },
        },
    });
    vec![
        ("a VAT entry overwritten by a GST entry", vat.clone(), gst.clone()),
        ("a GST entry overwritten by itself", gst.clone(), gst),
        ("a Kenyan entry overwritten by a VAT entry", kenyan, vat),
    ]
}

/// Edit `account_id`'s stored record in place.
///
/// The per-account counterpart of the suite's whole-store clock helpers
/// (`hold_validations_open` / `make_validations_due`): a case here needs ONE
/// store holding accounts in several states at once, which a helper that moves
/// every record cannot build.
fn with_record(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
    edit: impl FnOnce(&mut StoredTaxRegistration),
) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    let stored = store
        .registrations
        .get_mut(account_id)
        .unwrap_or_else(|| panic!("account {account_id} must already hold a record"));
    edit(stored);
}

/// Hold `account_id`'s TRN validation open, so its record stays the outstanding
/// `Pending` one however long the test takes to reach the next call.
///
/// `pub(crate)` because `tests/all/batch_put_mixed_row_codes.rs` stages the same
/// account state: one way to make a record outstanding, so two suites cannot
/// stage it two different ways.
pub(crate) fn hold_open(engine: &Arc<engine::InMemoryEngine>, account_id: &str) {
    with_record(engine, account_id, |stored| {
        stored.status = STATUS_PENDING.to_string();
        stored.validation_due_at = Some(Instant::now() + Duration::from_secs(3_600));
    });
}

/// Put `account_id`'s record in `status`, with nothing left owed on it: that
/// status IS the record's state, not one the clock is about to move.
///
/// `pub(crate)` for the reason [`hold_open`] gives.
pub(crate) fn force_status(engine: &Arc<engine::InMemoryEngine>, account_id: &str, status: &str) {
    with_record(engine, account_id, |stored| {
        stored.status = status.to_string();
        stored.validation_due_at = None;
    });
}

/// The `(accountId, code, message)` of every row of a batch answer's `errors`
/// list, in the order the response carries them.
///
/// `pub(crate)` because `tests/all/registration_type_refusals.rs` reads the same
/// list for the per-account refusals it asserts: one reader, so two suites
/// cannot read the batch's error channel two different ways.
pub(crate) fn error_rows(answer: &Answer) -> Vec<(String, String, String)> {
    answer
        .get("errors")
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let member = |name: &str| {
                        row.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (member("accountId"), member("code"), member("message"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `verificationDetails` a stored record can carry: the rows differ in how
/// many documents it holds and in the form each was submitted in (an inline
/// file, an S3 location), none of which the rule depends on.
fn document_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("one inline file", document()),
        (
            "one S3 location",
            json!({
                "taxRegistrationDocuments": [
                    { "s3Location": { "bucket": "trn-docs", "key": "vat-certificate.pdf" } },
                ],
            }),
        ),
        (
            "two documents, one of each form",
            json!({
                "taxRegistrationDocuments": [
                    { "s3Location": { "bucket": "trn-docs", "key": "vat-certificate.pdf" } },
                    { "file": { "fileName": "director-id.png", "fileContent": "aGVsbG8=" } },
                ],
            }),
        ),
    ]
}

/// The `(taxDocumentName, taxDocumentAccessToken)` of every document a read
/// reports for the account's stored registration, in the order the response
/// carries them.
fn document_metadata(answer: &Answer) -> Vec<(String, String)> {
    answer
        .body
        .pointer("/taxRegistration/taxDocumentMetadatas")
        .and_then(Value::as_array)
        .map(|documents| {
            documents
                .iter()
                .map(|reported| {
                    let member = |name: &str| {
                        reported
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (member("taxDocumentName"), member("taxDocumentAccessToken"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What `GetTaxRegistration` reports for `account_id`.
async fn read(
    router: &smithy_server_runtime::MultiServiceRouter,
    account_id: &str,
) -> Answer {
    call(router, "GetTaxRegistration", &json!({ "accountId": account_id })).await
}

/// Assert that `answer` refused a one-account write of `account_id` on the
/// channel `writer` renders a conflict on, and that it stored nothing.
fn assert_refused(case: &str, writer: &str, answer: &Answer, account_id: &str) {
    if writer == BATCH_WRITER {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the batch call itself succeeds, got {answer:?}"
        );
        let rows = error_rows(answer);
        let named: Vec<String> = rows.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            named,
            vec![account_id.to_string()],
            "case {case:?}: the batch must report exactly the conflicting account, got {rows:?}"
        );
        assert_eq!(
            (rows[0].1.as_str(), rows[0].2.as_str()),
            (
                ERROR_CODE_GENERAL_CONFLICT,
                PENDING_REGISTRATION_CONFLICT_MESSAGE,
            ),
            "case {case:?}: the row must carry the envelope this refusal publishes, got \
             {rows:?}"
        );
        assert!(
            answer.body.get("status").is_none(),
            "case {case:?}: a batch that stored nothing has no stored status, got {:?}",
            answer.body
        );
        return;
    }
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: the write must be refused with a conflict, got {answer:?}"
    );
    // The conflict class publishes one code, and this condition has one
    // sentence, so the single-account refusal carries the same envelope the
    // batch row above does.
    assert_eq!(
        (
            answer.get("errorCode").as_str(),
            answer.get("message").as_str(),
        ),
        (
            Some(ERROR_CODE_GENERAL_CONFLICT),
            Some(PENDING_REGISTRATION_CONFLICT_MESSAGE),
        ),
        "case {case:?}: expected the pending-TRN refusal's own envelope, got {:?}",
        answer.body
    );
}

/// RULE: a write aimed at an account whose stored TRN is still `Pending` is
/// refused, and that account's record is left exactly as it was.
///
/// Driven through both writers on each side: whichever writer stored the
/// outstanding TRN, either writer refuses the next write of it.
#[tokio::test]
async fn a_write_is_refused_while_the_accounts_trn_is_outstanding() {
    let account_id = "000000000001";
    for (row, stored_entry, submitted_entry) in entry_pairs() {
        for setup_writer in WRITERS {
            for writer in WRITERS {
                let case = format!("{row}, stored via {setup_writer}, rewritten via {writer}");
                let (router, engine) = emulator_with_engine();
                let setup = write(&router, setup_writer, account_id, &stored_entry).await;
                assert_eq!(
                    setup.status, 200,
                    "case {case:?}: the setup write must succeed, got {setup:?}"
                );
                assert_eq!(
                    setup.get("status").as_str(),
                    Some(STATUS_PENDING),
                    "case {case:?}: the setup write must report an outstanding TRN, got {:?}",
                    setup.body
                );
                hold_open(&engine, account_id);
                let before = read(&router, account_id).await;
                assert_eq!(
                    before.status, 200,
                    "case {case:?}: the stored record must read back, got {before:?}"
                );

                let answer = write(&router, writer, account_id, &submitted_entry).await;
                assert_refused(&case, writer, &answer, account_id);

                let after = read(&router, account_id).await;
                assert_eq!(
                    after.body, before.body,
                    "case {case:?}: the refused write must leave the record untouched, got {:?}",
                    after.body
                );
                assert_eq!(
                    after
                        .body
                        .pointer("/taxRegistration/registrationId")
                        .and_then(Value::as_str),
                    stored_entry.get("registrationId").and_then(Value::as_str),
                    "case {case:?}: the account must still hold the entry it had, got {:?}",
                    after.body
                );
                assert_eq!(
                    after
                        .body
                        .pointer("/taxRegistration/status")
                        .and_then(Value::as_str),
                    Some(STATUS_PENDING),
                    "case {case:?}: the outstanding TRN must still be outstanding, got {:?}",
                    after.body
                );
            }
        }
    }
}

/// RULE: only an OUTSTANDING TRN refuses the write — a record in any other
/// status the model lists is replaced, as a rewrite always was.
///
/// The rows are every value of `TaxRegistrationStatus`
/// (`smithy_query(query="shape", shape="TaxRegistrationStatus")`), including the
/// two this emulator's own lifecycle never reaches, driven through both writers.
#[tokio::test]
async fn only_an_outstanding_trn_refuses_the_write() {
    let account_id = "000000000001";
    let (_, stored_entry, submitted_entry) = entry_pairs()
        .into_iter()
        .next()
        .expect("the table has rows");
    for status in REGISTRATION_STATUSES {
        for writer in WRITERS {
            let case = format!("a record in status {status} rewritten via {writer}");
            let (router, engine) = emulator_with_engine();
            let setup = write(&router, "PutTaxRegistration", account_id, &stored_entry).await;
            assert_eq!(
                setup.status, 200,
                "case {case:?}: the setup write must succeed, got {setup:?}"
            );
            force_status(&engine, account_id, status);

            let answer = write(&router, writer, account_id, &submitted_entry).await;
            if *status == STATUS_PENDING {
                assert_refused(&case, writer, &answer, account_id);
                continue;
            }
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a TRN that is not outstanding may be rewritten, got {answer:?}"
            );
            assert!(
                error_rows(&answer).is_empty(),
                "case {case:?}: no account should have failed, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.get("status").as_str(),
                Some(STATUS_PENDING),
                "case {case:?}: the write reports the status it stored, got {:?}",
                answer.body
            );
            let after = read(&router, account_id).await;
            assert_eq!(
                after
                    .body
                    .pointer("/taxRegistration/registrationId")
                    .and_then(Value::as_str),
                submitted_entry.get("registrationId").and_then(Value::as_str),
                "case {case:?}: the account must hold the rewritten entry, got {:?}",
                after.body
            );
        }
    }
}

/// RULE: the record a refused write leaves untouched is untouched for its
/// DOCUMENTS too — the access tokens the write that stored it minted still name
/// its documents, because the refusal bumped no revision.
///
/// The rows are the document shapes a stored record can carry, driven through
/// both writers. The entry the refused write submits carries no documents at
/// all, so a write that went through would empty the reported metadata.
#[tokio::test]
async fn a_refused_write_keeps_the_stored_records_document_metadata() {
    let account_id = "000000000001";
    let (_, stored_entry, submitted_entry) = entry_pairs()
        .into_iter()
        .next()
        .expect("the table has rows");
    for (documents, verification) in document_rows() {
        let submitted_count = verification
            .get("taxRegistrationDocuments")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        for writer in WRITERS {
            let case = format!("{documents}, rewritten via {writer}");
            let (router, engine) = emulator_with_engine();
            let mut stored = stored_entry.clone();
            stored["verificationDetails"] = verification.clone();
            let setup = write(&router, "PutTaxRegistration", account_id, &stored).await;
            assert_eq!(
                setup.status, 200,
                "case {case:?}: the setup write must succeed, got {setup:?}"
            );
            hold_open(&engine, account_id);

            let before = read(&router, account_id).await;
            let reported = document_metadata(&before);
            assert_eq!(
                reported.len(),
                submitted_count,
                "case {case:?}: expected {submitted_count} document(s) reported, got {reported:?}"
            );
            assert!(
                reported
                    .iter()
                    .all(|(name, token)| !name.is_empty() && !token.is_empty()),
                "case {case:?}: every reported document needs a name and an access token, got \
                 {reported:?}"
            );

            let answer = write(&router, writer, account_id, &submitted_entry).await;
            assert_refused(&case, writer, &answer, account_id);

            let after = read(&router, account_id).await;
            assert_eq!(
                document_metadata(&after),
                reported,
                "case {case:?}: the refused write must leave the stored document tokens alone, \
                 got {:?}",
                document_metadata(&after)
            );
            assert_eq!(
                after.body, before.body,
                "case {case:?}: the refused write must leave the record untouched, got {:?}",
                after.body
            );
        }
    }
}

/// How a test seeds one account of a batch, and what the batch must then do with
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seeded {
    /// Holds a TRN whose validation is still outstanding: the batch must report
    /// it and leave its record alone.
    Outstanding,
    /// Holds a TRN whose validation has run: the batch must rewrite it.
    Settled,
    /// Holds nothing at all: the batch must store the entry for it.
    Nothing,
}

/// RULE: a batch write reports every account whose TRN is still outstanding in
/// its `errors` list — naming the account and why, with that record untouched —
/// while every other account it names is stored, and it reports a stored status
/// only when it stored something.
#[tokio::test]
async fn a_batch_reports_each_outstanding_account_and_stores_the_rest() {
    let (_, stored_entry, submitted_entry) = entry_pairs()
        .into_iter()
        .next()
        .expect("the table has rows");
    // Rows: how many accounts of each kind the one request names — one
    // outstanding account alone, mixes of the three kinds, a batch where every
    // account is outstanding, and one where none is.
    let rows: &[(&str, usize, usize, usize)] = &[
        ("one outstanding account alone", 1, 0, 0),
        ("one outstanding, one unregistered", 1, 0, 1),
        ("two outstanding, three unregistered", 2, 0, 3),
        ("one of each kind", 1, 1, 1),
        ("three outstanding accounts", 3, 0, 0),
        ("two settled, two unregistered", 0, 2, 2),
    ];
    for (case, outstanding, settled, unregistered) in rows.iter().copied() {
        let (router, engine) = emulator_with_engine();

        // Interleave the kinds, so the order the response reports failures in is
        // asserted against the order the REQUEST named them rather than against
        // a grouping of the seeding.
        let mut named: Vec<(String, Seeded)> = Vec::new();
        for index in 0..outstanding.max(settled).max(unregistered) {
            for (kind, count) in [
                (Seeded::Outstanding, outstanding),
                (Seeded::Settled, settled),
                (Seeded::Nothing, unregistered),
            ] {
                if index < count {
                    let offset = match kind {
                        Seeded::Outstanding => 1,
                        Seeded::Settled => 101,
                        Seeded::Nothing => 201,
                    };
                    named.push((format!("{:012}", offset + index), kind));
                }
            }
        }

        for (account_id, kind) in &named {
            if *kind == Seeded::Nothing {
                continue;
            }
            let setup = write(&router, "PutTaxRegistration", account_id, &stored_entry).await;
            assert_eq!(
                setup.status, 200,
                "case {case:?}: seeding {account_id} failed, got {setup:?}"
            );
            match kind {
                Seeded::Outstanding => hold_open(&engine, account_id),
                _ => force_status(&engine, account_id, STATUS_VERIFIED),
            }
        }

        let ids: Vec<&str> = named
            .iter()
            .map(|(account_id, _)| account_id.as_str())
            .collect();
        let answer = call(
            &router,
            BATCH_WRITER,
            &json!({ "accountIds": &ids, "taxRegistrationEntry": submitted_entry }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the batch call itself succeeds, got {answer:?}"
        );

        let error_list = error_rows(&answer);
        let reported: Vec<String> = error_list.iter().map(|(id, _, _)| id.clone()).collect();
        let expected: Vec<String> = named
            .iter()
            .filter(|(_, kind)| *kind == Seeded::Outstanding)
            .map(|(account_id, _)| account_id.clone())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: exactly the outstanding accounts must be reported, in the order the \
             request named them, got {error_list:?}"
        );
        for (account_id, code, message) in &error_list {
            assert_eq!(
                (code.as_str(), message.as_str()),
                (
                    ERROR_CODE_GENERAL_CONFLICT,
                    PENDING_REGISTRATION_CONFLICT_MESSAGE,
                ),
                "case {case:?}: {account_id}'s row must carry the envelope this refusal \
                 publishes, got {error_list:?}"
            );
        }

        let stored_anything = named.iter().any(|(_, kind)| *kind != Seeded::Outstanding);
        assert_eq!(
            answer.get("status").as_str(),
            if stored_anything {
                Some(STATUS_PENDING)
            } else {
                None
            },
            "case {case:?}: the batch reports a stored status only when it stored something, got \
             {:?}",
            answer.body
        );

        for (account_id, kind) in &named {
            let after = read(&router, account_id).await;
            let expected_entry = if *kind == Seeded::Outstanding {
                &stored_entry
            } else {
                &submitted_entry
            };
            assert_eq!(
                after.status, 200,
                "case {case:?}: {account_id} must hold a record, got {after:?}"
            );
            assert_eq!(
                after
                    .body
                    .pointer("/taxRegistration/registrationId")
                    .and_then(Value::as_str),
                expected_entry.get("registrationId").and_then(Value::as_str),
                "case {case:?}: {account_id} ({kind:?}) must hold the right entry, got {:?}",
                after.body
            );
        }
    }
}

/// RULE: the two things that end a TRN's outstanding state let the write
/// through — deleting the pending registration, which is the remedy the model
/// documents, and the validation the clock owed running.
///
/// Driven through both writers, so neither has a release path the other lacks.
#[tokio::test]
async fn releasing_the_outstanding_trn_lets_the_write_through() {
    let account_id = "000000000001";
    let (_, stored_entry, submitted_entry) = entry_pairs()
        .into_iter()
        .next()
        .expect("the table has rows");
    for release in ["delete the pending TRN", "let its validation fall due"] {
        for writer in WRITERS {
            let case = format!("{release}, then write via {writer}");
            let (router, engine) = emulator_with_engine();
            let setup = write(&router, "PutTaxRegistration", account_id, &stored_entry).await;
            assert_eq!(
                setup.status, 200,
                "case {case:?}: the setup write must succeed, got {setup:?}"
            );
            hold_open(&engine, account_id);
            let refused = write(&router, writer, account_id, &submitted_entry).await;
            assert_refused(&case, writer, &refused, account_id);

            if release == "delete the pending TRN" {
                let deleted = call(
                    &router,
                    "DeleteTaxRegistration",
                    &json!({ "accountId": account_id }),
                )
                .await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: the pending TRN must be deletable, got {deleted:?}"
                );
            } else {
                make_validations_due(&engine);
            }

            let accepted = write(&router, writer, account_id, &submitted_entry).await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: the released account must be writable, got {accepted:?}"
            );
            assert!(
                error_rows(&accepted).is_empty(),
                "case {case:?}: no account should have failed, got {:?}",
                accepted.body
            );
            assert_eq!(
                accepted.get("status").as_str(),
                Some(STATUS_PENDING),
                "case {case:?}: the write reports the status it stored, got {:?}",
                accepted.body
            );
            let after = read(&router, account_id).await;
            assert_eq!(
                after
                    .body
                    .pointer("/taxRegistration/registrationId")
                    .and_then(Value::as_str),
                submitted_entry.get("registrationId").and_then(Value::as_str),
                "case {case:?}: the account must hold the entry the write submitted, got {:?}",
                after.body
            );
        }
    }
}
