// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `UpdateCustomAction` and the read that proves what it persisted
//! (`GetCustomAction`), over the wire (restJson1) through the same `MultiServiceRouter`
//! the production router serves with.
//!
//! The rule every assertion here is about: an update of a custom action carries the
//! action's WHOLE mutable state rather than a patch — `Definition` is required even though
//! the stored action already has one — so the stored action's definition, alias and
//! attachments afterwards are exactly the members the request carried, an omitted optional
//! member being cleared; what the ARN fixes (the action's name) and the tags, which only
//! the tagging operations write, are left alone; an ARN of the form this account's actions
//! are minted under that holds no action is refused with the modelled 404
//! `ResourceNotFoundException`, and an ARN no create can mint for this account with the
//! modelled 403 `UnauthorizedException` (the same pair
//! `tests/all/custom_action_fail_closed.rs` asserts for the other custom-action-addressed
//! operations).
//!
//! The tables span the class rather than one recorded request: the rows differ in which
//! optional members the update carries, in how many attachments, criteria and variables
//! they hold, in the criteria operator, and in the length and charset of the alias and the
//! command text — including each published bound of those shapes.

use serde_json::{json, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

/// The modelled error an ARN of the minted form holding no custom action is refused with.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The modelled error an ARN no custom action of this account can be minted under is refused
/// with.
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";

/// A custom action is account-scoped, so the ARN a create mints carries an EMPTY region
/// segment (`state::custom_action_arn`).
fn custom_action_arn(action_name: &str) -> String {
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// The same ARN with a region segment filled in — a well-formed `CustomActionArn` that no
/// create ever mints.
fn custom_action_arn_in_region(region: &str, action_name: &str) -> String {
    format!("arn:aws:chatbot:{region}:{ACCOUNT}:custom-action/{action_name}")
}

/// The create every update row starts from: it carries EVERY optional member, so a row
/// that leaves one out shows the update clearing it rather than inheriting it.
fn baseline_create(action_name: &str) -> Value {
    json!({
        "ActionName": action_name,
        "Definition": {"CommandText": "ssm start-session --target $InstanceId"},
        "AliasName": "start-session",
        "Attachments": [{
            "NotificationType": "aws-cloudwatch-alarm",
            "ButtonText": "Start a session",
            "Criteria": [{"Operator": "HAS_VALUE", "VariableName": "InstanceId"}],
            "Variables": {"InstanceId": "$.detail.instanceId"},
        }],
        "Tags": [{"TagKey": "team", "TagValue": "ops"}],
    })
}

/// Create the baseline action `action_name` and answer its ARN.
async fn create_baseline(emulator: &Emulator, action_name: &str) -> String {
    let created = emulator
        .call("/create-custom-action", &baseline_create(action_name))
        .await;
    assert_eq!(
        created.status, 200,
        "creating {action_name:?} answers 200, got {} with {}",
        created.status, created.body
    );
    created.body["CustomActionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {action_name:?}: no CustomActionArn in {}", created.body))
        .to_string()
}

/// Read the action `arn` back, asserting the read succeeded, and answer the `CustomAction`
/// it reports.
async fn get_ok(emulator: &Emulator, arn: &str) -> Value {
    let answer = emulator
        .call("/get-custom-action", &json!({"CustomActionArn": arn}))
        .await;
    assert_eq!(
        answer.status, 200,
        "reading {arn} answers 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["CustomAction"].clone()
}

/// One row of the update table: the members the update carries beside `CustomActionArn`,
/// and the definition/alias/attachments the stored action must hold afterwards.
struct UpdateCase {
    label: &'static str,
    /// The request body members other than `CustomActionArn`.
    update: Value,
    /// The members of the `CustomAction` the update decides — everything the stored action
    /// must report besides `CustomActionArn` and `ActionName`.
    expected: Value,
}

/// The update cases, each differing from the others along what the update rule generalises
/// over: which optional members the request carries, how many attachments it defines, how
/// many criteria and variables each attachment holds, which criteria operator it names,
/// and the length and charset of the alias and of the command text.
///
/// The operators are the model's whole published `CustomActionAttachmentCriteriaOperator`
/// value set (`smithy_query`: `HAS_VALUE`, `EQUALS`); the bounds the rows sit on are the
/// published maxima of `CustomActionAttachmentCriteriaList` and
/// `CustomActionAttachmentVariables` (len 1..5), of `CustomActionAliasName` (len 1..30) and
/// of `CustomActionDefinition$CommandText` (len 1..100).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "only the required Definition: the alias and the attachments are cleared",
            update: json!({"Definition": {"CommandText": "s3 ls"}}),
            expected: json!({"Definition": {"CommandText": "s3 ls"}}),
        },
        UpdateCase {
            label: "a new definition and a new alias, no attachments",
            update: json!({
                "Definition": {"CommandText": "lambda invoke --function-name $Function"},
                "AliasName": "run-lambda",
            }),
            expected: json!({
                "Definition": {"CommandText": "lambda invoke --function-name $Function"},
                "AliasName": "run-lambda",
            }),
        },
        UpdateCase {
            label: "attachments but no alias, with one EQUALS criterion",
            update: json!({
                "Definition": {"CommandText": "cloudwatch describe-alarms"},
                "Attachments": [{
                    "NotificationType": "aws-cloudwatch-alarm",
                    "ButtonText": "Describe alarms",
                    "Criteria": [
                        {"Operator": "EQUALS", "VariableName": "State", "Value": "ALARM"},
                    ],
                    "Variables": {"State": "$.detail.state.value"},
                }],
            }),
            expected: json!({
                "Definition": {"CommandText": "cloudwatch describe-alarms"},
                "Attachments": [{
                    "NotificationType": "aws-cloudwatch-alarm",
                    "ButtonText": "Describe alarms",
                    "Criteria": [
                        {"Operator": "EQUALS", "VariableName": "State", "Value": "ALARM"},
                    ],
                    "Variables": {"State": "$.detail.state.value"},
                }],
            }),
        },
        UpdateCase {
            label: "every optional member, two attachments, the criteria list and the \
                    variable map at their maxima",
            update: json!({
                "Definition": {"CommandText": "codedeploy stop-deployment --deployment-id $Id"},
                "AliasName": "stop-deploy",
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
                    {"ButtonText": "Open event"},
                ],
            }),
            expected: json!({
                "Definition": {"CommandText": "codedeploy stop-deployment --deployment-id $Id"},
                "AliasName": "stop-deploy",
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
                    {"ButtonText": "Open event"},
                ],
            }),
        },
        UpdateCase {
            label: "the longest allowed alias and command text, and an empty attachment list",
            update: json!({
                "Definition": {"CommandText": "c".repeat(100)},
                "AliasName": "A-_".repeat(10),
                "Attachments": [],
            }),
            expected: json!({
                "Definition": {"CommandText": "c".repeat(100)},
                "AliasName": "A-_".repeat(10),
                "Attachments": [],
            }),
        },
        UpdateCase {
            label: "the shortest allowed alias and command text",
            update: json!({
                "Definition": {"CommandText": "s"},
                "AliasName": "a",
            }),
            expected: json!({
                "Definition": {"CommandText": "s"},
                "AliasName": "a",
            }),
        },
        UpdateCase {
            label: "the definition the create carried, with another alias",
            update: json!({
                "Definition": {"CommandText": "ssm start-session --target $InstanceId"},
                "AliasName": "session",
            }),
            expected: json!({
                "Definition": {"CommandText": "ssm start-session --target $InstanceId"},
                "AliasName": "session",
            }),
        },
    ]
}

/// The whole `CustomAction` the stored action must report after `case` was applied to the
/// action `arn` named `action_name`.
fn expected_action(case: &UpdateCase, arn: &str, action_name: &str) -> Value {
    let mut expected = case.expected.clone();
    expected["CustomActionArn"] = json!(arn);
    expected["ActionName"] = json!(action_name);
    expected
}

#[tokio::test]
async fn an_update_replaces_the_actions_mutable_state_with_what_the_request_carries() {
    for case in update_cases() {
        // A fresh store per row, so every row updates the same baseline action.
        let emulator = Emulator::new();
        let action_name = "updated-action";
        let arn = create_baseline(&emulator, action_name).await;

        let mut request = case.update.clone();
        request["CustomActionArn"] = json!(arn);
        let answer = emulator.call("/update-custom-action", &request).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the update answers 200, got {} with {}",
            case.label, answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("CustomActionArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {:?}: the update answers the ARN it was addressed to, got {}",
            case.label,
            answer.body
        );

        // The read must come out of the store, not out of the update request.
        let stored = get_ok(&emulator, &arn).await;
        assert_eq!(
            stored,
            expected_action(&case, &arn, action_name),
            "case {:?}: the stored action differs from the expected one, got {stored}",
            case.label
        );

        // The tags the create carried are not the update's business: only the tagging
        // operations write them.
        let listed = emulator
            .call("/list-tags-for-resource", &json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: reading the action's tags answers 200, got {} with {}",
            case.label, listed.status, listed.body
        );
        assert_eq!(
            listed.body.get("Tags"),
            Some(&json!([{"TagKey": "team", "TagValue": "ops"}])),
            "case {:?}: the update must leave the tags alone, got {}",
            case.label,
            listed.body
        );
    }
}

#[tokio::test]
async fn successive_updates_each_replace_the_state_the_previous_one_left() {
    // One action, every row applied in turn: each read must report that row's state alone,
    // so nothing carries over from the row before it.
    let emulator = Emulator::new();
    let action_name = "serially_updated-1";
    let arn = create_baseline(&emulator, action_name).await;

    for case in update_cases() {
        let mut request = case.update.clone();
        request["CustomActionArn"] = json!(arn);
        let answer = emulator.call("/update-custom-action", &request).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the update answers 200, got {} with {}",
            case.label, answer.status, answer.body
        );

        let stored = get_ok(&emulator, &arn).await;
        assert_eq!(
            stored,
            expected_action(&case, &arn, action_name),
            "case {:?}: the stored action must be this update's state alone, got {stored}",
            case.label
        );
    }
}

#[tokio::test]
async fn an_update_is_addressed_by_arn_alone_whatever_region_it_is_signed_for() {
    let emulator = Emulator::new();
    // Two actions, each created from a different region: a custom action is
    // account-scoped, so both ARNs are region-less and either one is updated from a
    // request signed for any region.
    let here_name = "signed-here";
    let there_name = "signed-there";
    let here = create_baseline(&emulator, here_name).await;
    let created_there = emulator
        .call_in_region(
            OTHER_REGION,
            "/create-custom-action",
            &baseline_create(there_name),
        )
        .await;
    assert_eq!(
        created_there.status, 200,
        "creating {there_name:?} in {OTHER_REGION} answers 200, got {} with {}",
        created_there.status, created_there.body
    );
    let there = custom_action_arn(there_name);
    assert_eq!(
        created_there.body.get("CustomActionArn").and_then(Value::as_str),
        Some(there.as_str()),
        "a create signed for {OTHER_REGION} must mint the region-less ARN, got {}",
        created_there.body
    );

    // Update the action created in the OTHER region from a request signed for REGION, and
    // the one created here from a request signed for the other: the signing region is not
    // part of the lookup.
    let pairs = [(&there, there_name, REGION), (&here, here_name, OTHER_REGION)];
    for (arn, _, signing_region) in pairs {
        let answer = emulator
            .call_in_region(
                signing_region,
                "/update-custom-action",
                &json!({
                    "CustomActionArn": arn,
                    "Definition": {"CommandText": format!("s3 ls --region {signing_region}")},
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {arn} signed for {signing_region}: expected 200, got {} with {}",
            answer.status, answer.body
        );
    }

    for (arn, action_name, signing_region) in pairs {
        let stored = get_ok(&emulator, arn).await;
        assert_eq!(
            stored,
            json!({
                "CustomActionArn": arn,
                "ActionName": action_name,
                "Definition": {"CommandText": format!("s3 ls --region {signing_region}")},
            }),
            "case {arn}: the update signed for {signing_region} must be the one stored, \
             got {stored}"
        );
    }
}

#[tokio::test]
async fn updating_an_arn_the_store_does_not_hold_is_refused_as_its_arn_form_calls_for() {
    let emulator = Emulator::new();
    let present = create_baseline(&emulator, "present").await;
    let before = get_ok(&emulator, &present).await;

    // Each row is a well-formed `CustomActionArn` (the modelled pattern
    // `^arn:aws:chatbot:…:custom-action/[a-zA-Z0-9_-]{1,64}$`) this account holds no action
    // under, with the refusal its ARN form calls for: the rows of the minted (region-less,
    // this-account) form differ in the action name's length and charset and are the modelled
    // 404, while a region segment or another account's id puts the ARN outside this account's
    // namespace, which is the modelled 403 — so neither refusal follows from the spelling of
    // the name.
    let unknown = [
        (
            "a name nothing was created under",
            custom_action_arn("absent"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "the shortest allowed name",
            custom_action_arn("a"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "the longest allowed name",
            custom_action_arn(&"z".repeat(64)),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "a mixed-case name",
            custom_action_arn("Mixed_Case-9"),
            404,
            RESOURCE_NOT_FOUND_EXCEPTION,
        ),
        (
            "the created name spelled with a region segment",
            custom_action_arn_in_region(REGION, "present"),
            403,
            UNAUTHORIZED_EXCEPTION,
        ),
        (
            "the created name spelled with another region segment",
            custom_action_arn_in_region(OTHER_REGION, "present"),
            403,
            UNAUTHORIZED_EXCEPTION,
        ),
        (
            "another account's ARN for the created name",
            "arn:aws:chatbot::999999999999:custom-action/present".to_string(),
            403,
            UNAUTHORIZED_EXCEPTION,
        ),
    ];

    for (label, arn, expected_status, expected_code) in &unknown {
        let answer = emulator
            .call(
                "/update-custom-action",
                &json!({
                    "CustomActionArn": arn,
                    "Definition": {"CommandText": "s3 ls"},
                }),
            )
            .await;
        assert_eq!(
            answer.status, *expected_status,
            "case {label:?} ({arn}): expected HTTP {expected_status}, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            *expected_code,
            "case {label:?} ({arn}): expected the modelled {expected_code}, got {}",
            answer.body
        );

        // A refused update creates nothing: the ARN is still one this account holds no
        // action under, and the read refuses it the same way.
        let read = emulator
            .call("/get-custom-action", &json!({"CustomActionArn": arn}))
            .await;
        assert_eq!(
            read.status, *expected_status,
            "case {label:?} ({arn}): a refused update must persist nothing, got {} with {}",
            read.status, read.body
        );
    }

    // The action that IS there is untouched by those refused updates.
    let after = get_ok(&emulator, &present).await;
    assert_eq!(
        after, before,
        "the stored action must be unchanged by the refused updates, got {after}"
    );
}
