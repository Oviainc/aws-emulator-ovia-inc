// SPDX-License-Identifier: CC0-1.0

//! What LAMBDA makes of an access point EFS minted, across the two services it spans.
//!
//! RULE: an id EFS assigns to a resource it creates carries the body AWS mints that id
//! with — the resource's prefix followed by 17 lowercase hex digits — so the ARN built
//! from it is one a consuming service admits: Lambda takes an EFS access point as a
//! function's file system only as
//! `arn:aws…:elasticfilesystem:<region>:<account>:access-point/fsap-[a-f0-9]{17}`
//! (`smithy_query(query="shape",shape="FileSystemConfig",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`),
//! and refuses anything else at its boundary with `400 ValidationException`.
//!
//! The ARN has to cross a service boundary, which is why this suite lives here: the access
//! point is created through EFS's own `CreateAccessPoint` on the same engine that serves
//! Lambda, so what `CreateFunction` is handed is the value EFS really minted and not a
//! copy written here.
//!
//! The rows vary what the rule generalises over: which draw of EFS's id counter the access
//! point came from, whether two functions mount the SAME access point, whether the access
//! point belongs to a second file system, and where the function mounts it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state as efs_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::lambda_signing_name_routing::{AUTHORIZATION, REGION, ROLE, ZIP_FILE};

/// A SigV4 `Authorization` header scoped to EFS's signing name and to [`REGION`], which is
/// what the router routes an EFS request on and what the EFS handlers read the Region of a
/// minted ARN from.
fn efs_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTEFS/20260101/{REGION}/elasticfilesystem/\
         aws4_request, SignedHeaders=host, Signature=0000000000000000000000000000000000000000"
    )
}

/// One emulator carrying both services over ONE shared engine, as the production router
/// builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_efs_magnolioapiservice_v20150201::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    router
}

/// One answer, reduced to what the assertions read: the status, the wire error code (`""`
/// on a success) and the JSON body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

/// Dispatch one restJson1 call to `host` with the credential scope `authorization`.
async fn call(
    router: &MultiServiceRouter,
    host: &str,
    authorization: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    Answer {
        status,
        code,
        body: parsed,
    }
}

/// One call to EFS.
async fn efs(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let host = format!("elasticfilesystem.{REGION}.amazonaws.com");
    call(router, &host, &efs_authorization(), method, uri, body).await
}

/// One call to the Lambda control plane.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!("lambda.{REGION}.amazonaws.com");
    call(router, &host, AUTHORIZATION, method, uri, body).await
}

/// The string member `key` of `body`.
fn member(body: &Value, key: &str) -> String {
    body[key].as_str().unwrap_or_default().to_string()
}

/// Create an EFS file system and report its id, waiting out the clock-owed
/// `creating` -> `available` transition an access point cannot be added before.
async fn available_file_system(router: &MultiServiceRouter, token: &str) -> String {
    let created = efs(
        router,
        "POST",
        "/2015-02-01/file-systems",
        Some(json!({ "CreationToken": token })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateFileSystem {token:?}: expected 201, got {} {}",
        created.status, created.body
    );
    tokio::time::sleep(efs_state::CREATING_FOR + Duration::from_millis(150)).await;
    member(&created.body, "FileSystemId")
}

/// Create an access point on `file_system_id` and report its ARN, as EFS minted it.
async fn access_point_arn(
    router: &MultiServiceRouter,
    case: &str,
    file_system_id: &str,
    token: &str,
) -> String {
    let created = efs(
        router,
        "POST",
        "/2015-02-01/access-points",
        Some(json!({
            "ClientToken": token,
            "FileSystemId": file_system_id,
            "PosixUser": { "Uid": 1001, "Gid": 1001 },
            "RootDirectory": {
                "Path": "/export/lambda",
                "CreationInfo": { "OwnerUid": 1001, "OwnerGid": 1001, "Permissions": "750" },
            },
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateAccessPoint expected 200, got {} {}",
        created.status, created.body
    );
    let arn = member(&created.body, "AccessPointArn");
    assert!(
        arn.ends_with(&format!(
            "access-point/{}",
            member(&created.body, "AccessPointId")
        )),
        "case {case:?}: the minted ARN names the minted id, got {arn:?}"
    );
    arn
}

/// The `CreateFunction` body for a function mounting `configs`.
fn create_body(name: &str, configs: &Value) -> Value {
    json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": ZIP_FILE },
        "FileSystemConfigs": configs,
    })
}

#[tokio::test]
async fn lambda_mounts_the_access_point_efs_minted() {
    let router = emulator();
    let shared = available_file_system(&router, "lambda-shared-mount").await;
    let other = available_file_system(&router, "lambda-other-mount").await;

    // Two access points on one file system and one on a second, each from its own draw of
    // EFS's id counter.
    let first = access_point_arn(&router, "the first access point", &shared, "shared-1").await;
    let second = access_point_arn(&router, "the second access point", &shared, "shared-2").await;
    let elsewhere =
        access_point_arn(&router, "an access point of another file system", &other, "other-1").await;

    // (case, the function's name, the access point it mounts, where it mounts it).
    let rows: Vec<(&str, &str, &String, &str)> = vec![
        (
            "the first function mounting the shared access point",
            "efs-reader",
            &first,
            "/mnt/shared",
        ),
        (
            "a second function mounting the SAME access point at the same path",
            "efs-writer",
            &first,
            "/mnt/shared",
        ),
        (
            "a function mounting a second access point of the same file system",
            "efs-second-view",
            &second,
            "/mnt/data",
        ),
        (
            "a function mounting an access point of another file system",
            "efs-other-file-system",
            &elsewhere,
            "/mnt/other",
        ),
    ];

    for (case, name, arn, path) in rows {
        let configs = json!([{ "Arn": arn, "LocalMountPath": path }]);
        let created = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            Some(create_body(name, &configs)),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateFunction expected 201, got {} {:?} {}",
            created.status, created.code, created.body
        );
        assert_eq!(
            created.body["FileSystemConfigs"], configs,
            "case {case:?}: the stored file system configuration is the one submitted, got \
             {:?}",
            created.body["FileSystemConfigs"]
        );

        // ROUND TRIP: an independent read reports the same mount, so the function really
        // carries the EFS access point and not just an accepted request.
        let read = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetFunctionConfiguration expected 200, got {} {:?} {}",
            read.status, read.code, read.body
        );
        assert_eq!(
            read.body["FileSystemConfigs"], configs,
            "case {case:?}: the read reports the mount the create stored, got {:?}",
            read.body["FileSystemConfigs"]
        );
    }
}

#[tokio::test]
async fn an_access_point_arn_of_another_width_is_refused_at_lambda_s_boundary() {
    let router = emulator();
    let file_system = available_file_system(&router, "lambda-width-rows").await;
    let minted = access_point_arn(&router, "the minted access point", &file_system, "width-1").await;
    let body = minted
            .rsplit('/')
            .next()
            .and_then(|id| id.strip_prefix("fsap-"))
            .unwrap_or_default()
            .to_string();
    let prefix = minted
        .strip_suffix(&body)
        .expect("the ARN ends with the id's hex body")
        .to_string();

    // (case, the ARN the request carries, whether Lambda admits it). The rows walk the
    // width of the id's hex body around the one the consuming pattern pins, so the
    // accepted row is the only one that is accepted BECAUSE of its width.
    let rows: Vec<(String, String, bool)> = vec![
        (
            format!("the body EFS mints, {} hex digits", body.len()),
            minted.clone(),
            true,
        ),
        (
            "one hex digit short".to_string(),
            format!("{prefix}{}", &body[..body.len() - 1]),
            false,
        ),
        (
            "one hex digit too many".to_string(),
            format!("{prefix}{body}0"),
            false,
        ),
        (
            "the shortest body this service's own id pattern admits".to_string(),
            format!("{prefix}{}", &body[..8]),
            false,
        ),
    ];

    for (index, (case, arn, admitted)) in rows.into_iter().enumerate() {
        let name = format!("efs-width-{index}");
        let configs = json!([{ "Arn": arn, "LocalMountPath": "/mnt/shared" }]);
        let created = lambda(
            &router,
            "POST",
            "/2015-03-31/functions",
            Some(create_body(&name, &configs)),
        )
        .await;
        if admitted {
            assert_eq!(
                created.status, 201,
                "case {case:?}: CreateFunction expected 201, got {} {:?} {}",
                created.status, created.code, created.body
            );
            assert_eq!(
                created.body["FileSystemConfigs"], configs,
                "case {case:?}: the stored file system configuration is the one submitted, \
                 got {:?}",
                created.body["FileSystemConfigs"]
            );
        } else {
            assert_eq!(
                created.status, 400,
                "case {case:?}: expected the boundary refusal, got {} {:?} {}",
                created.status, created.code, created.body
            );
            assert!(
                created.code.contains("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?} in {}",
                created.code,
                created.body
            );
        }
    }
}
