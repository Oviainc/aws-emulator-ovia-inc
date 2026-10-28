// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the two-step every POLICY operation shares.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a policy operation reaches a stored document in two steps, in
//! this order — `{identifier}` is resolved first and nothing resolving is the absent
//! GROUP, then the stored document is looked at and a group that exists while carrying
//! none is the absent POLICY — so `GetInvestigationGroupPolicy` and
//! `DeleteInvestigationGroupPolicy` answer the SAME status, error code and message for
//! the same stored state, whichever of them carried the call.
//!
//! Rows differ along what decides the answer: how the identifier fails to resolve (a
//! bare label the modelled pattern admits, an ARN differing from the stored one in one
//! field) versus whether a document is stored, including rows where a document IS
//! stored but the identifier still resolves nothing, which is what pins the ORDER of
//! the two steps.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::state;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for: the one the row's group is created in,
/// so an identifier naming any other Region resolves nothing at this endpoint.
const REGION: &str = "us-west-2";

/// The name every row's group is created with.
const GROUP_NAME: &str = "policy-two-step";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in [`REGION`], the signal the router routes on and the
/// handlers read the region and the caller principal from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{REGION}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status, modelled error shape (empty on a success) and parsed body of one call. An
/// empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string())
        .unwrap_or_default();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, error_type, parsed)
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is what
/// an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Create a group that must succeed, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter) -> String {
    let (status, code, created) = call(
        router,
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": GROUP_NAME, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        status, 201,
        "creating the row's group: expected 201, got {status} / {code} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the create response carried no arn, got {created:?}"))
        .to_string()
}

async fn put_policy(router: &MultiServiceRouter, identifier: &str, document: &str) -> (u16, String, Value) {
    call(
        router,
        "POST",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        Some(&json!({ "policy": document })),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, identifier: &str) -> (u16, String, Value) {
    call(
        router,
        "GET",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        None,
    )
    .await
}

async fn delete_policy(router: &MultiServiceRouter, identifier: &str) -> (u16, String, Value) {
    call(
        router,
        "DELETE",
        &format!("/investigationGroups/{}/policy", label(identifier)),
        None,
    )
    .await
}

/// The message this service words an absent-resource 404 with, for the modelled
/// resource `resource`.
fn absent(resource: &str) -> String {
    format!("{resource} does not exist.")
}

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The id segment of a group ARN.
fn id_of(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .unwrap_or_else(|| panic!("a group ARN carries an id segment, got {arn:?}"))
        .to_string()
}

/// Where the shared two-step stops for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopsAt {
    /// Step one: nothing resolved, so the GROUP is the absent resource.
    AbsentGroup,
    /// Step two: the group resolved and carries no document, so the POLICY is.
    AbsentPolicy,
    /// Neither: a document is stored, so both operations serve the call.
    StoredDocument,
}

/// One row: what the operations are addressed with, what the group was left carrying,
/// and where the two-step therefore stops.
struct Case {
    what: &'static str,
    /// The `{identifier}` both operations are called with, from the stored group's ARN.
    identifier: fn(&str) -> String,
    /// The document stored on the group before the call, if any.
    document: Option<&'static str>,
    stops_at: StopsAt,
}

/// The rows, spanning both steps: the identifier forms that resolve no group (whether
/// or not a document is stored, which is what pins step one as the FIRST step) and the
/// stored states a resolved group can be in.
fn cases() -> Vec<Case> {
    // A document the operation accepts (`policy` is "The policy, in JSON format"), in
    // the smallest and a realistic form.
    const SMALLEST: &str = "{}";
    const REALISTIC: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"aiops.alarms.cloudwatch.amazonaws.com"},"Action":["aiops:CreateInvestigation"],"Resource":"*"}]}"#;

    vec![
        Case {
            what: "a name no group carries",
            identifier: |_| "no-such-group".to_string(),
            document: None,
            stops_at: StopsAt::AbsentGroup,
        },
        Case {
            what: "the group's own name, with a document stored",
            identifier: |_| GROUP_NAME.to_string(),
            document: Some(SMALLEST),
            stops_at: StopsAt::AbsentGroup,
        },
        Case {
            what: "the group's own bare id, with a document stored",
            identifier: id_of,
            document: Some(REALISTIC),
            stops_at: StopsAt::AbsentGroup,
        },
        Case {
            what: "an ARN in another Region, with a document stored",
            identifier: |arn| arn.replace(&format!(":{REGION}:"), ":eu-west-1:"),
            document: Some(SMALLEST),
            stops_at: StopsAt::AbsentGroup,
        },
        Case {
            what: "an ARN in another account, with a document stored",
            identifier: |arn| arn.replace(":000000000000:", ":111122223333:"),
            document: Some(SMALLEST),
            stops_at: StopsAt::AbsentGroup,
        },
        Case {
            what: "an ARN in another partition, with a document stored",
            identifier: |arn| arn.replace("arn:aws:", "arn:aws-us-gov:"),
            document: Some(REALISTIC),
            stops_at: StopsAt::AbsentGroup,
        },
        Case {
            what: "an ARN carrying another id, with a document stored",
            identifier: |arn| arn.replace(&id_of(arn), &"Z".repeat(16)),
            document: Some(SMALLEST),
            stops_at: StopsAt::AbsentGroup,
        },
        Case {
            what: "the group's ARN, carrying no document",
            identifier: str::to_string,
            document: None,
            stops_at: StopsAt::AbsentPolicy,
        },
        Case {
            what: "the group's ARN, carrying the smallest document",
            identifier: str::to_string,
            document: Some(SMALLEST),
            stops_at: StopsAt::StoredDocument,
        },
        Case {
            what: "the group's ARN, carrying a realistic policy",
            identifier: str::to_string,
            document: Some(REALISTIC),
            stops_at: StopsAt::StoredDocument,
        },
    ]
}

#[tokio::test]
async fn both_policy_operations_take_the_same_two_step_to_a_stored_document() {
    let group_absent = absent(state::INVESTIGATION_GROUP_RESOURCE);
    let policy_absent = absent(state::INVESTIGATION_GROUP_POLICY_RESOURCE);
    assert_ne!(
        group_absent, policy_absent,
        "the two conditions must be distinguishable, got {group_absent:?} for both"
    );

    for case in cases() {
        // A fresh store per row: an account holds one investigation group per Region,
        // and the delete half of the row mutates what it resolved.
        let router = emulator();
        let arn = create_ok(&router).await;
        if let Some(document) = case.document {
            let (status, code, put) = put_policy(&router, &arn, document).await;
            assert_eq!(
                status, 200,
                "case {:?}: storing the row's document must succeed, got {status} / {code} / {put:?}",
                case.what
            );
        }
        let identifier = (case.identifier)(&arn);
        let what = case.what;

        // The reporting half first: it must not change what the mutating half sees.
        let (get_status, get_code, got) = get_policy(&router, &identifier).await;
        match case.stops_at {
            StopsAt::StoredDocument => {
                assert_eq!(
                    (get_status, got.get("policy").and_then(Value::as_str)),
                    (200, case.document),
                    "case {what:?}: expected the stored document back, \
                     got {get_status} / {get_code} / {got:?}"
                );
                assert_eq!(
                    got.get("investigationGroupArn").and_then(Value::as_str),
                    Some(arn.as_str()),
                    "case {what:?}: expected the resolved group's ARN, got {got:?}"
                );
            }
            StopsAt::AbsentGroup | StopsAt::AbsentPolicy => {
                let expected = if case.stops_at == StopsAt::AbsentGroup {
                    &group_absent
                } else {
                    &policy_absent
                };
                assert_eq!(
                    (get_status, get_code.as_str(), message(&got)),
                    (404, "ResourceNotFoundException", expected.clone()),
                    "case {what:?}: the read must report the resource the two-step \
                     stopped on, got {get_status} / {get_code} / {got:?}"
                );
                assert!(
                    !message(&got).contains(&identifier),
                    "case {what:?}: the message must not echo the identifier \
                     {identifier:?}, got {got:?}"
                );
            }
        }

        // The mutating half: the SAME two-step, so the same answer for the same state.
        let (del_status, del_code, deleted) = delete_policy(&router, &identifier).await;
        match case.stops_at {
            StopsAt::StoredDocument => {
                assert_eq!(
                    del_status, 200,
                    "case {what:?}: expected 200 from the delete, \
                     got {del_status} / {del_code} / {deleted:?}"
                );
                // The document is gone, which moves this row to the SECOND step: the
                // group is still there, so what is now absent is the policy.
                let (status, code, after) = get_policy(&router, &identifier).await;
                assert_eq!(
                    (status, code.as_str(), message(&after)),
                    (404, "ResourceNotFoundException", policy_absent.clone()),
                    "case {what:?}: after the delete the policy is the absent \
                     resource, got {status} / {code} / {after:?}"
                );
            }
            StopsAt::AbsentGroup | StopsAt::AbsentPolicy => {
                assert_eq!(
                    (del_status, del_code.as_str(), message(&deleted)),
                    (get_status, get_code.as_str(), message(&got)),
                    "case {what:?}: both policy operations must answer one stored \
                     state identically, got {del_status} / {del_code} / {deleted:?} \
                     from the delete and {get_status} / {get_code} / {got:?} from the read"
                );
            }
        }

        // Whatever the row answered, the GROUP itself is untouched and still readable.
        let (status, code, group) = call(
            &router,
            "GET",
            &format!("/investigationGroups/{}", label(&arn)),
            None,
        )
        .await;
        assert_eq!(
            (status, group.get("arn").and_then(Value::as_str)),
            (200, Some(arn.as_str())),
            "case {what:?}: the group must survive a policy call, \
             got {status} / {code} / {group:?}"
        );
    }
}
