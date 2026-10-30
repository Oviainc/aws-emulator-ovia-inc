// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the custom-browser lifecycle (`CreateBrowser`,
//! `GetBrowser`, `DeleteBrowser`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The settle span `src/state.rs` gives a browser's clock-owed transitions, plus
/// slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_300);

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=CreateBrowser`).
fn client_token(seed: &str) -> String {
    let token = format!("{seed}{}", "0".repeat(40));
    token[..40].to_string()
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(REGION))
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreateBrowser`: `PUT /browsers` (`smithy_query operation=CreateBrowser`).
async fn create_browser(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "PUT", "/browsers", Some(body)).await
}

/// `GetBrowser`: `GET /browsers/{browserId}`.
async fn get_browser(router: &MultiServiceRouter, browser_id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/browsers/{browser_id}"), None).await
}

/// `DeleteBrowser`: `DELETE /browsers/{browserId}`.
async fn delete_browser(router: &MultiServiceRouter, browser_id: &str) -> (u16, Value) {
    send(router, "DELETE", &format!("/browsers/{browser_id}"), None).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `BrowserId`'s pattern for a minted browser,
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$` (`smithy_query shape=BrowserId`),
/// checked by hand because this crate carries no regex engine.
fn is_modelled_browser_id(id: &str, name: &str) -> bool {
    let Some(suffix) = id.strip_prefix(&format!("{name}-")) else {
        return false;
    };
    suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC
/// 3339 form, naming a real instant. `createdAt` and `lastUpdatedAt` target
/// `DateTimestamp`, which carries `@timestampFormat("date-time")` (`smithy_query
/// operation=GetBrowser`), so an epoch-seconds number is not the modelled type.
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {rendered:?}")
    });
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {rendered:?}"
    );
}

/// A browser is created from the request and read back through `GetBrowser`, for every
/// shape of the configuration `CreateBrowser` accepts.
///
/// The rows differ along the members the request carries — the description, the
/// execution role, the network mode and its VPC configuration, the recording
/// configuration, the signing configuration, the enterprise policies, the certificates
/// and the mounted file systems — which is what the stored browser reports. Source for
/// the value lists: `BrowserNetworkMode`'s values, `RecordingConfig`'s and
/// `BrowserSigningConfigInput`'s `enabled` defaults and the `ToolsFileSystemConfiguration`
/// union's members (`smithy_query shape=BrowserNetworkMode,RecordingConfig,
/// BrowserSigningConfigInput,ToolsFileSystemConfiguration`).
#[tokio::test]
async fn a_browser_is_created_and_read_back_for_every_configuration() {
    let router = router();

    // (case, name, the members beyond `name` and `networkConfiguration`,
    //  the network configuration)
    let rows: [(&str, &str, Value, Value); 5] = [
        (
            "only the required members",
            "plainBrowser",
            json!({}),
            json!({"networkMode": "PUBLIC"}),
        ),
        (
            "a described browser with an execution role",
            "describedBrowser",
            json!({
                "description": "the shared browser",
                "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/BrowserRole")
            }),
            json!({"networkMode": "PUBLIC"}),
        ),
        (
            "a browser on a caller-supplied VPC",
            "vpcBrowser",
            json!({}),
            json!({
                "networkMode": "VPC",
                "vpcConfig": {
                    "securityGroups": ["sg-0123456789abcdef0"],
                    "subnets": ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"]
                }
            }),
        ),
        (
            "a recorded and signing browser",
            "recordedBrowser",
            json!({
                "recording": {
                    "enabled": true,
                    "s3Location": {"bucket": "recordings", "prefix": "sessions/"}
                },
                "browserSigning": {"enabled": true}
            }),
            json!({"networkMode": "PUBLIC"}),
        ),
        (
            "a browser carrying policies, certificates and file systems",
            "equippedBrowser",
            json!({
                // `BrowserEnterprisePolicyType`'s values are MANAGED | RECOMMENDED
                // (`smithy_query shape=BrowserEnterprisePolicyType`).
                "enterprisePolicies": [{
                    "location": {"s3": {"bucket": "policies", "prefix": "enterprise/"}},
                    "type": "MANAGED"
                }],
                "certificates": [{"location": {"secretsManager": {
                    "secretArn": format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:ca-1")
                }}}],
                // The ARNs the members' own patterns admit (`smithy_query
                // shape=S3FilesConfiguration`).
                "filesystemConfigurations": [{"s3FilesConfiguration": {
                    "accessPointArn": format!(
                        "arn:aws:s3files:{REGION}:{ACCOUNT}:file-system/fs-0123456789abcdef0\
                         /access-point/fsap-0123456789abcdef0"
                    ),
                    "mountPath": "/mnt/data",
                    "fileSystemArn": format!(
                        "arn:aws:s3files:{REGION}:{ACCOUNT}:file-system/fs-0123456789abcdef0"
                    )
                }}],
                "browserSigning": {"enabled": false}
            }),
            json!({"networkMode": "PUBLIC"}),
        ),
    ];

    // The browsers the table creates, so the clock-owed transition is waited out ONCE
    // after the loop rather than per row.
    let mut settling: Vec<(&str, String, Value)> = Vec::new();

    for (case, name, extra, network) in rows {
        let mut body = json!({"name": name, "networkConfiguration": network.clone()});
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }

        let (status, created) = create_browser(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateBrowser, got {status} ({created})"
        );

        let id = text(&created, "browserId");
        assert!(
            is_modelled_browser_id(&id, name),
            "case {case:?}: expected a browserId of the form <name>-<10 alphanumerics>, got {id:?}"
        );
        assert_eq!(
            text(&created, "browserArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:browser-custom/{id}"),
            "case {case:?}: expected the modelled BrowserArn for {id}, got {}",
            text(&created, "browserArn")
        );
        assert_eq!(
            text(&created, "status"),
            "CREATING",
            "case {case:?}: a browser is created in CREATING, got {}",
            text(&created, "status")
        );
        assert_date_time(case, "createdAt", &created["createdAt"]);

        let (status, fetched) = get_browser(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBrowser, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "name"),
            name,
            "case {case:?}: the name reads back as sent, got {}",
            text(&fetched, "name")
        );
        assert_eq!(
            text(&fetched, "browserArn"),
            text(&created, "browserArn"),
            "case {case:?}: GetBrowser reports the ARN CreateBrowser handed back, got {}",
            text(&fetched, "browserArn")
        );
        for member in [
            "description",
            "executionRoleArn",
            "recording",
            "enterprisePolicies",
            "certificates",
            "filesystemConfigurations",
        ] {
            assert_eq!(
                fetched[member],
                body.get(member).cloned().unwrap_or(Value::Null),
                "case {case:?}: {member} reads back as sent, got {}",
                fetched[member]
            );
        }
        assert_eq!(
            fetched["networkConfiguration"], network,
            "case {case:?}: the networkConfiguration reads back as sent, got {}",
            fetched["networkConfiguration"]
        );
        // `BrowserSigningConfigInput` and `BrowserSigningConfigOutput` declare the same
        // one member, so a read reports exactly what the create carried.
        assert_eq!(
            fetched["browserSigning"],
            body.get("browserSigning").cloned().unwrap_or(Value::Null),
            "case {case:?}: the signing configuration reads back as sent, got {}",
            fetched["browserSigning"]
        );
        assert_eq!(
            text(&fetched, "status"),
            "CREATING",
            "case {case:?}: the browser is still CREATING straight after the create, got {}",
            text(&fetched, "status")
        );
        assert!(
            fetched["failureReason"].is_null(),
            "case {case:?}: a browser that did not fail reports no failureReason, got {}",
            fetched["failureReason"]
        );
        assert_eq!(
            fetched["createdAt"], created["createdAt"],
            "case {case:?}: GetBrowser reports the create instant CreateBrowser handed back, \
             got {} against {}",
            fetched["createdAt"], created["createdAt"]
        );
        assert_eq!(
            fetched["lastUpdatedAt"], fetched["createdAt"],
            "case {case:?}: expected lastUpdatedAt and createdAt to be the one create instant, \
             got {} and {}",
            fetched["lastUpdatedAt"], fetched["createdAt"]
        );
        for member in ["createdAt", "lastUpdatedAt"] {
            assert_date_time(case, member, &fetched[member]);
        }

        settling.push((case, id, fetched["createdAt"].clone()));
    }

    // The transition to READY is the clock's: it is due at the settle instant whether or
    // not anybody reads the browser.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, created_at) in settling {
        let (status, fetched) = get_browser(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetBrowser, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "status"),
            "READY",
            "case {case:?}: the clock owes the browser READY once its settle span has passed, \
             got {}",
            text(&fetched, "status")
        );
        assert_eq!(
            fetched["createdAt"], created_at,
            "case {case:?}: a read does not re-date the browser, got {} against {created_at}",
            fetched["createdAt"]
        );
    }
}

/// An accepted deletion reports `DELETING` and the browser is gone once the span the
/// clock owes passes — for a browser deleted while still `CREATING` and for one deleted
/// after it reached `READY`, and whether the delete is sent once or repeated.
#[tokio::test]
async fn a_deleted_browser_reports_deleting_and_is_then_not_found() {
    let router = router();

    // (case, name, whether to wait the create out first, how many deletes to send)
    let rows: [(&str, &str, bool, usize); 3] = [
        ("deleted while still creating", "earlyBrowser", false, 1),
        ("deleted once it is ready", "readyBrowser", true, 1),
        ("deleted twice", "repeatBrowser", true, 2),
    ];

    for (case, name, wait_for_ready, deletes) in rows {
        let (status, created) = create_browser(
            &router,
            &json!({"name": name, "networkConfiguration": {"networkMode": "PUBLIC"}}),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateBrowser, got {status} ({created})"
        );
        let id = text(&created, "browserId");

        if wait_for_ready {
            tokio::time::sleep(PAST_SETTLE).await;
            let (_, ready) = get_browser(&router, &id).await;
            assert_eq!(
                text(&ready, "status"),
                "READY",
                "case {case:?}: the browser reaches READY before the delete, got {}",
                text(&ready, "status")
            );
        }

        for attempt in 1..=deletes {
            let (status, deleted) = delete_browser(&router, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?} (delete {attempt}): expected HTTP 202 from DeleteBrowser, got \
                 {status} ({deleted})"
            );
            assert_eq!(
                text(&deleted, "browserId"),
                id,
                "case {case:?} (delete {attempt}): DeleteBrowser reports the browser it deleted, \
                 got {}",
                text(&deleted, "browserId")
            );
            assert_eq!(
                text(&deleted, "status"),
                "DELETING",
                "case {case:?} (delete {attempt}): an accepted deletion reports DELETING, got {}",
                text(&deleted, "status")
            );
            assert_date_time(case, "lastUpdatedAt", &deleted["lastUpdatedAt"]);
        }

        // The browser reports `DELETING` until the removal the clock owes comes due.
        let (status, deleting) = get_browser(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a browser whose deletion is under way is still readable, got \
             {status} ({deleting})"
        );
        assert_eq!(
            text(&deleting, "status"),
            "DELETING",
            "case {case:?}: a browser whose deletion is under way reports DELETING, got {}",
            text(&deleting, "status")
        );

        tokio::time::sleep(PAST_SETTLE).await;
        let (status, missing) = get_browser(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a removed browser is the modelled 404, got {status} ({missing})"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {} ({missing})",
            error_code(&missing)
        );
        assert_eq!(
            text(&missing, "message"),
            format!("Browser with ID {id} does not exist"),
            "case {case:?}: the refusal names the browser id the request carried, got {}",
            text(&missing, "message")
        );

        // The name the removed browser held is this account's to use again.
        let (status, recreated) = create_browser(
            &router,
            &json!({"name": name, "networkConfiguration": {"networkMode": "PUBLIC"}}),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: the name of a removed browser can be used again, got {status} \
             ({recreated})"
        );
        assert_ne!(
            text(&recreated, "browserId"),
            id,
            "case {case:?}: a second create under the name mints its own browser, got {}",
            text(&recreated, "browserId")
        );
    }
}

/// What `CreateBrowser` refuses, and with which modelled error: a name this account
/// already holds, and a VPC-mode network configuration carrying no VPC configuration.
///
/// The rows differ along the rule that refuses the request, and each names the status and
/// error code a client branches on.
#[tokio::test]
async fn create_browser_refuses_a_duplicate_name_and_vpc_mode_without_a_vpc_config() {
    let router = router();

    let (status, created) = create_browser(
        &router,
        &json!({"name": "taken", "networkConfiguration": {"networkMode": "PUBLIC"}}),
    )
    .await;
    assert_eq!(
        status, 202,
        "the first create of a name succeeds, got {status} ({created})"
    );
    let taken_id = text(&created, "browserId");

    // (case, the request body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 4] = [
        (
            "the same name again",
            json!({"name": "taken", "networkConfiguration": {"networkMode": "PUBLIC"}}),
            409,
            "ConflictException",
        ),
        (
            "the same name with a different configuration",
            json!({
                "name": "taken",
                "description": "another browser",
                "networkConfiguration": {"networkMode": "PUBLIC"}
            }),
            409,
            "ConflictException",
        ),
        (
            "VPC mode with no vpcConfig",
            json!({"name": "unconfigured", "networkConfiguration": {"networkMode": "VPC"}}),
            400,
            "ValidationException",
        ),
        (
            "a name that differs only in case is its own browser",
            json!({"name": "Taken", "networkConfiguration": {"networkMode": "PUBLIC"}}),
            202,
            "",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, answered) = create_browser(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answered})"
        );
        if expected_status == 202 {
            continue;
        }
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
    }

    // The refused creates persisted nothing, and the one browser the name holds is still
    // the one the first create minted.
    let (status, fetched) = get_browser(&router, &taken_id).await;
    assert_eq!(
        status, 200,
        "the browser the first create minted is still readable, got {status} ({fetched})"
    );
    assert!(
        fetched["description"].is_null(),
        "a refused create does not overwrite the stored browser, got {}",
        fetched["description"]
    );
}

/// A `clientToken` replayed against `CreateBrowser` answers with the browser it first
/// produced, and an id naming no browser is the modelled 404 on both the read and the
/// delete.
#[tokio::test]
async fn a_replayed_client_token_mints_one_browser_and_an_unknown_id_is_not_found() {
    let router = router();

    let token = client_token("idem");
    let body = json!({
        "name": "onceBrowser",
        "clientToken": token,
        "networkConfiguration": {"networkMode": "PUBLIC"}
    });
    let (status, first) = create_browser(&router, &body).await;
    assert_eq!(
        status, 202,
        "the first create succeeds, got {status} ({first})"
    );

    // (case, the replayed body)
    let rows: [(&str, Value); 2] = [
        ("the same request again", body.clone()),
        (
            "the same token with other members changed",
            json!({
                "name": "onceBrowser",
                "clientToken": token,
                "description": "changed",
                "networkConfiguration": {"networkMode": "PUBLIC"}
            }),
        ),
    ];

    for (case, replayed) in rows {
        let (status, answered) = create_browser(&router, &replayed).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a replayed token is not an error, got {status} ({answered})"
        );
        assert_eq!(
            text(&answered, "browserId"),
            text(&first, "browserId"),
            "case {case:?}: a replayed token answers with the first browser, got {}",
            text(&answered, "browserId")
        );
    }

    // A different token under the same name is a second create of a held name.
    let (status, conflicted) = create_browser(
        &router,
        &json!({
            "name": "onceBrowser",
            "clientToken": client_token("other"),
            "networkConfiguration": {"networkMode": "PUBLIC"}
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "a fresh token does not bypass the unique-name rule, got {status} ({conflicted})"
    );

    // (case, the id addressed, expected status)
    let reads: [(&str, String, u16); 2] = [
        ("the minted id", text(&first, "browserId"), 200),
        ("an id no browser holds", "absentBrowser-0123456789".to_string(), 404),
    ];

    for (case, browser_id, expected_status) in reads {
        let (status, answered) = get_browser(&router, &browser_id).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from GetBrowser, got {status} \
             ({answered})"
        );
        let (status, answered) = delete_browser(&router, &browser_id).await;
        let expected_delete_status = if expected_status == 200 {
            202
        } else {
            expected_status
        };
        assert_eq!(
            status, expected_delete_status,
            "case {case:?}: expected HTTP {expected_delete_status} from DeleteBrowser, got \
             {status} ({answered})"
        );
        if expected_status == 404 {
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {} ({answered})",
                error_code(&answered)
            );
            assert_eq!(
                text(&answered, "message"),
                format!("Browser with ID {browser_id} does not exist"),
                "case {case:?}: the refusal names the browser id the request carried, got {}",
                text(&answered, "message")
            );
        }
    }
}
