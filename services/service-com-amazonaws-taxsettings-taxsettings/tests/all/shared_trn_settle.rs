// SPDX-License-Identifier: CC0-1.0

//! The clock-owed TRN settle rule, driven over the restJson1 wire through a read
//! of each kind of stored registration.
//!
//! RULE: a stored registration carries the instant the validation of its number
//! falls due, and a read reports what that validation reached exactly once that
//! instant has passed — before it the status the write stored, from it on the
//! outcome the number earned, and never a further move afterwards, because
//! settling clears the due instant. Both kinds of stored registration — the
//! account's own TRN and a supplemental one — have that transition, for the same
//! reason, and both settle through the ONE rule in `state.rs`, driven for every
//! record by `settle_due`. So the table below drives that rule through
//! `GetTaxRegistration` and through `ListSupplementalTaxRegistrations`: a change
//! to it moves both answers or neither.
//!
//! The table spans the dimensions the rule generalises over: the kind of stored
//! registration, where the due instant sits relative to the read (ahead of it,
//! at it, behind it, and cleared), the outcome the stored number earns, and
//! repeated reads.
//!
//! The harness (`emulator_with_engine`, `call`, the supplemental write and
//! listing, and the own-TRN status reader) lives in
//! `tax_registration_lifecycle.rs`, `supplemental_tax_registrations.rs` and
//! `fidelity_rules.rs`; nothing here builds a second one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, STATUS_PENDING, STATUS_VERIFIED,
    TRN_VALIDATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::fidelity_rules::read_status;
use crate::supplemental_tax_registrations::{entry_for, list_all, put as put_supplemental};
use crate::tax_registration_lifecycle::{call, emulator_with_engine, Answer};

/// The account every row registers under. Which account holds the record is not
/// a dimension of this rule.
const ACCOUNT_ID: &str = "000000000001";

/// One kind of stored registration whose validation the clock owes, with the
/// read path that reports its status.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// The account's own TRN, read through `GetTaxRegistration`.
    OwnTrn,
    /// A supplemental TRN, read through `ListSupplementalTaxRegistrations`.
    Supplemental,
}

/// Where the due instant sits relative to the read that follows it.
#[derive(Clone, Copy, Debug)]
enum Due {
    /// Well ahead of the read: the validation is still outstanding.
    Ahead,
    /// At the read: the clock has made the validation due.
    AtTheRead,
    /// Well behind the read: a validation nobody has read since it fell due.
    Behind,
    /// No due instant at all — the record of a validation that has settled, so
    /// nothing is owed and no read may move it.
    Cleared,
}

/// Every position of the due instant the rule distinguishes.
const DUE_POSITIONS: &[Due] = &[Due::Ahead, Due::AtTheRead, Due::Behind, Due::Cleared];

impl Due {
    /// The due instant this position stores, read just before the read under
    /// test. Never a sleep: the due instant is state.
    fn instant(self) -> Option<Instant> {
        let now = Instant::now();
        let an_hour = Duration::from_secs(3_600);
        match self {
            Self::Ahead => Some(now + an_hour),
            Self::AtTheRead => Some(now),
            Self::Behind => Some(now.checked_sub(an_hour).unwrap_or(now)),
            Self::Cleared => None,
        }
    }

    /// Whether the read that follows finds a validation the clock has already
    /// made due.
    const fn already_due(self) -> bool {
        matches!(self, Self::AtTheRead | Self::Behind)
    }

    /// Whether the record still carries a due instant after that read.
    const fn stays_outstanding(self) -> bool {
        matches!(self, Self::Ahead)
    }
}

impl Kind {
    /// The operation a row reads the status through.
    const fn operation(self) -> &'static str {
        match self {
            Self::OwnTrn => "GetTaxRegistration",
            Self::Supplemental => "ListSupplementalTaxRegistrations",
        }
    }

    /// Store one registration of this kind carrying `registration_id`.
    async fn store(self, router: &MultiServiceRouter, registration_id: &str) -> Answer {
        match self {
            Self::OwnTrn => {
                call(
                    router,
                    "PutTaxRegistration",
                    &json!({
                        "accountId": ACCOUNT_ID,
                        "taxRegistrationEntry": {
                            "registrationId": registration_id,
                            "registrationType": "VAT",
                            "legalName": "Beispiel GmbH",
                            "legalAddress": {
                                "addressLine1": "Unter den Linden 1",
                                "city": "Berlin",
                                "postalCode": "10117",
                                "countryCode": "DE",
                            },
                        },
                    }),
                )
                .await
            }
            Self::Supplemental => {
                put_supplemental(router, &entry_for("CH", registration_id, "Acme AG")).await
            }
        }
    }

    /// The status [`Self::operation`] reports for the registration carrying
    /// `registration_id`, or `None` when it reports no such registration.
    async fn reported_status(
        self,
        router: &MultiServiceRouter,
        registration_id: &str,
    ) -> Option<String> {
        match self {
            Self::OwnTrn => read_status(router, self.operation(), ACCOUNT_ID).await,
            Self::Supplemental => list_all(router, None)
                .await
                .into_iter()
                .find(|listed| {
                    listed.get("registrationId").and_then(Value::as_str) == Some(registration_id)
                })
                .and_then(|listed| {
                    listed
                        .get("status")
                        .and_then(Value::as_str)
                        .map(std::string::ToString::to_string)
                }),
        }
    }

    /// Put the store in the state this kind of write needs: a supplemental
    /// registration supplements the account's own, so the account must already
    /// hold a settled registration of its own for such a write to be reached.
    fn prepare(self, engine: &Arc<engine::InMemoryEngine>) {
        if matches!(self, Self::Supplemental) {
            crate::supplemental_tax_registrations::hold_a_settled_own_registration(engine);
        }
    }

    /// Point the validation of every stored registration of this kind at `due`,
    /// the way the clock would have.
    ///
    /// The suites beside this one hold a validation open or make it due through
    /// their own pair of helpers; this rule needs the due instant as a VALUE —
    /// ahead of the read, at it, behind it and cleared — and needs it for either
    /// kind of record, which is exactly what the shared rule generalises over.
    fn point_validation_at(self, engine: &Arc<engine::InMemoryEngine>, due: Option<Instant>) {
        let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
        let mut store = state.lock();
        match self {
            Self::OwnTrn => {
                for stored in store.registrations.values_mut() {
                    stored.validation_due_at = due;
                }
            }
            Self::Supplemental => {
                for stored in store.supplemental_registrations.values_mut() {
                    stored.validation_due_at = due;
                }
            }
        }
    }

    /// The status and due instant the store holds for this kind's one record —
    /// the ground truth a reported status is compared against, read from the
    /// store rather than from the operation under test.
    fn stored_validation(self, engine: &Arc<engine::InMemoryEngine>) -> (String, Option<Instant>) {
        let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
        let store = state.lock();
        let held = match self {
            Self::OwnTrn => store
                .registrations
                .values()
                .next()
                .map(|stored| (stored.status.clone(), stored.validation_due_at)),
            Self::Supplemental => store
                .supplemental_registrations
                .values()
                .next()
                .map(|stored| (stored.status.clone(), stored.validation_due_at)),
        };
        held.unwrap_or_else(|| panic!("{self:?}: the write must have stored a record"))
    }
}

/// One stored registration, and the status its validation reaches once the clock
/// has made it due.
struct Row {
    case: &'static str,
    kind: Kind,
    /// The number the registration is stored with.
    registration_id: &'static str,
    /// The status the record settles to.
    settles_to: &'static str,
}

/// One registration of each kind, driven through the settle rule the clock owes.
///
/// Every number below is one its jurisdiction's authority issues, because the
/// write refuses any other (`state::check_registration_number`, pinned in
/// `tests/all/trn_validation_outcome.rs`): what this table varies is the KIND of
/// record the shared rule has to move and the spelling its number was stored in.
/// Supplemental registration is held only for jurisdictions
/// `crate::state::SUPPLEMENTAL_TRN_COUNTRIES` names, and this service holds no
/// published structure for that jurisdiction's numbers
/// (`crate::state::registration_number`), so nothing contradicts either
/// supplemental number.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an own TRN carrying its country prefix",
            kind: Kind::OwnTrn,
            registration_id: "DE136695976",
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "the same own TRN written without that prefix",
            kind: Kind::OwnTrn,
            registration_id: "136695976",
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a supplemental TRN written with the separators the register prints",
            kind: Kind::Supplemental,
            registration_id: "CHE-123.456.789",
            settles_to: STATUS_VERIFIED,
        },
        Row {
            case: "a supplemental TRN written as bare characters",
            kind: Kind::Supplemental,
            registration_id: "CHE987654321",
            settles_to: STATUS_VERIFIED,
        },
    ]
}

/// RULE: every kind of stored registration reports the status its write stored
/// while its validation is still outstanding and the status that validation
/// reached from the instant it falls due, settles at most once because the due
/// instant is cleared, and never moves a record that carries no due instant.
#[tokio::test]
async fn every_kind_of_registration_settles_by_the_shared_rule() {
    for row in rows() {
        for due in DUE_POSITIONS.iter().copied() {
            let case = format!(
                "{} ({:?}) read through {} with its validation {due:?}",
                row.case,
                row.registration_id,
                row.kind.operation()
            );
            let (router, engine) = emulator_with_engine();
            row.kind.prepare(&engine);
            let written = row.kind.store(&router, row.registration_id).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: the entry must be accepted, got {written:?}"
            );
            assert_eq!(
                written.get("status").as_str(),
                Some(STATUS_PENDING),
                "case {case:?}: the write must report the outstanding status, got {:?}",
                written.body
            );

            row.kind.point_validation_at(&engine, due.instant());
            let expected = if due.already_due() {
                row.settles_to
            } else {
                STATUS_PENDING
            };

            // Read twice: settling materialises what the clock owed, so the
            // second read of a record the first one settled answers the same.
            for attempt in 0..2 {
                let status = row.kind.reported_status(&router, row.registration_id).await;
                assert_eq!(
                    status.as_deref(),
                    Some(expected),
                    "case {case:?}: read {attempt} must report {expected:?}, got {status:?}"
                );
            }

            let (stored_status, stored_due) = row.kind.stored_validation(&engine);
            assert_eq!(
                stored_status, expected,
                "case {case:?}: the store must hold {expected:?} after the reads, got \
                 {stored_status:?}"
            );
            assert_eq!(
                stored_due.is_some(),
                due.stays_outstanding(),
                "case {case:?}: a record that settled carries no due instant and one still \
                 outstanding keeps its own, got {stored_due:?}"
            );
        }
    }
}

/// RULE: a write of either kind stores the registration under the status its
/// validation is outstanding in, paired with the instant that validation falls
/// due — one validation delay after the write.
#[tokio::test]
async fn a_write_of_either_kind_stores_one_outstanding_validation() {
    for row in rows() {
        let case = format!("{} ({:?})", row.case, row.registration_id);
        let (router, engine) = emulator_with_engine();
        row.kind.prepare(&engine);
        let before = Instant::now();
        let written = row.kind.store(&router, row.registration_id).await;
        let after = Instant::now();
        assert_eq!(
            written.status, 200,
            "case {case:?}: the entry must be accepted, got {written:?}"
        );

        let (status, due) = row.kind.stored_validation(&engine);
        assert_eq!(
            status, STATUS_PENDING,
            "case {case:?}: the write must store the outstanding status, got {status:?}"
        );
        let due = due.unwrap_or_else(|| {
            panic!("case {case:?}: the write must record when the validation falls due, got none")
        });
        assert!(
            due >= before + TRN_VALIDATION_DELAY && due <= after + TRN_VALIDATION_DELAY,
            "case {case:?}: the validation must fall due one {TRN_VALIDATION_DELAY:?} after the \
             write, got {:?} after it",
            due - before
        );
    }
}
