// SPDX-License-Identifier: CC0-1.0

//! What a SUCCESSFUL AWS Chatbot answer looks like on the wire, over restJson1 through the
//! same `MultiServiceRouter` the production router serves with.
//!
//! Four rules, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. a create answers HTTP 200 carrying its result document — every create this service
//!    implements, whatever members the request carries and whichever region it is signed
//!    for;
//! 2. a resource AWS Chatbot scopes to the ACCOUNT is named by an ARN with an EMPTY region
//!    segment, so the custom action one create mints is the same resource a request signed
//!    for any other region addresses;
//! 3. an optional member the request did not name is LEFT OUT of the write's answer
//!    rather than serialized as its type's zero value, so a client cannot mistake
//!    "nothing configured this" for "configured off", while the matching READ answers the
//!    account's whole set (the account preferences are the pair of optional booleans this
//!    service has);
//! 4. a listing reports exactly what the account's own state holds, so an account that
//!    authorized nothing is answered an empty list and never a resource no request created.
//!
//! The plumbing is the shared `tests/all/wire.rs` harness.

use serde_json::{json, Value};
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;

use crate::wire::{Emulator, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The Slack workspace the Slack rows configure a channel in, authorized through the
/// out-of-band state seam (no operation in this model performs that handshake).
const SLACK_TEAM_ID: &str = "TWIRESHAPE1";
const SLACK_TEAM_NAME: &str = "Wire Shape Workspace";

/// This service mounted alone, with the Slack workspace the rows below need authorized.
fn emulator() -> Emulator {
    let emulator = Emulator::new();
    chatbot::state::authorize_slack_workspace(&*emulator.engine, SLACK_TEAM_ID, SLACK_TEAM_NAME)
        .expect("the shared engine holds the chatbot store");
    emulator
}

/// One create row: the operation's path, the request, and the member its result document
/// must carry (`smithy_query` operation=Create…: the single output member of each).
struct CreateCase {
    label: &'static str,
    path: &'static str,
    request: Value,
    result_member: &'static str,
}

/// The creates this service implements, each over more than one shape of request: only the
/// required members, and every optional member the operation takes. The rows differ along
/// what rule 1 generalises over — which operation, and how much the request carries.
fn create_cases(suffix: &str) -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "Chime webhook, only the required members",
            path: "/create-chime-webhook-configuration",
            request: json!({
                "ConfigurationName": format!("chime-minimal-{suffix}"),
                "WebhookDescription": "Ops/Room",
                "WebhookUrl": "https://hooks.chime.aws/incomingwebhooks/abc-123?token=tok-456",
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
                "IamRoleArn": IAM_ROLE_ARN,
            }),
            result_member: "WebhookConfiguration",
        },
        CreateCase {
            label: "Chime webhook, every optional member too",
            path: "/create-chime-webhook-configuration",
            request: json!({
                "ConfigurationName": format!("chime-full-{suffix}"),
                "WebhookDescription": "Ops/Escalations",
                "WebhookUrl": "https://hooks.chime.aws/incomingwebhooks/def-456?token=tok-789",
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
                "IamRoleArn": IAM_ROLE_ARN,
                "LoggingLevel": "INFO",
                "Tags": [{"TagKey": "env", "TagValue": "prod"}],
            }),
            result_member: "WebhookConfiguration",
        },
        CreateCase {
            label: "Slack channel, only the required members",
            path: "/create-slack-channel-configuration",
            request: json!({
                "ConfigurationName": format!("slack-minimal-{suffix}"),
                "SlackTeamId": SLACK_TEAM_ID,
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
            }),
            result_member: "ChannelConfiguration",
        },
        CreateCase {
            label: "Slack channel, every optional member too",
            path: "/create-slack-channel-configuration",
            request: json!({
                "ConfigurationName": format!("slack-full-{suffix}"),
                "SlackTeamId": SLACK_TEAM_ID,
                "SlackChannelId": "C0123ABCD",
                "SlackChannelName": "ops-room",
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
                "IamRoleArn": IAM_ROLE_ARN,
                "LoggingLevel": "ERROR",
                "GuardrailPolicyArns": ["arn:aws:iam::aws:policy/AdministratorAccess"],
                "UserAuthorizationRequired": true,
                "Tags": [{"TagKey": "env", "TagValue": "prod"}],
            }),
            result_member: "ChannelConfiguration",
        },
        CreateCase {
            label: "custom action, only the required members",
            path: "/create-custom-action",
            request: json!({
                "ActionName": format!("action-minimal-{suffix}"),
                "Definition": {"CommandText": "ssm list-documents"},
            }),
            result_member: "CustomActionArn",
        },
        CreateCase {
            label: "custom action, every optional member too",
            path: "/create-custom-action",
            request: json!({
                "ActionName": format!("action-full-{suffix}"),
                "Definition": {"CommandText": "s3 ls"},
                "AliasName": "list-buckets",
                "Attachments": [{"ButtonText": "List buckets"}],
                "Tags": [{"TagKey": "env", "TagValue": "prod"}],
                "ClientToken": "0123456789abcdef0123456789abcdef0",
            }),
            result_member: "CustomActionArn",
        },
    ]
}

#[tokio::test]
async fn every_implemented_create_answers_200_with_its_result_document() {
    let emulator = emulator();

    // Signed for either region, so the status is the operation's answer and not a property
    // of one signing region.
    for region in [REGION, OTHER_REGION] {
        for case in create_cases(region) {
            let answer = emulator
                .call_in_region(region, case.path, &case.request)
                .await;
            assert_eq!(
                answer.status, 200,
                "case {:?} in {region}: expected HTTP 200, got {} with {}",
                case.label, answer.status, answer.body
            );
            assert!(
                answer.body.get(case.result_member).is_some(),
                "case {:?} in {region}: the result document must carry {}, got {}",
                case.label,
                case.result_member,
                answer.body
            );
        }
    }
}

/// The action names rule 2 is asserted over: the published `CustomActionName` bounds
/// (`smithy_query` operation=CreateCustomAction: len 1..64, pattern
/// `^[a-zA-Z0-9_-]{1,64}$`) and the charset in between.
fn action_names() -> Vec<String> {
    vec![
        "a".to_string(),
        "z".repeat(64),
        "Mixed_Case-9".to_string(),
        "0123456789".to_string(),
    ]
}

#[tokio::test]
async fn a_minted_custom_action_arn_carries_an_empty_region_segment() {
    for (index, action_name) in action_names().into_iter().enumerate() {
        // A fresh store per row: a custom action is account-scoped, so one name is one
        // action for the whole account.
        let emulator = Emulator::new();
        // Alternate the signing region, so the minted ARN cannot be carrying it.
        let signing_region = if index % 2 == 0 { REGION } else { OTHER_REGION };

        let created = emulator
            .call_in_region(
                signing_region,
                "/create-custom-action",
                &json!({
                    "ActionName": action_name,
                    "Definition": {"CommandText": "ssm list-documents"},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {action_name:?} in {signing_region}: expected HTTP 200, got {} with {}",
            created.status, created.body
        );

        let arn = created
            .body
            .get("CustomActionArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {action_name:?}: no CustomActionArn in {}", created.body)
            })
            .to_string();
        assert_eq!(
            arn,
            format!("arn:aws:chatbot::{ACCOUNT}:custom-action/{action_name}"),
            "case {action_name:?} in {signing_region}: the ARN must carry no region, got {arn:?}"
        );

        // Read as an ARN rather than as a string: the region is the fourth segment, and it
        // is empty.
        let segments: Vec<&str> = arn.split(':').collect();
        assert_eq!(
            segments.get(3).copied(),
            Some(""),
            "case {action_name:?}: the ARN's region segment must be empty, got {arn:?}"
        );

        // The ARN the create answered is the one the store holds: the action reads back
        // through it from either region.
        for region in [REGION, OTHER_REGION] {
            let read = emulator
                .call_in_region(region, "/get-custom-action", &json!({"CustomActionArn": arn}))
                .await;
            assert_eq!(
                read.status, 200,
                "case {action_name:?}: {arn} must read back in {region}, got {} with {}",
                read.status, read.body
            );
            assert_eq!(
                read.body["CustomAction"]["CustomActionArn"],
                json!(arn),
                "case {action_name:?}: the stored action must report its own ARN, got {}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn an_unwritten_account_preference_is_absent_from_the_write_that_did_not_name_it() {
    // The modelled `AccountPreferences` members are optional booleans with no default
    // (`smithy_query` shape=AccountPreferences), so each row names a different subset of
    // them: the write's answer carries exactly the ones it set, while the read reports the
    // account's whole set (an opt-in nothing turned on reading as off).
    let rows: [(&str, Value, Vec<&str>); 4] = [
        ("neither member", json!({}), Vec::new()),
        (
            "only UserAuthorizationRequired",
            json!({"UserAuthorizationRequired": true}),
            vec!["UserAuthorizationRequired"],
        ),
        (
            "only TrainingDataCollectionEnabled, set to false",
            json!({"TrainingDataCollectionEnabled": false}),
            vec!["TrainingDataCollectionEnabled"],
        ),
        (
            "both members",
            json!({"UserAuthorizationRequired": false, "TrainingDataCollectionEnabled": true}),
            vec!["UserAuthorizationRequired", "TrainingDataCollectionEnabled"],
        ),
    ];

    for (case, write, present) in rows {
        // A fresh store per row, so "absent" means nothing ever wrote it.
        let emulator = Emulator::new();

        let answer = emulator.call("/update-account-preferences", &write).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        let written = answer
            .body
            .get("AccountPreferences")
            .and_then(Value::as_object)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no AccountPreferences object in {}",
                    answer.body
                )
            });

        for member in ["UserAuthorizationRequired", "TrainingDataCollectionEnabled"] {
            if present.contains(&member) {
                assert_eq!(
                    written.get(member),
                    write.get(member),
                    "case {case:?}: {member} was written, so the write must answer it as \
                     written, got {}",
                    answer.body
                );
            } else {
                assert!(
                    !written.contains_key(member),
                    "case {case:?}: the write did not name {member}, so it must be absent \
                     from the answer, got {}",
                    answer.body
                );
            }
        }

        // The read answers the account's whole preference set out of the same state: the
        // member the write named with the value it wrote, the other one off.
        let read = emulator.call("/get-account-preferences", &json!({})).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {}",
            read.status, read.body
        );
        let expected = json!({
            "UserAuthorizationRequired": write
                .get("UserAuthorizationRequired")
                .cloned()
                .unwrap_or(json!(false)),
            "TrainingDataCollectionEnabled": write
                .get("TrainingDataCollectionEnabled")
                .cloned()
                .unwrap_or(json!(false)),
        });
        assert_eq!(
            read.body.get("AccountPreferences"),
            Some(&expected),
            "case {case:?}: the read must answer {expected}, got {}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_listing_reports_only_what_the_accounts_own_state_holds() {
    // The Microsoft Teams registry: nothing in this model authorizes a team, so an account
    // that has not been through the out-of-band handshake holds none — and the listing says
    // so instead of reporting teams no request created. The rows walk the registry from
    // empty up, so the answer tracks the state rather than a fixture.
    let teams: [(&str, &str, Option<&str>); 3] = [
        (
            "11111111-1111-1111-1111-111111111111",
            "0f0e0d0c-0b0a-4009-8008-000700060005",
            Some("deadbeef-0001-4000-8000-000000000001"),
        ),
        (
            "11111111-1111-1111-1111-111111111111",
            "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
            None,
        ),
        (
            "22222222-2222-2222-2222-222222222222",
            "FFFFFFFF-FFFF-4FFF-8FFF-FFFFFFFFFFFF",
            Some("DEADBEEF-0002-4000-8000-000000000002"),
        ),
    ];

    let emulator = Emulator::new();
    for authorized in 0..=teams.len() {
        if authorized > 0 {
            let (tenant_id, team_id, team_name) = teams[authorized - 1];
            chatbot::state::authorize_microsoft_teams_team(
                &*emulator.engine,
                tenant_id,
                team_id,
                team_name,
            )
            .expect("the shared engine holds the chatbot store");
        }

        let answer = emulator.call("/list-ms-teams-configured-teams", &json!({})).await;
        assert_eq!(
            answer.status, 200,
            "case {authorized} authorized: expected HTTP 200, got {} with {}",
            answer.status, answer.body
        );
        let listed = answer
            .body
            .get("ConfiguredTeams")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {authorized} authorized: no ConfiguredTeams list in {}",
                    answer.body
                )
            });
        assert_eq!(
            listed.len(),
            authorized,
            "case {authorized} authorized: the listing must report exactly the authorized \
             teams, got {listed:?}"
        );

        let listed_ids: Vec<&str> = listed
            .iter()
            .map(|team| team["TeamId"].as_str().unwrap_or_default())
            .collect();
        for (_, team_id, _) in teams.iter().take(authorized) {
            assert!(
                listed_ids.contains(team_id),
                "case {authorized} authorized: {team_id} is authorized, so it must be listed, \
                 got {listed_ids:?}"
            );
        }
        for (_, team_id, _) in teams.iter().skip(authorized) {
            assert!(
                !listed_ids.contains(team_id),
                "case {authorized} authorized: {team_id} is not authorized, so it must not be \
                 listed, got {listed_ids:?}"
            );
        }
    }
}
