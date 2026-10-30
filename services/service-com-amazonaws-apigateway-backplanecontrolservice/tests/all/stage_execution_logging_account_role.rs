// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ACCOUNT-level precondition of a stage's execution logging:
//! what `UpdateStage` does with a method-setting patch that turns execution logging on
//! while the account carries no CloudWatch Logs role.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `GetStage` / `UpdateAccount` helpers are this
//! crate's existing harness, reused from `rest_api_lifecycle`, `stage_lifecycle` and
//! `account_settings`.
//!
//! The RULE under test: API Gateway writes a method's execution log events under the
//! account-level CloudWatch Logs role, so a stage configuration that turns execution
//! logging on for any method scope — a logging level other than `OFF`, or the full
//! request/response data trace — is refused at CONFIGURATION time with HTTP 400
//! `BadRequestException` while the account's role is unset, and the refused patch writes
//! nothing; a configuration that turns on no execution logging is unaffected by the
//! account's role.
//!
//! The rows span what that generalises over: both settings that turn execution logging on,
//! each documented level that is not `OFF`, the published group-and-name spelling and the
//! `MethodSetting` member spelling of each, the stage-wide scope and a named method, an
//! `add` beside a `replace`, a list carrying another op the stage would have applied, and
//! the settings and members a role-less account places no condition on at all. Each row is
//! driven on an account with no role, then on the same account once a role is configured,
//! then on a second stage after the role is cleared again.

use serde_json::{json, Value};

use crate::account_settings::{get_account, patch_account, role_arn};
use crate::rest_api_lifecycle::{assert_failure, create_api, string_member, Answer, Gateway};
use crate::stage_lifecycle::{create_deployed_stage, get_stage};

/// The stage every row is deployed to.
const STAGE: &str = "prod";

/// The second stage of the same API, which a row patches once the account's role has been
/// cleared again — a scope no earlier step of the row has configured.
const OTHER_STAGE: &str = "staging";

/// The JSON Pointer of the account member that carries the CloudWatch Logs role.
const CLOUDWATCH_ROLE_PATH: &str = "/cloudwatchRoleArn";

/// The message API Gateway refuses a stage configuration that turns execution logging on
/// without the account's CloudWatch Logs role with: entirely fixed text for this error
/// class, naming neither the stage, the method scope nor the setting the patch carried.
const ROLE_REQUIRED: &str =
    "CloudWatch Logs role ARN must be set in account settings to enable logging";

/// One row: the case name, the `patchOperations` it sends, and the `methodSettings` entries
/// the stage must report once the patch is accepted — each the key the path addressed, the
/// `MethodSetting` member it writes and that member's value.
type GatedRow = (
    &'static str,
    Vec<Value>,
    Vec<(&'static str, &'static str, Value)>,
);

/// One `replace` of `path` with `value`.
fn replace(path: &str, value: &str) -> Value {
    json!({ "op": "replace", "path": path, "value": value })
}

/// `UpdateStage` for `stage_name` of `api` with `operations`.
async fn update_stage(
    gateway: &Gateway,
    api: &str,
    stage_name: &str,
    operations: &[Value],
) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{stage_name}"),
            Some(json!({ "patchOperations": operations })),
        )
        .await
}

/// An API deployed to [`STAGE`] and to [`OTHER_STAGE`], answering the API id.
async fn deployed_api(gateway: &Gateway, name: &str) -> String {
    let (api, _root) = create_api(gateway, name).await;
    let deployment = create_deployed_stage(gateway, &api, STAGE).await;
    let other = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({ "stageName": OTHER_STAGE, "deploymentId": deployment })),
        )
        .await;
    assert_eq!(
        other.status, 201,
        "case CreateStage({OTHER_STAGE}) for {name}: expected 201, got {} with {:?}",
        other.status, other.body
    );
    api
}

/// Assert `answer` is the refusal of this error class: the modelled status and code, and
/// the fixed sentence of [`ROLE_REQUIRED`].
fn assert_role_required(answer: &Answer, case: &str) {
    assert_failure(answer, 400, "BadRequestException", case);
    assert_eq!(
        answer.body["message"].as_str(),
        Some(ROLE_REQUIRED),
        "case {case}: expected the account-role sentence, got {:?}",
        answer.body["message"]
    );
}

/// The account's role as `GetAccount` reports it.
async fn reported_role(gateway: &Gateway) -> Value {
    get_account(gateway).await[CLOUDWATCH_ROLE_PATH
        .strip_prefix('/')
        .expect("the pointer names a member")]
    .clone()
}

/// Every patch that turns execution logging ON, in both spellings of both settings.
///
/// Source for the spellings: `MethodSetting$loggingLevel` and
/// `MethodSetting$dataTraceEnabled` (`smithy_query(query="shape", shape="MethodSetting")`)
/// for the member names, and API Gateway's published patch paths for a stage's method
/// settings (`/{method_setting_key}/logging/loglevel`, `/logging/dataTrace`) for the
/// grouped names. `loggingLevel` is documented with "Valid values are OFF, ERROR, and
/// INFO", so the levels that turn logging on are ERROR and INFO — both are rows, under the
/// stage-wide wildcard and under a named method.
fn gated_rows() -> Vec<GatedRow> {
    vec![
        (
            "every method of the stage at INFO",
            vec![replace("/*/*/logging/loglevel", "INFO")],
            vec![("*/*", "loggingLevel", json!("INFO"))],
        ),
        (
            "every method of the stage at ERROR",
            vec![replace("/*/*/logging/loglevel", "ERROR")],
            vec![("*/*", "loggingLevel", json!("ERROR"))],
        ),
        (
            "one named method at INFO",
            vec![replace("/~1pets/GET/logging/loglevel", "INFO")],
            vec![("~1pets/GET", "loggingLevel", json!("INFO"))],
        ),
        (
            "a level addressed by its member name",
            vec![replace("/~1pets/POST/loggingLevel", "ERROR")],
            vec![("~1pets/POST", "loggingLevel", json!("ERROR"))],
        ),
        (
            "a level under the logLevel spelling of its group",
            vec![replace("/*/*/logging/logLevel", "INFO")],
            vec![("*/*", "loggingLevel", json!("INFO"))],
        ),
        (
            "an add rather than a replace of the level",
            vec![json!({ "op": "add", "path": "/*/*/logging/loglevel", "value": "INFO" })],
            vec![("*/*", "loggingLevel", json!("INFO"))],
        ),
        (
            "the data trace of every method of the stage",
            vec![replace("/*/*/logging/dataTrace", "true")],
            vec![("*/*", "dataTraceEnabled", json!(true))],
        ),
        (
            "the data trace addressed by its member name",
            vec![replace("/~1pets/DELETE/dataTraceEnabled", "true")],
            vec![("~1pets/DELETE", "dataTraceEnabled", json!(true))],
        ),
        (
            "both logging settings of one method at once",
            vec![
                replace("/~1pets/PUT/logging/loglevel", "INFO"),
                replace("/~1pets/PUT/logging/dataTrace", "true"),
            ],
            vec![
                ("~1pets/PUT", "loggingLevel", json!("INFO")),
                ("~1pets/PUT", "dataTraceEnabled", json!(true)),
            ],
        ),
        (
            "a list whose other op the stage would have applied",
            vec![
                replace("/*/*/metrics/enabled", "true"),
                replace("/*/*/logging/loglevel", "INFO"),
            ],
            vec![
                ("*/*", "metricsEnabled", json!(true)),
                ("*/*", "loggingLevel", json!("INFO")),
            ],
        ),
    ]
}

#[tokio::test]
async fn turning_execution_logging_on_needs_the_accounts_cloudwatch_role() {
    for (index, (case, operations, expected)) in gated_rows().into_iter().enumerate() {
        let gateway = Gateway::new();
        let api = deployed_api(&gateway, &format!("logging-role-{index}")).await;

        // 1. The account nobody has configured carries no role, so the configuration is
        //    refused — and the refused patch writes NOTHING: the stage, every op of the
        //    list included, reads back exactly as it did before.
        assert_eq!(
            reported_role(&gateway).await,
            Value::Null,
            "case {case}: expected no account role before one is set"
        );
        let before = get_stage(&gateway, &api, STAGE).await;
        let refused = update_stage(&gateway, &api, STAGE, &operations).await;
        assert_role_required(&refused, case);
        let after = get_stage(&gateway, &api, STAGE).await;
        assert_eq!(
            after.body, before.body,
            "case {case}: expected the refused patch to leave the stage at {:?}, got {:?}",
            before.body, after.body
        );

        // 2. With the account's role established the very same configuration applies, and
        //    the settings it wrote read back through an independent GetStage.
        let configured = role_arn(&format!("apigw-logs-{index}"));
        patch_account(
            &gateway,
            "replace",
            CLOUDWATCH_ROLE_PATH,
            Some(&configured),
        )
        .await;
        let applied = update_stage(&gateway, &api, STAGE, &operations).await;
        assert_eq!(
            applied.status, 200,
            "case {case}: expected 200 from UpdateStage once the role is set, got {} with {:?}",
            applied.status, applied.body
        );
        let read = get_stage(&gateway, &api, STAGE).await;
        assert_eq!(
            read.body, applied.body,
            "case {case}: GetStage must report what UpdateStage answered, got {:?} after {:?}",
            read.body, applied.body
        );
        for (key, member, value) in &expected {
            let actual = &read.body["methodSettings"][*key][*member];
            assert_eq!(
                actual, value,
                "case {case}: expected {member} under {key} to be {value:?}, got {actual:?}"
            );
        }

        // 3. The role is the account's CURRENT setting, not a one-off: cleared again, the
        //    same configuration on a stage that was never gated is refused once more, and
        //    that stage is left untouched too.
        patch_account(&gateway, "replace", CLOUDWATCH_ROLE_PATH, Some("")).await;
        assert_eq!(
            reported_role(&gateway).await,
            Value::Null,
            "case {case}: expected the cleared account to report no role"
        );
        let other_before = get_stage(&gateway, &api, OTHER_STAGE).await;
        let refused_again = update_stage(&gateway, &api, OTHER_STAGE, &operations).await;
        assert_role_required(&refused_again, case);
        let other_after = get_stage(&gateway, &api, OTHER_STAGE).await;
        assert_eq!(
            other_after.body, other_before.body,
            "case {case}: expected the second stage to be left at {:?}, got {:?}",
            other_before.body, other_after.body
        );
    }
}

#[tokio::test]
async fn a_configuration_that_enables_no_execution_logging_needs_no_account_role() {
    // The settings and members a role-less account places NO condition on: logging turned
    // explicitly off, the data trace turned off, the level removed again, every method
    // setting that emits no execution log events (metrics, throttling, caching — the other
    // members of `MethodSetting`), and a member of the stage itself.
    let rows: Vec<(&str, Vec<Value>, Vec<&str>, Value)> = vec![
        (
            "logging explicitly off",
            vec![replace("/*/*/logging/loglevel", "OFF")],
            vec!["methodSettings", "*/*", "loggingLevel"],
            json!("OFF"),
        ),
        (
            "logging off by its member name",
            vec![replace("/*/*/loggingLevel", "OFF")],
            vec!["methodSettings", "*/*", "loggingLevel"],
            json!("OFF"),
        ),
        (
            "the data trace turned off",
            vec![replace("/*/*/logging/dataTrace", "false")],
            vec!["methodSettings", "*/*", "dataTraceEnabled"],
            json!(false),
        ),
        (
            "a level set to off and then removed",
            vec![
                replace("/*/*/logging/loglevel", "OFF"),
                json!({ "op": "remove", "path": "/*/*/logging/loglevel" }),
            ],
            vec!["methodSettings", "*/*", "loggingLevel"],
            Value::Null,
        ),
        (
            "the CloudWatch metrics of every method",
            vec![replace("/*/*/metrics/enabled", "true")],
            vec!["methodSettings", "*/*", "metricsEnabled"],
            json!(true),
        ),
        (
            "a throttle burst limit",
            vec![replace("/~1pets/GET/throttling/burstLimit", "42")],
            vec!["methodSettings", "~1pets/GET", "throttlingBurstLimit"],
            json!(42),
        ),
        (
            "method caching with a TTL",
            vec![
                replace("/*/*/caching/enabled", "true"),
                replace("/*/*/caching/ttlInSeconds", "60"),
            ],
            vec!["methodSettings", "*/*", "cacheTtlInSeconds"],
            json!(60),
        ),
        (
            "the stage's access log destination",
            vec![
                replace(
                    "/accessLogSettings/destinationArn",
                    "arn:aws:logs:us-east-1:000000000000:log-group:api-access",
                ),
                replace("/accessLogSettings/format", "$context.requestId"),
            ],
            vec!["accessLogSettings", "format"],
            json!("$context.requestId"),
        ),
        (
            "a member of the stage itself",
            vec![replace("/description", "no logging wired")],
            vec!["description"],
            json!("no logging wired"),
        ),
    ];

    for (index, (case, operations, keys, expected)) in rows.into_iter().enumerate() {
        let gateway = Gateway::new();
        let api = deployed_api(&gateway, &format!("no-logging-{index}")).await;
        assert_eq!(
            reported_role(&gateway).await,
            Value::Null,
            "case {case}: expected no account role for this row"
        );

        let applied = update_stage(&gateway, &api, STAGE, &operations).await;
        assert_eq!(
            applied.status, 200,
            "case {case}: expected 200 from UpdateStage on a role-less account, got {} with {:?}",
            applied.status, applied.body
        );
        let read = get_stage(&gateway, &api, STAGE).await;
        let mut actual = &read.body;
        for key in &keys {
            actual = &actual[*key];
        }
        assert_eq!(
            *actual, expected,
            "case {case}: expected {keys:?} to be {expected:?}, got {actual:?}"
        );
        // The stage still serves the deployment it was created for: this patch is a
        // configuration of settings, not a redeployment.
        let served = string_member(&read.body, "deploymentId", case);
        assert!(
            !served.is_empty(),
            "case {case}: expected the stage to still name its deployment, got {:?}",
            read.body
        );
    }
}
