// SPDX-License-Identifier: CC0-1.0

//! What this registry PUBLISHES when an image arrives in or leaves a repository.
//!
//! RULE: every image action a call really performs is published onto the account's default
//! event bus as an `ECR Image Action` event of the source `aws.ecr`, whose `detail` carries
//! the outcome, the repository, the image's digest, the action that was performed and —
//! when the action involved one — the image's tag. Those five members are what a rule's
//! `EventPattern` filters on, so they are the contract this suite pins; which rules the
//! event then reaches, and what their targets do with it, is EventBridge's business and is
//! exercised over the assembled router (`core/router/tests/all/`).
//!
//! The rows differ along what the rule generalises over: the action performed (a push, a
//! delete), whether the action involved a tag, and the repository and digest it names.
//!
//! The second test is the other half of the rule: publication is BEST EFFORT, so a
//! registry mounted with no event bus to publish onto still performs and reports the push
//! and the delete themselves — the event is a consequence of the action, never a condition
//! of it.

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::image_events::{
    ImageAction, ACTION_TYPE_DELETE, ACTION_TYPE_PUSH, DEFAULT_EVENT_BUS,
    IMAGE_ACTION_DETAIL_TYPE, EVENT_SOURCE, RESULT_SUCCESS,
};

use crate::wire::{at, emulator, ok, refused};

/// The identity of the event this registry publishes, as AWS names it.
#[test]
fn an_image_action_is_published_as_awss_own_event_identity() {
    assert_eq!(
        EVENT_SOURCE, "aws.ecr",
        "the source of an Amazon ECR event is aws.ecr, got {EVENT_SOURCE}"
    );
    assert_eq!(
        IMAGE_ACTION_DETAIL_TYPE, "ECR Image Action",
        "the detail type of an image action is 'ECR Image Action', got \
         {IMAGE_ACTION_DETAIL_TYPE}"
    );
    assert_eq!(
        DEFAULT_EVENT_BUS, "default",
        "an event a service generates goes to the account's default bus, got \
         {DEFAULT_EVENT_BUS}"
    );
}

/// Every row: the `detail` the published event carries for one action, member for member.
#[test]
fn the_published_detail_states_the_action_the_registry_performed() {
    let at_instant = ::std::time::SystemTime::now();
    // (case, the action performed, the detail the event must carry)
    let rows: Vec<(&str, ImageAction, Value)> = vec![
        (
            "a tagged push",
            ImageAction {
                repository_name: "orders".to_string(),
                action_type: ACTION_TYPE_PUSH,
                image_digest: "sha256:aaaa".to_string(),
                image_tag: Some("release".to_string()),
                at: at_instant,
            },
            json!({
                "result": RESULT_SUCCESS,
                "repository-name": "orders",
                "image-digest": "sha256:aaaa",
                "action-type": "PUSH",
                "image-tag": "release",
            }),
        ),
        (
            // An untagged push involves no tag, so the event states none rather than an
            // empty one.
            "an untagged push",
            ImageAction {
                repository_name: "project-a/nginx".to_string(),
                action_type: ACTION_TYPE_PUSH,
                image_digest: "sha256:bbbb".to_string(),
                image_tag: None,
                at: at_instant,
            },
            json!({
                "result": RESULT_SUCCESS,
                "repository-name": "project-a/nginx",
                "image-digest": "sha256:bbbb",
                "action-type": "PUSH",
            }),
        ),
        (
            "a delete of one tag of an image",
            ImageAction {
                repository_name: "payments".to_string(),
                action_type: ACTION_TYPE_DELETE,
                image_digest: "sha256:cccc".to_string(),
                image_tag: Some("v1".to_string()),
                at: at_instant,
            },
            json!({
                "result": RESULT_SUCCESS,
                "repository-name": "payments",
                "image-digest": "sha256:cccc",
                "action-type": "DELETE",
                "image-tag": "v1",
            }),
        ),
        (
            "a delete of an untagged image by digest",
            ImageAction {
                repository_name: "payments".to_string(),
                action_type: ACTION_TYPE_DELETE,
                image_digest: "sha256:dddd".to_string(),
                image_tag: None,
                at: at_instant,
            },
            json!({
                "result": RESULT_SUCCESS,
                "repository-name": "payments",
                "image-digest": "sha256:dddd",
                "action-type": "DELETE",
            }),
        ),
    ];

    for (case, action, expected) in rows {
        let detail = action.detail();
        assert_eq!(
            detail, expected,
            "case {case:?}: expected the detail {expected}, got {detail}"
        );
    }
}

/// A push and a delete are performed and reported whatever becomes of the event they
/// publish: this registry is mounted alone, so there is no event bus to publish onto, and
/// both calls still answer from the records they wrote.
#[tokio::test]
async fn a_push_and_a_delete_are_served_with_no_event_bus_to_publish_onto() {
    let router = emulator();
    let case = "a registry with nowhere to publish";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": "events"}),
    )
    .await;

    let manifest = json!({"schemaVersion": 2, "marker": "events"}).to_string();
    let pushed = ok(
        &router,
        case,
        "PutImage",
        &json!({
            "repositoryName": "events",
            "imageManifest": manifest,
            "imageTag": "v1",
        }),
    )
    .await;
    let digest = at(&pushed, "/image/imageId/imageDigest")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        digest.starts_with("sha256:"),
        "case {case:?}: the push must report the digest it stored the manifest under, got \
         {pushed}"
    );

    // The image the push stored is the one the registry reports.
    let described = ok(
        &router,
        case,
        "DescribeImages",
        &json!({"repositoryName": "events", "imageIds": [{"imageTag": "v1"}]}),
    )
    .await;
    assert_eq!(
        at(&described, "/imageDetails/0/imageDigest").as_str(),
        Some(digest.as_str()),
        "case {case:?}: the stored image must read back at its tag, got {described}"
    );

    // And the delete removes it, reporting the identifier it removed.
    let deleted = ok(
        &router,
        case,
        "BatchDeleteImage",
        &json!({"repositoryName": "events", "imageIds": [{"imageDigest": digest}]}),
    )
    .await;
    assert_eq!(
        at(&deleted, "/imageIds/0/imageDigest").as_str(),
        Some(digest.as_str()),
        "case {case:?}: the delete must report the image it removed, got {deleted}"
    );
    assert_eq!(
        at(&deleted, "/failures").as_array().map(Vec::len),
        Some(0),
        "case {case:?}: nothing must have failed, got {deleted}"
    );

    // The image is gone, which the registry's own read reports.
    refused(
        &router,
        case,
        "DescribeImages",
        &json!({"repositoryName": "events", "imageIds": [{"imageDigest": digest}]}),
        "ImageNotFoundException",
    )
    .await;
}
