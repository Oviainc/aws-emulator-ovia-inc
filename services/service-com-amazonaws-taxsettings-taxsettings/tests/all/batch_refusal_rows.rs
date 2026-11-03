// SPDX-License-Identifier: CC0-1.0

//! The per-account `errors[]` rows the batch write and the batch delete report,
//! end to end over the restJson1 wire.
//!
//! Two rules, one per batch operation, each about the channel that names the
//! accounts an operation could not act on:
//!
//! * `BatchPutTaxRegistration` reports a row for EVERY account it names when the
//!   submitted number is not written the way its jurisdiction's authority writes
//!   one — the class's own token as the row's `message`, no `code`, and nothing
//!   stored for any account, so the call carries no top-level `status` either:
//!   `status` is the status a registration was STORED with, and a call that
//!   stored none has none.
//! * `BatchDeleteTaxRegistration` reports a row for an account that holds no
//!   registration, carrying the token this service words that class with, and
//!   still deletes every account named beside it.
//!
//! Each table spans the class its rule generalises over: the jurisdictions and
//! registration types whose number structures this service publishes, how many
//! accounts one call names (one, several, the published bound, a repeated id),
//! and — for the delete — which of the named accounts hold a registration and
//! where in the list they sit. The single-account counterpart of each refusal is
//! asserted beside it, so the two channels cannot drift apart: the refusal is one
//! decision rendered two ways.
//!
//! The harness (`emulator`, `emulator_with_engine`, `call`, `make_validations_due`)
//! lives in `tax_registration_lifecycle.rs` and the row reader in
//! `pending_trn_conflict.rs`; nothing here builds a second copy of either.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    no_tax_registration, ERROR_CODE_FIELD_VALIDATION_FAILED, INVALID_TAX_REGISTRATION_NUMBER,
    STATUS_PENDING, TAX_REGISTRATION_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::pending_trn_conflict::error_rows;
use crate::tax_registration_lifecycle::{
    call, emulator, emulator_with_engine, field_list, make_validations_due, Answer,
};

/// One entry the batch write submits: the jurisdiction and type it is held
/// under, a number written the way that authority writes one and a value that is
/// not.
struct NumberCase {
    case: &'static str,
    /// The `taxRegistrationEntry` carrying a number written the published way.
    admitted: Value,
    /// The same entry carrying a value that is not — differing from the
    /// admitted one only in its length or its character layout, which is what
    /// the write judges (`state::check_registration_number`; whether a number of
    /// the published layout is CONFIRMED by its authority's rule is the
    /// validation's own question, pinned in
    /// `tests/all/trn_validation_outcome.rs`).
    refused: Value,
}

/// A German VAT entry carrying `registration_id`.
fn german_vat(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "VAT",
        "legalName": "Beispiel GmbH",
        "legalAddress": { "postalCode": "10117", "countryCode": "DE" },
    })
}

/// A French VAT entry. France's documented write rule is the `sirenNumber`
/// block, which both entries carry, so the number is what each row is judged on.
fn french_vat(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "VAT",
        "legalName": "Exemple SARL",
        "legalAddress": { "postalCode": "75002", "countryCode": "FR" },
        "additionalTaxInformation": {
            "franceAdditionalInfo": { "sirenNumber": "345678901" },
        },
    })
}

/// A Brazilian CNPJ entry, whose legal name and address the service fills in.
fn brazilian_cnpj(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "CNPJ",
    })
}

/// An Indian PAN entry, whose legal address the service fills in.
fn indian_pan(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "PAN",
        "legalName": "Example India Private Limited",
    })
}

/// An Indian GST entry. India's write rule requires the address's
/// `stateOrRegion`.
fn indian_gst(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "GST",
        "legalName": "Example India Private Limited",
        "legalAddress": {
            "stateOrRegion": "Karnataka",
            "city": "Bengaluru",
            "postalCode": "560025",
            "countryCode": "IN",
        },
    })
}

/// A Kenyan TIN entry. Kenya's write rule requires the `personType`.
fn kenyan_tin(registration_id: &str) -> Value {
    json!({
        "registrationId": registration_id,
        "registrationType": "TIN",
        "legalName": "Example Kenya Limited",
        "legalAddress": { "postalCode": "00100", "countryCode": "KE" },
        "additionalTaxInformation": {
            "kenyaAdditionalInfo": { "personType": "Business" },
        },
    })
}

/// One row per jurisdiction and registration type whose number structure this
/// service publishes, so the rule is asserted over the whole class rather than
/// over the one jurisdiction an issue cited.
fn number_cases() -> Vec<NumberCase> {
    vec![
        NumberCase {
            case: "a German VAT number",
            admitted: german_vat("DE136695976"),
            refused: german_vat("DE13669597"),
        },
        NumberCase {
            case: "a German VAT number without its country prefix",
            admitted: german_vat("136695976"),
            refused: german_vat("1366959761"),
        },
        NumberCase {
            case: "a French VAT number",
            admitted: french_vat("FR15345678901"),
            refused: french_vat("FR1534567890"),
        },
        NumberCase {
            case: "a Brazilian CNPJ",
            admitted: brazilian_cnpj("12.345.678/0001-95"),
            refused: brazilian_cnpj("12.345.678/0001-9"),
        },
        NumberCase {
            case: "an Indian PAN",
            admitted: indian_pan("ABCDE1234F"),
            refused: indian_pan("ABCD12345F"),
        },
        NumberCase {
            case: "an Indian GSTIN",
            admitted: indian_gst("29ABCDE1234F1Z5"),
            refused: indian_gst("29ABCDE1234F1X5"),
        },
        NumberCase {
            case: "a Kenyan PIN",
            admitted: kenyan_tin("P051234567M"),
            refused: kenyan_tin("X051234567M"),
        },
    ]
}

/// The account-id lists one call names: one account, several, the published
/// `@length` bound of `accountIds` and a list repeating an id, which names one
/// account.
fn account_lists() -> Vec<(&'static str, Vec<String>, Vec<String>)> {
    let bound: Vec<String> = (1..=550).map(|index| format!("{index:012}")).collect();
    vec![
        (
            "one account",
            vec!["000000000001".to_string()],
            vec!["000000000001".to_string()],
        ),
        (
            "three accounts",
            vec![
                "000000000001".to_string(),
                "000000000002".to_string(),
                "000000000003".to_string(),
            ],
            vec![
                "000000000001".to_string(),
                "000000000002".to_string(),
                "000000000003".to_string(),
            ],
        ),
        (
            "the same account named twice",
            vec!["000000000004".to_string(), "000000000004".to_string()],
            vec!["000000000004".to_string()],
        ),
        ("the 550 accounts the bound admits", bound.clone(), bound),
    ]
}

/// The `GetTaxRegistration` answer for `account_id`.
async fn read(router: &MultiServiceRouter, account_id: &str) -> Answer {
    call(
        router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await
}

/// RULE: a batch write whose submitted number is not written the way its
/// jurisdiction's authority writes one answers 200 with one row per addressed
/// account — the class's token as the row's `message` and no `code` — stores
/// nothing for any of them and reports no top-level `status`; the same entry
/// carrying a number written that way stores every account and reports the
/// status it stored them with.
#[tokio::test]
async fn an_invalid_number_is_reported_against_every_account_the_batch_names() {
    for number in number_cases() {
        for (list_case, account_ids, addressed) in account_lists() {
            let case = format!("{} for {list_case}", number.case);
            let router = emulator();

            let refused = call(
                &router,
                "BatchPutTaxRegistration",
                &json!({
                    "accountIds": account_ids,
                    "taxRegistrationEntry": number.refused,
                }),
            )
            .await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (200, ""),
                "case {case:?}: the batch call itself succeeds, got {refused:?}"
            );
            let rows = error_rows(&refused);
            let reported: Vec<String> = rows.iter().map(|(id, _, _)| id.clone()).collect();
            assert_eq!(
                reported, addressed,
                "case {case:?}: every addressed account must be reported, in the order the \
                 request named them, got {rows:?}"
            );
            for (account_id, code, message) in &rows {
                assert_eq!(
                    (code.as_str(), message.as_str()),
                    ("", INVALID_TAX_REGISTRATION_NUMBER),
                    "case {case:?}: {account_id}'s row carries the class's token and no code, got \
                     {rows:?}"
                );
            }
            assert!(
                refused.body.get("status").is_none(),
                "case {case:?}: a batch that stored nothing has no stored status, got {:?}",
                refused.body
            );
            for account_id in &addressed {
                let held = read(&router, account_id).await;
                assert_eq!(
                    (held.status, held.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: nothing must be stored for {account_id}, got {held:?}"
                );
            }

            // The control: the same entry with a number the authority issues is
            // stored for every account the call names.
            let stored = call(
                &router,
                "BatchPutTaxRegistration",
                &json!({
                    "accountIds": account_ids,
                    "taxRegistrationEntry": number.admitted,
                }),
            )
            .await;
            assert_eq!(
                (stored.status, stored.get("status").as_str()),
                (200, Some(STATUS_PENDING)),
                "case {case:?}: an admitted number is stored for every account, got {stored:?}"
            );
            assert!(
                error_rows(&stored).is_empty(),
                "case {case:?}: an admitted number fails no account, got {:?}",
                stored.body
            );
        }
    }
}

/// RULE (the same decision on the single-account channel): the write of ONE
/// account refuses the same values with the modelled `ValidationException`,
/// carrying the class's token as its message, the code for a field that failed
/// validation and the parameter that carried the number.
#[tokio::test]
async fn the_single_account_write_refuses_the_same_numbers() {
    for number in number_cases() {
        let case = number.case;
        let router = emulator();
        let refused = call(
            &router,
            "PutTaxRegistration",
            &json!({
                "accountId": "000000000001",
                "taxRegistrationEntry": number.refused,
            }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: the single-account write must refuse it, got {refused:?}"
        );
        assert_eq!(
            (
                refused.get("errorCode").as_str(),
                refused.get("message").as_str(),
                field_list(&refused),
            ),
            (
                Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
                Some(INVALID_TAX_REGISTRATION_NUMBER),
                vec!["REGISTRATION_ID".to_string()],
            ),
            "case {case:?}: expected the refusal class's own envelope, got {:?}",
            refused.body
        );
        let stored = call(
            &router,
            "PutTaxRegistration",
            &json!({
                "accountId": "000000000001",
                "taxRegistrationEntry": number.admitted,
            }),
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the admitted number must be stored, got {stored:?}"
        );
    }
}

/// One delete case: which of the accounts the call names hold a registration.
struct DeleteCase {
    case: &'static str,
    /// The accounts the request names, in request order.
    named: Vec<String>,
    /// Those of them a registration is stored for first.
    holding: Vec<String>,
}

/// The delete rows: the accounts a call names against which of them hold a
/// registration, spanning where a holder sits in the list, how many accounts are
/// named and a repeated id.
fn delete_cases() -> Vec<DeleteCase> {
    let id = |index: usize| format!("{index:012}");
    vec![
        DeleteCase {
            case: "one account holding nothing",
            named: vec![id(1)],
            holding: vec![],
        },
        DeleteCase {
            case: "one account holding a registration",
            named: vec![id(1)],
            holding: vec![id(1)],
        },
        DeleteCase {
            case: "a holder named before an account holding nothing",
            named: vec![id(1), id(2)],
            holding: vec![id(1)],
        },
        DeleteCase {
            case: "a holder named after an account holding nothing",
            named: vec![id(1), id(2)],
            holding: vec![id(2)],
        },
        DeleteCase {
            case: "two accounts holding nothing around a holder",
            named: vec![id(1), id(2), id(3)],
            holding: vec![id(2)],
        },
        DeleteCase {
            case: "no account holding anything",
            named: vec![id(1), id(2), id(3)],
            holding: vec![],
        },
        DeleteCase {
            case: "an account holding nothing named twice",
            named: vec![id(4), id(4)],
            holding: vec![],
        },
    ]
}

/// RULE: the batch delete reports a row for each account that holds no
/// registration — the token this service words that class with, and no `code` —
/// in the order the request named those accounts, while every account that did
/// hold one is deleted.
#[tokio::test]
async fn an_account_holding_no_registration_is_reported_against_that_account() {
    for row in delete_cases() {
        let case = row.case;
        let (router, engine) = emulator_with_engine();
        for account_id in &row.holding {
            let stored = call(
                &router,
                "PutTaxRegistration",
                &json!({
                    "accountId": account_id,
                    "taxRegistrationEntry": german_vat("DE136695976"),
                }),
            )
            .await;
            assert_eq!(
                stored.status, 200,
                "case {case:?}: seeding {account_id} must succeed, got {stored:?}"
            );
        }
        // A pending TRN is not what the delete decision is about here, so the
        // clock settles what it owed before the call.
        make_validations_due(&engine);

        let deleted = call(
            &router,
            "BatchDeleteTaxRegistration",
            &json!({ "accountIds": row.named }),
        )
        .await;
        assert_eq!(
            (deleted.status, deleted.error_code.as_str()),
            (200, ""),
            "case {case:?}: the batch call itself succeeds, got {deleted:?}"
        );

        let mut expected: Vec<String> = Vec::new();
        for account_id in &row.named {
            if !row.holding.contains(account_id) && !expected.contains(account_id) {
                expected.push(account_id.clone());
            }
        }
        let rows = error_rows(&deleted);
        let reported: Vec<String> = rows.iter().map(|(id, _, _)| id.clone()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: exactly the accounts holding nothing must be reported, in request \
             order, got {rows:?}"
        );
        for (account_id, code, message) in &rows {
            assert_eq!(
                (code.as_str(), message.as_str()),
                ("", TAX_REGISTRATION_NOT_FOUND),
                "case {case:?}: {account_id}'s row carries the class's token and no code, got \
                 {rows:?}"
            );
        }
        for account_id in &row.holding {
            let held = read(&router, account_id).await;
            assert_eq!(
                (held.status, held.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: {account_id}'s registration must be deleted, got {held:?}"
            );
        }
    }
}

/// RULE (the same decision on the single-account channel): deleting the
/// registration of an account that holds none answers the modelled
/// `ResourceNotFoundException`, carrying that exception's own sentence — the
/// wording each channel of this class is recorded with: the exception's
/// sentence here, the class's token in the batch row above.
#[tokio::test]
async fn the_single_account_delete_answers_the_missing_resource_refusal() {
    let router = emulator();
    for operation in ["GetTaxRegistration", "DeleteTaxRegistration"] {
        for account_id in [
            "000000000001",
            "999999999999",
            engine::emulator_account_id(),
        ] {
            let case = format!("{operation} for {account_id}");
            let answer = call(&router, operation, &json!({ "accountId": account_id })).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected the missing-resource refusal, got {answer:?}"
            );
            assert_eq!(
                answer.get("message").as_str(),
                Some(no_tax_registration().message.as_str()),
                "case {case:?}: expected the exception's own wording, got {:?}",
                answer.body
            );
        }
    }
}
