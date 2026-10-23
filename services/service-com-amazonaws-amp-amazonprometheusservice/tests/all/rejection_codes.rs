// SPDX-License-Identifier: CC0-1.0

//! Which ERROR CODE this service answers on two rejection paths, end to end over HTTP.
//!
//! Two rules, one suite each, asserted over a table of inputs rather than on one recorded
//! request:
//!
//! * a `revisionId` that does not name the revision a workspace's resource-based policy is
//!   currently at is a value the CALLER got wrong, so every operation taking that
//!   optimistic lock refuses it with the modelled `ValidationException` (400) naming the
//!   `revisionId` member — never with the 409 `ConflictException` that says the resource's
//!   STATUS forbids the call;
//! * a `resourceArn` the ARN-addressed `/tags/{resourceArn}` operations resolve to no live
//!   resource is refused 404 with `state::RESOURCE_NOT_FOUND_CODE`, whatever resource type
//!   the ARN names.
//!
//! The harness (`Amp`, `Reply`, `text`, `wait_for_due_transition`) is the one
//! `tests/all/workspace_lifecycle.rs` builds: one `MultiServiceRouter` with this service
//! mounted on one shared engine, so routing, the `restJson1` wire shapes and the error
//! envelopes are all exercised.

use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    stale_revision_requirement, NO_POLICY_REVISION_ID, RESOURCE_NOT_FOUND_CODE,
    RESOURCE_NOT_FOUND_MESSAGE, VALIDATION_REASON,
};
use serde_json::{json, Value};

/// A resource-based policy document: `policyDocument` is modelled a plain `String`, so
/// what it carries only has to differ between puts for a replacement to be observable.
fn policy_document(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": sid,
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()) },
            "Action": ["aps:QueryMetrics"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// `PUT /workspaces/{workspaceId}/policy`, with `revisionId` in the body where the model
/// binds it.
async fn put_policy(amp: &Amp, region: &str, workspace_id: &str, body: &Value) -> Reply {
    amp.call(
        "PUT",
        &format!("/workspaces/{workspace_id}/policy"),
        region,
        &body.to_string(),
    )
    .await
}

/// `DELETE /workspaces/{workspaceId}/policy`, with `revisionId` on the query string
/// (`@httpQuery`). The values this suite sends carry no character needing escaping.
async fn delete_policy(amp: &Amp, region: &str, workspace_id: &str, revision: &str) -> Reply {
    amp.call(
        "DELETE",
        &format!("/workspaces/{workspace_id}/policy?revisionId={revision}"),
        region,
        "",
    )
    .await
}

async fn describe_policy(amp: &Amp, region: &str, workspace_id: &str) -> Reply {
    amp.call(
        "GET",
        &format!("/workspaces/{workspace_id}/policy"),
        region,
        "",
    )
    .await
}

/// The policy a workspace currently holds, as `{document, status, revisionId}` — what a
/// refused call must leave untouched.
async fn stored_policy(amp: &Amp, region: &str, workspace_id: &str) -> (String, String, String) {
    let reply = describe_policy(amp, region, workspace_id).await;
    (
        text(&reply.body["policyDocument"]),
        text(&reply.body["policyStatus"]),
        text(&reply.body["revisionId"]),
    )
}

/// Every assertion the optimistic-lock refusal makes: the modelled `ValidationException`
/// at 400, naming the `revisionId` member and the requirement it failed in `fieldList`,
/// in `reason` and in `message` — the one composition every aps refusal takes.
fn assert_refused_as_a_bad_revision(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: a revisionId that is not the current one must be refused 400, \
         got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["reason"]),
        VALIDATION_REASON,
        "case {case:?}: `reason` is @required and must name the reason aps reports, \
         got {reply:?}"
    );
    assert_eq!(
        reply.body["fieldList"],
        json!([{ "name": "revisionId", "message": stale_revision_requirement() }]),
        "case {case:?}: expected one fieldList entry naming revisionId, got {reply:?}"
    );
    assert_eq!(
        text(&reply.body["message"]),
        format!("Invalid revisionId: {}", stale_revision_requirement()),
        "case {case:?}: expected the message to name the member and its requirement, \
         got {reply:?}"
    );
}

/// Every value of `revisionId` that is not the revision the policy is at is refused as a
/// bad MEMBER (400 `ValidationException`), by `PutResourcePolicy` and by
/// `DeleteResourcePolicy` alike, with the stored policy left exactly as it was.
///
/// One row per form the stale value can take: the revision the policy was at before the
/// last accepted put, a well-formed revision id no put of this store ever minted,
/// `NO_POLICY` (which asserts the workspace holds none, and it holds one), a value that is
/// not a revision id at all, and the empty string (`revisionId` carries no `@length`, so
/// the model accepts it and the handler decides).
#[tokio::test]
async fn a_revision_id_that_is_not_the_current_one_is_refused_as_a_bad_member() {
    struct Case {
        name: &'static str,
        /// `None` means "the revision the policy was at before the last accepted put",
        /// which only the running workspace can supply.
        revision: Option<&'static str>,
        /// Whether the refusal's text can be checked for not echoing the sent value: the
        /// requirement itself names `NO_POLICY`, so that row has nothing to compare.
        echo_free: bool,
    }

    let cases = [
        Case {
            name: "the previous revision",
            revision: None,
            echo_free: true,
        },
        Case {
            name: "a well-formed revision this store never minted",
            revision: Some("00000000-0000-4000-8000-000000000000"),
            echo_free: true,
        },
        Case {
            name: "NO_POLICY while the workspace holds a policy",
            revision: Some(NO_POLICY_REVISION_ID),
            echo_free: false,
        },
        Case {
            name: "a value that is not a revision id",
            revision: Some("not-a-revision"),
            echo_free: true,
        },
        Case {
            name: "the empty string",
            revision: Some(""),
            echo_free: true,
        },
    ];

    for operation in ["PutResourcePolicy", "DeleteResourcePolicy"] {
        for case in &cases {
            let region = "eu-west-1";
            let amp = Amp::new();
            let workspace_id = amp.active_workspace(region, "revision-lock").await;

            // Two accepted puts, so the workspace has a PREVIOUS revision as well as a
            // current one.
            let first = put_policy(
                &amp,
                region,
                &workspace_id,
                &json!({ "policyDocument": policy_document("First") }),
            )
            .await;
            let previous = text(&first.body["revisionId"]);
            wait_for_due_transition();
            let second = put_policy(
                &amp,
                region,
                &workspace_id,
                &json!({ "policyDocument": policy_document("Second") }),
            )
            .await;
            let current = text(&second.body["revisionId"]);
            wait_for_due_transition();
            let before = stored_policy(&amp, region, &workspace_id).await;

            let named = case.revision.map_or(previous.clone(), ToString::to_string);
            let label = format!("{operation} naming {}", case.name);
            let reply = if operation == "PutResourcePolicy" {
                put_policy(
                    &amp,
                    region,
                    &workspace_id,
                    &json!({
                        "policyDocument": policy_document("Stale"),
                        "revisionId": named,
                    }),
                )
                .await
            } else {
                delete_policy(&amp, region, &workspace_id, &named).await
            };
            assert_refused_as_a_bad_revision(&label, &reply);
            if case.echo_free && !named.is_empty() {
                let rendered = reply.body.to_string();
                assert!(
                    !rendered.contains(&named),
                    "case {label:?}: the refusal must not echo the submitted revisionId \
                     {named:?}, got {rendered:?}"
                );
            }
            assert_eq!(
                stored_policy(&amp, region, &workspace_id).await,
                before,
                "case {label:?}: a refused call must leave the stored policy untouched"
            );
            assert_eq!(
                before.2, current,
                "case {label:?}: the policy must still be at the revision the last \
                 accepted put minted"
            );

            // The control: the SAME call carrying the current revision is accepted, and a
            // put advances the revision.
            let accepted = if operation == "PutResourcePolicy" {
                let accepted = put_policy(
                    &amp,
                    region,
                    &workspace_id,
                    &json!({
                        "policyDocument": policy_document("Fresh"),
                        "revisionId": current,
                    }),
                )
                .await;
                assert_ne!(
                    text(&accepted.body["revisionId"]),
                    current,
                    "case {label:?}: an accepted put must advance the revisionId, \
                     got {accepted:?}"
                );
                accepted
            } else {
                delete_policy(&amp, region, &workspace_id, &current).await
            };
            assert_eq!(
                accepted.status, 202,
                "case {label:?}: the same call naming the current revision must be \
                 accepted, got {accepted:?}"
            );
        }
    }
}

/// The first put on a workspace that holds NO policy takes the same lock, so the only
/// value it accepts beside an absent one is `NO_POLICY` — every other value is the same
/// 400 `ValidationException`, and the workspace is left holding nothing.
#[tokio::test]
async fn the_first_put_refuses_every_revision_id_but_no_policy() {
    let region = "us-east-2";
    for (case, revision, accepted) in [
        ("NO_POLICY", NO_POLICY_REVISION_ID, true),
        ("a revision this store never minted", "00000000-0000-4000-8000-000000000000", false),
        ("a value that is not a revision id", "nothing-like-a-revision", false),
        ("the empty string", "", false),
    ] {
        let amp = Amp::new();
        let workspace_id = amp.active_workspace(region, "first-put").await;
        let reply = put_policy(
            &amp,
            region,
            &workspace_id,
            &json!({
                "policyDocument": policy_document("Initial"),
                "revisionId": revision,
            }),
        )
        .await;
        if accepted {
            assert_eq!(
                reply.status, 202,
                "case {case:?}: the first put must be accepted, got {reply:?}"
            );
            continue;
        }
        assert_refused_as_a_bad_revision(case, &reply);
        let still_empty = describe_policy(&amp, region, &workspace_id).await;
        assert_eq!(
            still_empty.status, 404,
            "case {case:?}: a refused first put must attach no policy, got {still_empty:?}"
        );
    }
}

/// `ConflictException` (409) still means what it means: a status that forbids the call at
/// all. A put onto a workspace that is still `CREATING` carries no `revisionId` to get
/// wrong, so the refusal must stay the 409 — this is the control that the fidelity fix
/// moved only the optimistic-lock refusal.
#[tokio::test]
async fn a_status_that_forbids_the_call_is_still_a_conflict() {
    let region = "us-west-2";
    let amp = Amp::new();
    let creating = amp
        .call("POST", "/workspaces", region, &json!({ "alias": "too-early" }).to_string())
        .await;
    let workspace_id = text(&creating.body["workspaceId"]);
    let reply = put_policy(
        &amp,
        region,
        &workspace_id,
        &json!({ "policyDocument": policy_document("TooEarly") }),
    )
    .await;
    assert_eq!(
        reply.status, 409,
        "case \"workspace CREATING\": a status refusal must stay 409, got {reply:?}"
    );
    assert_eq!(
        reply.error_code(),
        "ConflictException",
        "case \"workspace CREATING\": unexpected error code, got {reply:?}"
    );
}

/// An ARN the tagging operations resolve to no live resource is refused 404 with
/// `state::RESOURCE_NOT_FOUND_CODE`, whatever resource type the ARN names: an unknown
/// workspace, a workspace that was deleted, an anomaly detector that was deleted, a
/// detector inside a workspace that never existed, and an ARN naming a resource type this
/// service has no store for. All three of `TagResource`, `UntagResource` and
/// `ListTagsForResource` answer alike.
#[tokio::test]
async fn an_arn_that_resolves_to_nothing_is_refused_with_one_code() {
    let region = "us-west-2";
    let amp = Amp::new();
    let account = engine::emulator_account_id();

    // A workspace created and then deleted: its ARN was live and is not any more.
    let deleted_workspace = amp.active_workspace(region, "doomed").await;
    let deleted_workspace_arn = text(
        &amp.call("GET", &format!("/workspaces/{deleted_workspace}"), region, "")
            .await
            .body["workspace"]["arn"],
    );
    amp.delete(region, &deleted_workspace, None).await;

    // An anomaly detector created and then deleted inside a workspace that is still
    // there — the ARN type the recorded lifecycle exercises.
    let workspace_id = amp.active_workspace(region, "detectors").await;
    let created = amp
        .call(
            "POST",
            &format!("/workspaces/{workspace_id}/anomalydetectors"),
            region,
            &json!({
                "alias": "doomed-detector",
                "configuration": { "randomCutForest": { "query": "sum(up)" } },
            })
            .to_string(),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "seeding a detector must be accepted, got {created:?}"
    );
    let detector_arn = text(&created.body["arn"]);
    let detector_id = text(&created.body["anomalyDetectorId"]);
    wait_for_due_transition();
    amp.call(
        "DELETE",
        &format!("/workspaces/{workspace_id}/anomalydetectors/{detector_id}"),
        region,
        "",
    )
    .await;
    wait_for_due_transition();

    let cases: Vec<(&str, String)> = vec![
        (
            "a workspace ARN no store ever minted",
            format!("arn:aws:aps:{region}:{account}:workspace/ws-00000000-0000-4000-8000-000000000000"),
        ),
        ("the ARN of a deleted workspace", deleted_workspace_arn),
        ("the ARN of a deleted anomaly detector", detector_arn),
        (
            "an anomaly detector ARN inside a workspace that never existed",
            format!(
                "arn:aws:aps:{region}:{account}:anomalydetector/\
                 ws-00000000-0000-4000-8000-000000000000/ad-00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "a rule groups namespace ARN no store ever minted",
            format!(
                "arn:aws:aps:{region}:{account}:rulegroupsnamespace/\
                 ws-00000000-0000-4000-8000-000000000000/absent"
            ),
        ),
        (
            "a workspace ARN in another region",
            format!("arn:aws:aps:eu-north-1:{account}:workspace/ws-00000000-0000-4000-8000-000000000000"),
        ),
    ];

    for (case, arn) in cases {
        let replies = [
            ("ListTagsForResource", amp.list_tags(region, &arn).await),
            (
                "TagResource",
                amp.tag(region, &arn, &json!({ "team": "obs" })).await,
            ),
            ("UntagResource", amp.untag(region, &arn, &["team"]).await),
        ];
        for (operation, reply) in replies {
            let label = format!("{operation} on {case}");
            assert_eq!(
                reply.status, 404,
                "case {label:?}: an ARN resolving to nothing must answer 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                RESOURCE_NOT_FOUND_CODE,
                "case {label:?}: unexpected error code, got {reply:?}"
            );
            let message = text(&reply.body["message"]);
            assert_eq!(
                message, RESOURCE_NOT_FOUND_MESSAGE,
                "case {label:?}: expected the constant message \
                 {RESOURCE_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            assert!(
                !message.contains(&arn),
                "case {label:?}: the message must carry nothing from the request, \
                 got {message:?}"
            );
        }
    }

    // The control: a LIVE workspace ARN is tagged and read back, so the refusal above is
    // about the ARN resolving to nothing rather than about the tagging path itself.
    let live_arn = text(
        &amp.call("GET", &format!("/workspaces/{workspace_id}"), region, "")
            .await
            .body["workspace"]["arn"],
    );
    let tagged = amp.tag(region, &live_arn, &json!({ "team": "obs" })).await;
    assert_eq!(
        tagged.status, 200,
        "case \"a live workspace ARN\": TagResource must be accepted, got {tagged:?}"
    );
    let listed = amp.list_tags(region, &live_arn).await;
    assert_eq!(
        text(&listed.body["tags"]["team"]),
        "obs",
        "case \"a live workspace ARN\": the tag must be read back, got {listed:?}"
    );
}
