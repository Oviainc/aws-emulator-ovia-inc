// SPDX-License-Identifier: CC0-1.0

//! The shape of an AWS Backup client-error RESPONSE.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! body a `backup` SDK client parses — not a handler called directly. The rule
//! under test, over a table of operations and values rather than one recorded
//! request:
//!
//! * a request this service refuses is answered with the modelled exception the
//!   operation declares, serialized as THAT shape: the body carries the shape's
//!   `Message` member, and its `Code` member for a condition this service has a
//!   token for (`ERROR_2403`, `ERROR_1101`, `ERROR_3607`), with
//!   `x-amzn-errortype` naming the shape;
//! * a value outside the model's `MaxResults` `@range`, and one outside the
//!   `AggregationPeriod` enum, are refusals of this kind on EVERY operation
//!   that takes them — not framework refusals in a shape the model does not
//!   declare;
//! * the `AggregationPeriod` message is built from the request: it names the
//!   rejected value and the values the model admits;
//! * `AlreadyExistsException` discloses no state about the vault that already
//!   exists: its `Arn` and `CreatorRequestId` members stay unset;
//! * the refusals are not blanket answers — a value the model admits is NOT
//!   refused with them.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` naming this service's signing name (`backup`), which
/// is how the router picks the service.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/backup/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The region in [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "us-west-2";

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// What a response says about itself: the HTTP status, the error shape named by
/// `x-amzn-errortype` (how restJson1 resolves an error shape), and the parsed
/// body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// A body member as a string, or `"<unset>"` when the member is absent —
    /// the distinction this suite exists to pin.
    fn member(&self, name: &str) -> String {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }

    /// The error shape the body itself names (restJson1's `__type`).
    fn json_type(&self) -> String {
        self.member("__type")
    }
}

fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<&str>) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |b| {
        bytes::Bytes::copy_from_slice(b.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body: parsed,
    }
}

async fn create_vault(router: &MultiServiceRouter, name: &str, body: &str) -> Answer {
    call(router, "PUT", &format!("/backup-vaults/{name}"), Some(body)).await
}

/// Start one backup job in `vault` and return its `BackupJobId`.
async fn start_job(router: &MultiServiceRouter, vault: &str) -> String {
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let payload = serde_json::json!({
        "BackupVaultName": vault,
        "ResourceArn": resource,
        "IamRoleArn": role,
    })
    .to_string();
    let answer = call(router, "PUT", "/backup-jobs", Some(&payload)).await;
    assert_eq!(
        answer.status, 200,
        "setup: StartBackupJob should succeed, got {} {:?}",
        answer.status, answer.body
    );
    answer.member("BackupJobId")
}

/// Every route that takes a `MaxResults`, with the name its `@httpQuery` binds
/// it under (`smithy_query` operation `ListProtectedResources` /
/// `ListBackupVaults` / `ListRecoveryPointsByBackupVault` /
/// `List{Backup,Copy,Restore}JobSummaries`).
const MAX_RESULTS_ROUTES: [(&str, &str, &str); 6] = [
    ("ListProtectedResources", "/resources", "maxResults"),
    ("ListBackupVaults", "/backup-vaults", "maxResults"),
    (
        "ListRecoveryPointsByBackupVault",
        "/backup-vaults/code-vault/recovery-points",
        "maxResults",
    ),
    (
        "ListBackupJobSummaries",
        "/audit/backup-job-summaries",
        "MaxResults",
    ),
    (
        "ListCopyJobSummaries",
        "/audit/copy-job-summaries",
        "MaxResults",
    ),
    (
        "ListRestoreJobSummaries",
        "/audit/restore-job-summaries",
        "MaxResults",
    ),
];

/// Every route that takes an `AggregationPeriod`.
const AGGREGATION_ROUTES: [(&str, &str); 3] = [
    ("ListBackupJobSummaries", "/audit/backup-job-summaries"),
    ("ListCopyJobSummaries", "/audit/copy-job-summaries"),
    ("ListRestoreJobSummaries", "/audit/restore-job-summaries"),
];

/// A resource this service does not hold is reported with the condition's own
/// `Code` and a `Message`, and the sentence does NOT echo the id asked about —
/// the one AWS states for it names no part of the request.
///
/// The rows vary the id's form (the minted form, a bare word, a long hex
/// string), and the same emulator answers 200 for the id it did mint, so the
/// refusal is derived from what is stored rather than returned for everything.
#[tokio::test]
async fn an_unknown_backup_job_is_refused_with_its_code_and_a_fixed_message() {
    let router = router();
    create_vault(&router, "code-vault", "{}").await;
    let known = start_job(&router, "code-vault").await;

    let found = call(&router, "GET", &format!("/backup-jobs/{known}"), None).await;
    assert_eq!(
        found.status, 200,
        "case \"the job this emulator minted\": expected 200, got {} {:?}",
        found.status, found.body
    );

    let unknown = [
        ("the minted form, never minted", "db89c7cd-0000-4000-8000-000000000001"),
        ("a bare word", "no-such-job"),
        ("a long hex string", "0123456789abcdef0123456789abcdef"),
        ("a single digit", "7"),
    ];
    for (case, job_id) in unknown {
        let answer = call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
        assert_eq!(
            (
                answer.status,
                answer.errortype.as_str(),
                answer.json_type().as_str(),
                answer.member("Code").as_str(),
                answer.member("Message").as_str(),
            ),
            (
                400,
                "ResourceNotFoundException",
                "ResourceNotFoundException",
                "ERROR_2403",
                "Cannot find Backup job",
            ),
            "case {case:?} ({job_id:?}): expected 400 ResourceNotFoundException \
             with Code=ERROR_2403 and the fixed message, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            !answer.member("Message").contains(job_id),
            "case {case:?} ({job_id:?}): the message must not echo the requested id, got {:?}",
            answer.member("Message")
        );
    }
}

/// A create naming a vault that already exists is refused with the condition's
/// `Code` and sentence, and discloses nothing about the stored vault: `Arn` and
/// `CreatorRequestId` stay unset even when the original create supplied both a
/// token and a key.
///
/// The rows vary what the FIRST create carried, because that is the state the
/// refusal must not leak.
#[tokio::test]
async fn a_duplicate_vault_is_refused_with_its_code_and_discloses_no_stored_state() {
    let key = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555");
    let cases: Vec<(&str, String)> = vec![
        ("a bare create", "{}".to_string()),
        (
            "a create with a CreatorRequestId",
            serde_json::json!({ "CreatorRequestId": "first-call" }).to_string(),
        ),
        (
            "a create with a CreatorRequestId and an EncryptionKeyArn",
            serde_json::json!({ "CreatorRequestId": "first-call", "EncryptionKeyArn": key })
                .to_string(),
        ),
        (
            "a create with tags",
            serde_json::json!({ "BackupVaultTags": { "team": "platform" } }).to_string(),
        ),
    ];

    for (index, (case, first)) in cases.iter().enumerate() {
        let router = router();
        let name = format!("dup-vault-{index}");
        let created = create_vault(&router, &name, first).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup create should succeed, got {} {:?}",
            created.status, created.body
        );

        // A second create with DIFFERENT parameters is not a retry of the
        // first, so it collides.
        let answer = create_vault(&router, &name, r#"{"CreatorRequestId":"second-call"}"#).await;
        assert_eq!(
            (
                answer.status,
                answer.errortype.as_str(),
                answer.member("Code").as_str(),
                answer.member("Message").as_str(),
            ),
            (
                400,
                "AlreadyExistsException",
                "ERROR_1101",
                "Backup vault with the same name already exists",
            ),
            "case {case:?}: expected 400 AlreadyExistsException with Code=ERROR_1101, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            (
                answer.member("Arn").as_str(),
                answer.member("CreatorRequestId").as_str()
            ),
            ("<unset>", "<unset>"),
            "case {case:?}: the refusal must disclose neither Arn nor CreatorRequestId, got {:?}",
            answer.body
        );
    }
}

/// A `MaxResults` outside the model's `@range(min: 1, max: 1000)` is this
/// service's `InvalidParameterValueException` — `Code` `ERROR_3607`, with a
/// `Message` — on every operation that takes one.
///
/// The rows walk off each published bound and well past it; the published
/// bounds themselves must NOT be refused that way (an unimplemented operation
/// answers 501 for them, an implemented one 200 — neither is this refusal).
#[tokio::test]
async fn a_max_results_outside_the_published_range_is_the_modelled_refusal() {
    let router = router();
    create_vault(&router, "code-vault", "{}").await;

    let rejected = [
        ("one below the published minimum", "0"),
        ("negative", "-1"),
        ("far below", "-1000"),
        ("one above the published maximum", "1001"),
        ("far above", "100000"),
    ];
    for (operation, path, parameter) in MAX_RESULTS_ROUTES {
        for (case, value) in rejected {
            let answer = call(
                &router,
                "GET",
                &format!("{path}?{parameter}={value}"),
                None,
            )
            .await;
            assert_eq!(
                (
                    answer.status,
                    answer.errortype.as_str(),
                    answer.member("Code").as_str(),
                    answer.member("Message").as_str(),
                ),
                (
                    400,
                    "InvalidParameterValueException",
                    "ERROR_3607",
                    "Max Results is invalid",
                ),
                "case {case:?} ({operation} {parameter}={value}): expected 400 \
                 InvalidParameterValueException with Code=ERROR_3607 and a Message, got {} {:?}",
                answer.status,
                answer.body
            );
        }

        // Non-vacuity: each published bound is admitted, so the refusal above
        // is the range's answer and not the route's.
        for (case, value) in [
            ("the published minimum", "1"),
            ("the published maximum", "1000"),
        ] {
            let answer = call(
                &router,
                "GET",
                &format!("{path}?{parameter}={value}"),
                None,
            )
            .await;
            assert!(
                answer.status == 200 || answer.status == 501,
                "case {case:?} ({operation} {parameter}={value}): a page size the model \
                 admits must be served (200) or unimplemented (501), got {} {:?}",
                answer.status,
                answer.body
            );
        }
    }
}

/// An `AggregationPeriod` the model's enum does not list is this service's
/// `InvalidParameterValueException`, with a `Message` BUILT FROM THE REQUEST:
/// the rejected value, then the values the model admits.
///
/// The rows vary the rejected spelling (unknown token, wrong case, a near-miss
/// of each admitted value); every admitted value is then exercised, and none of
/// them is refused.
#[tokio::test]
async fn an_unknown_aggregation_period_names_the_value_and_the_admitted_set() {
    let router = router();
    let admitted = ["ONE_DAY", "SEVEN_DAYS", "FOURTEEN_DAYS"];
    let expected_tail = format!(
        "Input aggregation period must be within {}",
        admitted.join(", ")
    );

    let rejected = [
        ("an unknown token", "NOT_A_PERIOD"),
        ("the lower-case spelling of an admitted value", "one_day"),
        ("a plausible period the enum omits", "THIRTY_DAYS"),
        ("a near-miss of ONE_DAY", "ONE_DAYS"),
        ("a near-miss of FOURTEEN_DAYS", "FOURTEEN_DAY"),
        ("a numeric period", "14"),
    ];
    for (operation, path) in AGGREGATION_ROUTES {
        for (case, value) in rejected {
            let answer = call(
                &router,
                "GET",
                &format!("{path}?AggregationPeriod={value}"),
                None,
            )
            .await;
            assert_eq!(
                (
                    answer.status,
                    answer.errortype.as_str(),
                    answer.member("Message").as_str(),
                ),
                (
                    400,
                    "InvalidParameterValueException",
                    format!("Invalid input aggregation id {value}. {expected_tail}").as_str(),
                ),
                "case {case:?} ({operation} AggregationPeriod={value}): expected 400 \
                 InvalidParameterValueException naming the value and the admitted set, \
                 got {} {:?}",
                answer.status,
                answer.body
            );
        }

        // Non-vacuity: every value the enum lists is admitted, so what is
        // refused above is the value and not the parameter.
        for value in admitted {
            let answer = call(
                &router,
                "GET",
                &format!("{path}?AggregationPeriod={value}"),
                None,
            )
            .await;
            assert_eq!(
                answer.status, 501,
                "case {value:?} ({operation}): an admitted period must reach the \
                 operation, which is still a scaffold (501), got {} {:?}",
                answer.status, answer.body
            );
        }
    }
}
