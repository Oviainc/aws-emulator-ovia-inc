// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CreateCustomAction` and the read that proves what it
//! persisted (`GetCustomAction`), driven over the wire (restJson1) through the same
//! `MultiServiceRouter` the production router serves with.
//!
//! Every assertion here is about the RULE each operation follows, exercised over a table
//! of inputs rather than one recorded request:
//!
//! * a created custom action is persisted under
//!   `arn:aws:chatbot:{region}:{account}:custom-action/{ActionName}` and read back by
//!   `GetCustomAction` exactly as created, whatever subset of the optional members
//!   (`AliasName`, `Attachments`, `Tags`, `ClientToken`) the create carried and however
//!   many attachments, criteria or variables they hold;
//! * one `ActionName` per region: a second create for a name a region already holds is a
//!   409 `ConflictException` and leaves the stored action alone, while the SAME
//!   `ClientToken` the action was minted under makes the retry answer the original
//!   create's ARN instead (the model's `@idempotencyToken`);
//! * the ARN is regional, so the same `ActionName` can be created once per region and an
//!   ARN no region holds is a 404 `ResourceNotFoundException`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for, so the minted ARNs carry it.
const REGION: &str = "us-east-1";

/// A second region, for the rows that show an ARN — and so a name — is regional.
const OTHER_REGION: &str = "us-west-2";

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled
/// out here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

/// A `ClientToken` satisfying the model's `ClientToken` (len 33..126, pattern
/// `^[\x21-\x7F]+$` — 33 printable, space-free characters).
const CLIENT_TOKEN: &str = "0123456789abcdef0123456789abcdef0";

/// A second, different token of the same shape.
const OTHER_CLIENT_TOKEN: &str = "fedcba9876543210fedcba9876543210f";

/// A SigV4 credential scope naming this service and `region`; `MultiServiceRouter`
/// routes on the scope's service name and the handlers read the region from it.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/chatbot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// A router mounting only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_chatbot_wheatleyorchestration_20171011::EmulatedService::new(engine),
    );
    router
}

/// POST `body` to `path`, signed for `region`, and return the status and the parsed JSON
/// body (`Value::Null` for an empty body, as a 204 has).
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://chatbot.{region}.amazonaws.com{path}"))
        .header("host", format!("chatbot.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(region))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{path}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// POST `body` to `path`, signed for [`REGION`].
async fn call(router: &MultiServiceRouter, path: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, path, body).await
}

/// The error code (`__type`) a failure response carries.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

fn arn_in_region(region: &str, action_name: &str) -> String {
    format!("arn:aws:chatbot:{region}:{ACCOUNT}:custom-action/{action_name}")
}

fn expected_arn(action_name: &str) -> String {
    arn_in_region(REGION, action_name)
}

/// Create `request` and return the `CustomActionArn` it answers.
async fn create_ok(router: &MultiServiceRouter, request: &Value) -> String {
    create_ok_in_region(router, REGION, request).await
}

async fn create_ok_in_region(router: &MultiServiceRouter, region: &str, request: &Value) -> String {
    let (status, body) = call_in_region(router, region, "/create-custom-action", request).await;
    assert_eq!(
        status, 201,
        "create {request} in {region}: expected 201, got {status} with {body}"
    );
    body.get("CustomActionArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {request}: no CustomActionArn in {body}"))
        .to_string()
}

/// Read the action `arn` back, asserting the read succeeded, and answer the
/// `CustomAction` it reports.
async fn get_ok(router: &MultiServiceRouter, arn: &str) -> Value {
    get_ok_in_region(router, REGION, arn).await
}

async fn get_ok_in_region(router: &MultiServiceRouter, region: &str, arn: &str) -> Value {
    let request = json!({"CustomActionArn": arn});
    let (status, body) = call_in_region(router, region, "/get-custom-action", &request).await;
    assert_eq!(
        status, 200,
        "get {arn} in {region}: expected 200, got {status} with {body}"
    );
    body.get("CustomAction")
        .cloned()
        .unwrap_or_else(|| panic!("get {arn}: no CustomAction in {body}"))
}

/// One row of the create table: the request body, and the `CustomAction` it must leave in
/// the store.
struct CreateCase {
    label: &'static str,
    request: Value,
    expected: Value,
}

/// The create cases, each differing from the others along what the create rule
/// generalises over: which optional members the request carries, how many attachments it
/// defines, how many criteria and variables each attachment holds, which criteria
/// operator it names, and how many tags come with it.
///
/// The operators are the model's whole published `CustomActionAttachmentCriteriaOperator`
/// value set (`smithy_query`: `HAS_VALUE`, `EQUALS`), and the list bounds exercised below
/// — 5 criteria and 5 variables — are that shape's published maxima
/// (`CustomActionAttachmentCriteriaList` / `CustomActionAttachmentVariables`, len 1..5).
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "only the required members",
            request: json!({
                "ActionName": "minimal",
                "Definition": {"CommandText": "ssm start-session"},
            }),
            expected: json!({
                "CustomActionArn": expected_arn("minimal"),
                "ActionName": "minimal",
                "Definition": {"CommandText": "ssm start-session"},
            }),
        },
        CreateCase {
            label: "an alias but no attachments and no tags",
            request: json!({
                "ActionName": "alias_only-1",
                "Definition": {"CommandText": "lambda invoke --function-name $Function"},
                "AliasName": "run-lambda",
                "Tags": [],
                "ClientToken": CLIENT_TOKEN,
            }),
            expected: json!({
                "CustomActionArn": expected_arn("alias_only-1"),
                "ActionName": "alias_only-1",
                "Definition": {"CommandText": "lambda invoke --function-name $Function"},
                "AliasName": "run-lambda",
            }),
        },
        CreateCase {
            label: "one attachment, one HAS_VALUE criterion, one variable, one tag",
            request: json!({
                "ActionName": "one-attachment",
                "Definition": {"CommandText": "cloudwatch describe-alarms"},
                "AliasName": "alarms",
                "Attachments": [{
                    "NotificationType": "aws-cloudwatch-alarm",
                    "ButtonText": "Describe alarms",
                    "Criteria": [{"Operator": "HAS_VALUE", "VariableName": "AlarmName"}],
                    "Variables": {"AlarmName": "$.detail.alarmName"},
                }],
                "Tags": [{"TagKey": "env", "TagValue": "dev"}],
            }),
            expected: json!({
                "CustomActionArn": expected_arn("one-attachment"),
                "ActionName": "one-attachment",
                "Definition": {"CommandText": "cloudwatch describe-alarms"},
                "AliasName": "alarms",
                "Attachments": [{
                    "NotificationType": "aws-cloudwatch-alarm",
                    "ButtonText": "Describe alarms",
                    "Criteria": [{"Operator": "HAS_VALUE", "VariableName": "AlarmName"}],
                    "Variables": {"AlarmName": "$.detail.alarmName"},
                }],
            }),
        },
        CreateCase {
            label: "two attachments, the criteria list and the variable map at their maxima",
            request: json!({
                "ActionName": "TwoAttachments_2",
                "Definition": {"CommandText": "codedeploy stop-deployment --deployment-id $Id"},
                "Attachments": [
                    {
                        "NotificationType": "aws-codedeploy-deployment",
                        "ButtonText": "Stop deployment",
                        "Criteria": [
                            {"Operator": "EQUALS", "VariableName": "State", "Value": "FAILED"},
                            {"Operator": "HAS_VALUE", "VariableName": "Id"},
                            {"Operator": "EQUALS", "VariableName": "Region", "Value": "us-east-1"},
                            {"Operator": "HAS_VALUE", "VariableName": "Account"},
                            {"Operator": "EQUALS", "VariableName": "App", "Value": "checkout"},
                        ],
                        "Variables": {
                            "State": "$.detail.state",
                            "Id": "$.detail.deploymentId",
                            "Region": "$.region",
                            "Account": "$.account",
                            "App": "$.detail.application",
                        },
                    },
                    {
                        "NotificationType": "aws-health-event",
                        "ButtonText": "Open event",
                    },
                ],
                "Tags": [
                    {"TagKey": "env", "TagValue": "prod"},
                    {"TagKey": "owner", "TagValue": ""},
                ],
                "ClientToken": OTHER_CLIENT_TOKEN,
            }),
            expected: json!({
                "CustomActionArn": expected_arn("TwoAttachments_2"),
                "ActionName": "TwoAttachments_2",
                "Definition": {"CommandText": "codedeploy stop-deployment --deployment-id $Id"},
                "Attachments": [
                    {
                        "NotificationType": "aws-codedeploy-deployment",
                        "ButtonText": "Stop deployment",
                        "Criteria": [
                            {"Operator": "EQUALS", "VariableName": "State", "Value": "FAILED"},
                            {"Operator": "HAS_VALUE", "VariableName": "Id"},
                            {"Operator": "EQUALS", "VariableName": "Region", "Value": "us-east-1"},
                            {"Operator": "HAS_VALUE", "VariableName": "Account"},
                            {"Operator": "EQUALS", "VariableName": "App", "Value": "checkout"},
                        ],
                        "Variables": {
                            "State": "$.detail.state",
                            "Id": "$.detail.deploymentId",
                            "Region": "$.region",
                            "Account": "$.account",
                            "App": "$.detail.application",
                        },
                    },
                    {
                        "NotificationType": "aws-health-event",
                        "ButtonText": "Open event",
                    },
                ],
            }),
        },
        CreateCase {
            label: "an attachment with no criteria and the longest allowed action name",
            // `ActionName` is len 1..64, pattern `^[a-zA-Z0-9_-]{1,64}$` (smithy_query:
            // CustomActionName): this row sits on the published upper bound.
            request: json!({
                "ActionName": "a".repeat(64),
                "Definition": {"CommandText": "s3 ls"},
                "Attachments": [{"ButtonText": "List buckets"}],
            }),
            expected: json!({
                "CustomActionArn": expected_arn(&"a".repeat(64)),
                "ActionName": "a".repeat(64),
                "Definition": {"CommandText": "s3 ls"},
                "Attachments": [{"ButtonText": "List buckets"}],
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_custom_action_is_persisted_and_read_back_exactly_as_created() {
    let router = router();

    for case in create_cases() {
        let arn = create_ok(&router, &case.request).await;
        let expected_arn = case.expected["CustomActionArn"].as_str().unwrap();
        assert_eq!(
            arn, expected_arn,
            "case {:?}: created ARN differs, got {arn:?}",
            case.label
        );

        // Read it back: the answer must come out of the store, not out of the create
        // request.
        let action = get_ok(&router, &arn).await;
        assert_eq!(
            action, case.expected,
            "case {:?}: the stored custom action differs from the expected one, got {action}",
            case.label
        );
    }
}

#[tokio::test]
async fn an_action_name_is_regional_so_the_same_name_is_created_once_per_region() {
    let router = router();

    for case in create_cases() {
        let action_name = case.request["ActionName"].as_str().unwrap().to_string();
        let here = create_ok(&router, &case.request).await;
        let there = create_ok_in_region(&router, OTHER_REGION, &case.request).await;
        assert_eq!(
            here,
            arn_in_region(REGION, &action_name),
            "case {:?}: the ARN must name the request's region, got {here:?}",
            case.label
        );
        assert_eq!(
            there,
            arn_in_region(OTHER_REGION, &action_name),
            "case {:?}: the ARN must name the request's region, got {there:?}",
            case.label
        );

        // An ARN fully qualifies an action, so either region's ARN reads back from
        // either region's request, and each holds its own record.
        for arn in [&here, &there] {
            let action = get_ok_in_region(&router, OTHER_REGION, arn).await;
            assert_eq!(
                action["CustomActionArn"].as_str(),
                Some(arn.as_str()),
                "case {:?}: {arn} must read back as itself, got {action}",
                case.label
            );
        }
    }
}

/// One row of the duplicate-create table: the token the second create carries (if any),
/// and whether that makes it the minting create's idempotent retry.
struct DuplicateCase {
    label: &'static str,
    second_client_token: Option<&'static str>,
    is_retry: bool,
}

#[tokio::test]
async fn a_duplicate_action_name_conflicts_unless_it_retries_the_minting_client_token() {
    let cases = [
        DuplicateCase {
            label: "the same client token: the minting create's retry",
            second_client_token: Some(CLIENT_TOKEN),
            is_retry: true,
        },
        DuplicateCase {
            label: "a different client token",
            second_client_token: Some(OTHER_CLIENT_TOKEN),
            is_retry: false,
        },
        DuplicateCase {
            label: "no client token at all",
            second_client_token: None,
            is_retry: false,
        },
    ];

    for case in cases {
        // A fresh store per row, so each row's second create races the same first one.
        let router = router();
        let first = json!({
            "ActionName": "duplicated",
            "Definition": {"CommandText": "ec2 describe-instances"},
            "AliasName": "instances",
            "ClientToken": CLIENT_TOKEN,
        });
        let arn = create_ok(&router, &first).await;
        let stored = get_ok(&router, &arn).await;

        // The second create asks for the same name with a DIFFERENT definition and
        // alias, so a create that wrote anything would be visible in the read below.
        let mut second = json!({
            "ActionName": "duplicated",
            "Definition": {"CommandText": "ec2 stop-instances --instance-ids $Id"},
            "AliasName": "stop",
        });
        if let Some(token) = case.second_client_token {
            second["ClientToken"] = json!(token);
        }
        let (status, body) = call(&router, "/create-custom-action", &second).await;

        if case.is_retry {
            assert_eq!(
                status, 201,
                "case {:?}: a retry of the minting create must succeed, got {status} with {body}",
                case.label
            );
            assert_eq!(
                body.get("CustomActionArn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {:?}: a retry must answer the original ARN, got {body}",
                case.label
            );
        } else {
            assert_eq!(
                status, 409,
                "case {:?}: expected 409, got {status} with {body}",
                case.label
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {:?}: expected ConflictException, got {body}",
                case.label
            );
        }

        // Either way the stored action is the one the FIRST create left: a retry answers
        // that create's result and a refused create writes nothing.
        let after = get_ok(&router, &arn).await;
        assert_eq!(
            after, stored,
            "case {:?}: the stored action must be unchanged, got {after}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_create_past_the_store_bound_is_refused_and_persists_nothing() {
    let router = router();
    // The emulator's own bound on how many custom actions the store holds at all
    // (`state::MAX_CUSTOM_ACTIONS`), counted across regions.
    let bound = service_com_amazonaws_chatbot_wheatleyorchestration_20171011::state::MAX_CUSTOM_ACTIONS;

    for index in 0..bound {
        create_ok(
            &router,
            &json!({
                "ActionName": format!("bounded-{index}"),
                "Definition": {"CommandText": "s3 ls"},
            }),
        )
        .await;
    }

    // Past the bound a create is refused — in THIS region and in another one, because
    // the count is the whole store's rather than a region's.
    for region in [REGION, OTHER_REGION] {
        let request = json!({
            "ActionName": "one-too-many",
            "Definition": {"CommandText": "s3 ls"},
        });
        let (status, body) = call_in_region(&router, region, "/create-custom-action", &request).await;
        assert_eq!(
            status, 403,
            "create in {region} past the bound of {bound}: expected 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "LimitExceededException",
            "create in {region} past the bound of {bound}: expected LimitExceededException, got {body}"
        );

        // Nothing was persisted for the refused name.
        let refused = json!({"CustomActionArn": arn_in_region(region, "one-too-many")});
        let (status, body) = call_in_region(&router, region, "/get-custom-action", &refused).await;
        assert_eq!(
            status, 404,
            "the refused name must not be stored in {region}, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn getting_an_arn_the_store_does_not_hold_is_resource_not_found() {
    let router = router();
    let created = create_ok(
        &router,
        &json!({
            "ActionName": "present",
            "Definition": {"CommandText": "s3 ls"},
        }),
    )
    .await;

    let cases = [
        ("a name nothing was created under", expected_arn("absent")),
        (
            "the created name in another region",
            arn_in_region(OTHER_REGION, "present"),
        ),
        (
            "another account's ARN for the created name",
            format!("arn:aws:chatbot:{REGION}:999999999999:custom-action/present"),
        ),
    ];

    for (label, arn) in cases {
        let request = json!({"CustomActionArn": arn});
        let (status, body) = call(&router, "/get-custom-action", &request).await;
        assert_eq!(
            status, 404,
            "case {label:?} ({arn}): expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?} ({arn}): expected ResourceNotFoundException, got {body}"
        );
    }

    // The action that IS there is untouched by those failed reads.
    let action = get_ok(&router, &created).await;
    assert_eq!(
        action["ActionName"].as_str(),
        Some("present"),
        "the created action must still read back, got {action}"
    );
}
