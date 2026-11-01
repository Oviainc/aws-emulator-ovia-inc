// SPDX-License-Identifier: CC0-1.0

//! What the BATCH a findings-metadata write names does to the service's monotonic
//! clock when the write moves no finding, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The ONE rule under test: a mutating operation takes an instant off the service's
//! monotonic "this resource changed now" clock only once a write has really landed, so
//! a call that leaves every stored record exactly as it found it — whether it is
//! accepted and changes no stored value, or refused outright — consumes no instant, and
//! the `CreatedAt`/`UpdatedAt` a LATER accepted call stamps therefore still reports the
//! wall-clock instant of that change rather than one pushed into the future by the
//! calls before it.
//!
//! The `no_op_batch_clock` suite asserts that rule over the OPERATIONS whose write is
//! per stored record; the rows here vary the other dimension of the same rule for the
//! per-finding metadata writes — the SET of `FindingIds` one request names. `FindingIds`
//! is `len 0..50` with each entry `len 1..300`
//! (`smithy_query(shape="FindingIds")`), so the rows span a subset of the findings the
//! detector holds, one id repeated up to the largest batch the model allows, batches of
//! ids the detector holds none of at both published entry lengths, a held id beside an
//! unheld one, and the detector's findings in the state the store starts them in.
//! Because an unheld id is a refusal for the archive write and simply absent for the
//! feedback write, the rows also span both answers: 200 and 400.
//!
//! Observable per request: the clock a mutation reads is what the detector's own
//! `UpdatedAt` records when a sub-resource is added to it
//! (`smithy_query(operation="GetDetector")` models it as a String), so a burst of such
//! calls between two accepted mutations is enough to see whether they consumed
//! instants.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    all_finding_ids, call, enabled_detector, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::no_op_batch_clock::{
    a_burst_of_it_consumes_no_instant, findings_snapshot, NO_OPS_PER_BURST,
};

/// The finding types each row's detector is populated with. Source: the finding types
/// this service's sample catalogue publishes (`CreateSampleFindings`); the rule is about
/// the clock, not the type, so the rows only need the findings to be distinguishable.
const SAMPLE_TYPES: [&str; 3] = [
    "Recon:EC2/Portscan",
    "UnauthorizedAccess:EC2/SSHBruteForce",
    "Policy:S3/AccountBlockPublicAccessDisabled",
];

/// The largest `FindingIds` one request may carry, and the published bounds on one
/// entry: `len 0..50`, each entry `len 1..300`
/// (`smithy_query(shape="FindingIds")`).
const MAX_FINDING_IDS: usize = 50;
const SHORTEST_FINDING_ID: usize = 1;
const LONGEST_FINDING_ID: usize = 300;

/// Which `FindingIds` a row's request names.
#[derive(Debug, Clone, Copy)]
enum Ids {
    /// The first `n` findings the detector holds, in list order.
    Held(usize),
    /// The detector's first finding, named `n` times in ONE batch.
    HeldRepeated(usize),
    /// `n` ids of the minted form (32 lowercase hexadecimal characters) that no
    /// `CreateSampleFindings` produced.
    Unheld(usize),
    /// Ids the detector holds none of, at the shortest and the longest entry the model
    /// allows and in a form the service never mints.
    UnheldAtTheModelledBounds,
    /// The detector's first finding beside one id it does not hold.
    HeldAndUnheld,
}

/// An id of the minted form the detector holds no finding for: 32 lowercase hexadecimal
/// characters, distinct per `index`.
fn unheld_id(index: usize) -> String {
    format!("ffffffffffffffffffffffffffff{index:04x}")
}

impl Ids {
    /// The `findingIds` value a request carries, given the ids the detector holds.
    fn resolve(self, held: &[String]) -> Vec<String> {
        match self {
            Ids::Held(count) => held.iter().take(count).cloned().collect(),
            Ids::HeldRepeated(count) => vec![held[0].clone(); count],
            Ids::Unheld(count) => (0..count).map(unheld_id).collect(),
            Ids::UnheldAtTheModelledBounds => vec![
                "x".repeat(SHORTEST_FINDING_ID),
                "f".repeat(LONGEST_FINDING_ID),
                "no-such-finding".to_string(),
            ],
            Ids::HeldAndUnheld => vec![held[0].clone(), unheld_id(0)],
        }
    }
}

/// One row: the metadata write the burst repeats, the batch it names, the writes that
/// put the detector's findings into the state the burst then asks for again, and the
/// answer every call of the burst carries.
struct Row {
    case: &'static str,
    /// The `/findings/{segment}` writes that run ONCE before the burst, over EVERY
    /// finding the detector holds, in order. These do write, which is what makes the
    /// burst a repeat.
    prepare: &'static [&'static str],
    /// The segment under `/detector/{detectorId}/findings/` the burst posts to, from
    /// each operation's `http` binding (`smithy_query(operation="ArchiveFindings,\
    /// UnarchiveFindings,UpdateFindingsFeedback")`).
    segment: &'static str,
    ids: Ids,
    /// The members the write carries beside `findingIds` — `feedback` and its optional
    /// `comments` for the feedback write, nothing for the archive write.
    extra: fn() -> Value,
    /// `true` when the burst addresses a `DetectorId` the store does not hold, so the
    /// call is refused before any id is resolved.
    absent_detector: bool,
    expected_status: u16,
}

fn no_extra() -> Value {
    json!({})
}

fn useful() -> Value {
    json!({"feedback": "USEFUL"})
}

fn not_useful_with_comments() -> Value {
    json!({"feedback": "NOT_USEFUL", "comments": "triaged: a false positive"})
}

/// The rows, spanning the batch shapes a findings-metadata write can name while moving
/// nothing — and both answers such a call carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "archive, naming one of the findings a first archive already archived",
            prepare: &["archive"],
            segment: "archive",
            ids: Ids::Held(1),
            extra: no_extra,
            absent_detector: false,
            expected_status: 200,
        },
        Row {
            case: "archive, naming an already-archived finding 50 times — the largest batch \
                   the model allows",
            prepare: &["archive"],
            segment: "archive",
            ids: Ids::HeldRepeated(MAX_FINDING_IDS),
            extra: no_extra,
            absent_detector: false,
            expected_status: 200,
        },
        Row {
            case: "unarchive, on every finding in the state the store created it in",
            prepare: &[],
            segment: "unarchive",
            ids: Ids::Held(SAMPLE_TYPES.len()),
            extra: no_extra,
            absent_detector: false,
            expected_status: 200,
        },
        Row {
            case: "feedback, naming one id of the minted form the detector does not hold",
            prepare: &[],
            segment: "feedback",
            ids: Ids::Unheld(1),
            extra: useful,
            absent_detector: false,
            expected_status: 200,
        },
        Row {
            case: "feedback, naming 50 ids the detector does not hold",
            prepare: &[],
            segment: "feedback",
            ids: Ids::Unheld(MAX_FINDING_IDS),
            extra: not_useful_with_comments,
            absent_detector: false,
            expected_status: 200,
        },
        Row {
            case: "feedback, naming unheld ids at the shortest and the longest entry the \
                   model allows",
            prepare: &[],
            segment: "feedback",
            ids: Ids::UnheldAtTheModelledBounds,
            extra: useful,
            absent_detector: false,
            expected_status: 200,
        },
        Row {
            case: "feedback, re-submitting the recorded value beside an id the detector does \
                   not hold",
            prepare: &["feedback"],
            segment: "feedback",
            ids: Ids::HeldAndUnheld,
            extra: not_useful_with_comments,
            absent_detector: false,
            expected_status: 200,
        },
        Row {
            case: "archive, refused by an id the detector does not hold beside one it holds",
            prepare: &[],
            segment: "archive",
            ids: Ids::HeldAndUnheld,
            extra: no_extra,
            absent_detector: false,
            expected_status: 400,
        },
        Row {
            case: "unarchive, refused by a batch naming only ids the detector does not hold",
            prepare: &["archive"],
            segment: "unarchive",
            ids: Ids::Unheld(2),
            extra: no_extra,
            absent_detector: false,
            expected_status: 400,
        },
        Row {
            case: "feedback, refused by a DetectorId the store does not hold",
            prepare: &[],
            segment: "feedback",
            ids: Ids::Held(SAMPLE_TYPES.len()),
            extra: useful,
            absent_detector: true,
            expected_status: 400,
        },
    ]
}

/// An enabled detector holding one finding per entry of [`SAMPLE_TYPES`], and its
/// finding ids in list order.
async fn detector_with_findings(router: &Router, case: &str) -> (String, Vec<String>) {
    let detector_id = enabled_detector(router).await;
    let (status, sampled) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": SAMPLE_TYPES})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected create-sample-findings to answer 200, got {status} {sampled:?}"
    );
    let ids = all_finding_ids(router, &detector_id, MAX_FINDING_IDS as i64).await;
    assert_eq!(
        ids.len(),
        SAMPLE_TYPES.len(),
        "case {case:?}: expected one finding per sample type, got {ids:?}"
    );
    (detector_id, ids)
}

/// The body a row's write carries: its batch, plus whatever members the write itself
/// needs.
fn body_over(ids: &[String], extra: &Value) -> Value {
    let mut body = json!({"findingIds": ids});
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

// ---------------------------------------------------------------------------
// The rule, over the batch shapes that move no finding.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_findings_metadata_batch_that_moves_nothing_consumes_no_instant() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass or fail on another
        // row's writes.
        let router = new_router();
        let (detector_id, held) = detector_with_findings(&router, case).await;

        let extra = (row.extra)();
        // The preparing writes run over EVERY finding the detector holds, so the batch
        // the burst names is already in the state it asks for.
        for segment in row.prepare {
            let (status, prepared) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}/findings/{segment}"),
                Some(body_over(&held, &extra)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the preparing {segment:?} write to answer 200, got \
                 {status} {prepared:?}"
            );
        }

        let named = row.ids.resolve(&held);
        assert!(
            named.len() <= MAX_FINDING_IDS,
            "case {case:?}: a request may name at most {MAX_FINDING_IDS} ids, got {}",
            named.len()
        );
        let body = body_over(&named, &extra);
        let addressed = if row.absent_detector {
            ABSENT_DETECTOR_ID
        } else {
            detector_id.as_str()
        };
        let path = format!("/detector/{addressed}/findings/{}", row.segment);

        let before = findings_snapshot(&router, &detector_id, &held, case).await;
        a_burst_of_it_consumes_no_instant(
            &router,
            &detector_id,
            case,
            &path,
            &body,
            row.expected_status,
        )
        .await;
        let after = findings_snapshot(&router, &detector_id, &held, case).await;

        assert_eq!(
            after, before,
            "case {case:?}: {NO_OPS_PER_BURST} calls that move no finding must leave every \
             finding exactly as it was, expected {before:?}, got {after:?}"
        );
    }
}
