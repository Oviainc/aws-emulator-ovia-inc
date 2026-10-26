// SPDX-License-Identifier: CC0-1.0

//! The tax-inheritance election, over the restJson1 wire: what
//! `PutTaxInheritance` records and what `GetTaxInheritance` then reports.
//!
//! The table spans the class the rule generalises over — every `HeritageStatus`
//! value, in each order, repeated, and in sequences of several writes — rather
//! than the one request that happened to be recorded.
//!
//! The harness (`emulator_with_engine`, `mount_on`, `call`) lives in
//! `tax_registration_lifecycle.rs`; nothing here builds a second one.

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    DEFAULT_HERITAGE_STATUS, HERITAGE_STATUS_OPT_IN, HERITAGE_STATUS_OPT_OUT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_registration_lifecycle::{call, emulator, emulator_with_engine, mount_on};

/// Every value of the model's `HeritageStatus` enum
/// (`smithy_query(query="shape", shape="HeritageStatus")`: `OptIn | OptOut`).
const HERITAGE_STATUSES: &[&str] = &[HERITAGE_STATUS_OPT_IN, HERITAGE_STATUS_OPT_OUT];

/// One sequence of elections to record, in order.
struct Sequence {
    case: &'static str,
    submitted: &'static [&'static str],
}

/// Record `election` and assert the write was served with the modelled empty
/// success.
async fn put_election(router: &MultiServiceRouter, case: &str, election: &str) {
    let put = call(
        router,
        "PutTaxInheritance",
        &json!({ "heritageStatus": election }),
    )
    .await;
    assert_eq!(
        (put.status, put.error_code.as_str()),
        (200, ""),
        "case {case:?}: expected the election {election:?} to be served with 200, got {put:?}"
    );
    // The modelled output has no members, so the body carries none.
    assert_eq!(
        put.body,
        json!({}),
        "case {case:?}: expected an empty body for the election {election:?}, got {:?}",
        put.body
    );
}

/// The election `GetTaxInheritance` reports.
async fn read_election(router: &MultiServiceRouter) -> Value {
    let got = call(router, "GetTaxInheritance", &json!({})).await;
    assert_eq!(got.status, 200, "GetTaxInheritance expected 200, got {got:?}");
    got.get("heritageStatus").clone()
}

/// RULE: a write of the account's tax-inheritance election records the submitted
/// `HeritageStatus` and answers the modelled empty success; the read that follows
/// reports the value last recorded, whichever value and however many writes came
/// before it.
#[tokio::test]
async fn an_election_that_was_recorded_is_the_election_that_is_reported() {
    for sequence in [
        Sequence {
            case: "a single OptIn",
            submitted: &[HERITAGE_STATUS_OPT_IN],
        },
        Sequence {
            case: "a single OptOut",
            submitted: &[HERITAGE_STATUS_OPT_OUT],
        },
        Sequence {
            case: "OptIn then OptOut",
            submitted: &[HERITAGE_STATUS_OPT_IN, HERITAGE_STATUS_OPT_OUT],
        },
        Sequence {
            case: "OptOut then OptIn",
            submitted: &[HERITAGE_STATUS_OPT_OUT, HERITAGE_STATUS_OPT_IN],
        },
        Sequence {
            case: "the same election twice",
            submitted: &[HERITAGE_STATUS_OPT_IN, HERITAGE_STATUS_OPT_IN],
        },
        Sequence {
            case: "the default election written over itself",
            submitted: &[DEFAULT_HERITAGE_STATUS, DEFAULT_HERITAGE_STATUS],
        },
        Sequence {
            case: "both values, back and forth",
            submitted: &[
                HERITAGE_STATUS_OPT_OUT,
                HERITAGE_STATUS_OPT_IN,
                HERITAGE_STATUS_OPT_IN,
                HERITAGE_STATUS_OPT_OUT,
                HERITAGE_STATUS_OPT_IN,
            ],
        },
    ] {
        // A fresh emulator per sequence, so each starts from an account with no
        // election recorded.
        let router = emulator();
        let before = read_election(&router).await;
        assert_eq!(
            before,
            json!(DEFAULT_HERITAGE_STATUS),
            "case {:?}: expected the default election before any write, got {before:?}",
            sequence.case
        );

        for (index, election) in sequence.submitted.iter().enumerate() {
            put_election(&router, sequence.case, election).await;
            // The read reports what the write recorded, and reading it does not
            // change it: the same answer twice in a row.
            for read in 1..=2 {
                let reported = read_election(&router).await;
                assert_eq!(
                    reported,
                    json!(election),
                    "case {:?}: expected write {index} to be reported by read {read}, got \
                     {reported:?}",
                    sequence.case
                );
            }
        }
    }
}

/// RULE: the election is the account's state, not a per-request value — every
/// caller of the service reads the one a write recorded.
#[tokio::test]
async fn a_recorded_election_is_shared_state() {
    for election in HERITAGE_STATUSES {
        let (router, engine) = emulator_with_engine();
        put_election(&router, election, election).await;

        // A second router over the SAME engine is another caller of the same
        // service: it reads the election that was recorded, not a fresh default.
        let other = mount_on(&engine);
        let reported = read_election(&other).await;
        assert_eq!(
            reported,
            json!(election),
            "case {election:?}: expected the recorded election on a second router, got \
             {reported:?}"
        );
    }
}

/// RULE: a request that names no election records nothing — it is refused with
/// the modelled `ValidationException` (400) / `MissingInput`, and the election
/// the account already had still answers the read.
#[tokio::test]
async fn a_write_that_names_no_election_records_nothing() {
    for (case, recorded) in [
        ("no election recorded yet", None),
        ("OptIn recorded", Some(HERITAGE_STATUS_OPT_IN)),
        ("OptOut recorded", Some(HERITAGE_STATUS_OPT_OUT)),
    ] {
        let router = emulator();
        if let Some(election) = recorded {
            put_election(&router, case, election).await;
        }

        let refused = call(&router, "PutTaxInheritance", &json!({})).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        assert_eq!(
            refused.get("errorCode").as_str(),
            Some("MissingInput"),
            "case {case:?}: expected the MissingInput code, got {:?}",
            refused.body
        );

        let reported = read_election(&router).await;
        assert_eq!(
            reported,
            json!(recorded.unwrap_or(DEFAULT_HERITAGE_STATUS)),
            "case {case:?}: expected the refusal to record nothing, got {reported:?}"
        );
    }
}
