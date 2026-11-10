// SPDX-License-Identifier: CC0-1.0

//! `TagResource` / `ListTags`: the tagging calls address a resource by ARN, and
//! the tags they write and read are the STORED resource's own.
//!
//! The rules under test:
//!
//! * a tag written through `TagResource` lands on the record the resource's own
//!   create wrote, so the tag read reports the create's tags and the written
//!   ones as one set — whichever kind of resource the ARN names;
//! * writing MERGES: a key the resource already carries takes the value just
//!   sent, a new key is added, and a key the request does not name is left
//!   alone;
//! * an ARN that is not an ARN is a bad parameter value, and one that names no
//!   resource of this account is the one not-found refusal — for both calls;
//! * a page is `MaxResults` pairs plus a cursor to the next one.
//!
//! Every case drives real HTTP through the router; the harness is the shared one
//! in `vault_and_backup_job.rs`.

use std::thread::sleep;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_point_deletion::{router_with_engine, start_point};
use crate::vault_and_backup_job::{call, create_vault, error_code, string_at, ACCOUNT, PAST_SETTLE, REGION};

/// The sentence this service states for "the ARN a tagging call carries names
/// no resource of this account" — fixed text.
const NOT_FOUND: &str = "Cannot find resource";

/// `POST /tags/{ResourceArn}`, as the model binds the write.
async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{resource_arn}"),
        Some(&json!({"Tags": tags}).to_string()),
    )
    .await
}

/// `GET /tags/{ResourceArn}{query}`, as the model binds the read.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/tags/{resource_arn}{query}"),
        None,
    )
    .await
}

/// The tags the read reports, as a JSON object.
fn reported_tags(body: &Value) -> Value {
    body.get("Tags").cloned().unwrap_or(Value::Null)
}

/// A tag written through the tagging call joins the tags the resource's own
/// create stored, and every later read of the resource reports one set.
///
/// The rows differ along what the rule generalises over: the KIND of resource
/// the ARN names, and whether the create had already tagged it.
#[tokio::test]
async fn a_written_tag_joins_the_tags_the_resource_was_created_with() {
    let (router, _engine) = router_with_engine();
    // A vault created WITH tags, and one created without.
    let (status, created) = create_vault(
        &router,
        "tagged-vault",
        &json!({"BackupVaultTags": {"team": "backups"}}).to_string(),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let (status, bare) = create_vault(&router, "bare-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: the untagged vault should be created, got {status} {bare:?}"
    );
    let point = start_point(
        &router,
        "tagged-vault",
        &format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
        None,
    )
    .await;
    sleep(PAST_SETTLE);
    let (status, configured) = call(
        &router,
        "PUT",
        "/tiering-configurations",
        Some(
            &json!({
                "TieringConfiguration": {
                    "TieringConfigurationName": "tagged_tiering",
                    "BackupVaultName": "*",
                    "ResourceSelection": [{
                        "Resources": ["*"],
                        "ResourceType": "S3",
                        "TieringDownSettingsInDays": 60,
                    }],
                },
            })
            .to_string(),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateTieringConfiguration should succeed, got {status} {configured:?}"
    );

    let vault_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:tagged-vault");
    let bare_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:bare-vault");
    let tiering_arn =
        format!("arn:aws:backup:{REGION}:{ACCOUNT}:tiering-configuration:tagged_tiering");
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a vault the create had tagged",
            vault_arn.as_str(),
            json!({"team": "backups", "owner": "ops"}),
        ),
        (
            "a vault the create left untagged",
            bare_arn.as_str(),
            json!({"owner": "ops"}),
        ),
        (
            "a recovery point",
            point.as_str(),
            json!({"owner": "ops"}),
        ),
        (
            "a tiering configuration",
            tiering_arn.as_str(),
            json!({"owner": "ops"}),
        ),
    ];

    for (case, resource_arn, expected) in cases {
        let (status, tagged) = tag_resource(&router, resource_arn, json!({"owner": "ops"})).await;
        assert_eq!(
            (status, error_code(&tagged).as_str()),
            (200, "<none>"),
            "case {case:?}: expected TagResource 200, got {status} {tagged:?}"
        );
        let (status, read) = list_tags(&router, resource_arn, "").await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (200, "<none>"),
            "case {case:?}: expected ListTags 200, got {status} {read:?}"
        );
        assert_eq!(
            reported_tags(&read),
            expected,
            "case {case:?}: the read must report the create's tags and the written ones, got \
             {read:?}"
        );
    }

    // Writing MERGES: a key already carried takes the new value, a new key is
    // added, and a key the request does not name survives.
    let (status, merged) = tag_resource(
        &router,
        &vault_arn,
        json!({"owner": "platform", "stage": "prod"}),
    )
    .await;
    assert_eq!(status, 200, "the merge should succeed, got {merged:?}");
    let (_, read) = list_tags(&router, &vault_arn, "").await;
    assert_eq!(
        reported_tags(&read),
        json!({"team": "backups", "owner": "platform", "stage": "prod"}),
        "a write must merge into the stored set, got {read:?}"
    );
    // And it reached only the resource it named.
    let (_, other) = list_tags(&router, &bare_arn, "").await;
    assert_eq!(
        reported_tags(&other),
        json!({"owner": "ops"}),
        "a write must not reach another resource's tags, got {other:?}"
    );
}

/// An ARN that names no resource of this account is ONE condition for both
/// calls, and a value that is not an ARN is a bad parameter value.
#[tokio::test]
async fn an_arn_that_names_no_resource_is_one_refusal_for_both_calls() {
    let (router, _engine) = router_with_engine();
    let (status, created) = create_vault(&router, "tagged-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );

    let absent: Vec<(&str, String)> = vec![
        (
            "a vault that was never created",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:never-created"),
        ),
        (
            "a recovery point nothing answers to",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:does-not-exist"),
        ),
        (
            "a vault of another account",
            format!("arn:aws:backup:{REGION}:111122223333:backup-vault:tagged-vault"),
        ),
        (
            // An ARN of another service, with no `/` in its resource field:
            // a path label carrying one is a routing question rather than a
            // tagging one, which this suite is not about.
            "a resource of another service",
            format!("arn:aws:sns:{REGION}:{ACCOUNT}:backup-notifications"),
        ),
    ];
    for (case, resource_arn) in absent {
        for (call_name, answer) in [
            (
                "TagResource",
                tag_resource(&router, &resource_arn, json!({"owner": "ops"})).await,
            ),
            ("ListTags", list_tags(&router, &resource_arn, "").await),
        ] {
            let (status, refused) = answer;
            assert_eq!(
                (
                    status,
                    error_code(&refused).as_str(),
                    string_at(&refused, "Message").as_str()
                ),
                (400, "ResourceNotFoundException", NOT_FOUND),
                "case {case:?} on {call_name}: expected the not-found refusal, got {status} \
                 {refused:?}"
            );
        }
    }

    let malformed = ["not-an-arn", "arn:aws:backup"];
    for value in malformed {
        for (call_name, answer) in [
            (
                "TagResource",
                tag_resource(&router, value, json!({"owner": "ops"})).await,
            ),
            ("ListTags", list_tags(&router, value, "").await),
        ] {
            let (status, refused) = answer;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "InvalidParameterValueException"),
                "case {value:?} on {call_name}: expected the parameter refusal, got {status} \
                 {refused:?}"
            );
        }
    }
}

/// A page is `MaxResults` pairs plus a cursor to the next one, and the pages
/// together are the resource's whole tag set exactly once.
#[tokio::test]
async fn the_tag_read_pages_over_the_stored_set() {
    let (router, _engine) = router_with_engine();
    let (status, created) = create_vault(&router, "tagged-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let vault_arn = format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:tagged-vault");
    let (status, tagged) = tag_resource(
        &router,
        &vault_arn,
        json!({"a": "1", "b": "2", "c": "3"}),
    )
    .await;
    assert_eq!(status, 200, "setup: the write should succeed, got {tagged:?}");

    let mut walked: Vec<(String, String)> = Vec::new();
    let mut query = "?maxResults=2".to_string();
    for page in 1..=2 {
        let (status, read) = list_tags(&router, &vault_arn, &query).await;
        assert_eq!(status, 200, "page {page}: expected 200, got {read:?}");
        let pairs = reported_tags(&read);
        let object = pairs
            .as_object()
            .unwrap_or_else(|| panic!("page {page}: the read reports a map, got {pairs:?}"));
        walked.extend(
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string())),
        );
        let token = string_at(&read, "NextToken");
        if page == 1 {
            assert_eq!(
                object.len(),
                2,
                "page {page}: a page of two reports two pairs, got {read:?}"
            );
            assert!(
                !token.is_empty(),
                "page {page}: a page with more behind it carries a cursor, got {read:?}"
            );
            query = format!("?maxResults=2&nextToken={token}");
        } else {
            assert_eq!(
                object.len(),
                1,
                "page {page}: the last page reports what is left, got {read:?}"
            );
            assert!(
                token.is_empty(),
                "page {page}: the last page carries no cursor, got {read:?}"
            );
        }
    }
    walked.sort();
    assert_eq!(
        walked,
        vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
            ("c".to_string(), "3".to_string()),
        ],
        "the pages together must report the whole tag set once"
    );
}
