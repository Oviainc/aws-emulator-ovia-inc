// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `DeleteCustomAction` and the reads that prove what it removed,
//! over the wire (restJson1) through the same `MultiServiceRouter` the production router
//! serves with.
//!
//! The rule every assertion here is about: a delete REMOVES the custom action from the
//! store, so every later call addressed to that ARN answers exactly as it does for an
//! action that was never created — `GetCustomAction` with the modelled 404
//! `ResourceNotFoundException` naming the action and the account, and the delete itself with
//! the modelled 404 its own error class carries, which is the error code and status ALONE:
//! this is an error class AWS Chatbot words nothing for, so its answer has no message member
//! at all. The record's tags go with it, and the name is free for a new create again. The
//! delete answers an empty result document, is addressed by ARN alone (a custom action being
//! account-scoped), leaves every other action alone, and an ARN no create can mint for this
//! account is refused with the modelled 403 `UnauthorizedException` having removed nothing
//! (the rule `tests/all/custom_action_fail_closed.rs` asserts across the operations addressed
//! by a custom-action ARN).
//!
//! The table spans what the rule generalises over rather than one recorded request: the
//! action names differ in length (each published bound included) and charset, the deleted
//! actions differ in which optional members and how many tags they carry, and each row
//! deletes one action out of several.

use serde_json::{json, Value};

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const CREATE_PATH: &str = "/create-custom-action";
const DELETE_PATH: &str = "/delete-custom-action";
const GET_PATH: &str = "/get-custom-action";
const LIST_TAGS_PATH: &str = "/list-tags-for-resource";

/// The modelled error one of this account's custom-action ARNs that holds no action is
/// refused with.
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The modelled error an ARN that could name no custom action of this account's is refused
/// with.
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";

/// A custom action is account-scoped, so the ARN a create mints carries an EMPTY region
/// segment (`state::custom_action_arn`).
fn custom_action_arn(action_name: &str) -> String {
    format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}")
}

/// The same ARN with a region segment filled in — a well-formed `CustomActionArn` that no
/// create ever mints, so it could name no action of this account's.
fn custom_action_arn_in_region(region: &str, action_name: &str) -> String {
    format!("arn:aws:chatbot:{region}:{ACCOUNT}:custom-action/{action_name}")
}

/// The `Message` AWS Chatbot answers for a custom action this account holds none of, with
/// the action name taken from the requested ARN and the account id from the caller's
/// account.
fn not_found_message(action_name: &str) -> String {
    format!("sdk action [{action_name}] not found for account [{ACCOUNT}]")
}

/// Assert `answer` is the modelled 404 a delete addressed to one of this account's
/// custom-action ARNs that holds no action carries: the error code and the status, and no
/// message member of either spelling — AWS Chatbot words this error class nothing.
fn assert_not_found_without_a_message(label: &str, arn: &str, answer: &crate::wire::Answer) {
    assert_eq!(
        answer.status, 404,
        "case {label:?} ({arn}): expected HTTP 404, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {label:?} ({arn}): expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("Message"),
        None,
        "case {label:?} ({arn}): this error class carries no message, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("message"),
        None,
        "case {label:?} ({arn}): this error class carries no message, got {}",
        answer.body
    );
}

/// Create the action `request` names and answer the ARN it was minted under.
async fn create_ok(emulator: &Emulator, request: &Value) -> String {
    let created = emulator.call(CREATE_PATH, request).await;
    assert_eq!(
        created.status, 200,
        "create {request}: expected 200, got {} with {}",
        created.status, created.body
    );
    created.body["CustomActionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("create {request}: no CustomActionArn in {}", created.body))
        .to_string()
}

/// Read `arn` back, asserting the read is served, and answer the `CustomAction` it reports.
async fn get_ok(emulator: &Emulator, arn: &str) -> Value {
    let answer = emulator.call(GET_PATH, &json!({"CustomActionArn": arn})).await;
    assert_eq!(
        answer.status, 200,
        "reading {arn} answers 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["CustomAction"].clone()
}

/// Assert `arn` names no action: the read answers the modelled 404 naming `action_name` and
/// the account.
async fn assert_absent(emulator: &Emulator, label: &str, arn: &str, action_name: &str) {
    let answer = emulator.call(GET_PATH, &json!({"CustomActionArn": arn})).await;
    assert_eq!(
        answer.status, 404,
        "case {label:?} ({arn}): a deleted action must read back as absent, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {label:?} ({arn}): expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("Message").and_then(Value::as_str),
        Some(not_found_message(action_name).as_str()),
        "case {label:?} ({arn}): the not-found Message must name the action and the account, \
         got {}",
        answer.body
    );
}

/// One row of the delete table: the create whose action is then deleted.
struct DeleteCase {
    label: &'static str,
    /// The `ActionName` the row's action carries, and therefore the name its ARN gives it.
    action_name: String,
    create: Value,
}

/// The delete cases, each differing from the others along what the delete rule generalises
/// over: the action name's length (`CustomActionName`, len 1..64 — both published bounds
/// included) and charset, and which optional members (`AliasName`, `Attachments`, `Tags`)
/// the deleted action carries.
fn delete_cases() -> Vec<DeleteCase> {
    let names = [
        "deleted-action",
        "a",
        &"z".repeat(64),
        "Mixed_Case-9",
        "tagged_action-1",
    ];
    let bodies = [
        json!({"Definition": {"CommandText": "s3 ls"}}),
        json!({
            "Definition": {"CommandText": "ssm start-session --target $InstanceId"},
            "AliasName": "start-session",
        }),
        json!({
            "Definition": {"CommandText": "cloudwatch describe-alarms"},
            "Attachments": [{
                "NotificationType": "aws-cloudwatch-alarm",
                "ButtonText": "Describe alarms",
                "Criteria": [{"Operator": "HAS_VALUE", "VariableName": "AlarmName"}],
                "Variables": {"AlarmName": "$.detail.alarmName"},
            }],
        }),
        json!({
            "Definition": {"CommandText": "lambda invoke --function-name $Function"},
            "AliasName": "run-lambda",
            "Tags": [{"TagKey": "env", "TagValue": "dev"}],
        }),
        json!({
            "Definition": {"CommandText": "ec2 describe-instances"},
            "AliasName": "instances",
            "Attachments": [{"ButtonText": "List instances"}],
            "Tags": [
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "team", "TagValue": "ops"},
            ],
        }),
    ];
    let labels = [
        "only the required members",
        "an alias",
        "one attachment",
        "an alias and one tag",
        "every optional member and two tags",
    ];

    names
        .iter()
        .zip(bodies)
        .zip(labels)
        .map(|((name, body), label)| {
            let mut create = body;
            create["ActionName"] = json!(name);
            DeleteCase {
                label,
                action_name: (*name).to_string(),
                create,
            }
        })
        .collect()
}

/// The bystander actions every row creates beside the one it deletes, so a delete that
/// removed too much is visible.
fn bystanders() -> Vec<Value> {
    vec![
        json!({
            "ActionName": "bystander-1",
            "Definition": {"CommandText": "s3api list-buckets"},
            "AliasName": "buckets",
        }),
        json!({
            "ActionName": "bystander_2",
            "Definition": {"CommandText": "logs describe-log-groups"},
            "Tags": [{"TagKey": "keep", "TagValue": "me"}],
        }),
    ]
}

#[tokio::test]
async fn deleting_an_action_removes_it_and_leaves_every_other_action_alone() {
    for case in delete_cases() {
        // A fresh store per row, so each row deletes out of the same arrangement.
        let emulator = Emulator::new();
        let arn = create_ok(&emulator, &case.create).await;
        assert_eq!(
            arn,
            custom_action_arn(&case.action_name),
            "case {:?}: the create must mint the account-scoped ARN, got {arn}",
            case.label
        );

        let mut kept: Vec<(String, Value)> = Vec::new();
        for create in bystanders() {
            let bystander = create_ok(&emulator, &create).await;
            let stored = get_ok(&emulator, &bystander).await;
            kept.push((bystander, stored));
        }

        let deleted = emulator
            .call(DELETE_PATH, &json!({"CustomActionArn": arn}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: the delete answers 200, got {} with {}",
            case.label, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {:?}: the delete answers an empty result document, got {}",
            case.label,
            deleted.body
        );

        assert_absent(&emulator, case.label, &arn, &case.action_name).await;

        for (bystander, stored) in &kept {
            let after = get_ok(&emulator, bystander).await;
            assert_eq!(
                &after, stored,
                "case {:?}: deleting {arn} must leave {bystander} as it was, got {after}",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn a_repeated_delete_is_not_found_and_the_name_is_free_with_none_of_its_tags() {
    for case in delete_cases() {
        let emulator = Emulator::new();
        let arn = create_ok(&emulator, &case.create).await;

        let deleted = emulator
            .call(DELETE_PATH, &json!({"CustomActionArn": arn}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: the delete answers 200, got {} with {}",
            case.label, deleted.status, deleted.body
        );

        // The action is gone, so a repeat of the same delete has nothing to remove and
        // reports the action absent with the modelled 404 — the error code and status alone,
        // the answer AWS Chatbot gives this error class, which is also the answer for a name
        // nothing was ever created under.
        for attempt in 2..=3 {
            let again = emulator
                .call(DELETE_PATH, &json!({"CustomActionArn": arn}))
                .await;
            assert_not_found_without_a_message(
                &format!("{} (delete attempt {attempt})", case.label),
                &arn,
                &again,
            );
            assert_absent(&emulator, case.label, &arn, &case.action_name).await;
        }

        // The name is free again, and the new action carries none of the deleted one's
        // state: the record owned its tags, so they went with it.
        let recreated = create_ok(
            &emulator,
            &json!({
                "ActionName": case.action_name,
                "Definition": {"CommandText": "sts get-caller-identity"},
            }),
        )
        .await;
        assert_eq!(
            recreated, arn,
            "case {:?}: re-creating the deleted name must mint the same ARN, got {recreated}",
            case.label
        );
        assert_eq!(
            get_ok(&emulator, &arn).await,
            json!({
                "CustomActionArn": arn,
                "ActionName": case.action_name,
                "Definition": {"CommandText": "sts get-caller-identity"},
            }),
            "case {:?}: the re-created action must carry only what its create named",
            case.label
        );
        let listed = emulator
            .call(LIST_TAGS_PATH, &json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            listed.body.get("Tags"),
            Some(&json!([])),
            "case {:?}: the deleted action's tags must not survive it, got {}",
            case.label,
            listed.body
        );
    }
}

#[tokio::test]
async fn deleting_one_of_this_account_s_names_that_holds_no_action_is_not_found_with_no_message() {
    let emulator = Emulator::new();
    let present = create_ok(
        &emulator,
        &json!({
            "ActionName": "present",
            "Definition": {"CommandText": "s3 ls"},
            "AliasName": "here",
            "Tags": [{"TagKey": "keep", "TagValue": "me"}],
        }),
    )
    .await;
    let stored = get_ok(&emulator, &present).await;

    // The store cannot tell a delete of an action removed a moment ago from a delete of a
    // name nothing was ever created under, so both are the same modelled 404 carrying no
    // message. The rows differ in the action name's length (`CustomActionName`, len 1..64 —
    // both published bounds included) and charset, so the answer cannot follow from one
    // spelling of the name.
    for name in [
        "never-created".to_string(),
        "a".to_string(),
        "z".repeat(64),
        "Mixed_Case-9".to_string(),
    ] {
        let arn = custom_action_arn(&name);
        let answer = emulator
            .call(DELETE_PATH, &json!({"CustomActionArn": arn}))
            .await;
        assert_not_found_without_a_message(&name, &arn, &answer);
        assert_absent(&emulator, &name, &arn, &name).await;
    }

    // None of those refusals touched the action that IS there.
    let after = get_ok(&emulator, &present).await;
    assert_eq!(
        after, stored,
        "the refused deletes must leave the stored action unchanged, got {after}"
    );
    let listed = emulator
        .call(LIST_TAGS_PATH, &json!({"ResourceARN": present}))
        .await;
    assert_eq!(
        listed.body.get("Tags"),
        Some(&json!([{"TagKey": "keep", "TagValue": "me"}])),
        "the refused deletes must leave the stored action's tags, got {}",
        listed.body
    );
}

#[tokio::test]
async fn a_delete_is_addressed_by_arn_alone_whatever_region_it_is_signed_for() {
    // A custom action is account-scoped, so one action name is one action for the whole
    // account and the signing region is not part of the lookup: the action created from
    // one region is deleted from the other and vice versa.
    for (create_region, delete_region) in [(REGION, OTHER_REGION), (OTHER_REGION, REGION)] {
        let emulator = Emulator::new();
        let action_name = "region-crossing";
        let created = emulator
            .call_in_region(
                create_region,
                CREATE_PATH,
                &json!({
                    "ActionName": action_name,
                    "Definition": {"CommandText": "s3 ls"},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "a create signed for {create_region} answers 200, got {} with {}",
            created.status, created.body
        );
        let arn = custom_action_arn(action_name);

        let deleted = emulator
            .call_in_region(delete_region, DELETE_PATH, &json!({"CustomActionArn": arn}))
            .await;
        assert_eq!(
            deleted.status, 200,
            "a delete signed for {delete_region} must reach the action created in \
             {create_region}, got {} with {}",
            deleted.status, deleted.body
        );

        for region in [REGION, OTHER_REGION] {
            let answer = emulator
                .call_in_region(region, GET_PATH, &json!({"CustomActionArn": arn}))
                .await;
            assert_eq!(
                answer.status, 404,
                "the deleted action must be absent in {region} too, got {} with {}",
                answer.status, answer.body
            );
        }
    }
}

#[tokio::test]
async fn deleting_an_arn_that_names_no_action_of_this_account_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let present = create_ok(
        &emulator,
        &json!({
            "ActionName": "present",
            "Definition": {"CommandText": "s3 ls"},
            "AliasName": "here",
        }),
    )
    .await;
    let stored = get_ok(&emulator, &present).await;

    // Each row is a well-formed `CustomActionArn` that is not one of this account's, so
    // the delete is refused before anything about the store is disclosed. The rows differ
    // in the region segment they carry and in the account they name.
    let refused = [
        (
            "the created name spelled with a region segment",
            custom_action_arn_in_region(REGION, "present"),
        ),
        (
            "the created name spelled with another region segment",
            custom_action_arn_in_region(OTHER_REGION, "present"),
        ),
        (
            "another account's ARN for the created name",
            "arn:aws:chatbot::999999999999:custom-action/present".to_string(),
        ),
        (
            "another account's ARN for a name nothing was created under",
            "arn:aws:chatbot::999999999999:custom-action/absent".to_string(),
        ),
    ];

    for (label, arn) in &refused {
        let answer = emulator
            .call(DELETE_PATH, &json!({"CustomActionArn": arn}))
            .await;
        assert_eq!(
            answer.status, 403,
            "case {label:?} ({arn}): expected HTTP 403, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            UNAUTHORIZED_EXCEPTION,
            "case {label:?} ({arn}): expected the modelled {UNAUTHORIZED_EXCEPTION}, got {}",
            answer.body
        );
    }

    // None of those refusals touched the action that IS there.
    let after = get_ok(&emulator, &present).await;
    assert_eq!(
        after, stored,
        "the refused deletes must leave the stored action unchanged, got {after}"
    );
}
