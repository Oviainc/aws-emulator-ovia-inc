// SPDX-License-Identifier: CC0-1.0

//! What an ACCEPTED delegated-administrator call that changes nothing leaves behind,
//! end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite) and the helpers that drive
//! the three delegated-administrator operations are shared with the
//! `organization_admin_accounts` suite, so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rule under test is the one the `no_op_mutation_clock` suite asserts over the
//! detector's sub-resources, here over the SET of ACCEPTED calls on the
//! organization's delegated administrator that still write nothing: a mutating
//! operation reads the service's monotonic "this resource changed now" clock only on
//! the path that actually writes, so a call which leaves the store exactly as it
//! found it consumes no instant either — and the `CreatedAt`/`UpdatedAt` a LATER
//! accepted call stamps therefore still reports the wall-clock instant of that
//! change rather than one pushed into the future by the no-ops before it.
//!
//! Observable per request: that clock is state-wide, so what a mutation reads is what
//! the detector's own `UpdatedAt` records when a sub-resource is added to it
//! (`smithy_query(operation="GetDetector")` models it as a String). A burst of
//! accepted no-op administrator calls followed by one accepted create is therefore
//! enough to see whether the no-ops consumed instants: the create's stamp may sit no
//! further ahead than the wall clock the burst really took, whereas a handler that
//! reads the clock before it knows it is going to write spends an instant per call on
//! top of that and stamps an instant in the future.
//!
//! The rows differ along what the rule generalises over: the operation repeated
//! (the removal and the designation), the state the record is in when the burst runs
//! (a removal in flight, and an administrator settled under the enabled status), how
//! the record reached that state (designated, removed, removed and turned around,
//! removed again), and the Region the designation was made through.

use serde_json::json;

use crate::detector_lifecycle::{call, call_in, enabled_detector, new_router, Router};
use crate::organization_admin_accounts::{
    assert_reports, disable_admin, enable_admin, DISABLE_IN_PROGRESS, ENABLED,
};
use crate::refused_create_clock::{as_millis, detector_updated_at, READING_SLACK_MILLIS};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    now_millis, NO_ORGANIZATION_ADMIN_MESSAGE,
};

/// How many no-op calls one burst sends. A call that writes nothing consumes no
/// instant, so the burst may be any size; it is this large because a handler that
/// reads the clock BEFORE it knows whether it writes spends at least one millisecond
/// of it per call, and the burst must push that clock more than
/// [`READING_SLACK_MILLIS`] past the wall clock for the difference to be an
/// assertion.
const NO_OPS_PER_BURST: usize = 300;

/// One row: a designation to make, the calls that put its record into the state the
/// burst runs against, and the accepted call the burst then repeats.
struct Row {
    case: &'static str,
    /// The Region the designation is made through, one per row, so no two rows share
    /// a partition of the administrator store.
    region: &'static str,
    admin_account_id: &'static str,
    /// The `/admin/{segment}` calls made ONCE after the designation, each of which
    /// may write; the burst below is what must not.
    prelude: &'static [&'static str],
    /// The segment the burst POSTs to, over and over, with the row's account id.
    burst_segment: &'static str,
    /// The `AdminStatus` a read reports while the burst runs
    /// (`smithy_query(shape="AdminStatus")`).
    reported_status: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a removal repeated while the removal it repeats is still in flight",
            region: "us-east-1",
            admin_account_id: "111122223333",
            prelude: &["disable"],
            burst_segment: "disable",
            reported_status: DISABLE_IN_PROGRESS,
        },
        Row {
            case: "a designation repeated for the account the Region already holds",
            region: "eu-west-1",
            admin_account_id: "999999999999",
            prelude: &[],
            burst_segment: "enable",
            reported_status: ENABLED,
        },
        Row {
            case: "a designation repeated after it turned a pending removal around",
            region: "ap-southeast-1",
            admin_account_id: "012345678901",
            prelude: &["disable", "enable"],
            burst_segment: "enable",
            reported_status: ENABLED,
        },
        Row {
            case: "a removal repeated after a turned-around removal was asked for again",
            region: "zz-invented-5",
            admin_account_id: "271828182845",
            prelude: &["disable", "enable", "disable"],
            burst_segment: "disable",
            reported_status: DISABLE_IN_PROGRESS,
        },
    ]
}

/// Designate `admin_account_id` through `region` again and ask for its removal once,
/// so a burst of repeated removals whose span fell due mid-burst has a pending
/// removal to repeat again. The removal is a REAL write, so the caller counts it.
async fn reprime_pending_removal(
    router: &Router,
    region: &str,
    admin_account_id: &str,
    case: &str,
) {
    let (status, body) = enable_admin(router, region, admin_account_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the re-designation, got {status} {body:?}"
    );
    let (status, body) = disable_admin(router, region, admin_account_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the removal asked for again, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The rule: an accepted administrator call that writes nothing consumes no instant,
// so the next accepted mutation's timestamp is still a wall-clock instant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accepted_no_op_administrator_calls_leave_the_clock_where_the_wall_clock_is() {
    for row in rows() {
        let case = row.case;
        // A router per row, so a row cannot pass or fail on another row's writes, and
        // a detector to carry the timestamp the state-wide clock is read through.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        let (status, body) = enable_admin(&router, row.region, row.admin_account_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the designation, got {status} {body:?}"
        );
        for segment in row.prelude {
            let (status, body) = call_in(
                &router,
                row.region,
                "POST",
                &format!("/admin/{segment}"),
                Some(json!({"adminAccountId": row.admin_account_id})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from the {segment:?} that sets the row up, got \
                 {status} {body:?}"
            );
        }
        assert_reports(
            &router,
            row.region,
            row.admin_account_id,
            row.reported_status,
            case,
        )
        .await;

        // The instant the last ACCEPTED write stamped, and the wall clock just after
        // it: from here to the accepted create below, every call the service sees
        // leaves the store as it found it apart from the re-primes counted below, so
        // the clock may advance only by the time that really passes.
        let before_burst = detector_updated_at(&router, &detector_id, case).await;
        let wall_before = now_millis();

        let burst_path = format!("/admin/{}", row.burst_segment);
        let body = json!({"adminAccountId": row.admin_account_id});
        // A removal's span is not zero and not infinite, so a burst of repeated
        // removals can outlive the one it repeats: when it does, the record is gone,
        // the next call is REFUSED, and the row re-primes a pending removal to go on
        // repeating. Each re-prime is one real write, so each is one instant the
        // measurement below allows.
        let mut reprimes: i64 = 0;
        for attempt in 0..NO_OPS_PER_BURST {
            let (status, answered) =
                call_in(&router, row.region, "POST", &burst_path, Some(body.clone())).await;
            // The ONE refusal a removal that outlived its span is answered with: the
            // Region holds no delegated administrator any more. Any other refusal is a
            // repeat that was not accepted, which is the thing this row asserts, so it
            // falls through to the 200 below and fails the case.
            let span_fell_due = status == 400
                && answered["message"].as_str() == Some(NO_ORGANIZATION_ADMIN_MESSAGE);
            if span_fell_due {
                reprime_pending_removal(&router, row.region, row.admin_account_id, case).await;
                reprimes += 1;
                continue;
            }
            assert_eq!(
                status, 200,
                "case {case:?}: no-op call {attempt} must answer 200, got {status} {answered:?}"
            );
        }

        // A call that changes nothing changes nothing about the record either: it
        // still reports the same account under the same status.
        assert_reports(
            &router,
            row.region,
            row.admin_account_id,
            row.reported_status,
            case,
        )
        .await;

        // Nor about the detector, which the administrator operations do not name at
        // all.
        let after_burst = detector_updated_at(&router, &detector_id, case).await;
        assert_eq!(
            after_burst, before_burst,
            "case {case:?}: {NO_OPS_PER_BURST} no-op administrator calls must not move the \
             detector's updatedAt, expected {before_burst:?}, got {after_burst:?}"
        );

        // The accepted create right after the burst: it writes, so it stamps the
        // instant it happened — and that instant is a WALL-CLOCK instant, not one the
        // no-ops pushed into the future.
        let probe_collection = format!("/detector/{detector_id}/ipset");
        let (status, created) = call(
            &router,
            "POST",
            &probe_collection,
            Some(json!({
                "name": "probe-ip-set",
                "format": "TXT",
                "location": "s3://guardduty-lists/probe-ip-set.txt",
                "activate": false,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the probing create to answer 200, got {status} {created:?}"
        );
        let minted = created["ipSetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ipSetId, got {created:?}"))
            .to_string();

        let wall_after = now_millis();
        let stamped = detector_updated_at(&router, &detector_id, case).await;
        assert!(
            stamped > after_burst,
            "case {case:?}: the accepted create changed the detector, so its updatedAt must move \
             past {after_burst:?}, got {stamped:?}"
        );
        let allowance = READING_SLACK_MILLIS + reprimes;
        // The stamp is an instant that really happened: it sits at the wall clock, not
        // ahead of it, because the no-op calls before it consumed nothing.
        let stamped_millis = as_millis(&stamped, case);
        assert!(
            stamped_millis <= wall_after + allowance,
            "case {case:?}: {NO_OPS_PER_BURST} no-op administrator calls consume no instant, so \
             the create after them stamps a wall-clock instant at most {allowance} ms past \
             {wall_after}, got {stamped_millis} ({stamped:?})"
        );
        // And the same measurement the sibling clock suites make: how far the clock
        // moved across the burst, against how far the WALL clock moved over the same
        // span.
        let elapsed = wall_after - wall_before;
        let advance = stamped_millis - as_millis(&before_burst, case);
        assert!(
            advance <= elapsed + allowance,
            "case {case:?}: the clock may advance only by the {elapsed} ms that really passed \
             (plus {allowance} ms of slack), got {advance} ms (from {before_burst:?} to \
             {stamped:?})"
        );

        // Round trip: the IPSet the probing create minted is one a read resolves, so
        // the timestamp asserted above records a change that really landed.
        let (status, read_back) =
            call(&router, "GET", &format!("{probe_collection}/{minted}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a read of the minted ipSetId to answer 200, got {status} \
             {read_back:?}"
        );
    }
}
