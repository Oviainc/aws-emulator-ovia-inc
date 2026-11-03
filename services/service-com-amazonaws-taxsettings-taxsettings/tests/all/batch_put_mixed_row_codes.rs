// SPDX-License-Identifier: CC0-1.0

//! What ONE `BatchPutTaxRegistration` call reports as the `code` of each row of
//! its `errors` list, when the accounts it names are refused by DIFFERENT
//! branches of its per-account loop.
//!
//! RULE: a per-account row carries the SERVICE-DEFINED code the refusal it
//! reports publishes — the discriminator that refusal's modelled exception
//! carries in `errorCode` — and carries no `code` member at all for a refusal
//! class this service publishes no code for, whichever branch of the loop built
//! the row and whatever the same call did for the accounts named beside it.
//! Never an exception's own shape name: the wire already carries that in
//! `__type` / `x-amzn-errortype`, and a client reading `code` to classify a
//! failure would take a shape name for a service code.
//! `BatchPutTaxRegistrationError$code` targets the model's free-form `ErrorCode`
//! and is not `@required` (`smithy_query(query="shape",
//! shape="BatchPutTaxRegistrationError")`), so a class with no code to report
//! leaves the member off the wire rather than filling it with something
//! invented.
//!
//! `tests/all/batch_row_error_codes.rs` asserts the rule one refusal class at a
//! time; this suite asserts it across the branches of ONE response. Each row of
//! the table below is one call, and states per account what the call must do —
//! taken from the rule, not computed from the handler. They span the mixes the
//! rule generalises over: which branch refused each account (the write's
//! validation refusal, which publishes a code; the pending-TRN conflict and the
//! resident-bound conflict, which publish none), how many accounts each branch
//! refused, their position in the request, whether the call stored anything at
//! all, and whether the submitted type is one any stored type blocks.
//!
//! The harness (`emulator_with_engine`, `call`) lives in
//! `tax_registration_lifecycle.rs`, the entry builder in
//! `registration_type_refusals.rs`, the per-account record staging in
//! `pending_trn_conflict.rs` and the row reader and store-filling in
//! `batch_row_error_codes.rs`; nothing here builds a second copy of any of them.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ERROR_CODE_GENERAL_CONFLICT, STATUS_PENDING, STATUS_VERIFIED, TYPES_BLOCKED_BY_A_STORED_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::batch_row_error_codes::{fill_the_store, rows_of, submitted_jurisdiction};
use crate::pending_trn_conflict::{force_status, hold_open};
use crate::registration_type_refusals::{entry_of, VALIDATION_EXCEPTION_ERROR_CODES};
use crate::tax_registration_lifecycle::{call, emulator_with_engine};

/// The state ONE account the call names is in before the call.
#[derive(Clone, Copy, Debug)]
enum World {
    /// The account holds no tax registration at all.
    HoldsNothing,
    /// The account holds a registration of this type whose TRN validation has
    /// already run, so a write may replace it.
    Settled(&'static str),
    /// The account holds a registration of this type whose TRN validation is
    /// still outstanding, so a write of it is refused.
    Outstanding(&'static str),
}

impl World {
    /// The registration type the account already holds, or `None` when it holds
    /// no record.
    const fn stored_type(self) -> Option<&'static str> {
        match self {
            Self::HoldsNothing => None,
            Self::Settled(registration_type) | Self::Outstanding(registration_type) => {
                Some(registration_type)
            }
        }
    }
}

/// What the one call must do for one of the accounts it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// The submitted entry becomes the account's registration.
    Stored,
    /// The account is reported in a row of `errors` carrying this `code`, where
    /// `None` means the member is left off the wire.
    Refused(Option<&'static str>),
}

/// One account the call names: the state it is in, and what the call must do for
/// it.
struct Account {
    id: &'static str,
    world: World,
    expected: Expected,
}

/// One `BatchPutTaxRegistration` call.
struct Case {
    /// How the case reads in an assertion.
    name: &'static str,
    /// The `registrationType` of the one entry the call submits for every
    /// account it names.
    submitted_type: &'static str,
    /// Whether the store already holds as many registrations as the emulator
    /// keeps resident, so a write that would ADD one has no room for it.
    store_at_its_bound: bool,
    /// The accounts the call names, in request order.
    accounts: &'static [Account],
}

/// The accounts the cases address. Which account holds which record is not a
/// dimension of this rule; the last one is only ever named while the store is at
/// its bound, so its write is the one with no room.
const FIRST_ACCOUNT: &str = "000000000001";
const SECOND_ACCOUNT: &str = "000000000002";
const THIRD_ACCOUNT: &str = "000000000003";
const NEWCOMER_ACCOUNT: &str = "999999999999";

/// The submitted registration type a stored type blocks, the stored type that
/// blocks it, and the code the refusal publishes: the model's
/// `ValidationExceptionErrorCode` value `GSTExistenceBlockSetPAN`
/// (`smithy_query(query="shape", shape="ValidationExceptionErrorCode")`), which
/// says the existence of a stored goods-and-services-tax registration blocks
/// setting the Permanent Account Number type for that account.
const BLOCKED_TYPE: &str = "PAN";
/// See [`BLOCKED_TYPE`]: the stored type that blocks it.
const BLOCKING_TYPE: &str = "GST";
/// See [`BLOCKED_TYPE`]: the published code of that refusal.
const BLOCKED_PAIR_CODE: &str = "GSTExistenceBlockSetPAN";

/// A registration type no stored type blocks, for a call whose refusals are all
/// conflicts.
const UNBLOCKED_TYPE: &str = "TIN";
/// A second unblocked type, so a record held before the call can differ from the
/// one the call submits.
const OTHER_TYPE: &str = "VAT";

/// The calls the rule is asserted over.
const CASES: &[Case] = &[
    Case {
        name: "a blocked stored type beside an account holding nothing",
        submitted_type: BLOCKED_TYPE,
        store_at_its_bound: false,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Settled(BLOCKING_TYPE),
                expected: Expected::Refused(Some(BLOCKED_PAIR_CODE)),
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Stored,
            },
        ],
    },
    Case {
        name: "a blocked stored type, an outstanding TRN and an account holding nothing",
        submitted_type: BLOCKED_TYPE,
        store_at_its_bound: false,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Settled(BLOCKING_TYPE),
                expected: Expected::Refused(Some(BLOCKED_PAIR_CODE)),
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::Outstanding(UNBLOCKED_TYPE),
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
            Account {
                id: THIRD_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Stored,
            },
        ],
    },
    Case {
        name: "the stored account named after the two the call refused",
        submitted_type: BLOCKED_TYPE,
        store_at_its_bound: false,
        accounts: &[
            Account {
                id: THIRD_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Stored,
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::Outstanding(OTHER_TYPE),
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
            Account {
                id: FIRST_ACCOUNT,
                world: World::Settled(BLOCKING_TYPE),
                expected: Expected::Refused(Some(BLOCKED_PAIR_CODE)),
            },
        ],
    },
    Case {
        name: "two outstanding TRNs and an account holding nothing",
        submitted_type: BLOCKED_TYPE,
        store_at_its_bound: false,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Outstanding(OTHER_TYPE),
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::Outstanding(UNBLOCKED_TYPE),
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
            Account {
                id: THIRD_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Stored,
            },
        ],
    },
    Case {
        name: "every account the call names refused, one per branch",
        submitted_type: BLOCKED_TYPE,
        store_at_its_bound: false,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Settled(BLOCKING_TYPE),
                expected: Expected::Refused(Some(BLOCKED_PAIR_CODE)),
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::Outstanding(UNBLOCKED_TYPE),
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
        ],
    },
    Case {
        // The account's STORED TYPE is judged against the entry before the write
        // is attempted, so the caller is told which type blocks the write rather
        // than that the record is outstanding: a published code, not the
        // conflict's absent one.
        name: "a blocked stored type whose own TRN is still outstanding",
        submitted_type: BLOCKED_TYPE,
        store_at_its_bound: false,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Outstanding(BLOCKING_TYPE),
                expected: Expected::Refused(Some(BLOCKED_PAIR_CODE)),
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Stored,
            },
        ],
    },
    Case {
        // A write that REWRITES a record adds nothing, so the bound refuses only
        // the account the store holds no record for.
        name: "a store at its resident bound, beside an account it can rewrite",
        submitted_type: UNBLOCKED_TYPE,
        store_at_its_bound: true,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Settled(UNBLOCKED_TYPE),
                expected: Expected::Stored,
            },
            Account {
                id: NEWCOMER_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
        ],
    },
    Case {
        name: "a store at its bound, with a blocked stored type beside it",
        submitted_type: BLOCKED_TYPE,
        store_at_its_bound: true,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Settled(BLOCKING_TYPE),
                expected: Expected::Refused(Some(BLOCKED_PAIR_CODE)),
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::Settled(OTHER_TYPE),
                expected: Expected::Stored,
            },
            Account {
                id: NEWCOMER_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
        ],
    },
    Case {
        name: "a submitted type no stored type blocks",
        submitted_type: UNBLOCKED_TYPE,
        store_at_its_bound: false,
        accounts: &[
            Account {
                id: FIRST_ACCOUNT,
                world: World::Settled(BLOCKING_TYPE),
                expected: Expected::Stored,
            },
            Account {
                id: SECOND_ACCOUNT,
                world: World::Outstanding(OTHER_TYPE),
                expected: Expected::Refused(Some(ERROR_CODE_GENERAL_CONFLICT)),
            },
            Account {
                id: THIRD_ACCOUNT,
                world: World::HoldsNothing,
                expected: Expected::Stored,
            },
        ],
    },
];

/// The entry an account is written with, or the call submits: the shared
/// builder's entry of `registration_type`, held in the jurisdiction that type is
/// submitted for.
fn entry_for(registration_type: &str) -> Value {
    entry_of(registration_type, submitted_jurisdiction(registration_type))
}

/// The `registrationId` of that entry — what a read of the record must report.
fn registration_id_of(registration_type: &str) -> String {
    entry_for(registration_type)
        .get("registrationId")
        .and_then(Value::as_str)
        .expect("the shared builder sets a registrationId")
        .to_string()
}

/// Put every account of `case` in the state the case names, and answer the
/// emulator serving it.
async fn stage(case: &Case) -> MultiServiceRouter {
    let (router, engine) = emulator_with_engine();
    for account in case.accounts {
        let Some(stored_type) = account.world.stored_type() else {
            continue;
        };
        let written = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": account.id, "taxRegistrationEntry": entry_for(stored_type) }),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {:?}: the setup write of {} must succeed, got {written:?}",
            case.name, account.id
        );
        match account.world {
            // One account outstanding beside another already settled is a state
            // the whole-store clock helpers cannot build, which is why the
            // per-account ones exist.
            World::Outstanding(_) => hold_open(&engine, account.id),
            _ => force_status(&engine, account.id, STATUS_VERIFIED),
        }
    }
    if case.store_at_its_bound {
        fill_the_store(&engine);
    }
    router
}

/// The one call the case makes: every account it names, in request order, and
/// the one entry it submits for all of them.
fn batch_request(case: &Case) -> Value {
    let account_ids: Vec<&str> = case.accounts.iter().map(|account| account.id).collect();
    json!({
        "accountIds": account_ids,
        "taxRegistrationEntry": entry_for(case.submitted_type),
    })
}

/// The `(registrationType, registrationId)` of the record `account_id` holds, or
/// `None` when this service holds none for it.
async fn stored_record(router: &MultiServiceRouter, account_id: &str) -> Option<(String, String)> {
    let answer = call(
        router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    if answer.status != 200 {
        return None;
    }
    let member = |name: &str| {
        answer
            .body
            .pointer(&format!("/taxRegistration/{name}"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Some((member("registrationType"), member("registrationId")))
}

/// RULE: in one call, each refused account's row carries the service-defined
/// code of the refusal that row reports — and no `code` member at all for a
/// refusal class this service publishes none for — whichever branch of the loop
/// refused it and whatever the call did for the accounts beside it.
#[tokio::test]
async fn one_call_reports_each_refusal_with_the_code_that_refusal_publishes() {
    for case in CASES {
        let case_name = case.name;
        let router = stage(case).await;
        let answer = call(&router, "BatchPutTaxRegistration", &batch_request(case)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (200, ""),
            "case {case_name:?}: the batch call itself must succeed, got {answer:?}"
        );

        let rows = rows_of(&answer);
        let reported: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|(account_id, code, _)| (account_id.as_str(), code.as_deref()))
            .collect();
        let wanted: Vec<(&str, Option<&str>)> = case
            .accounts
            .iter()
            .filter_map(|account| match account.expected {
                Expected::Refused(code) => Some((account.id, code)),
                Expected::Stored => None,
            })
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case_name:?}: expected the rows {wanted:?}, got {rows:?}"
        );
        for (account_id, code, message) in &rows {
            assert!(
                !message.is_empty(),
                "case {case_name:?}: the row for {account_id} must say why, got {rows:?}"
            );
            // A code a row does carry is one of the codes this service
            // publishes — a value of the model's `ValidationExceptionErrorCode`
            // enum, or the one code the conflict class publishes — so it can
            // never be read as an exception shape name.
            if let Some(code) = code {
                assert!(
                    VALIDATION_EXCEPTION_ERROR_CODES.contains(&code.as_str())
                        || code == ERROR_CODE_GENERAL_CONFLICT,
                    "case {case_name:?}: {code:?} is not a code this service publishes, \
                     got {rows:?}"
                );
                assert!(
                    !code.ends_with("Exception"),
                    "case {case_name:?}: the row must not carry an exception shape name, \
                     got {code:?}"
                );
                assert!(
                    code.len() <= 50,
                    "case {case_name:?}: the code must fit the modelled length 0..50, \
                     got {code:?}"
                );
            }
        }

        // `status` is the status the registrations were stored with, reported
        // only when the call stored at least one.
        let stored_any = case
            .accounts
            .iter()
            .any(|account| account.expected == Expected::Stored);
        let status = answer.get("status").as_str().map(str::to_string);
        if stored_any {
            assert_eq!(
                status.as_deref(),
                Some(STATUS_PENDING),
                "case {case_name:?}: expected the stored status {STATUS_PENDING}, got {:?}",
                answer.body
            );
        } else {
            assert!(
                status.is_none(),
                "case {case_name:?}: a call that stored nothing has no stored status, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE (the same call's writes): a row reports a refusal rather than a write,
/// so every account the call refused keeps the record it had — and every account
/// it did not refuse holds the submitted entry, read back through
/// `GetTaxRegistration`.
#[tokio::test]
async fn a_refused_account_keeps_its_record_while_the_rest_of_the_call_is_stored() {
    for case in CASES {
        let case_name = case.name;
        let router = stage(case).await;
        let answer = call(&router, "BatchPutTaxRegistration", &batch_request(case)).await;
        assert_eq!(
            answer.status, 200,
            "case {case_name:?}: the batch call itself must succeed, got {answer:?}"
        );

        for account in case.accounts {
            let wanted = match account.expected {
                Expected::Stored => Some((
                    case.submitted_type.to_string(),
                    registration_id_of(case.submitted_type),
                )),
                Expected::Refused(_) => account.world.stored_type().map(|stored_type| {
                    (stored_type.to_string(), registration_id_of(stored_type))
                }),
            };
            let held = stored_record(&router, account.id).await;
            assert_eq!(
                held, wanted,
                "case {case_name:?}: {} was {:?}, so it must hold {wanted:?}, got {held:?}",
                account.id, account.expected
            );
        }
    }
}

/// The codes a row can carry are the ones this service publishes for a blocked
/// (stored type, submitted type) pair, so every pair in the catalog it publishes
/// them from must be exercised by a row of the table above: a pair added there
/// gets a case here rather than silently going untested, and the code this suite
/// states for a pair is the one the service publishes for it.
#[test]
fn every_published_blocked_pair_is_covered_by_a_case() {
    for &(stored_type, submitted_type, code) in TYPES_BLOCKED_BY_A_STORED_TYPE {
        let covered = CASES.iter().any(|case| {
            case.submitted_type == submitted_type
                && case.accounts.iter().any(|account| {
                    account.world.stored_type() == Some(stored_type)
                        && account.expected == Expected::Refused(Some(code))
                })
        });
        assert!(
            covered,
            "the published pair (stored {stored_type}, submitted {submitted_type}) and its \
             code {code:?} are exercised by no case of this suite"
        );
    }
}
