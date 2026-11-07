// SPDX-License-Identifier: CC0-1.0

//! The calls that answer about the ONE agreement an id member of their input
//! names, driven on a store no other call has read, end to end over HTTP.
//!
//! Every call here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: a call whose answer depends on the one agreement its
//! input names answers about that agreement as the clock has already left it, so
//! the answer never depends on whether some unrelated call looked at the store
//! first. An agreement reaches its own stored end date at that instant whether
//! or not anybody calls, and `EndTimeBehaviorType` names what that date does —
//! `RENEW` is "a new agreement is created from the accepted terms of this
//! agreement", `EXPIRE` is "the agreement ends and isn't renewed or replaced"
//! (`smithy_query(query="shape", shape="EndTimeBehavior")`). So from that date on
//! the renewal is an agreement of the store, carrying the accepted terms it
//! re-states from the source — which is what `GetAgreementTerms` reports
//! ("obtains details about the terms in an agreement",
//! `smithy_query(query="operation", operation="GetAgreementTerms")`) and an
//! agreement every call that resolves one holds, while an id no end date ever
//! created is still refused by every one of them — as the agreement that was
//! looked for where the agreement is the resource, and as the input fault on the
//! member where the call acts on a payment request that agreement only scopes.
//!
//! The rows differ along what the rule generalises over: which contract length
//! the acceptor purchased, how many whole lengths have passed since it was
//! signed (one, two, as many as the renewal term allows), which link of the
//! chain is asked about, and which call resolves the agreement — the readers of
//! the agreement itself, the readers and writers that resolve it as the PARENT
//! of a child they name, and the quote that acts upon it as its source.
//!
//! The clock cannot be wound forward 30 days inside a test, so the agreement's
//! own stored window is wound BACK instead: the harness that does it
//! (`aged_agreement`, `days`) is the one `agreement_expiry` owns, and the wire
//! harness (`call`, `describe`, `emulator_over_engine`, `invoices_of`, `search`,
//! `term`, `with_agreement_type`) is `agreement_lifecycle`'s, with the terms of
//! an auto-renewing purchase (`renewing_terms`) `agreement_renewals`', the batch
//! (`entry`, `submit`) `billing_adjustments`', and the charge report
//! (`charges`, `items`, `references`) `purchase_order_updates`'.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_expiry::{aged_agreement, days};
use crate::agreement_lifecycle::{
    call, describe, emulator_over_engine, invoices_of, search, term, with_agreement_type, Answer,
};
use crate::agreement_renewals::renewing_terms;
use crate::billing_adjustments::{entry, submit};
use crate::error_bodies::assert_input_fault;
use crate::purchase_order_updates::{charges, items, references};

/// The resource kinds a not-found refusal names in `resourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`. Spelled out here rather
/// than imported from the crate, so a handler naming another kind fails these
/// rows instead of agreeing with them.
const AGREEMENT: &str = "Agreement";
/// See [`AGREEMENT`].
const PAYMENT_REQUEST: &str = "PaymentRequest";
/// See [`AGREEMENT`].
const AGREEMENT_CANCELLATION_REQUEST: &str = "AgreementCancellationRequest";
/// See [`AGREEMENT`].
const BILLING_ADJUSTMENT_REQUEST: &str = "BillingAdjustmentRequest";

/// The status this service answers a lookup that found nothing with, which the
/// crate's `unknown_ids` suite pins against the recorded AWS responses.
const NOT_FOUND_STATUS: u16 = 400;

/// The `ValidationExceptionReason` a call that takes `agreementId` as INPUT
/// refuses a value naming no agreement with
/// (`smithy_query(query="shape", shape="ValidationExceptionReason")`).
const INVALID_AGREEMENT_ID: &str = "INVALID_AGREEMENT_ID";

/// Ids of the CHILD resources these calls name beside the agreement, each
/// within its modelled pattern — `^pr-[a-zA-Z0-9]+$`, `^acr-[a-zA-Z0-9]+$` and
/// `^ba-[a-zA-Z0-9]+$` (`smithy_query(query="operation",
/// operation="GetAgreementPaymentRequest,GetAgreementCancellationRequest,
/// GetBillingAdjustmentRequest")`) — and none of them a child this emulator's
/// minter ever produced, so the child is what the call cannot resolve once the
/// agreement has.
const UNKNOWN_PAYMENT_REQUEST: &str = "pr-ffffffffffffffff";
/// See [`UNKNOWN_PAYMENT_REQUEST`].
const UNKNOWN_CANCELLATION_REQUEST: &str = "acr-ffffffffffffffff";
/// See [`UNKNOWN_PAYMENT_REQUEST`].
const UNKNOWN_BILLING_ADJUSTMENT: &str = "ba-ffffffffffffffff";

/// An agreement id within `AgreementId`'s `^[A-Za-z0-9_/-]+$` that names no
/// agreement any end date of these stores could have created.
const UNKNOWN_AGREEMENT: &str = "agmt-ffffffffffffffff";

/// One store to ask about: the terms an acceptor accepted, how long before now
/// the first agreement of the chain was signed, and how many renewals the clock
/// therefore owes it.
///
/// `pub(crate)` because the suite over the invoices one of those calls reports
/// (`tests/all/unsettled_store_invoice_reports.rs`) ages the same stores rather
/// than spelling a second copy of this harness.
pub(crate) struct Aged {
    pub(crate) case: &'static str,
    pub(crate) requested: Vec<Value>,
    pub(crate) signed_ago: Duration,
    pub(crate) renewals: usize,
}

/// The rows: one renewal, several, and as many as the renewal term allows, over
/// three of the contract lengths the rate card prices.
///
/// `pub(crate)` for the same reason as [`Aged`].
pub(crate) fn aged_rows() -> Vec<Aged> {
    vec![
        Aged {
            case: "a month's purchase a day past its end date",
            requested: renewing_terms("P1M"),
            signed_ago: days(31),
            renewals: 1,
        },
        Aged {
            case: "two whole months on: the renewal renews in turn",
            requested: renewing_terms("P1M"),
            signed_ago: days(61),
            renewals: 2,
        },
        Aged {
            case: "as many months past as the renewal term allows renewals",
            requested: renewing_terms("P1M"),
            signed_ago: days(91),
            renewals: 3,
        },
        Aged {
            case: "a year's purchase a day past its end date, at another length",
            requested: renewing_terms("P12M"),
            signed_ago: days(366),
            renewals: 1,
        },
        Aged {
            case: "a three-year purchase a day past its end date",
            requested: renewing_terms("P36M"),
            signed_ago: days(1096),
            renewals: 1,
        },
    ]
}

/// A store built exactly as `row` says, answering the emulator over it and the
/// id of the first agreement of the chain.
///
/// Ids are minted from the store's own counter, so two stores built from the
/// same row mint the same ids — which is what lets a call under test name a
/// renewal on a store nothing else has read.
///
/// `pub(crate)` for the same reason as [`Aged`].
pub(crate) async fn aged_store(row: &Aged, proposal: &str) -> (MultiServiceRouter, String) {
    let (router, engine) = emulator_over_engine();
    let source = aged_agreement(
        &router,
        &engine,
        proposal,
        row.requested.clone(),
        row.signed_ago,
    )
    .await;
    (router, source)
}

/// The ids of every agreement of the chain `source` starts, oldest first, read
/// from a store an UNRELATED call has already settled — the sibling reader
/// `DescribeAgreement` followed by the search over the chain
/// (`AgreementViewSummary$initialAgreementId`: "you can also use it as the
/// `InitialAgreementId` filter value to return every agreement in the same
/// chain").
///
/// `pub(crate)` for the same reason as [`Aged`].
pub(crate) async fn chain_after_an_unrelated_read(router: &MultiServiceRouter, source: &str) -> Vec<String> {
    let described = describe(router, source).await;
    assert_eq!(
        described.status, 200,
        "settling the chain of {source} through a sibling reader must succeed, got {} with {:?}",
        described.status, described.body
    );
    let answer = search(
        router,
        json!({
            "filters": with_agreement_type(
                &json!([{"name": "InitialAgreementId", "values": [source]}]),
            ),
            "maxResults": 50,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "searching the chain of {source} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let mut summaries: Vec<Value> = answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    summaries.sort_by(|left, right| {
        instant(left, "startTime")
            .partial_cmp(&instant(right, "startTime"))
            .expect("every agreement reports a start time")
    });
    summaries
        .iter()
        .map(|summary| {
            summary["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The instant `member` of one reported record, in epoch seconds.
///
/// `pub(crate)` for the same reason as [`Aged`].
pub(crate) fn instant(reported: &Value, member: &str) -> f64 {
    reported[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be reported, got {reported:?}"))
}

/// The accepted terms `agreement` reports, as the call under test answers them.
async fn terms(router: &MultiServiceRouter, agreement: &str) -> Answer {
    call(
        router,
        "GetAgreementTerms",
        json!({"agreementId": agreement}),
    )
    .await
}

/// The `(union member, id)` of every accepted term an answer carries, in the
/// order it reports them.
fn term_pairs(answer: &Answer) -> Vec<(String, String)> {
    answer.body["acceptedTerms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|accepted| {
            let object = accepted
                .as_object()
                .unwrap_or_else(|| panic!("an accepted term is a union object, got {accepted:?}"));
            let (member, payload) = object
                .iter()
                .next()
                .unwrap_or_else(|| panic!("an accepted term carries one member, got {accepted:?}"));
            (
                member.clone(),
                payload["id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The accepted terms of the renewal an end date created are reported by the
/// call that exists to report them, even when it is the FIRST call the store
/// sees — and an unrelated call before it changes that answer in no way.
#[tokio::test]
async fn the_terms_of_the_renewal_the_clock_owes_are_reported_without_an_earlier_call() {
    for (index, row) in aged_rows().into_iter().enumerate() {
        let case = row.case;
        let proposal = format!("ap-unsettledterms{index}");

        // A store of the row read through an unrelated call first, which says
        // which ids the clock owes this chain.
        let (probe, source) = aged_store(&row, &proposal).await;
        let chain = chain_after_an_unrelated_read(&probe, &source).await;
        assert_eq!(
            chain.first().map(String::as_str),
            Some(source.as_str()),
            "case {case:?}: the chain starts at the purchase that was accepted, got {chain:?}"
        );
        assert_eq!(
            chain.len() - 1,
            row.renewals,
            "case {case:?}: expected {} renewal(s) the clock owes, got {chain:?}",
            row.renewals
        );

        for renewal in &chain[1..] {
            // A store of its own, built identically, where the call under test
            // is the FIRST it sees.
            let (router, same) = aged_store(&row, &proposal).await;
            assert_eq!(
                same, source,
                "case {case:?}: the identical setup must mint the identical agreement id"
            );

            let first = terms(&router, renewal).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the renewal {renewal} is an agreement whose terms this call \
                 reports, got {} with {:?}",
                first.status, first.body
            );
            let reported = term_pairs(&first);
            assert!(
                !reported.is_empty(),
                "case {case:?}: the renewal {renewal} carries the accepted terms it re-states, \
                 got {:?}",
                first.body
            );

            // "A new agreement is created from the accepted terms of this
            // agreement": the terms the renewal re-states are the source's.
            let of_source = terms(&router, &source).await;
            assert_eq!(
                of_source.status, 200,
                "case {case:?}: the source's own terms must be reported, got {} with {:?}",
                of_source.status, of_source.body
            );
            assert_eq!(
                reported,
                term_pairs(&of_source),
                "case {case:?}: expected the renewal {renewal} to carry the source's accepted \
                 terms {:?}, got {reported:?}",
                term_pairs(&of_source)
            );

            // The unrelated call that used to be what admitted the renewal
            // changes nothing: the identical request answers the identical body.
            let described = describe(&router, &source).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the unrelated read must succeed, got {} with {:?}",
                described.status, described.body
            );
            let again = terms(&router, renewal).await;
            assert_eq!(
                again.body, first.body,
                "case {case:?}: an unrelated read must not change the terms of {renewal}, \
                 expected {:?}, got {:?}",
                first.body, again.body
            );
        }

        // Settling admits the agreements the clock owed and nothing else: an id
        // no end date created is still the agreement that was looked for.
        let (router, _same) = aged_store(&row, &proposal).await;
        let refused = terms(&router, UNKNOWN_AGREEMENT).await;
        assert_not_found(case, AGREEMENT, UNKNOWN_AGREEMENT, &refused);
    }
}

/// What the agreement resolving means for one call's answer.
enum Expect {
    /// The call answers ABOUT the agreement, so it is served — and the answer
    /// carries this pointer, which an empty 200 would not.
    Served(&'static str),
    /// The call names a child the store holds none of, so once the agreement
    /// has resolved the refusal is about that CHILD: `(resourceType,
    /// resourceId)`.
    ChildNotFound(&'static str, &'static str),
}

/// How one call answers an `agreementId` that resolves to no agreement of the
/// store.
#[derive(Clone, Copy)]
enum Unresolvable {
    /// The agreement is the resource the call looks for, or the one it acts
    /// upon, so the reference is the modelled not-found naming `Agreement`.
    NotFound,
    /// The call acts on a payment request, and `agreementId` only says which
    /// agreement that request was sent under — so a value naming no agreement
    /// is the input fault on that member
    /// (`payment_request_agreement_ids`).
    InputFault,
}

/// One call whose answer depends on the ONE agreement an id member of its input
/// names.
struct SingleAgreement {
    operation: &'static str,
    /// The member carrying that agreement's id.
    member: &'static str,
    /// The other members the model makes `@required`, each a value the call
    /// accepts — so the only thing a row varies is the agreement it names.
    others: Value,
    expect: Expect,
    /// How the call refuses an id that names no agreement at all.
    unresolvable: Unresolvable,
}

impl SingleAgreement {
    fn request(&self, agreement: &str) -> Value {
        let mut body = self.others.clone();
        body[self.member] = json!(agreement);
        body
    }
}

/// Every call of this service whose answer depends on the one agreement its
/// input names: the readers of the agreement itself, the readers and writers
/// that resolve it as the PARENT of the child they name, and the quote that
/// "acts upon" it as its source. The required members come from
/// `smithy_query(query="operation", operation="…")` for each.
fn single_agreement_calls() -> Vec<SingleAgreement> {
    vec![
        SingleAgreement {
            operation: "DescribeAgreement",
            member: "agreementId",
            others: json!({}),
            expect: Expect::Served("/agreementId"),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "GetAgreementTerms",
            member: "agreementId",
            others: json!({}),
            expect: Expect::Served("/acceptedTerms/0"),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "GetAgreementEntitlements",
            member: "agreementId",
            others: json!({}),
            expect: Expect::Served("/agreementEntitlements/0"),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "GetAgreementPaymentRequest",
            member: "agreementId",
            others: json!({"paymentRequestId": UNKNOWN_PAYMENT_REQUEST}),
            expect: Expect::ChildNotFound(PAYMENT_REQUEST, UNKNOWN_PAYMENT_REQUEST),
            unresolvable: Unresolvable::InputFault,
        },
        SingleAgreement {
            operation: "AcceptAgreementPaymentRequest",
            member: "agreementId",
            others: json!({"paymentRequestId": UNKNOWN_PAYMENT_REQUEST}),
            expect: Expect::ChildNotFound(PAYMENT_REQUEST, UNKNOWN_PAYMENT_REQUEST),
            unresolvable: Unresolvable::InputFault,
        },
        SingleAgreement {
            operation: "RejectAgreementPaymentRequest",
            member: "agreementId",
            others: json!({
                "paymentRequestId": UNKNOWN_PAYMENT_REQUEST,
                "rejectionReason": "The invoice has already been settled",
            }),
            expect: Expect::ChildNotFound(PAYMENT_REQUEST, UNKNOWN_PAYMENT_REQUEST),
            unresolvable: Unresolvable::InputFault,
        },
        SingleAgreement {
            operation: "CancelAgreementPaymentRequest",
            member: "agreementId",
            others: json!({"paymentRequestId": UNKNOWN_PAYMENT_REQUEST}),
            expect: Expect::ChildNotFound(PAYMENT_REQUEST, UNKNOWN_PAYMENT_REQUEST),
            unresolvable: Unresolvable::InputFault,
        },
        SingleAgreement {
            operation: "GetAgreementCancellationRequest",
            member: "agreementId",
            others: json!({"agreementCancellationRequestId": UNKNOWN_CANCELLATION_REQUEST}),
            expect: Expect::ChildNotFound(
                AGREEMENT_CANCELLATION_REQUEST,
                UNKNOWN_CANCELLATION_REQUEST,
            ),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "AcceptAgreementCancellationRequest",
            member: "agreementId",
            others: json!({"agreementCancellationRequestId": UNKNOWN_CANCELLATION_REQUEST}),
            expect: Expect::ChildNotFound(
                AGREEMENT_CANCELLATION_REQUEST,
                UNKNOWN_CANCELLATION_REQUEST,
            ),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "RejectAgreementCancellationRequest",
            member: "agreementId",
            others: json!({
                "agreementCancellationRequestId": UNKNOWN_CANCELLATION_REQUEST,
                "rejectionReason": "The buyer keeps the agreement",
            }),
            expect: Expect::ChildNotFound(
                AGREEMENT_CANCELLATION_REQUEST,
                UNKNOWN_CANCELLATION_REQUEST,
            ),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "CancelAgreementCancellationRequest",
            member: "agreementId",
            others: json!({
                "agreementCancellationRequestId": UNKNOWN_CANCELLATION_REQUEST,
                "cancellationReason": "Withdrawn by the proposer",
            }),
            expect: Expect::ChildNotFound(
                AGREEMENT_CANCELLATION_REQUEST,
                UNKNOWN_CANCELLATION_REQUEST,
            ),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "GetBillingAdjustmentRequest",
            member: "agreementId",
            others: json!({"billingAdjustmentRequestId": UNKNOWN_BILLING_ADJUSTMENT}),
            expect: Expect::ChildNotFound(BILLING_ADJUSTMENT_REQUEST, UNKNOWN_BILLING_ADJUSTMENT),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            operation: "ListAgreementInvoiceLineItems",
            member: "agreementId",
            // `groupBy` is `@required` on this listing.
            others: json!({"groupBy": "INVOICE_ID"}),
            expect: Expect::Served("/agreementInvoiceLineItemGroupSummaries/0"),
            unresolvable: Unresolvable::NotFound,
        },
        SingleAgreement {
            // "This parameter is required for all intents except NEW", and an
            // amendment acts upon the agreement it names.
            operation: "CreateAgreementRequest",
            member: "sourceAgreementIdentifier",
            others: json!({"intent": "AMEND", "requestedTerms": [term("eula.v1")]}),
            expect: Expect::Served("/agreementRequestId"),
            unresolvable: Unresolvable::NotFound,
        },
    ]
}

/// Assert one refusal is the modelled not-found for the resource that was
/// looked for, at the status this service answers a lookup that found nothing
/// with.
fn assert_not_found(case: &str, resource_type: &str, resource_id: &str, answer: &Answer) {
    assert_eq!(
        answer.status, NOT_FOUND_STATUS,
        "case {case:?}: a lookup that found nothing answers HTTP {NOT_FOUND_STATUS}, got {} with \
         {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceType"),
        resource_type,
        "case {case:?}: expected resourceType {resource_type:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("/resourceId"),
        resource_id,
        "case {case:?}: expected resourceId {resource_id:?}, got {:?}",
        answer.body
    );
}

/// Assert the agreement a call names resolved: either the call was served, or
/// the only thing it could not resolve is the child it also names.
fn assert_agreement_resolved(case: &str, agreement: &str, row: &SingleAgreement, answer: &Answer) {
    match row.expect {
        Expect::Served(pointer) => {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the agreement {agreement} the clock owes must be served, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer
                    .body
                    .pointer(pointer)
                    .is_some_and(|carried| !carried.is_null()),
                "case {case:?}: the answer must carry {pointer}, got {:?}",
                answer.body
            );
        }
        Expect::ChildNotFound(resource_type, resource_id) => {
            assert_not_found(case, resource_type, resource_id, answer);
            assert_ne!(
                answer.str_at("/resourceId"),
                agreement,
                "case {case:?}: the agreement {agreement} the clock owes resolved, so the refusal \
                 is about the child, got {:?}",
                answer.body
            );
        }
    }
}

/// The renewal an end date created is an agreement every call that resolves one
/// holds, even when that call is the first the store sees — and the same request
/// is answered the same way on a store an unrelated call has already settled.
#[tokio::test]
async fn every_single_agreement_call_resolves_the_renewal_the_clock_owes() {
    let row = Aged {
        case: "a month's purchase a day past its end date",
        requested: renewing_terms("P1M"),
        signed_ago: days(31),
        renewals: 1,
    };
    let proposal = "ap-unsettledsingle";

    let (probe, source) = aged_store(&row, proposal).await;
    let chain = chain_after_an_unrelated_read(&probe, &source).await;
    assert_eq!(
        chain.len(),
        row.renewals + 1,
        "{}: expected the source and {} renewal(s), got {chain:?}",
        row.case,
        row.renewals
    );
    let renewal = chain
        .last()
        .cloned()
        .expect("the chain holds at least the source");
    assert_ne!(
        renewal, source,
        "the renewal the end date created is an agreement of its own, got {chain:?}"
    );

    for under_test in single_agreement_calls() {
        let case = under_test.operation;

        // The store where the call under test is the FIRST one: nothing else
        // has settled the end date that created the renewal it names.
        let (unsettled, same) = aged_store(&row, proposal).await;
        assert_eq!(
            same, source,
            "case {case:?}: the identical setup must mint the identical agreement id"
        );
        let first = call(&unsettled, case, under_test.request(&renewal)).await;
        assert_agreement_resolved(case, &renewal, &under_test, &first);

        // The identical request on a store an unrelated call settled first: the
        // same answer, so the call's own answer never depended on that call.
        let (settled, same) = aged_store(&row, proposal).await;
        assert_eq!(
            same, source,
            "case {case:?}: the identical setup must mint the identical agreement id"
        );
        let described = describe(&settled, &source).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the unrelated read must succeed, got {} with {:?}",
            described.status, described.body
        );
        let after = call(&settled, case, under_test.request(&renewal)).await;
        assert_agreement_resolved(case, &renewal, &under_test, &after);
        assert_eq!(
            first.status,
            after.status,
            "case {case:?}: expected the status the settled store answers ({}), got {}",
            after.status,
            first.status
        );
        assert_eq!(
            first.error_code(),
            after.error_code(),
            "case {case:?}: expected the error code the settled store answers ({:?}), got {:?}",
            after.error_code(),
            first.error_code()
        );
    }
}

/// The complement: an `agreementId` no end date of the store could have created
/// is still refused by every one of those calls — settling admits the agreements
/// the clock owed and nothing else. Each call refuses it in its own class: the
/// modelled not-found naming `Agreement` where the agreement is the resource the
/// call looks for or acts upon, and the input fault on the member where the call
/// acts on a payment request the agreement only scopes
/// (`payment_request_agreement_ids`).
#[tokio::test]
async fn an_agreement_id_no_end_date_created_is_still_refused() {
    let row = Aged {
        case: "a month's purchase a day past its end date",
        requested: renewing_terms("P1M"),
        signed_ago: days(31),
        renewals: 1,
    };
    let (router, _source) = aged_store(&row, "ap-unsettledunknown").await;

    for under_test in single_agreement_calls() {
        let case = under_test.operation;
        let answer = call(&router, case, under_test.request(UNKNOWN_AGREEMENT)).await;
        match under_test.unresolvable {
            Unresolvable::NotFound => {
                assert_not_found(case, AGREEMENT, UNKNOWN_AGREEMENT, &answer)
            }
            Unresolvable::InputFault => {
                assert_input_fault(case, INVALID_AGREEMENT_ID, under_test.member, &answer)
            }
        }
    }
}

/// The same rule where the record a call acts on is a CHILD of the renewal the
/// clock owes: the invoice the renewal's acceptance billed and the charge its
/// own quote minted belong to the store from that end date on, so the batch
/// that adjusts the one and the report that carries the other reach them on a
/// store nothing else has read.
#[tokio::test]
async fn the_children_of_the_renewal_the_clock_owes_are_reached_without_an_earlier_call() {
    let row = Aged {
        case: "a month's purchase a day past its end date",
        requested: renewing_terms("P1M"),
        signed_ago: days(31),
        renewals: 1,
    };
    let proposal = "ap-unsettledchildren";

    let (probe, source) = aged_store(&row, proposal).await;
    let chain = chain_after_an_unrelated_read(&probe, &source).await;
    assert_eq!(
        chain.len(),
        row.renewals + 1,
        "{}: expected the source and {} renewal(s), got {chain:?}",
        row.case,
        row.renewals
    );
    let renewal = chain
        .last()
        .cloned()
        .expect("the chain holds at least the source");
    let invoices = invoices_of(&probe, &renewal).await;
    let invoice = invoices.first().cloned().unwrap_or_else(|| {
        panic!("the renewal {renewal} is billed its prepaid charge, got {invoices:?}")
    });
    let charges_of_renewal = charges(&probe, json!({"agreementId": &renewal})).await;
    assert_eq!(
        charges_of_renewal.status, 200,
        "listing the charges of {renewal} must succeed, got {} with {:?}",
        charges_of_renewal.status, charges_of_renewal.body
    );
    let charge = items(&charges_of_renewal)
        .first()
        .and_then(|charge| charge["id"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!(
                "the renewal {renewal} holds charges of its own, got {:?}",
                charges_of_renewal.body
            )
        });

    // The batch, as the FIRST call its store sees: the invoice it adjusts is an
    // invoice of the renewal, so the entry is created rather than reported as a
    // reference to something the store does not hold.
    let (unsettled, same) = aged_store(&row, proposal).await;
    assert_eq!(
        same, source,
        "the identical setup must mint the identical agreement id"
    );
    let submitted = submit(
        &unsettled,
        vec![entry(&renewal, &invoice, "1", "tok-unsettled-renewal")],
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "case \"BatchCreateBillingAdjustmentRequest\": the batch must be served, got {} with {:?}",
        submitted.status, submitted.body
    );
    assert_eq!(
        submitted.body["errors"].as_array().map(Vec::len),
        Some(0),
        "case \"BatchCreateBillingAdjustmentRequest\": the invoice of the renewal {renewal} is an \
         invoice this store holds, got {:?}",
        submitted.body
    );
    let created = submitted.str_at("/items/0/billingAdjustmentRequestId");
    assert!(
        !created.is_empty(),
        "case \"BatchCreateBillingAdjustmentRequest\": the entry must create a request, got {:?}",
        submitted.body
    );

    // The charge report, as the FIRST call ITS store sees: the charges of the
    // renewal are charges of the store from that end date on, so they are
    // reported without any earlier call having settled anything.
    let (also_unsettled, same) = aged_store(&row, proposal).await;
    assert_eq!(
        same, source,
        "the identical setup must mint the identical agreement id"
    );
    let first_read = charges(&also_unsettled, json!({"agreementId": &renewal})).await;
    assert_eq!(
        first_read.status, 200,
        "case \"ListAgreementCharges\": the renewal {renewal} is an agreement this store holds, \
         got {} with {:?}",
        first_read.status, first_read.body
    );
    let reported: Vec<String> = items(&first_read)
        .iter()
        .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        reported.contains(&charge),
        "case \"ListAgreementCharges\": expected the charge {charge} of the renewal {renewal}, \
         got {reported:?}"
    );
    // And the association that charge carries is the one its own agreement's
    // acceptance recorded — none here, since nothing associated anything.
    let carried = references(&also_unsettled, &renewal).await;
    assert_eq!(
        carried.get(&charge),
        Some(&None),
        "case \"ListAgreementCharges\": expected charge {charge} to carry no association, got \
         {carried:?}"
    );
}
