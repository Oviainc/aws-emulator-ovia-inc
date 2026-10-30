// SPDX-License-Identifier: CC0-1.0

//! The one recorded-delete codec both deletes of this service answer a retry
//! from, driven end to end over the wire through an operation from each.
//!
//! RULE under test: a delete whose `clientToken` the ledger already served
//! answers its retry from the record that call kept — the retired resource's
//! identifier, ARN, status and creation time, exactly as the original call
//! answered them, however many times the retry arrives and whether it arrives
//! while the clock still holds the resource or after the clock has removed it —
//! and a record missing any one of the members a served delete keeps is refused
//! with 500 `InternalServerException` rather than served as an answer. The
//! members are the ones both outputs carry beside the retired resource's
//! identifier (`smithy_query(query="operation",
//! operation="DeleteCluster,DeleteStream")`: a `@required` `arn`, `status` and
//! `creationTime` on each).
//!
//! The table spans the class rather than one request: BOTH deletes
//! (`DeleteCluster` over a cluster, `DeleteStream` over a stream of a cluster)
//! crossed with the token forms `ClientToken` admits, both retry windows, and
//! each member a record can be missing. What this suite adds over
//! `tests/all/delete_cluster_idempotency.rs` and `tests/all/stream_deletion.rs`,
//! which each pin one delete, is that the two answer by the SAME rule: the one
//! encoding of the answer they record, and the one decision about a record that
//! does not hold all of it.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    encode_instant, request_fingerprint, ComAmazonawsDsqlDsqlState, RecordedCall, RecordedDelete,
    TokenScope, ARN_MEMBER, CREATION_TIME_MEMBER, STATUS_DELETING, STATUS_MEMBER,
    STREAM_STATUS_DELETING,
};

use crate::error_contract::stream_request;
use crate::harness::{
    expected_arn, expected_stream_arn, wait_for_the_clock, Answer, Dsql,
};

/// The region every case signs for, so a seeded ARN is the one its resource was
/// minted with.
const REGION: &str = "us-east-1";

/// One delete this service serves: the operation whose recorded answer a retry
/// is served from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delete {
    /// `DeleteCluster`, which retires the cluster itself.
    Cluster,
    /// `DeleteStream`, which deletes a stream of a cluster.
    Stream,
}

/// Every delete of this service.
const DELETES: [Delete; 2] = [Delete::Cluster, Delete::Stream];

impl Delete {
    /// The operation's name, for naming the case in a failure.
    fn name(self) -> &'static str {
        match self {
            Delete::Cluster => "DeleteCluster",
            Delete::Stream => "DeleteStream",
        }
    }

    /// The ledger this delete records its answer in.
    fn scope(self) -> TokenScope {
        match self {
            Delete::Cluster => TokenScope::DeleteCluster,
            Delete::Stream => TokenScope::DeleteStream,
        }
    }

    /// The status a served delete of this resource reports.
    fn deleting(self) -> &'static str {
        match self {
            Delete::Cluster => STATUS_DELETING,
            Delete::Stream => STREAM_STATUS_DELETING,
        }
    }
}

/// A persisted resource one of the deletes can be addressed at: the cluster, and
/// the stream of it when the case deletes a stream.
struct Target {
    cluster: String,
    stream: Option<String>,
}

impl Target {
    /// The resource the delete retires, which is what its record is keyed by.
    fn resource(&self) -> &str {
        self.stream.as_deref().unwrap_or(&self.cluster)
    }

    /// The ARN that resource was minted with in `REGION`.
    fn arn(&self) -> String {
        match &self.stream {
            None => expected_arn(REGION, &self.cluster),
            Some(stream) => expected_stream_arn(REGION, &self.cluster, stream),
        }
    }

    /// The fingerprint the handler computes for a delete addressed at this
    /// resource — what binds a recorded token to this very request.
    fn fingerprint(&self) -> String {
        match &self.stream {
            None => request_fingerprint(std::slice::from_ref(&self.cluster)),
            Some(stream) => {
                request_fingerprint(&[self.cluster.clone(), stream.clone()])
            }
        }
    }
}

/// A persisted resource `which` can be deleted, settled so the delete is
/// accepted straight away.
async fn target(which: Delete, dsql: &Dsql) -> Target {
    let cluster = dsql.create_settled().await;
    match which {
        Delete::Cluster => Target {
            cluster,
            stream: None,
        },
        Delete::Stream => {
            let created = dsql
                .create_stream(
                    &cluster,
                    stream_request(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc"),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "{}: CreateStream must succeed, got {} {:?}",
                which.name(),
                created.status,
                created.body
            );
            let stream = created.str("streamIdentifier");
            wait_for_the_clock().await;
            Target {
                cluster,
                stream: Some(stream),
            }
        }
    }
}

/// The delete `which` names, carrying `token` as its `client-token`.
async fn delete(dsql: &Dsql, target: &Target, token: &str) -> Answer {
    match &target.stream {
        None => dsql.delete_with_token(&target.cluster, token).await,
        Some(stream) => {
            dsql.delete_stream(&target.cluster, stream, &format!("?client-token={token}"))
                .await
        }
    }
}

/// A read of the resource the delete is addressed at.
async fn read_back(dsql: &Dsql, target: &Target) -> Answer {
    match &target.stream {
        None => dsql.get(&target.cluster).await,
        Some(stream) => dsql.get_stream(&target.cluster, stream).await,
    }
}

/// Every `@required` member of the delete's output as one comparable value, so a
/// retry's WHOLE answer is compared with the original's rather than one member
/// of it.
fn answered(which: Delete, answer: &Answer) -> Value {
    match which {
        Delete::Cluster => json!({
            "arn": answer.get("arn"),
            "creationTime": answer.get("creationTime"),
            "identifier": answer.get("identifier"),
            "status": answer.get("status"),
        }),
        Delete::Stream => json!({
            "arn": answer.get("arn"),
            "clusterIdentifier": answer.get("clusterIdentifier"),
            "creationTime": answer.get("creationTime"),
            "status": answer.get("status"),
            "streamIdentifier": answer.get("streamIdentifier"),
        }),
    }
}

/// Token forms spanning what `ClientToken` admits (`smithy_query(query="shape",
/// shape="ClientToken")`: `@length(1, 128)`, `@pattern("^[!-~]+$")`): the
/// shortest, an ordinary one, punctuation only, and one at the published
/// maximum. The characters a URI query string reads itself (`&`, `=`, `#`, `%`,
/// `+`, `?`) are left out: they are not what this rule generalises over.
fn token_shapes() -> Vec<(&'static str, String)> {
    vec![
        ("one character, ClientToken's minimum", "t".to_string()),
        ("an ordinary token", "retry-of-a-delete".to_string()),
        ("punctuation only", "~!*()-_.".to_string()),
        ("128 characters, ClientToken's maximum", "z".repeat(128)),
    ]
}

/// RULE: both deletes answer a retry from the one answer they recorded — the
/// same ARN, status, creation time and retired-resource identifier the original
/// call answered — in the window the clock still holds the resource and in the
/// window after it has removed it, however many times the retry arrives, and
/// without retiring anything else in its place.
#[tokio::test]
async fn both_deletes_answer_a_retry_from_the_one_recorded_answer() {
    for which in DELETES {
        for (shape, token) in token_shapes() {
            let case = format!("{}, with {shape}", which.name());
            let dsql = Dsql::in_region(REGION);
            let target = target(which, &dsql).await;

            let first = delete(&dsql, &target, &token).await;
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first delete must answer 200, got {} {:?}",
                first.status, first.body
            );
            let expected = answered(which, &first);
            assert_eq!(
                first.str("status"),
                which.deleting(),
                "case {case:?}: the first delete must report {}, got {:?}",
                which.deleting(),
                first.str("status")
            );

            // The retry inside the window the clock still holds the resource in.
            let inside = delete(&dsql, &target, &token).await;
            assert_eq!(
                inside.status, 200,
                "case {case:?}: the retry inside the deleting window must answer 200, \
                 got {} {:?}",
                inside.status, inside.body
            );
            assert_eq!(
                answered(which, &inside),
                expected,
                "case {case:?}: the retry inside the deleting window must answer the \
                 first call's result {expected:?}, got {:?}",
                answered(which, &inside)
            );

            // Once the clock has removed the resource the store cannot answer
            // this request at all: the record is the only place that answer
            // still exists.
            wait_for_the_clock().await;
            let gone = read_back(&dsql, &target).await;
            assert_eq!(
                gone.status, 404,
                "case {case:?}: the clock must have removed the resource, got {} {:?}",
                gone.status, gone.body
            );

            for round in 0..3 {
                let late = delete(&dsql, &target, &token).await;
                assert_eq!(
                    late.status, 200,
                    "case {case:?}: retry #{round} after the removal must answer 200, \
                     got {} {:?}",
                    late.status, late.body
                );
                assert_eq!(
                    answered(which, &late),
                    expected,
                    "case {case:?}: retry #{round} after the removal must answer the \
                     first call's result {expected:?}, got {:?}",
                    answered(which, &late)
                );
            }

            // The retries had no effect of their own: a delete that is NOT a
            // repeat of a recorded one still finds the resource gone.
            let fresh = delete(&dsql, &target, "a-token-no-call-used").await;
            assert_eq!(
                fresh.status, 404,
                "case {case:?}: a delete that repeats nothing must report the live \
                 store, got {} {:?}",
                fresh.status, fresh.body
            );
        }
    }
}

/// The creation time a seeded record answers with: a sub-second instant, so the
/// codec's encoding is exercised on more than a whole number of seconds.
fn seeded_creation_time() -> SystemTime {
    UNIX_EPOCH + Duration::new(1_700_000_000, 500_000_000)
}

/// RULE: a record holding every member a served delete keeps is served as that
/// delete's answer, and a record missing any one of them is refused with 500
/// `InternalServerException` — never served as an answer with an empty member,
/// and never by retiring the resource in its place — whichever delete the retry
/// addresses.
#[tokio::test]
async fn an_incomplete_delete_record_is_refused_by_both_deletes() {
    // The row `None` is the control: the complete record a served delete keeps.
    let rows = [
        None,
        Some(ARN_MEMBER),
        Some(STATUS_MEMBER),
        Some(CREATION_TIME_MEMBER),
    ];

    for which in DELETES {
        for missing in rows {
            let case = format!(
                "{}, a record missing {}",
                which.name(),
                missing.unwrap_or("nothing")
            );
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let dsql = Dsql::sharing(&engine, REGION);
            let target = target(which, &dsql).await;

            let complete = RecordedDelete {
                resource: target.resource().to_string(),
                arn: target.arn(),
                status: which.deleting().to_string(),
                creation_time: seeded_creation_time(),
            };
            // The same record written out member by member, so a row can drop
            // exactly one of them.
            let mut seeded = RecordedCall::new(&complete.resource);
            for (member, value) in [
                (STATUS_MEMBER, complete.status.clone()),
                (ARN_MEMBER, complete.arn.clone()),
                (
                    CREATION_TIME_MEMBER,
                    encode_instant(complete.creation_time),
                ),
            ] {
                if missing != Some(member) {
                    seeded = seeded.answering(member, &value);
                }
            }
            if missing.is_none() {
                assert_eq!(
                    seeded,
                    complete.clone().into_record(),
                    "case {case:?}: a served delete records exactly the three members \
                     this row writes, got {:?}",
                    complete.clone().into_record()
                );
            }

            let token = "a-token-the-ledger-already-holds";
            engine
                .state::<ComAmazonawsDsqlDsqlState>()
                .lock()
                .tokens(which.scope())
                .record(REGION, token.to_string(), target.fingerprint(), seeded);

            let answer = delete(&dsql, &target, token).await;
            match missing {
                None => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: a complete record must be served, got {} {:?}",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        answer.str("arn"),
                        complete.arn,
                        "case {case:?}: the answer must carry the recorded ARN, got {:?}",
                        answer.str("arn")
                    );
                    assert_eq!(
                        answer.str("status"),
                        complete.status,
                        "case {case:?}: the answer must carry the recorded status, got {:?}",
                        answer.str("status")
                    );
                    let seconds = answer.get("creationTime").as_f64().unwrap_or_default();
                    let expected = complete
                        .creation_time
                        .duration_since(UNIX_EPOCH)
                        .expect("the seeded instant is after the epoch")
                        .as_secs_f64();
                    assert!(
                        (seconds - expected).abs() < 1e-3,
                        "case {case:?}: the answer must carry the recorded creation time \
                         {expected}, got {seconds}"
                    );
                }
                Some(member) => {
                    assert_eq!(
                        (answer.status, answer.error.as_deref()),
                        (500, Some("InternalServerException")),
                        "case {case:?}: a record missing {member} must be refused with 500 \
                         InternalServerException, got {} {:?}",
                        answer.status,
                        answer.body
                    );
                }
            }

            // Served or refused, the replay retired nothing: the resource the
            // call named is still exactly where the case left it.
            let read = read_back(&dsql, &target).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: answering from the ledger must retire nothing, got {} {:?}",
                read.status, read.body
            );
            assert_ne!(
                read.str("status"),
                which.deleting(),
                "case {case:?}: answering from the ledger must not start a deletion, got {:?}",
                read.str("status")
            );
        }
    }
}

/// RULE: the recorded-delete codec is a round trip — every member a served
/// delete answers comes back from its record unchanged, the creation time at the
/// resolution the store holds it — and a record missing any one of those members
/// decodes to no answer at all.
#[test]
fn the_recorded_delete_codec_round_trips_every_answered_member() {
    // Instants spanning what a stored creation time can be: the epoch itself, a
    // whole second, the largest sub-second part, and an instant this process
    // really read.
    let instants = [
        ("the epoch", UNIX_EPOCH),
        (
            "a whole second",
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        ),
        (
            "the largest sub-second part",
            UNIX_EPOCH + Duration::new(1_700_000_000, 999_999_999),
        ),
        ("an instant this process read", SystemTime::now()),
    ];
    // One answer of each delete's shape: a cluster ARN, and a stream ARN nested
    // under its cluster's.
    let answers = [
        (
            Delete::Cluster,
            "aaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            expected_arn(REGION, "aaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ),
        (
            Delete::Stream,
            "bbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            expected_stream_arn(
                REGION,
                "aaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        ),
    ];

    for (which, resource, arn) in answers {
        for (when, creation_time) in instants {
            let case = format!("{}, {when}", which.name());
            let recorded = RecordedDelete {
                resource: resource.clone(),
                arn: arn.clone(),
                status: which.deleting().to_string(),
                creation_time,
            };

            let record = recorded.clone().into_record();
            assert_eq!(
                record.resource, recorded.resource,
                "case {case:?}: the record must be keyed by the retired resource, got {:?}",
                record.resource
            );
            assert_eq!(
                RecordedDelete::from_record(&record),
                Some(recorded.clone()),
                "case {case:?}: the record must decode back to the answer it was written \
                 from, got {:?}",
                RecordedDelete::from_record(&record)
            );

            // A record missing any one of the members a delete keeps is no
            // answer: that is the one decision both handlers refuse on.
            for dropped in [ARN_MEMBER, STATUS_MEMBER, CREATION_TIME_MEMBER] {
                let mut partial = RecordedCall::new(&recorded.resource);
                for (member, value) in [
                    (STATUS_MEMBER, recorded.status.clone()),
                    (ARN_MEMBER, recorded.arn.clone()),
                    (CREATION_TIME_MEMBER, encode_instant(creation_time)),
                ] {
                    if member != dropped {
                        partial = partial.answering(member, &value);
                    }
                }
                assert_eq!(
                    RecordedDelete::from_record(&partial),
                    None,
                    "case {case:?}: a record missing {dropped} is no answer, got {:?}",
                    RecordedDelete::from_record(&partial)
                );
            }
        }
    }
}
