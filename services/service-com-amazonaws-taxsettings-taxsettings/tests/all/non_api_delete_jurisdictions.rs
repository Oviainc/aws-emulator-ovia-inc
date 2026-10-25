// SPDX-License-Identifier: CC0-1.0

//! Which stored tax registrations the delete API may remove, end to end over the
//! restJson1 wire.
//!
//! The rule under test: a stored tax registration held in a jurisdiction whose
//! registration the API's own documentation carves out
//! (`crate::state::COUNTRIES_WITH_NON_API_REGISTRATION_DELETE`) is not removed by
//! a delete call — the call is refused with the modelled `ValidationException`
//! (HTTP 400) and the record stays readable through every later read — while a
//! registration held anywhere else is removed as before. The jurisdiction is the
//! one the record is HELD in (its stored `legalAddress.countryCode`), not the
//! `TaxRegistrationType` it was submitted with, and both delete paths
//! (`DeleteTaxRegistration` and `BatchDeleteTaxRegistration`) apply it.
//!
//! The rows span what the rule generalises over: the jurisdiction (every entry of
//! the carved-out catalog, and several that are not in it), the letter case the
//! `countryCode` was submitted in, the registration type stored under that
//! jurisdiction (including a Brazil-specific type stored in a jurisdiction that
//! is NOT carved out, which is deletable — the key is the country), which account
//! the call addresses (a named one and the caller's own, by omitting `accountId`)
//! and the delete path, with the carved-out account sitting beside deletable ones
//! and ones holding nothing in a batch.
//!
//! The harness (`emulator`, `call`) lives in `tax_registration_lifecycle.rs`;
//! nothing here builds a second copy of it.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    non_api_delete_country, parameter_name, COUNTRIES_WITH_NON_API_REGISTRATION_DELETE,
    ERROR_CODE_FIELD_VALIDATION_FAILED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_registration_lifecycle::{call, emulator, field_list};

/// One stored registration a delete is aimed at: the jurisdiction its legal
/// address is submitted with, and the type it is registered under.
struct Row {
    case: &'static str,
    /// The `countryCode` of the registration's legal address, as submitted.
    country_code: &'static str,
    /// A value of the model's `TaxRegistrationType` enum. `PAN` is left out:
    /// AWS derives its legal address from the tax number, so an entry carrying
    /// one is refused before anything is stored
    /// (`tests/all/fidelity_rules.rs` covers that rule).
    registration_type: &'static str,
    /// The account the registration is stored under, and the one the delete
    /// names. `None` addresses the caller's own account — the request omits
    /// `accountId`, which every `accountId` member of this service documents as
    /// defaulting to the caller's account.
    account_id: Option<&'static str>,
}

/// Registrations held in a jurisdiction the delete API carves out.
const CARVED_OUT: &[Row] = &[
    Row {
        case: "CNPJ registration in the carved-out jurisdiction",
        country_code: "BR",
        registration_type: "CNPJ",
        account_id: Some("000000000001"),
    },
    Row {
        case: "CPF registration in the carved-out jurisdiction",
        country_code: "BR",
        registration_type: "CPF",
        account_id: Some("000000000002"),
    },
    Row {
        case: "VAT registration in the carved-out jurisdiction",
        country_code: "BR",
        registration_type: "VAT",
        account_id: Some("000000000003"),
    },
    Row {
        case: "carved-out jurisdiction submitted in lower case",
        country_code: "br",
        registration_type: "TIN",
        account_id: Some("000000000004"),
    },
    Row {
        case: "carved-out jurisdiction held by the caller's own account",
        country_code: "BR",
        registration_type: "CNPJ",
        account_id: None,
    },
];

/// Registrations held anywhere else, which the delete API removes.
const DELETABLE: &[Row] = &[
    Row {
        case: "VAT registration in another jurisdiction",
        country_code: "DE",
        registration_type: "VAT",
        account_id: Some("000000000011"),
    },
    Row {
        case: "another jurisdiction submitted in lower case",
        country_code: "pl",
        registration_type: "NIP",
        account_id: Some("000000000012"),
    },
    // The rule is keyed on the jurisdiction the record is HELD in, not on the
    // type it is registered under: a type the carved-out jurisdiction uses is
    // deletable when the record is held elsewhere.
    Row {
        case: "a carved-out jurisdiction's registration type held elsewhere",
        country_code: "PT",
        registration_type: "CNPJ",
        account_id: Some("000000000013"),
    },
    Row {
        case: "another jurisdiction held by the caller's own account",
        country_code: "NL",
        registration_type: "VAT",
        account_id: None,
    },
];

/// The account a row's calls address: the id it names, or the caller's own when
/// it names none.
fn addressed(row: &Row) -> String {
    row.account_id
        .map_or_else(|| engine::emulator_account_id().to_string(), str::to_string)
}

/// A `TaxRegistrationEntry` of `registration_type` whose legal address is held in
/// `country_code`. Only the members every entry needs are filled in, plus
/// `stateOrRegion`, which the documented rule of one of these rows (a Brazilian
/// CPF registration) requires and which every other country admits as optional;
/// no other country in these rows has a documented per-country requirement
/// (`crate::state::COUNTRY_RULES`), which `tests/all/country_requirements.rs`
/// covers for the ones that do.
fn entry(registration_type: &str, country_code: &str) -> Value {
    json!({
        "registrationId": format!("{registration_type}-0000000001"),
        "registrationType": registration_type,
        "legalName": "Example Holdings",
        "legalAddress": {
            "city": "Example City",
            "stateOrRegion": "Example Region",
            "postalCode": "01001-000",
            "countryCode": country_code,
        },
    })
}

/// Store `row`'s registration, and answer the request body a call addressing it
/// carries (naming the account, or naming none for the caller's own).
async fn store(router: &MultiServiceRouter, row: &Row) -> Value {
    let mut request = json!({
        "taxRegistrationEntry": entry(row.registration_type, row.country_code),
    });
    let body = match row.account_id {
        Some(account_id) => {
            request
                .as_object_mut()
                .expect("an object")
                .insert("accountId".to_string(), json!(account_id));
            json!({ "accountId": account_id })
        }
        None => json!({}),
    };
    let put = call(router, "PutTaxRegistration", &request).await;
    assert_eq!(
        put.status, 200,
        "case {:?}: storing the registration must succeed, got {put:?}",
        row.case
    );
    body
}

/// Assert that `account_id` reports a registration of `registration_type`.
async fn assert_still_reported(
    router: &MultiServiceRouter,
    case: &str,
    read_body: &Value,
    registration_type: &str,
) {
    let read = call(router, "GetTaxRegistration", read_body).await;
    assert_eq!(
        (read.status, read.error_code.as_str()),
        (200, ""),
        "case {case:?}: the refused delete must leave the registration readable, got {read:?}"
    );
    assert_eq!(
        read.get("taxRegistration")
            .get("registrationType")
            .and_then(Value::as_str),
        Some(registration_type),
        "case {case:?}: expected the stored {registration_type} registration, got {:?}",
        read.body
    );
}

/// RULE (single-account path): a delete aimed at a registration held in a
/// carved-out jurisdiction is refused with `ValidationException` (400) naming the
/// jurisdiction, and the record is left readable.
#[tokio::test]
async fn a_carved_out_jurisdictions_registration_is_not_deleted_by_the_api() {
    for row in CARVED_OUT {
        let case = row.case;
        let router = emulator();
        let body = store(&router, row).await;
        let country = non_api_delete_country(row.country_code)
            .unwrap_or_else(|| panic!("case {case:?}: the row's jurisdiction must be carved out"));

        let refused = call(&router, "DeleteTaxRegistration", &body).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.get("errorCode").as_str(),
            Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            "case {case:?}: expected errorCode {ERROR_CODE_FIELD_VALIDATION_FAILED}, got {:?}",
            refused.body
        );
        assert_eq!(
            field_list(&refused),
            vec![parameter_name("accountId")],
            "case {case:?}: the refusal must name the parameter that selected the record, got {:?}",
            refused.body
        );
        let message = refused.get("message").as_str().unwrap_or_default().to_string();
        assert!(
            message.contains(country),
            "case {case:?}: the message must state the jurisdiction {country}, got {message:?}"
        );

        assert_still_reported(&router, case, &body, row.registration_type).await;

        // The listing reports it too: nothing was removed from the store.
        let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
        let accounts: Vec<&str> = listed
            .get("accountDetails")
            .as_array()
            .map(|details| {
                details
                    .iter()
                    .filter_map(|detail| detail.get("accountId").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            accounts,
            vec![addressed(row).as_str()],
            "case {case:?}: the listing must still carry the registration, got {:?}",
            listed.body
        );

        // Repeating the call answers the same way: the refusal removed nothing,
        // so it is not a one-shot.
        let again = call(&router, "DeleteTaxRegistration", &body).await;
        assert_eq!(
            (again.status, &again.body),
            (refused.status, &refused.body),
            "case {case:?}: the second delete must answer as the first did, got {again:?}"
        );
    }
}

/// RULE (batch path): the same record is refused in a batch delete — reported in
/// `errors` with the code the single-account delete raises and left in place —
/// while the deletable accounts of the same batch are still removed and an
/// account holding nothing is still reported not found.
#[tokio::test]
async fn a_carved_out_jurisdictions_registration_is_not_deleted_by_the_batch() {
    const DELETABLE_IN_BATCH: &str = "000000000021";
    const HOLDS_NOTHING: &str = "000000000022";

    for row in CARVED_OUT {
        let case = row.case;
        let router = emulator();
        let body = store(&router, row).await;
        let carved = addressed(row);
        store(
            &router,
            &Row {
                case: "a deletable account beside the carved-out one",
                country_code: "DE",
                registration_type: "VAT",
                account_id: Some(DELETABLE_IN_BATCH),
            },
        )
        .await;

        let answer = call(
            &router,
            "BatchDeleteTaxRegistration",
            &json!({ "accountIds": [DELETABLE_IN_BATCH, carved, HOLDS_NOTHING] }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: the batch delete itself must succeed, got {answer:?}"
        );
        let reported: Vec<(String, String)> = answer
            .get("errors")
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .map(|error| {
                        (
                            error
                                .get("accountId")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            error
                                .get("code")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported,
            vec![
                (carved.clone(), "ValidationException".to_string()),
                (
                    HOLDS_NOTHING.to_string(),
                    "ResourceNotFoundException".to_string()
                ),
            ],
            "case {case:?}: expected the carved-out account refused and the empty one not found, \
             got {:?}",
            answer.body
        );

        assert_still_reported(&router, case, &body, row.registration_type).await;

        let deleted = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": DELETABLE_IN_BATCH }),
        )
        .await;
        assert_eq!(
            (deleted.status, deleted.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the rest of the batch must still be deleted, got {deleted:?}"
        );
    }
}

/// RULE (the other side of it): a registration held in a jurisdiction that is
/// not carved out is removed by both delete paths, so later reads answer 404.
#[tokio::test]
async fn a_registration_held_elsewhere_is_deleted_by_both_paths() {
    for row in DELETABLE {
        let case = row.case;
        assert!(
            non_api_delete_country(row.country_code).is_none(),
            "case {case:?}: the row's jurisdiction must not be carved out, got {:?}",
            row.country_code
        );
        let account_id = addressed(row);

        for operation in ["DeleteTaxRegistration", "BatchDeleteTaxRegistration"] {
            let router = emulator();
            let body = store(&router, row).await;
            let request = if operation == "DeleteTaxRegistration" {
                body.clone()
            } else {
                json!({ "accountIds": [&account_id] })
            };
            let deleted = call(&router, operation, &request).await;
            assert_eq!(
                (deleted.status, deleted.error_code.as_str()),
                (200, ""),
                "case {case:?} via {operation}: the delete must succeed, got {deleted:?}"
            );
            assert_eq!(
                deleted.get("errors").as_array().map(Vec::len).unwrap_or(0),
                0,
                "case {case:?} via {operation}: no account may be reported as failed, got {:?}",
                deleted.body
            );

            let read = call(&router, "GetTaxRegistration", &body).await;
            assert_eq!(
                (read.status, read.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} via {operation}: expected 404 after the delete, got {read:?}"
            );
        }
    }
}

/// RULE (over the whole catalog): EVERY jurisdiction the carve-out names is
/// refused and left in place, whichever letter case it was submitted in — not
/// only the rows spelled out above, so a jurisdiction added to the catalog is
/// covered here.
#[tokio::test]
async fn every_carved_out_jurisdiction_is_refused() {
    assert!(
        !COUNTRIES_WITH_NON_API_REGISTRATION_DELETE.is_empty(),
        "the carve-out catalog must name at least one jurisdiction"
    );
    for (country_code, country) in COUNTRIES_WITH_NON_API_REGISTRATION_DELETE {
        for submitted in [country_code.to_uppercase(), country_code.to_lowercase()] {
            let case = format!("{country} as {submitted:?}");
            let router = emulator();
            let body = json!({ "accountId": "000000000031" });
            let put = call(
                &router,
                "PutTaxRegistration",
                &json!({
                    "accountId": "000000000031",
                    "taxRegistrationEntry": entry("VAT", &submitted),
                }),
            )
            .await;
            assert_eq!(
                put.status, 200,
                "case {case:?}: storing the registration must succeed, got {put:?}"
            );

            let refused = call(&router, "DeleteTaxRegistration", &body).await;
            assert_eq!(
                (refused.status, refused.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {refused:?}"
            );
            assert!(
                refused
                    .get("message")
                    .as_str()
                    .is_some_and(|message| message.contains(country)),
                "case {case:?}: the message must state the jurisdiction, got {:?}",
                refused.body
            );

            assert_still_reported(&router, &case, &body, "VAT").await;
        }
    }
}
