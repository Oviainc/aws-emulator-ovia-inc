// SPDX-License-Identifier: CC0-1.0

//! The status of the agreement a replacement acts upon, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: a status transition of this service moves an agreement
//! only while it is still `ACTIVE`, so an agreement that has already reached a
//! terminal `AgreementStatus` keeps the status it reached — and the
//! `lastUpdateTime` of the call that put it there — whatever later call asks for
//! another move. Accepting a `REPLACE` request is one such transition: it
//! supersedes an `ACTIVE` source, and leaves a source that has already left
//! `ACTIVE` exactly as it found it, while still minting its own agreement. Every
//! report of the source — `DescribeAgreement`, `SearchAgreements` and its
//! `Status` filter — therefore keeps agreeing on the status it reached.
//!
//! The rows span what the rule generalises over: the status the source already
//! carries, the path that put it there (a direct cancel, an approved
//! cancellation request, an earlier replacement), the `Intent` of the request
//! accepted over it (`REPLACE`, which moves an active source, and `AMEND`, which
//! documents no move at all) and the number of replacements that name the same
//! source.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_cancellation_requests::cancellation_over;
use crate::agreement_lifecycle::{
    accept, agreement_over, agreement_type_filter, call, cancel, create, describe, emulator,
    search, term, Answer,
};

/// The `AgreementStatus` values this suite asserts over
/// (`smithy_query(query="shape", shape="AgreementStatus")`), spelled out here
/// rather than imported from the crate so a handler that stores something else
/// fails this test instead of agreeing with it.
const ACTIVE: &str = "ACTIVE";
/// See [`ACTIVE`].
const CANCELLED: &str = "CANCELLED";
/// See [`ACTIVE`].
const REPLACED: &str = "REPLACED";

/// The statuses an agreement of this emulator can be reported under, so a row
/// can assert that its source is reported under exactly one of them.
const REPORTED_STATUSES: [&str; 3] = [ACTIVE, CANCELLED, REPLACED];

/// `Intent`'s values that name a source agreement
/// (`smithy_query(query="shape", shape="Intent")`): "This parameter is required
/// for all non-NEW intents (i.e., AMEND or REPLACE)"
/// (`CreateAgreementRequestInput$sourceAgreementIdentifier`).
const REPLACE: &str = "REPLACE";
/// See [`REPLACE`].
const AMEND: &str = "AMEND";

/// Where the source agreement already stands when a request naming it is
/// accepted: every path this service has out of `ACTIVE`, plus the agreement
/// that never left it.
#[derive(Debug, Clone, Copy)]
enum SourceState {
    /// Still active: nothing has moved it.
    Active,
    /// The acceptor cancelled it directly (`CancelAgreement`).
    Cancelled,
    /// The buyer approved the seller's cancellation request
    /// (`AcceptAgreementCancellationRequest`), whose approval is what cancels
    /// the agreement.
    CancellationApproved,
    /// An earlier accepted `REPLACE` request already superseded it.
    AlreadyReplaced,
}

impl SourceState {
    /// The `AgreementStatus` the source carries once this path has run.
    fn status(self) -> &'static str {
        match self {
            Self::Active => ACTIVE,
            Self::Cancelled | Self::CancellationApproved => CANCELLED,
            Self::AlreadyReplaced => REPLACED,
        }
    }

    /// The status the source carries AFTER a request of `intent` naming it is
    /// accepted: a replacement supersedes a source that is still active, and
    /// every other combination leaves the status the source already reached.
    fn after(self, intent: &str) -> &'static str {
        match (self, intent) {
            (Self::Active, REPLACE) => REPLACED,
            _ => self.status(),
        }
    }
}

/// One agreement driven to the state `state` names, asserted to report the
/// status that path leaves it in before anything is accepted over it.
async fn source_agreement(
    router: &MultiServiceRouter,
    proposal: &str,
    state: SourceState,
) -> String {
    let agreement_id = agreement_over(router, proposal, vec![term("eula.v1")]).await;
    match state {
        SourceState::Active => {}
        SourceState::Cancelled => {
            let cancelled = cancel(router, &agreement_id).await;
            assert_eq!(
                cancelled.status, 200,
                "case {state:?}: cancelling the active agreement must succeed, got {} with {:?}",
                cancelled.status, cancelled.body
            );
        }
        SourceState::CancellationApproved => {
            let request_id = cancellation_over(router, &agreement_id, "OTHER").await;
            let approved = call(
                router,
                "AcceptAgreementCancellationRequest",
                json!({
                    "agreementId": agreement_id,
                    "agreementCancellationRequestId": request_id,
                }),
            )
            .await;
            assert_eq!(
                approved.status, 200,
                "case {state:?}: approving the cancellation request must succeed, got {} with {:?}",
                approved.status, approved.body
            );
        }
        SourceState::AlreadyReplaced => {
            let answer =
                accept_over(router, REPLACE, &format!("{proposal}first"), &agreement_id).await;
            assert_eq!(
                answer.status, 200,
                "case {state:?}: the first replacement must be accepted, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }

    let described = describe(router, &agreement_id).await;
    assert_eq!(
        described.str_at("/status"),
        state.status(),
        "case {state:?}: the source must report {} before anything is accepted over it, got {:?}",
        state.status(),
        described.body
    );
    agreement_id
}

/// Create a request of `intent` naming `source` and accept it, answering what
/// the accept said.
///
/// `agreementProposalIdentifier` "is required if the intent is not AMEND", so an
/// amendment carries none and takes its proposal from the agreement it acts
/// upon.
async fn accept_over(
    router: &MultiServiceRouter,
    intent: &str,
    proposal: &str,
    source: &str,
) -> Answer {
    let mut request = json!({
        "intent": intent,
        "sourceAgreementIdentifier": source,
        "requestedTerms": [term("eula.v2")],
    });
    if intent != AMEND {
        request["agreementProposalIdentifier"] = json!(proposal);
    }
    let created = create(router, request).await;
    assert_eq!(
        created.status, 200,
        "creating the {intent} request over {source} must succeed, got {} with {:?}",
        created.status, created.body
    );
    accept(
        router,
        json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
    )
    .await
}

/// The one `AgreementViewSummary` `SearchAgreements` reports for
/// `agreement_id`.
async fn view_summary(router: &MultiServiceRouter, agreement_id: &str) -> Value {
    let found = search(router, json!({"filters": [agreement_type_filter()]})).await;
    assert_eq!(
        found.status, 200,
        "searching for {agreement_id} must succeed, got {} with {:?}",
        found.status, found.body
    );
    found.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["agreementId"].as_str() == Some(agreement_id))
        .unwrap_or_else(|| panic!("{agreement_id} must be searchable, got {:?}", found.body))
}

/// The `lastUpdateTime` `SearchAgreements` reports for `agreement_id`.
async fn last_update_time(router: &MultiServiceRouter, agreement_id: &str) -> f64 {
    let summary = view_summary(router, agreement_id).await;
    summary["lastUpdateTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("{agreement_id} must report a lastUpdateTime, got {summary:?}"))
}

/// The `agreementId`s `SearchAgreements` selects under the `Status` filter
/// `status`.
async fn ids_with_status(router: &MultiServiceRouter, status: &str) -> Vec<String> {
    let found = search(
        router,
        json!({"filters": [agreement_type_filter(), {"name": "Status", "values": [status]}]}),
    )
    .await;
    assert_eq!(
        found.status, 200,
        "searching status {status} must succeed, got {} with {:?}",
        found.status, found.body
    );
    found.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["agreementId"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The rule: only an ACTIVE source is moved
// ---------------------------------------------------------------------------

/// Accepting a request that names a source agreement moves that source to
/// `REPLACED` only when the request replaces it and the source is still
/// `ACTIVE`; in every other row the source keeps the status it had reached, and
/// the `lastUpdateTime` the call that put it there stamped.
#[tokio::test]
async fn accepting_a_replacement_never_moves_a_source_that_already_left_active() {
    let router = emulator();

    let states = [
        SourceState::Active,
        SourceState::Cancelled,
        SourceState::CancellationApproved,
        SourceState::AlreadyReplaced,
    ];
    for (index, state) in states.into_iter().enumerate() {
        for intent in [REPLACE, AMEND] {
            let case = format!("{intent} over a source that is {state:?}");
            let suffix = format!("{index}{}", intent.to_lowercase());
            let source = source_agreement(&router, &format!("ap-src{suffix}"), state).await;
            let before = last_update_time(&router, &source).await;

            let accepted = accept_over(&router, intent, &format!("ap-new{suffix}"), &source).await;
            assert_eq!(
                accepted.status, 200,
                "case {case}: accepting must succeed, got {} with {:?}",
                accepted.status, accepted.body
            );
            let minted = accepted.str_at("/agreementId");
            assert!(
                !minted.is_empty() && minted != source,
                "case {case}: the accept mints its own agreement, got {minted:?} against the \
                 source {source:?}"
            );

            // The source: the status the rule leaves it in, agreed on by both
            // reports, and the instant of the last call that actually moved it.
            let expected = state.after(intent);
            let described = describe(&router, &source).await;
            assert_eq!(
                described.str_at("/status"),
                expected,
                "case {case}: expected the source to be {expected}, got {:?}",
                described.body
            );
            let summary = view_summary(&router, &source).await;
            assert_eq!(
                summary["status"].as_str(),
                Some(expected),
                "case {case}: the search summary must agree on {expected}, got {summary:?}"
            );
            let after = last_update_time(&router, &source).await;
            if expected == state.status() {
                assert_eq!(
                    after, before,
                    "case {case}: a call that moves nothing leaves lastUpdateTime at {before}, \
                     got {after}"
                );
            } else {
                assert!(
                    after > before,
                    "case {case}: the transition stamps lastUpdateTime past {before}, got {after}"
                );
            }

            // The `Status` filter selects the source under exactly the status it
            // carries, so a client that searches by status still finds it there.
            for status in REPORTED_STATUSES {
                let selected = ids_with_status(&router, status).await;
                assert_eq!(
                    selected.contains(&source),
                    status == expected,
                    "case {case}: the Status {status} filter must {} {source}, got {selected:?}",
                    if status == expected {
                        "select"
                    } else {
                        "leave out"
                    }
                );
            }

            // The agreement the accept minted is its own, active record, and a
            // replacement or amendment stays in the chain its source started.
            let new_described = describe(&router, &minted).await;
            assert_eq!(
                new_described.str_at("/status"),
                ACTIVE,
                "case {case}: the minted agreement is active, got {:?}",
                new_described.body
            );
            assert_eq!(
                new_described.str_at("/initialAgreementId"),
                describe(&router, &source).await.str_at("/initialAgreementId"),
                "case {case}: the minted agreement keeps the chain of its source, got {:?}",
                new_described.body
            );
        }
    }
}

/// However many replacements name the same source, only the first one moves it:
/// the source keeps `REPLACED` and the `lastUpdateTime` that first acceptance
/// stamped, and every later replacement still mints its own active agreement.
#[tokio::test]
async fn only_the_first_replacement_of_a_source_moves_it() {
    let router = emulator();

    let source = source_agreement(&router, "ap-chain", SourceState::Active).await;
    let mut minted = Vec::new();
    let mut moved_at = None;
    for attempt in 1..=3 {
        let accepted = accept_over(&router, REPLACE, &format!("ap-chain{attempt}"), &source).await;
        assert_eq!(
            accepted.status, 200,
            "case replacement {attempt}: accepting must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        minted.push(accepted.str_at("/agreementId"));

        let described = describe(&router, &source).await;
        assert_eq!(
            described.str_at("/status"),
            REPLACED,
            "case replacement {attempt}: the source is REPLACED, got {:?}",
            described.body
        );
        let stamped = last_update_time(&router, &source).await;
        match moved_at {
            None => moved_at = Some(stamped),
            Some(first) => assert_eq!(
                stamped, first,
                "case replacement {attempt}: the source keeps the lastUpdateTime {first} of the \
                 acceptance that moved it, got {stamped}"
            ),
        }
    }

    assert_eq!(
        minted.iter().collect::<std::collections::BTreeSet<_>>().len(),
        minted.len(),
        "each replacement mints its own agreement, got {minted:?}"
    );
    for (attempt, agreement_id) in minted.iter().enumerate() {
        let described = describe(&router, agreement_id).await;
        assert_eq!(
            described.str_at("/status"),
            ACTIVE,
            "case replacement {}: the minted agreement is active, got {:?}",
            attempt + 1,
            described.body
        );
    }
}
