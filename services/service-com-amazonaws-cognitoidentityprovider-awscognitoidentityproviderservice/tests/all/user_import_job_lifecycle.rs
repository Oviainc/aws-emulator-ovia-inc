// SPDX-License-Identifier: CC0-1.0

//! User import jobs over the wire: `CreateUserImportJob` stores a job in the
//! pool it names, and every later `DescribeUserImportJob`,
//! `ListUserImportJobs`, `StartUserImportJob` and `StopUserImportJob` reads and
//! moves the STORED job, driven through the router exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rules under test:
//!
//! * a created job is stored under a minted `JobId` and renders `JobName`,
//!   `UserPoolId`, `CloudWatchLogsRoleArn` and `PasswordHashingAlgorithm`
//!   exactly as submitted, with the three user counters at zero, no CSV result
//!   invented, and a `PreSignedUrl` naming this job's CSV on the endpoint the
//!   create arrived at (and none at all when the request named no endpoint);
//! * a job's `Status` is computed from the stored lifecycle instants and the
//!   clock: `Created` until started, `Pending` until the import is due,
//!   `InProgress` from then on, `Stopping` until the stop is due and `Stopped`
//!   from then on, and `Expired` for a job left unstarted past its window;
//! * only a job waiting to be started, with the CSV file uploaded to its
//!   `PreSignedUrl`, can be started and only a running job can be stopped, so
//!   any other status is a `PreconditionNotMetException` that leaves the stored
//!   job alone;
//! * a read renders the stored job and changes nothing a later caller sees.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use cognito_idp::state;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Long enough that the clock has made a started job's import due.
const PAST_THE_START: Duration =
    state::JOB_START_SPAN.saturating_add(Duration::from_millis(250));

/// Long enough that the clock has made a stopping job's stop due.
const PAST_THE_STOP: Duration = state::JOB_STOP_SPAN.saturating_add(Duration::from_millis(250));

/// A CloudWatch Logs role in the account every emulated resource reports.
const LOCAL_ROLE: &str = "arn:aws:iam::000000000000:role/cognito-import";

/// A CloudWatch Logs role in ANOTHER account, which this operation accepts: the
/// role is only read where the import logs, so a well-formed ARN is stored and
/// rendered back rather than verified here.
const FOREIGN_ROLE: &str = "arn:aws:iam::123456789012:role/service-role/untrusted-import";

/// The endpoint host the suite's requests address, which a locator this service
/// hands out has to name.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// One emulator with just this service mounted, beside the engine its state
/// lives in.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    (router, engine)
}

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    router_and_engine().0
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// The stored job as `DescribeUserImportJob` renders it.
async fn describe(router: &MultiServiceRouter, user_pool_id: &str, job_id: &str) -> Value {
    let read = ok(
        router,
        "DescribeUserImportJob",
        &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
    )
    .await;
    read["UserImportJob"].clone()
}

/// The `Status` of the stored job.
async fn status(router: &MultiServiceRouter, user_pool_id: &str, job_id: &str) -> String {
    describe(router, user_pool_id, job_id)
        .await
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or("<no Status>")
        .to_string()
}

/// A CSV file with the pool's header row and one user, which a client uploads to
/// a job's `PreSignedUrl` before the import is started.
const CSV_FILE: &str = "cognito:username,email\nalice,alice@example.com\n";

/// Upload [`CSV_FILE`] to `job`'s `PreSignedUrl` — the ANONYMOUS `PUT` a client
/// writes the file with, carrying no credential and no `X-Amz-Target`, only the
/// path of the locator this service handed out — and require it to be accepted.
///
/// "Starts importing users from a CSV file" (`StartUserImportJob`), so a job has
/// nothing to import until its upload target carries one.
async fn upload_csv(router: &MultiServiceRouter, case: &str, job: &Value) {
    let url = job["PreSignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a created job carries its locator: {job}"));
    let path = url
        .strip_prefix(&format!("https://{ENDPOINT_HOST}"))
        .unwrap_or_else(|| panic!("case {case:?}: {url} must be a locator on {ENDPOINT_HOST}"));
    let request = http::Request::builder()
        .method("PUT")
        .uri(path)
        .header("host", ENDPOINT_HOST)
        .header("content-type", "text/csv")
        .body(Bytes::from(CSV_FILE))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?}: the upload target {path} must accept the CSV, got {}",
        response.status()
    );
}

/// One row of the `CreateUserImportJob` table: the job name, the logging role
/// and the password hashing algorithm the request sets. The rows differ along
/// what the rule generalises over — the form and length of the name, the account
/// and path of the role ARN, and which hashing algorithm (if any) is submitted.
struct CreateCase {
    case: &'static str,
    job_name: String,
    role_arn: &'static str,
    /// `None` leaves `PasswordHashingAlgorithm` OUT of the request.
    algorithm: Option<&'static str>,
}

impl CreateCase {
    /// This case's `CreateUserImportJob` request for `user_pool_id`: an unset
    /// `PasswordHashingAlgorithm` is left OUT of the request, not sent as null.
    fn request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("JobName".to_string(), json!(self.job_name));
        body.insert(
            "CloudWatchLogsRoleArn".to_string(),
            json!(self.role_arn),
        );
        if let Some(algorithm) = self.algorithm {
            body.insert("PasswordHashingAlgorithm".to_string(), json!(algorithm));
        }
        Value::Object(body)
    }
}

/// The create table: one row per shape of request the rule spans. The
/// algorithms are the modelled `PasswordHashingAlgorithmType` values
/// (`smithy_query` shape `PasswordHashingAlgorithmType`), each tested, plus a
/// request that submits none.
fn create_cases() -> Vec<CreateCase> {
    let mut cases = vec![
        CreateCase {
            case: "the shortest job name the model admits",
            job_name: "j".to_string(),
            role_arn: LOCAL_ROLE,
            algorithm: None,
        },
        CreateCase {
            case: "a job name with the punctuation the pattern admits",
            job_name: "import users+v2=prod,final.run@now-1".to_string(),
            role_arn: LOCAL_ROLE,
            algorithm: Some("BCRYPT"),
        },
        CreateCase {
            case: "the longest job name the model admits",
            job_name: "n".repeat(128),
            role_arn: LOCAL_ROLE,
            algorithm: Some("SCRYPT"),
        },
        CreateCase {
            case: "a role in another account, which is accepted unverified",
            job_name: "foreign-role-job".to_string(),
            role_arn: FOREIGN_ROLE,
            algorithm: Some("ARGON2ID"),
        },
    ];
    // Every modelled algorithm is exercised, including the one no row above
    // names, so the stored value is not read off a single case.
    for algorithm in cognito_idp::ops::types::PASSWORD_HASHING_ALGORITHM_TYPE_VALUES {
        if cases
            .iter()
            .all(|row| row.algorithm != Some(*algorithm))
        {
            cases.push(CreateCase {
                case: "a modelled hashing algorithm no other row submits",
                job_name: format!("algorithm-{algorithm}"),
                role_arn: LOCAL_ROLE,
                algorithm: Some(algorithm),
            });
        }
    }
    cases
}

#[tokio::test]
async fn a_created_job_is_stored_and_read_back_as_created() {
    let router = router();
    let user_pool_id = pool(&router, "import-create-pool").await;

    let mut seen_ids: Vec<String> = Vec::new();
    for row in create_cases() {
        let case = row.case;
        let created = ok(
            &router,
            "CreateUserImportJob",
            &row.request(&user_pool_id),
        )
        .await;
        let job = created["UserImportJob"].clone();

        let job_id = job["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a job must carry a minted JobId, got {job}"))
            .to_string();
        // `UserImportJobIdType`: pattern ^import-[0-9a-zA-Z-]+$, length 1..55.
        // Amazon Cognito mints the id as `import-` followed by a FIXED-WIDTH
        // token of mixed-case alphanumerics (`"JobId": "import-TZqNQvDRnW"` in
        // the API reference's import-job examples), so the token's width and
        // charset are asserted, not just the prefix: a client that persists or
        // validates the id against the service's own form rejects another shape.
        let token = job_id.strip_prefix("import-").unwrap_or_else(|| {
            panic!("case {case:?}: expected a JobId of the modelled form import-<token>, got {job_id:?}")
        });
        assert_eq!(
            token.len(),
            10,
            "case {case:?}: expected the minted token to be 10 characters wide, got {job_id:?}"
        );
        assert!(
            token.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: expected the minted token to carry only mixed-case alphanumerics, got {job_id:?}"
        );
        assert!(
            (1..=55).contains(&job_id.len())
                && job_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {case:?}: expected a JobId inside the modelled length and pattern, got {job_id:?}"
        );
        assert!(
            !seen_ids.contains(&job_id),
            "case {case:?}: each job must get an id of its own, got the repeat {job_id:?}"
        );
        seen_ids.push(job_id.clone());

        assert_eq!(
            job["JobName"],
            json!(row.job_name),
            "case {case:?}: the stored JobName must be the submitted one, got {job}"
        );
        assert_eq!(
            job["UserPoolId"],
            json!(user_pool_id),
            "case {case:?}: the job must name the pool it was created in, got {job}"
        );
        assert_eq!(
            job["CloudWatchLogsRoleArn"],
            json!(row.role_arn),
            "case {case:?}: the stored role ARN must be the submitted one, got {job}"
        );
        assert_eq!(
            job["Status"],
            json!("Created"),
            "case {case:?}: a job that was not started is Created, got {job}"
        );
        match row.algorithm {
            Some(algorithm) => assert_eq!(
                job["PasswordHashingAlgorithm"],
                json!(algorithm),
                "case {case:?}: the stored algorithm must be the submitted one, got {job}"
            ),
            None => assert!(
                job.get("PasswordHashingAlgorithm").is_none(),
                "case {case:?}: a job created without an algorithm reports none, got {job}"
            ),
        }
        for counter in ["ImportedUsers", "SkippedUsers", "FailedUsers"] {
            assert_eq!(
                job[counter],
                json!(0),
                "case {case:?}: nothing was imported, so {counter} is zero, got {job}"
            );
        }
        assert!(
            job.get("CreationDate").is_some(),
            "case {case:?}: a created job carries its CreationDate, got {job}"
        );
        assert_eq!(
            job["PreSignedUrl"],
            json!(format!(
                "https://{ENDPOINT_HOST}/user-import-jobs/{user_pool_id}/{job_id}"
            )),
            "case {case:?}: the CSV locator must name this job on the endpoint the \
             create arrived at, got {job}"
        );
        for absent in ["StartDate", "CompletionDate", "CompletionMessage"] {
            assert!(
                job.get(absent).is_none(),
                "case {case:?}: a job that never ran reports no {absent}, got {job}"
            );
        }

        // The round trip: the reader renders the STORED job, not the request.
        let read = describe(&router, &user_pool_id, &job_id).await;
        assert_eq!(
            read, job,
            "case {case:?}: DescribeUserImportJob must render the stored job, got {read}"
        );
        // A read changes nothing a later caller sees.
        let again = describe(&router, &user_pool_id, &job_id).await;
        assert_eq!(
            again, read,
            "case {case:?}: a read must not change what the next read sees, got {again}"
        );
    }

    // Every created job is in the pool's listing, rendered exactly as the
    // reader renders it.
    let listed = ok(
        &router,
        "ListUserImportJobs",
        &json!({ "UserPoolId": user_pool_id, "MaxResults": 60 }),
    )
    .await;
    let jobs = listed["UserImportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("the listing must carry UserImportJobs, got {listed}"))
        .clone();
    assert_eq!(
        jobs.len(),
        seen_ids.len(),
        "case \"every created job is listed\": expected {} jobs, got {listed}",
        seen_ids.len()
    );
    for job_id in &seen_ids {
        let read = describe(&router, &user_pool_id, job_id).await;
        assert!(
            jobs.contains(&read),
            "case \"{job_id} is listed as it is described\": expected {read} in {listed}"
        );
    }
}

#[tokio::test]
async fn a_job_reaches_the_status_the_clock_owes_it() {
    // Each row: when the stop is asked for, and the statuses the job passes
    // through. A stop is accepted both before and after the import became due,
    // so the rule is not read off one ordering.
    for (case, stop_once_in_progress) in [
        ("stopped while the import is still pending", false),
        ("stopped once the import is in progress", true),
    ] {
        let router = router();
        let user_pool_id = pool(&router, "import-status-pool").await;
        let created = ok(
            &router,
            "CreateUserImportJob",
            &json!({
                "UserPoolId": user_pool_id,
                "JobName": "status-job",
                "CloudWatchLogsRoleArn": LOCAL_ROLE,
            }),
        )
        .await;
        let job_id = created["UserImportJob"]["JobId"]
            .as_str()
            .expect("a minted job id")
            .to_string();
        // A job imports the CSV uploaded to its target, so the file is written
        // there before the import is started.
        upload_csv(&router, case, &created["UserImportJob"]).await;

        let started = ok(
            &router,
            "StartUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await;
        // The span is never zero, so a caller reading straight after the start
        // still sees the transient status.
        assert_eq!(
            started["UserImportJob"]["Status"],
            json!("Pending"),
            "case {case:?}: a started job is Pending before the import is due, got {started}"
        );
        assert!(
            started["UserImportJob"].get("StartDate").is_some(),
            "case {case:?}: a started job carries its StartDate, got {started}"
        );
        let pending = status(&router, &user_pool_id, &job_id).await;
        assert_eq!(
            pending, "Pending",
            "case {case:?}: expected Pending from the reader, got {pending}"
        );

        if stop_once_in_progress {
            tokio::time::sleep(PAST_THE_START).await;
            let in_progress = status(&router, &user_pool_id, &job_id).await;
            assert_eq!(
                in_progress, "InProgress",
                "case {case:?}: expected InProgress once the clock owes it, got {in_progress}"
            );
        }

        let stopped = ok(
            &router,
            "StopUserImportJob",
            &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
        )
        .await;
        assert_eq!(
            stopped["UserImportJob"]["Status"],
            json!("Stopping"),
            "case {case:?}: a stopped job is Stopping before the stop is due, got {stopped}"
        );
        assert!(
            stopped["UserImportJob"].get("CompletionDate").is_none(),
            "case {case:?}: a job that has not stopped yet reports no CompletionDate, got {stopped}"
        );

        tokio::time::sleep(PAST_THE_STOP).await;
        let final_read = describe(&router, &user_pool_id, &job_id).await;
        assert_eq!(
            final_read["Status"],
            json!("Stopped"),
            "case {case:?}: expected Stopped once the clock owes it, got {final_read}"
        );
        assert!(
            final_read.get("CompletionDate").is_some(),
            "case {case:?}: a stopped job carries its CompletionDate, got {final_read}"
        );
        assert!(
            final_read.get("StartDate").is_some(),
            "case {case:?}: a stopped job keeps the StartDate it was given, got {final_read}"
        );
        for counter in ["ImportedUsers", "SkippedUsers", "FailedUsers"] {
            assert_eq!(
                final_read[counter],
                json!(0),
                "case {case:?}: no CSV was imported, so {counter} is zero, got {final_read}"
            );
        }
    }
}

#[tokio::test]
async fn only_a_waiting_job_starts_and_only_a_running_job_stops() {
    // Each row: the operation, the status the stored job is in when it is
    // called, the HTTP status and the wire error code expected.
    let cases: Vec<(&str, &str, &str, u16, &str)> = vec![
        ("StartUserImportJob", "Pending", "pending", 400, "PreconditionNotMetException"),
        (
            "StartUserImportJob",
            "InProgress",
            "in progress",
            400,
            "PreconditionNotMetException",
        ),
        ("StartUserImportJob", "Stopping", "stopping", 400, "PreconditionNotMetException"),
        ("StopUserImportJob", "Created", "created", 400, "PreconditionNotMetException"),
        ("StopUserImportJob", "Stopping", "stopping", 400, "PreconditionNotMetException"),
    ];

    for (operation, reached, label, expected_status, expected_code) in cases {
        let router = router();
        let user_pool_id = pool(&router, "import-precondition-pool").await;
        let created = ok(
            &router,
            "CreateUserImportJob",
            &json!({
                "UserPoolId": user_pool_id,
                "JobName": format!("job-{label}"),
                "CloudWatchLogsRoleArn": LOCAL_ROLE,
            }),
        )
        .await;
        let job_id = created["UserImportJob"]["JobId"]
            .as_str()
            .expect("a minted job id")
            .to_string();
        // Every row that drives the job past `Created` starts it, which needs
        // the CSV on its upload target.
        upload_csv(&router, label, &created["UserImportJob"]).await;
        let start = json!({ "UserPoolId": user_pool_id, "JobId": job_id });

        // Drive the stored job into the status this row is about.
        match reached {
            "Created" => {}
            "Pending" => {
                ok(&router, "StartUserImportJob", &start).await;
            }
            "InProgress" => {
                ok(&router, "StartUserImportJob", &start).await;
                tokio::time::sleep(PAST_THE_START).await;
            }
            "Stopping" => {
                ok(&router, "StartUserImportJob", &start).await;
                ok(&router, "StopUserImportJob", &start).await;
            }
            other => panic!("case {label:?}: unreachable setup status {other}"),
        }
        let before = describe(&router, &user_pool_id, &job_id).await;
        assert_eq!(
            before["Status"],
            json!(reached),
            "case {label:?}: the setup must reach {reached}, got {before}"
        );

        let (actual_status, payload) = call(&router, operation, &start).await;
        assert_eq!(
            actual_status, expected_status,
            "case {label:?}: expected HTTP {expected_status} from {operation}, got {actual_status} with {payload}"
        );
        let actual_code = error_code(&payload);
        assert!(
            actual_code.ends_with(expected_code),
            "case {label:?}: expected {expected_code} from {operation}, got {actual_code} in {payload}"
        );

        // The refusal left the stored job exactly as it was.
        let after = describe(&router, &user_pool_id, &job_id).await;
        assert_eq!(
            after["Status"],
            json!(reached),
            "case {label:?}: a refused {operation} must not move the stored job, got {after}"
        );
        assert_eq!(
            after["StartDate"], before["StartDate"],
            "case {label:?}: a refused {operation} must not restamp the job, got {after}"
        );
    }
}

#[tokio::test]
async fn a_job_or_pool_that_is_not_stored_is_a_400() {
    let router = router();
    let user_pool_id = pool(&router, "import-400-pool").await;
    let created = ok(
        &router,
        "CreateUserImportJob",
        &json!({
            "UserPoolId": user_pool_id,
            "JobName": "stored-job",
            "CloudWatchLogsRoleArn": LOCAL_ROLE,
        }),
    )
    .await;
    let job_id = created["UserImportJob"]["JobId"]
        .as_str()
        .expect("a minted job id")
        .to_string();
    // A pool id of the modelled form that no create ever minted.
    let absent_pool = "us-east-1_absentpool";
    // A job id of the modelled form that no create ever minted.
    let absent_job = "import-00000000-0000-4000-8000-000000000000";

    for operation in [
        "CreateUserImportJob",
        "DescribeUserImportJob",
        "StartUserImportJob",
        "StopUserImportJob",
        "ListUserImportJobs",
    ] {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(absent_pool));
        match operation {
            "CreateUserImportJob" => {
                body.insert("JobName".to_string(), json!("job"));
                body.insert("CloudWatchLogsRoleArn".to_string(), json!(LOCAL_ROLE));
            }
            "ListUserImportJobs" => {
                body.insert("MaxResults".to_string(), json!(10));
            }
            _ => {
                body.insert("JobId".to_string(), json!(job_id));
            }
        }
        let (status, payload) = call(&router, operation, &Value::Object(body)).await;
        assert_eq!(
            status, 400,
            "case \"{operation} on an absent user pool\": expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case \"{operation} on an absent user pool\": expected ResourceNotFoundException, got {code} in {payload}"
        );
    }

    for operation in [
        "DescribeUserImportJob",
        "StartUserImportJob",
        "StopUserImportJob",
    ] {
        let (status, payload) = call(
            &router,
            operation,
            &json!({ "UserPoolId": user_pool_id, "JobId": absent_job }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"{operation} on an absent job\": expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("ResourceNotFoundException"),
            "case \"{operation} on an absent job\": expected ResourceNotFoundException, got {code} in {payload}"
        );
    }
}

#[tokio::test]
async fn a_listing_pages_the_stored_jobs_and_refuses_a_foreign_token() {
    let router = router();
    let user_pool_id = pool(&router, "import-paging-pool").await;
    let other_pool = pool(&router, "import-paging-other").await;

    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        let job = ok(
            &router,
            "CreateUserImportJob",
            &json!({
                "UserPoolId": user_pool_id,
                "JobName": format!("paged-job-{index}"),
                "CloudWatchLogsRoleArn": LOCAL_ROLE,
            }),
        )
        .await;
        created.push(
            job["UserImportJob"]["JobId"]
                .as_str()
                .expect("a minted job id")
                .to_string(),
        );
    }
    created.sort();

    // Each row: the page size asked for, and how many pages the five stored
    // jobs take at that size.
    for (max_results, expected_pages) in [(1, 5), (2, 3), (5, 1), (60, 1)] {
        let mut seen: Vec<String> = Vec::new();
        let mut pages = 0;
        let mut token: Option<String> = None;
        loop {
            let mut body = Map::new();
            body.insert("UserPoolId".to_string(), json!(user_pool_id));
            body.insert("MaxResults".to_string(), json!(max_results));
            if let Some(token) = &token {
                body.insert("PaginationToken".to_string(), json!(token));
            }
            let page = ok(&router, "ListUserImportJobs", &Value::Object(body)).await;
            pages += 1;
            let jobs = page["UserImportJobs"]
                .as_array()
                .unwrap_or_else(|| panic!("case {max_results}: a page carries UserImportJobs, got {page}"))
                .clone();
            assert!(
                jobs.len() <= max_results as usize,
                "case {max_results}: a page must hold at most {max_results} jobs, got {page}"
            );
            for job in &jobs {
                let job_id = job["JobId"].as_str().expect("a listed job id").to_string();
                assert!(
                    !seen.contains(&job_id),
                    "case {max_results}: {job_id} must appear on one page only, got {page}"
                );
                seen.push(job_id);
            }
            token = page
                .get("PaginationToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= 5,
                "case {max_results}: the listing must terminate, got {pages} pages"
            );
        }
        assert_eq!(
            pages, expected_pages,
            "case {max_results}: expected {expected_pages} page(s), got {pages}"
        );
        seen.sort();
        assert_eq!(
            seen, created,
            "case {max_results}: the pages together must serve every stored job, got {seen:?}"
        );
    }

    // A token one pool's listing minted names no page of another pool's.
    let first = ok(
        &router,
        "ListUserImportJobs",
        &json!({ "UserPoolId": user_pool_id, "MaxResults": 1 }),
    )
    .await;
    let token = first["PaginationToken"]
        .as_str()
        .expect("a first page of five leaves a token")
        .to_string();
    for (case, body) in [
        (
            "a token from another pool's listing",
            json!({ "UserPoolId": other_pool, "MaxResults": 1, "PaginationToken": token }),
        ),
        (
            "a token this service never minted",
            json!({ "UserPoolId": user_pool_id, "MaxResults": 1, "PaginationToken": "not-a-token" }),
        ),
    ] {
        let (status, payload) = call(&router, "ListUserImportJobs", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert!(
            code.ends_with("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {code} in {payload}"
        );
    }

    // The other pool's listing is present and empty rather than absent.
    let empty = ok(
        &router,
        "ListUserImportJobs",
        &json!({ "UserPoolId": other_pool, "MaxResults": 10 }),
    )
    .await;
    assert_eq!(
        empty["UserImportJobs"],
        json!([]),
        "case \"a pool with no import job\": expected an empty list, got {empty}"
    );
}

/// A locator is minted only where the create named an endpoint to point at, and
/// it is STORED with the job rather than recomputed per read.
#[tokio::test]
async fn a_create_that_names_no_endpoint_gets_no_csv_locator() {
    let router = router();
    let user_pool_id = pool(&router, "import-no-endpoint-pool").await;

    // A request with no `Host` header and no authority in its URI names no
    // endpoint for a locator to point at; its credential scope still routes it.
    let body = json!({
        "UserPoolId": user_pool_id,
        "JobName": "no-endpoint-job",
        "CloudWatchLogsRoleArn": LOCAL_ROLE,
    });
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            "AWSCognitoIdentityProviderService.CreateUserImportJob",
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case \"a create naming no endpoint\": expected HTTP 200, got {:?}",
        String::from_utf8_lossy(response.body())
    );
    let payload: Value =
        serde_json::from_slice(response.body()).expect("the response body is JSON");
    let job = payload["UserImportJob"].clone();
    assert!(
        job.get("PreSignedUrl").is_none(),
        "case \"a create naming no endpoint\": no locator is invented, got {job}"
    );

    // The locator is STORED with the job, not recomputed per read: a later read
    // that does name an endpoint still reports none.
    let job_id = job["JobId"].as_str().expect("a minted job id").to_string();
    let read = describe(&router, &user_pool_id, &job_id).await;
    assert!(
        read.get("PreSignedUrl").is_none(),
        "case \"reading a job minted without a locator\": expected none, got {read}"
    );
}

/// One row of the unstarted-expiry table.
#[derive(Debug)]
struct ExpiryCase {
    case: &'static str,
    /// How long before `now` the job was created.
    created_ago: Duration,
    /// Whether the job was started before the question is asked.
    started: bool,
    expected: &'static str,
}

#[tokio::test]
async fn an_unstarted_job_expires_only_once_its_window_is_past() {
    let now = SystemTime::now();
    let window = state::JOB_EXPIRY_SPAN;
    let cases = vec![
        ExpiryCase {
            case: "created at this instant",
            created_ago: Duration::ZERO,
            started: false,
            expected: "Created",
        },
        ExpiryCase {
            case: "one second before the window ends",
            created_ago: window - Duration::from_secs(1),
            started: false,
            expected: "Created",
        },
        ExpiryCase {
            case: "exactly at the end of the window",
            created_ago: window,
            started: false,
            expected: "Expired",
        },
        ExpiryCase {
            case: "one second past the end of the window",
            created_ago: window + Duration::from_secs(1),
            started: false,
            expected: "Expired",
        },
        ExpiryCase {
            case: "long past the window",
            created_ago: 30 * window,
            started: false,
            expected: "Expired",
        },
        ExpiryCase {
            case: "started before the window ended, read long after",
            created_ago: 30 * window,
            started: true,
            expected: "InProgress",
        },
    ];

    for row in cases {
        let case = row.case;
        let mut job = state::UserImportJob::new(
            state::new_user_import_job_id(),
            "expiry-job".to_string(),
            "us-east-1_expiry".to_string(),
            LOCAL_ROLE.to_string(),
            None,
            None,
            now - row.created_ago,
        );
        if row.started {
            job.start(now - row.created_ago);
        }
        let actual = job.status(now);
        assert_eq!(
            actual, row.expected,
            "case {case:?}: expected status {}, got {actual}",
            row.expected
        );
    }

    // The same rule over the wire: a job whose STORED creation instant is past
    // the window reads as `Expired` and cannot be started, since "the job can't
    // be started" once it has expired.
    let (router, engine) = router_and_engine();
    let user_pool_id = pool(&router, "import-expiry-pool").await;
    let created = ok(
        &router,
        "CreateUserImportJob",
        &json!({
            "UserPoolId": user_pool_id,
            "JobName": "expiring-job",
            "CloudWatchLogsRoleArn": LOCAL_ROLE,
        }),
    )
    .await;
    let job_id = created["UserImportJob"]["JobId"]
        .as_str()
        .expect("a minted job id")
        .to_string();

    // Rewind the STORED creation instant: the window is a full day, so no test
    // can wait it out.
    {
        let state = state::service_state(&*engine).expect("this service's state");
        let mut guard = state.lock();
        let job = guard
            .pool_mut(&user_pool_id)
            .and_then(|pool| pool.user_import_jobs.get_mut(&job_id))
            .expect("the stored job");
        job.creation_date = SystemTime::now() - (state::JOB_EXPIRY_SPAN + Duration::from_secs(60));
    }

    let expired = status(&router, &user_pool_id, &job_id).await;
    assert_eq!(
        expired, "Expired",
        "case \"a job left unstarted past its window\": expected Expired, got {expired}"
    );
    let (status_code, payload) = call(
        &router,
        "StartUserImportJob",
        &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case \"starting an expired job\": expected HTTP 400, got {status_code} with {payload}"
    );
    let code = error_code(&payload);
    assert!(
        code.ends_with("PreconditionNotMetException"),
        "case \"starting an expired job\": expected PreconditionNotMetException, got {code} in {payload}"
    );
}
