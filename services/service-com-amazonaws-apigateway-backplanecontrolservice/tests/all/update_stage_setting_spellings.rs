// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SPELLING of an `UpdateStage` method-setting path: the
//! setting's published group-and-name (`/~1pets/GET/metrics/enabled`) and the
//! `MethodSetting` member that names the same setting in one reference token
//! (`/~1pets/GET/metricsEnabled`).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the request builder and the
//! `CreateRestApi` / `CreateDeployment` / `GetStage` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle` and `stage_lifecycle`.
//!
//! The rule under test: a method-setting path spells the method either in TWO reference
//! tokens (`/~1pets/GET/…`) or in one (`/~1pets~1GET/…`), and the setting may be addressed
//! by its published name or by its modelled member name — both write the SAME member of
//! the entry the method is keyed by, which is the method path as the pointer spelled it
//! where it took two tokens and the decoded path where it took one. A path that leaves no
//! setting beyond the method, or whose tail names none, is refused and changes nothing.
//!
//! Asserted over a TABLE rather than one recorded call: every setting of the
//! `MethodSetting` shape in both spellings, each with a value that differs from the one a
//! created entry carries, and the key forms a client may spell (an escaped resource path,
//! the root resource, a `~1`-escaped key, the `*/*` wildcard).

use serde_json::{json, Value};

use crate::account_settings::{patch_account, role_arn};
use crate::rest_api_lifecycle::{assert_failure, create_api, Answer, Gateway};
use crate::stage_lifecycle::{create_deployment, get_stage};

/// One row of the setting catalog: the setting's published name, its `MethodSetting`
/// member name, the value a patch sets it to and the JSON the stage must then report.
///
/// Source: the `MethodSetting` shape (`smithy_query(query="shape",
/// shape="MethodSetting")`) for the member names, and API Gateway's published
/// patch-operations paths for a stage's method settings for the grouped names. Every
/// value differs from the one a created entry carries, so a row cannot pass by accident.
type SettingRow = (&'static str, &'static str, &'static str, Value);

fn setting_catalog() -> Vec<SettingRow> {
    vec![
        ("metrics/enabled", "metricsEnabled", "true", json!(true)),
        ("logging/loglevel", "loggingLevel", "INFO", json!("INFO")),
        (
            "logging/dataTrace",
            "dataTraceEnabled",
            "true",
            json!(true),
        ),
        (
            "throttling/burstLimit",
            "throttlingBurstLimit",
            "77",
            json!(77),
        ),
        (
            "throttling/rateLimit",
            "throttlingRateLimit",
            "12.5",
            json!(12.5),
        ),
        ("caching/enabled", "cachingEnabled", "true", json!(true)),
        (
            "caching/ttlInSeconds",
            "cacheTtlInSeconds",
            "120",
            json!(120),
        ),
        (
            "caching/dataEncrypted",
            "cacheDataEncrypted",
            "true",
            json!(true),
        ),
        (
            "caching/requireAuthorizationForCacheControl",
            "requireAuthorizationForCacheControl",
            "false",
            json!(false),
        ),
        (
            "caching/unauthorizedCacheControlHeaderStrategy",
            "unauthorizedCacheControlHeaderStrategy",
            "FAIL_WITH_403",
            json!("FAIL_WITH_403"),
        ),
    ]
}

/// `UpdateStage` for `stage_name` of `api`, replacing `path` with `value`.
async fn replace(
    gateway: &Gateway,
    api: &str,
    stage_name: &str,
    path: &str,
    value: &str,
) -> Answer {
    gateway
        .call(
            "PATCH",
            &format!("/restapis/{api}/stages/{stage_name}"),
            Some(json!({
                "patchOperations": [{ "op": "replace", "path": path, "value": value }]
            })),
        )
        .await
}

/// `UpdateStage` for `stage_name`, asserting the modelled 200 and that `GetStage` reads
/// the very same record back, and answering the body.
async fn replace_ok(
    gateway: &Gateway,
    api: &str,
    stage_name: &str,
    case: &str,
    path: &str,
    value: &str,
) -> Value {
    let answer = replace(gateway, api, stage_name, path, value).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected the modelled 200 from UpdateStage, got {} with {:?}",
        answer.status, answer.body
    );
    let read = get_stage(gateway, api, stage_name).await;
    assert_eq!(
        read.body, answer.body,
        "case {case}: GetStage must report what UpdateStage answered, got {:?} after {:?}",
        read.body, answer.body
    );
    answer.body
}

/// An API with one deployment served by a stage named `prod`.
async fn deployed_stage(gateway: &Gateway, name: &str) -> String {
    let (api, _) = create_api(gateway, name).await;
    create_deployment(
        gateway,
        &api,
        &format!("CreateDeployment for {name}"),
        json!({ "stageName": "prod" }),
    )
    .await;
    api
}

#[tokio::test]
async fn either_spelling_of_a_setting_writes_the_same_method_setting() {
    let gateway = Gateway::new();
    let api = deployed_stage(&gateway, "setting-spellings").await;
    // The catalog carries the two settings that turn a method's execution logging on,
    // which API Gateway writes under the ACCOUNT's CloudWatch Logs role, so the account
    // carries one before the table runs — the precondition the sibling
    // `stage_execution_logging_account_role` suite covers.
    patch_account(
        &gateway,
        "replace",
        "/cloudwatchRoleArn",
        Some(&role_arn("setting-spellings")),
    )
    .await;

    for (published, member, value, expected) in setting_catalog() {
        // One method path per spelling, so each row proves which setting the spelling
        // wrote rather than merely that something changed. The key is the method path as
        // the pointer spelled it.
        let published_key = format!("~1{member}-published/GET");
        let member_key = format!("~1{member}-member/GET");
        for (spelling, path, key) in [
            (
                published,
                format!("/~1{member}-published/GET/{published}"),
                &published_key,
            ),
            (
                member,
                format!("/~1{member}-member/GET/{member}"),
                &member_key,
            ),
        ] {
            let case = format!("UpdateStage(replace {path} = {value})");
            let body = replace_ok(&gateway, &api, "prod", &case, &path, value).await;
            assert_eq!(
                body["methodSettings"][key.as_str()][member], expected,
                "case {case}: the {spelling} spelling must write {member} as {expected:?} under {key}, got {:?}",
                body["methodSettings"][key.as_str()]
            );
        }

        // The two spellings are the same setting of the same shape: the entries differ
        // only in the key the request addressed them under.
        let read = get_stage(&gateway, &api, "prod").await;
        assert_eq!(
            read.body["methodSettings"][published_key.as_str()],
            read.body["methodSettings"][member_key.as_str()],
            "case {member}: both spellings must leave the same MethodSetting, got {:?} and {:?}",
            read.body["methodSettings"][published_key.as_str()],
            read.body["methodSettings"][member_key.as_str()]
        );
    }
}

#[tokio::test]
async fn a_setting_addressed_by_its_member_name_is_keyed_by_the_method_it_names() {
    // Rows differ along how a client spells the method path: a child resource and a nested
    // one with the resource path's slashes escaped and the method in its own token, the
    // root resource, the whole method path inside ONE token (which is keyed decoded), and
    // the `*/*` wildcard `Stage$methodSettings` documents for every method of the stage.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a child resource", "/~1pets/GET/metricsEnabled", "~1pets/GET"),
        ("the root resource", "/~1/GET/metricsEnabled", "~1/GET"),
        (
            "a nested resource path",
            "/~1pets~1{petId}/GET/metricsEnabled",
            "~1pets~1{petId}/GET",
        ),
        (
            "the whole method path in one token",
            "/~1pets~1POST/metricsEnabled",
            "/pets/POST",
        ),
        ("every method", "/*/*/metricsEnabled", "*/*"),
    ];

    let gateway = Gateway::new();
    let api = deployed_stage(&gateway, "setting-member-keys").await;

    let mut expected_keys: Vec<String> = Vec::new();
    for (case, path, key) in cases {
        let case = format!("{case} / UpdateStage(replace {path})");
        let body = replace_ok(&gateway, &api, "prod", &case, path, "true").await;
        assert_eq!(
            body["methodSettings"][key]["metricsEnabled"],
            json!(true),
            "case {case}: expected metricsEnabled under {key}, got {:?}",
            body["methodSettings"]
        );
        // Every path patched so far is still there, and nothing else is: one patch writes
        // one entry of the map.
        expected_keys.push(key.to_string());
        expected_keys.sort();
        let mut reported: Vec<String> = body["methodSettings"]
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        reported.sort();
        assert_eq!(
            reported, expected_keys,
            "case {case}: expected the keys {expected_keys:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_tail_that_names_no_setting_is_refused_and_changes_nothing() {
    let gateway = Gateway::new();
    let api = deployed_stage(&gateway, "setting-spelling-refusals").await;
    replace_ok(
        &gateway,
        &api,
        "prod",
        "UpdateStage(the baseline)",
        "/~1pets/GET/metricsEnabled",
        "true",
    )
    .await;
    let baseline = get_stage(&gateway, &api, "prod").await.body;

    // Rows span the ways a method-setting path can name no setting of a method: a member
    // name that is not one, a published name whose group and name do not go together, a
    // path that stops at the method, a setting with no method path before it, and a
    // one-token method path followed by a GROUPED setting name, which is the spelling where
    // the tokens beyond the method name no setting at all.
    let cases: Vec<(&str, &str)> = vec![
        ("a member name no setting has", "/~1pets/GET/bogusSetting"),
        ("a group that has no such name", "/~1pets/GET/metrics/bogus"),
        ("a name under the wrong group", "/~1pets/GET/metrics/dataTrace"),
        ("a path that stops at the method", "/~1pets/GET"),
        ("a setting with no method path", "/metricsEnabled"),
        ("a published name with no method path", "/metrics/enabled"),
        (
            "a one-token method path with a grouped setting name",
            "/~1pets~1GET/throttling/rateLimit",
        ),
        (
            "a one-token method path with no setting at all",
            "/~1pets~1GET",
        ),
    ];

    for (case, path) in cases {
        let case = format!("{case} / UpdateStage(replace {path})");
        let answer = replace(&gateway, &api, "prod", path, "true").await;
        assert_failure(&answer, 400, "BadRequestException", &case);
    }

    let after = get_stage(&gateway, &api, "prod").await;
    assert_eq!(
        after.body, baseline,
        "case GetStage after the refusals: a refused patch must change nothing, got {:?} after {:?}",
        after.body, baseline
    );
}
