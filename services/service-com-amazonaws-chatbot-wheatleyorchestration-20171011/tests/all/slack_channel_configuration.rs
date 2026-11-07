// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Slack channel chat-configuration lifecycle, driven over
//! the wire (restJson1) through the same `MultiServiceRouter` the production router
//! serves with.
//!
//! Every assertion here is about the RULE each operation follows, exercised over a
//! table of inputs rather than one recorded request:
//!
//! * a created configuration is persisted and read back unchanged by
//!   `DescribeSlackChannelConfigurations` (whatever subset of the optional members the
//!   create carried);
//! * `MaxResults` splits the SAME total answer into pages, whatever the page size;
//! * `UpdateSlackChannelConfiguration` writes only the members the request carries;
//! * `TagResource` merges by `TagKey`;
//! * an ARN the store does not hold is a `ResourceNotFoundException`, and a duplicate
//!   `ConfigurationName` is a `ConflictException`.
//!
//! The create rule's other half — that a configuration can only be created for a Slack
//! workspace authorized with the account — lives in
//! `tests/all/slack_workspace_authorization.rs`; the workspaces the rows below name are
//! authorized by [`router`] through the state seam before any create runs.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{post, REGION};

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled
/// out here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";
const OTHER_IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-other-role";

/// The Slack workspaces the requests below name, each with the display name it is
/// authorized under.
///
/// A workspace has to be authorized with the account before a channel can be configured
/// in it, and on AWS that happens through the one-time Slack client (OAuth) handshake,
/// which no operation in this model performs — so [`router`] registers these through the
/// state seam, exactly as the console handshake would.
const AUTHORIZED_WORKSPACES: [(&str, &str); 4] = [
    ("T0123ABCD", "Primary Workspace"),
    ("T9999ZZZZ", "Secondary Workspace"),
    ("T2", "Two"),
    ("T3", "Three"),
];

/// A router mounting only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    for (slack_team_id, slack_team_name) in AUTHORIZED_WORKSPACES {
        service_com_amazonaws_chatbot_wheatleyorchestration_20171011::state::authorize_slack_workspace(
            &*engine,
            slack_team_id,
            slack_team_name,
        )
        .expect("the shared engine holds the chatbot store");
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_chatbot_wheatleyorchestration_20171011::EmulatedService::new(engine),
    );
    router
}

/// POST `body` to `path`, signed for `region`, and return the status and the parsed
/// JSON body (`Value::Null` for an empty body, as a 204 has).
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    path: &str,
    body: &Value,
) -> (u16, Value) {
    let answer = post(router, region, path, body).await;
    (answer.status, answer.body)
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

fn arn_in_region(region: &str, configuration_name: &str) -> String {
    format!(
        "arn:aws:chatbot:{region}:{ACCOUNT}:chat-configuration/slack-channel/{configuration_name}"
    )
}

fn expected_arn(configuration_name: &str) -> String {
    arn_in_region(REGION, configuration_name)
}

/// One row of the create table: the request body, and the configuration it must leave
/// in the store.
struct CreateCase {
    label: &'static str,
    request: Value,
    expected: Value,
}

/// The create cases, each differing from the others along what the create rule
/// generalises over: which optional members are present, how many entries the ARN
/// lists carry, which `LoggingLevel` value, and how many tags.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "only the required members",
            request: json!({
                "ConfigurationName": "minimal",
                "SlackTeamId": "T0123ABCD",
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("minimal"),
                "ConfigurationName": "minimal",
                "SlackTeamId": "T0123ABCD",
                "SlackTeamName": "Primary Workspace",
                "SlackChannelId": "C0123ABCD",
                "SlackChannelName": "C0123ABCD",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [],
                "GuardrailPolicyArns": [],
                "Tags": [],
                "State": "ENABLED",
            }),
        },
        CreateCase {
            label: "every member, two SNS topics and two tags",
            request: json!({
                "ConfigurationName": "full-config_1",
                "SlackTeamId": "T9999ZZZZ",
                "SlackChannelId": "C9999ZZZZ",
                "SlackChannelName": "ops-alerts",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                    "arn:aws:sns:us-west-2:000000000000:pages",
                ],
                "LoggingLevel": "INFO",
                "GuardrailPolicyArns": ["arn:aws:iam::000000000000:policy/ReadOnly"],
                "UserAuthorizationRequired": true,
                "Tags": [
                    {"TagKey": "env", "TagValue": "dev"},
                    {"TagKey": "team", "TagValue": "sre"},
                ],
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("full-config_1"),
                "ConfigurationName": "full-config_1",
                "SlackTeamId": "T9999ZZZZ",
                "SlackTeamName": "Secondary Workspace",
                "SlackChannelId": "C9999ZZZZ",
                "SlackChannelName": "ops-alerts",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                    "arn:aws:sns:us-west-2:000000000000:pages",
                ],
                "LoggingLevel": "INFO",
                "GuardrailPolicyArns": ["arn:aws:iam::000000000000:policy/ReadOnly"],
                "UserAuthorizationRequired": true,
                "Tags": [
                    {"TagKey": "env", "TagValue": "dev"},
                    {"TagKey": "team", "TagValue": "sre"},
                ],
                "State": "ENABLED",
            }),
        },
        CreateCase {
            // `LoggingLevel` is `^(ERROR|INFO|NONE)$` (smithy_query: CustomerCwLogLevel);
            // this row and the two around it cover all three published values.
            label: "logging ERROR and authorization not required",
            request: json!({
                "ConfigurationName": "logging-error",
                "SlackTeamId": "T2",
                "SlackChannelId": "C2",
                "SlackChannelName": "a",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:one"],
                "LoggingLevel": "ERROR",
                "UserAuthorizationRequired": false,
                "Tags": [{"TagKey": "only", "TagValue": ""}],
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("logging-error"),
                "ConfigurationName": "logging-error",
                "SlackTeamId": "T2",
                "SlackTeamName": "Two",
                "SlackChannelId": "C2",
                "SlackChannelName": "a",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:one"],
                "LoggingLevel": "ERROR",
                "GuardrailPolicyArns": [],
                "UserAuthorizationRequired": false,
                "Tags": [{"TagKey": "only", "TagValue": ""}],
                "State": "ENABLED",
            }),
        },
        CreateCase {
            label: "logging NONE and no tags",
            request: json!({
                "ConfigurationName": "logging-none",
                "SlackTeamId": "T3",
                "SlackChannelId": "C3",
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "LoggingLevel": "NONE",
                "Tags": [],
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("logging-none"),
                "ConfigurationName": "logging-none",
                "SlackTeamId": "T3",
                "SlackTeamName": "Three",
                "SlackChannelId": "C3",
                "SlackChannelName": "C3",
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "SnsTopicArns": [],
                "LoggingLevel": "NONE",
                "GuardrailPolicyArns": [],
                "Tags": [],
                "State": "ENABLED",
            }),
        },
    ]
}

/// A create request for `name`, carrying only the required members.
fn minimal_create(name: &str) -> Value {
    json!({
        "ConfigurationName": name,
        "SlackTeamId": "T0123ABCD",
        "SlackChannelId": "C0123ABCD",
        "IamRoleArn": IAM_ROLE_ARN,
    })
}

async fn create_ok(router: &MultiServiceRouter, request: &Value) -> Value {
    let (status, body) = call(router, "/create-slack-channel-configuration", request).await;
    assert_eq!(
        status, 200,
        "create {request}: expected 200, got {status} with {body}"
    );
    body.get("ChannelConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("create {request}: no ChannelConfiguration in {body}"))
}

async fn describe_ok(router: &MultiServiceRouter, request: &Value) -> Value {
    let (status, body) = call(router, "/describe-slack-channel-configurations", request).await;
    assert_eq!(
        status, 200,
        "describe {request}: expected 200, got {status} with {body}"
    );
    body
}

fn configurations(body: &Value) -> &Vec<Value> {
    body.get("SlackChannelConfigurations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no SlackChannelConfigurations list in {body}"))
}

#[tokio::test]
async fn a_created_configuration_is_persisted_and_described_exactly_as_created() {
    let router = router();
    let cases = create_cases();

    for case in &cases {
        let created = create_ok(&router, &case.request).await;
        assert_eq!(
            created, case.expected,
            "case {:?}: created configuration differs from the expected one, got {created}",
            case.label
        );

        // Read it back through Describe: the answer must come out of the store, not out
        // of the create request.
        let arn = case.expected["ChatConfigurationArn"].as_str().unwrap();
        let described = describe_ok(&router, &json!({"ChatConfigurationArn": arn})).await;
        let listed = configurations(&described);
        assert_eq!(
            listed.len(),
            1,
            "case {:?}: describe by ARN must answer exactly one configuration, got {described}",
            case.label
        );
        assert_eq!(
            listed[0], case.expected,
            "case {:?}: described configuration differs from the created one, got {}",
            case.label, listed[0]
        );
        assert!(
            described.get("NextToken").is_none(),
            "case {:?}: an unpaginated describe carries no NextToken, got {described}",
            case.label
        );
    }

    // Every configuration is in the unfiltered answer, exactly once.
    let all = describe_ok(&router, &json!({})).await;
    let listed = configurations(&all);
    assert_eq!(
        listed.len(),
        cases.len(),
        "the store must hold every created configuration, got {all}"
    );
    for case in &cases {
        assert!(
            listed.contains(&case.expected),
            "case {:?}: missing from the unfiltered describe, got {all}",
            case.label
        );
    }
}

#[tokio::test]
async fn describe_by_an_unknown_arn_answers_an_empty_list() {
    let router = router();
    create_ok(&router, &minimal_create("present")).await;

    let described = describe_ok(
        &router,
        &json!({"ChatConfigurationArn": expected_arn("absent")}),
    )
    .await;
    assert_eq!(
        configurations(&described),
        &Vec::<Value>::new(),
        "describe by an ARN the store does not hold answers an empty list, got {described}"
    );
}

#[tokio::test]
async fn max_results_pages_the_same_total_answer_whatever_the_page_size() {
    let router = router();
    // Nine configurations, so the page sizes below divide the total evenly, unevenly,
    // and not at all.
    let names: Vec<String> = (1..=9).map(|n| format!("cfg-{n}")).collect();
    for name in &names {
        create_ok(&router, &minimal_create(name)).await;
    }

    let unpaginated = describe_ok(&router, &json!({})).await;
    assert_eq!(
        configurations(&unpaginated).len(),
        names.len(),
        "a request with no MaxResults returns everything, got {unpaginated}"
    );
    assert!(
        unpaginated.get("NextToken").is_none(),
        "a request with no MaxResults returns no NextToken, got {unpaginated}"
    );

    // 1 and 100 are the published bounds of MaxResults (smithy_query: range 1..100);
    // 2, 4 and 9 sit inside it, dividing nine unevenly, unevenly, and exactly.
    for page_size in [1_u32, 2, 4, 9, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = Map::new();
            request.insert("MaxResults".to_string(), json!(page_size));
            if let Some(token) = &next_token {
                request.insert("NextToken".to_string(), json!(token));
            }
            let body = describe_ok(&router, &Value::Object(request)).await;
            let page = configurations(&body);
            pages += 1;
            assert!(
                page.len() as u32 <= page_size,
                "page size {page_size}: page {pages} holds {} configurations, got {body}",
                page.len()
            );
            assert!(
                !page.is_empty(),
                "page size {page_size}: page {pages} is empty, got {body}"
            );
            for configuration in page {
                seen.push(configuration["ConfigurationName"].as_str().unwrap().to_string());
            }
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let remaining = names.len() - seen.len();
            assert_eq!(
                next_token.is_some(),
                remaining > 0,
                "page size {page_size}: after page {pages}, {remaining} configurations are \
                 unreturned but NextToken is {next_token:?}"
            );
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= names.len() + 1,
                "page size {page_size}: pagination did not terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, names,
            "page size {page_size}: the pages must cover every configuration once, in ARN \
             order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let router = router();
    create_ok(&router, &minimal_create("paged-1")).await;
    create_ok(&router, &minimal_create("paged-2")).await;

    // A cursor is one of this region's configuration ARNs; every row below is a string
    // that is not one, however well formed it looks.
    let cases: Vec<(&str, String)> = vec![
        ("not base64 at all", "not-a-token".to_string()),
        (
            "base64 of something that is not an ARN",
            aws_smithy_types::base64::encode("AAAA"),
        ),
        (
            "base64 of an ARN from another region's scan",
            aws_smithy_types::base64::encode(arn_in_region("eu-west-1", "paged-1")),
        ),
        (
            "base64 of an ARN of another service",
            aws_smithy_types::base64::encode(IAM_ROLE_ARN),
        ),
    ];

    for (label, token) in &cases {
        let (status, body) = call(
            &router,
            "/describe-slack-channel-configurations",
            &json!({"MaxResults": 1, "NextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: an unissued NextToken is a 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterException",
            "case {label:?}: an unissued NextToken is an InvalidParameterException, got {body}"
        );
    }

    // The control: the token the service itself issued resumes the scan.
    let first = describe_ok(&router, &json!({"MaxResults": 1})).await;
    let token = first["NextToken"].as_str().expect("a NextToken").to_string();
    let second = describe_ok(&router, &json!({"MaxResults": 1, "NextToken": token})).await;
    assert_eq!(
        configurations(&second)[0]["ConfigurationName"],
        json!("paged-2"),
        "an issued token resumes after the first configuration, got {second}"
    );
}

/// One row of the update table: the members the update carries, and the members of the
/// stored configuration it must leave changed.
struct UpdateCase {
    label: &'static str,
    update: Value,
    changed: Value,
}

#[tokio::test]
async fn update_writes_only_the_members_the_request_carries() {
    let cases = vec![
        UpdateCase {
            label: "the required SlackChannelId alone",
            update: json!({"SlackChannelId": "CNEW0001"}),
            changed: json!({"SlackChannelId": "CNEW0001"}),
        },
        UpdateCase {
            label: "the logging level",
            update: json!({"SlackChannelId": "C0123ABCD", "LoggingLevel": "ERROR"}),
            changed: json!({"LoggingLevel": "ERROR"}),
        },
        UpdateCase {
            label: "the channel name",
            update: json!({"SlackChannelId": "C0123ABCD", "SlackChannelName": "renamed"}),
            changed: json!({"SlackChannelName": "renamed"}),
        },
        UpdateCase {
            label: "the SNS topic list grows",
            update: json!({
                "SlackChannelId": "C0123ABCD",
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:first",
                    "arn:aws:sns:us-east-1:000000000000:second",
                ],
            }),
            changed: json!({
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:first",
                    "arn:aws:sns:us-east-1:000000000000:second",
                ],
            }),
        },
        UpdateCase {
            label: "the SNS topic list is emptied",
            update: json!({"SlackChannelId": "C0123ABCD", "SnsTopicArns": []}),
            changed: json!({"SnsTopicArns": []}),
        },
        UpdateCase {
            label: "the role, the guardrails and the user-authorization flag at once",
            update: json!({
                "SlackChannelId": "C0123ABCD",
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "GuardrailPolicyArns": ["arn:aws:iam::000000000000:policy/ReadOnly"],
                "UserAuthorizationRequired": true,
            }),
            changed: json!({
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "GuardrailPolicyArns": ["arn:aws:iam::000000000000:policy/ReadOnly"],
                "UserAuthorizationRequired": true,
            }),
        },
    ];

    for case in &cases {
        // A fresh store per row, so each row starts from the same configuration.
        let router = router();
        let created = create_ok(
            &router,
            &json!({
                "ConfigurationName": "updatable",
                "SlackTeamId": "T0123ABCD",
                "SlackChannelId": "C0123ABCD",
                "SlackChannelName": "before",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
                "LoggingLevel": "INFO",
                "Tags": [{"TagKey": "env", "TagValue": "dev"}],
            }),
        )
        .await;
        let arn = created["ChatConfigurationArn"].as_str().unwrap().to_string();

        let mut expected = created.as_object().unwrap().clone();
        for (member, value) in case.changed.as_object().unwrap() {
            expected.insert(member.clone(), value.clone());
        }
        let expected = Value::Object(expected);

        let mut request = case.update.as_object().unwrap().clone();
        request.insert("ChatConfigurationArn".to_string(), json!(arn));
        let (status, body) = call(
            &router,
            "/update-slack-channel-configuration",
            &Value::Object(request),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} with {body}",
            case.label
        );
        assert_eq!(
            body["ChannelConfiguration"], expected,
            "case {:?}: the update must answer the updated record, got {body}",
            case.label
        );

        // The mutation is in the store, not just in the response.
        let described = describe_ok(&router, &json!({"ChatConfigurationArn": arn})).await;
        assert_eq!(
            configurations(&described)[0],
            expected,
            "case {:?}: a later describe must report the mutated record, got {described}",
            case.label
        );
    }
}

#[tokio::test]
async fn tag_resource_merges_by_tag_key_and_list_tags_reports_the_merged_set() {
    let router = router();
    let created = create_ok(
        &router,
        &json!({
            "ConfigurationName": "tagged",
            "SlackTeamId": "T0123ABCD",
            "SlackChannelId": "C0123ABCD",
            "IamRoleArn": IAM_ROLE_ARN,
            "Tags": [{"TagKey": "env", "TagValue": "dev"}],
        }),
    )
    .await;
    let arn = created["ChatConfigurationArn"].as_str().unwrap().to_string();

    // Each row is applied in turn: the expected set is the merge of everything so far,
    // keyed by TagKey, in key order.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the tags passed at create time",
            json!([]),
            json!([{"TagKey": "env", "TagValue": "dev"}]),
        ),
        (
            "a new key is added",
            json!([{"TagKey": "team", "TagValue": "sre"}]),
            json!([
                {"TagKey": "env", "TagValue": "dev"},
                {"TagKey": "team", "TagValue": "sre"},
            ]),
        ),
        (
            "an existing key is replaced",
            json!([{"TagKey": "env", "TagValue": "prod"}]),
            json!([
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "team", "TagValue": "sre"},
            ]),
        ),
        (
            "several keys at once, one of them a replacement",
            json!([
                {"TagKey": "team", "TagValue": "platform"},
                {"TagKey": "cost-center", "TagValue": "42"},
            ]),
            json!([
                {"TagKey": "cost-center", "TagValue": "42"},
                {"TagKey": "env", "TagValue": "prod"},
                {"TagKey": "team", "TagValue": "platform"},
            ]),
        ),
    ];

    for (label, tags, expected) in &cases {
        if !tags.as_array().unwrap().is_empty() {
            let (status, body) = call(
                &router,
                "/tag-resource",
                &json!({"ResourceARN": arn, "Tags": tags}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} with {body}"
            );
        }

        let (status, body) = call(
            &router,
            "/list-tags-for-resource",
            &json!({"ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Tags"], *expected,
            "case {label:?}: list-tags must report the merged set, got {body}"
        );

        // The configuration itself carries the same tags.
        let described = describe_ok(&router, &json!({"ChatConfigurationArn": arn})).await;
        assert_eq!(
            configurations(&described)[0]["Tags"], *expected,
            "case {label:?}: the configuration must carry the merged tags, got {described}"
        );
    }
}

#[tokio::test]
async fn a_duplicate_configuration_name_conflicts() {
    let router = router();
    create_ok(&router, &minimal_create("taken")).await;

    let (status, body) = call(
        &router,
        "/create-slack-channel-configuration",
        &minimal_create("taken"),
    )
    .await;
    assert_eq!(
        status, 409,
        "a duplicate ConfigurationName is a 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "a duplicate ConfigurationName is a ConflictException, got {body}"
    );

    // The conflict left the one stored configuration alone.
    let described = describe_ok(&router, &json!({})).await;
    assert_eq!(
        configurations(&described).len(),
        1,
        "a refused create must store nothing, got {described}"
    );
}

#[tokio::test]
async fn delete_removes_the_configuration_and_a_second_delete_is_not_found() {
    let router = router();
    let created = create_ok(
        &router,
        &json!({
            "ConfigurationName": "doomed",
            "SlackTeamId": "T0123ABCD",
            "SlackChannelId": "C0123ABCD",
            "IamRoleArn": IAM_ROLE_ARN,
            "Tags": [{"TagKey": "env", "TagValue": "dev"}],
        }),
    )
    .await;
    let arn = created["ChatConfigurationArn"].as_str().unwrap().to_string();
    create_ok(&router, &minimal_create("survivor")).await;

    let (status, body) = call(
        &router,
        "/delete-slack-channel-configuration",
        &json!({"ChatConfigurationArn": arn}),
    )
    .await;
    assert_eq!(
        status, 204,
        "a delete answers 204, got {status} with {body}"
    );

    let described = describe_ok(&router, &json!({})).await;
    let remaining: Vec<&str> = configurations(&described)
        .iter()
        .map(|configuration| configuration["ConfigurationName"].as_str().unwrap())
        .collect();
    assert_eq!(
        remaining,
        vec!["survivor"],
        "a deleted configuration is gone and its neighbours are not, got {described}"
    );

    // Its tags went with it: the ARN is no longer a resource at all.
    for path in [
        "/delete-slack-channel-configuration",
        "/update-slack-channel-configuration",
    ] {
        let request = json!({"ChatConfigurationArn": arn, "SlackChannelId": "C0123ABCD"});
        let (status, body) = call(&router, path, &request).await;
        assert_eq!(
            status, 404,
            "{path} on a deleted ARN is a 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{path} on a deleted ARN is a ResourceNotFoundException, got {body}"
        );
    }
    for path in ["/tag-resource", "/list-tags-for-resource"] {
        let request = json!({"ResourceARN": arn, "Tags": [{"TagKey": "k", "TagValue": "v"}]});
        let (status, body) = call(&router, path, &request).await;
        assert_eq!(
            status, 404,
            "{path} on a deleted ARN is a 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "{path} on a deleted ARN is a ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn an_arn_no_configuration_was_ever_created_for_is_not_found() {
    let router = router();
    let unknown = expected_arn("never-created");

    let cases: Vec<(&str, Value)> = vec![
        (
            "/update-slack-channel-configuration",
            json!({"ChatConfigurationArn": unknown, "SlackChannelId": "C0123ABCD"}),
        ),
        (
            "/delete-slack-channel-configuration",
            json!({"ChatConfigurationArn": unknown}),
        ),
        (
            "/tag-resource",
            json!({"ResourceARN": unknown, "Tags": [{"TagKey": "k", "TagValue": "v"}]}),
        ),
        (
            "/list-tags-for-resource",
            json!({"ResourceARN": unknown}),
        ),
    ];

    for (path, request) in &cases {
        let (status, body) = call(&router, path, request).await;
        assert_eq!(
            status, 404,
            "case {path:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {path:?}: expected a ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_configuration_belongs_to_the_region_its_request_was_signed_for() {
    let router = router();
    // Chat configurations are regional, so ONE name can exist once per region and each
    // instance is only listed in its own.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        let (status, body) = call_in_region(
            &router,
            region,
            "/create-slack-channel-configuration",
            &minimal_create("shared-name"),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region:?}: the same name in another region is not a conflict, got \
             {status} with {body}"
        );
        assert_eq!(
            body["ChannelConfiguration"]["ChatConfigurationArn"],
            json!(arn_in_region(region, "shared-name")),
            "region {region:?}: the minted ARN must carry the signing region, got {body}"
        );
    }

    for region in regions {
        let (status, body) = call_in_region(
            &router,
            region,
            "/describe-slack-channel-configurations",
            &json!({}),
        )
        .await;
        assert_eq!(status, 200, "region {region:?}: expected 200, got {body}");
        let listed: Vec<&Value> = configurations(&body).iter().collect();
        assert_eq!(
            listed.len(),
            1,
            "region {region:?}: a listing holds only its own region's configurations, got \
             {body}"
        );
        assert_eq!(
            listed[0]["ChatConfigurationArn"],
            json!(arn_in_region(region, "shared-name")),
            "region {region:?}: the listed configuration must be this region's, got {body}"
        );
    }
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_the_configuration_quota() {
    // `state::MAX_SLACK_CHANNEL_CONFIGURATIONS`, duplicated here so the test fails if
    // the bound moves without the test being revisited.
    const QUOTA: usize = 100;
    let router = router();
    for n in 0..QUOTA {
        let (status, body) = call(
            &router,
            "/create-slack-channel-configuration",
            &minimal_create(&format!("cfg-{n:04}")),
        )
        .await;
        assert_eq!(
            status, 200,
            "configuration {n} is within the quota of {QUOTA}, got {status} with {body}"
        );
    }

    let (status, body) = call(
        &router,
        "/create-slack-channel-configuration",
        &minimal_create("one-too-many"),
    )
    .await;
    assert_eq!(
        status, 403,
        "a create past the quota of {QUOTA} is a 403, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "a create past the quota is a LimitExceededException, got {body}"
    );

    // The bound is the whole STORE's, so signing for another region does not hand the
    // caller a fresh budget.
    let (status, body) = call_in_region(
        &router,
        "eu-west-1",
        "/create-slack-channel-configuration",
        &minimal_create("other-region"),
    )
    .await;
    assert_eq!(
        status, 403,
        "a full store refuses a create signed for any region, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "a full store refuses a create signed for any region with LimitExceededException, \
         got {body}"
    );

    // The refused create stored nothing, and the full store is still readable one
    // bounded page at a time.
    let described = describe_ok(&router, &json!({})).await;
    assert_eq!(
        configurations(&described).len(),
        QUOTA,
        "the store must hold exactly the quota, got {described}"
    );
    assert!(
        described.get("NextToken").is_none(),
        "a full store fits in one default page, got {described}"
    );
    let (status, body) = call(
        &router,
        "/describe-slack-channel-configurations",
        &json!({"ChatConfigurationArn": expected_arn("one-too-many")}),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} with {body}");
    assert_eq!(
        configurations(&body),
        &Vec::<Value>::new(),
        "a refused create must store nothing, got {body}"
    );
}

#[tokio::test]
async fn a_tag_merge_past_the_tag_limit_is_refused_and_changes_nothing() {
    // The modelled `TagList` is `length 0..200` (smithy_query: TagResourceRequest), so
    // 200 is the most tags one resource carries.
    const MAX_TAGS: usize = 200;

    let router = router();
    let tags_from = |range: std::ops::Range<usize>| -> Value {
        Value::Array(
            range
                .map(|n| json!({"TagKey": format!("key-{n:04}"), "TagValue": "v"}))
                .collect(),
        )
    };

    let created_tags = tags_from(0..150);
    let created = create_ok(
        &router,
        &json!({
            "ConfigurationName": "many-tags",
            "SlackTeamId": "T0123ABCD",
            "SlackChannelId": "C0123ABCD",
            "IamRoleArn": IAM_ROLE_ARN,
            "Tags": created_tags,
        }),
    )
    .await;
    let arn = created["ChatConfigurationArn"].as_str().unwrap().to_string();

    // Each row merges onto the stored 150 keys; only the last would exceed 200.
    let cases: Vec<(&str, Value, Option<usize>)> = vec![
        ("replacing 50 keys leaves 150", tags_from(0..50), Some(150)),
        ("50 new keys reach exactly the limit", tags_from(150..200), Some(MAX_TAGS)),
        ("one more key would exceed the limit", tags_from(200..201), None),
    ];

    for (label, tags, expected_total) in &cases {
        let (status, body) = call(
            &router,
            "/tag-resource",
            &json!({"ResourceARN": arn, "Tags": tags}),
        )
        .await;
        match expected_total {
            Some(_) => assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} with {body}"
            ),
            None => {
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected 400, got {status} with {body}"
                );
                // The stored total the merge would reach is a request-constraint
                // violation, so it carries this service's generic client-input class and
                // states no message — not the modelled `TooManyTagsException`, which words
                // the request LIST being too long. `tests/all/tag_resource_cap.rs` asserts
                // that rule across every kind of resource that carries tags.
                assert_eq!(
                    error_code(&body),
                    "InvalidRequestException",
                    "case {label:?}: expected an InvalidRequestException, got {body}"
                );
                assert_eq!(
                    body.get("message"),
                    Some(&Value::Null),
                    "case {label:?}: this refusal states no message, got {body}"
                );
            }
        }

        // A refused merge leaves the stored tags exactly as they were.
        let (status, body) = call(
            &router,
            "/list-tags-for-resource",
            &json!({"ResourceARN": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["Tags"].as_array().map(Vec::len),
            Some(expected_total.unwrap_or(MAX_TAGS)),
            "case {label:?}: the stored tag count is wrong, got {body}"
        );
    }
}
