// SPDX-License-Identifier: CC0-1.0

//! `CreateTieringConfiguration` and the calls over what it stores
//! (`GetTieringConfiguration`, `ListTieringConfigurations`,
//! `DeleteTieringConfiguration`).
//!
//! The rules under test:
//!
//! * a tiering configuration is addressed by NAME and the name identifies one
//!   resource of the account, so however many creates of one name arrive
//!   together exactly ONE stores a configuration and every other is refused as
//!   already existing — and the survivor is what the reads report;
//! * a name the account holds no configuration under is the one not-found
//!   refusal, answered by the read and by the delete alike, so a
//!   delete-then-read answers as a read of a name nothing ever created does;
//! * a create's selections are stored and reported back by both reads, so the
//!   configuration a create stored and the one a read reports agree;
//! * a repeated `CreatorRequestId` takes no second action: it answers with the
//!   configuration that token created.
//!
//! Every case drives real HTTP through the router; the harness is the shared one
//! in `vault_and_backup_job.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{call, create_vault, error_code, router, string_at, ACCOUNT, REGION};

/// The sentence this service states for "the tiering configuration asked about
/// is not one this account holds" — fixed text, naming no part of the request.
const NOT_FOUND: &str = "Cannot find tiering configuration";

/// The least `TieringDownSettingsInDays` the model's `@range` admits, and the
/// most: "must be a positive integer between 60 and 36500 days" (model doc for
/// the member, whose `@range` is `min: 60, max: 36500`).
const MIN_TIERING_DAYS: i64 = 60;
const MAX_TIERING_DAYS: i64 = 36_500;

/// `PUT /tiering-configurations`, as the model binds the create.
async fn create_tiering(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        "/tiering-configurations",
        Some(&body.to_string()),
    )
    .await
}

/// `GET /tiering-configurations/{TieringConfigurationName}`.
async fn get_tiering(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/tiering-configurations/{name}"),
        None,
    )
    .await
}

/// `DELETE /tiering-configurations/{TieringConfigurationName}`.
async fn delete_tiering(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/tiering-configurations/{name}"),
        None,
    )
    .await
}

/// `GET /tiering-configurations{query}`.
async fn list_tiering(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/tiering-configurations{query}"),
        None,
    )
    .await
}

/// A create body naming `name`, applying to `vault`, with `selections`.
fn create_body(name: &str, vault: &str, selections: Value) -> Value {
    json!({
        "TieringConfiguration": {
            "TieringConfigurationName": name,
            "BackupVaultName": vault,
            "ResourceSelection": selections,
        },
    })
}

/// One resource selection: "currently limited to `S3`" is what the model
/// documents for a tiering configuration's `ResourceType`.
fn selection(resources: Value, days: i64) -> Value {
    json!({
        "Resources": resources,
        "ResourceType": "S3",
        "TieringDownSettingsInDays": days,
    })
}

/// The names the listing reports, in the order it reports them.
fn listed_names(page: &Value) -> Vec<String> {
    page.get("TieringConfigurations")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| string_at(entry, "TieringConfigurationName"))
                .collect()
        })
        .unwrap_or_default()
}

/// A created configuration round-trips: the read reports exactly what was
/// stored, the listing reports it, and the delete makes every later read of the
/// name answer the one not-found refusal.
///
/// The rows differ along what the rule generalises over: the vault the
/// configuration applies to (a named one and the documented `*` wildcard), the
/// number of selections, the resources each names, and the transition day count
/// — including both published ends of its range.
#[tokio::test]
async fn a_created_tiering_configuration_round_trips_and_a_deleted_one_is_gone() {
    let bucket = "arn:aws:s3:::demo-bucket".to_string();
    let other_bucket = "arn:aws:s3:::other-bucket".to_string();
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "every vault, every resource, at the floor",
            "tier_all",
            "*",
            json!([selection(json!(["*"]), MIN_TIERING_DAYS)]),
        ),
        (
            "one vault, one bucket, at the ceiling",
            "tier_one_vault",
            "tiering-vault",
            json!([selection(json!([bucket.clone()]), MAX_TIERING_DAYS)]),
        ),
        (
            "two selections with different day counts",
            "tier_two_selections",
            "tiering-vault",
            json!([
                selection(json!([bucket.clone()]), 90),
                selection(json!([other_bucket.clone()]), 365),
            ]),
        ),
        (
            "one selection naming two buckets",
            "tier_two_resources",
            "tiering-vault",
            json!([selection(json!([bucket, other_bucket]), 180)]),
        ),
    ];

    for (case, name, vault, selections) in cases {
        let router = router();
        let (status, created) = create_vault(&router, "tiering-vault", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );

        let body = create_body(name, vault, selections.clone());
        let (status, answered) = create_tiering(&router, &body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (200, "<none>"),
            "case {case:?}: expected CreateTieringConfiguration 200, got {status} {answered:?}"
        );
        let expected_arn =
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:tiering-configuration:{name}");
        assert_eq!(
            (
                string_at(&answered, "TieringConfigurationName").as_str(),
                string_at(&answered, "TieringConfigurationArn").as_str()
            ),
            (name, expected_arn.as_str()),
            "case {case:?}: the create must answer with the configuration it stored, got \
             {answered:?}"
        );
        assert!(
            answered.get("CreationTime").is_some(),
            "case {case:?}: the create reports when it stored the configuration, got {answered:?}"
        );

        let (status, read) = get_tiering(&router, name).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (200, "<none>"),
            "case {case:?}: expected GetTieringConfiguration 200, got {status} {read:?}"
        );
        let configuration = read
            .get("TieringConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            (
                string_at(&configuration, "TieringConfigurationName").as_str(),
                string_at(&configuration, "TieringConfigurationArn").as_str(),
                string_at(&configuration, "BackupVaultName").as_str(),
                configuration
                    .get("ResourceSelection")
                    .cloned()
                    .unwrap_or(Value::Null)
            ),
            (name, expected_arn.as_str(), vault, selections),
            "case {case:?}: the read must report what the create stored, got {read:?}"
        );

        let (status, listed) = list_tiering(&router, "").await;
        assert_eq!(status, 200, "case {case:?}: expected the listing to answer 200");
        assert_eq!(
            listed_names(&listed),
            vec![name.to_string()],
            "case {case:?}: the listing must report the stored configuration, got {listed:?}"
        );

        let (status, deleted) = delete_tiering(&router, name).await;
        assert_eq!(
            (status, error_code(&deleted).as_str()),
            (200, "<none>"),
            "case {case:?}: expected DeleteTieringConfiguration 200, got {status} {deleted:?}"
        );
        for (read_name, answer) in [
            ("GetTieringConfiguration", get_tiering(&router, name).await),
            (
                "DeleteTieringConfiguration",
                delete_tiering(&router, name).await,
            ),
        ] {
            let (status, refused) = answer;
            assert_eq!(
                (
                    status,
                    error_code(&refused).as_str(),
                    string_at(&refused, "Message").as_str()
                ),
                (400, "ResourceNotFoundException", NOT_FOUND),
                "case {case:?} on {read_name}: a deleted configuration is gone, got {status} \
                 {refused:?}"
            );
        }
        let (_, listed) = list_tiering(&router, "").await;
        assert!(
            listed_names(&listed).is_empty(),
            "case {case:?}: the deleted configuration must leave the listing, got {listed:?}"
        );
    }
}

/// However many creates of ONE name arrive together, exactly one stores a
/// configuration: every other is refused as already existing, and the reads
/// report the single survivor.
///
/// The rows differ in how many creates race for the name, so the rule is
/// asserted over the class rather than over one burst size.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_of_one_name_have_exactly_one_winner() {
    for racers in [2_usize, 6, 11] {
        let router = Arc::new(router());
        let (status, created) = create_vault(&router, "tiering-vault", "{}").await;
        assert_eq!(
            status, 200,
            "case {racers} racers: setup CreateBackupVault should succeed, got {status} {created:?}"
        );

        let body = create_body(
            "tier_contended",
            "tiering-vault",
            json!([selection(json!(["*"]), MIN_TIERING_DAYS)]),
        );
        // Every create is in flight at once, on four worker threads, so each
        // one reaches the store without knowing about the others.
        let mut pending = Vec::new();
        for _ in 0..racers {
            let router = Arc::clone(&router);
            let body = body.clone();
            pending.push(tokio::spawn(async move {
                create_tiering(&router, &body).await
            }));
        }
        let mut answers: Vec<(u16, Value)> = Vec::with_capacity(racers);
        for task in pending {
            answers.push(task.await.expect("a racing create to finish"));
        }

        let winners: Vec<&(u16, Value)> =
            answers.iter().filter(|(status, _)| *status == 200).collect();
        assert_eq!(
            winners.len(),
            1,
            "case {racers} racers: expected exactly one winning create, got {answers:?}"
        );
        for (status, answer) in answers.iter().filter(|(status, _)| *status != 200) {
            assert_eq!(
                (
                    *status,
                    error_code(answer).as_str(),
                    string_at(answer, "Message").as_str()
                ),
                (
                    400,
                    "AlreadyExistsException",
                    "Tiering configuration with the same name already exists"
                ),
                "case {racers} racers: expected every loser refused as already existing, got \
                 {status} {answer:?}"
            );
        }

        // The reads report the one survivor, whose ARN is the winner's.
        let (status, read) = get_tiering(&router, "tier_contended").await;
        assert_eq!(
            status, 200,
            "case {racers} racers: the survivor must be readable, got {status} {read:?}"
        );
        let configuration = read
            .get("TieringConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            string_at(&configuration, "TieringConfigurationArn"),
            string_at(&winners[0].1, "TieringConfigurationArn"),
            "case {racers} racers: the read must report the winning create's configuration"
        );
        let (_, listed) = list_tiering(&router, "").await;
        assert_eq!(
            listed_names(&listed),
            vec!["tier_contended".to_string()],
            "case {racers} racers: exactly one configuration may be stored, got {listed:?}"
        );
    }
}

/// A repeated `CreatorRequestId` takes no second action: it answers with the
/// configuration that token created, and the store still holds one.
#[tokio::test]
async fn a_repeated_creator_request_id_replays_the_first_create() {
    let router = router();
    let (status, created) = create_vault(&router, "tiering-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );

    let mut body = create_body(
        "tier_replayed",
        "tiering-vault",
        json!([selection(json!(["*"]), MIN_TIERING_DAYS)]),
    );
    body["CreatorRequestId"] = json!("one-configuration-only");

    let (status, first) = create_tiering(&router, &body).await;
    assert_eq!(status, 200, "the first create should succeed, got {first:?}");
    let (status, replayed) = create_tiering(&router, &body).await;
    assert_eq!(
        (status, error_code(&replayed).as_str()),
        (200, "<none>"),
        "a replayed CreatorRequestId must answer as the first call did, got {status} {replayed:?}"
    );
    assert_eq!(
        (
            string_at(&first, "TieringConfigurationArn"),
            string_at(&first, "CreationTime")
        ),
        (
            string_at(&replayed, "TieringConfigurationArn"),
            string_at(&replayed, "CreationTime")
        ),
        "a replay must report the identifiers the first call reported"
    );
    let (_, listed) = list_tiering(&router, "").await;
    assert_eq!(
        listed_names(&listed),
        vec!["tier_replayed".to_string()],
        "a replay must not store a second configuration, got {listed:?}"
    );

    // A create of ANOTHER name with no token is a second configuration, which
    // is what makes the replay above about the token.
    let other = create_body(
        "tier_other",
        "tiering-vault",
        json!([selection(json!(["*"]), MIN_TIERING_DAYS)]),
    );
    let (status, second) = create_tiering(&router, &other).await;
    assert_eq!(status, 200, "a second name should be stored, got {second:?}");
    let (_, listed) = list_tiering(&router, "").await;
    assert_eq!(
        listed_names(&listed),
        vec!["tier_other".to_string(), "tier_replayed".to_string()],
        "the listing reports both configurations in name order, got {listed:?}"
    );
}

/// A page is `MaxResults` items plus a cursor to the next one, and the pages
/// together are every stored configuration exactly once.
#[tokio::test]
async fn the_listing_pages_over_the_stored_configurations() {
    let router = router();
    let (status, created) = create_vault(&router, "tiering-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let names = ["tier_a", "tier_b", "tier_c"];
    for name in names {
        let (status, answered) = create_tiering(
            &router,
            &create_body(
                name,
                "tiering-vault",
                json!([selection(json!(["*"]), MIN_TIERING_DAYS)]),
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "setup {name}: the create should succeed, got {status} {answered:?}"
        );
    }

    let mut walked: Vec<String> = Vec::new();
    let mut query = "?maxResults=1".to_string();
    for page in 1..=names.len() {
        let (status, listed) = list_tiering(&router, &query).await;
        assert_eq!(status, 200, "page {page}: expected 200, got {listed:?}");
        let reported = listed_names(&listed);
        assert_eq!(
            reported.len(),
            1,
            "page {page}: a page of one must report one configuration, got {listed:?}"
        );
        walked.extend(reported);
        let token = string_at(&listed, "NextToken");
        if page == names.len() {
            assert!(
                token.is_empty(),
                "page {page}: the last page carries no cursor, got {listed:?}"
            );
        } else {
            assert!(
                !token.is_empty(),
                "page {page}: a page with more behind it carries a cursor, got {listed:?}"
            );
            query = format!("?maxResults=1&nextToken={token}");
        }
    }
    assert_eq!(
        walked,
        names.map(str::to_string).to_vec(),
        "the pages together must report every configuration once, in name order"
    );

    // A cursor this service did not vend names no page of the result set.
    let (status, refused) = list_tiering(&router, "?nextToken=deadbeef").await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidParameterValueException"),
        "an unknown cursor is a bad parameter value, got {status} {refused:?}"
    );
}
