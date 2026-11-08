// SPDX-License-Identifier: CC0-1.0

//! The SUCCESS STATUS line Amplify answers a served request with, over the wire.
//!
//! Every case drives the real wire path (the shared harness in
//! `tests/all/app_branch_lifecycle.rs`), so what is asserted is the status and the body
//! an AWS SDK client — or a hand-rolled client that parses a body only when the status
//! says there is one — actually receives.
//!
//! The rule under test: a successful response that carries NO document, which is only
//! an operation whose modelled output declares no members, is answered `204 No Content`
//! when the request that asked for it REMOVED something, and a response that reports a
//! document is answered `200 OK`. In this service that makes `UntagResource`
//! (`DELETE /tags/{resourceArn}`, no output members) the one No Content answer, while
//! its `POST` sibling `TagResource` — the same empty output shape, a write rather than
//! a removal — and every `DELETE` that reports the record it acted on keep 200.
//!
//! The framing follows the status: a No Content answer carries no body bytes at all and
//! states the length of that nothing, while a 200 over an output shape with no members
//! is the empty JSON object.
//!
//! The table spans the class the rule generalises over: both halves of the empty-output
//! pair, every implemented `DELETE` of this service, and the two taggable resource
//! kinds; and the removal rows vary what the request names (one key, the modelled
//! maximum of fifty, a key the resource never carried) so the status is shown not to
//! depend on how much the removal actually changed. The status of a REFUSED request is
//! pinned too: No Content is a success code, so no refusal may answer it.

use serde_json::{json, Value};

use crate::app_branch_lifecycle::{string_member, Emulator, Reply};
use crate::resource_tags::{encode_label, list_tags, seed, tag_resource, Kind, KINDS};
use crate::resource_untagging::untag_resource;

/// The status a response carrying no document answers a served removal with.
const NO_CONTENT: u16 = 204;

/// The status a response that reports a document answers with.
const OK: u16 = 200;

/// The length a No Content answer states for the body it does not carry.
const NO_CONTENT_LENGTH: &str = "0";

/// The operations this suite drives, each one named as the model names it.
#[derive(Debug, Clone, Copy)]
enum Operation {
    /// `POST /tags/{resourceArn}`; `TagResourceResponse` declares no members.
    TagResource,
    /// `DELETE /tags/{resourceArn}`; `UntagResourceResponse` declares no members.
    UntagResource,
    /// `DELETE /apps/{appId}`; reports `app`.
    DeleteApp,
    /// `DELETE /apps/{appId}/branches/{branchName}`; reports `branch`.
    DeleteBranch,
    /// `DELETE /apps/{appId}/domains/{domainName}`; reports `domainAssociation`.
    DeleteDomainAssociation,
    /// `DELETE /apps/{appId}/branches/{branchName}/jobs/{jobId}/stop`; reports
    /// `jobSummary`.
    StopJob,
}

/// What a case's reply must carry under the status it answers.
#[derive(Debug, Clone, Copy)]
enum ExpectedBody {
    /// Zero bytes: a No Content answer cannot carry a body at all.
    Nothing,
    /// The empty JSON object a 200 frames an output shape with no members as.
    EmptyObject,
    /// The member the reported document is carried on.
    Document(&'static str),
}

/// One status case: the operation, the status it must answer, and what its body must
/// carry under that status.
struct StatusCase {
    case: &'static str,
    operation: Operation,
    expected_status: u16,
    body: ExpectedBody,
}

/// The status table.
///
/// Every row's `expected_status` and `body` come from the operation's own binding and
/// output shape
/// (`smithy_query(query="digest")` lists the method, the output shape and the members
/// of each): the two operations whose output declares no members, split by whether the
/// request removes or writes, and every `DELETE` of this service that is implemented
/// and reports the record it acted on.
fn status_cases() -> Vec<StatusCase> {
    vec![
        StatusCase {
            case: "a served removal reporting no document",
            operation: Operation::UntagResource,
            expected_status: NO_CONTENT,
            body: ExpectedBody::Nothing,
        },
        StatusCase {
            case: "a served write reporting no document",
            operation: Operation::TagResource,
            expected_status: OK,
            body: ExpectedBody::EmptyObject,
        },
        StatusCase {
            case: "a delete reporting the app it removed",
            operation: Operation::DeleteApp,
            expected_status: OK,
            body: ExpectedBody::Document("app"),
        },
        StatusCase {
            case: "a delete reporting the branch it removed",
            operation: Operation::DeleteBranch,
            expected_status: OK,
            body: ExpectedBody::Document("branch"),
        },
        StatusCase {
            case: "a delete reporting the domain association it removed",
            operation: Operation::DeleteDomainAssociation,
            expected_status: OK,
            body: ExpectedBody::Document("domainAssociation"),
        },
        StatusCase {
            case: "a delete-bound stop reporting the job it stopped",
            operation: Operation::StopJob,
            expected_status: OK,
            body: ExpectedBody::Document("jobSummary"),
        },
    ]
}

/// Seed whatever `operation` acts on into a fresh emulator and drive that one call.
async fn drive(operation: Operation) -> Reply {
    let emulator = Emulator::new();
    match operation {
        Operation::TagResource => {
            let seeded = seed(&emulator, Kind::App, None).await;
            tag_resource(&emulator, &seeded.arn, &json!({ "env": "dev" })).await
        }
        Operation::UntagResource => {
            let seeded = seed(&emulator, Kind::App, Some(&json!({ "env": "dev" }))).await;
            untag_resource(&emulator, &seeded.arn, &["env".to_string()]).await
        }
        Operation::DeleteApp => {
            let app_id = emulator.app_id_of_new_app("status-host").await;
            emulator.delete_app(&app_id).await
        }
        Operation::DeleteBranch => {
            let app_id = emulator.app_id_of_new_app("status-host").await;
            let created = emulator
                .create_branch(&app_id, &json!({ "branchName": "main" }))
                .await;
            assert_eq!(created.status, OK, "CreateBranch must succeed, got {created:?}");
            emulator.delete_branch(&app_id, "main").await
        }
        Operation::DeleteDomainAssociation => {
            let app_id = emulator.app_id_of_new_app("status-host").await;
            let created = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/domains"),
                    Some(&json!({
                        "domainName": "status.example.com",
                        "subDomainSettings": [],
                    })),
                )
                .await;
            assert_eq!(
                created.status, OK,
                "CreateDomainAssociation must succeed, got {created:?}"
            );
            emulator
                .call(
                    "DELETE",
                    &format!("/apps/{app_id}/domains/status.example.com"),
                    None,
                )
                .await
        }
        Operation::StopJob => {
            let app_id = emulator
                .app_id_of_new_repository_app("status-host")
                .await;
            let branch = emulator
                .create_branch(&app_id, &json!({ "branchName": "main" }))
                .await;
            assert_eq!(branch.status, OK, "CreateBranch must succeed, got {branch:?}");
            let started = emulator
                .call(
                    "POST",
                    &format!("/apps/{app_id}/branches/main/jobs"),
                    Some(&json!({ "jobType": "MANUAL" })),
                )
                .await;
            assert_eq!(started.status, OK, "StartJob must succeed, got {started:?}");
            let job_id = string_member(&started.member("jobSummary"), "jobId");
            emulator
                .call(
                    "DELETE",
                    &format!("/apps/{app_id}/branches/main/jobs/{job_id}/stop"),
                    None,
                )
                .await
        }
    }
}

#[tokio::test]
async fn a_served_call_answers_no_content_only_where_it_reports_no_document() {
    for case in status_cases() {
        let reply = drive(case.operation).await;
        assert_eq!(
            reply.status, case.expected_status,
            "case {:?} ({:?}): expected HTTP {}, got {reply:?}",
            case.case, case.operation, case.expected_status
        );
        match case.body {
            ExpectedBody::Nothing => assert!(
                reply.body == Value::Null,
                "case {:?} ({:?}): a No Content answer carries no body at all, got {:?}",
                case.case,
                case.operation,
                reply.body
            ),
            ExpectedBody::EmptyObject => assert_eq!(
                reply.body,
                json!({}),
                "case {:?} ({:?}): an output shape with no members is framed as the empty \
                 object, got {:?}",
                case.case,
                case.operation,
                reply.body
            ),
            ExpectedBody::Document(member) => assert!(
                reply.body.get(member).is_some(),
                "case {:?} ({:?}): expected the reported {member:?} document, got {:?}",
                case.case,
                case.operation,
                reply.body
            ),
        }
    }
}

#[tokio::test]
async fn a_no_content_answer_carries_no_body_bytes_and_states_the_length_it_has() {
    // The framing, not the parsed body: a No Content answer sends zero body bytes and
    // states the length of that nothing. (Over HTTP/1.1 the writer the transport is
    // built on drops the header from a 204 — this is the framing the in-process
    // response carries.)
    for kind in KINDS {
        let emulator = Emulator::new();
        let seeded = seed(&emulator, kind, Some(&json!({ "env": "dev" }))).await;

        let response = emulator
            .call_http(
                "DELETE",
                &format!("/tags/{}?tagKeys=env", encode_label(&seeded.arn)),
                None,
            )
            .await;
        assert_eq!(
            response.status().as_u16(),
            NO_CONTENT,
            "case {kind:?}: expected HTTP {NO_CONTENT}, got {:?}",
            response.status()
        );
        assert!(
            response.body().is_empty(),
            "case {kind:?}: a No Content answer sends no body bytes, got {:?}",
            String::from_utf8_lossy(response.body())
        );
        let framing = response
            .headers()
            .get("content-length")
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string());
        assert_eq!(
            framing.as_deref(),
            Some(NO_CONTENT_LENGTH),
            "case {kind:?}: a No Content answer states the length it has, got {framing:?}"
        );

        // The contrast: a 200 over the same tag map frames the document it reports, so
        // the framing follows the answer rather than being blanked everywhere.
        let listed = emulator
            .call_http("GET", &format!("/tags/{}", encode_label(&seeded.arn)), None)
            .await;
        assert_eq!(
            listed.status().as_u16(),
            OK,
            "case {kind:?}: ListTagsForResource expected HTTP {OK}, got {:?}",
            listed.status()
        );
        assert!(
            !listed.body().is_empty(),
            "case {kind:?}: an answer that reports a document carries it, got no bytes"
        );
    }
}

/// One removal case: what the resource carries, the keys the request names, and the map
/// left behind — the status is 204 for every one of them, however much was removed.
struct RemovalCase {
    case: &'static str,
    carried: Value,
    removed: Vec<String>,
    left: Value,
}

/// Fifty keys with their values: `TagKeyList` is modelled `length 1..50`
/// (`smithy_query(query="operation",operation="UntagResource")`), so fifty keys is the
/// most one request may name.
fn numbered_tags(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("key.{index}"), json!(format!("value-{index}"))))
            .collect(),
    )
}

fn numbered_keys(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("key.{index}")).collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

#[tokio::test]
async fn every_served_removal_answers_no_content_and_the_removal_still_happened() {
    let cases = vec![
        RemovalCase {
            case: "one key of several",
            carried: json!({ "env": "dev", "owner": "platform" }),
            removed: keys(&["env"]),
            left: json!({ "owner": "platform" }),
        },
        RemovalCase {
            case: "every key the resource carries",
            carried: json!({ "env": "dev", "owner": "platform" }),
            removed: keys(&["env", "owner"]),
            left: json!({}),
        },
        RemovalCase {
            case: "a key the resource never carried, so nothing changes",
            carried: json!({ "env": "dev" }),
            removed: keys(&["never-set"]),
            left: json!({ "env": "dev" }),
        },
        RemovalCase {
            case: "the modelled maximum of fifty keys",
            carried: numbered_tags(50),
            removed: numbered_keys(50),
            left: json!({}),
        },
    ];

    for case in cases {
        for kind in KINDS {
            let emulator = Emulator::new();
            let seeded = seed(&emulator, kind, Some(&case.carried)).await;

            let removed = untag_resource(&emulator, &seeded.arn, &case.removed).await;
            assert_eq!(
                removed.status, NO_CONTENT,
                "case {:?} ({kind:?}): expected HTTP {NO_CONTENT}, got {removed:?}",
                case.case
            );
            assert!(
                removed.body == Value::Null,
                "case {:?} ({kind:?}): a No Content answer carries no body at all, got {:?}",
                case.case,
                removed.body
            );

            // The status is not a claim on its own: the state the removal left is read
            // back through the operation that reports it.
            let listed = list_tags(&emulator, &seeded.arn).await;
            assert_eq!(
                listed.member("tags"),
                case.left,
                "case {:?} ({kind:?}): expected the remaining map {}, got {:?}",
                case.case,
                case.left,
                listed.member("tags")
            );
        }
    }
}

#[tokio::test]
async fn a_refused_removal_never_answers_a_success_status() {
    // No Content is a SUCCESS code, so a refusal must not carry it — whether the ARN
    // names nothing (the modelled `ResourceNotFoundException`, HTTP 404) or the request
    // is outside the modelled `tagKeys` bound (refused at the boundary).
    let emulator = Emulator::new();
    let seeded = seed(&emulator, Kind::App, Some(&json!({ "env": "dev" }))).await;

    let mut past_the_bound = numbered_keys(50);
    past_the_bound.push("one.too.many".to_string());

    for (label, arn, removed) in [
        (
            "an ARN naming no resource of this service",
            format!("{}0000", seeded.arn),
            keys(&["env"]),
        ),
        (
            "a tagKeys list past the modelled maximum",
            seeded.arn.clone(),
            past_the_bound,
        ),
        (
            "a request binding no tagKeys at all",
            seeded.arn.clone(),
            Vec::new(),
        ),
    ] {
        let refused = untag_resource(&emulator, &arn, &removed).await;
        assert!(
            refused.status >= 400,
            "case {label:?}: expected a refusal, got {refused:?}"
        );
        assert_ne!(
            refused.status, NO_CONTENT,
            "case {label:?}: a refusal must not answer a success status, got {refused:?}"
        );
    }

    // None of the refusals touched the tag map.
    assert_eq!(
        list_tags(&emulator, &seeded.arn).await.member("tags"),
        json!({ "env": "dev" }),
        "the refused removals must leave the tagged app exactly as it was"
    );
}
