// SPDX-License-Identifier: CC0-1.0

//! What a batch operation's per-account `errors[]` row reports as its `code`,
//! end to end over the restJson1 wire.
//!
//! RULE: a per-account row carries the SERVICE-DEFINED code the refusal it
//! reports publishes — the same discriminator that refusal's modelled exception
//! carries in `errorCode` — and carries no `code` at all for a refusal class this
//! service publishes no code for. Never an exception's own shape name: the wire
//! already carries that in `__type` / `x-amzn-errortype`, and a client reading
//! `code` to classify a failure would take a shape name for a service code.
//! `BatchDeleteTaxRegistrationError$code` and `BatchPutTaxRegistrationError$code`
//! both target the model's free-form `ErrorCode`, the very shape the exceptions
//! target their `errorCode` at, and neither is `@required`
//! (`smithy_query(query="shape", shape="BatchDeleteTaxRegistrationError")`), so
//! the member is left off the wire rather than filled with something invented.
//!
//! The rows span the refusal classes the two batch operations can report against
//! ONE account — a missing registration, a jurisdiction the delete API carves
//! out, a stored registration type that blocks the submitted one, an outstanding
//! TRN and a store with no room — read from the catalogs the service publishes
//! them from, so a jurisdiction or a blocked type pair added to a catalog is
//! covered here. Both of the service's published code classes are covered (a
//! field-validation refusal and the type-existence one), as is each class that
//! publishes none.
//!
//! The harness (`emulator_with_engine`, `call`, `hold_validations_open`,
//! `make_validations_due`) lives in `tax_registration_lifecycle.rs` and the entry
//! builder in `registration_type_refusals.rs`; nothing here builds a second copy
//! of either.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    type_jurisdiction, type_with_derived_legal_address, ComAmazonawsTaxsettingsTaxsettingsState,
    StoredTaxRegistration, COUNTRIES_WITH_NON_API_REGISTRATION_DELETE,
    ERROR_CODE_FIELD_VALIDATION_FAILED, ERROR_CODE_GENERAL_CONFLICT, MAX_STORED_REGISTRATIONS,
    TYPES_BLOCKED_BY_A_STORED_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::registration_type_refusals::{entry_of, VALIDATION_EXCEPTION_ERROR_CODES};
use crate::tax_registration_lifecycle::{
    call, emulator_with_engine, hold_validations_open, make_validations_due, Answer,
};

/// The account every refusal below is aimed at. Which account holds the record
/// is not a dimension of this rule.
const ACCOUNT_ID: &str = "000000000001";

/// The account the store-at-its-bound row addresses: one the store does not
/// already hold, so the write needs room it has not got.
const NEWCOMER_ACCOUNT_ID: &str = "999999999999";

/// A jurisdiction no per-type rule of this service turns on, for an entry whose
/// registration type the model ties to no single country.
const ANY_JURISDICTION: &str = "DE";

/// The registration type a carved-out jurisdiction's record is stored under: one
/// the model ties to no single country (`crate::state::TYPE_JURISDICTIONS`), so
/// it can be held in whichever jurisdiction the carve-out catalog names.
const PORTABLE_TYPE: &str = "TIN";

/// One per-account refusal a batch operation reports: the world to set up, and
/// what the request aimed at that account then asks for.
#[derive(Clone, Copy, Debug)]
enum Refusal {
    /// The account holds no registration at all — the missing-resource class of
    /// the delete decision.
    HoldsNothing,
    /// The account's record is held in a jurisdiction this API cannot remove one
    /// from, named by its `Address$countryCode` — the delete decision's
    /// validation refusal.
    CarvedOutJurisdiction(&'static str),
    /// The account's stored registration type blocks the submitted one — the
    /// write's validation refusal, with the code the service publishes for the
    /// pair.
    BlockedRegistrationType {
        stored: &'static str,
        submitted: &'static str,
        code: &'static str,
    },
    /// The account's stored TRN is still outstanding — the write's conflict.
    OutstandingTrn,
    /// The store holds as many registrations as it can, so a new record has no
    /// room — the write's other conflict.
    NoRoomForANewRecord,
}

impl Refusal {
    /// The service-defined code the row must carry, or `None` for a class this
    /// service publishes no code for: it publishes none for a missing resource,
    /// which is why that class's exception carries no `errorCode` on the wire
    /// either, and one code for the whole conflict class.
    const fn code(self) -> Option<&'static str> {
        match self {
            Self::CarvedOutJurisdiction(_) => Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
            Self::BlockedRegistrationType { code, .. } => Some(code),
            Self::OutstandingTrn | Self::NoRoomForANewRecord => {
                Some(ERROR_CODE_GENERAL_CONFLICT)
            }
            Self::HoldsNothing => None,
        }
    }

    /// The account the refusal is reported against.
    const fn account_id(self) -> &'static str {
        match self {
            Self::NoRoomForANewRecord => NEWCOMER_ACCOUNT_ID,
            _ => ACCOUNT_ID,
        }
    }

    /// The batch operation that reports the refusal in a row of `errors`, and
    /// the single-account operation that renders the same refusal as its
    /// modelled error.
    const fn operations(self) -> (&'static str, &'static str) {
        match self {
            Self::HoldsNothing | Self::CarvedOutJurisdiction(_) => {
                ("BatchDeleteTaxRegistration", "DeleteTaxRegistration")
            }
            _ => ("BatchPutTaxRegistration", "PutTaxRegistration"),
        }
    }

    /// The entry the write submits, for a refusal a write meets.
    fn submitted_entry(self) -> Option<Value> {
        match self {
            Self::HoldsNothing | Self::CarvedOutJurisdiction(_) => None,
            Self::BlockedRegistrationType { submitted, .. } => {
                Some(entry_of(submitted, submitted_jurisdiction(submitted)))
            }
            Self::OutstandingTrn | Self::NoRoomForANewRecord => Some(entry_of(
                PORTABLE_TYPE,
                submitted_jurisdiction(PORTABLE_TYPE),
            )),
        }
    }

    /// Set up the world this refusal is met in, and answer the emulator serving
    /// it.
    async fn stage(self, case: &str) -> MultiServiceRouter {
        let (router, engine) = emulator_with_engine();
        match self {
            // Nothing stored: the account holds no record to delete.
            Self::HoldsNothing => {}
            Self::CarvedOutJurisdiction(country_code) => {
                store(
                    &router,
                    case,
                    ACCOUNT_ID,
                    &entry_of(PORTABLE_TYPE, Some(country_code)),
                )
                .await;
            }
            Self::BlockedRegistrationType { stored, .. } => {
                store(
                    &router,
                    case,
                    ACCOUNT_ID,
                    &entry_of(stored, submitted_jurisdiction(stored)),
                )
                .await;
                // Let the stored record's validation fall due, so the write
                // meets the type refusal rather than the pending-TRN conflict.
                make_validations_due(&engine);
            }
            Self::OutstandingTrn => {
                store(
                    &router,
                    case,
                    ACCOUNT_ID,
                    &entry_of(PORTABLE_TYPE, submitted_jurisdiction(PORTABLE_TYPE)),
                )
                .await;
                hold_validations_open(&engine);
            }
            Self::NoRoomForANewRecord => fill_the_store(&engine),
        }
        router
    }

    /// The batch request that meets this refusal, naming the one account.
    fn batch_request(self) -> Value {
        let mut request = json!({ "accountIds": [self.account_id()] });
        if let Some(entry) = self.submitted_entry() {
            request
                .as_object_mut()
                .expect("an object")
                .insert("taxRegistrationEntry".to_string(), entry);
        }
        request
    }

    /// The single-account request that meets the same refusal.
    fn single_account_request(self) -> Value {
        let mut request = json!({ "accountId": self.account_id() });
        if let Some(entry) = self.submitted_entry() {
            request
                .as_object_mut()
                .expect("an object")
                .insert("taxRegistrationEntry".to_string(), entry);
        }
        request
    }
}

/// The jurisdiction an entry of `registration_type` is submitted for: none at
/// all for a type whose legal address AWS derives from the tax number (an entry
/// of one may submit no address), the one country the model ties the type to
/// when it ties it to one, and otherwise a country no per-type rule turns on.
///
/// `pub(crate)` because `tests/all/batch_put_mixed_row_codes.rs` submits the
/// same entries: one jurisdiction per type, so the two suites cannot submit one
/// type two different ways.
pub(crate) fn submitted_jurisdiction(registration_type: &str) -> Option<&'static str> {
    if type_with_derived_legal_address(registration_type).is_some() {
        return None;
    }
    Some(
        type_jurisdiction(registration_type)
            .map_or(ANY_JURISDICTION, |(country_code, _)| country_code),
    )
}

/// Store `entry` under `account_id` through the single-account write.
async fn store(router: &MultiServiceRouter, case: &str, account_id: &str, entry: &Value) {
    let written = call(
        router,
        "PutTaxRegistration",
        &json!({ "accountId": account_id, "taxRegistrationEntry": entry }),
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case {case:?}: the setup write must succeed, got {written:?}"
    );
}

/// Fill the store to the last registration it keeps resident, so the next new
/// record has no room.
///
/// `pub(crate)` because `tests/all/batch_put_mixed_row_codes.rs` meets the same
/// bound: the records it pads with are named apart from any account a test
/// addresses, so an account that already holds one still holds it.
pub(crate) fn fill_the_store(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    for index in 0..MAX_STORED_REGISTRATIONS {
        store.registrations.insert(
            format!("{:012}", index + 100_000),
            StoredTaxRegistration::default(),
        );
    }
}

/// The `(accountId, code, message)` of every row of a batch answer's `errors`
/// list, with an ABSENT `code` kept as `None`.
///
/// `pending_trn_conflict::error_rows` flattens an absent member to the empty
/// string, which is the one distinction this rule turns on, so this suite reads
/// the list itself.
///
/// `pub(crate)` because `tests/all/batch_put_mixed_row_codes.rs` turns on the
/// same distinction: one reader, so the two suites cannot read an absent `code`
/// two different ways.
pub(crate) fn rows_of(answer: &Answer) -> Vec<(String, Option<String>, String)> {
    answer
        .get("errors")
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    let member = |name: &str| {
                        row.get(name)
                            .and_then(Value::as_str)
                            .map(std::string::ToString::to_string)
                    };
                    (
                        member("accountId").unwrap_or_default(),
                        member("code"),
                        member("message").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every refusal class a batch operation reports against one account, read from
/// the catalogs this service publishes them from.
fn refusals() -> Vec<(String, Refusal)> {
    let mut rows = vec![
        (
            "an account holding no registration".to_string(),
            Refusal::HoldsNothing,
        ),
        (
            "an account whose stored TRN is still outstanding".to_string(),
            Refusal::OutstandingTrn,
        ),
        (
            "a new record the store has no room for".to_string(),
            Refusal::NoRoomForANewRecord,
        ),
    ];
    // Every jurisdiction the delete carve-out names
    // (`crate::state::COUNTRIES_WITH_NON_API_REGISTRATION_DELETE`).
    for (country_code, country) in COUNTRIES_WITH_NON_API_REGISTRATION_DELETE {
        rows.push((
            format!("a record held in {country}"),
            Refusal::CarvedOutJurisdiction(country_code),
        ));
    }
    // Every (stored type, submitted type) pair the service publishes a code for
    // (`crate::state::TYPES_BLOCKED_BY_A_STORED_TYPE`).
    for (stored, submitted, code) in TYPES_BLOCKED_BY_A_STORED_TYPE {
        rows.push((
            format!("a {submitted} write over a stored {stored}"),
            Refusal::BlockedRegistrationType {
                stored,
                submitted,
                code,
            },
        ));
    }
    rows
}

/// RULE: the row a batch operation reports a per-account refusal in carries the
/// service-defined code that refusal publishes, and no `code` member at all for
/// a refusal class this service publishes no code for — never an exception shape
/// name.
#[tokio::test]
async fn a_batch_row_carries_the_service_code_of_the_refusal_it_reports() {
    for (case, refusal) in refusals() {
        let router = refusal.stage(&case).await;
        let (batch, _) = refusal.operations();
        let answer = call(&router, batch, &refusal.batch_request()).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case:?}: the batch call itself must succeed, got {answer:?}"
        );

        let rows = rows_of(&answer);
        let expected = refusal.code().map(std::string::ToString::to_string);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: exactly the one refused account must be reported, got {rows:?}"
        );
        let (account_id, code, message) = &rows[0];
        assert_eq!(
            account_id,
            refusal.account_id(),
            "case {case:?}: the row must name the account it refused, got {rows:?}"
        );
        assert_eq!(
            code, &expected,
            "case {case:?}: expected the code {expected:?}, got {code:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: the row must say why, got {rows:?}"
        );
        // Whatever the class, a code the row does carry is one of the codes this
        // service publishes — a value of the model's `ValidationExceptionErrorCode`
        // enum, or the one code the conflict class publishes — so it can never be
        // read as a shape name.
        if let Some(code) = code {
            assert!(
                VALIDATION_EXCEPTION_ERROR_CODES.contains(&code.as_str())
                    || code == ERROR_CODE_GENERAL_CONFLICT,
                "case {case:?}: {code:?} is not a code this service publishes, got {rows:?}"
            );
            assert!(
                !code.ends_with("Exception"),
                "case {case:?}: the row must not carry an exception shape name, got {code:?}"
            );
            assert!(
                code.len() <= 50,
                "case {case:?}: the code must fit the modelled length 0..50, got {code:?}"
            );
        }
    }
}

/// RULE (the same code on both channels): the code a batch row carries is the
/// `errorCode` the single-account operation's modelled exception carries for the
/// same refusal — so one client classifying a failure reads one discriminator
/// whichever operation it called, and a class that publishes none reports none on
/// either channel.
#[tokio::test]
async fn a_batch_row_publishes_the_code_its_single_account_refusal_publishes() {
    for (case, refusal) in refusals() {
        let batch_code = {
            let router = refusal.stage(&case).await;
            let (batch, _) = refusal.operations();
            let answer = call(&router, batch, &refusal.batch_request()).await;
            let rows = rows_of(&answer);
            assert_eq!(
                rows.len(),
                1,
                "case {case:?}: exactly the one refused account must be reported, got {rows:?}"
            );
            rows[0].1.clone()
        };

        let router = refusal.stage(&case).await;
        let (_, single) = refusal.operations();
        let refused = call(&router, single, &refusal.single_account_request()).await;
        assert!(
            refused.status >= 400,
            "case {case:?}: the single-account call must be refused, got {refused:?}"
        );
        let single_code = refused
            .get("errorCode")
            .as_str()
            .map(std::string::ToString::to_string);
        assert_eq!(
            batch_code, single_code,
            "case {case:?}: the batch row's code {batch_code:?} must be the code the \
             single-account refusal publishes, got {single_code:?} in {:?}",
            refused.body
        );
        // The shape name the batch row must NOT carry is the one the wire's own
        // error-type channel already carries.
        assert!(
            !refused.error_code.is_empty(),
            "case {case:?}: the refusal must name its error type on the wire, got {refused:?}"
        );
        assert_ne!(
            batch_code.as_deref(),
            Some(refused.error_code.as_str()),
            "case {case:?}: the row must not repeat the exception's shape name, got {batch_code:?}"
        );
    }
}
