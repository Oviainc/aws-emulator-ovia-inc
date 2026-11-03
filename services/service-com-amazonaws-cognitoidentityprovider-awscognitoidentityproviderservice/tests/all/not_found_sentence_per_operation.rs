// SPDX-License-Identifier: CC0-1.0

//! Which sentence a refusal of an unresolved lookup carries, driven through the
//! router exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! A refusal's text belongs to the OPERATION that answers it rather than to the
//! resource kind, and the operations that resolve the SAME resource by the same
//! request members word it the same way. The rules under test:
//!
//! * The three operations addressed by the pair `UserPoolId` + `JobId` —
//!   `DescribeUserImportJob`, `StartUserImportJob` and `StopUserImportJob` —
//!   report a `JobId` the named pool holds no user import job for with one
//!   sentence: the fixed `The import job ` and ` does not exist`, carrying no
//!   trailing period, around the `JobId` the request asked for.
//! * `StopUserImportJob` refuses a job that is not running by reporting the
//!   status the STORE and the clock put that job in, with no job id in the text.
//! * An operation that resolves ONE app client by the pair `UserPoolId` +
//!   `ClientId` reports the unresolved app client rather than the pool: a
//!   `UserPoolId` no create minted fails that same lookup, and the sentence
//!   names both ids the request carried, as it does for an id that names an app
//!   client of another pool. Only a STORED pool that holds no such app client is
//!   reported with the sentence the asking operation words for an id that names
//!   none.
//! * A `UserPoolId` no stored pool answers to carries the sentence its own
//!   operation words: `CreateUserPoolDomain` and `CreateUserPoolReplica` join
//!   `UserPool with ID: ` to `does not exist.` with no separator, as
//!   `SetLogDeliveryConfiguration` does. That wording is asserted beside every
//!   other operation's in `per_operation_refusal_wording.rs`; what is asserted
//!   here is that such a refusal configures nothing and leaves a stored pool
//!   configurable.
//!
//! The rows of each table differ along what its rule generalises over — the form
//! and length of the id asked for, the status the stored job is in, the way the
//! lookup failed and the operation asking — and every assertion names its case
//! and reports the actual value. Every refusal is followed by a read proving it
//! stored nothing.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use cognito_idp::state;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The endpoint every request of this suite arrives at.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A CloudWatch Logs role a user import job logs its results to.
const IMPORT_ROLE: &str = "arn:aws:iam::000000000000:role/cognito-import";

/// A CSV file with the pool's header row and one user, which a client uploads to
/// a job's `PreSignedUrl` before the import is started.
const CSV_FILE: &str = "cognito:username,email\nalice,alice@example.com\n";

/// Long enough that the clock has made a stopping job's stop due.
const PAST_THE_STOP: Duration = state::JOB_STOP_SPAN.saturating_add(Duration::from_millis(250));

/// Ids satisfying `UserPoolIdType`'s pattern `^[\w-]+_[0-9a-zA-Z]+$`
/// (`smithy_query(query="shape",shape="UserPoolIdType")`) that no create minted,
/// so a lookup is what fails rather than a modelled constraint: two Regions, a
/// numeric suffix and an upper-case one.
const ABSENT_POOLS: [&str; 3] = [
    "us-east-1_absentpool",
    "eu-west-2_0000000000",
    "us-west-2_ZZZZZZZZZ",
];

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
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
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// Call `operation`, requiring the refusal `code`, and return its message.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
) -> String {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, code),
        "case {case:?}: expected a 400 {code}, got {status} with {payload}"
    );
    error_message(&payload).to_string()
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body, under either casing the protocol admits.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool named `pool_name` and return its minted id.
async fn pool(router: &MultiServiceRouter, case: &str, pool_name: &str) -> String {
    let created = ok(
        router,
        case,
        "CreateUserPool",
        &json!({ "PoolName": pool_name }),
    )
    .await;
    created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted pool id, got {created}"))
        .to_string()
}

/// Create one app client of `user_pool_id` and return its minted `ClientId`.
async fn client(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    client_name: &str,
) -> String {
    let created = ok(
        router,
        case,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": client_name,
            "GenerateSecret": true,
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted client id, got {created}"))
        .to_string()
}

/// Create a user import job on `user_pool_id`, upload [`CSV_FILE`] to the
/// locator it hands out so the job can be started, and return its `JobId`.
async fn import_job(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    job_name: &str,
) -> String {
    let created = ok(
        router,
        case,
        "CreateUserImportJob",
        &json!({
            "UserPoolId": user_pool_id,
            "JobName": job_name,
            "CloudWatchLogsRoleArn": IMPORT_ROLE,
        }),
    )
    .await;
    let job = &created["UserImportJob"];
    let url = job["PreSignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a created job carries its locator: {job}"));
    let path = url
        .strip_prefix(&format!("https://{ENDPOINT_HOST}"))
        .unwrap_or_else(|| panic!("case {case:?}: {url} must be a locator on {ENDPOINT_HOST}"));
    // The ANONYMOUS `PUT` a client writes the CSV file with: no credential and
    // no `X-Amz-Target`, only the path of the locator this service handed out.
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
    job["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a minted job id, got {job}"))
        .to_string()
}

/// The `Status` the stored job `job_id` of `user_pool_id` is in.
async fn job_status(
    router: &MultiServiceRouter,
    case: &str,
    user_pool_id: &str,
    job_id: &str,
) -> String {
    let read = ok(
        router,
        case,
        "DescribeUserImportJob",
        &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
    )
    .await;
    read["UserImportJob"]["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a stored job carries a status, got {read}"))
        .to_string()
}

#[tokio::test]
async fn an_unresolved_import_job_is_reported_with_one_sentence_by_every_operation() {
    // The ids span `UserImportJobIdType` — pattern `^import-[0-9a-zA-Z-]+$`,
    // length 1..55 (`smithy_query(query="shape",shape="UserImportJobIdType")`):
    // the shortest the pattern admits, the form a create mints, a mixed-case
    // one, and one at the published length maximum.
    let longest = format!("import-{}", "9".repeat(55 - "import-".len()));
    let asked: &[(&str, String)] = &[
        ("the shortest id the pattern admits", "import-a".to_string()),
        (
            "an id of the minted form",
            "import-00000000-0000-4000-8000-000000000000".to_string(),
        ),
        ("a mixed-case id", "import-Never-Minted-42".to_string()),
        ("an id at the modelled maximum length, 55", longest),
    ];

    // Every operation addressed by the pair `UserPoolId` + `JobId`.
    let operations = ["DescribeUserImportJob", "StartUserImportJob", "StopUserImportJob"];

    let router = router();
    let user_pool_id = pool(&router, "import job lookup", "import-lookup-pool").await;
    // One job that DOES exist, so a refusal cannot be the pool holding none.
    let live = import_job(&router, "import job lookup", &user_pool_id, "live-job").await;

    for (way, job_id) in asked {
        for operation in operations {
            let case = format!("{operation} asked for {way}");
            let message = refusal(
                &router,
                &case,
                operation,
                &json!({ "UserPoolId": user_pool_id, "JobId": job_id }),
                "ResourceNotFoundException",
            )
            .await;
            let expected = format!("The import job {job_id} does not exist");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The sentence quotes no pool, so no caller reads one pool's
            // contents out of another's refusal.
            assert!(
                !message.contains(user_pool_id.as_str()),
                "case {case:?}: the sentence must quote no pool, got {message:?}"
            );
        }
    }

    // Every one of those was a refusal: the stored job is still there, still
    // waiting to be started.
    let status = job_status(&router, "import job lookup", &user_pool_id, &live).await;
    assert_eq!(
        status,
        state::JOB_STATUS_CREATED,
        "the stored job must be untouched by the refusals, got {status:?}"
    );
}

#[tokio::test]
async fn stopping_a_job_that_is_not_running_reports_the_status_the_store_put_it_in() {
    // How the stored job comes to be in a status that is not running, and the
    // status it is then in. `InProgress` and `Pending` are the running ones, so
    // every other reachable status is here.
    let ways: &[(&str, bool, bool)] = &[
        // (case, start it, wait for the stop to fall due)
        ("a job that was never started", false, false),
        ("a job whose stop is still in flight", true, false),
        ("a job whose stop the clock has completed", true, true),
    ];

    for (case, started, past_the_stop) in ways {
        let router = router();
        let user_pool_id = pool(&router, case, "stop-precondition-pool").await;
        let job_id = import_job(&router, case, &user_pool_id, "stop-precondition-job").await;
        let body = json!({ "UserPoolId": user_pool_id, "JobId": job_id });
        if *started {
            ok(&router, case, "StartUserImportJob", &body).await;
            ok(&router, case, "StopUserImportJob", &body).await;
        }
        if *past_the_stop {
            tokio::time::sleep(PAST_THE_STOP).await;
        }

        // The status the STORE and the clock put the job in is what the refusal
        // has to report, so it is read back rather than assumed.
        let status = job_status(&router, case, &user_pool_id, &job_id).await;
        let message = refusal(
            &router,
            case,
            "StopUserImportJob",
            &body,
            "PreconditionNotMetException",
        )
        .await;
        let expected = format!("Job is not running. The specified job has a status of {status}");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        // The sentence reports the status alone: neither id the request carried
        // reaches it.
        assert!(
            !message.contains(job_id.as_str()) && !message.contains(user_pool_id.as_str()),
            "case {case:?}: the sentence must quote no id, got {message:?}"
        );

        // The refused stop left the stored job exactly as it was.
        let after = job_status(&router, case, &user_pool_id, &job_id).await;
        assert_eq!(
            after, status,
            "case {case:?}: a refused stop must not move the stored job, got {after:?}"
        );
    }
}

/// How the app client a request is addressed by comes to be one the pool it
/// names does not hold.
#[derive(Debug, Clone, Copy)]
enum Unresolved {
    /// The `UserPoolId` names no pool this emulator holds, so the pool it names
    /// is not the one holding the app client.
    PoolNoCreateMinted,
    /// A live app client of ANOTHER pool: the id resolves, the pairing does not.
    AppClientOfAnotherPool,
    /// A `ClientIdType`-shaped id no create minted, asked for in a STORED pool.
    AppClientNoCreateMinted,
}

#[tokio::test]
async fn a_pair_addressed_app_client_lookup_names_both_ids_unless_the_pool_is_stored() {
    // The operations that resolve ONE app client by the pair `UserPoolId` +
    // `ClientId`, with the members each one carries beside the pair so the
    // app-client lookup is what the request fails on.
    let operations: &[(&str, Value)] = &[
        ("ListUserPoolClientSecrets", json!({})),
        (
            "SetUICustomization",
            json!({ "CSS": ".label { color: #111111; }" }),
        ),
    ];

    let router = router();
    let case = "app client lookup";
    let user_pool_id = pool(&router, case, "pair-lookup-pool").await;
    let other_pool = pool(&router, case, "pair-lookup-other-pool").await;
    let live = client(&router, case, &user_pool_id, "pair-live-client").await;
    let elsewhere = client(&router, case, &other_pool, "pair-elsewhere-client").await;

    let mut ways: Vec<(String, Unresolved, String, String)> = Vec::new();
    for absent in ABSENT_POOLS {
        ways.push((
            format!("a pool no create minted, {absent}, holding a live app client"),
            Unresolved::PoolNoCreateMinted,
            absent.to_string(),
            live.clone(),
        ));
        // The same absent pool paired with an id no create minted anywhere: the
        // pool named is still not one holding the app client, so the sentence is
        // the same whether the id resolves elsewhere or nowhere.
        ways.push((
            format!("a pool no create minted, {absent}, and an app client no create minted"),
            Unresolved::PoolNoCreateMinted,
            absent.to_string(),
            "neverminted1234".to_string(),
        ));
    }
    ways.push((
        "a live app client of another pool".to_string(),
        Unresolved::AppClientOfAnotherPool,
        user_pool_id.clone(),
        elsewhere.clone(),
    ));
    // Inside `ClientIdType`'s pattern `^[\w+]+$` and length 1..128
    // (`smithy_query(query="shape",shape="ClientIdType")`), so the lookup is
    // what fails rather than a modelled constraint.
    for asked in ["1h57kf5cpq17m0eml12eukdi1o", "z"] {
        ways.push((
            format!("an app client no create minted, {asked}"),
            Unresolved::AppClientNoCreateMinted,
            user_pool_id.clone(),
            asked.to_string(),
        ));
    }

    for (way, unresolved, asked_pool, asked_client) in &ways {
        for (operation, beside) in operations {
            let case = format!("{operation} on {way}");
            let mut body = Map::new();
            body.insert("UserPoolId".to_string(), json!(asked_pool));
            body.insert("ClientId".to_string(), json!(asked_client));
            for (name, value) in beside.as_object().expect("the members are an object") {
                body.insert(name.clone(), value.clone());
            }
            let message = refusal(
                &router,
                &case,
                operation,
                &Value::Object(body),
                "ResourceNotFoundException",
            )
            .await;
            // A pool that is not the one holding the app client — whether it
            // holds another or is not stored at all — is reported with both ids
            // the request carried; only a stored pool holding no such client is
            // reported with this operation's sentence for an id that names none.
            let expected = match unresolved {
                Unresolved::AppClientNoCreateMinted => {
                    format!("User pool client {asked_client} does not exist.")
                }
                _ => format!(
                    "User pool client {asked_client} does not exist in user pool {asked_pool}."
                ),
            };
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // Every one of those was a refusal: each pool still holds the app client its
    // create stored, with the secret that create minted, and the branding of the
    // live client is still unset.
    for (holder, held) in [(&user_pool_id, &live), (&other_pool, &elsewhere)] {
        let listed = ok(
            &router,
            case,
            "ListUserPoolClientSecrets",
            &json!({ "UserPoolId": holder, "ClientId": held }),
        )
        .await;
        let secrets = listed["ClientSecrets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a listed secret set, got {listed}"));
        assert_eq!(
            secrets.len(),
            1,
            "case {case:?}: the app client still holds the secret its create minted, got {listed}"
        );
    }
    let read = ok(
        &router,
        case,
        "GetUICustomization",
        &json!({ "UserPoolId": user_pool_id, "ClientId": live }),
    )
    .await;
    assert_eq!(
        read["UICustomization"]["CSS"],
        Value::Null,
        "case {case:?}: a refused set stored no style sheet, got {read}"
    );
}

#[tokio::test]
async fn a_create_refused_on_an_absent_pool_configures_nothing() {
    // The two operations whose pool lookup this change rewords; the sentence
    // each answers is asserted beside every other operation's in
    // `per_operation_refusal_wording.rs`, so what is asserted HERE is that the
    // refusal stored nothing and left the live pool configurable.
    let operations: &[(&str, Value)] = &[
        (
            "CreateUserPoolDomain",
            json!({ "Domain": "domain-of-an-absent-pool" }),
        ),
        ("CreateUserPoolReplica", json!({ "RegionName": "us-west-2" })),
    ];

    let router = router();
    let case = "pool lookup";
    // One pool that DOES exist, so a refusal cannot be the store being empty.
    let live_pool = pool(&router, case, "pool-sentence-live").await;

    for absent in ABSENT_POOLS {
        for (operation, beside) in operations {
            let case = format!("{operation} against {absent}");
            let mut body = Map::new();
            body.insert("UserPoolId".to_string(), json!(absent));
            for (name, value) in beside.as_object().expect("the members are an object") {
                body.insert(name.clone(), value.clone());
            }
            let message = refusal(
                &router,
                &case,
                operation,
                &Value::Object(body),
                "ResourceNotFoundException",
            )
            .await;
            assert!(
                message.contains(absent),
                "case {case:?}: the sentence names the pool asked for, got {message:?}"
            );
        }
    }

    // None of those refusals configured anything: the stored pool still takes
    // the domain they were refused, and reads it back, and holds no replica
    // beside its own primary.
    ok(
        &router,
        case,
        "CreateUserPoolDomain",
        &json!({ "UserPoolId": live_pool, "Domain": "pool-sentence-live-domain" }),
    )
    .await;
    let described = ok(
        &router,
        case,
        "DescribeUserPoolDomain",
        &json!({ "Domain": "pool-sentence-live-domain" }),
    )
    .await;
    assert_eq!(
        described["DomainDescription"]["UserPoolId"],
        json!(live_pool),
        "case {case:?}: the stored domain must read back on its pool, got {described}"
    );
    let replicas = ok(
        &router,
        case,
        "ListUserPoolReplicas",
        &json!({ "UserPoolId": live_pool }),
    )
    .await;
    assert_eq!(
        replicas["UserPoolReplicas"]
            .as_array()
            .map(|listed| listed.len()),
        Some(1),
        "case {case:?}: the pool still reports its own primary replica alone, got {replicas}"
    );
}
