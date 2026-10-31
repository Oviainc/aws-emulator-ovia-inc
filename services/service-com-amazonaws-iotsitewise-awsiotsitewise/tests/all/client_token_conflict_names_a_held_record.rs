// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` already spent on a DIFFERENT request means once the record
//! that token's own call produced is GONE, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these rows pin: the modelled conflict reports "the resource that
//! conflicts with this operation" through its required `resourceId`/`resourceArn`
//! (`smithy_query(query="shape", shape="ConflictingOperationException")`), so a
//! request reusing a spent token is refused only while this store still HOLDS the
//! record that token's call produced. The records a workspace's delete forgets — a
//! search, an asynchronous query, an enrichment job, a dataset export job: what
//! running the workspace produced, which no call of its own removes — can be gone
//! without the caller ever naming them, and a request whose token was spent on one
//! of those then conflicts with nothing: it is served as the fresh call it now is,
//! exactly as a RETRY of that token already is.
//!
//! Each row therefore drives one record through three states of the same spent
//! token:
//!
//! 1. while the record is held, the differing request is the modelled 409 naming
//!    that record (the conflict is not weakened by this change);
//! 2. with the workspace deleted and opened again — so the record is gone while the
//!    request's own workspace is not — the same differing request is SERVED, mints
//!    a record of its own and reads back through the operation's own describe;
//! 3. that served call is itself what the token now names, so repeating it replays
//!    the record it minted rather than starting a second.
//!
//! The rows span what the rule generalises over: the kind of record a workspace's
//! delete forgets (search, query, enrichment job, dataset export job), the
//! operation that produces it (`StartSearch`, `StartQuery`, `CreateEnrichmentJob`,
//! `CreateDatasetExportJob`), the status it answers with (200 and 202), the member
//! the differing request differs in (a natural-language statement, a SQL statement,
//! the dataset a job runs over, an export destination) and the id member it is
//! named by (`searchId`, `queryId`, `jobId`).
//!
//! The HTTP harness (`call`, `error_code`, `router`) is `asset_lifecycle`'s and the
//! dataset create is `enrichment_jobs`'; both are reused rather than copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::asset_lifecycle::{call, error_code, router};
use crate::enrichment_jobs::create_dataset;

/// The workspace every row works in: deleted and opened again under the SAME name,
/// so the request the row resends is the same request throughout and only the
/// record its token was spent on has gone.
const WORKSPACE: &str = "plant-floor";

/// A `TimeInNanos` body, as the two members the shape declares.
fn instant(seconds: i64) -> Value {
    json!({ "timeInSeconds": seconds, "offsetInNanos": 0 })
}

/// A `clientToken` built from `seed`: the modelled `ClientToken` is `len 36..64,
/// pattern ^\S{36,64}$`, so the seed is padded with non-space characters to the
/// shortest admitted length.
fn client_token(seed: &str) -> String {
    let mut token: String = seed.chars().filter(|c| !c.is_whitespace()).collect();
    while token.len() < 36 {
        token.push('z');
    }
    token.truncate(36);
    token
}

/// `body` with `clientToken` added, as each of these operations takes it in the
/// payload.
fn with_token(body: &Value, token: &str) -> Value {
    let mut carried = body.clone();
    carried
        .as_object_mut()
        .expect("an object request body")
        .insert("clientToken".to_string(), json!(token));
    carried
}

/// One row: an operation that produces a record a workspace's delete forgets, the
/// request its token is spent on and a DIFFERENT request of the same operation.
struct Row {
    case: &'static str,
    /// The route the record is produced through, under the workspace it runs in.
    start: fn(&str) -> String,
    /// The status the operation models for a served call.
    served: u16,
    /// The output member the produced record is named by.
    id_member: &'static str,
    /// The request the row's token is spent on, built from the two datasets the
    /// fixture holds (rows that run over no dataset ignore both).
    first: fn(&str, &str) -> Value,
    /// The same operation differing in ONE member, so it repeats no call.
    second: fn(&str, &str) -> Value,
    /// The operation's own describe, given the workspace and the record's id.
    read: fn(&str, &str) -> String,
}

/// The rows: one per kind of record a workspace's delete drops with it.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a search",
            start: |workspace| format!("/workspaces/{workspace}/searches"),
            served: 202,
            id_member: "searchId",
            first: |_, _| json!({ "queryStatement": "turbine vibration last week" }),
            second: |_, _| json!({ "queryStatement": "pump pressure yesterday" }),
            read: |workspace, id| format!("/workspaces/{workspace}/searches/{id}"),
        },
        Row {
            case: "an asynchronous query",
            start: |workspace| format!("/workspaces/{workspace}/queries"),
            served: 202,
            id_member: "queryId",
            first: |_, _| json!({ "queryStatement": "SELECT asset_name FROM asset" }),
            second: |_, _| json!({ "queryStatement": "SELECT asset_id FROM asset_property" }),
            read: |workspace, id| format!("/workspaces/{workspace}/queries/{id}"),
        },
        Row {
            case: "an enrichment job",
            start: |workspace| format!("/workspaces/{workspace}/enrichment-jobs"),
            // `CreateEnrichmentJob` answers 200, not the 202 the other three do.
            served: 200,
            id_member: "jobId",
            first: |one, _| {
                json!({
                    "jobConfiguration": {
                        "eventDetection": {
                            "datasetId": one,
                            "trimSettings": {
                                "startTime": instant(1_700_000_000),
                                "endTime": instant(1_700_003_600),
                            },
                        },
                    },
                })
            },
            // Another dataset, so this request collides with no job running over
            // the first one either.
            second: |_, two| {
                json!({
                    "jobConfiguration": {
                        "eventDetection": {
                            "datasetId": two,
                            "trimSettings": {
                                "startTime": instant(1_700_000_000),
                                "endTime": instant(1_700_003_600),
                            },
                        },
                    },
                })
            },
            read: |workspace, id| format!("/workspaces/{workspace}/enrichment-jobs/{id}"),
        },
        Row {
            case: "a dataset export job",
            start: |workspace| format!("/workspaces/{workspace}/dataset-export-jobs"),
            served: 202,
            id_member: "jobId",
            first: |one, _| {
                json!({
                    "destinationS3Uri": "s3://sitewise-exports/clips/",
                    "errorReportLocation": { "s3Uri": "s3://sitewise-exports/errors/" },
                    "input": { "dataset": { "datasetId": one } },
                })
            },
            second: |one, _| {
                json!({
                    "destinationS3Uri": "s3://sitewise-exports/other-clips/",
                    "errorReportLocation": { "s3Uri": "s3://sitewise-exports/errors/" },
                    "input": { "dataset": { "datasetId": one } },
                })
            },
            read: |workspace, id| format!("/workspaces/{workspace}/dataset-export-jobs/{id}"),
        },
    ]
}

/// `true` when `arn` is an ARN of this service whose resource section ENDS with
/// `id`: the modelled `ResourceArn` form (`^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`),
/// naming the record the conflict reports.
///
/// Hand-rolled as an INDEPENDENT oracle rather than compared with whatever the
/// handler built, so an ARN naming some other resource cannot agree with itself and
/// pass.
fn names_the_record(arn: &str, id: &str) -> bool {
    let sections: Vec<&str> = arn.split(':').collect();
    sections.len() >= 6
        && sections[0] == "arn"
        && sections[1].starts_with("aws")
        && sections[2] == "iotsitewise"
        && arn
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-:/_.".contains(&byte))
        && sections[5..].join(":").ends_with(&format!("/{id}"))
}

/// `CreateWorkspace`, asserted accepted: every record below is produced inside a
/// workspace this service really holds.
async fn open_workspace(router: &MultiServiceRouter, case: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "workspaceName": WORKSPACE,
            "encryptionConfiguration": { "encryptionType": "SITEWISE_DEFAULT_ENCRYPTION" },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: opening {WORKSPACE} expected 200, got {status} {body}"
    );
}

/// `DeleteWorkspace`, asserted accepted: this is what forgets the records the rows
/// produced, without any call naming one of them.
async fn delete_workspace(router: &MultiServiceRouter, case: &str) {
    let (status, body) = call(router, "DELETE", &format!("/workspaces/{WORKSPACE}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting {WORKSPACE} expected 200, got {status} {body}"
    );
}

/// The id a served call reports, asserted present and of the row's own member.
fn produced_id(row: &Row, body: &Value) -> String {
    body[row.id_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: no {} in {body}",
                row.case, row.id_member
            )
        })
        .to_string()
}

#[tokio::test]
async fn a_spent_token_conflicts_only_while_the_store_holds_what_it_was_spent_on() {
    for row in rows() {
        // A router per row, so each row sees only what its own calls left behind.
        let router = router();
        open_workspace(&router, row.case).await;
        // Two datasets OUTSIDE any workspace (`CreateDataset`'s `workspaceName` is
        // optional), so they are no content of the workspace the row deletes and
        // the requests that name them are the same requests before and after that
        // delete.
        let one = create_dataset(&router, &format!("{} source one", row.case)).await;
        let two = create_dataset(&router, &format!("{} source two", row.case)).await;

        let token = client_token(row.case);
        let first_body = (row.first)(&one, &two);
        let second_body = (row.second)(&one, &two);
        let start_uri = (row.start)(WORKSPACE);

        let (status, first) =
            call(&router, "POST", &start_uri, Some(with_token(&first_body, &token))).await;
        assert_eq!(
            status, row.served,
            "case {:?}: the first call expected {}, got {status} {first}",
            row.case, row.served
        );
        let first_id = produced_id(&row, &first);

        // STATE 1 — the record is held, so the differing request conflicts with it
        // and is refused, naming it by both required members.
        let (status, refused) =
            call(&router, "POST", &start_uri, Some(with_token(&second_body, &token))).await;
        assert_eq!(
            (status, error_code(&refused)),
            (409, "ConflictingOperationException"),
            "case {:?}: a token spent on a held record expected the modelled conflict, \
             got {status} {refused}",
            row.case
        );
        assert_eq!(
            refused["resourceId"].as_str().unwrap_or(""),
            first_id,
            "case {:?}: the conflict names the record the token's call produced, got {refused}",
            row.case
        );
        let reported_arn = refused["resourceArn"].as_str().unwrap_or("");
        assert!(
            names_the_record(reported_arn, &first_id),
            "case {:?}: expected an ARN naming {first_id}, got {reported_arn:?}",
            row.case
        );

        // The workspace's delete forgets the record, and the workspace is opened
        // again under the same name — so the request below is unchanged and only
        // what its token was spent on has gone.
        delete_workspace(&router, row.case).await;
        open_workspace(&router, row.case).await;
        let (status, gone) = call(&router, "GET", &(row.read)(WORKSPACE, &first_id), None).await;
        assert_eq!(
            status, 404,
            "case {:?}: the record the token was spent on must be gone, got {status} {gone}",
            row.case
        );

        // STATE 2 — nothing is left for the request to conflict with, so it is
        // served as the fresh call it now is and produces a record of its own.
        let (status, served) =
            call(&router, "POST", &start_uri, Some(with_token(&second_body, &token))).await;
        assert_eq!(
            status, row.served,
            "case {:?}: with nothing left to conflict with, the request expected {}, \
             got {status} {served}",
            row.case, row.served
        );
        let second_id = produced_id(&row, &served);
        assert_ne!(
            second_id, first_id,
            "case {:?}: the served call must produce a record of its own, got {first_id} again",
            row.case
        );
        let (status, read_back) = call(&router, "GET", &(row.read)(WORKSPACE, &second_id), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the record the served call produced must read back, got {status} \
             {read_back}",
            row.case
        );
        assert_eq!(
            read_back[row.id_member].as_str().unwrap_or(""),
            second_id,
            "case {:?}: the read must report the record the served call produced, got {read_back}",
            row.case
        );

        // STATE 3 — that served call is what the token names now, so repeating it
        // replays the record it produced instead of producing a second.
        let (status, replayed) =
            call(&router, "POST", &start_uri, Some(with_token(&second_body, &token))).await;
        assert_eq!(
            status, row.served,
            "case {:?}: the retry of the served call expected {}, got {status} {replayed}",
            row.case, row.served
        );
        assert_eq!(
            produced_id(&row, &replayed),
            second_id,
            "case {:?}: a retry answers from the record its own call produced, got {replayed}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_token_spent_on_a_held_record_goes_on_refusing_every_differing_request() {
    // The conflict is not weakened for a record that is STILL held: the same
    // differing request, sent again and again while the record stands, is refused
    // every time and produces nothing.
    for row in rows() {
        let router = router();
        open_workspace(&router, row.case).await;
        let one = create_dataset(&router, &format!("{} held source one", row.case)).await;
        let two = create_dataset(&router, &format!("{} held source two", row.case)).await;

        let token = client_token(&format!("held-{}", row.case));
        let start_uri = (row.start)(WORKSPACE);
        let (status, first) = call(
            &router,
            "POST",
            &start_uri,
            Some(with_token(&(row.first)(&one, &two), &token)),
        )
        .await;
        assert_eq!(
            status, row.served,
            "case {:?}: the first call expected {}, got {status} {first}",
            row.case, row.served
        );
        let first_id = produced_id(&row, &first);

        for attempt in 1..=3 {
            let (status, refused) = call(
                &router,
                "POST",
                &start_uri,
                Some(with_token(&(row.second)(&one, &two), &token)),
            )
            .await;
            assert_eq!(
                (status, error_code(&refused)),
                (409, "ConflictingOperationException"),
                "case {:?} attempt {attempt}: expected the modelled conflict, got {status} \
                 {refused}",
                row.case
            );
            assert_eq!(
                refused["resourceId"].as_str().unwrap_or(""),
                first_id,
                "case {:?} attempt {attempt}: the conflict names the held record, got {refused}",
                row.case
            );
        }

        // The refusals produced nothing: the record the first call produced is the
        // one the store still reports, and a retry of THAT request still answers
        // from it.
        let (status, held) = call(&router, "GET", &(row.read)(WORKSPACE, &first_id), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held record must still read back, got {status} {held}",
            row.case
        );
        let (status, retried) = call(
            &router,
            "POST",
            &start_uri,
            Some(with_token(&(row.first)(&one, &two), &token)),
        )
        .await;
        assert_eq!(
            status, row.served,
            "case {:?}: the retry expected {}, got {status} {retried}",
            row.case, row.served
        );
        assert_eq!(
            produced_id(&row, &retried),
            first_id,
            "case {:?}: a refused request must not unspend the token, got {retried}",
            row.case
        );
    }
}
