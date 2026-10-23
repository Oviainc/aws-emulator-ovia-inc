// SPDX-License-Identifier: CC0-1.0

//! How Strategy Recommendations WORDS a refusal, end to end over HTTP.
//!
//! The rule under test has two halves, and both are properties of the SERVICE rather
//! than of any one operation, member or value:
//!
//! 1. a request carrying a value that fails a rule the model declares is refused with
//!    400 `ValidationException` whose message is the service's one sentence,
//!    `Invalid request body` — the `ValidationException` the model documents as "The
//!    request body isn't valid." — naming neither the member that failed nor the value
//!    it rejected, whatever the constraint kind and wherever the value was bound (JSON
//!    body or URI label). The rows below cover the kinds the scaffold's boundary
//!    validator enforces: `@length`, `@range` and an absent `@required` member;
//! 2. a request naming an id that resolves to no resource is refused with the sentence
//!    the service uses for that condition, which likewise carries nothing from the
//!    request — in particular not the id that failed to resolve. Where the operation
//!    reports the id as an unresolvable PARAMETER that is
//!    `Looks like you provided invalid or missing parameters.`; where it reports the
//!    resource as ABSENT (`StopAssessment`'s recorded 404) it is
//!    `Invalid resourceId: `. One sentence per condition, never a rendering of the
//!    request.
//!
//! So each table below spans the class the rule generalises over and asserts the ONE
//! sentence for every row; the recorded probes (a 70-character `S3Bucket`, a
//! `speedOfMigration` of 99, a `GetImportFileTask` id nothing minted) are three rows
//! among many. The complementary table pins that the refusal is scoped to values that
//! actually fail a constraint: every published bound is accepted AT the bound.
//!
//! Both sentences are spelled out here independently of `src/state.rs`'s constants, so a
//! wrong message in the crate fails here rather than agreeing with itself.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{call, emulator, Answer};

/// This service's message for a value that fails a rule the model declares.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// This service's message for an id that resolves to no resource, on an operation that
/// reports it as an unresolvable parameter.
const INVALID_OR_MISSING_PARAMETERS: &str = "Looks like you provided invalid or missing parameters.";

/// This service's message for an id that resolves to no resource, on an operation that
/// reports the resource as absent — the recorded `StopAssessment` 404.
const INVALID_RESOURCE_ID: &str = "Invalid resourceId: ";

/// `[0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+` within `importS3Bucket`'s `@length(0, 63)`, from
/// `smithy_query(query="shape",shape="importS3Bucket")`: a legal bucket name of exactly
/// `len` characters.
fn bucket_of_length(len: usize) -> String {
    let prefix = "migrationhub-strategy-";
    assert!(len > prefix.len(), "a bucket name shorter than the prefix");
    format!("{prefix}{}", "a".repeat(len - prefix.len()))
}

/// A `StartImportFileTask` body carrying only the three required members, every one of
/// them legal.
fn valid_import() -> Value {
    json!({
        "name": "refusal-wording",
        "S3Bucket": "migrationhub-strategy-import",
        "s3key": "servers.csv",
    })
}

/// A `PutPortfolioPreferences` body setting one `BusinessGoals` member to `rank`.
///
/// Every member of `BusinessGoals` targets `BusinessGoalsInteger` `@range(1, 5)`, from
/// `smithy_query(query="shape",shape="BusinessGoals,BusinessGoalsInteger")`.
fn business_goal(member: &str, rank: i64) -> Value {
    json!({"prioritizeBusinessGoals": {"businessGoals": {member: rank}}})
}

/// One request the boundary validator must refuse: what it is, and how to send it.
struct Case {
    /// What the row is probing, named in every assertion.
    what: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

fn post(what: &'static str, uri: &str, body: Value) -> Case {
    Case {
        what,
        method: "POST",
        uri: uri.to_string(),
        body: Some(body),
    }
}

fn get(what: &'static str, uri: String) -> Case {
    Case {
        what,
        method: "GET",
        uri,
        body: None,
    }
}

/// Requests that fail a declared rule, spanning the constraint kinds the model uses and
/// both places a value is bound.
///
/// Members and their constraints come from
/// `smithy_query(query="shape",shape="StartImportFileTaskRequest,importS3Bucket,BusinessGoals,BusinessGoalsInteger,AsyncTaskId")`:
/// `S3Bucket` -> `importS3Bucket` `@length(0,63)` `@pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+`,
/// required alongside `name` and `s3key`; `BusinessGoals`' members ->
/// `BusinessGoalsInteger` `@range(1,5)`; `AsyncTaskId` `@length(0,52)`
/// `@pattern [0-9a-z-:]+`.
///
/// Every row's constraint is declared on the member's TARGET shape, which is what the
/// generated schema carries, and is a `@length`, `@range` or `@required` — the three the
/// scaffold's boundary validator enforces. (`name` and `s3key` target the unconstrained
/// `String` and carry their own `@length`/`@pattern` on the member, and `@pattern` is not
/// enforced at all; both are gaps in the generated schema rather than in how this service
/// WORDS a refusal, so no row rests on either.)
fn refused_cases() -> Vec<Case> {
    let mut long_bucket = valid_import();
    long_bucket["S3Bucket"] = json!(bucket_of_length(70));
    let mut one_past_bucket_max = valid_import();
    one_past_bucket_max["S3Bucket"] = json!(bucket_of_length(64));
    let mut no_name = valid_import();
    let _ = no_name.as_object_mut().map(|body| body.remove("name"));
    let mut no_key = valid_import();
    let _ = no_key.as_object_mut().map(|body| body.remove("s3key"));

    vec![
        // length: the recorded probe, then the first value past the published bound.
        post(
            "S3Bucket of 70 characters, past importS3Bucket's @length max of 63",
            "/start-import-file-task",
            long_bucket,
        ),
        post(
            "S3Bucket of 64 characters, one past importS3Bucket's @length max of 63",
            "/start-import-file-task",
            one_past_bucket_max,
        ),
        // an absent required member, which the model requires just as firmly.
        post(
            "no name, which StartImportFileTaskRequest requires",
            "/start-import-file-task",
            no_name,
        ),
        post(
            "no s3key, which StartImportFileTaskRequest requires",
            "/start-import-file-task",
            no_key,
        ),
        // range, on a member nested two structures deep: the recorded probe, then each
        // side of the published bound.
        post(
            "speedOfMigration of 99, past BusinessGoalsInteger's @range max of 5",
            "/put-portfolio-preferences",
            business_goal("speedOfMigration", 99),
        ),
        post(
            "speedOfMigration of 6, one past BusinessGoalsInteger's @range max of 5",
            "/put-portfolio-preferences",
            business_goal("speedOfMigration", 6),
        ),
        post(
            "speedOfMigration of 0, one below BusinessGoalsInteger's @range min of 1",
            "/put-portfolio-preferences",
            business_goal("speedOfMigration", 0),
        ),
        post(
            "licenseCostReduction of 0, a different member of the same shape",
            "/put-portfolio-preferences",
            business_goal("licenseCostReduction", 0),
        ),
        post(
            "modernizeInfrastructureWithCloudNativeTechnologies of 42, same shape again",
            "/put-portfolio-preferences",
            business_goal("modernizeInfrastructureWithCloudNativeTechnologies", 42),
        ),
        // a value bound to a URI label rather than to the body.
        get(
            "an assessment id of 53 characters, one past AsyncTaskId's @length max of 52",
            format!("/get-assessment/{}", "a".repeat(53)),
        ),
    ]
}

/// Every refused request gets 400 `ValidationException` with the service's ONE message —
/// no member path, no rejected value, no generic AWS-SDK constraint envelope.
#[tokio::test]
async fn a_declared_rule_is_refused_with_the_services_one_validation_message() {
    let router = emulator();
    for case in refused_cases() {
        let Case {
            what,
            method,
            uri,
            body,
        } = case;
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_REQUEST_BODY,
            "case {what:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
            answer.body
        );
    }
}

/// A request at each published bound is NOT refused: the one message is the answer to a
/// genuine constraint failure, not to every request.
///
/// The assertion is "not a validation refusal", so it holds whatever the operation then
/// does with the request it accepted: the point of the table is that the boundary
/// validator let it through to the handler. (`PutPortfolioPreferences` now saves it and
/// answers 200 — the round trip itself is pinned in `portfolio_preferences.rs`.)
#[tokio::test]
async fn a_value_at_a_published_bound_is_not_refused() {
    let router = emulator();

    let mut at_bucket_max = valid_import();
    at_bucket_max["S3Bucket"] = json!(bucket_of_length(63));
    // The shortest name the SERVICE accepts, not the shortest `@pattern` admits: the
    // member documents a `migrationhub-strategy-` prefix, so the shortest acceptable
    // bucket is that prefix plus one character. A two-character name satisfies the bound
    // this table is about and is still refused, for the prefix rule rather than for the
    // bound — see `assessment_error_paths.rs`.
    let mut shortest_bucket = valid_import();
    shortest_bucket["S3Bucket"] = json!(bucket_of_length("migrationhub-strategy-".len() + 1));

    let accepted = vec![
        post(
            "S3Bucket of 63 characters, AT importS3Bucket's @length max",
            "/start-import-file-task",
            at_bucket_max,
        ),
        post(
            "the shortest S3Bucket the service's documented prefix admits",
            "/start-import-file-task",
            shortest_bucket,
        ),
        post(
            "speedOfMigration of 1, AT BusinessGoalsInteger's @range min",
            "/put-portfolio-preferences",
            business_goal("speedOfMigration", 1),
        ),
        post(
            "speedOfMigration of 5, AT BusinessGoalsInteger's @range max",
            "/put-portfolio-preferences",
            business_goal("speedOfMigration", 5),
        ),
        post(
            "licenseCostReduction of 3, inside the same @range",
            "/put-portfolio-preferences",
            business_goal("licenseCostReduction", 3),
        ),
    ];

    for case in accepted {
        let Case {
            what,
            method,
            uri,
            body,
        } = case;
        let answer = call(&router, method, &uri, body).await;
        assert_ne!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: a value at a published bound was refused: {:?}",
            answer.body
        );
        assert_ne!(
            answer.status, 400,
            "case {what:?}: a value at a published bound was refused with {:?}",
            answer.body
        );
    }
}

/// Ids that satisfy every declared rule and still resolve to nothing. Each row differs
/// in the SHAPE of the id, so nothing about the answer can depend on the id itself.
///
/// Each id matches `AsyncTaskId`'s `[0-9a-z-:]+` within 52 characters as well as the
/// looser `.*\S.*` of `GetImportFileTaskRequest$id`, so one list serves every operation
/// and every refusal here comes from the handler rather than from the boundary validator.
const UNRESOLVABLE_IDS: &[(&str, &str)] = &[
    ("the all-zero uuid", "00000000-0000-0000-0000-000000000000"),
    (
        "a minted-looking id nothing minted",
        "import-file-task-00000000ffff",
    ),
    ("a single character", "z"),
    ("a colon-separated id", "task:0:never-created"),
    (
        "52 characters, at AsyncTaskId's @length max",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
];

/// One operation's answer to an unresolvable id: which operation asked, what came back,
/// the status/code pair pinned for it, and the sentence its condition is worded with.
struct Probe {
    operation: &'static str,
    answer: Answer,
    pinned: Option<(u16, &'static str)>,
    message: &'static str,
}

/// Every operation that resolves an id answers an id it cannot resolve with the service's
/// ONE sentence for that condition — and with nothing from the request in it.
///
/// Each operation's status and error code is pinned to what the corpus records for its
/// route: `GetImportFileTask` and `StopAssessment` report the resource as absent (404 /
/// `ResourceNotFoundException`), while `GetAssessment` refuses the id as an invalid
/// parameter (400 / `ValidationException`). `StopAssessment` is the one of the three that
/// words the condition as an absent resource rather than an unresolvable parameter, so it
/// carries its own sentence.
#[tokio::test]
async fn an_unresolvable_id_is_refused_with_the_services_one_parameter_message() {
    let router = emulator();

    for (shape, id) in UNRESOLVABLE_IDS {
        let probes: Vec<Probe> = vec![
            Probe {
                operation: "GetImportFileTask",
                answer: call(&router, "GET", &format!("/get-import-file-task/{id}"), None).await,
                pinned: Some((404, "ResourceNotFoundException")),
                message: INVALID_OR_MISSING_PARAMETERS,
            },
            Probe {
                operation: "GetAssessment",
                answer: call(&router, "GET", &format!("/get-assessment/{id}"), None).await,
                pinned: Some((400, "ValidationException")),
                message: INVALID_OR_MISSING_PARAMETERS,
            },
            Probe {
                operation: "StopAssessment",
                answer: call(
                    &router,
                    "POST",
                    "/stop-assessment",
                    Some(json!({"assessmentId": id})),
                )
                .await,
                pinned: Some((404, "ResourceNotFoundException")),
                message: INVALID_RESOURCE_ID,
            },
        ];

        for Probe {
            operation,
            answer,
            pinned,
            message: expected,
        } in probes
        {
            let case = format!("{operation} with {shape} ({id:?})");
            assert!(
                (400..500).contains(&answer.status),
                "case {case:?}: expected a 4xx refusal, got {} with {:?}",
                answer.status,
                answer.body
            );
            if let Some((status, code)) = pinned {
                assert_eq!(
                    answer.status, status,
                    "case {case:?}: expected HTTP {status}, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    code,
                    "case {case:?}: expected {code}, got {:?}",
                    answer.body
                );
            }
            let message = answer.string("/message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the service's {expected:?}, got {:?}",
                answer.body
            );
            assert!(
                !message.contains(id),
                "case {case:?}: the refusal quoted the caller's id back at it: {message:?}"
            );
        }
    }
}

/// A resolvable id is still answered from state: the one sentence above is a refusal,
/// not a blanket answer for every read.
#[tokio::test]
async fn a_resolvable_id_is_still_answered_from_state() {
    let router = emulator();
    let started = call(
        &router,
        "POST",
        "/start-import-file-task",
        Some(valid_import()),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "a legal StartImportFileTask was refused: {:?}",
        started.body
    );
    let id = started.string("/id");

    let got = read_import(&router, &id).await;
    assert_eq!(
        got.status, 200,
        "case {id:?}: a minted import file task was not reported: {:?}",
        got.body
    );
    assert_eq!(
        got.string("/id"),
        id,
        "case {id:?}: a different id was reported: {:?}",
        got.body
    );
    assert_eq!(
        got.string("/message"),
        "",
        "case {id:?}: a success carried a refusal message: {:?}",
        got.body
    );
}

async fn read_import(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/get-import-file-task/{id}"), None).await
}
