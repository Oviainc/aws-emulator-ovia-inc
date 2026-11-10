// SPDX-License-Identifier: CC0-1.0

//! The message every `ConflictException` of this service carries, per the state
//! it conflicted with.
//!
//! RULE: a refusal's message is the fixed text the service words for the
//! condition being refused, so one error class whose code covers several
//! conditions still tells the caller WHICH state conflicted — a write of an
//! account's tax registration refused because that account's TRN is still
//! awaiting validation carries the service's pending-TRN sentence
//! (`state::PENDING_REGISTRATION_CONFLICT_MESSAGE`), and a conflict the service
//! words no sentence of its own for carries the class's general sentence
//! (`state::STATE_CONFLICT_MESSAGE`). Both are entirely fixed text: neither
//! interpolates an account id, a registration id, a jurisdiction or any other
//! value the request or the store carries.
//!
//! The table spans the classes the rule generalises over: every condition this
//! service refuses with the conflict class, each on every channel its operations
//! render a conflict on (a 409 response body, and a per-account row of a batch's
//! `errors` list), over entries differing in jurisdiction, registration type and
//! registration number and over several account ids — none of which the message
//! turns on, which is what "fixed text" means and what the last test asserts
//! directly.
//!
//! The harness (`emulator_with_engine`, `call`), the writer fan-out (`WRITERS`,
//! `write`), the outstanding-record staging (`hold_open`, `error_rows`), the
//! resident-bound padding (`fill_the_store`), the entry builder (`entry_of`) and
//! the supplemental write (`emulator_supplementing`, `entry_for`, `put`,
//! `supported_country`) all come from the suites that own them; nothing here
//! builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ERROR_CODE_GENERAL_CONFLICT, MAX_STORED_REGISTRATIONS,
    PENDING_REGISTRATION_CONFLICT_MESSAGE, STATE_CONFLICT_MESSAGE, STATUS_PENDING,
};

use crate::batch_row_error_codes::fill_the_store;
use crate::country_rules::{write, WRITERS};
use crate::pending_trn_conflict::{error_rows, hold_open};
use crate::registration_type_refusals::entry_of;
use crate::shared_supplemental_lifecycle::supported_country;
use crate::supplemental_tax_registrations::{
    emulator_supplementing, entry_for, put as put_supplemental,
};
use crate::tax_registration_lifecycle::{call, emulator_with_engine, Answer};

/// The batch writer, whose conflict channel is a row of `errors` rather than a
/// response status.
const BATCH_WRITER: &str = "BatchPutTaxRegistration";

/// The supplemental writer, whose conflicts are the ones this service words no
/// sentence of its own for.
const SUPPLEMENTAL_WRITER: &str = "PutSupplementalTaxRegistration";

/// One state a call of this service can conflict with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Condition {
    /// The account's own stored TRN is still awaiting validation, so the write
    /// of its registration is refused and the record left as it was.
    PendingOwnRegistration,
    /// A write that would ADD a registration meets the resident store bound.
    NoRoomForANewRegistration,
    /// A supplemental write for an account holding no settled registration of
    /// its own: there is nothing for it to supplement.
    NothingToSupplement,
    /// A supplemental write addressing a supplemental record whose validation is
    /// still outstanding.
    PendingSupplementalRegistration,
}

impl Condition {
    /// The message a refusal of this condition carries.
    fn expected_message(self) -> &'static str {
        match self {
            Self::PendingOwnRegistration => PENDING_REGISTRATION_CONFLICT_MESSAGE,
            Self::NoRoomForANewRegistration
            | Self::NothingToSupplement
            | Self::PendingSupplementalRegistration => STATE_CONFLICT_MESSAGE,
        }
    }
}

/// The request values a case varies: the account a tax-registration write
/// addresses and the `(registrationType, countryCode)` of the entries it
/// submits, beside the `(registrationId, legalName)` the supplemental write
/// submits.
///
/// The type-and-country pair picks the number its jurisdiction's authority issues
/// (`crate::fidelity_rules::number_of_type`), so the rows differ in jurisdiction
/// and in registration number as well as in account id. EVERY condition is
/// staged from the row's own values — a condition staged from fixed values could
/// not show that the message carries none of them.
#[derive(Debug, Clone, Copy)]
struct Inputs {
    registration_case: &'static str,
    supplemental_case: &'static str,
    account_id: &'static str,
    stored: (&'static str, &'static str),
    submitted: (&'static str, &'static str),
    supplemental: (&'static str, &'static str),
}

impl Inputs {
    /// How a case of `condition` reads: the values that condition is staged
    /// from, so a failure names the request that met it.
    fn described(self, condition: Condition) -> &'static str {
        match condition {
            Condition::PendingOwnRegistration | Condition::NoRoomForANewRegistration => {
                self.registration_case
            }
            Condition::NothingToSupplement | Condition::PendingSupplementalRegistration => {
                self.supplemental_case
            }
        }
    }

    /// The supplemental entry this row submits, held in the one jurisdiction
    /// this service registers supplemental TRNs in.
    fn supplemental_entry(self) -> Value {
        entry_for(supported_country(), self.supplemental.0, self.supplemental.1)
    }

    /// Every value the request or the store named, for a refusal of `condition`
    /// — none of which its message may carry.
    fn named_values(self, condition: Condition) -> Vec<String> {
        match condition {
            Condition::PendingOwnRegistration | Condition::NoRoomForANewRegistration => vec![
                self.account_id.to_string(),
                self.stored.1.to_string(),
                self.submitted.1.to_string(),
                number_of(self.stored),
                number_of(self.submitted),
                // The resident bound the padded store sits on.
                MAX_STORED_REGISTRATIONS.to_string(),
            ],
            Condition::NothingToSupplement | Condition::PendingSupplementalRegistration => vec![
                self.supplemental.0.to_string(),
                self.supplemental.1.to_string(),
                supported_country().to_string(),
                // The account a supplemental write is always about.
                engine::emulator_account_id().to_string(),
            ],
        }
    }
}

/// The inputs every condition below is staged over.
///
/// The jurisdictions of the tax-registration rows are ones this service states
/// no additional-information requirement for (`crate::state::COUNTRY_RULES`),
/// because what each row varies is the values the refusal must NOT carry — not
/// whether the entry clears validation.
const INPUTS: &[Inputs] = &[
    Inputs {
        registration_case: "a German VAT registration rewritten by a Dutch one",
        supplemental_case: "a supplemental VAT registration of Acme",
        account_id: "000000000001",
        stored: ("VAT", "DE"),
        submitted: ("VAT", "NL"),
        supplemental: ("CHE-123.456.789", "Acme AG"),
    },
    Inputs {
        registration_case: "an Irish VAT registration rewritten by itself",
        supplemental_case: "a supplemental VAT registration at the 1-character bound",
        account_id: "000000000002",
        stored: ("VAT", "IE"),
        submitted: ("VAT", "IE"),
        supplemental: ("1", "B"),
    },
];

/// The entry `pair` names: an entry of that registration type, held in that
/// jurisdiction.
fn entry(pair: (&str, &str)) -> Value {
    entry_of(pair.0, Some(pair.1))
}

/// The `registrationId` the entry `pair` names carries.
fn number_of(pair: (&str, &str)) -> String {
    entry(pair)["registrationId"]
        .as_str()
        .expect("the builder names the number")
        .to_string()
}

/// The `(errorCode, message)` the refusal in `answer` carries, whichever channel
/// `operation` renders a conflict on: the body of a 409 response, or the single
/// row of a batch answer's `errors` list.
fn refusal_envelope(case: &str, operation: &str, answer: &Answer) -> (String, String) {
    if operation == BATCH_WRITER {
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the batch call itself succeeds, got {answer:?}"
        );
        let rows = error_rows(answer);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: expected exactly one refused account, got {rows:?}"
        );
        return (rows[0].1.clone(), rows[0].2.clone());
    }
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (409, "ConflictException"),
        "case {case:?}: the call must be refused with the conflict class, got {answer:?}"
    );
    let member = |name: &str| {
        answer
            .get(name)
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a {name} on the refusal, got {:?}", answer.body)
            })
            .to_string()
    };
    (member("errorCode"), member("message"))
}

/// Drive `operation` into `condition` with `inputs`, and answer the refusal's
/// `(errorCode, message)`.
async fn refuse(
    case: &str,
    condition: Condition,
    operation: &str,
    inputs: Inputs,
) -> (String, String) {
    match condition {
        Condition::PendingOwnRegistration => {
            let (router, engine) = emulator_with_engine();
            let setup = write(&router, operation, inputs.account_id, &entry(inputs.stored)).await;
            assert_eq!(
                (setup.status, setup.get("status").as_str()),
                (200, Some(STATUS_PENDING)),
                "case {case:?}: the setup write must store an outstanding TRN, got {setup:?}"
            );
            // The record stays the outstanding one however long the test takes
            // to reach the next call.
            hold_open(&engine, inputs.account_id);
            let answer =
                write(&router, operation, inputs.account_id, &entry(inputs.submitted)).await;
            refusal_envelope(case, operation, &answer)
        }
        Condition::NoRoomForANewRegistration => {
            let (router, engine) = emulator_with_engine();
            // The padding is named apart from the account this write addresses,
            // so the write is one that would ADD a record.
            fill_the_store(&engine);
            let answer =
                write(&router, operation, inputs.account_id, &entry(inputs.submitted)).await;
            refusal_envelope(case, operation, &answer)
        }
        Condition::NothingToSupplement => {
            // The account holds no registration of its own, so there is nothing
            // for this write to supplement.
            let (router, _engine) = emulator_with_engine();
            let answer = put_supplemental(&router, &inputs.supplemental_entry()).await;
            refusal_envelope(case, operation, &answer)
        }
        Condition::PendingSupplementalRegistration => {
            let (router, _engine) = emulator_supplementing();
            let supplemental = inputs.supplemental_entry();
            let first = put_supplemental(&router, &supplemental).await;
            assert_eq!(
                (first.status, first.get("status").as_str()),
                (200, Some(STATUS_PENDING)),
                "case {case:?}: the first supplemental write must store an outstanding record, \
                 got {first:?}"
            );
            let answer = put_supplemental(&router, &supplemental).await;
            refusal_envelope(case, operation, &answer)
        }
    }
}

/// Every condition this service refuses with the conflict class, paired with the
/// operations that refuse it: the TRN write conditions on both writers of a
/// `TaxRegistrationEntry`, the supplemental ones on the supplemental write.
fn staged() -> Vec<(Condition, &'static str)> {
    let mut staged = Vec::new();
    for operation in WRITERS {
        staged.push((Condition::PendingOwnRegistration, *operation));
        staged.push((Condition::NoRoomForANewRegistration, *operation));
    }
    staged.push((Condition::NothingToSupplement, SUPPLEMENTAL_WRITER));
    staged.push((
        Condition::PendingSupplementalRegistration,
        SUPPLEMENTAL_WRITER,
    ));
    staged
}

/// RULE: a conflict carries the one code the class publishes and the fixed text
/// the service words for the condition it refused — the pending-TRN sentence for
/// a write blocked by the account's own outstanding TRN, the general sentence for
/// a conflict the service words none of its own for.
#[tokio::test]
async fn a_conflict_carries_the_message_the_service_words_for_the_state_it_conflicted_with() {
    for (condition, operation) in staged() {
        for inputs in INPUTS.iter().copied() {
            let case = format!(
                "{condition:?} refused by {operation} with {}",
                inputs.described(condition)
            );
            let (code, message) = refuse(&case, condition, operation, inputs).await;
            assert_eq!(
                code, ERROR_CODE_GENERAL_CONFLICT,
                "case {case:?}: expected the one code the conflict class publishes, got {code:?}"
            );
            assert_eq!(
                message,
                condition.expected_message(),
                "case {case:?}: expected the message this service words for that state, got \
                 {message:?}"
            );
        }
    }
}

/// RULE (the fixed-text half of it): a conflict's message interpolates nothing —
/// the same condition reads the same, byte for byte, whatever account, number or
/// jurisdiction the refused request named, and names none of them.
#[tokio::test]
async fn a_conflicts_message_carries_no_value_of_the_request_or_the_store() {
    for (condition, operation) in staged() {
        let mut seen: Vec<String> = Vec::new();
        for inputs in INPUTS.iter().copied() {
            let case = format!(
                "{condition:?} refused by {operation} with {}",
                inputs.described(condition)
            );
            let (_, message) = refuse(&case, condition, operation, inputs).await;
            // Nothing the request or the store named may appear in the text —
            // the values THIS condition was staged from.
            for value in inputs.named_values(condition) {
                assert!(
                    !message.contains(&value),
                    "case {case:?}: the message must carry no value of the request or the store, \
                     but {message:?} carries {value:?}"
                );
            }
            seen.push(message);
        }
        assert!(
            seen.windows(2).all(|pair| pair[0] == pair[1]),
            "case {condition:?} refused by {operation}: the message is fixed text, so every \
             input must read the same, got {seen:?}"
        );
    }
}

/// RULE: the sentence belongs to the STATE, not to the operation — the account
/// whose TRN is outstanding is refused with the pending-TRN sentence on the
/// single-account channel and on the batch's per-account row alike, and the
/// accounts named beside it in one batch are still written.
#[tokio::test]
async fn the_pending_sentence_reaches_a_row_beside_accounts_that_are_written() {
    let outstanding = "000000000001";
    let written = ["000000000002", "000000000003"];
    let (router, engine) = emulator_with_engine();

    let setup = write(&router, "PutTaxRegistration", outstanding, &entry(("VAT", "DE"))).await;
    assert_eq!(
        setup.status, 200,
        "the setup write must succeed, got {setup:?}"
    );
    hold_open(&engine, outstanding);

    let submitted = entry(("VAT", "NL"));
    let mut named = vec![outstanding];
    named.extend_from_slice(&written);
    let answer = call(
        &router,
        BATCH_WRITER,
        &json!({ "accountIds": named, "taxRegistrationEntry": submitted }),
    )
    .await;
    let rows = error_rows(&answer);
    assert_eq!(
        rows,
        vec![(
            outstanding.to_string(),
            ERROR_CODE_GENERAL_CONFLICT.to_string(),
            PENDING_REGISTRATION_CONFLICT_MESSAGE.to_string(),
        )],
        "exactly the outstanding account is refused, with the pending-TRN sentence, got {rows:?}"
    );
    for account_id in written {
        let held = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            (
                held.status,
                held.body
                    .pointer("/taxRegistration/registrationId")
                    .and_then(Value::as_str),
            ),
            (
                200,
                submitted["registrationId"].as_str(),
            ),
            "account {account_id} named beside the refused one must still be written, got {held:?}"
        );
    }
}
