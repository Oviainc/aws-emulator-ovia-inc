// SPDX-License-Identifier: CC0-1.0

//! The one supplemental-TRN write decision and the one removal decision, driven
//! over the restJson1 wire through BOTH operations that take them.
//!
//! RULE: whether a write of a supplemental tax registration ADDS a record or
//! REPLACES one is decided by the TRN the entry names — its `registrationType`
//! and `registrationId` — in the one place a supplemental record becomes a stored
//! one: a write naming a TRN the store already holds rewrites that record under
//! the `authorityId` it was first stored under and adds nothing, so it is always
//! admitted, while a write that would add a record is refused with the modelled
//! `ConflictException` once the store holds as many records as the emulator keeps
//! resident; and a stored record stops being one in the one place that removes
//! it, which refuses an `authorityId` the store does not hold and removes
//! nothing else. So `PutSupplementalTaxRegistration` and
//! `DeleteSupplementalTaxRegistration` cannot judge occupancy, identity or
//! removal their own way, and the two cannot leave the store in states that
//! disagree: each delete frees exactly the slot the next add needs, and no
//! `authorityId` is ever handed out twice.
//!
//! The rows span what the decision generalises over: how many records are
//! resident when the write arrives (an empty store, one far below the resident
//! bound, one record below it, and one AT it), whether the write names a TRN the
//! store holds or one it does not, and the `registrationId` the new TRN is named
//! by (one character — the low bound of its 1..=200 length — a punctuated number,
//! and the 200-character bound itself). Every row then drives the removal
//! decision: a live `authorityId`, that same id once it names nothing, and the
//! write again now that a slot is free.
//!
//! What a refused jurisdiction, a blank member or a listing page answers are
//! separate rules with their own suites (`supplemental_tax_registrations.rs`,
//! `listing_page_bounds.rs`); what this suite pins is that one write decision and
//! one removal decision answer both operations.
//!
//! The harness (`emulator_with_engine`, `call`, `field_list`) lives in
//! `tax_registration_lifecycle.rs` and the supplemental entry builder, the write
//! and the listing reader in `supplemental_tax_registrations.rs`; nothing here
//! builds a second copy of either.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use service_com_amazonaws_taxsettings_taxsettings::ops::types::{
    Address, SupplementalTaxRegistrationEntry,
};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    invalid_authority_id, parameter_name, state_conflict,
    ComAmazonawsTaxsettingsTaxsettingsState, ERROR_CODE_FIELD_VALIDATION_FAILED,
    MAX_STORED_SUPPLEMENTAL_REGISTRATIONS, SUPPLEMENTAL_TRN_COUNTRIES, TRN_VALIDATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::supplemental_tax_registrations::{entry_for, list_all, put};
use crate::tax_registration_lifecycle::{call, emulator_with_engine, field_list, Answer};

/// The jurisdiction every row registers in, read from the catalog the
/// jurisdiction rule is keyed on (`crate::state::SUPPLEMENTAL_TRN_COUNTRIES`) so
/// this suite asserts nothing about which jurisdictions those are. Which one a
/// record is held in is not a dimension of the write decision.
pub(crate) fn supported_country() -> &'static str {
    SUPPLEMENTAL_TRN_COUNTRIES
        .first()
        .expect("the supported set names at least one jurisdiction")
        .0
}

/// The `registrationId` of the `index`-th seeded record (1-based).
fn seeded_registration_id(index: usize) -> String {
    format!("CHE-SEEDED-{index:05}")
}

/// Make the store hold `count` supplemental records, answering their
/// `authorityId`s in store order.
///
/// Seeded through the one state method a write takes rather than through `count`
/// wire calls, because what each row is about is the decision the write at the
/// END of the seeding takes; the resident bound is a thousand records, so seeding
/// it over the wire would say nothing extra and cost a thousand requests per row.
fn seed_to(engine: &Arc<engine::InMemoryEngine>, count: usize) -> Vec<String> {
    // A supplemental registration supplements the account's own, so the write
    // decision each row is about is only reached once the account holds a
    // settled registration of its own.
    crate::supplemental_tax_registrations::hold_a_settled_own_registration(engine);
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    let mut ids = Vec::with_capacity(count);
    for index in 1..=count {
        let entry = SupplementalTaxRegistrationEntry {
            registration_id: seeded_registration_id(index),
            registration_type: "VAT".to_string(),
            legal_name: format!("Seeded {index} AG"),
            address: Address {
                postal_code: "8001".to_string(),
                country_code: supported_country().to_string(),
                ..Address::default()
            },
        };
        match store.store_supplemental_registration(&entry, Instant::now()) {
            Ok((authority_id, _status)) => ids.push(authority_id),
            Err(refusal) => panic!(
                "seeding record {index} of {count} must be admitted, got {:?}",
                refusal.message
            ),
        }
    }
    // The seeded records carry the validation their write started; a write aimed
    // at a record whose validation is still outstanding conflicts with the
    // current state of that record, so what the clock owed is settled here and
    // each row's write meets the decision the row is about.
    store.settle_due(Instant::now() + TRN_VALIDATION_DELAY);
    ids
}

/// The TRN a row's write names.
#[derive(Debug)]
enum Trn {
    /// Rewrite the record the `index`-th seeded entry (1-based) is held under:
    /// the write addresses a TRN the store already holds.
    Held(usize),
    /// Register this `registrationId`, which no resident record carries.
    New(String),
}

/// One store state a write arrives at.
#[derive(Debug)]
struct Row {
    case: String,
    /// How many records are resident when the write arrives.
    resident: usize,
    names: Trn,
}

impl Row {
    /// Whether the write would ADD a record rather than rewrite one.
    fn adds(&self) -> bool {
        matches!(self.names, Trn::New(_))
    }

    /// What the write is answered, derived from the rule rather than from either
    /// handler: only an ADD is refused, and only once the store is at the
    /// resident bound.
    fn refused(&self) -> bool {
        self.adds() && self.resident >= MAX_STORED_SUPPLEMENTAL_REGISTRATIONS
    }

    /// The `registrationId` the write names.
    fn registration_id(&self) -> String {
        match &self.names {
            Trn::Held(index) => seeded_registration_id(*index),
            Trn::New(registration_id) => registration_id.clone(),
        }
    }

    /// The entry the write submits: this row's TRN, under a `legalName` no
    /// seeded record carries, so a rewrite is observable in the record it left
    /// behind.
    fn entry(&self) -> Value {
        entry_for(
            supported_country(),
            &self.registration_id(),
            "Written By This Row AG",
        )
    }
}

/// The store states the write decision distinguishes.
///
/// The `registrationId`s the new TRNs are named by span the member's published
/// length (`GenericString`, 1..=200 — the `SupplementalTaxRegistrationEntry`
/// contract in `src/ops/put_supplemental_tax_registration.rs`): one character,
/// a punctuated number, and the 200-character bound.
fn rows() -> Vec<Row> {
    let bound = MAX_STORED_SUPPLEMENTAL_REGISTRATIONS;
    vec![
        Row {
            case: "an empty store, a TRN it does not hold".to_string(),
            resident: 0,
            names: Trn::New("CHE-000.000.001".to_string()),
        },
        Row {
            case: "a store far below the bound, a TRN it does not hold".to_string(),
            resident: 3,
            names: Trn::New("X".to_string()),
        },
        Row {
            case: "a store far below the bound, a TRN it already holds".to_string(),
            resident: 3,
            names: Trn::Held(2),
        },
        Row {
            case: "a store one record below the bound, a TRN it does not hold".to_string(),
            resident: bound - 1,
            names: Trn::New("C".repeat(200)),
        },
        Row {
            case: "a store at the bound, a TRN it already holds".to_string(),
            resident: bound,
            names: Trn::Held(bound / 2),
        },
        Row {
            case: "a store at the bound, a TRN it does not hold".to_string(),
            resident: bound,
            names: Trn::New("CHE-999.999.999".to_string()),
        },
    ]
}

/// A string member of one reported record, empty when the record omits it.
fn member(record: &Value, name: &str) -> String {
    record
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every supplemental TRN the listing reports, as its `authorityId` paired with
/// the `registrationId` and `legalName` the record carries — the store as a
/// caller can see it.
async fn reported(router: &MultiServiceRouter) -> Vec<(String, String, String)> {
    list_all(router, None)
        .await
        .iter()
        .map(|record| {
            (
                member(record, "authorityId"),
                member(record, "registrationId"),
                member(record, "legalName"),
            )
        })
        .collect()
}

/// The `authorityId` a write reported, empty when it reported none.
pub(crate) fn reported_authority(answer: &Answer) -> String {
    answer
        .get("authorityId")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Delete `authority_id` through `DeleteSupplementalTaxRegistration`.
async fn delete(router: &MultiServiceRouter, authority_id: &str) -> Answer {
    call(
        router,
        "DeleteSupplementalTaxRegistration",
        &serde_json::json!({ "authorityId": authority_id }),
    )
    .await
}

/// RULE: the write decision and the removal decision are each taken in one
/// place, so a write naming a held TRN replaces that record whatever the store's
/// occupancy, a write that would add one is refused exactly at the resident
/// bound, a delete frees exactly the slot the next add needs, and an
/// `authorityId` that names nothing is refused while removing nothing.
#[tokio::test]
async fn the_write_and_the_delete_take_one_decision_each() {
    for row in rows() {
        let (router, engine) = emulator_with_engine();
        let seeded = seed_to(&engine, row.resident);
        let mut ever_minted: BTreeSet<String> = seeded.iter().cloned().collect();
        assert_eq!(
            seeded.len(),
            row.resident,
            "case {:?}: the store must hold the row's records before the write",
            row.case
        );

        // The write, through the first operation that takes the decision.
        let written = put(&router, &row.entry()).await;
        let after_write = if row.refused() {
            assert_eq!(
                (written.status, written.error_code.as_str()),
                (409, "ConflictException"),
                "case {:?}: an add past the resident bound must be refused, got {written:?}",
                row.case
            );
            assert_eq!(
                written.get("message").as_str(),
                Some(state_conflict().message.as_str()),
                "case {:?}: expected the conflict class's own wording, got {:?}",
                row.case,
                written.get("message")
            );
            let listed = reported(&router).await;
            assert_eq!(
                listed.len(),
                row.resident,
                "case {:?}: a refused write must store nothing, got {} records",
                row.case,
                listed.len()
            );
            assert!(
                !listed
                    .iter()
                    .any(|(_, registration_id, _)| *registration_id == row.registration_id()),
                "case {:?}: the refused TRN must not be stored, got {listed:?}",
                row.case
            );
            None
        } else {
            assert_eq!(
                written.status, 200,
                "case {:?}: this write adds nothing or there is room for it, so it must be \
                 admitted, got {written:?}",
                row.case
            );
            let authority_id = reported_authority(&written);
            match &row.names {
                Trn::Held(index) => assert_eq!(
                    authority_id,
                    seeded[index - 1],
                    "case {:?}: a write of a held TRN must answer the authorityId it was first \
                     stored under, got {authority_id:?}",
                    row.case
                ),
                Trn::New(_) => assert!(
                    !authority_id.is_empty() && !ever_minted.contains(&authority_id),
                    "case {:?}: a TRN the store does not hold must get an authorityId of its own, \
                     got {authority_id:?}",
                    row.case
                ),
            }
            ever_minted.insert(authority_id.clone());

            let listed = reported(&router).await;
            let expected_len = row.resident + usize::from(row.adds());
            assert_eq!(
                listed.len(),
                expected_len,
                "case {:?}: a replacement adds no record and an add adds exactly one, so the \
                 store must hold {expected_len} records, got {}",
                row.case,
                listed.len()
            );
            assert!(
                listed.contains(&(
                    authority_id.clone(),
                    row.registration_id(),
                    "Written By This Row AG".to_string()
                )),
                "case {:?}: the write's own record must read back under {authority_id:?}, got \
                 {} records",
                row.case,
                listed.len()
            );
            Some(authority_id)
        };

        // The removal, through the other operation that takes the decision: a
        // live record goes, whichever path stored it.
        let live = after_write.clone().unwrap_or_else(|| seeded[0].clone());
        let before_delete = reported(&router).await.len();
        let removed = delete(&router, &live).await;
        assert_eq!(
            removed.status, 200,
            "case {:?}: a live authorityId must delete, got {removed:?}",
            row.case
        );
        let listed = reported(&router).await;
        assert_eq!(
            listed.len(),
            before_delete - 1,
            "case {:?}: a delete removes exactly the record it names, got {} records after {}",
            row.case,
            listed.len(),
            before_delete
        );
        assert!(
            !listed
                .iter()
                .any(|(authority_id, _, _)| *authority_id == live),
            "case {:?}: the deleted record must be gone, got {listed:?}",
            row.case
        );

        // The same id now names nothing, so the removal decision refuses it and
        // removes nothing further.
        let again = delete(&router, &live).await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (400, "ValidationException"),
            "case {:?}: an authorityId that names nothing must be refused, got {again:?}",
            row.case
        );
        assert_eq!(
            again.get("message").as_str(),
            Some(invalid_authority_id().message.as_str()),
            "case {:?}: expected the unknown-authority refusal's own wording, got {:?}",
            row.case,
            again.get("message")
        );
        assert_eq!(
            again.get("errorCode").as_str(),
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            "case {:?}: expected the field-validation code, got {:?}",
            row.case,
            again.get("errorCode")
        );
        assert_eq!(
            field_list(&again),
            vec![parameter_name("authorityId")],
            "case {:?}: the refusal must name the parameter it is about, got {:?}",
            row.case,
            field_list(&again)
        );
        assert_eq!(
            reported(&router).await.len(),
            listed.len(),
            "case {:?}: a refused delete must remove nothing",
            row.case
        );

        // A freed slot is a slot the next add may take — the occupancy the one
        // write decision judges, not the request that asked.
        let rewritten = put(&router, &row.entry()).await;
        assert_eq!(
            rewritten.status, 200,
            "case {:?}: with a slot free the same write must be admitted, got {rewritten:?}",
            row.case
        );
        let authority_id = reported_authority(&rewritten);
        assert!(
            !authority_id.is_empty() && !ever_minted.contains(&authority_id),
            "case {:?}: the record the freed slot took is a new one, so its authorityId must be \
             one no record has carried, got {authority_id:?}",
            row.case
        );
        let listed = reported(&router).await;
        let expected_len = row.resident + usize::from(row.adds() && !row.refused());
        assert_eq!(
            listed.len(),
            expected_len,
            "case {:?}: the store must hold {expected_len} records once the freed slot is taken, \
             got {}",
            row.case,
            listed.len()
        );
    }
}
