// SPDX-License-Identifier: CC0-1.0

//! `BatchPutTaxRegistration` over the wire: the per-account fan-out of
//! `PutTaxRegistration`.
//!
//! The tables span the classes the rules generalise over — the number of
//! accounts a request names (each published bound of `accountIds` and one past
//! it), repeated accounts, accounts that already hold a registration, and several
//! entries — rather than the one recorded request.
//!
//! The harness (`emulator`, `emulator_with_engine`, `call`, `hold_validations_open`)
//! lives in `tax_registration_lifecycle.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, StoredTaxRegistration, ERROR_CODE_GENERAL_CONFLICT,
    MAX_STORED_REGISTRATIONS, STATE_CONFLICT_MESSAGE, STATUS_PENDING,
};

use crate::declared_member_defaults::address_as_stored;
use crate::tax_registration_lifecycle::{
    call, emulator, emulator_with_engine, hold_validations_open, make_validations_due,
};

/// `n` distinct 12-digit account ids.
fn account_ids(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("{i:012}")).collect()
}

/// The entries the fan-out table stores: they differ in registration type,
/// sector, country and whether they carry a country block, none of which the
/// fan-out depends on.
pub(crate) fn entries() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal VAT entry",
            json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
        ),
        (
            "GST entry with a sector and a certified email",
            json!({
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
            }),
        ),
        (
            "Kenyan entry carrying its required country block",
            json!({
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
            }),
        ),
    ]
}

/// The members a submitted entry and the registration a read reports share
/// verbatim.
///
/// `legalAddress` is NOT among them: two of its members declare a default the
/// write materialises, so it is asserted through
/// `crate::declared_member_defaults::address_as_stored`.
const ECHOED_MEMBERS: &[&str] = &[
    "registrationId",
    "registrationType",
    "legalName",
    "sector",
    "certifiedEmailId",
];

/// RULE: a batch write stores the submitted entry as the tax registration of
/// EVERY account the request names — reporting the status it stored them with and
/// an empty error list — and each of those registrations reads back exactly as a
/// single-account write of the same entry would.
#[tokio::test]
async fn a_batch_write_registers_every_account_it_names() {
    // Rows: the published minimum of `accountIds` (1), a middle count, the
    // maximum (550), and a list that names one account twice.
    let counts: &[usize] = &[1, 2, 5, 550];
    for (case, entry) in entries() {
        for count in counts {
            let case = format!("{case} over {count} accounts");
            let (router, engine) = emulator_with_engine();
            let ids = account_ids(*count);
            let answer = call(
                &router,
                "BatchPutTaxRegistration",
                &json!({ "accountIds": &ids, "taxRegistrationEntry": entry }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the batch write must succeed, got {answer:?}"
            );
            assert_eq!(
                answer.get("errors").as_array().map(Vec::len),
                Some(0),
                "case {case:?}: no account should have failed, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.get("status").as_str(),
                Some(STATUS_PENDING),
                "case {case:?}: the batch reports the status it stored, got {:?}",
                answer.body
            );

            // Every named account now holds the entry, read back through the
            // single-account reader.
            hold_validations_open(&engine);
            for account_id in &ids {
                let read = call(
                    &router,
                    "GetTaxRegistration",
                    &json!({ "accountId": account_id }),
                )
                .await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: {account_id} must hold a registration, got {read:?}"
                );
                for member in ECHOED_MEMBERS {
                    assert_eq!(
                        read.body
                            .pointer("/taxRegistration")
                            .and_then(|registration| registration.get(member)),
                        entry.get(member),
                        "case {case:?}: {account_id}'s {member} must be what was submitted, got \
                         {:?}",
                        read.body
                    );
                }
                // The legal address is what was submitted, with the defaults the
                // model declares for `addressLine1` and `city` materialised.
                let submitted_address = entry
                    .get("legalAddress")
                    .expect("every entry submits a legal address");
                assert_eq!(
                    read.body.pointer("/taxRegistration/legalAddress"),
                    Some(&address_as_stored(submitted_address)),
                    "case {case:?}: {account_id}'s legalAddress must be what was submitted with \
                     the model's declared defaults materialised, got {:?}",
                    read.body
                );
                assert_eq!(
                    read.body
                        .pointer("/taxRegistration/status")
                        .and_then(Value::as_str),
                    Some(STATUS_PENDING),
                    "case {case:?}: {account_id} must hold the stored status, got {:?}",
                    read.body
                );
            }

            // An account the request did not name holds nothing.
            let unnamed = call(
                &router,
                "GetTaxRegistration",
                &json!({ "accountId": "999999999999" }),
            )
            .await;
            assert_eq!(
                unnamed.status, 404,
                "case {case:?}: an unnamed account must hold nothing, got {unnamed:?}"
            );
        }
    }

    // A list that names one account twice registers it once, with the entry.
    let (router, _engine) = emulator_with_engine();
    let answer = call(
        &router,
        "BatchPutTaxRegistration",
        &json!({
            "accountIds": ["000000000001", "000000000001", "000000000002"],
            "taxRegistrationEntry": entries()[0].1,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"repeated account\": the batch write must succeed, got {answer:?}"
    );
    assert_eq!(
        answer.get("errors").as_array().map(Vec::len),
        Some(0),
        "case \"repeated account\": a repeated id names one write, not a conflict with the record \
         this call wrote, got {:?}",
        answer.body
    );
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
        vec!["000000000001", "000000000002"],
        "case \"repeated account\": a repeated account is registered once, got {accounts:?}"
    );
}

/// RULE: a batch write replaces the record of an account that already holds one
/// whose validation has run, exactly as a single-account write does, rather than
/// adding a second.
///
/// A record whose validation is still outstanding is NOT replaced — that rule,
/// for both writers, is `tests/all/pending_trn_conflict.rs` — so the clock is
/// moved on before the rewrite here.
#[tokio::test]
async fn a_batch_write_replaces_a_record_the_account_already_holds() {
    let (router, engine) = emulator_with_engine();
    let account_id = "000000000007";
    let first = &entries()[0].1;
    let second = &entries()[1].1;
    let put = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": account_id, "taxRegistrationEntry": first }),
    )
    .await;
    assert_eq!(put.status, 200, "setup put failed: {:?}", put.body);
    // The first write's validation falls due, the way the clock would have made
    // it: the record is no longer an outstanding TRN, so it may be rewritten.
    make_validations_due(&engine);

    let batch = call(
        &router,
        "BatchPutTaxRegistration",
        &json!({ "accountIds": [account_id], "taxRegistrationEntry": second }),
    )
    .await;
    assert_eq!(
        batch.status, 200,
        "case \"rewrite\": the batch write must succeed, got {batch:?}"
    );
    hold_validations_open(&engine);
    let read = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    for member in ECHOED_MEMBERS {
        assert_eq!(
            read.body
                .pointer("/taxRegistration")
                .and_then(|registration| registration.get(member)),
            second.get(member),
            "case \"rewrite\": {member} must be the rewritten value, got {:?}",
            read.body
        );
    }
    let rewritten_address = second
        .get("legalAddress")
        .expect("the rewriting entry submits a legal address");
    assert_eq!(
        read.body.pointer("/taxRegistration/legalAddress"),
        Some(&address_as_stored(rewritten_address)),
        "case \"rewrite\": legalAddress must be the rewritten address, got {:?}",
        read.body
    );
    let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
    assert_eq!(
        listed.get("accountDetails").as_array().map(Vec::len),
        Some(1),
        "case \"rewrite\": one account, one record, got {:?}",
        listed.body
    );
}

/// RULE: `accountIds` must hold between 1 and 550 accounts — a request outside
/// that is refused with `ValidationException` naming the member and the bound it
/// missed, and stores nothing.
///
/// Each published bound is tested, and one past it on each side.
#[tokio::test]
async fn account_ids_outside_its_published_bounds_is_refused() {
    let entry = &entries()[0].1;
    // (case, the list, whether the model admits that many)
    let rows: Vec<(String, Vec<String>, bool)> = vec![
        ("no accounts".to_string(), account_ids(0), false),
        ("the minimum".to_string(), account_ids(1), true),
        ("the maximum".to_string(), account_ids(550), true),
        ("one past the maximum".to_string(), account_ids(551), false),
    ];
    for (case, ids, admitted) in rows {
        let router = emulator();
        let answer = call(
            &router,
            "BatchPutTaxRegistration",
            &json!({ "accountIds": &ids, "taxRegistrationEntry": entry }),
        )
        .await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {} accounts is admitted, got {answer:?}",
                ids.len()
            );
            continue;
        }
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: {} accounts must be refused, got {answer:?}",
            ids.len()
        );
        let message = answer
            .get("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        // The envelope this service words a constraint violation with: the
        // member path and the constraint it missed, and nothing of the value.
        let bound = if ids.is_empty() {
            "Member must have length greater than or equal to 1".to_string()
        } else {
            "Member must have length less than or equal to 550".to_string()
        };
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value at 'accountIds' failed to satisfy constraint: \
                 {bound}"
            ),
            "case {case:?}: the refusal must name the member and its constraint, got {message:?}"
        );
        // Nothing was stored: the first account the request named holds nothing.
        let read = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": "000000000001" }),
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused batch must store nothing, got {read:?}"
        );
    }
}

/// RULE: an entry no account may be registered with refuses the whole batch with
/// `ValidationException` and stores nothing — the refusal is of the call, not a
/// per-account error, because it is the same entry for every account.
#[tokio::test]
async fn an_entry_the_service_will_not_store_refuses_the_whole_batch() {
    // Rows: one per class of entry refusal — the country block its country
    // requires, a legal name no reader could report, a blank value the model's
    // non-blank pattern forbids, and an address submitted for a type AWS derives
    // it from.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "Kenyan entry without its country block",
            json!({
                "registrationId": "P123456789K",
                "registrationType": "VAT",
                "legalName": "Acme Kenya Limited",
                "legalAddress": {
                    "addressLine1": "Kenyatta Ave 1",
                    "city": "Nairobi",
                    "postalCode": "00100",
                    "countryCode": "KE",
                },
            }),
            // Kenya's documented rule names the member inside the block
            // (`crate::state::COUNTRY_RULES`), and a refusal names the parameter
            // its member path ends in.
            "PERSON_TYPE",
        ),
        (
            "entry with no legal name",
            json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
            "LEGAL_NAME",
        ),
        (
            "entry with no legal address",
            json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
            }),
            "LEGAL_ADDRESS",
        ),
        (
            "entry with a blank registration id",
            json!({
                "registrationId": "   ",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
            "REGISTRATION_ID",
        ),
        (
            "PAN entry submitting a legal address",
            json!({
                "registrationId": "ABCDE1234F",
                "registrationType": "PAN",
                "legalName": "Acme India Private Limited",
                "legalAddress": { "postalCode": "560025", "countryCode": "IN" },
            }),
            "LEGAL_ADDRESS",
        ),
    ];
    // The same entries are refused however many accounts the batch names.
    for (case, entry, parameter) in rows {
        for count in [1usize, 3] {
            let case = format!("{case} over {count} accounts");
            let router = emulator();
            let ids = account_ids(count);
            let answer = call(
                &router,
                "BatchPutTaxRegistration",
                &json!({ "accountIds": &ids, "taxRegistrationEntry": entry }),
            )
            .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: the batch must be refused, got {answer:?}"
            );
            let named: Vec<String> = answer
                .get("fieldList")
                .as_array()
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(|field| field.get("name").and_then(Value::as_str))
                        .map(std::string::ToString::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                named,
                vec![parameter.to_string()],
                "case {case:?}: the refusal must name {parameter}, got {:?}",
                answer.body
            );
            for account_id in &ids {
                let read = call(
                    &router,
                    "GetTaxRegistration",
                    &json!({ "accountId": account_id }),
                )
                .await;
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: {account_id} must hold nothing, got {read:?}"
                );
            }
        }
    }
}

/// RULE: a batch delete removes the registration of every account it names, and
/// reports an account that held none against that account rather than failing the
/// call — the per-account fan-out of the single-account delete.
///
/// The rows span the mixes a request can name: all registered, none registered,
/// and both at once, over each published bound of `accountIds` (1 and 5) and one
/// past it.
#[tokio::test]
async fn a_batch_delete_removes_every_registered_account_it_names() {
    let entry = &entries()[0].1;
    // (case, how many of the named accounts hold a registration, how many are named)
    let rows: &[(&str, usize, usize)] = &[
        ("one registered account", 1, 1),
        ("one unregistered account", 0, 1),
        ("five registered accounts", 5, 5),
        ("five accounts, two registered", 2, 5),
        ("five unregistered accounts", 0, 5),
    ];
    for (case, registered, named) in rows {
        let router = emulator();
        let ids = account_ids(*named);
        for account_id in ids.iter().take(*registered) {
            let put = call(
                &router,
                "PutTaxRegistration",
                &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
            )
            .await;
            assert_eq!(put.status, 200, "case {case:?}: setup put failed: {put:?}");
        }

        let answer = call(
            &router,
            "BatchDeleteTaxRegistration",
            &json!({ "accountIds": &ids }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the batch delete must succeed, got {answer:?}"
        );
        let errors = answer.get("errors").as_array().cloned().unwrap_or_default();
        let failed: Vec<&str> = errors
            .iter()
            .filter_map(|error| error.get("accountId").and_then(Value::as_str))
            .collect();
        let expected: Vec<&str> = ids
            .iter()
            .skip(*registered)
            .map(std::string::String::as_str)
            .collect();
        assert_eq!(
            failed, expected,
            "case {case:?}: exactly the accounts holding nothing must be reported, got {errors:?}"
        );
        for error in &errors {
            // This service publishes no service-defined code for a missing
            // registration, so the row carries none rather than the exception's
            // shape name.
            assert_eq!(
                error.get("code").and_then(Value::as_str),
                None,
                "case {case:?}: the error must carry no code, got {error:?}"
            );
        }

        // Every named account now holds nothing, and a second delete reports
        // them all.
        for account_id in &ids {
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
        let again = call(
            &router,
            "BatchDeleteTaxRegistration",
            &json!({ "accountIds": &ids }),
        )
        .await;
        assert_eq!(
            again.get("errors").as_array().map(Vec::len),
            Some(ids.len()),
            "case {case:?}: a second delete reports every account, got {:?}",
            again.body
        );
    }

    // One past the published maximum is refused, and nothing is deleted.
    let router = emulator();
    let ids = account_ids(6);
    for account_id in &ids {
        call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
        )
        .await;
    }
    let answer = call(
        &router,
        "BatchDeleteTaxRegistration",
        &json!({ "accountIds": &ids }),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (400, "ValidationException"),
        "case \"six accounts\": more than five must be refused, got {answer:?}"
    );
    for account_id in &ids {
        let read = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case \"six accounts\": a refused delete must remove nothing, got {read:?}"
        );
    }
}

/// RULE: the primary-TRN lifecycle runs end to end over the four write and read
/// paths — write one account, read it, delete it, write a batch, delete that
/// batch — and the store is empty again at the end.
#[tokio::test]
async fn the_primary_trn_lifecycle_completes() {
    let (router, engine) = emulator_with_engine();
    let account_id = "000000000001";
    let entry = &entries()[2].1;

    let put = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
    )
    .await;
    assert_eq!(put.status, 200, "Put must succeed, got {put:?}");
    hold_validations_open(&engine);
    let read = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(read.status, 200, "Get must report it, got {read:?}");
    let deleted = call(
        &router,
        "DeleteTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    assert_eq!(deleted.status, 200, "Delete must succeed, got {deleted:?}");

    let batch = call(
        &router,
        "BatchPutTaxRegistration",
        &json!({ "accountIds": [account_id], "taxRegistrationEntry": entry }),
    )
    .await;
    assert_eq!(batch.status, 200, "BatchPut must succeed, got {batch:?}");
    let batch_deleted = call(
        &router,
        "BatchDeleteTaxRegistration",
        &json!({ "accountIds": [account_id] }),
    )
    .await;
    assert_eq!(
        batch_deleted.get("errors").as_array().map(Vec::len),
        Some(0),
        "BatchDelete must delete what BatchPut wrote, got {:?}",
        batch_deleted.body
    );
    let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
    assert_eq!(
        listed.get("accountDetails").as_array().map(Vec::len),
        Some(0),
        "the store must be empty at the end of the lifecycle, got {:?}",
        listed.body
    );
}

/// RULE: an account the service could not write while the entry itself was fine
/// is reported in the response's `errors` list — naming that account and why —
/// and the rest of the batch is still stored.
///
/// The one such reason this emulator has is its resident bound: a record it has
/// no room for is the refusal `PutTaxRegistration` answers `ConflictException`
/// for, reported per account here.
#[tokio::test]
async fn an_account_the_batch_could_not_write_is_reported_against_that_account() {
    let (router, engine) = emulator_with_engine();
    let entry = &entries()[0].1;

    // Fill the store to its bound, then name one account that is already in it
    // and one that is not.
    {
        let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
        let mut store = state.lock();
        for index in 0..MAX_STORED_REGISTRATIONS {
            store.registrations.insert(
                format!("{:012}", index + 100_000),
                StoredTaxRegistration::default(),
            );
        }
    }
    let resident = format!("{:012}", 100_000);
    let newcomer = "999999999999";
    let answer = call(
        &router,
        "BatchPutTaxRegistration",
        &json!({
            "accountIds": [resident, newcomer],
            "taxRegistrationEntry": entry,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"store at its bound\": the batch itself succeeds, got {answer:?}"
    );
    let errors = answer.get("errors").as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        1,
        "case \"store at its bound\": exactly the account with no room fails, got {errors:?}"
    );
    assert_eq!(
        errors[0].get("accountId").and_then(Value::as_str),
        Some(newcomer),
        "case \"store at its bound\": the error must name the account, got {errors:?}"
    );
    assert_eq!(
        (
            errors[0].get("code").and_then(Value::as_str),
            errors[0].get("message").and_then(Value::as_str),
        ),
        (
            Some(ERROR_CODE_GENERAL_CONFLICT),
            Some(STATE_CONFLICT_MESSAGE),
        ),
        "case \"store at its bound\": the row must carry the envelope the conflict class \
         publishes, got {errors:?}"
    );

    // The account that was already resident was still written.
    hold_validations_open(&engine);
    let read = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": resident }),
    )
    .await;
    assert_eq!(
        read.body
            .pointer("/taxRegistration/registrationId")
            .and_then(Value::as_str),
        entry.get("registrationId").and_then(Value::as_str),
        "case \"store at its bound\": the resident account was written, got {:?}",
        read.body
    );
    // And the one with no room holds nothing.
    let read = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": newcomer }),
    )
    .await;
    assert_eq!(
        read.status, 404,
        "case \"store at its bound\": the refused account holds nothing, got {read:?}"
    );
}
