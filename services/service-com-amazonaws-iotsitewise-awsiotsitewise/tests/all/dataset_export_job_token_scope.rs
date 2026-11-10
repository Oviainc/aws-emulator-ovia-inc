// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` IDENTIFIES for `CreateDatasetExportJob`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: a `clientToken` names the one SUCCESSFUL call of
//! THAT operation that spent it, so only a repeat of that call's whole request
//! replays its answer — any other request under the token repeats no call and is
//! refused the modelled conflict naming the resource the token's own call made,
//! while a token a REFUSED request carried was never spent and a token spent on
//! another operation does not bear on this one.
//!
//! The suite beside this one (`dataset_export_jobs`) pins the rule over the
//! members a create is recognised by; these rows span what it does not:
//!
//! * every token the model admits — `ClientToken` is `@length 36..64` with
//!   `pattern ^\S{36,64}$` (smithy_query query="shape" shape="ClientToken"), so
//!   the rows walk both published bounds, a length between them and a token that
//!   is not a UUID at all, plus one character past each bound;
//! * a token carried by a create that was REFUSED, which spends nothing whatever
//!   refused it;
//! * a token already spent on a DIFFERENT operation of this service, which is a
//!   different token as far as this operation is concerned.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `is_modelled_id`)
//! and the export-job helpers (`create`, `describe`, `create_body`,
//! `create_body_from`, `jobs_held`, `create_dataset`) are the ones the sibling
//! suites define; all are reused rather than copied.

use serde_json::{json, Value};

use super::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine};
use super::dataset_export_jobs::{
    create, create_body, create_body_from, create_dataset, describe, jobs_held, ASSOCIATED_SERIES,
    DESTINATION, DISASSOCIATED_SERIES, ERROR_REPORT, OTHER_DESTINATION,
};

/// The workspace these suites work in.
const WORKSPACE: &str = "export-tokens";

/// The message a uniqueness refusal of this service carries: AWS names the
/// DIMENSION of the request that collided, not an identifier the rejected request
/// would have minted.
const CONFLICT_MESSAGE: &str = "Another dataset export job is already using the same clientToken";

/// A token of `length` characters, built from characters `^\S{36,64}$` admits.
fn token_of(length: usize, seed: char) -> String {
    std::iter::repeat_n(seed, length).collect()
}

/// A `ProcessingInput` naming one time series by id.
fn one_series(series: &str) -> Value {
    json!({ "timeseries": [{ "timeSeriesId": series }] })
}

/// Assert `body` is the modelled conflict naming `job_id`.
fn assert_conflict(case: &str, status: u16, body: &Value, workspace: &str, job_id: &str) {
    assert_eq!(
        status, 409,
        "case {case:?}: a request differing from the call its token was spent on expected 409, \
         got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ConflictingOperationException",
        "case {case:?}: expected ConflictingOperationException, got {body}"
    );
    assert_eq!(
        body["resourceId"].as_str().unwrap_or(""),
        job_id,
        "case {case:?}: the conflict must name the job the token's own call started, got {body}"
    );
    assert!(
        body["resourceArn"]
            .as_str()
            .unwrap_or_default()
            .ends_with(&format!("{workspace}/{job_id}")),
        "case {case:?}: the conflict must carry that job's ARN, got {body}"
    );
    assert_eq!(
        body["message"].as_str().unwrap_or(""),
        CONFLICT_MESSAGE,
        "case {case:?}: unexpected refusal message in {body}"
    );
}

/// One row of the token-form table: a `clientToken` the model admits, or one past
/// a published bound.
struct TokenCase {
    label: &'static str,
    token: String,
    /// Whether `ClientToken`'s constraints admit the token at all.
    admitted: bool,
}

/// The rows: `ClientToken` is `@length 36..64`, `pattern ^\S{36,64}$`
/// (smithy_query query="shape" shape="ClientToken"), so the forms span both
/// published bounds, a length between them, a token carrying no hyphens at all —
/// the pattern constrains nothing but the non-space characters and the count —
/// and one character past each bound.
fn token_cases() -> Vec<TokenCase> {
    vec![
        TokenCase {
            label: "a uuid, the shortest the length admits",
            token: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
            admitted: true,
        },
        TokenCase {
            label: "not a uuid, at the shortest length",
            token: token_of(36, 'z'),
            admitted: true,
        },
        TokenCase {
            label: "a punctuated token between the bounds",
            token: format!("{}{}", "token.spent/by#one-call_", token_of(26, '7')),
            admitted: true,
        },
        TokenCase {
            label: "the longest the length admits",
            token: token_of(64, 'q'),
            admitted: true,
        },
        TokenCase {
            label: "one character short of the shortest",
            token: token_of(35, 'z'),
            admitted: false,
        },
        TokenCase {
            label: "one character past the longest",
            token: token_of(65, 'q'),
            admitted: false,
        },
    ]
}

#[tokio::test]
async fn the_token_rule_holds_for_every_token_the_model_admits() {
    for case in token_cases() {
        let label = case.label;
        let token = case.token.as_str();
        // A router of its own per row, so the job counts below are about THIS
        // row's calls only.
        let (router, engine) = router_with_engine();
        let input = one_series(ASSOCIATED_SERIES);

        let (status, first) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;

        if !case.admitted {
            assert_eq!(
                status, 400,
                "case {label:?}: a token outside the modelled length expected 400, \
                 got {status} {first}"
            );
            let code = error_code(&first);
            assert!(
                matches!(code, "InvalidRequestException" | "ValidationException"),
                "case {label:?}: expected a client refusal of the token, got {code} in {first}"
            );
            let held = jobs_held(&engine);
            assert_eq!(
                held, 0,
                "case {label:?}: a refused create must start no job, the store holds {held}"
            );
            continue;
        }

        assert_eq!(
            status, 202,
            "case {label:?}: the first create expected 202, got {status} {first}"
        );
        let job_id = first["jobId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&job_id),
            "case {label:?}: the create must answer a modelled jobId, got {first}"
        );

        // The SAME request again under that token is a retry: it replays the job
        // the first call started and starts nothing.
        let (status, retried) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;
        assert_eq!(
            status, 202,
            "case {label:?}: a retry expected 202, got {status} {retried}"
        );
        assert_eq!(
            retried["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {label:?}: a retry must replay the job the first create started, got {retried}"
        );

        // A DIFFERENT request under the same token repeats no call, so it is the
        // modelled conflict naming the job that token's own call started.
        let (status, differing) = create(
            &router,
            WORKSPACE,
            create_body_from(OTHER_DESTINATION, ERROR_REPORT, &input, Some(token)),
        )
        .await;
        assert_conflict(label, status, &differing, WORKSPACE, &job_id);

        // The job the token's call started is untouched by either second call.
        let (status, described) = describe(&router, WORKSPACE, &job_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the read of the job got {status} {described}"
        );
        assert_eq!(
            described["destinationS3Uri"].as_str().unwrap_or(""),
            DESTINATION,
            "case {label:?}: neither second call may rewrite the job's destination, \
             got {described}"
        );
        let held = jobs_held(&engine);
        assert_eq!(
            held, 1,
            "case {label:?}: the retry and the refusal must have written nothing, \
             the store holds {held}"
        );
    }
}

/// One row of the refused-create table: a create this service refuses, and the
/// token it carried.
struct RefusalCase {
    label: &'static str,
    /// The workspace the create is addressed to. The row about an unopened
    /// workspace names one this suite never opens.
    workspace: &'static str,
    /// Whether the workspace has to be opened before the refused call.
    open_workspace: bool,
    /// The processing input the refused create carries, built from the id of the
    /// dataset this suite created.
    input: fn(&str) -> Value,
    status: u16,
    code: &'static str,
    token: &'static str,
}

/// The rows: a create is refused for a rule its own input shapes state, for a
/// resource the input names that the store does not hold, and for a workspace
/// nobody opened — three different refusals, each of which starts no job and so
/// spends no token.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            label: "an item naming both identifiers",
            workspace: WORKSPACE,
            open_workspace: true,
            input: |_| {
                json!({
                    "timeseries": [{
                        "timeSeriesId": ASSOCIATED_SERIES,
                        "propertyAlias": "/plant/line-1/rate",
                    }]
                })
            },
            status: 400,
            code: "InvalidRequestException",
            token: "11111111-aaaa-bbbb-cccc-111111111111",
        },
        RefusalCase {
            label: "a dataset no store holds",
            workspace: WORKSPACE,
            open_workspace: true,
            input: |_| json!({ "dataset": { "datasetId": "ffffffff-ffff-ffff-ffff-ffffffffffff" } }),
            status: 404,
            code: "ResourceNotFoundException",
            token: "22222222-aaaa-bbbb-cccc-222222222222",
        },
        RefusalCase {
            label: "a workspace nobody opened",
            workspace: "never-opened",
            open_workspace: false,
            input: |dataset_id| json!({ "dataset": { "datasetId": dataset_id } }),
            status: 404,
            code: "ResourceNotFoundException",
            token: "33333333-aaaa-bbbb-cccc-333333333333",
        },
    ]
}

#[tokio::test]
async fn a_token_a_refused_create_carried_is_not_spent() {
    for case in refusal_cases() {
        let label = case.label;
        let token = case.token;
        let (router, engine) = router_with_engine();
        let dataset_id = create_dataset(&router, "Export source").await;

        let refused_body = create_body(&(case.input)(&dataset_id), Some(token));
        let (status, refused) = if case.open_workspace {
            create(&router, case.workspace, refused_body).await
        } else {
            // Addressed directly, so the workspace this row is about stays
            // unopened.
            call(
                &router,
                "POST",
                &format!("/workspaces/{}/dataset-export-jobs", case.workspace),
                Some(refused_body),
            )
            .await
        };
        assert_eq!(
            status, case.status,
            "case {label:?}: expected {}, got {status} {refused}",
            case.status
        );
        assert_eq!(
            error_code(&refused),
            case.code,
            "case {label:?}: unexpected error code in {refused}"
        );
        let held = jobs_held(&engine);
        assert_eq!(
            held, 0,
            "case {label:?}: a refused create must start no job, the store holds {held}"
        );

        // A request the refused one differs from, under the SAME token. Had the
        // refusal spent the token, this would be refused the conflict; it is
        // served, because nothing ran under that token yet.
        let input = one_series(DISASSOCIATED_SERIES);
        let (status, served) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;
        assert_eq!(
            status, 202,
            "case {label:?}: a token a refused create carried must still be free, \
             got {status} {served}"
        );
        let job_id = served["jobId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&job_id),
            "case {label:?}: the served create must answer a modelled jobId, got {served}"
        );

        // And that call is what the token now names: its repeat replays the job.
        let (status, retried) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;
        assert_eq!(
            status, 202,
            "case {label:?}: a retry expected 202, got {status} {retried}"
        );
        assert_eq!(
            retried["jobId"].as_str().unwrap_or(""),
            job_id,
            "case {label:?}: a retry must replay the job the served create started, got {retried}"
        );
        let held = jobs_held(&engine);
        assert_eq!(
            held, 1,
            "case {label:?}: the retry must have written nothing, the store holds {held}"
        );
    }
}

#[tokio::test]
async fn a_token_is_scoped_to_the_operation_it_was_sent_to() {
    let (router, engine) = router_with_engine();
    let token = "44444444-aaaa-bbbb-cccc-444444444444";

    // The token is spent on ANOTHER operation of this service first.
    let (status, dataset) = call(
        &router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": "Export source",
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
            "clientToken": token,
        })),
    )
    .await;
    assert_eq!(status, 202, "the dataset create got {status} {dataset}");
    let dataset_id = dataset["datasetId"].as_str().unwrap_or("").to_string();

    // That operation holds the token against its OWN call: a different request of
    // it is refused the conflict.
    let (status, other_dataset) = call(
        &router,
        "POST",
        "/datasets",
        Some(json!({
            "datasetName": "Another source",
            "datasetSource": { "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" },
            "clientToken": token,
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "a differing dataset create under the spent token expected 409, got {status} {other_dataset}"
    );
    assert_eq!(
        error_code(&other_dataset),
        "ConflictingOperationException",
        "a differing dataset create: unexpected error code in {other_dataset}"
    );

    // CreateDatasetExportJob has served nothing under that value, so the same
    // token is free here: a token is scoped to the operation it was sent to.
    let input = json!({ "dataset": { "datasetId": dataset_id } });
    let (status, started) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;
    assert_eq!(
        status, 202,
        "a token spent on another operation must be free here, got {status} {started}"
    );
    let job_id = started["jobId"].as_str().unwrap_or("").to_string();
    assert!(
        is_modelled_id(&job_id),
        "the create must answer a modelled jobId, got {started}"
    );
    assert_ne!(
        job_id, dataset_id,
        "the job must be one of its own, not the resource the other operation's call made"
    );

    // From here the token names THIS operation's call: its repeat replays the job.
    let (status, retried) = create(&router, WORKSPACE, create_body(&input, Some(token))).await;
    assert_eq!(status, 202, "a retry expected 202, got {status} {retried}");
    assert_eq!(
        retried["jobId"].as_str().unwrap_or(""),
        job_id,
        "a retry must replay the job the create started, got {retried}"
    );

    // And a differing request under it is the conflict naming the JOB that call
    // started — not the dataset the other operation's call made.
    let (status, differing) = create(
        &router,
        WORKSPACE,
        create_body_from(OTHER_DESTINATION, ERROR_REPORT, &input, Some(token)),
    )
    .await;
    assert_conflict(
        "a differing export request",
        status,
        &differing,
        WORKSPACE,
        &job_id,
    );

    let held = jobs_held(&engine);
    assert_eq!(
        held, 1,
        "the retry and the refusal must have written nothing, the store holds {held}"
    );
}
