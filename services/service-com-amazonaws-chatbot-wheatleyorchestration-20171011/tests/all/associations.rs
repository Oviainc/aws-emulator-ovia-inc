// SPDX-License-Identifier: CC0-1.0

//! How a RESOURCE is linked to a channel configuration and unlinked again, driven over the
//! wire (restJson1) through the same `MultiServiceRouter` the production router serves
//! with.
//!
//! The rules every assertion here is about:
//!
//! * **Authorization before shape.** On both association operations the resource-scoped
//!   authorization decision on `Resource` is taken BEFORE that member's ARN shape is
//!   judged: an ARN no custom action of this account can be minted under — a malformed
//!   string, an ARN carrying a region segment, another account's, another resource type —
//!   names no target in the account's namespace and is refused HTTP 403
//!   `UnauthorizedException` with the fixed message `Unauthorized`, never as a
//!   client-input failure.
//! * **A target has to be able to host the link.** An associate against anything other
//!   than a channel configuration this account holds — an Amazon Chime webhook
//!   configuration, or an ARN no configuration is stored under — is refused with the
//!   modelled server error `InternalServiceError` (HTTP 500, the fixed message
//!   `Internal Server Error`) and leaves no association behind, however many callers ask
//!   at once.
//! * **A link round-trips, and both operations are idempotent.** A successful associate is
//!   reported by `ListAssociations`, repeating it leaves the configuration linked once,
//!   and a disassociate answers its empty 204 success whether or not a link was there to
//!   remove.
//!
//! Every table spans what the rule generalises over — the refused ARN's spelling, the
//! kind of target, the number of resources linked and the page size a listing asks for —
//! rather than one recorded request.

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_chatbot_wheatleyorchestration_20171011 as chatbot;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{post, OTHER_REGION, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled out
/// here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

/// The Slack workspace the channel configurations below are created in, with the display
/// name it is authorized under. A workspace has to be authorized with the account before a
/// channel can be configured in it, and no operation of this model performs that handshake
/// — so the harness registers it through the state seam, as the console handshake would.
const WORKSPACE: (&str, &str) = ("T0123ABCD", "Primary Workspace");

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";

/// The modelled 403 an ARN no custom action of this account can be minted under is refused
/// with, and the fixed sentence it carries.
const UNAUTHORIZED_EXCEPTION: &str = "UnauthorizedException";
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The modelled 500 a target that cannot host the link is refused with, and the fixed
/// sentence it carries.
const INTERNAL_SERVICE_ERROR: &str = "InternalServiceError";
const INTERNAL_SERVER_ERROR_MESSAGE: &str = "Internal Server Error";

/// A router mounting only this service over one shared engine, with [`WORKSPACE`]
/// authorized.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    chatbot::state::authorize_slack_workspace(&*engine, WORKSPACE.0, WORKSPACE.1)
        .expect("the shared engine holds the chatbot store");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(chatbot::EmulatedService::new(engine));
    router
}

/// Create the custom action `action_name` and answer the ARN it was minted under.
async fn given_custom_action(router: &MultiServiceRouter, action_name: &str) -> String {
    let answer = post(
        router,
        REGION,
        "/create-custom-action",
        &json!({
            "ActionName": action_name,
            "Definition": {"CommandText": "ssm list-documents"},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating the custom action {action_name:?}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["CustomActionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {action_name:?}: no CustomActionArn in {}", answer.body))
        .to_string()
}

/// Create the Slack channel configuration `name` and answer its `ChatConfigurationArn`.
async fn given_slack_channel_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let answer = post(
        router,
        REGION,
        "/create-slack-channel-configuration",
        &json!({
            "ConfigurationName": name,
            "SlackTeamId": WORKSPACE.0,
            "SlackChannelId": "C0123ABCD",
            "IamRoleArn": IAM_ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating the Slack channel configuration {name:?}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["ChannelConfiguration"]["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {name:?}: no ChatConfigurationArn in {}", answer.body))
        .to_string()
}

/// Create the Amazon Chime webhook configuration `name` and answer its
/// `ChatConfigurationArn`.
async fn given_chime_webhook_configuration(router: &MultiServiceRouter, name: &str) -> String {
    let answer = post(
        router,
        REGION,
        "/create-chime-webhook-configuration",
        &json!({
            "ConfigurationName": name,
            "WebhookDescription": "chatbot emulator webhook",
            "WebhookUrl": "https://hooks.chime.aws/incomingwebhooks/abc-123?token=tok-456",
            "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:chatbot-topic"],
            "IamRoleArn": IAM_ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "creating the Chime webhook configuration {name:?}: expected 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["WebhookConfiguration"]["ChatConfigurationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("creating {name:?}: no ChatConfigurationArn in {}", answer.body))
        .to_string()
}

/// The resources `ListAssociations` reports for `chat_configuration`, in the order it
/// reports them, reading every page.
async fn associated_resources(
    router: &MultiServiceRouter,
    chat_configuration: &str,
    max_results: Option<i64>,
) -> Vec<String> {
    let mut resources = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"ChatConfiguration": chat_configuration});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let answer = post(router, REGION, "/list-associations", &request).await;
        assert_eq!(
            answer.status, 200,
            "listing associations of {chat_configuration:?}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        let listed = answer.body["Associations"]
            .as_array()
            .unwrap_or_else(|| panic!("no Associations in {}", answer.body))
            .clone();
        for association in listed {
            resources.push(
                association["Resource"]
                    .as_str()
                    .unwrap_or_else(|| panic!("no Resource in {association}"))
                    .to_string(),
            );
        }
        next_token = answer.body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return resources;
        }
    }
}

#[tokio::test]
async fn a_resource_arn_no_action_of_this_account_can_be_minted_under_is_unauthorized() {
    let router = router();
    let chat_configuration = given_slack_channel_configuration(&router, "assoc-auth").await;
    // The action really exists, so nothing below is refused for want of a resource.
    let minted = given_custom_action(&router, "auth-probe").await;

    // Every row is an ARN no create of this account ever mints, differing in HOW it fails
    // to be one: the shape, the region segment, the account, the resource type, the
    // partition and the service. The first two do not satisfy the member's modelled
    // `@pattern` at all, which is the point — the authorization decision is taken before
    // the shape is judged, so they are refused alike.
    let rows: [(&str, String); 7] = [
        ("not an ARN at all", "not-a-valid-arn-at-all".to_string()),
        ("an empty resource", String::new()),
        (
            "a region segment filled in",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:custom-action/auth-probe"),
        ),
        (
            "another region",
            format!("arn:aws:chatbot:{OTHER_REGION}:{ACCOUNT}:custom-action/auth-probe"),
        ),
        (
            "another account",
            "arn:aws:chatbot::210987654321:custom-action/auth-probe".to_string(),
        ),
        (
            "another resource type",
            format!("arn:aws:chatbot::{ACCOUNT}:chat-configuration/slack-channel/assoc-auth"),
        ),
        (
            "another service",
            format!("arn:aws:lambda::{ACCOUNT}:custom-action/auth-probe"),
        ),
    ];

    for (case, resource) in &rows {
        for path in ["/associate-to-configuration", "/disassociate-from-configuration"] {
            let answer = post(
                &router,
                REGION,
                path,
                &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
            )
            .await;
            assert_eq!(
                answer.status, 403,
                "case {case:?} on {path}: expected HTTP 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                UNAUTHORIZED_EXCEPTION,
                "case {case:?} on {path}: expected the modelled {UNAUTHORIZED_EXCEPTION}, got {}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                UNAUTHORIZED_MESSAGE,
                "case {case:?} on {path}: expected the fixed refusal sentence, got {}",
                answer.body
            );
        }
    }

    // The refusals linked nothing, and the ARN a create DOES mint is served on the same
    // configuration — so the 403s came from the ARN's namespace and not from the call.
    assert_eq!(
        associated_resources(&router, &chat_configuration, None).await,
        Vec::<String>::new(),
        "a refused associate must link nothing"
    );
    let served = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": minted, "ChatConfiguration": chat_configuration}),
    )
    .await;
    assert_eq!(
        served.status, 201,
        "the minted ARN is served, got {} with {}",
        served.status, served.body
    );
}

#[tokio::test]
async fn a_target_that_cannot_host_the_link_is_refused_and_links_nothing() {
    let router = router();
    let resource = given_custom_action(&router, "unsupported-target").await;
    let chime = given_chime_webhook_configuration(&router, "chime-target").await;

    // Each row is a `ChatConfiguration` that cannot host an association, differing in WHY:
    // the chat client it configures takes none, or this account holds no configuration
    // under the ARN at all (of either client, and in either region).
    let rows: [(&str, String); 4] = [
        ("an Amazon Chime webhook configuration", chime.clone()),
        (
            "a Slack channel configuration that was never created",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/absent"),
        ),
        (
            "a Chime webhook configuration that was never created",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/chime-webhook/absent"),
        ),
        (
            "a Slack channel configuration of another region",
            format!(
                "arn:aws:chatbot:{OTHER_REGION}:{ACCOUNT}:chat-configuration/slack-channel/absent"
            ),
        ),
    ];

    for (case, chat_configuration) in &rows {
        // Fired eight ways at once against the SAME pair: the refusal is a property of the
        // target, so every caller is refused alike and none of them half-links anything.
        let request = json!({"Resource": resource, "ChatConfiguration": chat_configuration});
        let answers = tokio::join!(
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
            post(&router, REGION, "/associate-to-configuration", &request),
        );
        let answers = [
            answers.0, answers.1, answers.2, answers.3, answers.4, answers.5, answers.6, answers.7,
        ];
        for (index, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer.status, 500,
                "case {case:?} call {index}: expected HTTP 500, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                INTERNAL_SERVICE_ERROR,
                "case {case:?} call {index}: expected the modelled {INTERNAL_SERVICE_ERROR}, \
                 got {}",
                answer.body
            );
            assert_eq!(
                answer.message_capitalized(),
                INTERNAL_SERVER_ERROR_MESSAGE,
                "case {case:?} call {index}: expected the fixed refusal sentence, got {}",
                answer.body
            );
        }
        assert_eq!(
            associated_resources(&router, chat_configuration, None).await,
            Vec::<String>::new(),
            "case {case:?}: a refused associate must leave no association behind"
        );
    }

    // Unlinking from such a target is NOT an error: the caller asked for the resource not
    // to be linked, and it is not.
    for (case, chat_configuration) in &rows {
        let answer = post(
            &router,
            REGION,
            "/disassociate-from-configuration",
            &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
        )
        .await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: a disassociate is an idempotent no-op, got {} with {}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_link_round_trips_through_list_associations_and_both_calls_are_idempotent() {
    let router = router();
    let chat_configuration = given_slack_channel_configuration(&router, "assoc-roundtrip").await;
    let other_configuration = given_slack_channel_configuration(&router, "assoc-other").await;

    // The action names differ in length and charset, so what is linked cannot follow from
    // the spelling of one ARN.
    let action_names = ["a", "linked-action", &"z".repeat(64), "Mixed_Case-9"];
    let mut resources = Vec::new();
    for action_name in action_names {
        resources.push(given_custom_action(&router, action_name).await);
    }

    // Each row links one more resource, and twice: an associate is idempotent, so the
    // expectation after it is the whole set linked so far, in one stable order.
    let mut expected: Vec<String> = Vec::new();
    for resource in &resources {
        for attempt in 0..2 {
            let answer = post(
                &router,
                REGION,
                "/associate-to-configuration",
                &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
            )
            .await;
            assert_eq!(
                answer.status, 201,
                "case {resource:?} attempt {attempt}: expected HTTP 201, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer
                    .body
                    .as_object()
                    .is_none_or(|members| members.is_empty()),
                "case {resource:?} attempt {attempt}: the success carries no members, got {}",
                answer.body
            );
        }
        expected.push(resource.clone());
        expected.sort();
        assert_eq!(
            associated_resources(&router, &chat_configuration, None).await,
            expected,
            "case {resource:?}: expected every resource linked so far, exactly once"
        );
    }

    // The links belong to the configuration they were made against, not to the account.
    assert_eq!(
        associated_resources(&router, &other_configuration, None).await,
        Vec::<String>::new(),
        "another configuration holds none of these links"
    );

    // `MaxResults` splits the SAME total answer into pages, whatever the page size.
    for page_size in 1..=(expected.len() as i64 + 1) {
        assert_eq!(
            associated_resources(&router, &chat_configuration, Some(page_size)).await,
            expected,
            "case MaxResults={page_size}: paging must answer the same set in the same order"
        );
    }

    // Unlinking removes exactly one link, and a second unlink of the same resource is the
    // same empty success.
    for resource in &resources {
        for attempt in 0..2 {
            let answer = post(
                &router,
                REGION,
                "/disassociate-from-configuration",
                &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
            )
            .await;
            assert_eq!(
                answer.status, 204,
                "case {resource:?} attempt {attempt}: expected HTTP 204, got {} with {}",
                answer.status, answer.body
            );
        }
        expected.retain(|linked| linked != resource);
        assert_eq!(
            associated_resources(&router, &chat_configuration, None).await,
            expected,
            "case {resource:?}: expected only the remaining links"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_names_no_position_in_the_scan() {
    let router = router();
    let chat_configuration = given_slack_channel_configuration(&router, "assoc-tokens").await;
    let resource = given_custom_action(&router, "token-action").await;
    let associated = post(
        &router,
        REGION,
        "/associate-to-configuration",
        &json!({"Resource": resource, "ChatConfiguration": chat_configuration}),
    )
    .await;
    assert_eq!(
        associated.status, 201,
        "the associate answers 201, got {} with {}",
        associated.status, associated.body
    );

    // A cursor is one of this account's custom-action ARNs, base64-encoded. Each row is a
    // string that is not one, differing in how it fails to be: not base64, base64 of
    // something else, and a cursor of another listing of this service.
    let rows: [(&str, String); 4] = [
        ("not base64 at all", "!!!not-base64!!!".to_string()),
        (
            "base64 of a string that is no ARN",
            ::aws_smithy_types::base64::encode("page-2"),
        ),
        (
            "base64 of another account's action ARN",
            ::aws_smithy_types::base64::encode(
                "arn:aws:chatbot::210987654321:custom-action/token-action",
            ),
        ),
        (
            "a cursor of the configuration listing",
            ::aws_smithy_types::base64::encode(format!(
                "arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/assoc-tokens"
            )),
        ),
    ];
    for (case, token) in &rows {
        let answer = post(
            &router,
            REGION,
            "/list-associations",
            &json!({"ChatConfiguration": chat_configuration, "NextToken": token}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.code(),
            "InvalidRequestException",
            "case {case:?}: expected this service's client-input error class, got {}",
            answer.body
        );
    }

    // A configuration this account holds nothing under is not a refusal: it has no
    // association to report.
    let empty = associated_resources(
        &router,
        &format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/absent"),
        None,
    )
    .await;
    assert_eq!(
        empty,
        Vec::<String>::new(),
        "a configuration holding no association answers an empty list"
    );
}
