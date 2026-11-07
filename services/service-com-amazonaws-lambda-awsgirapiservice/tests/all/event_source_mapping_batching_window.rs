// SPDX-License-Identifier: CC0-1.0

//! The DECISION an event source mapping takes about the batch it has gathered:
//! `crate::state::batch_is_due`, the whole of what the poller behind
//! `CreateEventSourceMapping` asks of a gathered batch before it hands its function anything.
//!
//! The RULE asserted over the table below: a gathered batch goes for exactly two reasons —
//! it holds `BatchSize`, "the maximum number of records in each batch that Lambda pulls from
//! your stream or queue", or it has been gathering for the whole of
//! `MaximumBatchingWindowInSeconds`, "the maximum amount of time, in seconds, that Lambda
//! spends gathering records before invoking the function". Both are MAXIMA, so a batch that
//! fills goes without waiting the rest of the window out and a window that has run hands over
//! whatever was gathered however small it is; and a batch holding nothing never goes, because
//! there is nothing to hand over.
//!
//! The rows span what the rule generalises over: how many records are gathered (none, below
//! the size, exactly it, past it), how much of the window has run (none of it, part, exactly
//! it, past it) and which window is configured — both published bounds of
//! `@range(min: 0, max: 300)` and a span between them — so no row is the recorded case and
//! each bound is exercised.
//!
//! What the mapping then DOES with a due batch — taking the records out of its queue as they
//! arrive and handing its function every record of the window at once — is asserted end to
//! end over the wire, across the two services that own the halves of it, in
//! `core/router/tests/all/lambda_sqs_event_source_batching_window.rs`.

use std::time::Duration;

use service_com_amazonaws_lambda_awsgirapiservice::state::batch_is_due;

/// The published bounds of `MaximumBatchingWindowInSeconds` (`@range(min: 0, max: 300)`), and
/// a span between them.
const WINDOW_MIN: u64 = 0;
/// See [`WINDOW_MIN`].
const WINDOW_MAX: u64 = 300;
/// See [`WINDOW_MIN`].
const WINDOW_MID: u64 = 30;

/// One row: what the mapping has gathered, its `BatchSize`, how long it has been gathering,
/// the window it gathers over, and whether that batch is due.
struct Row {
    case: &'static str,
    gathered: usize,
    batch_size: usize,
    waited: u64,
    window: u64,
    due: bool,
}

/// RULE: a gathered batch is due when it holds `BatchSize` or when its
/// `MaximumBatchingWindowInSeconds` has run, and never when it holds nothing.
#[test]
fn a_gathered_batch_goes_when_it_is_full_or_out_of_time() {
    let rows = [
        Row {
            case: "nothing gathered, no window",
            gathered: 0,
            batch_size: 10,
            waited: 0,
            window: WINDOW_MIN,
            due: false,
        },
        Row {
            case: "nothing gathered, the window has run",
            gathered: 0,
            batch_size: 10,
            waited: WINDOW_MAX,
            window: WINDOW_MAX,
            due: false,
        },
        Row {
            case: "one record and no window at all",
            gathered: 1,
            batch_size: 10,
            waited: 0,
            window: WINDOW_MIN,
            due: true,
        },
        Row {
            case: "below the batch size, part of the window run",
            gathered: 3,
            batch_size: 10,
            waited: WINDOW_MID / 2,
            window: WINDOW_MID,
            due: false,
        },
        Row {
            case: "below the batch size, the window exactly run",
            gathered: 3,
            batch_size: 10,
            waited: WINDOW_MID,
            window: WINDOW_MID,
            due: true,
        },
        Row {
            case: "below the batch size, past the window",
            gathered: 1,
            batch_size: 10,
            waited: WINDOW_MAX + 1,
            window: WINDOW_MAX,
            due: true,
        },
        Row {
            case: "exactly the batch size, none of the window run",
            gathered: 10,
            batch_size: 10,
            waited: 0,
            window: WINDOW_MAX,
            due: true,
        },
        Row {
            case: "past the batch size, none of the window run",
            gathered: 11,
            batch_size: 10,
            waited: 0,
            window: WINDOW_MID,
            due: true,
        },
        Row {
            case: "one record, a batch size of one, the longest window",
            gathered: 1,
            batch_size: 1,
            waited: 0,
            window: WINDOW_MAX,
            due: true,
        },
    ];
    for row in rows {
        let Row {
            case,
            gathered,
            batch_size,
            waited,
            window,
            due,
        } = row;
        let answered = batch_is_due(
            gathered,
            batch_size,
            Duration::from_secs(waited),
            Duration::from_secs(window),
        );
        assert_eq!(
            answered, due,
            "case {case:?}: a batch of {gathered} record(s) of a BatchSize of {batch_size}, \
             gathered for {waited}s of a {window}s window, must{} be due, got {answered}",
            if due { "" } else { " not" }
        );
    }
}
