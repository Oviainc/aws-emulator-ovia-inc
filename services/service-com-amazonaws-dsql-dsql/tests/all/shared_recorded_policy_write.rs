// SPDX-License-Identifier: CC0-1.0

//! The one recorded-policy-write codec both policy writers of this service
//! answer a retry from, driven end to end over the wire through an operation
//! from each.
//!
//! RULE under test: a policy write whose `clientToken` the ledger already served
//! answers its retry from the version that call recorded — the version it issued
//! or the version it removed, exactly as the original call answered it, however
//! many times the retry arrives and whatever has happened to the cluster's
//! policy since — and the retry writes nothing of its own. The version is the
//! one member both outputs carry (`smithy_query(query="operation",
//! operation="PutClusterPolicy,DeleteClusterPolicy")`: a `@required`
//! `policyVersion` on each), so both writers encode and read back the same
//! record.
//!
//! The table spans the class rather than one request: BOTH writers
//! (`PutClusterPolicy` attaching a document, `DeleteClusterPolicy` removing one)
//! crossed with the token forms `ClientToken` admits and with what has happened
//! to the cluster's policy between the first call and its retry — nothing, a
//! later document attached over it, and the policy removed altogether — which is
//! exactly the set of states that make the recorded answer un-derivable from the
//! store.
//!
//! What this suite adds over `tests/all/cluster_policy.rs` and
//! `tests/all/idempotent_replay_result.rs`, which each pin one writer's replay,
//! and over `tests/all/incomplete_replay_record.rs`, which pins the refusal of a
//! record holding no answered member, is that the two writers answer by the SAME
//! rule: one encoding of the version they record, and one decision about a
//! record that does not hold it.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::json;
use service_com_amazonaws_dsql_dsql::state::{
    RecordedCall, RecordedPolicyWrite, POLICY_VERSION_MEMBER, STATUS_MEMBER,
};

use crate::harness::{Answer, Dsql};
use crate::policy_version_conflict_and_lockout::benign_document;

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// One policy writer of this service: an operation whose answer is a policy
/// version its retry has to be served from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Writer {
    /// `PutClusterPolicy`, which answers the version it issued.
    Put,
    /// `DeleteClusterPolicy`, which answers the version it removed.
    Delete,
}

/// Every policy writer of this service.
const WRITERS: [Writer; 2] = [Writer::Put, Writer::Delete];

impl Writer {
    /// The operation's name, for naming the case in a failure.
    fn name(self) -> &'static str {
        match self {
            Writer::Put => "PutClusterPolicy",
            Writer::Delete => "DeleteClusterPolicy",
        }
    }
}

/// What has happened to the cluster's policy between the first call and its
/// retry — the states in which the store no longer determines what that call
/// answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Since {
    /// Nothing further ran.
    Nothing,
    /// A later document was attached, so the attached version is another one.
    AnotherAttached,
    /// The policy was removed, so no version is attached at all.
    NoneAttached,
}

/// Every such state.
const SINCE: [Since; 3] = [Since::Nothing, Since::AnotherAttached, Since::NoneAttached];

impl Since {
    /// How the case names this state in a failure.
    fn label(self) -> &'static str {
        match self {
            Since::Nothing => "nothing having run since",
            Since::AnotherAttached => "another document attached since",
            Since::NoneAttached => "the policy removed since",
        }
    }
}

/// Token forms spanning what `ClientToken` admits (`smithy_query(query="shape",
/// shape="ClientToken")`: `@length(1, 128)`, `@pattern("^[!-~]+$")`): the
/// shortest, an ordinary one, punctuation only, and one at the published
/// maximum. The characters a URI query string reads itself (`&`, `=`, `#`, `%`,
/// `+`, `?`) are left out — `DeleteClusterPolicy` carries its token in the query
/// string — since they are not what this rule generalises over.
fn token_shapes() -> Vec<(&'static str, String)> {
    vec![
        ("one character, ClientToken's minimum", "t".to_string()),
        ("an ordinary token", "retry-of-a-policy-write".to_string()),
        ("punctuation only", "~!*()-_.".to_string()),
        ("128 characters, ClientToken's maximum", "z".repeat(128)),
    ]
}

/// A settled cluster the writer `which` can be addressed at, with a document
/// already attached where the writer removes one.
async fn target(which: Writer, dsql: &Dsql) -> String {
    let cluster = dsql.create_settled().await;
    if which == Writer::Delete {
        let attached = dsql
            .put_policy(&cluster, json!({"policy": benign_document(0)}))
            .await;
        assert_eq!(
            attached.status,
            200,
            "{}: the document the removal removes must attach, got {} {:?}",
            which.name(),
            attached.status,
            attached.body
        );
    }
    cluster
}

/// The write `which` names, carrying `token` as its `clientToken` — in the body
/// for the attach, in the `client-token` query parameter for the removal.
async fn write(which: Writer, dsql: &Dsql, cluster: &str, token: &str) -> Answer {
    match which {
        Writer::Put => {
            dsql.put_policy(
                cluster,
                json!({"policy": benign_document(0), "clientToken": token}),
            )
            .await
        }
        Writer::Delete => {
            dsql.delete_policy(cluster, &format!("?client-token={token}"))
                .await
        }
    }
}

/// The policy the cluster holds right now, as `GetClusterPolicy` reports it: the
/// status and the attached version, so a retry can be shown to have changed
/// neither.
async fn attached_policy(dsql: &Dsql, cluster: &str) -> (u16, String) {
    let read = dsql.get_policy(cluster).await;
    (read.status, read.str(POLICY_VERSION_MEMBER))
}

/// Put the cluster's policy into the state `since` names, with calls carrying no
/// `clientToken` of their own, so nothing in the ledger changes.
async fn make_it_so(since: Since, dsql: &Dsql, case: &str, cluster: &str) {
    match since {
        Since::Nothing => {}
        Since::AnotherAttached => {
            let attached = dsql
                .put_policy(cluster, json!({"policy": benign_document(1)}))
                .await;
            assert_eq!(
                attached.status, 200,
                "case {case:?}: the later document must attach, got {} {:?}",
                attached.status, attached.body
            );
        }
        Since::NoneAttached => {
            if attached_policy(dsql, cluster).await.0 == 200 {
                let removed = dsql.delete_policy(cluster, "").await;
                assert_eq!(
                    removed.status, 200,
                    "case {case:?}: the removal must succeed, got {} {:?}",
                    removed.status, removed.body
                );
            }
        }
    }
}

/// RULE: both policy writers answer a retry from the one version they recorded —
/// the version the original call issued or removed — whatever has since happened
/// to the cluster's policy, however many times the retry arrives, and without
/// writing a policy of their own.
#[tokio::test]
async fn both_policy_writers_answer_a_retry_from_the_one_recorded_version() {
    for which in WRITERS {
        for (shape, token) in token_shapes() {
            for since in SINCE {
                let case = format!("{}, with {shape}, {}", which.name(), since.label());
                let dsql = Dsql::in_region(REGION);
                let cluster = target(which, &dsql).await;

                let first = write(which, &dsql, &cluster, &token).await;
                assert_eq!(
                    first.status, 200,
                    "case {case:?}: the first write must answer 200, got {} {:?}",
                    first.status, first.body
                );
                let recorded = first.str(POLICY_VERSION_MEMBER);
                assert!(
                    !recorded.is_empty(),
                    "case {case:?}: the first write must answer a policyVersion, got {:?}",
                    first.body
                );

                make_it_so(since, &dsql, &case, &cluster).await;
                let left = attached_policy(&dsql, &cluster).await;

                for round in 0..3 {
                    let retry = write(which, &dsql, &cluster, &token).await;
                    assert_eq!(
                        retry.status, 200,
                        "case {case:?}: retry #{round} must answer 200, got {} {:?}",
                        retry.status, retry.body
                    );
                    assert_eq!(
                        retry.str(POLICY_VERSION_MEMBER),
                        recorded,
                        "case {case:?}: retry #{round} must answer the version the first call \
                         answered ({recorded:?}), got {:?}",
                        retry.str(POLICY_VERSION_MEMBER)
                    );
                    assert_eq!(
                        attached_policy(&dsql, &cluster).await,
                        left,
                        "case {case:?}: retry #{round} must leave the attached policy as the \
                         case left it ({left:?}), got {:?}",
                        attached_policy(&dsql, &cluster).await
                    );
                }
            }
        }
    }
}

/// RULE: the recorded-policy-write codec is a round trip — the version a served
/// write answers comes back from its record unchanged, under the member both
/// outputs carry, against the cluster the write acted on — and a record that
/// does not hold that member decodes to no answer at all, which is the one
/// decision both writers refuse on.
#[test]
fn the_recorded_policy_write_codec_round_trips_the_answered_version() {
    // The cluster a record is keyed by: `ClusterId`'s `^[a-z0-9]{26}$`.
    let cluster = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
    // Versions spanning what a policy write answers: the first version a cluster
    // issues, one several writes deep, and one past a 32-bit counter
    // (`state::Cluster::next_policy_version` counts in `u64`).
    let versions = [
        ("the first version a cluster issues", "1"),
        ("a version several writes deep", "42"),
        ("a version past a 32-bit counter", "4294967296"),
    ];

    for (what, version) in versions {
        let case = format!("{what} ({version})");
        let recorded = RecordedPolicyWrite {
            policy_version: version.to_string(),
        };

        let record = recorded.clone().into_record(cluster);
        assert_eq!(
            record.resource, cluster,
            "case {case:?}: the record must be keyed by the cluster written, got {:?}",
            record.resource
        );
        assert_eq!(
            record.answered(POLICY_VERSION_MEMBER),
            Some(version),
            "case {case:?}: the record must keep the version under the member both outputs \
             carry, got {:?}",
            record.answered(POLICY_VERSION_MEMBER)
        );
        assert_eq!(
            RecordedPolicyWrite::from_record(&record),
            Some(recorded),
            "case {case:?}: the record must decode back to the answer it was written from, \
             got {:?}",
            RecordedPolicyWrite::from_record(&record)
        );

        // A record that holds no version is no answer, whether it holds nothing
        // at all or only some other operation's member.
        for (what, partial) in [
            ("a record holding nothing", RecordedCall::new(cluster)),
            (
                "a record holding another member only",
                RecordedCall::new(cluster).answering(STATUS_MEMBER, version),
            ),
        ] {
            assert_eq!(
                RecordedPolicyWrite::from_record(&partial),
                None,
                "case {case:?}: {what} is no answer, got {:?}",
                RecordedPolicyWrite::from_record(&partial)
            );
        }
    }
}
