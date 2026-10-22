// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Amazon Chime webhook chat-configuration create/read
//! lifecycle, driven over the wire (restJson1) through the same `MultiServiceRouter` the
//! production router serves with.
//!
//! The rule every assertion here is about: creating a chat configuration mints a
//! region-qualified `ChatConfigurationArn` from the requested `ConfigurationName`,
//! persists every member the request carried (and the documented default for each
//! optional member it omitted), reports the configuration as `ENABLED`, and answers from
//! that stored record — so a later read answers exactly the same configuration, while a
//! second create for a `ConfigurationName` the region already holds is refused with the
//! modelled 409 and leaves the stored one untouched.
//!
//! The rule is exercised over a TABLE spanning what it generalises over — which optional
//! members a create carries, which published `LoggingLevel` value, how many SNS topics,
//! how many tags, and the shape and length of the description and the name — rather than
//! over one recorded request. The recorded case is one row.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for, so the minted ARNs carry it.
const REGION: &str = "us-east-1";

/// Another region, so the regional scoping of a listing can be checked.
const OTHER_REGION: &str = "us-west-2";

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), spelled
/// out here so the test pins the ARN a client actually sees.
const ACCOUNT: &str = "000000000000";

const IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-role";
const OTHER_IAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/chatbot-other-role";

const CREATE_PATH: &str = "/create-chime-webhook-configuration";
const DESCRIBE_PATH: &str = "/describe-chime-webhook-configurations";

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

/// POST `body` to `path`, signed for `region`, and return the status and the parsed
/// JSON body (`Value::Null` for an empty body).
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

fn arn_in_region(region: &str, configuration_name: &str) -> String {
    format!(
        "arn:aws:chatbot:{region}:{ACCOUNT}:chat-configuration/chime-webhook/{configuration_name}"
    )
}

fn expected_arn(configuration_name: &str) -> String {
    arn_in_region(REGION, configuration_name)
}

/// A `WebhookUrl` satisfying the published pattern
/// (`smithy_query` ChimeWebhookUrl:
/// `^https://hooks\.chime\.aws/incomingwebhooks/[A-Za-z0-9\-]+?\?token=[A-Za-z0-9\-]+$`),
/// distinct per `id` so no two rows share a webhook destination.
fn webhook_url(id: &str) -> String {
    format!("https://hooks.chime.aws/incomingwebhooks/{id}?token=tok-{id}")
}

/// One row of the create table: the request body, and the configuration it must leave
/// in the store.
struct CreateCase {
    label: &'static str,
    request: Value,
    expected: Value,
}

/// The create cases, each differing from the others along what the create rule
/// generalises over: which optional members are present, how many SNS topics the
/// required list carries, which `LoggingLevel` value, how many tags, and the length of
/// the description and the name.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "only the required members",
            request: json!({
                "ConfigurationName": "minimal",
                "WebhookDescription": "Room/Webhook",
                "WebhookUrl": webhook_url("minimal"),
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
                "IamRoleArn": IAM_ROLE_ARN,
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("minimal"),
                "ConfigurationName": "minimal",
                "WebhookDescription": "Room/Webhook",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
                "Tags": [],
                "State": "ENABLED",
            }),
        },
        CreateCase {
            // `LoggingLevel` is `^(ERROR|INFO|NONE)$` (smithy_query: CustomerCwLogLevel);
            // this row and the two below cover all three published values.
            label: "logging INFO, three SNS topics and two tags",
            request: json!({
                "ConfigurationName": "full-config_1",
                "WebhookDescription": "Ops Room/Pager Webhook",
                "WebhookUrl": webhook_url("full-1"),
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                    "arn:aws:sns:us-west-2:000000000000:pages",
                    "arn:aws:sns:eu-west-1:000000000000:audit",
                ],
                "LoggingLevel": "INFO",
                "Tags": [
                    {"TagKey": "env", "TagValue": "dev"},
                    {"TagKey": "team", "TagValue": "sre"},
                ],
            "IamRoleArn": IAM_ROLE_ARN,
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("full-config_1"),
                "ConfigurationName": "full-config_1",
                "WebhookDescription": "Ops Room/Pager Webhook",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [
                    "arn:aws:sns:us-east-1:000000000000:alerts",
                    "arn:aws:sns:us-west-2:000000000000:pages",
                    "arn:aws:sns:eu-west-1:000000000000:audit",
                ],
                "LoggingLevel": "INFO",
                "Tags": [
                    {"TagKey": "env", "TagValue": "dev"},
                    {"TagKey": "team", "TagValue": "sre"},
                ],
                "State": "ENABLED",
            }),
        },
        CreateCase {
            label: "logging ERROR, one tag with an empty value and a one-character description",
            request: json!({
                "ConfigurationName": "logging-error",
                // `WebhookDescription` is `length 1..255` (smithy_query:
                // ChimeWebhookDescription); this row sits at its minimum.
                "WebhookDescription": "a",
                "WebhookUrl": webhook_url("error"),
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:one"],
                "LoggingLevel": "ERROR",
                "Tags": [{"TagKey": "only", "TagValue": ""}],
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("logging-error"),
                "ConfigurationName": "logging-error",
                "WebhookDescription": "a",
                "IamRoleArn": OTHER_IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:one"],
                "LoggingLevel": "ERROR",
                "Tags": [{"TagKey": "only", "TagValue": ""}],
                "State": "ENABLED",
            }),
        },
        CreateCase {
            label: "logging NONE, an explicitly empty tag list and the longest description",
            request: json!({
                "ConfigurationName": "logging-NONE_9",
                // The maximum `WebhookDescription` length, 255 characters.
                "WebhookDescription": "d".repeat(255),
                "WebhookUrl": webhook_url("none"),
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:two"],
                "LoggingLevel": "NONE",
                "Tags": [],
                "IamRoleArn": IAM_ROLE_ARN,
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn("logging-NONE_9"),
                "ConfigurationName": "logging-NONE_9",
                "WebhookDescription": "d".repeat(255),
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:two"],
                "LoggingLevel": "NONE",
                "Tags": [],
                "State": "ENABLED",
            }),
        },
        CreateCase {
            // `ConfigurationName` is `length 1..128`, `^[A-Za-z0-9-_]+$` (smithy_query:
            // ConfigurationName); this row sits at its maximum length.
            label: "a 128-character configuration name",
            request: json!({
                "ConfigurationName": "n".repeat(128),
                "WebhookDescription": "Longest/Name",
                "WebhookUrl": webhook_url("longest"),
                "SnsTopicArns": [],
                "IamRoleArn": IAM_ROLE_ARN,
            }),
            expected: json!({
                "ChatConfigurationArn": expected_arn(&"n".repeat(128)),
                "ConfigurationName": "n".repeat(128),
                "WebhookDescription": "Longest/Name",
                "IamRoleArn": IAM_ROLE_ARN,
                "SnsTopicArns": [],
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
        "WebhookDescription": format!("Room/{name}"),
        "WebhookUrl": webhook_url(name),
        "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:alerts"],
        "IamRoleArn": IAM_ROLE_ARN,
    })
}

async fn create_ok(router: &MultiServiceRouter, request: &Value) -> Value {
    let (status, body) = call(router, CREATE_PATH, request).await;
    assert_eq!(
        status, 201,
        "create {request}: expected 201, got {status} with {body}"
    );
    body.get("WebhookConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("create {request}: no WebhookConfiguration in {body}"))
}

async fn describe_ok(router: &MultiServiceRouter, request: &Value) -> Value {
    let (status, body) = call(router, DESCRIBE_PATH, request).await;
    assert_eq!(
        status, 200,
        "describe {request}: expected 200, got {status} with {body}"
    );
    body
}

fn configurations(body: &Value) -> &Vec<Value> {
    body.get("WebhookConfigurations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no WebhookConfigurations list in {body}"))
}

#[tokio::test]
async fn a_created_webhook_is_persisted_and_described_exactly_as_created() {
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
async fn no_response_carries_the_webhook_url() {
    let router = router();

    // `ChimeWebhookConfiguration` models no `WebhookUrl` member (smithy_query: shape
    // ChimeWebhookConfiguration) and the input's is `@sensitive`, so neither the create
    // nor the read may hand the webhook destination back.
    for case in create_cases() {
        let url = case.request["WebhookUrl"].as_str().expect("a WebhookUrl");
        let created = create_ok(&router, &case.request).await;
        assert!(
            !created.to_string().contains(url),
            "case {:?}: the create response must not carry the WebhookUrl, got {created}",
            case.label
        );
    }

    let all = describe_ok(&router, &json!({})).await;
    for case in create_cases() {
        let url = case.request["WebhookUrl"].as_str().expect("a WebhookUrl");
        assert!(
            !all.to_string().contains(url),
            "case {:?}: the describe response must not carry the WebhookUrl, got {all}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_duplicate_configuration_name_is_a_conflict_and_leaves_the_stored_one_untouched() {
    let router = router();

    // The duplicate differs from the first create in every other member, so a create
    // that overwrote the stored configuration would be visible.
    let first = json!({
        "ConfigurationName": "duplicated",
        "WebhookDescription": "First/Webhook",
        "WebhookUrl": webhook_url("first"),
        "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:first"],
        "LoggingLevel": "INFO",
        "Tags": [{"TagKey": "take", "TagValue": "first"}],
        "IamRoleArn": IAM_ROLE_ARN,
    });
    let second = json!({
        "ConfigurationName": "duplicated",
        "WebhookDescription": "Second/Webhook",
        "WebhookUrl": webhook_url("second"),
        "SnsTopicArns": ["arn:aws:sns:us-east-1:000000000000:second"],
        "LoggingLevel": "NONE",
        "Tags": [{"TagKey": "take", "TagValue": "second"}],
        "IamRoleArn": OTHER_IAM_ROLE_ARN,
    });

    let created = create_ok(&router, &first).await;

    let (status, body) = call(&router, CREATE_PATH, &second).await;
    assert_eq!(
        status, 409,
        "a duplicate ConfigurationName is a 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "a duplicate ConfigurationName is a ConflictException, got {body}"
    );

    let described = describe_ok(&router, &json!({})).await;
    assert_eq!(
        configurations(&described),
        &vec![created],
        "a refused duplicate must leave exactly the first configuration stored, got {described}"
    );

    // The same name in ANOTHER region is a different configuration, so it is created.
    let (status, body) = call_in_region(&router, OTHER_REGION, CREATE_PATH, &second).await;
    assert_eq!(
        status, 201,
        "the same name in another region is a different configuration, got {status} with {body}"
    );
    assert_eq!(
        body["WebhookConfiguration"]["ChatConfigurationArn"],
        json!(arn_in_region(OTHER_REGION, "duplicated")),
        "a configuration's ARN carries the region it was created in, got {body}"
    );
}

#[tokio::test]
async fn a_listing_answers_only_its_own_regions_configurations() {
    let router = router();

    // One configuration per region, each named for its region so the answers cannot be
    // confused.
    for region in [REGION, OTHER_REGION] {
        let (status, body) =
            call_in_region(&router, region, CREATE_PATH, &minimal_create(region)).await;
        assert_eq!(
            status, 201,
            "region {region}: expected 201, got {status} with {body}"
        );
        assert_eq!(
            body["WebhookConfiguration"]["ChatConfigurationArn"],
            json!(arn_in_region(region, region)),
            "region {region}: the minted ARN must carry the request's region, got {body}"
        );
    }

    for region in [REGION, OTHER_REGION] {
        let (status, body) = call_in_region(&router, region, DESCRIBE_PATH, &json!({})).await;
        assert_eq!(
            status, 200,
            "region {region}: expected 200, got {status} with {body}"
        );
        let listed = configurations(&body);
        assert_eq!(
            listed.len(),
            1,
            "region {region}: a listing answers only its own region's configurations, got {body}"
        );
        assert_eq!(
            listed[0]["ConfigurationName"],
            json!(region),
            "region {region}: the wrong region's configuration is listed, got {body}"
        );
    }
}

#[tokio::test]
async fn describe_by_an_unknown_arn_answers_an_empty_list() {
    let router = router();
    create_ok(&router, &minimal_create("present")).await;

    // An ARN is a FILTER, not a lookup: one the store does not hold matches nothing,
    // whether no configuration has that name or the ARN belongs to another region or to
    // another kind of chat configuration.
    let cases = [
        ("a name the store does not hold", expected_arn("absent")),
        ("another region's ARN", arn_in_region(OTHER_REGION, "present")),
        (
            "a Slack channel configuration's ARN",
            format!("arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/present"),
        ),
    ];

    for (label, arn) in &cases {
        let described = describe_ok(&router, &json!({"ChatConfigurationArn": arn})).await;
        assert_eq!(
            configurations(&described),
            &Vec::<Value>::new(),
            "case {label:?}: describe by an ARN the store does not hold answers an empty list, \
             got {described}"
        );
    }
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
                seen.push(
                    configuration["ConfigurationName"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                );
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

    // A cursor is one of this region's Chime webhook configuration ARNs; every row
    // below is a string that is not one, however well formed it looks.
    let cases: Vec<(&str, String)> = vec![
        ("not base64 at all", "not-a-token".to_string()),
        (
            "base64 of something that is not an ARN",
            aws_smithy_types::base64::encode("AAAA"),
        ),
        (
            "base64 of an ARN from another region's scan",
            aws_smithy_types::base64::encode(arn_in_region(OTHER_REGION, "paged-1")),
        ),
        (
            "base64 of a Slack channel configuration's ARN",
            aws_smithy_types::base64::encode(format!(
                "arn:aws:chatbot:{REGION}:{ACCOUNT}:chat-configuration/slack-channel/paged-1"
            )),
        ),
        (
            "base64 of an ARN of another service",
            aws_smithy_types::base64::encode(IAM_ROLE_ARN),
        ),
    ];

    for (label, token) in &cases {
        let (status, body) = call(
            &router,
            DESCRIBE_PATH,
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
    let token = first["NextToken"]
        .as_str()
        .expect("a NextToken")
        .to_string();
    let second = describe_ok(&router, &json!({"MaxResults": 1, "NextToken": token})).await;
    assert_eq!(
        configurations(&second)[0]["ConfigurationName"],
        json!("paged-2"),
        "an issued token resumes after the first configuration, got {second}"
    );
}

#[tokio::test]
async fn a_create_past_the_store_bound_is_refused_with_a_limit_exceeded_error() {
    let router = router();

    // `state::MAX_CHIME_WEBHOOK_CONFIGURATIONS`, duplicated here so the test fails if
    // the bound moves without the rule being reconsidered.
    let bound = 100usize;
    for n in 0..bound {
        create_ok(&router, &minimal_create(&format!("bulk-{n:03}"))).await;
    }

    let (status, body) = call(&router, CREATE_PATH, &minimal_create("one-too-many")).await;
    assert_eq!(
        status, 403,
        "a create past the bound is a 403, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceededException",
        "a create past the bound is a LimitExceededException, got {body}"
    );

    let described = describe_ok(&router, &json!({"MaxResults": 100})).await;
    assert_eq!(
        configurations(&described).len(),
        bound,
        "a refused create must persist nothing, got {} configurations",
        configurations(&described).len()
    );
}
