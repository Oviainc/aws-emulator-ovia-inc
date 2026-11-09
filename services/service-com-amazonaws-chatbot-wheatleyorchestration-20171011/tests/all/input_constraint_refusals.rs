// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a request whose input does not satisfy a MODELLED constraint,
//! over the wire (restJson1) through the same `MultiServiceRouter` the production router
//! serves with.
//!
//! The rule every assertion here is about: a request refused because a member violates a
//! constraint the model declares — a pattern, a range, a length, or a required member that
//! is absent — is answered with this service's own modelled client-input error,
//! `InvalidRequestException` (HTTP 400), whichever operation was addressed and whichever
//! constraint failed; a request whose members satisfy every modelled constraint is not
//! refused that way, whatever the operation then does with it.
//!
//! `InvalidRequestException` is the only code the chatbot model has for this: it is
//! `@error("client") @httpError(400)`, documented "Your request input doesn't meet the
//! constraints required by AWS Chatbot", and the service model declares NO
//! `ValidationException` shape at all (`smithy_query(shape="ValidationException")` ->
//! not found), so the framework's synthetic `smithy.framework#ValidationException` names a
//! shape no chatbot client or code generator can know.
//!
//! The table below spans the class rather than one recorded request: four constraint KINDS
//! (pattern, range, length, required), eight operations — implemented, scaffolded and
//! reporting alike — and, for the `MaxResults` range, each published bound and one past it.
//! Every constraint a row violates is quoted from the model beside it
//! (`smithy_query(query="operation", operation="<Op>")`).

use serde_json::{json, Value};

use crate::wire::Emulator;

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// An `Arn` (`length 12..1224`, `pattern ^arn:aws:…$`) every create below can carry.
const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// An SNS topic ARN for the Chime webhook create's required `SnsTopicArns`.
const SNS_TOPIC_ARN: &str = "arn:aws:sns:us-east-1:000000000000:chatbot-topic";

/// A `ChimeWebhookUrl` that satisfies its modelled pattern
/// (`^https://hooks\.chime\.aws/incomingwebhooks/[A-Za-z0-9\-]+?\?token=[A-Za-z0-9\-]+$`).
const CHIME_WEBHOOK_URL: &str = "https://hooks.chime.aws/incomingwebhooks/abc-123?token=tok-456";

/// A `UUID` that satisfies its modelled pattern
/// (`^[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$`), for the Teams create's
/// `TeamId` and `TenantId`.
const UUID: &str = "12345678-1234-5678-9abc-123456789abc";

/// A `TeamsChannelId` that satisfies its modelled pattern
/// (`^([a-zA-Z0-9-_=+\/.,])*%3[aA]([a-zA-Z0-9-_=+\/.,])*%40([a-zA-Z0-9-_=+\/.,])*$`).
const TEAMS_CHANNEL_ID: &str = "19%3abcdef%40thread.tacv2";

/// A Slack workspace the store seeds an authorization for (`state::seed_slack_workspaces`),
/// so an accepted Slack create below reaches a workspace this account holds.
const SEEDED_SLACK_TEAM_ID: &str = "TEMULATOR001";

/// The modelled client-input error every constraint refusal must name.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The region the ARNs in the rows below name — the one [`Emulator::call`] signs for.
const REGION: &str = crate::wire::REGION;

/// One row: a request that violates ONE modelled constraint, named by `label`.
#[derive(Debug)]
struct RefusedCase {
    label: &'static str,
    path: &'static str,
    request: Value,
}

/// A Chime webhook create whose members are all valid except where a row overrides one.
fn chime_create(webhook_url: &str) -> Value {
    json!({
        "WebhookDescription": "chatbot emulator webhook",
        "WebhookUrl": webhook_url,
        "SnsTopicArns": [SNS_TOPIC_ARN],
        "IamRoleArn": IAM_ROLE_ARN,
        "ConfigurationName": "chime-config",
    })
}

/// A Teams channel create whose members are all valid except `team_id`.
fn teams_create(team_id: &str) -> Value {
    json!({
        "ChannelId": TEAMS_CHANNEL_ID,
        "TeamId": team_id,
        "TenantId": UUID,
        "IamRoleArn": IAM_ROLE_ARN,
        "ConfigurationName": "teams-config",
    })
}

/// A Slack channel create whose members are all valid except where a row overrides one.
fn slack_create(slack_team_id: &str, configuration_name: &str, logging_level: Value) -> Value {
    let mut request = json!({
        "SlackTeamId": slack_team_id,
        "SlackChannelId": "C0123ABCD",
        "IamRoleArn": IAM_ROLE_ARN,
        "ConfigurationName": configuration_name,
    });
    if let Some(level) = logging_level.as_str() {
        request["LoggingLevel"] = json!(level);
    }
    request
}

/// The refused rows: four constraint kinds across eight operations.
fn refused_cases() -> Vec<RefusedCase> {
    vec![
        // --- pattern ---
        RefusedCase {
            // CreateChimeWebhookConfiguration.WebhookUrl pattern requires the `?token=`
            // query string; this URL stops at the webhook id.
            label: "Chime WebhookUrl without the token query string",
            path: "/create-chime-webhook-configuration",
            request: chime_create("https://hooks.chime.aws/incomingwebhooks/missing-token-param"),
        },
        RefusedCase {
            // CreateMicrosoftTeamsChannelConfiguration.TeamId is a UUID: 36 characters, so
            // this value satisfies the LENGTH and fails the pattern alone.
            label: "Teams TeamId of the right length that is not a UUID",
            path: "/create-ms-teams-channel-configuration",
            request: teams_create("111111111111111111111111111111111111"),
        },
        RefusedCase {
            // DescribeSlackChannelConfigurations.NextToken (PaginationToken) pattern
            // `^[a-zA-Z0-9=\/+_.\-,#:\\"{}]{4,1276}$` admits no `!`.
            label: "Describe NextToken outside the PaginationToken charset",
            path: "/describe-slack-channel-configurations",
            request: json!({"NextToken": "invalid!token!!"}),
        },
        RefusedCase {
            // CreateSlackChannelConfiguration.SlackTeamId pattern `^[0-9A-Z]{1,255}$` is
            // upper-case only.
            label: "Slack SlackTeamId in lower case",
            path: "/create-slack-channel-configuration",
            request: slack_create("t0123abcd", "lower-case-team", Value::Null),
        },
        RefusedCase {
            // ConfigurationName pattern `^[A-Za-z0-9-_]+$` admits no space.
            label: "Slack ConfigurationName with a space",
            path: "/create-slack-channel-configuration",
            request: slack_create(SEEDED_SLACK_TEAM_ID, "has spaces", Value::Null),
        },
        RefusedCase {
            // CustomerCwLogLevel pattern `^(ERROR|INFO|NONE)$` is the whole published
            // catalogue of logging levels; `DEBUG` is not in it.
            label: "Slack LoggingLevel outside the published catalogue",
            path: "/create-slack-channel-configuration",
            request: slack_create(SEEDED_SLACK_TEAM_ID, "bad-level", json!("DEBUG")),
        },
        RefusedCase {
            // TagResource.ResourceARN pattern
            // `^arn:aws:(wheatley|chatbot):…$` — this is not an ARN at all.
            label: "TagResource ResourceARN that is not an ARN",
            path: "/tag-resource",
            request: json!({
                "ResourceARN": "not-an-arn",
                "Tags": [{"TagKey": "k", "TagValue": "v"}],
            }),
        },
        RefusedCase {
            // GetCustomAction.CustomActionArn pattern needs
            // `custom-action/[a-zA-Z0-9_-]{1,64}`; the action name here is empty.
            label: "GetCustomAction CustomActionArn with an empty action name",
            path: "/get-custom-action",
            request: json!({
                "CustomActionArn":
                    format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/"),
            }),
        },
        RefusedCase {
            // CreateCustomAction.ActionName pattern `^[a-zA-Z0-9_-]{1,64}$`.
            label: "CreateCustomAction ActionName with a space",
            path: "/create-custom-action",
            request: json!({
                "ActionName": "has spaces",
                "Definition": {"CommandText": "ssm list-documents"},
            }),
        },
        // --- range: each published MaxResults bound, and one past it ---
        RefusedCase {
            // ListCustomActions.MaxResults @range min 1.
            label: "ListCustomActions MaxResults one below the range minimum",
            path: "/list-custom-actions",
            request: json!({"MaxResults": 0}),
        },
        RefusedCase {
            // ListCustomActions.MaxResults @range max 100.
            label: "ListCustomActions MaxResults one above the range maximum",
            path: "/list-custom-actions",
            request: json!({"MaxResults": 101}),
        },
        RefusedCase {
            // DescribeSlackChannelConfigurations.MaxResults @range min 1 — the same range
            // on another operation.
            label: "Describe MaxResults one below the range minimum",
            path: "/describe-slack-channel-configurations",
            request: json!({"MaxResults": 0}),
        },
        RefusedCase {
            label: "Describe MaxResults one above the range maximum",
            path: "/describe-slack-channel-configurations",
            request: json!({"MaxResults": 101}),
        },
        // --- length ---
        RefusedCase {
            // ConfigurationName length 1..128.
            label: "Slack ConfigurationName one character past the length maximum",
            path: "/create-slack-channel-configuration",
            request: slack_create(SEEDED_SLACK_TEAM_ID, &"a".repeat(129), Value::Null),
        },
        RefusedCase {
            // CreateCustomAction.ClientToken length 33..126.
            label: "CreateCustomAction ClientToken below the length minimum",
            path: "/create-custom-action",
            request: json!({
                "ActionName": "short-token",
                "Definition": {"CommandText": "ssm list-documents"},
                "ClientToken": "too-short",
            }),
        },
        // --- a required member that is absent ---
        RefusedCase {
            label: "Slack create without the required IamRoleArn",
            path: "/create-slack-channel-configuration",
            request: json!({
                "SlackTeamId": SEEDED_SLACK_TEAM_ID,
                "SlackChannelId": "C0123ABCD",
                "ConfigurationName": "no-role",
            }),
        },
        RefusedCase {
            label: "Chime create without the required WebhookUrl",
            path: "/create-chime-webhook-configuration",
            request: json!({
                "WebhookDescription": "chatbot emulator webhook",
                "SnsTopicArns": [SNS_TOPIC_ARN],
                "IamRoleArn": IAM_ROLE_ARN,
                "ConfigurationName": "chime-config",
            }),
        },
        RefusedCase {
            label: "GetCustomAction without the required CustomActionArn",
            path: "/get-custom-action",
            request: json!({}),
        },
    ]
}

#[tokio::test]
async fn a_modelled_constraint_violation_is_refused_as_the_modelled_invalid_request_exception() {
    let emulator = Emulator::new();

    for case in refused_cases() {
        let answer = emulator.call(case.path, &case.request).await;
        let label = case.label;
        assert_eq!(
            answer.status, 400,
            "case {label:?} ({}): expected HTTP 400, got {} with {}",
            case.path, answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            INVALID_REQUEST_EXCEPTION,
            "case {label:?} ({}): expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?} \
             in {}",
            case.path,
            answer.code(),
            answer.body
        );
        // restJson1 resolves the error SHAPE from this header, so it has to name the
        // modelled error too — not the synthetic framework shape.
        assert_eq!(
            answer.error_type.as_deref(),
            Some(INVALID_REQUEST_EXCEPTION),
            "case {label:?} ({}): expected x-amzn-errortype {INVALID_REQUEST_EXCEPTION}, got \
             {:?}",
            case.path,
            answer.error_type
        );
    }
}

/// One row whose members satisfy every modelled constraint: the request must NOT be refused
/// as a constraint violation. `expected_status` is `None` where the operation addressed is
/// still a scaffold, so the only claim is that the refusal did not happen.
#[derive(Debug)]
struct AcceptedCase {
    label: &'static str,
    path: &'static str,
    request: Value,
    expected_status: Option<u16>,
}

#[tokio::test]
async fn input_inside_every_modelled_constraint_is_not_refused_as_a_constraint_violation() {
    let emulator = Emulator::new();
    // A Microsoft Teams channel configuration can only be created for a team AUTHORIZED
    // with the account, which is a different refusal from a constraint violation and
    // would hide the one this suite is about. The team the Teams row names is therefore
    // authorized here through the state seam standing in for the one-time Microsoft Teams
    // client handshake, exactly as the Slack rows name a seeded workspace.
    service_com_amazonaws_chatbot_wheatleyorchestration_20171011::state::authorize_microsoft_teams_team(
        &*emulator.engine,
        UUID,
        UUID,
        None,
    )
    .expect("the shared engine holds the chatbot store");

    let cases = vec![
        AcceptedCase {
            // The published lower bound of MaxResults @range 1..100.
            label: "Describe MaxResults at the range minimum",
            path: "/describe-slack-channel-configurations",
            request: json!({"MaxResults": 1}),
            expected_status: Some(200),
        },
        AcceptedCase {
            // The published upper bound.
            label: "Describe MaxResults at the range maximum",
            path: "/describe-slack-channel-configurations",
            request: json!({"MaxResults": 100}),
            expected_status: Some(200),
        },
        AcceptedCase {
            label: "ListCustomActions MaxResults at the range minimum",
            path: "/list-custom-actions",
            request: json!({"MaxResults": 1}),
            expected_status: None,
        },
        AcceptedCase {
            label: "ListCustomActions MaxResults at the range maximum",
            path: "/list-custom-actions",
            request: json!({"MaxResults": 100}),
            expected_status: None,
        },
        AcceptedCase {
            label: "Slack create with a LoggingLevel from the published catalogue",
            path: "/create-slack-channel-configuration",
            request: slack_create(SEEDED_SLACK_TEAM_ID, "accepted-config", json!("INFO")),
            expected_status: Some(200),
        },
        AcceptedCase {
            label: "CreateCustomAction with a ClientToken at the length minimum",
            path: "/create-custom-action",
            request: json!({
                "ActionName": "accepted-action",
                "Definition": {"CommandText": "ssm list-documents"},
                "ClientToken": "a".repeat(33),
            }),
            expected_status: Some(200),
        },
        AcceptedCase {
            label: "Chime create with a WebhookUrl that satisfies its pattern",
            path: "/create-chime-webhook-configuration",
            request: chime_create(CHIME_WEBHOOK_URL),
            expected_status: None,
        },
        AcceptedCase {
            label: "Teams create with a TeamId that is a UUID",
            path: "/create-ms-teams-channel-configuration",
            request: teams_create(UUID),
            expected_status: Some(200),
        },
    ];

    for case in cases {
        let answer = emulator.call(case.path, &case.request).await;
        let label = case.label;
        assert_ne!(
            answer.code(),
            INVALID_REQUEST_EXCEPTION,
            "case {label:?} ({}): input inside every modelled constraint must not be refused \
             as a constraint violation, got {} with {}",
            case.path,
            answer.status,
            answer.body
        );
        if let Some(expected) = case.expected_status {
            assert_eq!(
                answer.status, expected,
                "case {label:?} ({}): expected HTTP {expected}, got {} with {}",
                case.path, answer.status, answer.body
            );
        }
    }
}
