// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the account read a caller performs BEFORE it can gate a stage's
//! execution logs: `GetAccount`, beside the `UpdateAccount` that establishes the
//! account's CloudWatch role and the `UpdateStage` that sets the per-method
//! `loggingLevel` / `dataTrace` the gating is expressed with.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `PutMethod` / `CreateDeployment` / `GetStage` / `UpdateAccount`
//! helpers are this crate's existing harness, reused from `rest_api_lifecycle`,
//! `method_integration`, `stage_lifecycle` and `account_settings`.
//!
//! The RULE under test: a REPORTING operation of the account's own settings answers its
//! modelled success — HTTP 200 with the `Account` representation, never a status or an
//! error code outside the operation's modelled set — on EVERY state of the account
//! record and at every point of a configuration flow, and nothing a caller does to
//! another record of the service (an API, a deployment, a stage's method settings)
//! changes what it reports.
//!
//! The rows span what that generalises over: the whole documented `loggingLevel` value
//! set, the data trace on and off, the wildcard method scope and a named one, several
//! HTTP verbs, and the account read taken before the role is set, after it is set, and
//! after the stage has been gated. A level outside the documented set is refused, and the
//! refusal leaves both the stage and the account exactly as they were.

use serde_json::{json, Value};

use crate::account_settings::{patch_account, role_arn};
use crate::method_integration::resource_with_a_method;
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// The stage every row deploys its API to.
const STAGE: &str = "prod";

/// The JSON Pointer of the account member a caller wiring execution logging sets.
const CLOUDWATCH_ROLE_PATH: &str = "/cloudwatchRoleArn";

/// One gating case: the case name, the verb the method is put on, the method path an
/// `UpdateStage` patch addresses, the `methodSettings` key the stage reports that path
/// under (the patch path without its leading slash, JSON Pointer escapes included — the
/// key API Gateway reports a per-method setting under), the `loggingLevel` the row gates
/// execution logs at, and whether it asks for the data trace.
type GatingCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    bool,
);

/// `GetAccount` through the wire, asserting the operation's MODELLED success and
/// answering the `Account` body.
///
/// Deliberately stricter than `account_settings::get_account`, which asserts the status
/// only: this suite is where the account read's answer is held against the operation's
/// modelled error set, so it also asserts that no error code is reported at all — a
/// `501` carrying an error code the model never declares for this operation is exactly
/// the answer no client error branch can match.
async fn account(gateway: &Gateway, case: &str) -> Value {
    let answer: Answer = gateway.call("GET", "/account", None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from GetAccount, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error, None,
        "case {case:?}: expected GetAccount to report no error code, got {:?} with {:?}",
        answer.error, answer.body
    );
    assert!(
        answer.body.is_object(),
        "case {case:?}: expected the Account representation, got {:?}",
        answer.body
    );
    answer.body
}

/// An API with one resource carrying `verb`, deployed to [`STAGE`] — what a caller has to
/// have before a method setting can gate anything.
async fn deployed_api(gateway: &Gateway, name: &str, verb: &str) -> String {
    let (api, root) = create_api(gateway, name).await;
    let _resource = resource_with_a_method(gateway, &api, &root, "pets", verb, json!({})).await;
    let _deployment = create_deployment(
        gateway,
        &api,
        &format!("CreateDeployment({name})"),
        json!({ "stageName": STAGE }),
    )
    .await;
    api
}

/// `UpdateStage` gating the execution logs of `method_path` at `level`, with the data
/// trace on or off — the two `MethodSetting` members a caller drives execution logging
/// with.
async fn gate_execution_logs(
    gateway: &Gateway,
    api: &str,
    method_path: &str,
    level: &str,
    data_trace: bool,
) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{STAGE}"),
            Some(json!({
                "patchOperations": [
                    { "op": "replace", "path": format!("{method_path}/logging/loglevel"),
                      "value": level },
                    { "op": "replace", "path": format!("{method_path}/logging/dataTrace"),
                      "value": data_trace.to_string() }
                ]
            })),
        )
        .await
}

#[tokio::test]
async fn the_account_reports_its_cloudwatch_role_at_every_point_of_gating_execution_logs() {
    // The documented `loggingLevel` value set is the catalog these rows span:
    // `MethodSetting$loggingLevel` — "Valid values are OFF, ERROR, and INFO."
    // (smithy_query(query="shape",shape="MethodSetting")). `OFF` and `ERROR` are rows the
    // recorded case never drove; the data trace is exercised both ways on each, over the
    // wildcard scope and a named method of several verbs.
    let cases: &[GatingCase] = &[
        ("every method of the stage, logging off", "GET", "/*/*", "*/*", "OFF", false),
        ("every method of the stage at ERROR, data trace on", "GET", "/*/*", "*/*", "ERROR", true),
        ("every method of the stage at INFO, data trace on", "GET", "/*/*", "*/*", "INFO", true),
        ("every method of the stage at INFO, data trace off", "GET", "/*/*", "*/*", "INFO", false),
        (
            "one named method at INFO, data trace off",
            "GET",
            "/~1pets/GET",
            "~1pets/GET",
            "INFO",
            false,
        ),
        (
            "one named method at ERROR, data trace on",
            "POST",
            "/~1pets/POST",
            "~1pets/POST",
            "ERROR",
            true,
        ),
        (
            "one named method with logging off",
            "DELETE",
            "/~1pets/DELETE",
            "~1pets/DELETE",
            "OFF",
            false,
        ),
    ];

    for (case, verb, method_path, reported_key, level, data_trace) in cases {
        let gateway = Gateway::new();

        // 1. The read a caller takes FIRST, on an account nobody has configured: the
        //    answer is the Account representation, and the absent role is how it reports
        //    that no CloudWatch role is wired yet.
        let unconfigured = account(&gateway, case).await;
        assert_eq!(
            unconfigured["cloudwatchRoleArn"],
            Value::Null,
            "case {case:?}: expected no cloudwatchRoleArn before one is set, got \
             {unconfigured:?}"
        );

        // 2. Establishing the role, and reading it back: what the update reported is what
        //    the read reports.
        let expected_role = role_arn(&format!("apigw-logs-{level}"));
        let updated = patch_account(
            &gateway,
            "replace",
            CLOUDWATCH_ROLE_PATH,
            Some(&expected_role),
        )
        .await;
        let configured = account(&gateway, case).await;
        assert_eq!(
            configured["cloudwatchRoleArn"],
            json!(expected_role),
            "case {case:?}: expected GetAccount to read back the role UpdateAccount \
             reported {updated:?}, got {configured:?}"
        );
        assert_eq!(
            configured, updated,
            "case {case:?}: expected the read to report the body UpdateAccount reported \
             {updated:?}, got {configured:?}"
        );

        // 3. The stage whose execution logs the method setting gates.
        let api = deployed_api(&gateway, &format!("gated-{verb}-{level}-{data_trace}"), verb).await;
        let gated = gate_execution_logs(&gateway, &api, method_path, level, *data_trace).await;
        assert_eq!(
            gated.status, 200,
            "case {case:?}: expected 200 from UpdateStage, got {} with {:?}",
            gated.status, gated.body
        );

        // 4. The gating settings the update reported, and the ones a later GetStage reads
        //    back: the two render one record, so they cannot drift.
        let read_back = get_stage(&gateway, &api, STAGE).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected 200 from GetStage, got {} with {:?}",
            read_back.status, read_back.body
        );
        for (source, body) in [("UpdateStage", &gated.body), ("GetStage", &read_back.body)] {
            let settings = &body["methodSettings"][*reported_key];
            assert_eq!(
                settings["loggingLevel"],
                json!(level),
                "case {case:?}: expected {source} to report loggingLevel {level:?} under \
                 {reported_key:?}, got {body:?}"
            );
            assert_eq!(
                settings["dataTraceEnabled"],
                json!(data_trace),
                "case {case:?}: expected {source} to report dataTraceEnabled \
                 {data_trace:?} under {reported_key:?}, got {body:?}"
            );
        }

        // 5. And the account read AFTER the whole flow: a reporting operation, so the
        //    stage work changed nothing it reports.
        let after = account(&gateway, case).await;
        assert_eq!(
            after, configured,
            "case {case:?}: expected the account read after gating to report {configured:?}, \
             got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_logging_level_outside_the_documented_set_is_refused_and_gates_nothing() {
    let gateway = Gateway::new();
    let expected_role = role_arn("apigw-logs");
    patch_account(
        &gateway,
        "replace",
        CLOUDWATCH_ROLE_PATH,
        Some(&expected_role),
    )
    .await;
    let configured = account(&gateway, "the account the refusals run against").await;
    let api = deployed_api(&gateway, "refused-levels", "GET").await;
    let before = get_stage(&gateway, &api, STAGE).await;

    // `MethodSetting$loggingLevel` documents exactly OFF, ERROR and INFO, so every value
    // outside that set is refused. The sibling `update_stage` suite already pins the
    // refusal itself for one such value; these rows are the part that belongs here — that
    // a refused gating patch leaves BOTH the stage and the account read exactly as they
    // were — over the shapes a caller most easily reaches for: a level another logging
    // surface has, a case variant of a documented one, and no value at all.
    let rows: &[(&str, &str)] = &[
        ("a level no API Gateway stage has", "DEBUG"),
        ("a lower-case spelling of a documented level", "info"),
        ("the empty string", ""),
    ];
    for (case, level) in rows {
        let refused = gate_execution_logs(&gateway, &api, "/*/*", level, true).await;
        assert_failure(&refused, 400, "BadRequestException", case);

        // The refused patch wrote NOTHING: no method setting was created for the scope it
        // named, so the stage reads back exactly as it did before.
        let after = get_stage(&gateway, &api, STAGE).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?} ({level:?}): expected the stage to be untouched {:?}, got {:?}",
            before.body, after.body
        );

        // And the account read is still the operation's modelled success, reporting the
        // role the caller established.
        let still = account(&gateway, case).await;
        assert_eq!(
            still, configured,
            "case {case:?} ({level:?}): expected the account to still report {configured:?}, \
             got {still:?}"
        );
    }
}
