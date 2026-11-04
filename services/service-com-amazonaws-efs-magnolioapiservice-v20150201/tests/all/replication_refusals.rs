// SPDX-License-Identifier: CC0-1.0

//! How the EFS replication surface REFUSES a request that names no replication
//! configuration.
//!
//! Two rules, each asserted over a table that spans what it generalises over rather than
//! over the one request that exposed it:
//!
//! 1. `DescribeReplicationConfigurations` reports replication configurations, not file
//!    systems: a `FileSystemId` filter that matches none of them is answered
//!    `ReplicationNotFound` (404) with the FIXED sentence
//!    [`state::NO_REPLICATIONS_FOUND_MESSAGE`], whether the id names a file system that
//!    holds no configuration or no file system at all. Nothing on that read raises
//!    `FileSystemNotFound`, and the sentence interpolates nothing from the request.
//!
//! 2. The two operations that WRITE a replication configuration
//!    (`CreateReplicationConfiguration`, `DeleteReplicationConfiguration`) answer a
//!    not-found refusal with a document carrying the SENTENCE ALONE — the error shape
//!    travels in `x-amzn-errortype`, and the body has no `ErrorCode` member — while the
//!    READ above carries `ErrorCode` as well. The delete's `ReplicationNotFound` names
//!    the file system it could not find a configuration for; both operations'
//!    `FileSystemNotFound` keeps this service's one not-found construction.
//!
//! The id forms each table varies over come from the modelled `FileSystemId`
//! (`smithy_query(query="shape",shape="FileSystemId")`): `fs-` plus 8 to 40 hex digits,
//! or the file-system ARN. The ARN form is exercised on the read, whose `FileSystemId` is
//! an `httpQuery`; the two write operations bind it to an `httpLabel`, where an ARN's own
//! slashes are not a single path segment, so those tables use the bare id forms.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use super::harness::{settle, Emulator};

/// Ids that are well formed for the modelled `FileSystemId` pattern
/// (`^fs-[0-9a-f]{8,40}$`) and name no file system this account holds: both published
/// length bounds and one in between.
fn unknown_file_system_ids() -> Vec<String> {
    vec![
        "fs-00000000".to_string(),
        "fs-0123456789abcdef".to_string(),
        format!("fs-{}", "a".repeat(40)),
    ]
}

/// The ARN form of `wanted_id`, built from the ARN the emulator reports for the file
/// system `real_id` — so the partition, Region and account are the emulator's own and
/// no ARN is written out here.
async fn arn_of(efs: &Emulator, real_id: &str, wanted_id: &str) -> String {
    let reply = efs
        .describe_file_systems(&format!("FileSystemId={real_id}"))
        .await;
    assert_eq!(
        reply.status, 200,
        "DescribeFileSystems({real_id}): expected 200, got {} {:?}",
        reply.status, reply.body
    );
    let arn = reply.get("FileSystems")[0]["FileSystemArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let prefix = arn.strip_suffix(real_id).unwrap_or_else(|| {
        panic!("a file system's ARN ends with its id, got {arn:?} for {real_id}")
    });
    format!("{prefix}{wanted_id}")
}

/// The `ErrorCode` slot of an error document: the JSON value under the key, or `None`
/// when the document carries no such key at all — which is the difference rule 2 is
/// about, and which a `body["ErrorCode"].as_str()` reader cannot see.
fn error_code_slot(reply: &super::harness::Reply) -> Option<Value> {
    reply.body.as_object()?.get("ErrorCode").cloned()
}

/// Assert one refusal of a replication WRITE operation: its status, the shape named in
/// `x-amzn-errortype`, its sentence, and that its body carries no `ErrorCode` member.
fn assert_sentence_alone(
    case: &str,
    reply: &super::harness::Reply,
    shape: &str,
    sentence: &str,
) {
    assert_eq!(
        (reply.status, reply.error_type.as_deref()),
        (404, Some(shape)),
        "case {case:?}: wrong refusal, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.string("Message"),
        sentence,
        "case {case:?}: unexpected sentence, got {:?}",
        reply.get("Message")
    );
    assert_eq!(
        error_code_slot(reply),
        None,
        "case {case:?}: this body carries the sentence alone, so it must have no \
         ErrorCode member, got {:?}",
        reply.body
    );
}

// ---------------------------------------------------------------------------
// Rule 1: the read refuses every unmatched filter as ReplicationNotFound.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_replication_configurations_refuses_every_unmatched_filter_alike() {
    let efs = Emulator::new();

    // One configuration really exists, so the refusals below are not simply what an
    // empty store answers.
    let replicated = efs.available_file_system("refusal-replicated").await;
    let created = efs
        .create_replication_configuration(&replicated, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateReplicationConfiguration: expected 200, got {} {:?}",
        created.status, created.body
    );
    let destination = created.body["Destinations"][0]["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A file system that holds no configuration, one that is not `available` yet, and
    // one whose configuration was deleted again.
    let without = efs.available_file_system("refusal-without").await;
    let still_creating = efs
        .create_file_system(json!({ "CreationToken": "refusal-creating" }))
        .await
        .string("FileSystemId");
    let deleted_again = efs.available_file_system("refusal-deleted").await;
    let short_lived = efs
        .create_replication_configuration(&deleted_again, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        short_lived.status, 200,
        "CreateReplicationConfiguration({deleted_again}): expected 200, got {} {:?}",
        short_lived.status, short_lived.body
    );
    let removed = efs.delete_replication_configuration(&deleted_again, "").await;
    assert_eq!(
        removed.status, 204,
        "DeleteReplicationConfiguration({deleted_again}): expected 204, got {} {:?}",
        removed.status, removed.body
    );

    let mut cases: Vec<(String, String)> = vec![
        (
            "a file system that holds no configuration".to_string(),
            without.clone(),
        ),
        (
            "a file system that is still creating".to_string(),
            still_creating.clone(),
        ),
        (
            "a file system whose configuration was deleted".to_string(),
            deleted_again.clone(),
        ),
        (
            "the ARN of a file system that holds no configuration".to_string(),
            arn_of(&efs, &without, &without).await,
        ),
    ];
    for unknown in unknown_file_system_ids() {
        cases.push((format!("the unknown id {unknown}"), unknown.clone()));
        cases.push((
            format!("the ARN of the unknown id {unknown}"),
            arn_of(&efs, &without, &unknown).await,
        ));
    }

    for (case, requested) in &cases {
        let reply = efs
            .describe_replication_configurations(&format!("FileSystemId={requested}"))
            .await;
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (404, Some("ReplicationNotFound")),
            "case {case:?}: every unmatched filter is ReplicationNotFound, got {} {:?}",
            reply.status,
            reply.body
        );
        // This read's document carries `ErrorCode` as well as the sentence.
        assert_eq!(
            reply.string("ErrorCode"),
            "ReplicationNotFound",
            "case {case:?}: the read's ErrorCode member must name the error, got {:?}",
            reply.get("ErrorCode")
        );
        assert_eq!(
            reply.string("Message"),
            state::NO_REPLICATIONS_FOUND_MESSAGE,
            "case {case:?}: the sentence is fixed text, got {:?}",
            reply.get("Message")
        );
        // Fixed means value-free: nothing of the request reaches the sentence.
        assert!(
            !reply.string("Message").contains(requested.as_str()),
            "case {case:?}: the sentence must interpolate nothing from the request, got {:?}",
            reply.get("Message")
        );
    }

    // The positive control: the filter that DOES match is served, from either side of
    // the configuration and through the ARN form too, so the refusals above are a
    // property of the filter and not of the operation.
    for served in [
        replicated.clone(),
        destination.clone(),
        arn_of(&efs, &replicated, &replicated).await,
    ] {
        let reply = efs
            .describe_replication_configurations(&format!("FileSystemId={served}"))
            .await;
        assert_eq!(
            reply.status, 200,
            "a filter that matches {served}: expected 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.get("Replications")[0]["SourceFileSystemId"].as_str(),
            Some(replicated.as_str()),
            "a filter that matches {served}: must report the stored configuration, got {:?}",
            reply.get("Replications")
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the write operations refuse with the sentence alone.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_replication_write_operations_refuse_with_the_sentence_alone() {
    let efs = Emulator::new();
    let source = efs.available_file_system("sentence-alone-source").await;
    let without = efs.available_file_system("sentence-alone-without").await;
    let other = efs.available_file_system("sentence-alone-other").await;

    // CreateReplicationConfiguration: a source that does not exist, and an existing
    // source whose named destination does not exist — the two ids this operation
    // resolves.
    for unknown in unknown_file_system_ids() {
        let reply = efs
            .create_replication_configuration(&unknown, json!({ "Destinations": [{}] }))
            .await;
        assert_sentence_alone(
            &format!("CreateReplicationConfiguration with the unknown source {unknown}"),
            &reply,
            "FileSystemNotFound",
            &state::file_system_not_found_message(&unknown),
        );

        let reply = efs
            .create_replication_configuration(
                &source,
                json!({ "Destinations": [{ "FileSystemId": unknown }] }),
            )
            .await;
        assert_sentence_alone(
            &format!("CreateReplicationConfiguration with the unknown destination {unknown}"),
            &reply,
            "FileSystemNotFound",
            &state::file_system_not_found_message(&unknown),
        );
    }

    // DeleteReplicationConfiguration: a file system that exists but holds no
    // configuration names itself in the sentence, so the sentence varies with the
    // request where AWS's does.
    for (case, file_system_id) in [
        ("a file system with no configuration", without.clone()),
        ("another file system with no configuration", other.clone()),
        ("a file system that could be a source", source.clone()),
    ] {
        let reply = efs
            .delete_replication_configuration(&file_system_id, "")
            .await;
        assert_sentence_alone(
            &format!("DeleteReplicationConfiguration on {case}"),
            &reply,
            "ReplicationNotFound",
            &state::replication_configuration_not_found_message(&file_system_id),
        );
        assert!(
            reply.string("Message").contains(file_system_id.as_str()),
            "case {case:?}: the delete's sentence names the file system it was asked \
             about, got {:?}",
            reply.get("Message")
        );
    }

    // DeleteReplicationConfiguration on an id that names no file system at all.
    for unknown in unknown_file_system_ids() {
        let reply = efs.delete_replication_configuration(&unknown, "").await;
        assert_sentence_alone(
            &format!("DeleteReplicationConfiguration on the unknown id {unknown}"),
            &reply,
            "FileSystemNotFound",
            &state::file_system_not_found_message(&unknown),
        );
    }

    // And the delete of a configuration that IS there still succeeds, so the refusals
    // above are a property of the missing configuration and not of the operation.
    let created = efs
        .create_replication_configuration(&source, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateReplicationConfiguration: expected 200, got {} {:?}",
        created.status, created.body
    );
    settle().await;
    let deleted = efs.delete_replication_configuration(&source, "").await;
    assert_eq!(
        deleted.status, 204,
        "DeleteReplicationConfiguration of a stored configuration: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
}
