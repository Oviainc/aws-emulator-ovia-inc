// SPDX-License-Identifier: CC0-1.0

//! What `PutTaxExemption` persists, read back through another mount of the same
//! service.
//!
//! The rule under test: a submission answers from — and writes to — the
//! SERVICE's state, not a store private to the request that made it, so every
//! exemption the published catalog offers, once submitted, is reported by any
//! later reader of that state, including a service instance built after the
//! write; and each applied submission opens exactly one support case, while a
//! read opens none.
//!
//! `tests/all/tax_exemptions.rs` spans the submission's own members (how many
//! accounts it names, what they already hold, which refusals it takes). The table
//! here spans the other axis the rule generalises over: EVERY entry
//! `GetTaxExemptionTypes` publishes, taken from the catalog itself rather than
//! written out here, crossed with how the request names that entry (its
//! `description`, its `displayName`, or that name re-cased and padded) and
//! whether the jurisdiction carries a state — each read back through a second
//! `EmulatedService` mounted on the same engine.
//!
//! The harness (`emulator_with_engine`, `mount_on`, `call`) lives in
//! `tax_registration_lifecycle.rs`, the catalog's wire form in
//! `tax_exemption_types.rs` and the submitted certificate in
//! `tax_exemptions.rs`; nothing here builds a second copy of any of them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::ops::types::TaxExemptionType;
use service_com_amazonaws_taxsettings_taxsettings::state::{
    tax_exemption_type_catalog, ComAmazonawsTaxsettingsTaxsettingsState, EXEMPTION_STATUS_PENDING,
    MAX_PAGE_SIZE,
};

use crate::tax_exemption_types::wire_form;
use crate::tax_exemptions::certificate;
use crate::tax_registration_lifecycle::{call, emulator_with_engine, mount_on, Answer};

/// The `taxExemptions` a read reported for one account.
fn held_exemptions(answer: &Answer, account_id: &str) -> Vec<Value> {
    answer
        .get("taxExemptionDetailsMap")
        .get(account_id)
        .and_then(|details| details.get("taxExemptions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The account ids a read reported, sorted.
fn reported_accounts(answer: &Answer) -> Vec<String> {
    let mut accounts: Vec<String> = answer
        .get("taxExemptionDetailsMap")
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    accounts.sort();
    accounts
}

/// How a request names the catalog entry it asks for: both members that identify
/// a published entry, and one of them re-cased and padded.
#[derive(Clone, Copy, Debug)]
enum NamedBy {
    Description,
    DisplayName,
    PaddedDisplayName,
}

/// How many support cases this service has opened, and how many exemption
/// records it holds — read from the state the handlers share.
fn cases_and_records(engine: &Arc<engine::InMemoryEngine>) -> (u64, usize) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let store = state.lock();
    (store.minted_exemption_cases, store.stored_exemption_count())
}

/// One submission of the table: the account it names, the published entry it
/// asks for, and what a later read must report for that account.
struct Expectation {
    case: String,
    account: String,
    entry: TaxExemptionType,
    authority: Value,
}

/// RULE: a submission records the exemption it identifies in the service's
/// state, so any later reader of that state reports it — whichever published
/// entry it names and however it names one — and one applied submission opens
/// exactly one support case.
#[tokio::test]
async fn a_submission_is_persisted_for_every_published_type_and_read_by_another_mount() {
    let (writer, engine) = emulator_with_engine();
    // The whole published collection, so the table covers entries no issue
    // cited and grows with the catalog instead of pinning a hand-written list.
    let catalog = tax_exemption_type_catalog();
    assert!(
        !catalog.is_empty(),
        "case \"the published catalog\": expected the service to publish at least one exemption \
         type, got none"
    );

    let mut expectations: Vec<Expectation> = Vec::new();
    let mut case_ids: Vec<String> = Vec::new();
    for (index, entry) in catalog.iter().enumerate() {
        let display_name = entry
            .display_name
            .clone()
            .unwrap_or_else(|| panic!("catalog entry {index}: expected a displayName, got none"));
        let description = entry
            .description
            .clone()
            .unwrap_or_else(|| panic!("catalog entry {index}: expected a description, got none"));
        let jurisdiction = entry
            .applicable_jurisdictions
            .as_ref()
            .and_then(|jurisdictions| jurisdictions.first())
            .cloned()
            .unwrap_or_else(|| {
                panic!("catalog entry {index}: expected an applicable jurisdiction, got none")
            });

        let named_by = match index % 3 {
            0 => NamedBy::Description,
            1 => NamedBy::DisplayName,
            _ => NamedBy::PaddedDisplayName,
        };
        let exemption_type = match named_by {
            NamedBy::Description => description.clone(),
            NamedBy::DisplayName => display_name.clone(),
            NamedBy::PaddedDisplayName => format!("  {}  ", display_name.to_uppercase()),
        };
        // A jurisdiction that names a state applies in that state alone, so the
        // request names it; one that names a country only applies throughout
        // that country, and the state a certificate is held in is a fact about
        // the certificate — so those rows differ along whether one is submitted,
        // and in which case and padding.
        let submitted_state: Option<String> = match &jurisdiction.state {
            Some(state) => Some(state.clone()),
            None => match index % 3 {
                0 => None,
                1 => Some("Ohio".to_string()),
                _ => Some("  puerto rico  ".to_string()),
            },
        };
        let submitted_country = if index % 2 == 0 {
            jurisdiction.country.clone()
        } else {
            jurisdiction.country.to_lowercase()
        };

        let mut requested_authority = json!({ "country": submitted_country });
        if let Some(state) = &submitted_state {
            requested_authority["state"] = json!(state);
        }
        let mut expected_authority = json!({ "country": jurisdiction.country.to_uppercase() });
        if let Some(state) = &submitted_state {
            expected_authority["state"] = json!(state.trim());
        }

        let account = format!("6{index:011}");
        let case = format!("{display_name} named by {named_by:?} in {requested_authority}");
        let answer = call(
            &writer,
            "PutTaxExemption",
            &json!({
                "accountIds": [&account],
                "authority": requested_authority,
                "exemptionType": exemption_type,
                "exemptionCertificate": certificate("certificate.pdf"),
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the submission to be applied, got {answer:?}"
        );
        let case_id = answer
            .get("caseId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a caseId, got {answer:?}"))
            .to_string();
        assert!(
            (1..=200).contains(&case_id.chars().count()),
            "case {case:?}: caseId is a 1..200 string, got {case_id:?}"
        );
        assert!(
            !case_ids.contains(&case_id),
            "case {case:?}: every applied submission opens its own case, got {case_id:?} twice"
        );
        case_ids.push(case_id);

        expectations.push(Expectation {
            case,
            account,
            entry: entry.clone(),
            authority: expected_authority,
        });
    }

    // One case per applied submission, and one record per account written for.
    let (cases, records) = cases_and_records(&engine);
    let submissions = catalog.len();
    assert_eq!(
        (usize::try_from(cases).unwrap_or(usize::MAX), records),
        (submissions, submissions),
        "case \"what {submissions} submissions left in the state\": expected one case and one \
         record each, got {cases} case(s) and {records} record(s)"
    );

    // The reader: a SECOND mount of this service, constructed AFTER every write,
    // on the same engine. It holds no record of its own, so anything it reports
    // came from the service's state rather than from anything the writing
    // request owned.
    let reader = mount_on(&engine);
    // `BatchGetTaxExemptionsRequest$accountIds` is @length 1..=5
    // (`smithy_query(query="operation", operation="BatchGetTaxExemptions")`), so
    // the accounts are read a batch at a time.
    for batch in expectations.chunks(5) {
        let accounts: Vec<&str> = batch
            .iter()
            .map(|expectation| expectation.account.as_str())
            .collect();
        let read = call(
            &reader,
            "BatchGetTaxExemptions",
            &json!({ "accountIds": accounts }),
        )
        .await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (200, ""),
            "case \"reading {accounts:?} through another mount\": expected 200, got {read:?}"
        );
        for expectation in batch {
            let case = &expectation.case;
            let held = held_exemptions(&read, &expectation.account);
            assert_eq!(
                held.len(),
                1,
                "case {case:?}: expected the one submitted exemption, got {held:?}"
            );
            let record = &held[0];
            assert_eq!(
                record.get("taxExemptionType"),
                Some(&wire_form(&expectation.entry)),
                "case {case:?}: expected the published entry {}, got {record}",
                wire_form(&expectation.entry)
            );
            assert_eq!(
                record.get("authority"),
                Some(&expectation.authority),
                "case {case:?}: expected the submitted jurisdiction {}, got {record}",
                expectation.authority
            );
            assert_eq!(
                record.get("status").and_then(Value::as_str),
                Some(EXEMPTION_STATUS_PENDING),
                "case {case:?}: a submitted certificate is under review, got {record}"
            );
            assert!(
                record.get("systemEffectiveDate").is_some(),
                "case {case:?}: expected the instant the service recorded it, got {record}"
            );
        }
    }

    // The listing through the same second mount sees every account the
    // submissions named, in account-id order.
    let listed = call(
        &reader,
        "ListTaxExemptions",
        &json!({ "maxResults": MAX_PAGE_SIZE }),
    )
    .await;
    assert_eq!(
        (listed.status, listed.error_code.as_str()),
        (200, ""),
        "case \"listing through another mount\": expected 200, got {listed:?}"
    );
    // Every account this service reports exemptions for is in the listing, which
    // is every account a submission named plus the caller's own — the account the
    // emulator presents, which is in the family whether or not it holds an
    // exemption.
    let mut expected_accounts: Vec<String> = expectations
        .iter()
        .map(|expectation| expectation.account.clone())
        .chain(std::iter::once(engine::emulator_account_id().to_string()))
        .collect();
    expected_accounts.sort();
    expected_accounts.dedup();
    assert_eq!(
        reported_accounts(&listed),
        expected_accounts,
        "case \"listing through another mount\": expected every account a submission named, got \
         {listed:?}"
    );

    // Reading through either mount changed nothing: no case was opened and no
    // record added or dropped.
    let (cases_after, records_after) = cases_and_records(&engine);
    assert_eq!(
        (cases_after, records_after),
        (cases, records),
        "case \"the state after the reads\": expected {cases} case(s) and {records} record(s), got \
         {cases_after} and {records_after}"
    );
}
