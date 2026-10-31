// SPDX-License-Identifier: CC0-1.0

//! The one status-only recorded-answer codec both calls whose record holds just
//! the status they answered are served from, driven end to end over the wire
//! through an operation from each.
//!
//! RULE under test: a call whose only answer the store stops determining is a
//! status answers its retry from the status that call recorded — never from the
//! status the clock or a later call has moved the resource to since — however
//! many times the retry arrives, and the retry changes nothing; a record that
//! does not hold that status is no answer at all. The status is the one member
//! such a record carries (`smithy_query(query="operation",
//! operation="UpdateCluster,CreateStream")`: a `@required` `status` on each
//! output), so both calls encode and read back the same record.
//!
//! The table spans the class rather than one request: BOTH operations whose
//! record holds nothing but the status they answered (`UpdateCluster`, which
//! answers the status it moved the cluster to, and `CreateStream`, which answers
//! the status it minted the stream in) crossed with request forms each admits —
//! the members an update may carry, and a create with and without tags — and
//! with the two states the clock can be in when the retry arrives: the
//! transition still owed, and the resource already settled past the status the
//! first call answered, which is the state that makes the recorded answer
//! un-derivable from the store.
//!
//! What this suite adds over `tests/all/idempotent_replay_result.rs` and
//! `tests/all/stream_lifecycle.rs`, which each pin one call's replay, and over
//! `tests/all/incomplete_replay_record.rs`, which pins the refusal of a record
//! holding no answered member, is that the two calls answer by the SAME rule:
//! one encoding of the status they record, and one decision about a record that
//! does not hold it.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    RecordedCall, RecordedStatus, POLICY_VERSION_MEMBER, STATUS_ACTIVE, STATUS_IDLE,
    STATUS_MEMBER, STATUS_PENDING_SETUP, STATUS_UPDATING, STREAM_STATUS_CREATING,
    STREAM_STATUS_DELETING,
};

use crate::harness::{wait_for_the_clock, Answer, Dsql};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The `StreamOrdering` and `StreamFormat` every `CreateStream` case sends.
/// Source: `smithy_query(query="shape", shape="StreamOrdering,StreamFormat")` —
/// each enum holds exactly one value, `UNORDERED` and `JSON`.
const STREAM_ORDERING: &str = "UNORDERED";
/// See [`STREAM_ORDERING`].
const STREAM_FORMAT: &str = "JSON";

/// One call of this service whose recorded answer is a status and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    /// `UpdateCluster`, which answers the status it moved the cluster to.
    UpdateCluster,
    /// `CreateStream`, which answers the status it minted the stream in.
    CreateStream,
}

impl Call {
    /// The operation's name, for naming the case in a failure.
    fn name(self) -> &'static str {
        match self {
            Call::UpdateCluster => "UpdateCluster",
            Call::CreateStream => "CreateStream",
        }
    }

    /// The status a served call of this kind answers: the one the clock then
    /// moves the resource on from.
    fn answers(self) -> &'static str {
        match self {
            Call::UpdateCluster => STATUS_UPDATING,
            Call::CreateStream => STREAM_STATUS_CREATING,
        }
    }

    /// The output member naming the resource the call acted on, so a retry can
    /// be shown to answer for that same resource.
    fn resource_member(self) -> &'static str {
        match self {
            Call::UpdateCluster => "identifier",
            Call::CreateStream => "streamIdentifier",
        }
    }
}

/// One row: an operation, the request body it sends (without its token), and how
/// the row names itself in a failure.
struct Case {
    call: Call,
    label: String,
    body: Value,
    token: String,
    /// The cluster the row is addressed at.
    cluster: String,
}

/// A role and a Kinesis stream this account can pass, as `CreateStream`'s
/// `targetDefinition` carries them.
fn kinesis_target() -> Value {
    json!({
        "kinesis": {
            "streamArn": format!(
                "arn:aws:kinesis:{REGION}:{}:stream/dsql-cdc",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "roleArn": format!("arn:aws:iam::{}:role/dsql-cdc", engine::EMULATOR_ACCOUNT_ID),
        },
    })
}

/// The request forms this rule generalises over, per operation: the members an
/// `UpdateCluster` may change (`smithy_query(query="operation",
/// operation="UpdateCluster")`: `deletionProtectionEnabled`,
/// `kmsEncryptionKey`), and a `CreateStream` with and without `tags`.
fn request_forms() -> Vec<(Call, &'static str, Value)> {
    vec![
        (
            Call::UpdateCluster,
            "turning deletion protection on",
            json!({"deletionProtectionEnabled": true}),
        ),
        (
            Call::UpdateCluster,
            "moving the cluster onto a customer-managed key",
            json!({"kmsEncryptionKey": "alias/dsql-shared-status"}),
        ),
        (
            Call::CreateStream,
            "a stream carrying no tags",
            json!({
                "targetDefinition": kinesis_target(),
                "ordering": STREAM_ORDERING,
                "format": STREAM_FORMAT,
            }),
        ),
        (
            Call::CreateStream,
            "a stream carrying tags",
            json!({
                "targetDefinition": kinesis_target(),
                "ordering": STREAM_ORDERING,
                "format": STREAM_FORMAT,
                "tags": {"owner": "status-replay"},
            }),
        ),
    ]
}

/// Token forms spanning what `ClientToken` admits (`smithy_query(query="shape",
/// shape="ClientToken")`: `@length(1, 128)`, `@pattern("^[!-~]+$")`): an
/// ordinary one and one at the published maximum, each made distinct per row so
/// one ledger holds every row's record.
fn token_shapes(index: usize) -> Vec<(&'static str, String)> {
    vec![
        ("an ordinary token", format!("status-replay-{index}")),
        (
            "128 characters, ClientToken's maximum",
            format!("{index:03}{}", "z".repeat(125)),
        ),
    ]
}

/// Every row this suite drives, each with a settled cluster of its own — created
/// before the one wait, so the suite waits for the clock once rather than per
/// row.
async fn cases(dsql: &Dsql) -> Vec<Case> {
    let mut pending: Vec<(Call, String, Value, String)> = Vec::new();
    for (index, (call, form, body)) in request_forms().into_iter().enumerate() {
        for (shape, token) in token_shapes(index) {
            pending.push((
                call,
                format!("{}, {form}, with {shape}", call.name()),
                body.clone(),
                token,
            ));
        }
    }
    let clusters = dsql.create_settled_many(pending.len()).await;
    pending
        .into_iter()
        .zip(clusters)
        .map(|((call, label, body, token), cluster)| Case {
            call,
            label,
            body,
            token,
            cluster,
        })
        .collect()
}

/// The request `case` names, carrying its token.
async fn send(dsql: &Dsql, case: &Case) -> Answer {
    let mut body = case.body.clone();
    body["clientToken"] = json!(case.token);
    match case.call {
        Call::UpdateCluster => dsql.update(&case.cluster, body).await,
        Call::CreateStream => dsql.create_stream(&case.cluster, body).await,
    }
}

/// The status the resource the row acted on carries RIGHT NOW, read back through
/// the implemented reader of its own kind.
async fn live_status(dsql: &Dsql, case: &Case, resource: &str) -> String {
    let read = match case.call {
        Call::UpdateCluster => dsql.get(&case.cluster).await,
        Call::CreateStream => dsql.get_stream(&case.cluster, resource).await,
    };
    assert_eq!(
        read.status, 200,
        "case {:?}: the resource the call acted on must read back, got {} {:?}",
        case.label, read.status, read.body
    );
    read.str(STATUS_MEMBER)
}

/// How many streams the row's cluster holds, so a retry can be shown to mint
/// none.
async fn stream_count(dsql: &Dsql, case: &Case) -> usize {
    dsql.list_streams(&case.cluster, "?max-results=100")
        .await
        .get("streams")
        .as_array()
        .map_or(0, Vec::len)
}

/// Retry `case` three times, asserting every answer is the status and the
/// resource the first call answered, and that nothing the row can observe moved.
///
/// `live_status_pinned` is the status the resource itself must keep across the
/// retries, where the row is in a state the clock no longer moves — a stage in
/// which a transition is still owed pins the ledger's answer only, since the
/// clock is free to settle the resource while the retries run.
async fn retries_answer_the_record(
    dsql: &Dsql,
    case: &Case,
    stage: &str,
    answered: &str,
    resource: &str,
    live_status_pinned: Option<&str>,
) {
    let streams_before = stream_count(dsql, case).await;
    for round in 0..3 {
        let retry = send(dsql, case).await;
        assert_eq!(
            retry.status, 200,
            "case {:?} ({stage}): retry #{round} must answer 200, got {} {:?}",
            case.label, retry.status, retry.body
        );
        assert_eq!(
            retry.str(STATUS_MEMBER),
            answered,
            "case {:?} ({stage}): retry #{round} must answer the status the first call answered \
             ({answered:?}), got {:?}",
            case.label,
            retry.str(STATUS_MEMBER)
        );
        assert_eq!(
            retry.str(case.call.resource_member()),
            resource,
            "case {:?} ({stage}): retry #{round} must answer for the resource the first call \
             acted on ({resource:?}), got {:?}",
            case.label,
            retry.str(case.call.resource_member())
        );
        assert_eq!(
            stream_count(dsql, case).await,
            streams_before,
            "case {:?} ({stage}): retry #{round} must leave the cluster's streams as they were \
             ({streams_before}), got {}",
            case.label,
            stream_count(dsql, case).await
        );
        if let Some(pinned) = live_status_pinned {
            assert_eq!(
                live_status(dsql, case, resource).await,
                pinned,
                "case {:?} ({stage}): retry #{round} must leave the resource's live status as it \
                 was ({pinned:?}), got {:?}",
                case.label,
                live_status(dsql, case, resource).await
            );
        }
    }
}

/// RULE: both calls whose record holds nothing but the status they answered
/// serve a retry that status — while the clock still owes the transition and
/// after it has settled the resource past it — and the retry changes nothing.
#[tokio::test]
async fn both_status_only_replays_answer_the_recorded_status() {
    let dsql = Dsql::in_region(REGION);
    let cases = cases(&dsql).await;

    // Every row's first call, before the clock settles anything, so the status
    // each answered is the transient one.
    let mut answered: Vec<(String, String)> = Vec::with_capacity(cases.len());
    for case in &cases {
        let first = send(&dsql, case).await;
        assert_eq!(
            first.status, 200,
            "case {:?}: the first call must answer 200, got {} {:?}",
            case.label, first.status, first.body
        );
        assert_eq!(
            first.str(STATUS_MEMBER),
            case.call.answers(),
            "case {:?}: the first call must answer the transient status ({:?}), got {:?}",
            case.label,
            case.call.answers(),
            first.str(STATUS_MEMBER)
        );
        answered.push((
            first.str(STATUS_MEMBER),
            first.str(case.call.resource_member()),
        ));
    }

    // The transition still owed: the retry answers the record, which here reads
    // the same as the live resource.
    for (case, (status, resource)) in cases.iter().zip(&answered) {
        retries_answer_the_record(
            &dsql,
            case,
            "the transition still owed",
            status,
            resource,
            None,
        )
        .await;
    }

    wait_for_the_clock().await;

    // The clock has settled the resource past the status the first call
    // answered, so the live resource no longer determines that answer.
    for (case, (status, resource)) in cases.iter().zip(&answered) {
        // `ACTIVE` is the settled status of both kinds here — a cluster's and a
        // stream's enum spell it the same way.
        let settled = live_status(&dsql, case, resource).await;
        assert_eq!(
            settled, STATUS_ACTIVE,
            "case {:?}: the clock must settle the resource to {STATUS_ACTIVE:?}, got {settled:?}",
            case.label
        );
        assert_ne!(
            settled, *status,
            "case {:?}: the settled status must differ from the recorded one, so an answer read \
             off the store is distinguishable; both are {settled:?}",
            case.label
        );
        retries_answer_the_record(
            &dsql,
            case,
            "the resource settled",
            status,
            resource,
            Some(STATUS_ACTIVE),
        )
        .await;
    }
}

/// RULE: the status-only recorded-answer codec is a round trip — the status a
/// served call answers comes back from its record unchanged, under the member
/// both outputs carry, against the resource the call acted on — and a record
/// that does not hold that member decodes to no answer at all, which is the one
/// decision both handlers refuse on.
#[test]
fn the_recorded_status_codec_round_trips_the_answered_status() {
    // The resources such a record is keyed by: a cluster and a stream, both
    // `^[a-z0-9]{26}$` (`smithy_query(query="shape",
    // shape="ClusterId,StreamId")`).
    let resources = [
        ("a cluster identifier", "aaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("a stream identifier", "0123456789abcdefghijklmnop"),
    ];
    // Statuses spanning both catalogs a status-only record carries. Source:
    // `smithy_query(query="shape", shape="ClusterStatus,StreamStatus")` —
    // ClusterStatus: CREATING | ACTIVE | IDLE | INACTIVE | UPDATING | DELETING |
    // DELETED | FAILED | PENDING_SETUP | PENDING_DELETE; StreamStatus: CREATING
    // | ACTIVE | DELETING | DELETED | FAILED | IMPAIRED.
    let statuses = [
        ("the status an update answers", STATUS_UPDATING),
        ("the status a stream create answers", STREAM_STATUS_CREATING),
        ("a cluster status waiting on a peering", STATUS_PENDING_SETUP),
        ("a cluster status nothing here answers", STATUS_IDLE),
        ("a stream status on the way out", STREAM_STATUS_DELETING),
        ("a stream status this crate holds no constant for", "IMPAIRED"),
    ];

    for (what_resource, resource) in resources {
        for (what, status) in statuses {
            let case = format!("{what} ({status}) against {what_resource}");
            let recorded = RecordedStatus {
                status: status.to_string(),
            };

            let record = recorded.clone().into_record(resource);
            assert_eq!(
                record.resource, resource,
                "case {case:?}: the record must be keyed by the resource the call acted on, \
                 got {:?}",
                record.resource
            );
            assert_eq!(
                record.answered(STATUS_MEMBER),
                Some(status),
                "case {case:?}: the record must keep the status under the member both outputs \
                 carry, got {:?}",
                record.answered(STATUS_MEMBER)
            );
            assert_eq!(
                RecordedStatus::from_record(&record),
                Some(recorded),
                "case {case:?}: the record must decode back to the answer it was written from, \
                 got {:?}",
                RecordedStatus::from_record(&record)
            );

            // A record that holds no status is no answer, whether it holds
            // nothing at all or only some other operation's member.
            for (what, partial) in [
                ("a record holding nothing", RecordedCall::new(resource)),
                (
                    "a record holding another member only",
                    RecordedCall::new(resource).answering(POLICY_VERSION_MEMBER, status),
                ),
            ] {
                assert_eq!(
                    RecordedStatus::from_record(&partial),
                    None,
                    "case {case:?}: {what} is no answer, got {:?}",
                    RecordedStatus::from_record(&partial)
                );
            }
        }
    }
}
