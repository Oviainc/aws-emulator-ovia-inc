// SPDX-License-Identifier: CC0-1.0

//! The liveness bound on driving a session's training queue, asserted on the stored
//! session rather than over the wire.
//!
//! The RULE under test: driving the queue must TERMINATE for every state a caller's
//! submits can leave it in — the head either advances strictly, so a slot it has already
//! settled is never re-read, or the drive stops because there is no slot after the one it
//! stands on. `sequenceNumber` is an `i32`, so the last slot the queue can address is
//! `i32::MAX`, and a head left standing on it has nowhere to advance to.
//!
//! `sequence_gap_timeout.rs` drives the same range through the front door, where a submit
//! decides which slots exist; this suite starts from the stored session instead, because
//! the head only reaches the end of the range by advancing through it and no number of
//! requests can put it there. The rows span each kind of occupant the last slot can hold
//! (one that has already settled, one that settles on this very call, one whose operation
//! record is gone), and a companion table proves the head still advances once per settled
//! slot below it, so terminating is not bought by never advancing at all.

use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::ops::types::{
    OperationConfig, SaveWeightsConfig,
};
use service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::state::{
    Session, TrainingOp, STATUS_COMPLETED, STATUS_FAILED, STATUS_PENDING,
};

/// How long a call that must terminate is given before the case fails rather than hangs:
/// the work is in-memory bookkeeping over one session, so anything beyond this is a loop
/// that is not making progress.
const MUST_RETURN_WITHIN: Duration = Duration::from_secs(10);

/// A training operation record in `status`, occupying `sequence` of its session.
fn stored_op(
    operation_id: &str,
    sequence: i32,
    status: &'static str,
    submitted_at: SystemTime,
) -> TrainingOp {
    TrainingOp {
        operation_id: operation_id.to_string(),
        sequence_number: sequence,
        client_token: None,
        request_fingerprint: String::new(),
        // A save needs no model runtime, so this is an operation the emulator really does
        // execute: a pending one settles once its due instant has passed.
        config: OperationConfig::saveWeights(SaveWeightsConfig::default()),
        status,
        result: None,
        error_details: None,
        submitted_at,
        due_at: None,
        reserved: None,
    }
}

/// What occupies the queue head in one row of the table below.
struct Occupant {
    case: &'static str,
    /// The operation stored at the head slot, or `None` to leave the slot pointing at a
    /// record the session does not hold.
    op: Option<&'static str>,
    /// The status that operation carries when the queue is driven.
    status: &'static str,
}

#[test]
fn driving_the_queue_at_the_last_addressable_sequence_terminates_whatever_occupies_it() {
    // The head only ever advances by one, so the slot it can be left standing on is the
    // largest a `sequenceNumber` (an `i32`) can address. Rows differ along what occupies
    // that slot: an operation that has already settled, one that settles on this very
    // call, and a slot whose record the session no longer holds.
    let rows = [
        Occupant {
            case: "a completed operation",
            op: Some("training-completed"),
            status: STATUS_COMPLETED,
        },
        Occupant {
            case: "a failed operation",
            op: Some("training-failed"),
            status: STATUS_FAILED,
        },
        Occupant {
            case: "an operation that settles on this call",
            op: Some("training-pending"),
            status: STATUS_PENDING,
        },
        Occupant {
            case: "a slot whose operation record is gone",
            op: None,
            status: STATUS_COMPLETED,
        },
    ];

    for row in rows {
        // Submitted far enough back that a pending operation's execution span has passed,
        // so the row that settles on this call really does settle on it.
        let submitted_at = SystemTime::now() - Duration::from_secs(60);
        let mut session = Session::new("last-slot".to_string(), submitted_at);
        session.next_sequence_number = i32::MAX;
        session
            .sequence_index
            .insert(i32::MAX, "training-absent".to_string());
        if let Some(operation_id) = row.op {
            session
                .sequence_index
                .insert(i32::MAX, operation_id.to_string());
            session.training_ops.insert(
                operation_id.to_string(),
                stored_op(operation_id, i32::MAX, row.status, submitted_at),
            );
        }

        // Driven on a thread of its own: a queue that fails to advance spins for ever, so
        // the case must fail on a deadline rather than hang the suite.
        let (done, answered) = mpsc::channel();
        let case = row.case;
        std::thread::spawn(move || {
            session.advance(SystemTime::now());
            let _ = done.send(session.next_sequence_number);
        });
        let head = answered.recv_timeout(MUST_RETURN_WITHIN).unwrap_or_else(|_| {
            panic!(
                "case {case}: driving the queue at the last addressable sequence number must \
                 return, and did not within {MUST_RETURN_WITHIN:?}"
            )
        });
        assert_eq!(
            head,
            i32::MAX,
            "case {case}: there is no slot past the last addressable sequence number, so the \
             head stays on it, got {head}"
        );
    }
}

#[test]
fn a_queue_that_can_still_advance_moves_its_head_past_every_settled_slot() {
    // The counterpart of the table above: below the last addressable slot the head DOES
    // move, once per settled operation, so terminating cannot be bought by never
    // advancing. Rows differ along how many settled operations sit in front of it.
    for settled in [1_i32, 2, 5] {
        let submitted_at = SystemTime::now() - Duration::from_secs(60);
        let mut session = Session::new(format!("settled-{settled}"), submitted_at);
        for sequence in 1..=settled {
            let operation_id = format!("training-{sequence}");
            session
                .sequence_index
                .insert(sequence, operation_id.clone());
            session.training_ops.insert(
                operation_id.clone(),
                stored_op(&operation_id, sequence, STATUS_COMPLETED, submitted_at),
            );
        }

        session.advance(SystemTime::now());

        assert_eq!(
            session.next_sequence_number,
            settled + 1,
            "case {settled} settled operations: the head must stand on the first slot it has \
             not settled, got {}",
            session.next_sequence_number
        );
    }
}
