// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `StartOutpostDecommission`, driven over the wire through the
//! same front door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite
//! reuses) — so what it asserts is the emulator's HTTP behaviour, not a handler called
//! directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A decommission is STARTED once per Outpost: the call that starts it answers
//!    `REQUESTED` and records the request on the Outpost, and every later call answers
//!    `SKIPPED` because there is nothing left to start — whichever of the two forms the
//!    modelled `OutpostIdentifier` pattern accepts (bare id, full Outpost ARN) each
//!    call names the Outpost by.
//! 2. `ValidateOnly` validates the request and starts nothing: it answers `SKIPPED` and
//!    leaves the Outpost as it was, so a later call still STARTS the decommission — for
//!    any number of validate-only calls.
//! 3. A resource still associated with the Outpost blocks the decommission: while the
//!    store holds an order submitted for THIS Outpost, every call answers `BLOCKED`
//!    with that resource's `BlockingResourceType` and starts nothing. A quote in any
//!    other status, a quote of another Outpost, and a quote associated with no Outpost
//!    block nothing, and a call that is not blocked reports no `BlockingResourceTypes`
//!    at all.
//! 4. Starting a decommission is not a delete: the Outpost stays readable, exactly as
//!    it was.
//! 5. An `OutpostIdentifier` that names no Outpost is 404 `NotFoundException` with the
//!    identifier echo of the value AS SUPPLIED; one that is not an Outpost identifier
//!    at all is 400 `ValidationException`.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    identifier_not_found, DECOMMISSION_STATUS_BLOCKED, DECOMMISSION_STATUS_REQUESTED,
    DECOMMISSION_STATUS_SKIPPED, OUTPOST_IDENTIFIER_MEMBER, QUOTE_STATUS_CREATED,
    QUOTE_STATUS_EXPIRED, QUOTE_STATUS_ORDER_SUBMITTED,
};

use crate::quote_lifecycle::{create_quote, request};
use crate::site_and_outpost_lifecycle::{
    refusal, segment, settle, text, Outposts, ABSENT_OUTPOST_ID, ABSENT_SITE_ID, OUTPOST_ARN_PREFIX,
};

/// The `BlockingResourceType` an outstanding Outpost order is reported as, from
/// `smithy_query(query="shape",shape="BlockingResourceType")`'s value list.
const OUTPOST_ORDER_CANCELLABLE: &str = "OUTPOST_ORDER_CANCELLABLE";

/// The decommission route of one Outpost, by either identifier form.
fn decommission_route(identifier: &str) -> String {
    format!("/outposts/{}/decommission", segment(identifier))
}

/// `StartOutpostDecommission` on `identifier` with `body` as the payload, requiring
/// HTTP 200 and returning the whole output.
async fn decommission(outposts: &Outposts, identifier: &str, body: &Value) -> Value {
    outposts
        .ok("POST", &decommission_route(identifier), Some(body))
        .await
}

/// The `Status` of a `StartOutpostDecommission` answer.
fn status(output: &Value) -> String {
    text(output, "Status")
}

/// Move a stored quote into `quote_status`.
///
/// This is state a later slice of the API produces — an order submitted from a quote
/// (`CreateOrder` is still a scaffold stub) — reached through the shared engine, the
/// way [`Outposts::store`] exists to be reached. Every assertion about behaviour still
/// goes over the wire.
fn set_quote_status(outposts: &Outposts, quote_id: &str, quote_status: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let quote = store
        .quotes
        .get_mut(quote_id)
        .unwrap_or_else(|| panic!("the quote {quote_id} this case created is stored"));
    quote.quote.quote_status = Some(quote_status.to_string());
}

/// `CreateQuote` for `outpost_identifier` (or for no Outpost at all), returning the
/// minted `QuoteId`. The body is [`crate::quote_lifecycle`]'s: the two members the
/// model marks `@required`, plus the optional association.
async fn a_quote(outposts: &Outposts, outpost_identifier: Option<&str>) -> String {
    let association = match outpost_identifier {
        Some(identifier) => json!({ "OutpostIdentifier": identifier }),
        None => json!({}),
    };
    let quote = create_quote(outposts, &request("US", json!([]), association)).await;
    text(&quote, "QuoteId")
}

/// Rules 1 and 4: the first call that starts a decommission answers `REQUESTED`, every
/// later call answers `SKIPPED` whichever identifier form it names the Outpost by, and
/// the Outpost itself is untouched by all of it.
///
/// The rows differ along what the rule generalises over: which identifier form starts
/// the decommission, which form the later calls name it by, and the form the
/// `ValidateOnly` member arrives in (omitted, so the modelled default `false` applies,
/// or sent explicitly).
#[tokio::test]
async fn a_decommission_is_started_once_and_every_later_call_is_skipped() {
    let outposts = Outposts::new();
    // (case, index of the form the start names, index of the form the repeat names,
    //  the body the start arrives with)
    let cases: Vec<(&str, usize, usize, Value)> = vec![
        ("started by bare id, ValidateOnly omitted", 0, 0, json!({})),
        (
            "started by bare id, ValidateOnly sent as false, repeated by ARN",
            0,
            1,
            json!({ "ValidateOnly": false }),
        ),
        (
            "started by ARN, repeated by bare id",
            1,
            0,
            json!({ "ValidateOnly": false }),
        ),
        ("started by ARN, repeated by ARN", 1, 1, json!({})),
    ];

    // One Outpost per row, all created before the wait below so the whole suite pays
    // for the lifecycle transition once: rule 4 compares the Outpost before and after,
    // which is only a statement about the decommission once the clock owes it nothing.
    let mut outposts_under_test = Vec::new();
    for (case, ..) in &cases {
        outposts_under_test.push(outposts.site_and_outpost(case).await);
    }
    settle();

    for ((case, start_form, repeat_form, body), (outpost_id, outpost_arn)) in
        cases.iter().zip(&outposts_under_test)
    {
        // The two forms the modelled `OutpostIdentifier` pattern accepts for one
        // Outpost: the bare id, and the full Outpost ARN.
        let forms = [outpost_id.as_str(), outpost_arn.as_str()];
        let before = outposts.get_outpost(outpost_id).await;

        let started = decommission(&outposts, forms[*start_form], body).await;
        assert_eq!(
            status(&started),
            DECOMMISSION_STATUS_REQUESTED,
            "case {case:?}: the call that starts the decommission must answer \
             {DECOMMISSION_STATUS_REQUESTED}, got {started:?}"
        );
        assert_eq!(
            started.get("BlockingResourceTypes"),
            None,
            "case {case:?}: nothing blocks, so no blocking resource type is reported, \
             got {started:?}"
        );

        // Rule 1: the request was recorded, so there is nothing left to start — for a
        // repeat that asks to start it, and for one that only validates.
        for repeat_body in [json!({}), json!({ "ValidateOnly": true })] {
            let repeated = decommission(&outposts, forms[*repeat_form], &repeat_body).await;
            assert_eq!(
                status(&repeated),
                DECOMMISSION_STATUS_SKIPPED,
                "case {case:?}: a later call with {repeat_body} must answer \
                 {DECOMMISSION_STATUS_SKIPPED}, got {repeated:?}"
            );
            assert_eq!(
                repeated.get("BlockingResourceTypes"),
                None,
                "case {case:?}: nothing blocks a later call either, got {repeated:?}"
            );
        }

        // Rule 4: starting a decommission is not a delete, and moves nothing the
        // Outpost reports.
        let after = outposts.get_outpost(outpost_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: the Outpost must read back exactly as it was, got {after:?}"
        );
    }
}

/// Rule 2: a `ValidateOnly` call answers `SKIPPED` and starts nothing, so the next
/// call that does not ask to validate still STARTS the decommission.
///
/// The rows differ along how many validate-only calls precede the real one and which
/// identifier form they name the Outpost by — the rule holds for any number of them.
#[tokio::test]
async fn validate_only_answers_skipped_and_starts_nothing() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, usize, usize)> = vec![
        ("one validate-only call, by bare id", 1, 0),
        ("two validate-only calls, by bare id", 2, 0),
        ("three validate-only calls, by ARN", 3, 1),
    ];

    for (case, validations, form) in &cases {
        let (outpost_id, outpost_arn) = outposts.site_and_outpost(case).await;
        let forms = [outpost_id.as_str(), outpost_arn.as_str()];
        for attempt in 1..=*validations {
            let validated =
                decommission(&outposts, forms[*form], &json!({ "ValidateOnly": true })).await;
            assert_eq!(
                status(&validated),
                DECOMMISSION_STATUS_SKIPPED,
                "case {case:?}: validate-only call {attempt} must answer \
                 {DECOMMISSION_STATUS_SKIPPED}, got {validated:?}"
            );
        }
        let started = decommission(&outposts, forms[*form], &json!({})).await;
        assert_eq!(
            status(&started),
            DECOMMISSION_STATUS_REQUESTED,
            "case {case:?}: after {validations} validate-only call(s) the decommission \
             must still start, got {started:?}"
        );
    }
}

/// Rule 3: an order still outstanding on the Outpost blocks the decommission, and
/// nothing else in the store does.
///
/// The rows differ along the quote state the store holds: none at all, each modelled
/// `QuoteStatus` on a quote of the Outpost being decommissioned, and an order
/// submitted from a quote that names another Outpost or no Outpost at all. A blocked
/// call starts nothing, which the follow-up call in each row is what shows.
#[tokio::test]
async fn an_outstanding_order_blocks_the_decommission_and_nothing_else_does() {
    // (case, the quote's association, the quote's status, the expected answer)
    enum Association {
        None,
        ThisOutpost,
        AnotherOutpost,
        NoOutpost,
    }
    let cases: Vec<(&str, Association, &str, &str)> = vec![
        (
            "no quote at all",
            Association::None,
            QUOTE_STATUS_CREATED,
            DECOMMISSION_STATUS_REQUESTED,
        ),
        (
            "a quote of this Outpost still CREATED",
            Association::ThisOutpost,
            QUOTE_STATUS_CREATED,
            DECOMMISSION_STATUS_REQUESTED,
        ),
        (
            "a quote of this Outpost that has EXPIRED",
            Association::ThisOutpost,
            QUOTE_STATUS_EXPIRED,
            DECOMMISSION_STATUS_REQUESTED,
        ),
        (
            "an order submitted from a quote of this Outpost",
            Association::ThisOutpost,
            QUOTE_STATUS_ORDER_SUBMITTED,
            DECOMMISSION_STATUS_BLOCKED,
        ),
        (
            "an order submitted from a quote of ANOTHER Outpost",
            Association::AnotherOutpost,
            QUOTE_STATUS_ORDER_SUBMITTED,
            DECOMMISSION_STATUS_REQUESTED,
        ),
        (
            "an order submitted from a quote associated with no Outpost",
            Association::NoOutpost,
            QUOTE_STATUS_ORDER_SUBMITTED,
            DECOMMISSION_STATUS_REQUESTED,
        ),
    ];

    for (case, association, quote_status, expected) in &cases {
        // A store of its own per row: what blocks a decommission is what the store
        // holds, so each row starts from an empty one.
        let outposts = Outposts::new();
        let (outpost_id, _) = outposts.site_and_outpost(case).await;
        let (other_outpost_id, _) = outposts.site_and_outpost("another-outpost").await;
        let quote_id = match association {
            Association::None => None,
            Association::ThisOutpost => Some(a_quote(&outposts, Some(&outpost_id)).await),
            Association::AnotherOutpost => Some(a_quote(&outposts, Some(&other_outpost_id)).await),
            Association::NoOutpost => Some(a_quote(&outposts, None).await),
        };
        if let Some(quote_id) = &quote_id {
            set_quote_status(&outposts, quote_id, quote_status);
        }

        let answered = decommission(&outposts, &outpost_id, &json!({})).await;
        assert_eq!(
            status(&answered),
            *expected,
            "case {case:?}: expected Status {expected}, got {answered:?}"
        );
        let blocking = if *expected == DECOMMISSION_STATUS_BLOCKED {
            Some(json!([OUTPOST_ORDER_CANCELLABLE]))
        } else {
            None
        };
        assert_eq!(
            answered.get("BlockingResourceTypes"),
            blocking.as_ref(),
            "case {case:?}: expected BlockingResourceTypes {blocking:?}, got {answered:?}"
        );

        // What the first call did: a blocked call started nothing, so once the order
        // is gone the decommission still STARTS; an unblocked call did start it, so
        // the next one has nothing left to start.
        let expected_follow_up = if *expected == DECOMMISSION_STATUS_BLOCKED {
            // The order is no longer outstanding — the state a cancelled order leaves
            // the quote in is not this suite's subject, so the row simply takes the
            // quote back out of ORDER_SUBMITTED.
            let quote_id = quote_id
                .as_deref()
                .expect("a blocked row always seeded a quote");
            set_quote_status(&outposts, quote_id, QUOTE_STATUS_CREATED);
            DECOMMISSION_STATUS_REQUESTED
        } else {
            DECOMMISSION_STATUS_SKIPPED
        };
        let follow_up = decommission(&outposts, &outpost_id, &json!({})).await;
        assert_eq!(
            status(&follow_up),
            expected_follow_up,
            "case {case:?}: expected the next call to answer {expected_follow_up}, \
             got {follow_up:?}"
        );
    }
}

/// Rule 5: an `OutpostIdentifier` that names no Outpost is 404 `NotFoundException`
/// with the identifier echo of the value as supplied; one that is not an Outpost
/// identifier at all is 400 `ValidationException`. Nothing is started in either case.
#[tokio::test]
async fn an_identifier_that_names_no_outpost_is_refused() {
    let outposts = Outposts::new();
    let absent_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let cases = vec![
        refusal(
            "an Outpost id this emulator never minted",
            "POST",
            decommission_route(ABSENT_OUTPOST_ID),
            404,
            "NotFoundException",
        )
        .with_body(json!({})),
        refusal(
            "the ARN form of an Outpost that does not exist",
            "POST",
            decommission_route(&absent_arn),
            404,
            "NotFoundException",
        )
        .with_body(json!({})),
        refusal(
            "an absent Outpost, validate-only",
            "POST",
            decommission_route(ABSENT_OUTPOST_ID),
            404,
            "NotFoundException",
        )
        .with_body(json!({ "ValidateOnly": true })),
        refusal(
            "not an identifier of this API at all",
            "POST",
            decommission_route("not-an-outpost"),
            400,
            "ValidationException",
        )
        .with_body(json!({})),
        refusal(
            "a SITE identifier where an Outpost identifier belongs",
            "POST",
            decommission_route(ABSENT_SITE_ID),
            400,
            "ValidationException",
        )
        .with_body(json!({})),
        refusal(
            "an Outpost id one hex character short",
            "POST",
            decommission_route("op-0000000000000000"),
            400,
            "ValidationException",
        )
        .with_body(json!({})),
    ];
    outposts.expect_refusals(&cases).await;

    // The 404 echoes the member the identifier arrived in and the value AS SUPPLIED,
    // so the ARN form reads its ARN back rather than the bare id it resolves to.
    for supplied in [ABSENT_OUTPOST_ID, absent_arn.as_str()] {
        let reply = outposts
            .call("POST", &decommission_route(supplied), Some(&json!({})))
            .await;
        let expected = identifier_not_found(OUTPOST_IDENTIFIER_MEMBER, supplied);
        assert_eq!(
            text(&reply.body, "Message"),
            expected,
            "case {supplied:?}: expected $.Message {expected:?}, got {:?}",
            reply.body
        );
    }
}
