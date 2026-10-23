// SPDX-License-Identifier: CC0-1.0

//! A workspace's resource-based policy, end to end over HTTP: `PutResourcePolicy`,
//! `DescribeResourcePolicy` and `DeleteResourcePolicy`, each asserted against what the
//! service PERSISTED rather than against what the request carried.
//!
//! The rules under test, one per suite:
//!
//! * a put stores the caller's document at a freshly minted revision and every later
//!   read reports that stored document, revision and status;
//! * `revisionId` is an optimistic lock — absent it acts regardless, otherwise it must
//!   name the revision the workspace currently holds (`NO_POLICY` when it holds none),
//!   and any other value is refused with `ValidationException` (400) having changed
//!   nothing;
//! * `clientToken` makes a put idempotent, so a replay mints no second revision;
//! * a policy moves `CREATING`/`UPDATING` -> `ACTIVE` and `DELETING` -> gone on the
//!   clock, and a put or delete is refused while it or its workspace is not `ACTIVE`;
//! * every one of the three operations answers `ResourceNotFoundException` (404) for a
//!   resource that is not there.
//!
//! The suite drives the same request path as `workspace_lifecycle` — whose `Amp`
//! harness it reuses — so routing, the `restJson1` wire shapes, the modelled response
//! statuses and the modelled error envelopes are all exercised.

use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::ops::types::WORKSPACE_POLICY_STATUS_CODE_VALUES;
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    NO_POLICY_REVISION_ID, POLICY_STATUS_ACTIVE, POLICY_STATUS_CREATING, POLICY_STATUS_DELETING,
    POLICY_STATUS_UPDATING,
};

/// The route every resource-policy operation is bound to:
/// `PUT|GET|DELETE /workspaces/{workspaceId}/policy`.
fn policy_uri(workspace_id: &str) -> String {
    format!("/workspaces/{workspace_id}/policy")
}

/// `PutResourcePolicy`: `policyDocument`, and optionally `revisionId` / `clientToken`,
/// all in the JSON body.
async fn put_policy(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    body: &serde_json::Value,
) -> Reply {
    amp.call("PUT", &policy_uri(workspace_id), region, &body.to_string())
        .await
}

async fn describe_policy(amp: &Amp, region: &str, workspace_id: &str) -> Reply {
    amp.call("GET", &policy_uri(workspace_id), region, "").await
}

/// `DeleteResourcePolicy`: `revisionId` and `clientToken` ride the query string
/// (`@httpQuery`), and a revision id this service mints carries only hex and `-`, so it
/// needs no escaping.
async fn delete_policy(
    amp: &Amp,
    region: &str,
    workspace_id: &str,
    query: &[(&str, &str)],
) -> Reply {
    let uri = if query.is_empty() {
        policy_uri(workspace_id)
    } else {
        let pairs: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        format!("{}?{}", policy_uri(workspace_id), pairs.join("&"))
    };
    amp.call("DELETE", &uri, region, "").await
}

/// A resource-based policy document of the form the operation's own documentation
/// describes: statements granting Prometheus-compatible actions on the workspace to a
/// principal. `policyDocument` is modelled as a plain `String`, so the service stores
/// whatever the caller sent — these rows only differ in the document's size and shape.
fn policy_document(sid_prefix: &str, statements: usize) -> String {
    let statements: Vec<serde_json::Value> = (0..statements)
        .map(|index| {
            serde_json::json!({
                "Sid": format!("{sid_prefix}{index}"),
                "Effect": "Allow",
                "Principal": {
                    "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
                },
                "Action": [
                    "aps:QueryMetrics",
                    "aps:GetLabels",
                    "aps:GetSeries",
                    "aps:GetMetricMetadata",
                ],
                "Resource": "*",
            })
        })
        .collect();
    serde_json::json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A revision id is server-minted and opaque to the caller, so what a test can hold it
/// to is its form: non-empty, within what `revisionId`'s `String` can carry on the
/// query string, and the UUID this service mints (`state::mint_revision_id`).
fn assert_well_formed_revision_id(case: &str, revision_id: &str) {
    assert!(
        !revision_id.is_empty() && revision_id.len() <= 64,
        "case {case:?}: revisionId must be a non-empty, bounded string, got {revision_id:?}"
    );
    let groups: Vec<&str> = revision_id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: revisionId must be a UUID, got {revision_id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit())),
        "case {case:?}: a revisionId's UUID must be hexadecimal, got {revision_id:?}"
    );
}

/// What a describe reports: the document, the status and the revision.
#[derive(Debug, PartialEq, Eq)]
struct Described {
    document: String,
    status: String,
    revision: String,
}

async fn described(amp: &Amp, region: &str, workspace_id: &str) -> Described {
    let reply = describe_policy(amp, region, workspace_id).await;
    assert_eq!(
        reply.status, 200,
        "describing a policy that is attached must answer 200, got {reply:?}"
    );
    Described {
        document: text(&reply.body["policyDocument"]),
        status: text(&reply.body["policyStatus"]),
        revision: text(&reply.body["revisionId"]),
    }
}

/// Every status this crate reports for a resource-based policy is a value the modelled
/// `WorkspacePolicyStatusCode` enum declares — the catalog comes from the model, not
/// from this test.
#[test]
fn reported_statuses_are_modelled_workspace_policy_status_codes() {
    for status in [
        POLICY_STATUS_CREATING,
        POLICY_STATUS_ACTIVE,
        POLICY_STATUS_UPDATING,
        POLICY_STATUS_DELETING,
    ] {
        assert!(
            WORKSPACE_POLICY_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled WorkspacePolicyStatusCode \
             ({WORKSPACE_POLICY_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A put answers from the record it persisted and a describe reads that same record
/// back, at the revision the put minted: one row per thing the rule generalises over —
/// the document's size and shape, and the region the request is signed for. The second
/// put of each row is the same rule applied to a workspace that already holds a policy:
/// the document is REPLACED and the revision moves on.
#[tokio::test]
async fn put_persists_the_document_and_describe_reads_it_back() {
    struct Case {
        name: &'static str,
        region: &'static str,
        document: String,
        replacement: String,
    }

    let cases = vec![
        Case {
            name: "one statement",
            region: "us-west-2",
            document: policy_document("Share", 1),
            replacement: policy_document("Reshare", 1),
        },
        Case {
            name: "several statements",
            region: "eu-west-1",
            document: policy_document("Share", 5),
            replacement: policy_document("Share", 6),
        },
        Case {
            name: "an empty statement list",
            region: "us-east-1",
            document: serde_json::json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
            replacement: policy_document("Share", 1),
        },
        Case {
            name: "a document carrying non-ascii text",
            region: "ap-southeast-2",
            document: serde_json::json!({
                "Version": "2012-10-17",
                "Statement": [],
                "Comment": "métriques partagées — 共有",
            })
            .to_string(),
            replacement: policy_document("Partage", 2),
        },
        Case {
            name: "a large document",
            region: "eu-central-1",
            document: policy_document("Share", 40),
            replacement: policy_document("Share", 41),
        },
    ];

    for case in &cases {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(case.region, "policy-holder").await;

        let put = put_policy(
            &amp,
            case.region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": case.document }),
        )
        .await;
        assert_eq!(
            put.status, 202,
            "case {:?}: PutResourcePolicy must answer its modelled 202, got {put:?}",
            case.name
        );
        assert_eq!(
            text(&put.body["policyStatus"]),
            POLICY_STATUS_CREATING,
            "case {:?}: a first put leaves the policy CREATING, got {put:?}",
            case.name
        );
        let first_revision = text(&put.body["revisionId"]);
        assert_well_formed_revision_id(case.name, &first_revision);

        // The describe reads back the stored record, byte for byte.
        assert_eq!(
            described(&amp, case.region, &workspace_id).await,
            Described {
                document: case.document.clone(),
                status: POLICY_STATUS_CREATING.to_string(),
                revision: first_revision.clone(),
            },
            "case {:?}: a describe must report what the put persisted",
            case.name
        );

        // The clock owes CREATING -> ACTIVE; nothing else about the record moves.
        wait_for_due_transition();
        assert_eq!(
            described(&amp, case.region, &workspace_id).await,
            Described {
                document: case.document.clone(),
                status: POLICY_STATUS_ACTIVE.to_string(),
                revision: first_revision.clone(),
            },
            "case {:?}: once the transition is due the policy is ACTIVE at the same revision",
            case.name
        );

        // A second put over an existing policy REPLACES the document, reports UPDATING
        // and mints a revision of its own.
        let update = put_policy(
            &amp,
            case.region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": case.replacement }),
        )
        .await;
        assert_eq!(
            update.status, 202,
            "case {:?}: a put over an active policy must answer 202, got {update:?}",
            case.name
        );
        assert_eq!(
            text(&update.body["policyStatus"]),
            POLICY_STATUS_UPDATING,
            "case {:?}: a put over an existing policy leaves it UPDATING, got {update:?}",
            case.name
        );
        let second_revision = text(&update.body["revisionId"]);
        assert_well_formed_revision_id(case.name, &second_revision);
        assert_ne!(
            second_revision, first_revision,
            "case {:?}: every accepted put must mint a new revision",
            case.name
        );
        wait_for_due_transition();
        assert_eq!(
            described(&amp, case.region, &workspace_id).await,
            Described {
                document: case.replacement.clone(),
                status: POLICY_STATUS_ACTIVE.to_string(),
                revision: second_revision,
            },
            "case {:?}: a describe must report the replacement document",
            case.name
        );
    }
}

/// Which revision id a put of a workspace that ALREADY holds a policy may name: absent
/// or the current revision is accepted, and every other value is the stale writer the
/// optimistic lock exists to refuse — `ValidationException` (400), with the stored policy
/// left exactly as it was.
#[tokio::test]
async fn the_revision_lock_accepts_only_the_current_revision() {
    /// Which revision id the row sends, resolved against the live policy.
    #[derive(Debug)]
    enum Revision {
        /// "If you don't specify a revision ID, the policy is updated regardless of its
        /// current revision."
        Absent,
        Current,
        /// The revision the policy was at before the last accepted put.
        Previous,
        /// `NO_POLICY` names "this workspace holds no policy", which is false here.
        NoPolicy,
        /// A well-formed revision id no put of this store ever minted.
        Unknown,
        /// A value that is not a revision id at all.
        Malformed,
    }

    struct Case {
        name: &'static str,
        revision: Revision,
        accepted: bool,
    }

    let cases = vec![
        Case {
            name: "no revisionId at all",
            revision: Revision::Absent,
            accepted: true,
        },
        Case {
            name: "the current revision",
            revision: Revision::Current,
            accepted: true,
        },
        Case {
            name: "the previous revision",
            revision: Revision::Previous,
            accepted: false,
        },
        Case {
            name: "NO_POLICY on a workspace that holds a policy",
            revision: Revision::NoPolicy,
            accepted: false,
        },
        Case {
            name: "a revision this store never minted",
            revision: Revision::Unknown,
            accepted: false,
        },
        Case {
            name: "a value that is not a revision id",
            revision: Revision::Malformed,
            accepted: false,
        },
    ];

    let region = "us-west-2";
    for case in &cases {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "locked").await;

        // Two accepted puts, so the row has both a current and a previous revision.
        let first = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": policy_document("First", 1) }),
        )
        .await;
        let previous = text(&first.body["revisionId"]);
        wait_for_due_transition();
        let second = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": policy_document("Second", 2) }),
        )
        .await;
        let current = text(&second.body["revisionId"]);
        wait_for_due_transition();
        let before = described(&amp, region, &workspace_id).await;
        assert_eq!(
            before.revision, current,
            "case {:?}: the stored policy must be at the revision the last put minted",
            case.name
        );

        let revision = match case.revision {
            Revision::Absent => None,
            Revision::Current => Some(current.clone()),
            Revision::Previous => Some(previous.clone()),
            Revision::NoPolicy => Some(NO_POLICY_REVISION_ID.to_string()),
            Revision::Unknown => Some("00000000-0000-0000-0000-000000000000".to_string()),
            Revision::Malformed => Some("not-a-revision".to_string()),
        };
        let document = policy_document("Third", 3);
        let mut body = serde_json::json!({ "policyDocument": document });
        if let Some(revision) = &revision {
            body["revisionId"] = serde_json::Value::String(revision.clone());
        }
        let reply = put_policy(&amp, region, &workspace_id, &body).await;

        if case.accepted {
            assert_eq!(
                reply.status, 202,
                "case {:?}: the put must be accepted, got {reply:?}",
                case.name
            );
            let minted = text(&reply.body["revisionId"]);
            assert_ne!(
                minted, current,
                "case {:?}: the accepted put must mint a new revision",
                case.name
            );
            wait_for_due_transition();
            assert_eq!(
                described(&amp, region, &workspace_id).await,
                Described {
                    document: document.clone(),
                    status: POLICY_STATUS_ACTIVE.to_string(),
                    revision: minted,
                },
                "case {:?}: the accepted put must be what later reads report",
                case.name
            );
        } else {
            assert_eq!(
                reply.status, 400,
                "case {:?}: a put naming {revision:?} must be refused with 400, got {reply:?}",
                case.name
            );
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {:?}: unexpected error code, got {reply:?}",
                case.name
            );
            assert_eq!(
                described(&amp, region, &workspace_id).await,
                before,
                "case {:?}: a refused put must change nothing",
                case.name
            );
        }
    }
}

/// The FIRST put on a workspace that holds no policy: `NO_POLICY` is the only revision
/// id that names "no policy yet", so it and an absent revision id are accepted while
/// any other value is refused. The same holds again once a policy has been deleted —
/// the workspace is back to holding none.
#[tokio::test]
async fn the_first_put_takes_the_no_policy_revision() {
    struct Case {
        name: &'static str,
        revision: Option<&'static str>,
        accepted: bool,
    }

    let cases = [
        Case {
            name: "no revisionId at all",
            revision: None,
            accepted: true,
        },
        Case {
            name: "NO_POLICY",
            revision: Some(NO_POLICY_REVISION_ID),
            accepted: true,
        },
        Case {
            name: "a revision id on a workspace with no policy",
            revision: Some("00000000-0000-0000-0000-000000000000"),
            accepted: false,
        },
        Case {
            name: "a lowercase no_policy, which is a different string",
            revision: Some("no_policy"),
            accepted: false,
        },
    ];

    let region = "eu-west-1";
    for case in &cases {
        // Two workspaces per row: one that never held a policy, and one whose policy
        // was put and then deleted — both hold none when the row's put runs.
        let amp = Amp::new();
        let fresh = amp.active_workspace(region, "fresh").await;
        let emptied = amp.active_workspace(region, "emptied").await;
        put_policy(
            &amp,
            region,
            &emptied,
            &serde_json::json!({ "policyDocument": policy_document("Gone", 1) }),
        )
        .await;
        wait_for_due_transition();
        let deleted = delete_policy(&amp, region, &emptied, &[]).await;
        assert_eq!(
            deleted.status, 202,
            "case {:?}: DeleteResourcePolicy must answer its modelled 202, got {deleted:?}",
            case.name
        );
        wait_for_due_transition();

        for workspace_id in [&fresh, &emptied] {
            let document = policy_document("Opening", 1);
            let mut body = serde_json::json!({ "policyDocument": document });
            if let Some(revision) = case.revision {
                body["revisionId"] = serde_json::Value::String(revision.to_string());
            }
            let reply = put_policy(&amp, region, workspace_id, &body).await;

            if case.accepted {
                assert_eq!(
                    reply.status, 202,
                    "case {:?} on {workspace_id}: the first put must be accepted, got {reply:?}",
                    case.name
                );
                assert_eq!(
                    text(&reply.body["policyStatus"]),
                    POLICY_STATUS_CREATING,
                    "case {:?} on {workspace_id}: a first put leaves the policy CREATING, \
                     got {reply:?}",
                    case.name
                );
                let minted = text(&reply.body["revisionId"]);
                assert_well_formed_revision_id(case.name, &minted);
                assert_eq!(
                    described(&amp, region, workspace_id).await,
                    Described {
                        document,
                        status: POLICY_STATUS_CREATING.to_string(),
                        revision: minted,
                    },
                    "case {:?} on {workspace_id}: the describe must report the first put",
                    case.name
                );
            } else {
                assert_eq!(
                    reply.status, 400,
                    "case {:?} on {workspace_id}: the first put must be refused with 400, \
                     got {reply:?}",
                    case.name
                );
                assert_eq!(
                    reply.error_code(),
                    "ValidationException",
                    "case {:?} on {workspace_id}: unexpected error code, got {reply:?}",
                    case.name
                );
                let still_empty = describe_policy(&amp, region, workspace_id).await;
                assert_eq!(
                    still_empty.status, 404,
                    "case {:?} on {workspace_id}: a refused first put must attach nothing, \
                     got {still_empty:?}",
                    case.name
                );
            }
        }
    }
}

/// `clientToken` carries `@idempotencyToken`: replaying the call that produced the
/// current revision answers with THAT revision and mints no second one, whatever the
/// replay's own document says — while a different token (or none) is a new put.
#[tokio::test]
async fn replaying_a_client_token_mints_no_second_revision() {
    let region = "us-east-1";
    let amp = Amp::new();
    let workspace_id = amp.active_workspace(region, "idempotent").await;

    let document = policy_document("Once", 1);
    let first = put_policy(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "policyDocument": document, "clientToken": "token-one" }),
    )
    .await;
    assert_eq!(
        first.status, 202,
        "case \"first put\": must be accepted, got {first:?}"
    );
    let revision = text(&first.body["revisionId"]);

    // One row per form a replay takes: the identical request, and the same token with a
    // different document. Neither may mint a revision or change the stored document.
    let replays = [
        ("identical replay", document.clone()),
        ("same token, different document", policy_document("Twice", 7)),
    ];
    for (case, replayed_document) in &replays {
        let replay = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({
                "policyDocument": replayed_document,
                "clientToken": "token-one",
            }),
        )
        .await;
        assert_eq!(
            replay.status, 202,
            "case {case:?}: a replay must be accepted, got {replay:?}"
        );
        assert_eq!(
            text(&replay.body["revisionId"]),
            revision,
            "case {case:?}: a replay must answer with the revision the first call minted, \
             got {replay:?}"
        );
        assert_eq!(
            described(&amp, region, &workspace_id).await,
            Described {
                document: document.clone(),
                status: text(&replay.body["policyStatus"]),
                revision: revision.clone(),
            },
            "case {case:?}: a replay must leave the stored document alone"
        );
    }

    // A different token is a different call: it mints the next revision.
    wait_for_due_transition();
    let next_document = policy_document("Thrice", 2);
    let next = put_policy(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "policyDocument": next_document, "clientToken": "token-two" }),
    )
    .await;
    assert_eq!(
        next.status, 202,
        "case \"a second token\": must be accepted, got {next:?}"
    );
    let next_revision = text(&next.body["revisionId"]);
    assert_ne!(
        next_revision, revision,
        "case \"a second token\": a put under a fresh token must mint a new revision"
    );
    wait_for_due_transition();
    assert_eq!(
        described(&amp, region, &workspace_id).await,
        Described {
            document: next_document,
            status: POLICY_STATUS_ACTIVE.to_string(),
            revision: next_revision,
        },
        "case \"a second token\": the later put must be what reads report"
    );
}

/// A delete takes the same optimistic lock, then leaves the policy `DELETING` until the
/// clock owes its removal — after which the workspace holds no policy at all.
#[tokio::test]
async fn delete_makes_the_policy_observably_gone() {
    struct Case {
        name: &'static str,
        /// `true` when the row names the policy's current revision on the delete.
        with_revision: bool,
        /// `true` when the row sends a `clientToken` and replays the delete with it.
        with_token: bool,
    }

    let cases = [
        Case {
            name: "no revisionId and no clientToken",
            with_revision: false,
            with_token: false,
        },
        Case {
            name: "the current revisionId",
            with_revision: true,
            with_token: false,
        },
        Case {
            name: "a clientToken, replayed",
            with_revision: false,
            with_token: true,
        },
        Case {
            name: "both the current revisionId and a clientToken",
            with_revision: true,
            with_token: true,
        },
    ];

    let region = "us-west-2";
    for case in &cases {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "to-be-emptied").await;
        let put = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": policy_document("Doomed", 1) }),
        )
        .await;
        let revision = text(&put.body["revisionId"]);
        wait_for_due_transition();

        let mut query: Vec<(&str, &str)> = Vec::new();
        if case.with_revision {
            query.push(("revisionId", revision.as_str()));
        }
        if case.with_token {
            query.push(("clientToken", "delete-token"));
        }
        let deleted = delete_policy(&amp, region, &workspace_id, &query).await;
        assert_eq!(
            deleted.status, 202,
            "case {:?}: the delete must answer its modelled 202, got {deleted:?}",
            case.name
        );

        // DELETING, and still readable at its revision until the removal is due.
        assert_eq!(
            described(&amp, region, &workspace_id).await,
            Described {
                document: policy_document("Doomed", 1),
                status: POLICY_STATUS_DELETING.to_string(),
                revision: revision.clone(),
            },
            "case {:?}: a deleted policy stays readable as DELETING until the clock owes \
             its removal",
            case.name
        );

        if case.with_token {
            // Re-sending the very call that started the deletion is that same call.
            let replay = delete_policy(&amp, region, &workspace_id, &query).await;
            assert_eq!(
                replay.status, 202,
                "case {:?}: replaying the delete's clientToken must be accepted, got {replay:?}",
                case.name
            );
        }

        wait_for_due_transition();
        let gone = describe_policy(&amp, region, &workspace_id).await;
        assert_eq!(
            gone.status, 404,
            "case {:?}: once the removal is due the policy must be gone, got {gone:?}",
            case.name
        );
        assert_eq!(
            gone.error_code(),
            "ResourceNotFoundException",
            "case {:?}: unexpected error code, got {gone:?}",
            case.name
        );

        // And a delete of a policy that is no longer there names a missing resource.
        let again = delete_policy(&amp, region, &workspace_id, &query).await;
        assert_eq!(
            again.status, 404,
            "case {:?}: deleting a policy that is gone must answer 404, got {again:?}",
            case.name
        );
        assert_eq!(
            again.error_code(),
            "ResourceNotFoundException",
            "case {:?}: unexpected error code, got {again:?}",
            case.name
        );

        // The workspace itself is untouched by its policy's deletion.
        let workspace = amp
            .call("GET", &format!("/workspaces/{workspace_id}"), region, "")
            .await;
        assert_eq!(
            workspace.status, 200,
            "case {:?}: deleting the policy must leave the workspace, got {workspace:?}",
            case.name
        );
    }
}

/// A delete naming a revision the policy is not at is the same stale writer a put's
/// lock refuses: `ValidationException` (400), with the policy still attached.
#[tokio::test]
async fn delete_refuses_a_revision_the_policy_is_not_at() {
    let region = "eu-central-1";
    for (case, revision) in [
        ("the previous revision", None),
        (
            "a revision this store never minted",
            Some("00000000-0000-0000-0000-000000000000".to_string()),
        ),
        ("NO_POLICY", Some(NO_POLICY_REVISION_ID.to_string())),
    ] {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "locked-delete").await;
        let first = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": policy_document("First", 1) }),
        )
        .await;
        let previous = text(&first.body["revisionId"]);
        wait_for_due_transition();
        let second = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": policy_document("Second", 1) }),
        )
        .await;
        let current = text(&second.body["revisionId"]);
        wait_for_due_transition();
        let before = described(&amp, region, &workspace_id).await;

        let named = revision.unwrap_or(previous);
        let refused = delete_policy(
            &amp,
            region,
            &workspace_id,
            &[("revisionId", named.as_str())],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a delete naming {named:?} must be refused with 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: unexpected error code, got {refused:?}"
        );
        assert_eq!(
            described(&amp, region, &workspace_id).await,
            before,
            "case {case:?}: a refused delete must change nothing"
        );
        assert_eq!(
            before.revision, current,
            "case {case:?}: the policy must still be at the revision the last put minted"
        );
    }
}

/// "Update/Deletion is disallowed until Resource-based Policy is ACTIVE and Workspace is
/// ACTIVE" (`WorkspacePolicyStatusCode`): one row per transient state a put or a delete
/// can meet — a workspace still `CREATING`, a policy `CREATING`, one `UPDATING`, and one
/// `DELETING` — each refused with `ConflictException` (409).
#[tokio::test]
async fn a_policy_or_workspace_that_is_not_active_refuses_a_put_and_a_delete() {
    let region = "us-west-2";

    // A workspace that is still CREATING holds no policy, so only the workspace's own
    // status can refuse the first put.
    let amp = Amp::new();
    let creating = amp
        .call(
            "POST",
            "/workspaces",
            region,
            &serde_json::json!({ "alias": "still-creating" }).to_string(),
        )
        .await;
    let creating_id = text(&creating.body["workspaceId"]);
    let refused = put_policy(
        &amp,
        region,
        &creating_id,
        &serde_json::json!({ "policyDocument": policy_document("TooEarly", 1) }),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case \"workspace CREATING\": the put must be refused with 409, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ConflictException",
        "case \"workspace CREATING\": unexpected error code, got {refused:?}"
    );
    let unattached = describe_policy(&amp, region, &creating_id).await;
    assert_eq!(
        unattached.status, 404,
        "case \"workspace CREATING\": the refusal must attach nothing, got {unattached:?}"
    );

    // Each remaining row: drive the policy into the transient status, then try both a
    // put and a delete while it is there.
    for case in ["policy CREATING", "policy UPDATING", "policy DELETING"] {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "transient").await;
        let document = policy_document("Settling", 1);
        let first = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": document }),
        )
        .await;
        let mut revision = text(&first.body["revisionId"]);
        if case != "policy CREATING" {
            wait_for_due_transition();
        }
        if case == "policy UPDATING" {
            let update = put_policy(
                &amp,
                region,
                &workspace_id,
                &serde_json::json!({ "policyDocument": policy_document("Settling", 2) }),
            )
            .await;
            revision = text(&update.body["revisionId"]);
        }
        if case == "policy DELETING" {
            let deleted = delete_policy(&amp, region, &workspace_id, &[]).await;
            assert_eq!(
                deleted.status, 202,
                "case {case:?}: the delete that starts the removal must be accepted, \
                 got {deleted:?}"
            );
        }
        let before = described(&amp, region, &workspace_id).await;

        let put = put_policy(
            &amp,
            region,
            &workspace_id,
            &serde_json::json!({ "policyDocument": policy_document("Racing", 1) }),
        )
        .await;
        let delete = delete_policy(
            &amp,
            region,
            &workspace_id,
            &[("revisionId", revision.as_str())],
        )
        .await;
        for (operation, reply) in [("PutResourcePolicy", put), ("DeleteResourcePolicy", delete)] {
            assert_eq!(
                reply.status, 409,
                "case {case:?}: {operation} must be refused with 409, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ConflictException",
                "case {case:?}: {operation} returned an unexpected error code, got {reply:?}"
            );
        }
        assert_eq!(
            described(&amp, region, &workspace_id).await,
            before,
            "case {case:?}: the refusals must change nothing"
        );
    }
}

/// Every resource-policy operation answers `ResourceNotFoundException` (404) when the
/// resource it names is not there: the workspace, or a policy the workspace does not
/// hold. The report names the resource that is missing, and deleting the workspace takes
/// its policy with it.
#[tokio::test]
async fn a_missing_workspace_or_policy_is_a_resource_not_found() {
    let region = "us-east-1";
    let amp = Amp::new();

    // An unknown workspace, in each form a caller can name one.
    for unknown in ["ws-00000000-0000-0000-0000-000000000000", "not-a-workspace"] {
        let put = put_policy(
            &amp,
            region,
            unknown,
            &serde_json::json!({ "policyDocument": policy_document("Nowhere", 1) }),
        )
        .await;
        let describe = describe_policy(&amp, region, unknown).await;
        let delete = delete_policy(&amp, region, unknown, &[]).await;
        for (operation, reply) in [
            ("PutResourcePolicy", put),
            ("DescribeResourcePolicy", describe),
            ("DeleteResourcePolicy", delete),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {unknown:?}: {operation} must answer 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ResourceNotFoundException",
                "case {unknown:?}: {operation} returned an unexpected error code, got {reply:?}"
            );
            assert_eq!(
                text(&reply.body["resourceId"]),
                unknown,
                "case {unknown:?}: {operation} must name the resource it could not find, \
                 got {reply:?}"
            );
        }
    }

    // A known workspace that holds no policy: the policy is what is missing.
    let workspace_id = amp.active_workspace(region, "policy-free").await;
    let describe = describe_policy(&amp, region, &workspace_id).await;
    let delete = delete_policy(&amp, region, &workspace_id, &[]).await;
    for (operation, reply) in [
        ("DescribeResourcePolicy", describe),
        ("DeleteResourcePolicy", delete),
    ] {
        assert_eq!(
            reply.status, 404,
            "case \"no policy attached\": {operation} must answer 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case \"no policy attached\": {operation} returned an unexpected error code, \
             got {reply:?}"
        );
    }

    // Deleting the workspace releases the policy attached to it.
    put_policy(
        &amp,
        region,
        &workspace_id,
        &serde_json::json!({ "policyDocument": policy_document("Attached", 1) }),
    )
    .await;
    wait_for_due_transition();
    let deleted = amp
        .call("DELETE", &format!("/workspaces/{workspace_id}"), region, "")
        .await;
    assert_eq!(
        deleted.status, 202,
        "case \"workspace deleted\": DeleteWorkspace must answer 202, got {deleted:?}"
    );
    wait_for_due_transition();
    let orphaned = describe_policy(&amp, region, &workspace_id).await;
    assert_eq!(
        orphaned.status, 404,
        "case \"workspace deleted\": the policy must go with the workspace, got {orphaned:?}"
    );
    assert_eq!(
        orphaned.error_code(),
        "ResourceNotFoundException",
        "case \"workspace deleted\": unexpected error code, got {orphaned:?}"
    );
}
