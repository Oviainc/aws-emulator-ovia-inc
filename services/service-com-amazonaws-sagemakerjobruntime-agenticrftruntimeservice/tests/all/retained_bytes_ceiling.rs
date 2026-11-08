// SPDX-License-Identifier: CC0-1.0

//! The store-wide retained-byte ceiling is measured against what capturing a turn would
//! retain.
//!
//! RULE: a bound on a tracked total is checked against the SAME quantity the total
//! counts, so the store-wide ceiling (`state::MAX_RETAINED_BODY_BYTES`) on the retained
//! payload — a total summed over the request AND the response body of every captured
//! turn (`ServiceState::retained_body_bytes`) — is measured against both halves of the
//! turn being admitted: a turn whose halves TOGETHER would take the total past the
//! ceiling is refused as a quota with nothing written, whichever half carries the bytes,
//! and the tracked total therefore never passes the ceiling it names.
//!
//! Two suites, each a TABLE:
//!
//! * the capture path itself (`ServiceState::append_turn`, the one writer of a turn and
//!   the one maintainer of the total), whose rows vary HOW the room left under the
//!   ceiling is spent — by the request half, by the response half, by both — and by how
//!   much, under the room, exactly at it and past it;
//! * the two sampling operations over the wire, whose rows vary the `Body` size against
//!   the room left, so what is pinned is the emulator's HTTP answer: `402`
//!   `ServiceQuotaExceededException` for a turn that does not fit, `200` for one that
//!   does, and a total that grew by exactly the two halves the turn retained.

use std::time::SystemTime;

use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job_at_arn, minted_job_arn, mounted_service, send, Reply, JOB_ARN_HEADER, SAMPLE_PATH,
    SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER,
};

/// How many bytes of room every row leaves under the ceiling before it drives the turn
/// it is about.
///
/// Small enough that a request half of this size is well inside the per-turn bound
/// (`state::MAX_TURN_BODY_BYTES`), so a row that spends the room on the request half is
/// answered by the store-wide ceiling and not by the per-turn one, and large enough that
/// a small body plus the completion envelope served for it still fits.
const ROOM: usize = 4096;

/// The trajectory the seeding turns are retained on, which is never the trajectory a row
/// drives.
const SEED_TRAJECTORY: &str = "retained-seed";

/// Retain turns on `state` until exactly `room` bytes are left under the ceiling.
///
/// Each seeding turn carries a one-byte request half and a response half of up to a
/// mebibyte, so the store reaches a 64 MiB total without any single allocation of it.
fn retain_until_room_is(state: &mut ServiceState, job_arn: &str, room: usize) {
    const CHUNK: usize = 1 << 20;
    let headers = ::http::HeaderMap::new();
    loop {
        let left = state::MAX_RETAINED_BODY_BYTES - state.retained_body_bytes();
        assert!(
            left >= room,
            "seeding overshot: {left} byte(s) left under the ceiling, wanted {room}"
        );
        if left == room {
            break;
        }
        let add = (left - room).min(CHUNK);
        let request = [b'p'; 1];
        state
            .append_turn(
                &state::TurnRequest {
                    job_arn,
                    trajectory_id: SEED_TRAJECTORY,
                    request_body: &request,
                    headers: &headers,
                    streamed: false,
                    at: SystemTime::now(),
                },
                |_| (vec![b'r'; add - 1], state::JSON_CONTENT_TYPE.to_string()),
            )
            .expect("a seeding turn under the ceiling is captured");
    }
    assert_eq!(
        state.retained_body_bytes(),
        state::MAX_RETAINED_BODY_BYTES - room,
        "seeding left the store at {} byte(s), wanted {} with {room} byte(s) of room",
        state.retained_body_bytes(),
        state::MAX_RETAINED_BODY_BYTES - room
    );
}

/// Append one turn of `request_bytes` request bytes answered by `response_bytes` response
/// bytes to a store that has exactly [`ROOM`] bytes left under the ceiling.
fn append_turn_with_room(
    request_bytes: usize,
    response_bytes: usize,
) -> (ServiceState, Result<usize, state::AppendRefusal>) {
    let job_arn = minted_job_arn("AgentRFT", "retain-run");
    let mut store = ServiceState::default();
    retain_until_room_is(&mut store, &job_arn, ROOM);
    let headers = ::http::HeaderMap::new();
    let request = vec![b'q'; request_bytes];
    let answer = store
        .append_turn(
            &state::TurnRequest {
                job_arn: &job_arn,
                trajectory_id: "traj-room",
                request_body: &request,
                headers: &headers,
                streamed: false,
                at: SystemTime::now(),
            },
            |_| {
                (
                    vec![b's'; response_bytes],
                    state::JSON_CONTENT_TYPE.to_string(),
                )
            },
        )
        .map(|served| served.body.len());
    (store, answer)
}

/// RULE, on the one writer of a turn: the ceiling is checked against the request half
/// PLUS the response half, so the room left under it is spent by both halves together
/// and a pair that would pass it is refused with nothing written.
#[test]
fn the_ceiling_is_measured_against_both_halves_of_the_turn() {
    // Rows differ along WHICH half spends the room left under the ceiling and by HOW
    // MUCH: under the room, exactly at it, and past it. The last row carries a request
    // half past the PER-TURN bound (`state::MAX_TURN_BODY_BYTES`), which the store-wide
    // ceiling measuring both halves must not mask.
    let rows: [(&str, usize, usize, bool); 8] = [
        ("both halves well under the room", 8, 8, true),
        ("the pair exactly at the room", ROOM - 8, 8, true),
        ("the request half at the room, nothing for the response", ROOM, 0, true),
        ("the request half at the room, a one-byte response", ROOM, 1, false),
        ("the response half alone at the room", 1, ROOM, false),
        ("the pair one byte past the room", ROOM / 2, ROOM / 2 + 1, false),
        ("the request half alone past the room", ROOM + 1, 1, false),
        (
            "the request half past the per-turn bound",
            state::MAX_TURN_BODY_BYTES + 1,
            1,
            false,
        ),
    ];

    for (case, request_bytes, response_bytes, admitted) in rows {
        let retained_before = state::MAX_RETAINED_BODY_BYTES - ROOM;
        let turn_bytes = request_bytes + response_bytes;
        let seeded_trajectories = 1;
        let (store, answer) = append_turn_with_room(request_bytes, response_bytes);
        if admitted {
            assert!(
                answer.is_ok(),
                "case {case:?}: expected a turn of {turn_bytes} byte(s) inside the room to be \
                 captured, got {answer:?}"
            );
            assert_eq!(
                store.retained_body_bytes(),
                retained_before + turn_bytes,
                "case {case:?}: expected the total to grow by both halves ({turn_bytes}), got {}",
                store.retained_body_bytes()
            );
            assert!(
                store.retained_body_bytes() <= state::MAX_RETAINED_BODY_BYTES,
                "case {case:?}: expected the total to stay under the ceiling, got {}",
                store.retained_body_bytes()
            );
            assert_eq!(
                store.trajectories.len(),
                seeded_trajectories + 1,
                "case {case:?}: expected the captured turn's trajectory to exist, got {} \
                 trajectory/ies",
                store.trajectories.len()
            );
        } else {
            let expected = if request_bytes > state::MAX_TURN_BODY_BYTES {
                state::AppendRefusal::BodyTooLarge {
                    bytes: request_bytes,
                }
            } else {
                state::AppendRefusal::RetainedBytesQuota {
                    bytes: turn_bytes,
                    retained: retained_before,
                }
            };
            assert_eq!(
                answer.as_ref().err(),
                Some(&expected),
                "case {case:?}: expected the turn to be refused as {expected:?}, got {answer:?}"
            );
            assert_eq!(
                store.retained_body_bytes(),
                retained_before,
                "case {case:?}: expected a refused turn to retain nothing, got {}",
                store.retained_body_bytes()
            );
            assert_eq!(
                store.trajectories.len(),
                seeded_trajectories,
                "case {case:?}: expected a refused turn to create no trajectory, got {} \
                 trajectory/ies",
                store.trajectories.len()
            );
        }
    }
}

/// Dispatch one sampling call for `job` to `path`.
async fn sample(
    router: &MultiServiceRouter,
    path: &str,
    job: &str,
    trajectory_id: &str,
    payload: Vec<u8>,
) -> Reply {
    send(
        router,
        path,
        &[
            (JOB_ARN_HEADER, job),
            (TRAJECTORY_ID_HEADER, trajectory_id),
            ("content-type", "application/json"),
        ],
        payload,
    )
    .await
}

/// RULE, over the wire on both sampling operations: a sampling call whose turn — its
/// payload plus the response served for it — would take the store past the ceiling is
/// answered `402 ServiceQuotaExceededException` with nothing written, and one that fits
/// is served and grows the total by exactly those two halves.
#[tokio::test]
async fn a_turn_that_would_pass_the_ceiling_is_refused_as_a_quota() {
    // Rows differ along the `Body` size against the room left under the ceiling: a body
    // that fills the room exactly (so only the response half passes it), one past the
    // room, and one small enough that the room holds it with the completion served for
    // it.
    let rows: [(&str, usize, bool); 3] = [
        ("a body filling the room exactly", ROOM, false),
        ("a body one byte past the room", ROOM + 1, false),
        ("a body the room holds with its response", 64, true),
    ];

    for (case, size, served) in rows {
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let job_arn = minted_job_arn("AgentRFT", "retain-run");
            let (engine, router) = mounted_service();
            create_job_at_arn(&engine, &job_arn);
            let store = engine.state::<ServiceState>();
            retain_until_room_is(&mut store.lock(), &job_arn, ROOM);
            let retained_before = store.lock().retained_body_bytes();

            let reply = sample(&router, path, &job_arn, "traj-room", vec![b'q'; size]).await;

            let guard = store.lock();
            if served {
                assert_eq!(
                    reply.status,
                    200,
                    "case {case:?} on {path}: expected a turn the room holds to be served, got \
                     {} / {}",
                    reply.status,
                    reply.text()
                );
                assert_eq!(
                    guard.retained_body_bytes(),
                    retained_before + size + reply.body.len(),
                    "case {case:?} on {path}: expected the total to grow by the payload \
                     ({size}) and the {} byte(s) served, got {}",
                    reply.body.len(),
                    guard.retained_body_bytes()
                );
                assert!(
                    guard.retained_body_bytes() <= state::MAX_RETAINED_BODY_BYTES,
                    "case {case:?} on {path}: expected the total to stay under the ceiling of \
                     {}, got {}",
                    state::MAX_RETAINED_BODY_BYTES,
                    guard.retained_body_bytes()
                );
            } else {
                assert_eq!(
                    (reply.status, reply.error_code().as_str()),
                    (402, "ServiceQuotaExceededException"),
                    "case {case:?} on {path}: expected 402 ServiceQuotaExceededException, got {} \
                     {:?} / {}",
                    reply.status,
                    reply.error_type,
                    reply.text()
                );
                assert!(
                    reply
                        .message()
                        .contains(&state::MAX_RETAINED_BODY_BYTES.to_string())
                        && reply.message().contains("traj-room"),
                    "case {case:?} on {path}: expected the message to state the ceiling and the \
                     trajectory, got {:?}",
                    reply.message()
                );
                assert_eq!(
                    guard.retained_body_bytes(),
                    retained_before,
                    "case {case:?} on {path}: expected a refused call to retain nothing further, \
                     got {} against {retained_before}",
                    guard.retained_body_bytes()
                );
                assert!(
                    guard.trajectory(&job_arn, "traj-room").is_none(),
                    "case {case:?} on {path}: expected a refused call to create no trajectory"
                );
            }
        }
    }
}
