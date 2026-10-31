// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the custom code-interpreter lifecycle
//! (`CreateCodeInterpreter`, `GetCodeInterpreter`, `DeleteCodeInterpreter`).
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

/// The settle span `src/state.rs` gives a code interpreter's clock-owed transitions,
/// plus slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_300);

/// A `clientToken` inside the modelled `ClientToken` constraints (len 33..256,
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`, `smithy_query operation=CreateCodeInterpreter`).
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

/// `CreateCodeInterpreter`: `PUT /code-interpreters`
/// (`smithy_query operation=CreateCodeInterpreter`).
async fn create_code_interpreter(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "PUT", "/code-interpreters", Some(body)).await
}

/// `GetCodeInterpreter`: `GET /code-interpreters/{codeInterpreterId}`.
async fn get_code_interpreter(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/code-interpreters/{id}"), None).await
}

/// `DeleteCodeInterpreter`: `DELETE /code-interpreters/{codeInterpreterId}`.
async fn delete_code_interpreter(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "DELETE", &format!("/code-interpreters/{id}"), None).await
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

/// `CodeInterpreterId`'s pattern for a minted interpreter,
/// `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$` (`smithy_query shape=CodeInterpreterId`),
/// checked by hand because this crate carries no regex engine.
fn is_modelled_code_interpreter_id(id: &str, name: &str) -> bool {
    let Some(suffix) = id.strip_prefix(&format!("{name}-")) else {
        return false;
    };
    suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Assert one `DateTimestamp` member's wire form: a JSON STRING in the canonical RFC
/// 3339 form, naming a real instant. `createdAt` and `lastUpdatedAt` target
/// `DateTimestamp`, which carries `@timestampFormat("date-time")` (`smithy_query
/// operation=GetCodeInterpreter`), so an epoch-seconds number is not the modelled type.
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

/// A code interpreter is created from the request and read back through
/// `GetCodeInterpreter`, for every shape of the configuration `CreateCodeInterpreter`
/// accepts.
///
/// The rows differ along the members the request carries — the description, the
/// execution role, the network mode and its VPC configuration, the certificates, the
/// mounted file systems and the tags — which is what the stored interpreter reports.
/// Source for the value lists: `CodeInterpreterNetworkMode`'s values (PUBLIC | SANDBOX |
/// VPC) and the `ToolsFileSystemConfiguration` union's members (`smithy_query
/// shape=CodeInterpreterNetworkMode,ToolsFileSystemConfiguration`).
#[tokio::test]
async fn a_code_interpreter_is_created_and_read_back_for_every_configuration() {
    let router = router();

    // (case, name, the members beyond `name` and `networkConfiguration`,
    //  the network configuration)
    let rows: [(&str, &str, Value, Value); 5] = [
        (
            "only the required members, in the sandbox network mode",
            "plainInterpreter",
            json!({}),
            json!({"networkMode": "SANDBOX"}),
        ),
        (
            "a described interpreter with an execution role",
            "describedInterpreter",
            json!({
                "description": "the shared interpreter",
                "executionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/InterpreterRole")
            }),
            json!({"networkMode": "SANDBOX"}),
        ),
        (
            "an interpreter reaching the public network",
            "publicInterpreter",
            json!({}),
            json!({"networkMode": "PUBLIC"}),
        ),
        (
            "an interpreter on a caller-supplied VPC",
            "vpcInterpreter",
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
            "an interpreter carrying certificates, file systems and tags",
            "equippedInterpreter",
            json!({
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
                "tags": {"team": "research"}
            }),
            json!({"networkMode": "SANDBOX"}),
        ),
    ];

    // The interpreters the table creates, so the clock-owed transition is waited out ONCE
    // after the loop rather than per row.
    let mut settling: Vec<(&str, String, Value)> = Vec::new();

    for (case, name, extra, network) in rows {
        let mut body = json!({"name": name, "networkConfiguration": network.clone()});
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            body[member] = value.clone();
        }

        let (status, created) = create_code_interpreter(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateCodeInterpreter, got {status} ({created})"
        );

        let id = text(&created, "codeInterpreterId");
        assert!(
            is_modelled_code_interpreter_id(&id, name),
            "case {case:?}: expected a codeInterpreterId of the form <name>-<10 alphanumerics>, \
             got {id:?}"
        );
        assert_eq!(
            text(&created, "codeInterpreterArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:code-interpreter-custom/{id}"),
            "case {case:?}: expected the modelled CodeInterpreterArn for {id}, got {}",
            text(&created, "codeInterpreterArn")
        );
        assert_eq!(
            text(&created, "status"),
            "CREATING",
            "case {case:?}: a code interpreter is created in CREATING, got {}",
            text(&created, "status")
        );
        assert_date_time(case, "createdAt", &created["createdAt"]);

        let (status, fetched) = get_code_interpreter(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetCodeInterpreter, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "name"),
            name,
            "case {case:?}: the name reads back as sent, got {}",
            text(&fetched, "name")
        );
        assert_eq!(
            text(&fetched, "codeInterpreterArn"),
            text(&created, "codeInterpreterArn"),
            "case {case:?}: GetCodeInterpreter reports the ARN CreateCodeInterpreter handed back, \
             got {}",
            text(&fetched, "codeInterpreterArn")
        );
        for member in [
            "description",
            "executionRoleArn",
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
        assert_eq!(
            text(&fetched, "status"),
            "CREATING",
            "case {case:?}: the interpreter is still CREATING straight after the create, got {}",
            text(&fetched, "status")
        );
        assert!(
            fetched["failureReason"].is_null(),
            "case {case:?}: an interpreter that did not fail reports no failureReason, got {}",
            fetched["failureReason"]
        );
        assert_eq!(
            fetched["createdAt"], created["createdAt"],
            "case {case:?}: GetCodeInterpreter reports the create instant \
             CreateCodeInterpreter handed back, got {} against {}",
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
    // not anybody reads the interpreter.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id, created_at) in settling {
        let (status, fetched) = get_code_interpreter(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetCodeInterpreter, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "status"),
            "READY",
            "case {case:?}: the clock owes the interpreter READY once its settle span has \
             passed, got {}",
            text(&fetched, "status")
        );
        assert_eq!(
            fetched["createdAt"], created_at,
            "case {case:?}: a read does not re-date the interpreter, got {} against {created_at}",
            fetched["createdAt"]
        );
    }
}

/// An accepted deletion reports `DELETING` and the interpreter is gone once the span the
/// clock owes passes — for one deleted while still `CREATING` and for one deleted after
/// it reached `READY`, and whether the delete is sent once or repeated.
#[tokio::test]
async fn a_deleted_code_interpreter_reports_deleting_and_is_then_not_found() {
    let router = router();

    // (case, name, whether to wait the create out first, how many deletes to send)
    let rows: [(&str, &str, bool, usize); 3] = [
        ("deleted while still creating", "earlyInterpreter", false, 1),
        ("deleted once it is ready", "readyInterpreter", true, 1),
        ("deleted twice", "repeatInterpreter", true, 2),
    ];

    for (case, name, wait_for_ready, deletes) in rows {
        let (status, created) = create_code_interpreter(
            &router,
            &json!({"name": name, "networkConfiguration": {"networkMode": "SANDBOX"}}),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateCodeInterpreter, got {status} ({created})"
        );
        let id = text(&created, "codeInterpreterId");

        if wait_for_ready {
            tokio::time::sleep(PAST_SETTLE).await;
            let (_, ready) = get_code_interpreter(&router, &id).await;
            assert_eq!(
                text(&ready, "status"),
                "READY",
                "case {case:?}: the interpreter reaches READY before the delete, got {}",
                text(&ready, "status")
            );
        }

        for attempt in 1..=deletes {
            let (status, deleted) = delete_code_interpreter(&router, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?} (delete {attempt}): expected HTTP 202 from \
                 DeleteCodeInterpreter, got {status} ({deleted})"
            );
            assert_eq!(
                text(&deleted, "codeInterpreterId"),
                id,
                "case {case:?} (delete {attempt}): DeleteCodeInterpreter reports the interpreter \
                 it deleted, got {}",
                text(&deleted, "codeInterpreterId")
            );
            assert_eq!(
                text(&deleted, "status"),
                "DELETING",
                "case {case:?} (delete {attempt}): an accepted deletion reports DELETING, got {}",
                text(&deleted, "status")
            );
            assert_date_time(case, "lastUpdatedAt", &deleted["lastUpdatedAt"]);
        }

        // The interpreter reports `DELETING` until the removal the clock owes comes due.
        let (status, deleting) = get_code_interpreter(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an interpreter whose deletion is under way is still readable, got \
             {status} ({deleting})"
        );
        assert_eq!(
            text(&deleting, "status"),
            "DELETING",
            "case {case:?}: an interpreter whose deletion is under way reports DELETING, got {}",
            text(&deleting, "status")
        );

        tokio::time::sleep(PAST_SETTLE).await;
        let (status, missing) = get_code_interpreter(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a removed interpreter is the modelled 404, got {status} ({missing})"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {} ({missing})",
            error_code(&missing)
        );
        assert_eq!(
            text(&missing, "message"),
            format!("Code interpreter with ID {id} does not exist"),
            "case {case:?}: the refusal names the id the request carried, got {}",
            text(&missing, "message")
        );

        // The name the removed interpreter held is this account's to use again.
        let (status, recreated) = create_code_interpreter(
            &router,
            &json!({"name": name, "networkConfiguration": {"networkMode": "SANDBOX"}}),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: the name of a removed interpreter can be used again, got {status} \
             ({recreated})"
        );
        assert_ne!(
            text(&recreated, "codeInterpreterId"),
            id,
            "case {case:?}: a second create under the name mints its own interpreter, got {}",
            text(&recreated, "codeInterpreterId")
        );
    }
}

/// What `CreateCodeInterpreter` refuses, and with which modelled error: a name this
/// account already holds, and a VPC-mode network configuration carrying no VPC
/// configuration.
///
/// The rows differ along the rule that refuses the request, and each names the status and
/// error code a client branches on.
#[tokio::test]
async fn create_code_interpreter_refuses_a_duplicate_name_and_vpc_mode_without_a_vpc_config() {
    let router = router();

    let (status, created) = create_code_interpreter(
        &router,
        &json!({"name": "taken", "networkConfiguration": {"networkMode": "SANDBOX"}}),
    )
    .await;
    assert_eq!(
        status, 202,
        "the first create of a name succeeds, got {status} ({created})"
    );
    let taken_id = text(&created, "codeInterpreterId");

    // (case, the request body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 4] = [
        (
            "the same name again",
            json!({"name": "taken", "networkConfiguration": {"networkMode": "SANDBOX"}}),
            409,
            "ConflictException",
        ),
        (
            "the same name with a different configuration",
            json!({
                "name": "taken",
                "description": "another interpreter",
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
            "a name that differs only in case is its own interpreter",
            json!({"name": "Taken", "networkConfiguration": {"networkMode": "SANDBOX"}}),
            202,
            "",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, answered) = create_code_interpreter(&router, &body).await;
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

    // The refused creates persisted nothing, and the one interpreter the name holds is
    // still the one the first create minted.
    let (status, fetched) = get_code_interpreter(&router, &taken_id).await;
    assert_eq!(
        status, 200,
        "the interpreter the first create minted is still readable, got {status} ({fetched})"
    );
    assert!(
        fetched["description"].is_null(),
        "a refused create does not overwrite the stored interpreter, got {}",
        fetched["description"]
    );
}

/// A `clientToken` replayed against `CreateCodeInterpreter` answers with the interpreter
/// it first produced, and an id naming no interpreter is the modelled 404 on both the
/// read and the delete.
#[tokio::test]
async fn a_replayed_client_token_mints_one_interpreter_and_an_unknown_id_is_not_found() {
    let router = router();

    let token = client_token("idem");
    let body = json!({
        "name": "onceInterpreter",
        "clientToken": token,
        "networkConfiguration": {"networkMode": "SANDBOX"}
    });
    let (status, first) = create_code_interpreter(&router, &body).await;
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
                "name": "onceInterpreter",
                "clientToken": token,
                "description": "changed",
                "networkConfiguration": {"networkMode": "SANDBOX"}
            }),
        ),
    ];

    for (case, replayed) in rows {
        let (status, answered) = create_code_interpreter(&router, &replayed).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a replayed token is not an error, got {status} ({answered})"
        );
        assert_eq!(
            text(&answered, "codeInterpreterId"),
            text(&first, "codeInterpreterId"),
            "case {case:?}: a replayed token answers with the first interpreter, got {}",
            text(&answered, "codeInterpreterId")
        );
    }

    // A different token under the same name is a second create of a held name.
    let (status, conflicted) = create_code_interpreter(
        &router,
        &json!({
            "name": "onceInterpreter",
            "clientToken": client_token("other"),
            "networkConfiguration": {"networkMode": "SANDBOX"}
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "a fresh token does not bypass the unique-name rule, got {status} ({conflicted})"
    );

    // (case, the id addressed, expected status)
    let reads: [(&str, String, u16); 2] = [
        ("the minted id", text(&first, "codeInterpreterId"), 200),
        (
            "an id no interpreter holds",
            "absentInterpreter-0123456789".to_string(),
            404,
        ),
    ];

    for (case, code_interpreter_id, expected_status) in reads {
        let (status, answered) = get_code_interpreter(&router, &code_interpreter_id).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from GetCodeInterpreter, got \
             {status} ({answered})"
        );
        let (status, answered) = delete_code_interpreter(&router, &code_interpreter_id).await;
        let expected_delete_status = if expected_status == 200 {
            202
        } else {
            expected_status
        };
        assert_eq!(
            status, expected_delete_status,
            "case {case:?}: expected HTTP {expected_delete_status} from DeleteCodeInterpreter, \
             got {status} ({answered})"
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
                format!("Code interpreter with ID {code_interpreter_id} does not exist"),
                "case {case:?}: the refusal names the id the request carried, got {}",
                text(&answered, "message")
            );
        }
    }
}
