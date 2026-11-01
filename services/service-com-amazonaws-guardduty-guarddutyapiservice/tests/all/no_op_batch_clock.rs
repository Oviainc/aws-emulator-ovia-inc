// SPDX-License-Identifier: CC0-1.0

//! What an ACCEPTED call whose writes are decided one stored record at a time
//! leaves behind when it moves none of them, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rule under test is the one the `no_op_mutation_clock` suite asserts for the
//! single-record updates, here over the operations whose write is per RECORD — the
//! batches that report what they did not process in `UnprocessedAccounts`, the
//! partial detector update whose every configurable member is optional, and the
//! per-finding metadata writes: a mutating operation takes an instant off the
//! service's monotonic "this resource changed now" clock only once a write has
//! really landed, so a call that leaves every stored record exactly as it found it
//! consumes no instant — and the `CreatedAt`/`UpdatedAt` a LATER accepted call
//! stamps therefore still reports the wall-clock instant of that change rather than
//! one pushed into the future by the calls before it.
//!
//! Observable per request: the clock a mutation reads is what the detector's own
//! `UpdatedAt` records when a sub-resource is added to it
//! (`smithy_query(operation="GetDetector")` models it as a String), so a burst of
//! such calls between two accepted mutations is enough to see whether they consumed
//! instants: the clock may advance across the burst only by the wall-clock time that
//! really passed over the same span, whereas a handler that reads the clock before
//! it knows whether it writes spends an instant per call on top of that.
//!
//! The rows differ along what the rule generalises over: the operation, why the call
//! moves nothing (an account the administrator holds no record for, a record already
//! in the state the call asks for, the administrator's own account, a request naming
//! no configuration at all, a batch naming no id at all), how many ids one request
//! names, and whether the records at stake are members, the detector's own
//! configuration or findings.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::detector_lifecycle::{all_finding_ids, call, enabled_detector, new_router, Router};
use crate::member_association::{administrator, create_members};
use crate::refused_create_clock::{as_millis, detector_updated_at, READING_SLACK_MILLIS};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::now_millis;

/// How many copies of the call one burst sends. A call that writes nothing consumes
/// no instant, so the burst may be any size; it is this large because a handler that
/// reads the clock BEFORE it knows whether it writes spends at least one millisecond
/// of it per call, and the burst must advance that clock by more than
/// [`READING_SLACK_MILLIS`] past the wall-clock time the burst itself takes for the
/// difference to be an assertion.
pub(crate) const NO_OPS_PER_BURST: usize = 300;

/// The member account a row seeds, as `(accountId, email)`: 12 decimal digits
/// (`smithy_query(shape="AccountId")`) and the email `CreateMembers` records.
const ONE_MEMBER: [(&str, &str); 1] = [("111122223333", "member-one@example.com")];

/// Well-formed account ids no `CreateMembers` in this suite records.
const NON_MEMBER: &str = "444455556666";
const OTHER_NON_MEMBER: &str = "999999999999";

/// The finding types a findings row's detector is populated with. Source: the finding
/// types this service's sample catalogue publishes (`CreateSampleFindings`); the rule
/// is about the clock, not the type.
const SAMPLE_TYPES: [&str; 3] = [
    "Recon:EC2/PortProbeUnprotectedPort",
    "Trojan:EC2/BlackholeTraffic",
    "Policy:S3/BucketBlockPublicAccessDisabled",
];

/// One row: the member accounts to record first, the writes that put the store into
/// the state the burst then asks for again, and the accepted call the burst repeats.
struct Row {
    case: &'static str,
    /// Accounts `CreateMembers` records before the burst — empty for a row whose
    /// call names no member the administrator holds.
    members: &'static [(&'static str, &'static str)],
    /// The calls that run ONCE before the burst, each the path suffix under
    /// `/detector/{detectorId}` and the body it carries.
    prepare: Vec<(&'static str, Value)>,
    /// The call the burst repeats: the path suffix under `/detector/{detectorId}`
    /// and its body.
    suffix: &'static str,
    body: Value,
}

/// The rows, spanning the operations whose write is per stored record and the reasons
/// one of their accepted calls moves nothing. The path suffixes come from each
/// operation's `http` binding (`smithy_query(operation="DisassociateMembers,\
/// StopMonitoringMembers,StartMonitoringMembers,InviteMembers,CreateMembers,\
/// UpdateMemberDetectors,UpdateDetector,ArchiveFindings,UnarchiveFindings,\
/// UpdateFindingsFeedback")`).
fn rows() -> Vec<Row> {
    let member = ONE_MEMBER[0].0;
    vec![
        Row {
            case: "disassociate members, naming an account with no member record",
            members: &[],
            prepare: Vec::new(),
            suffix: "/member/disassociate",
            body: json!({"accountIds": [NON_MEMBER]}),
        },
        Row {
            case: "disassociate members, repeating a disassociation already accepted",
            members: &ONE_MEMBER,
            prepare: vec![(
                "/member/disassociate",
                json!({"accountIds": [member]}),
            )],
            suffix: "/member/disassociate",
            body: json!({"accountIds": [member]}),
        },
        Row {
            // Two ids rather than one: the count of named accounts is one of the
            // things the rule generalises over.
            case: "disassociate members, naming two accounts neither of which is a member",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member/disassociate",
            body: json!({"accountIds": [NON_MEMBER, OTHER_NON_MEMBER]}),
        },
        Row {
            case: "stop monitoring members, repeating a stop already accepted",
            members: &ONE_MEMBER,
            prepare: vec![("/member/stop", json!({"accountIds": [member]}))],
            suffix: "/member/stop",
            body: json!({"accountIds": [member]}),
        },
        Row {
            case: "stop monitoring members, naming an account with no member record",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member/stop",
            body: json!({"accountIds": [NON_MEMBER]}),
        },
        Row {
            // Monitoring is a desired state, so only a member whose monitoring was
            // STOPPED moves: this one never was.
            case: "start monitoring members, on a member whose monitoring was never stopped",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member/start",
            body: json!({"accountIds": [member]}),
        },
        Row {
            case: "start monitoring members, naming an account with no member record",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member/start",
            body: json!({"accountIds": [NON_MEMBER]}),
        },
        Row {
            case: "invite members, re-sending an invitation still pending",
            members: &ONE_MEMBER,
            prepare: vec![("/member/invite", json!({"accountIds": [member]}))],
            suffix: "/member/invite",
            body: json!({"accountIds": [member]}),
        },
        Row {
            case: "invite members, naming an account with no member record",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member/invite",
            body: json!({"accountIds": [NON_MEMBER]}),
        },
        Row {
            case: "create members, re-stating the details already stored",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member",
            body: json!({
                "accountDetails": [{"accountId": member, "email": ONE_MEMBER[0].1}],
            }),
        },
        Row {
            // The current account is the administrator in this relationship, so it is
            // never one of its own members: reported, and nothing is recorded.
            case: "create members, naming the administrator's own account",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member",
            body: json!({
                "accountDetails": [{
                    "accountId": administrator(),
                    "email": "administrator@example.com",
                }],
            }),
        },
        Row {
            case: "update member detectors, carrying no configuration at all",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member/detector/update",
            body: json!({"accountIds": [member]}),
        },
        Row {
            case: "update member detectors, re-stating the switch the account already carries",
            members: &ONE_MEMBER,
            prepare: vec![(
                "/member/detector/update",
                json!({
                    "accountIds": [member],
                    "features": [{"name": "EKS_AUDIT_LOGS", "status": "DISABLED"}],
                }),
            )],
            suffix: "/member/detector/update",
            body: json!({
                "accountIds": [member],
                "features": [{"name": "EKS_AUDIT_LOGS", "status": "DISABLED"}],
            }),
        },
        Row {
            case: "update member detectors, naming an account with no member record",
            members: &ONE_MEMBER,
            prepare: Vec::new(),
            suffix: "/member/detector/update",
            body: json!({
                "accountIds": [NON_MEMBER],
                "features": [{"name": "S3_DATA_EVENTS", "status": "DISABLED"}],
            }),
        },
        Row {
            // Every configurable member of this update is optional, so a request can
            // carry none of them — the detector's own configuration, not a member's.
            case: "update detector, carrying no configuration member",
            members: &[],
            prepare: Vec::new(),
            suffix: "",
            body: json!({}),
        },
        Row {
            // `FindingIds` is `len 0..50` (`smithy_query(operation="ArchiveFindings")`),
            // so a batch naming no id at all is a well-formed accepted request.
            case: "archive findings, naming no finding at all",
            members: &[],
            prepare: Vec::new(),
            suffix: "/findings/archive",
            body: json!({"findingIds": []}),
        },
        Row {
            case: "unarchive findings, naming no finding at all",
            members: &[],
            prepare: Vec::new(),
            suffix: "/findings/unarchive",
            body: json!({"findingIds": []}),
        },
        Row {
            case: "update findings feedback, naming no finding at all",
            members: &[],
            prepare: Vec::new(),
            suffix: "/findings/feedback",
            body: json!({"findingIds": [], "feedback": "USEFUL"}),
        },
    ]
}

/// Every member the administrator holds, as `ListMembers` reports them with
/// `onlyAssociated=false` — the reading a later caller gets, so comparing two of them
/// is what makes "the burst wrote nothing" an assertion about the store and not only
/// about a timestamp.
async fn members_snapshot(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/member?onlyAssociated=false"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected list-members to answer 200, got {status} {body:?}"
    );
    body
}

/// Every finding of the detector, read back through `GetFindings` and keyed by id.
pub(crate) async fn findings_snapshot(
    router: &Router,
    detector_id: &str,
    ids: &[String],
    case: &str,
) -> BTreeMap<String, Value> {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/get"),
        Some(json!({"findingIds": ids})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-findings to answer 200, got {status} {body:?}"
    );
    body["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|finding| {
            (
                finding["id"].as_str().unwrap_or_default().to_string(),
                finding,
            )
        })
        .collect()
}

/// Repeat one call that writes nothing [`NO_OPS_PER_BURST`] times — each answering
/// `expected_status`, the status the row says that call carries — and assert what the
/// rule requires of the clock: the detector the calls address does not move, and the
/// accepted create that follows the burst stamps a WALL-CLOCK instant rather than one
/// the burst pushed into the future. The probing create is read back, so the instant
/// asserted records a change that really landed.
pub(crate) async fn a_burst_of_it_consumes_no_instant(
    router: &Router,
    detector_id: &str,
    case: &str,
    path: &str,
    body: &Value,
    expected_status: u16,
) {
    // The instant the last ACCEPTED write stamped, and the wall clock just after it:
    // between these two readings and the accepted create below, every call the
    // service sees changes nothing, so the clock may advance only by the time that
    // really passes.
    let before_burst = detector_updated_at(router, detector_id, case).await;
    let wall_before = now_millis();

    for attempt in 0..NO_OPS_PER_BURST {
        let (status, answered) = call(router, "POST", path, Some(body.clone())).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: call {attempt} must answer {expected_status}, got {status} \
             {answered:?}"
        );
    }

    // Nothing about the detector that holds the records moved either.
    let after_burst = detector_updated_at(router, detector_id, case).await;
    assert_eq!(
        after_burst, before_burst,
        "case {case:?}: {NO_OPS_PER_BURST} calls that write nothing must not move the \
         detector's updatedAt, expected {before_burst:?}, got {after_burst:?}"
    );

    // The accepted create right after the burst: it writes, so it stamps the instant
    // it happened — and that instant is a WALL-CLOCK instant, not one the burst
    // pushed into the future.
    let probe_collection = format!("/detector/{detector_id}/ipset");
    let (status, created) = call(
        router,
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
    let stamped = detector_updated_at(router, detector_id, case).await;
    assert!(
        stamped > after_burst,
        "case {case:?}: the accepted create changed the detector, so its updatedAt must move \
         past {after_burst:?}, got {stamped:?}"
    );
    // The measurement: how far the clock moved across the burst, against how far the
    // WALL clock moved over the same span. A call that writes nothing consumes no
    // instant, so the two agree to within the slack above however long the burst
    // took — where a handler that read the clock before knowing whether it writes
    // would have spent an instant per call on top of it.
    let elapsed = wall_after - wall_before;
    let advance = as_millis(&stamped, case) - as_millis(&before_burst, case);
    assert!(
        advance <= elapsed + READING_SLACK_MILLIS,
        "case {case:?}: {NO_OPS_PER_BURST} calls that write nothing consume no instant, so the \
         clock may advance only by the {elapsed} ms that really passed (plus \
         {READING_SLACK_MILLIS} ms of slack), got {advance} ms (from {before_burst:?} to \
         {stamped:?})"
    );

    let (status, read_back) =
        call(router, "GET", &format!("{probe_collection}/{minted}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a read of the minted ipSetId to answer 200, got {status} \
         {read_back:?}"
    );
}

// ---------------------------------------------------------------------------
// The rule, over the batches that report rather than refuse and over the partial
// update whose members are all optional.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accepted_calls_that_move_no_record_leave_the_clock_where_the_wall_clock_is() {
    for row in rows() {
        let case = row.case;
        // A router and a detector per row, so a row cannot pass or fail on another
        // row's writes.
        let router = new_router();
        let detector_id = enabled_detector(&router).await;

        if !row.members.is_empty() {
            let (status, recorded) = create_members(&router, &detector_id, row.members).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the seeding create-members to answer 200, got \
                 {status} {recorded:?}"
            );
        }
        for (suffix, body) in &row.prepare {
            let (status, prepared) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}{suffix}"),
                Some(body.clone()),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the preparing {suffix:?} call to answer 200, got \
                 {status} {prepared:?}"
            );
        }

        // What a later caller sees of the members before the burst — including the
        // row that seeds none, where the assertion is that the burst recorded none.
        let before_members = members_snapshot(&router, &detector_id, case).await;

        let path = format!("/detector/{detector_id}{}", row.suffix);
        a_burst_of_it_consumes_no_instant(&router, &detector_id, case, &path, &row.body, 200).await;

        let after_members = members_snapshot(&router, &detector_id, case).await;
        assert_eq!(
            after_members, before_members,
            "case {case:?}: {NO_OPS_PER_BURST} calls that write nothing must leave every member \
             exactly as it was, expected {before_members:?}, got {after_members:?}"
        );
    }
}

/// One findings row: the per-finding metadata write, run once over every finding the
/// detector holds and then repeated — so each call of the burst asks for the state the
/// store is already in.
struct FindingRow {
    case: &'static str,
    /// The `/findings/{segment}` writes that run once before the burst, in order.
    prepare: &'static [&'static str],
    /// The segment the burst posts to, and the body it carries beside `findingIds`.
    segment: &'static str,
    extra: Value,
}

fn finding_rows() -> Vec<FindingRow> {
    vec![
        FindingRow {
            case: "archiving findings already archived",
            prepare: &["archive"],
            segment: "archive",
            extra: json!({}),
        },
        FindingRow {
            case: "unarchiving findings already unarchived",
            prepare: &["archive", "unarchive"],
            segment: "unarchive",
            extra: json!({}),
        },
        FindingRow {
            // `Feedback` declares USEFUL and NOT_USEFUL
            // (`smithy_query(shape="Feedback")`).
            case: "re-submitting the feedback already recorded",
            prepare: &["feedback"],
            segment: "feedback",
            extra: json!({"feedback": "USEFUL"}),
        },
        FindingRow {
            case: "re-submitting the feedback and the comments already recorded",
            prepare: &["feedback"],
            segment: "feedback",
            extra: json!({"feedback": "NOT_USEFUL", "comments": "triaged, a false positive"}),
        },
    ]
}

#[tokio::test]
async fn repeating_a_finding_metadata_write_consumes_no_instant() {
    for row in finding_rows() {
        let case = row.case;
        let router = new_router();
        let detector_id = enabled_detector(&router).await;
        let (status, sampled) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}/findings/create"),
            Some(json!({"findingTypes": SAMPLE_TYPES})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected create-sample-findings to answer 200, got {status} \
             {sampled:?}"
        );
        let ids = all_finding_ids(&router, &detector_id, 50).await;
        assert_eq!(
            ids.len(),
            SAMPLE_TYPES.len(),
            "case {case:?}: expected one finding per sample type, got {ids:?}"
        );

        // The body every call of this row carries: the ids, plus whatever the write
        // itself needs.
        let mut body = json!({"findingIds": ids});
        for (member, value) in row.extra.as_object().into_iter().flatten() {
            body[member] = value.clone();
        }

        // The writes that put every finding into the state the burst then asks for
        // again. These DO write, which is what makes the burst a repeat.
        for segment in row.prepare {
            let (status, prepared) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}/findings/{segment}"),
                Some(body.clone()),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the preparing {segment:?} write to answer 200, got \
                 {status} {prepared:?}"
            );
        }

        let before = findings_snapshot(&router, &detector_id, &ids, case).await;

        let path = format!("/detector/{detector_id}/findings/{}", row.segment);
        a_burst_of_it_consumes_no_instant(&router, &detector_id, case, &path, &body, 200).await;

        let after = findings_snapshot(&router, &detector_id, &ids, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: {NO_OPS_PER_BURST} repeats of a write that changes nothing must \
             leave every finding exactly as it was, expected {before:?}, got {after:?}"
        );
    }
}
