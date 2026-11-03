// SPDX-License-Identifier: CC0-1.0

//! `PutSupplementalTaxRegistration` against the state of the account it writes
//! for, end to end over the restJson1 wire.
//!
//! ONE rule: a write that conflicts with the current state of the registration
//! it addresses is refused with the modelled `ConflictException` (HTTP 409)
//! carrying the envelope the conflict class publishes — the service's code for a
//! state conflict and that class's fixed message — leaving the store exactly as
//! it was, and is applied once that state has moved on. Two states conflict,
//! both of them the rule the service documents for an account's own TRN ("You
//! can't set a TRN if there's a pending TRN. You'll need to delete the pending
//! TRN first"), read on the registration each write addresses:
//!
//! * a supplemental registration SUPPLEMENTS the account's own, so an account
//!   that holds no tax registration of its own — or whose own TRN is still
//!   outstanding — has nothing for a supplemental one to supplement;
//! * the supplemental record the write would replace has a validation still
//!   outstanding, a supplemental TRN being a registration of the same account,
//!   validated by the same clock-owed rule.
//!
//! The tables span the account states the first rule distinguishes (no
//! registration at all, an outstanding one, a settled one), what the second
//! write changes — nothing, the legal name, the address, the number's spelling —
//! and the ways out of the conflict, since both releases the record's own suite
//! names (letting the validation fall due, and deleting the record) must let the
//! write through.
//!
//! The harness (`emulator_with_engine`, `call`, `make_validations_due`) lives in
//! `tax_registration_lifecycle.rs` and the supplemental helpers (`put`,
//! `list_all`, `make_supplemental_validations_due`,
//! `hold_a_settled_own_registration`) in `supplemental_tax_registrations.rs`;
//! nothing here builds a second copy of either.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ERROR_CODE_GENERAL_CONFLICT, STATE_CONFLICT_MESSAGE, STATUS_PENDING, STATUS_VERIFIED,
};

use crate::supplemental_tax_registrations::{
    emulator_supplementing, entry_for, list_all,
    make_supplemental_validations_due, put as put_supplemental,
};
use crate::tax_registration_lifecycle::{call, emulator_with_engine, make_validations_due};

/// The entry the first write stores: a Swiss VAT registration, the jurisdiction
/// this service holds supplemental registrations for.
fn first_entry() -> Value {
    json!({
        "registrationId": "CHE-123.456.789",
        "registrationType": "VAT",
        "legalName": "Acme AG",
        "address": { "postalCode": "8001", "countryCode": "CH" },
    })
}

/// The second writes the table makes, each addressing the record the first one
/// stored: `(what differs, the entry)`.
fn second_entries() -> Vec<(&'static str, Value)> {
    vec![
        ("the same entry again", first_entry()),
        (
            "a different legal name",
            json!({
                "registrationId": "CHE-123.456.789",
                "registrationType": "VAT",
                "legalName": "Acme Holding AG",
                "address": { "postalCode": "8001", "countryCode": "CH" },
            }),
        ),
        (
            "a different address",
            json!({
                "registrationId": "CHE-123.456.789",
                "registrationType": "VAT",
                "legalName": "Acme AG",
                "address": {
                    "addressLine1": "Bahnhofstrasse 2",
                    "city": "Zurich",
                    "postalCode": "8002",
                    "countryCode": "CH",
                },
            }),
        ),
        (
            // The same NUMBER in another separator form names the same record,
            // so it addresses the outstanding registration too.
            "the number re-spelled",
            json!({
                "registrationId": "CHE123456789",
                "registrationType": "VAT",
                "legalName": "Acme AG",
                "address": { "postalCode": "8001", "countryCode": "CH" },
            }),
        ),
        (
            "the number re-spelled and padded",
            json!({
                "registrationId": "  che/123/456/789 ",
                "registrationType": "VAT",
                "legalName": "Acme AG",
                "address": { "postalCode": "8001", "countryCode": "CH" },
            }),
        ),
    ]
}

/// RULE: a supplemental registration supplements the account's own, so a write
/// for an account that holds none — or whose own TRN is still outstanding — is
/// refused with the conflict class's envelope and stores nothing, while the same
/// write is applied once the account's own registration has settled.
#[tokio::test]
async fn a_supplemental_write_needs_a_settled_registration_of_the_accounts_own() {
    // The entries differ along the number and the legal name, neither of which
    // the account-state rule turns on.
    let entries: &[(&str, &str, &str)] = &[
        ("a Swiss supplemental VAT registration", "CHE-123.456.789", "Acme AG"),
        ("another Swiss supplemental VAT registration", "CHE-987.654.321", "Beispiel AG"),
        ("a supplemental registration at the 1-character bound", "1", "A"),
    ];
    let own_registration = json!({
        "accountId": engine::emulator_account_id(),
        "taxRegistrationEntry": {
            "registrationId": "DE136695976",
            "registrationType": "VAT",
            "legalName": "Acme GmbH",
            "legalAddress": { "postalCode": "10117", "countryCode": "DE" },
        },
    });

    for (case, registration_id, legal_name) in entries.iter().copied() {
        let entry = entry_for("CH", registration_id, legal_name);

        // The account holds no registration of its own: nothing to supplement.
        let (router, engine) = emulator_with_engine();
        let refused = put_supplemental(&router, &entry).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: an account holding no registration of its own must be refused, got \
             {refused:?}"
        );
        assert_eq!(
            (
                refused.get("errorCode").as_str(),
                refused.get("message").as_str(),
            ),
            (
                Some(ERROR_CODE_GENERAL_CONFLICT),
                Some(STATE_CONFLICT_MESSAGE),
            ),
            "case {case:?}: expected the conflict class's own envelope, got {:?}",
            refused.body
        );
        assert!(
            list_all(&router, None).await.is_empty(),
            "case {case:?}: a refused write stores nothing, got {:?}",
            list_all(&router, None).await
        );

        // Its own TRN is written but its validation is still outstanding: still
        // nothing a supplemental registration may supplement.
        let stored = call(&router, "PutTaxRegistration", &own_registration).await;
        assert_eq!(
            (stored.status, stored.get("status").as_str()),
            (200, Some(STATUS_PENDING)),
            "case {case:?}: the account's own write must store an outstanding TRN, got {stored:?}"
        );
        let refused = put_supplemental(&router, &entry).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: an outstanding own TRN must refuse the write, got {refused:?}"
        );
        assert!(
            list_all(&router, None).await.is_empty(),
            "case {case:?}: a refused write stores nothing, got {:?}",
            list_all(&router, None).await
        );

        // Once the clock has settled that validation the same write is applied.
        make_validations_due(&engine);
        let applied = put_supplemental(&router, &entry).await;
        assert_eq!(
            (applied.status, applied.get("status").as_str()),
            (200, Some(STATUS_PENDING)),
            "case {case:?}: a settled own TRN must let the write through, got {applied:?}"
        );
        let held = list_all(&router, None).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: the write must store the one record, got {held:?}"
        );
        assert_eq!(
            held[0].get("registrationId").and_then(Value::as_str),
            Some(registration_id),
            "case {case:?}: the record must carry the submitted number, got {:?}",
            held[0]
        );
    }
}

/// RULE: the write is refused while the record it addresses has a validation
/// outstanding, the record is left as it was, and the same write is applied once
/// the clock has settled that validation.
#[tokio::test]
async fn a_write_is_refused_while_the_records_validation_is_outstanding() {
    for (differs, second) in second_entries() {
        let case = format!("a second write with {differs}");
        let (router, engine) = emulator_supplementing();

        let first = put_supplemental(&router, &first_entry()).await;
        assert_eq!(
            (first.status, first.get("status").as_str()),
            (200, Some(STATUS_PENDING)),
            "case {case:?}: the first write must store an outstanding registration, got {first:?}"
        );
        let authority_id = first
            .get("authorityId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an authorityId, got {first:?}"))
            .to_string();

        let refused = put_supplemental(&router, &second).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: the write must be refused while the record is outstanding, got \
             {refused:?}"
        );
        assert_eq!(
            (
                refused.get("errorCode").as_str(),
                refused.get("message").as_str(),
            ),
            (
                Some(ERROR_CODE_GENERAL_CONFLICT),
                Some(STATE_CONFLICT_MESSAGE),
            ),
            "case {case:?}: expected the conflict class's own envelope, got {:?}",
            refused.body
        );

        // The record is exactly the one the first write stored: one record, under
        // the id it was stored with, carrying that write's own members.
        let held = list_all(&router, None).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: a refused write stores nothing, got {held:?}"
        );
        assert_eq!(
            (
                held[0].get("authorityId").and_then(Value::as_str),
                held[0].get("registrationId").and_then(Value::as_str),
                held[0].get("legalName").and_then(Value::as_str),
            ),
            (
                Some(authority_id.as_str()),
                Some("CHE-123.456.789"),
                Some("Acme AG"),
            ),
            "case {case:?}: the record must be the one the first write stored, got {:?}",
            held[0]
        );

        // Once the clock has settled the validation the record is no longer
        // outstanding, so the very same write is applied — under the id the TRN
        // was first stored with.
        make_supplemental_validations_due(&engine);
        let applied = put_supplemental(&router, &second).await;
        assert_eq!(
            (applied.status, applied.get("authorityId").as_str()),
            (200, Some(authority_id.as_str())),
            "case {case:?}: a settled record may be rewritten, got {applied:?}"
        );
        let rewritten = list_all(&router, None).await;
        assert_eq!(
            rewritten.len(),
            1,
            "case {case:?}: the write must replace the one record, got {rewritten:?}"
        );
        for (member, value) in second.as_object().expect("the entry is an object") {
            if member == "address" {
                continue;
            }
            assert_eq!(
                rewritten[0].get(member),
                Some(value),
                "case {case:?}: the record must carry the last write's {member:?}, got {:?}",
                rewritten[0]
            );
        }
    }
}

/// RULE's other release: deleting the outstanding record leaves nothing for a
/// write to conflict with, so the next write of that TRN is applied and mints its
/// own `authorityId`.
#[tokio::test]
async fn deleting_the_outstanding_record_lets_the_write_through() {
    let (router, _engine) = emulator_supplementing();
    let first = put_supplemental(&router, &first_entry()).await;
    assert_eq!(
        first.status, 200,
        "the first write must store a record, got {first:?}"
    );
    let authority_id = first
        .get("authorityId")
        .as_str()
        .unwrap_or_else(|| panic!("expected an authorityId, got {first:?}"))
        .to_string();

    let removed = call(
        &router,
        "DeleteSupplementalTaxRegistration",
        &json!({ "authorityId": authority_id }),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "the outstanding record must be deletable, got {removed:?}"
    );

    let again = put_supplemental(&router, &first_entry()).await;
    assert_eq!(
        again.status, 200,
        "with the record gone the write must be applied, got {again:?}"
    );
    let minted = again.get("authorityId").as_str().unwrap_or_default();
    assert_ne!(
        minted, authority_id,
        "a TRN the store no longer holds gets an authorityId of its own, got {minted:?}"
    );
}

/// RULE: a write of a TRN the store does NOT hold is never refused for another
/// record's outstanding validation — the conflict is about the record the write
/// addresses.
#[tokio::test]
async fn another_trn_is_not_refused_for_an_outstanding_record() {
    let (router, _engine) = emulator_supplementing();
    let first = put_supplemental(&router, &first_entry()).await;
    assert_eq!(
        first.status, 200,
        "the first write must store a record, got {first:?}"
    );

    let other = json!({
        "registrationId": "CHE-987.654.321",
        "registrationType": "VAT",
        "legalName": "Beispiel AG",
        "address": { "postalCode": "8001", "countryCode": "CH" },
    });
    let stored = put_supplemental(&router, &other).await;
    assert_eq!(
        (stored.status, stored.get("status").as_str()),
        (200, Some(STATUS_PENDING)),
        "another TRN must be stored beside the outstanding one, got {stored:?}"
    );
    let held = list_all(&router, None).await;
    assert_eq!(
        held.len(),
        2,
        "the store must hold both registrations, got {held:?}"
    );
}

/// RULE: the status a settled supplemental registration reports is the one its
/// validation reached, so a caller that polls sees the record leave the state the
/// conflict is about.
#[tokio::test]
async fn a_settled_record_reports_what_its_validation_reached() {
    let (router, engine) = emulator_supplementing();
    let stored = put_supplemental(&router, &first_entry()).await;
    assert_eq!(
        stored.status, 200,
        "the write must store a record, got {stored:?}"
    );
    make_supplemental_validations_due(&engine);
    let held = list_all(&router, None).await;
    assert_eq!(
        held.first().and_then(|record| record.get("status")),
        Some(&json!(STATUS_VERIFIED)),
        "a settled record reports what its validation reached, got {held:?}"
    );
}
