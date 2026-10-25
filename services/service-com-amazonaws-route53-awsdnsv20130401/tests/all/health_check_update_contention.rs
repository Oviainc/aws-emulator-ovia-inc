// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateHealthCheck` under CONTENTION.
//!
//! The rule under test: a version-gated update is a compare-and-set, so the
//! comparison of the presented `HealthCheckVersion` against the stored one and
//! the write that follows it happen as one indivisible step. Of any number of
//! updates issued at once that all present the SAME current version, exactly one
//! is applied — HTTP 200, the health check one version on, carrying that
//! request's settings — and every other one is refused `HealthCheckVersionMismatch`
//! (HTTP 409) and changes nothing; the stored counter therefore advances by
//! exactly one per round of contention, never by the number of writers and never
//! not at all. An update that presents no version asks for no such gate, so
//! concurrent unconditional updates are ALL applied, each taking its own
//! consecutive version: the counter counts applied updates, so no increment is
//! lost to a read-modify-write race either.
//!
//! The suites below therefore vary the number of writers in flight, the number
//! of rounds of contention, and whether the writers present the gate at all,
//! rather than repeating one five-thread race. Each writer runs on its own OS
//! thread, released from a barrier, so the requests genuinely overlap instead of
//! interleaving at an await point, and every outcome is proved by reading the
//! health check back through `GetHealthCheck`.

use std::sync::Barrier;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::FIRST_HEALTH_CHECK_VERSION;

use crate::health_check_updates::{reported_version, update_health_check};
use crate::health_checks::{create_health_check, get_health_check, health_check_id};
use crate::hosted_zone_records::{router, Reply};

/// The error code a presented version that is not the stored one is refused
/// with: `HealthCheckVersionMismatch`, HTTP 409.
const VERSION_MISMATCH: &str = "HealthCheckVersionMismatch";

/// The refusal message that error carries, which is the shape's own
/// documentation sentence ("The value of HealthCheckVersion in the request
/// doesn't match the value of HealthCheckVersion in the health check."), named
/// here so the wording a loser is refused with is asserted and not merely its
/// code.
const VERSION_MISMATCH_MESSAGE: &str =
    "The value of HealthCheckVersion in the request doesn't match the value of \
     HealthCheckVersion in the health check.";

/// The `HealthCheckConfig` the health check every suite here contends over is
/// created with: an endpoint check whose `Port` and `ResourcePath` an update can
/// edit, so each writer can ask for a DIFFERENT setting and the health check
/// shows afterwards which writer won.
const CONTENDED_CONFIG: &str = "<Type>HTTP</Type>\
     <FullyQualifiedDomainName>contended.example.com</FullyQualifiedDomainName>\
     <Port>80</Port><ResourcePath>/</ResourcePath>\
     <RequestInterval>30</RequestInterval><FailureThreshold>3</FailureThreshold>";

/// Create the health check this suite contends over and answer its id.
async fn contended_health_check(
    router: &MultiServiceRouter,
    caller_reference: &str,
    case: &str,
) -> String {
    let created = create_health_check(router, caller_reference, CONTENDED_CONFIG).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected 201 from CreateHealthCheck, got {} with body {}",
        created.status, created.body
    );
    health_check_id(&created, case)
}

/// Send one `UpdateHealthCheck` per entry of `bodies`, each from its own OS
/// thread and all released from one barrier, and answer the replies in the order
/// of `bodies`.
///
/// Separate threads rather than tasks on one runtime: what is under test is a
/// race between two requests that each read the stored version and then write,
/// which only genuinely overlapping requests can expose.
fn updates_in_flight_together(
    router: &MultiServiceRouter,
    id: &str,
    bodies: &[String],
) -> Vec<Reply> {
    let barrier = Barrier::new(bodies.len());
    std::thread::scope(|scope| {
        let writers: Vec<_> = bodies
            .iter()
            .map(|body| {
                let router = router.clone();
                let barrier = &barrier;
                let id = id.to_string();
                let body = body.clone();
                scope.spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("each writer must get a runtime of its own");
                    // Built BEFORE the barrier, so what is released at once is
                    // the request rather than the runtime's construction.
                    barrier.wait();
                    runtime.block_on(update_health_check(&router, &id, &body))
                })
            })
            .collect();
        writers
            .into_iter()
            .map(|writer| writer.join().expect("a writer must not panic"))
            .collect()
    })
}

/// The index of the single reply that was applied, asserting that there is
/// exactly one and that every other reply is the documented refusal.
fn the_one_winner(replies: &[Reply], case: &str) -> usize {
    let statuses: Vec<u16> = replies.iter().map(|reply| reply.status).collect();
    let winners: Vec<usize> = replies
        .iter()
        .enumerate()
        .filter(|(_, reply)| reply.status == 200)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "case {case:?}: expected exactly one winner, got {} (statuses {statuses:?})",
        winners.len()
    );
    for (index, reply) in replies.iter().enumerate() {
        if index == winners[0] {
            continue;
        }
        assert_eq!(
            reply.status, 409,
            "case {case:?}: writer {index} expected HTTP 409, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            VERSION_MISMATCH,
            "case {case:?}: writer {index} expected {VERSION_MISMATCH}, got {:?}; body {}",
            reply.error_code(),
            reply.body
        );
        let message = reply.first("Error/Message").unwrap_or("");
        assert_eq!(
            message, VERSION_MISMATCH_MESSAGE,
            "case {case:?}: writer {index} expected the refusal message \
             {VERSION_MISMATCH_MESSAGE:?}, got {message:?}; body {}",
            reply.body
        );
    }
    winners[0]
}

/// The `ResourcePath` a reply reports for the health check.
fn reported_resource_path(reply: &Reply) -> &str {
    reply.first("HealthCheckConfig/ResourcePath").unwrap_or("")
}

#[tokio::test]
async fn exactly_one_update_presenting_the_current_version_is_applied() {
    // The writer count is what the rule generalises over: whatever the number of
    // requests presenting one version, one of them is the update and the rest
    // are refusals.
    for writers in [2usize, 3, 5, 8] {
        let case = format!("{writers} writers presenting one version");
        let router = router();
        let id = contended_health_check(&router, &format!("contended-{writers}"), &case).await;

        let before = get_health_check(&router, &id).await;
        let base = reported_version(&before, &case);
        assert_eq!(
            base, FIRST_HEALTH_CHECK_VERSION,
            "case {case:?}: a freshly created health check must report \
             HealthCheckVersion {FIRST_HEALTH_CHECK_VERSION}, got {base}; body {}",
            before.body
        );

        // Each writer asks for a DIFFERENT ResourcePath, so the health check
        // afterwards names which one was applied.
        let bodies: Vec<String> = (0..writers)
            .map(|writer| {
                format!(
                    "<HealthCheckVersion>{base}</HealthCheckVersion>\
                     <ResourcePath>/contended-{writer}</ResourcePath>"
                )
            })
            .collect();
        let replies = updates_in_flight_together(&router, &id, &bodies);

        let winner = the_one_winner(&replies, &case);
        let applied = reported_version(&replies[winner], &case);
        assert_eq!(
            applied,
            base + 1,
            "case {case:?}: the applied update must report HealthCheckVersion {}, got {applied}; \
             body {}",
            base + 1,
            replies[winner].body
        );
        let expected_path = format!("/contended-{winner}");
        let answered = reported_resource_path(&replies[winner]);
        assert_eq!(
            answered, expected_path,
            "case {case:?}: the applied update must answer ResourcePath {expected_path:?}, \
             got {answered:?}; body {}",
            replies[winner].body
        );

        // The store holds exactly the winner's update, one version on: no
        // loser's settings landed, and the counter did not move per writer.
        let after = get_health_check(&router, &id).await;
        let stored_version = reported_version(&after, &case);
        assert_eq!(
            stored_version,
            base + 1,
            "case {case:?}: the health check must stand at HealthCheckVersion {}, \
             got {stored_version}; body {}",
            base + 1,
            after.body
        );
        let stored_path = reported_resource_path(&after);
        assert_eq!(
            stored_path, expected_path,
            "case {case:?}: the health check must hold ResourcePath {expected_path:?}, \
             got {stored_path:?}; body {}",
            after.body
        );
    }
}

#[tokio::test]
async fn each_round_of_contention_moves_the_version_on_by_exactly_one() {
    // Round after round, the counter counts the updates that were APPLIED: the
    // version a round's winner leaves behind is the only token the next round
    // can present, so a read-modify-write loop makes exactly one step per round.
    const ROUNDS: i64 = 4;
    const WRITERS: usize = 5;

    let router = router();
    let case = "rounds of contention";
    let id = contended_health_check(&router, "contended-rounds", case).await;
    let mut held = reported_version(&get_health_check(&router, &id).await, case);

    for round in 0..ROUNDS {
        let round_case = format!("{case}: round {round}");
        let bodies: Vec<String> = (0..WRITERS)
            .map(|writer| {
                format!(
                    "<HealthCheckVersion>{held}</HealthCheckVersion>\
                     <ResourcePath>/round-{round}-{writer}</ResourcePath>"
                )
            })
            .collect();
        let replies = updates_in_flight_together(&router, &id, &bodies);
        let winner = the_one_winner(&replies, &round_case);

        let after = get_health_check(&router, &id).await;
        let moved = reported_version(&after, &round_case);
        assert_eq!(
            moved,
            held + 1,
            "case {round_case:?}: expected HealthCheckVersion {}, got {moved}; body {}",
            held + 1,
            after.body
        );
        let expected_path = format!("/round-{round}-{winner}");
        let stored_path = reported_resource_path(&after);
        assert_eq!(
            stored_path, expected_path,
            "case {round_case:?}: expected ResourcePath {expected_path:?}, got {stored_path:?}; \
             body {}",
            after.body
        );
        held = moved;
    }

    let expected = FIRST_HEALTH_CHECK_VERSION + ROUNDS;
    assert_eq!(
        held, expected,
        "case {case:?}: after {ROUNDS} rounds of {WRITERS} writers the health check must stand \
         at HealthCheckVersion {expected}, got {held}"
    );
}

#[tokio::test]
async fn concurrent_updates_presenting_no_version_are_all_applied_once_each() {
    // "We recommend that you use GetHealthCheck ... and that you include that
    // value in your UpdateHealthCheck request": the member is optional, so a
    // request that carries none asks for no gate and is applied unconditionally.
    // Every one of them is therefore applied, and each takes its OWN consecutive
    // version — which is what shows the counter is advanced inside the same
    // critical section as the write rather than read, incremented and written
    // back.
    const WRITERS: usize = 6;

    let router = router();
    let case = "writers presenting no version";
    let id = contended_health_check(&router, "contended-unconditional", case).await;
    let base = reported_version(&get_health_check(&router, &id).await, case);

    let bodies: Vec<String> = (0..WRITERS)
        .map(|writer| format!("<Port>{}</Port>", 8000 + writer))
        .collect();
    let replies = updates_in_flight_together(&router, &id, &bodies);

    let mut applied: Vec<i64> = Vec::new();
    for (index, reply) in replies.iter().enumerate() {
        assert_eq!(
            reply.status, 200,
            "case {case:?}: writer {index} expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        applied.push(reported_version(reply, case));
    }
    applied.sort_unstable();
    let expected: Vec<i64> = (1..=WRITERS as i64).map(|step| base + step).collect();
    assert_eq!(
        applied, expected,
        "case {case:?}: each applied update must take its own consecutive \
         HealthCheckVersion {expected:?}, got {applied:?}"
    );

    let after = get_health_check(&router, &id).await;
    let stored_version = reported_version(&after, case);
    let expected_version = base + WRITERS as i64;
    assert_eq!(
        stored_version, expected_version,
        "case {case:?}: the health check must stand at HealthCheckVersion \
         {expected_version}, got {stored_version}; body {}",
        after.body
    );
    // Last write wins, so the surviving port is one of the ports asked for —
    // and the one the last-applied update asked for.
    let stored_port = after.first("HealthCheckConfig/Port").unwrap_or("");
    let asked: Vec<String> = (0..WRITERS).map(|writer| (8000 + writer).to_string()).collect();
    assert!(
        asked.iter().any(|port| port == stored_port),
        "case {case:?}: the health check must hold one of the ports asked for \
         ({asked:?}), got {stored_port:?}; body {}",
        after.body
    );
}
