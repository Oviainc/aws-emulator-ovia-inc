// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT rules store: `CreateTopicRule` and the
//! `GetTopicRule` reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `CreateTopicRule` RECORDS a rule under its name, and a later
//! `GetTopicRule` of that name reports that record — the rule's ARN
//! (`arn:aws:iot:<region>:<account>:rule/<ruleName>`) and the stored `TopicRule`
//! member-for-member, with the payload's optional members resolved on the write (an
//! absent `ruleDisabled` is an enabled rule, an absent `awsIotSqlVersion` is the SQL
//! reference's default version) and its `createdAt` stable across reads. A second
//! create under a name the store already holds is refused
//! `ResourceAlreadyExistsException` (409) and changes nothing; a name the store does
//! not hold is refused with the operation's modelled `UnauthorizedException` (401)
//! and no rule.
//!
//! The table spans what that rule generalises over — the form and length of a rule
//! name, how many actions a payload carries and of which action types, which
//! optional payload members it carries, whether it carries an `errorAction`, and the
//! region the call is signed for — rather than one recorded request.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::MAX_TOPIC_RULES;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// The `awsIotSqlVersion` a rule created without one must report.
///
/// From `state::DEFAULT_AWS_IOT_SQL_VERSION`: the AWS IoT SQL reference's "SQL
/// versions" section lists `2015-10-08`, `2016-03-23` and `beta`, and `2015-10-08`
/// is the version used when a rule names none.
const DEFAULT_SQL_VERSION: &str = "2015-10-08";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing
/// name and `region`: the scope is how the router picks the claiming service, and
/// how the service picks the region its ARNs are scoped to. The signature itself is
/// never checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `rule` member of a `GetTopicRule` response.
    fn rule(&self) -> Option<&Value> {
        self.body.get("rule")
    }

    /// A member of the reported `TopicRule`, or `None` when the rule or the member
    /// is absent.
    fn rule_member(&self, name: &str) -> Option<&Value> {
        self.rule().and_then(|rule| rule.get(name))
    }

    /// `rule.createdAt` as the epoch seconds the restJson1 protocol renders a
    /// timestamp as.
    fn created_at(&self) -> Option<f64> {
        self.rule_member("createdAt").and_then(Value::as_f64)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        region: &str,
        tags: Option<&str>,
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json");
        // `CreateTopicRule$tags` is bound to the `x-amz-tagging` header as a raw
        // `key1=value1&key2=value2` string, not to a member of the payload.
        if let Some(tags) = tags {
            builder = builder.header("x-amz-tagging", tags);
        }
        let request = builder
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_rule(&self, rule_name: &str, payload: &Value) -> Reply {
        self.create_rule_in(rule_name, payload, REGION, None).await
    }

    async fn create_rule_in(
        &self,
        rule_name: &str,
        payload: &Value,
        region: &str,
        tags: Option<&str>,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/rules/{rule_name}"),
            Some(payload),
            region,
            tags,
        )
        .await
    }

    async fn get_rule(&self, rule_name: &str) -> Reply {
        self.call("GET", &format!("/rules/{rule_name}"), None, REGION, None)
            .await
    }
}

/// The ARN a rule created in `region` must carry.
fn expected_arn(region: &str, rule_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:rule/{rule_name}")
}

/// Epoch seconds now, for bounding a reported `createdAt`.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One create request: a label, the rule name, the region it is signed for, the
/// `TopicRulePayload` body, the `x-amz-tagging` header it carries, and what the
/// payload's optional members resolve to once stored.
struct Case {
    label: &'static str,
    rule_name: String,
    region: &'static str,
    payload: Value,
    tags: Option<&'static str>,
    /// What `rule.ruleDisabled` must report: the payload's value, or `false` when it
    /// carried none.
    rule_disabled: bool,
    /// What `rule.awsIotSqlVersion` must report: the payload's value, or
    /// [`DEFAULT_SQL_VERSION`] when it carried none.
    sql_version: &'static str,
}

/// The table every case list below is built from. It spans what the rule
/// generalises over:
///
/// * the form and length of `ruleName` — `RuleName` is modelled
///   `length 1..128, pattern ^[a-zA-Z0-9_]+$`
///   (`smithy_query(query="operation",operation="GetTopicRule")`), so the rows
///   include the shortest legal name, the longest, and one using every character
///   class the pattern allows;
/// * how many actions the payload carries — `ActionList` is modelled
///   `length 0..10` (`smithy_query(query="shape",shape="TopicRulePayload")`), so the
///   rows include the empty list the minimum admits, one action, three actions, and
///   the ten the maximum admits;
/// * which action a payload names — `Action`'s members are one per destination
///   service (`smithy_query(query="shape",shape="Action")`), so the rows span
///   `republish`, `lambda`, `sqs`, `sns` and `kinesis`, none of which the recorded
///   case cited;
/// * which optional payload members it carries — `description`, `ruleDisabled`
///   (absent, `false`, `true`), `awsIotSqlVersion` (absent and each version the SQL
///   reference lists) and `errorAction`;
/// * the region the call is signed for, which is the region the rule's ARN is
///   scoped to.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "only the two required members",
            rule_name: "minimal_rule".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT * FROM 'sensors/+/telemetry'",
                "actions": [],
            }),
            tags: None,
            rule_disabled: false,
            sql_version: DEFAULT_SQL_VERSION,
        },
        Case {
            label: "one republish action and a description",
            rule_name: "republish_rule".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT temperature FROM 'sensors/#' WHERE temperature > 30",
                "description": "fan out hot readings",
                "actions": [{
                    "republish": {
                        "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                        "topic": "alerts/hot",
                        "qos": 1,
                    },
                }],
            }),
            tags: None,
            rule_disabled: false,
            sql_version: DEFAULT_SQL_VERSION,
        },
        Case {
            label: "three actions of three different types, explicitly enabled",
            rule_name: "fanout_rule_3".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT * FROM 'devices/+/events'",
                "ruleDisabled": false,
                "awsIotSqlVersion": "2016-03-23",
                "actions": [
                    { "lambda": { "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:on-event" } },
                    {
                        "sqs": {
                            "roleArn": "arn:aws:iam::000000000000:role/iot-sqs",
                            "queueUrl": "https://sqs.us-east-1.amazonaws.com/000000000000/events",
                            "useBase64": false,
                        },
                    },
                    {
                        "sns": {
                            "targetArn": "arn:aws:sns:us-east-1:000000000000:events",
                            "roleArn": "arn:aws:iam::000000000000:role/iot-sns",
                            "messageFormat": "JSON",
                        },
                    },
                ],
            }),
            tags: None,
            rule_disabled: false,
            sql_version: "2016-03-23",
        },
        Case {
            label: "a disabled rule with an errorAction",
            rule_name: "disabled_rule".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT * FROM 'devices/+/shadow'",
                "ruleDisabled": true,
                "awsIotSqlVersion": "beta",
                "actions": [{
                    "kinesis": {
                        "roleArn": "arn:aws:iam::000000000000:role/iot-kinesis",
                        "streamName": "shadow-updates",
                        "partitionKey": "${topic()}",
                    },
                }],
                "errorAction": {
                    "republish": {
                        "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                        "topic": "errors",
                    },
                },
            }),
            tags: None,
            rule_disabled: true,
            sql_version: "beta",
        },
        Case {
            label: "the ten actions the ActionList maximum admits",
            rule_name: "ten_action_rule".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT * FROM 'bulk/#'",
                "actions": (0..10)
                    .map(|index| json!({
                        "republish": {
                            "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                            "topic": format!("bulk/out/{index}"),
                        },
                    }))
                    .collect::<Vec<Value>>(),
            }),
            tags: None,
            rule_disabled: false,
            sql_version: DEFAULT_SQL_VERSION,
        },
        Case {
            label: "a rule created with tags, which the create accepts as a header",
            rule_name: "tagged_rule".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT * FROM 'tagged/#'",
                "actions": [],
            }),
            tags: Some("team=platform&env=test"),
            rule_disabled: false,
            sql_version: DEFAULT_SQL_VERSION,
        },
        Case {
            label: "a name using every character class the pattern allows",
            rule_name: "Rule_01_aZ".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT VALUE payload FROM 'raw'",
                "actions": [],
            }),
            tags: None,
            rule_disabled: false,
            sql_version: DEFAULT_SQL_VERSION,
        },
        Case {
            label: "the shortest legal rule name, signed for another region",
            rule_name: "a".to_string(),
            region: "eu-west-1",
            payload: json!({
                "sql": "SELECT * FROM 'eu/#'",
                "actions": [{
                    "lambda": { "functionArn": "arn:aws:lambda:eu-west-1:000000000000:function:eu" },
                }],
            }),
            tags: None,
            rule_disabled: false,
            sql_version: DEFAULT_SQL_VERSION,
        },
        Case {
            label: "the longest legal rule name",
            rule_name: "r".repeat(128),
            region: REGION,
            payload: json!({
                "sql": "SELECT * FROM 'long/#'",
                "actions": [],
            }),
            tags: None,
            rule_disabled: false,
            sql_version: DEFAULT_SQL_VERSION,
        },
    ]
}

#[tokio::test]
async fn a_created_rule_round_trips_through_get_topic_rule() {
    let emulator = Emulator::new();
    let before = now_epoch_seconds();

    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_rule_in(&case.rule_name, &case.payload, case.region, case.tags)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateTopicRule must succeed, got {created:?}"
        );
        // `CreateTopicRule`'s output shape is `Unit`: a successful create reports no
        // members at all, and what it persisted is read back through `GetTopicRule`.
        assert_eq!(
            created.body,
            Value::Null,
            "case {label:?}: CreateTopicRule models no output members, got {created:?}"
        );

        let read = emulator.get_rule(&case.rule_name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetTopicRule must find the created rule, got {read:?}"
        );
        assert_eq!(
            read.text("ruleArn"),
            Some(expected_arn(case.region, &case.rule_name)),
            "case {label:?}: GetTopicRule must report the rule's ARN, got {read:?}"
        );
        assert_eq!(
            read.rule_member("ruleName").and_then(Value::as_str),
            Some(case.rule_name.as_str()),
            "case {label:?}: the rule must report the name it was created under, got {read:?}"
        );
        assert_eq!(
            read.rule_member("sql"),
            case.payload.get("sql"),
            "case {label:?}: the rule must report the persisted SQL, got {read:?}"
        );
        assert_eq!(
            read.rule_member("description"),
            case.payload.get("description"),
            "case {label:?}: the rule must report exactly the persisted description, got {read:?}"
        );
        assert_eq!(
            read.rule_member("actions"),
            case.payload.get("actions"),
            "case {label:?}: the rule must report the persisted actions, in order, got {read:?}"
        );
        assert_eq!(
            read.rule_member("errorAction"),
            case.payload.get("errorAction"),
            "case {label:?}: the rule must report exactly the persisted errorAction, got {read:?}"
        );
        assert_eq!(
            read.rule_member("ruleDisabled").and_then(Value::as_bool),
            Some(case.rule_disabled),
            "case {label:?}: expected ruleDisabled {}, got {read:?}",
            case.rule_disabled
        );
        assert_eq!(
            read.rule_member("awsIotSqlVersion").and_then(Value::as_str),
            Some(case.sql_version),
            "case {label:?}: expected awsIotSqlVersion {:?}, got {read:?}",
            case.sql_version
        );

        // `createdAt` is the instant of the create: it lies between the instant
        // before the first create and the instant after this read, so it is a real
        // clock reading rather than a fixed or zero timestamp.
        let created_at = read
            .created_at()
            .unwrap_or_else(|| panic!("case {label:?}: the rule reported no createdAt: {read:?}"));
        let after = now_epoch_seconds();
        assert!(
            (before - 1.0..=after + 1.0).contains(&created_at),
            "case {label:?}: createdAt {created_at} is outside [{before}, {after}], got {read:?}"
        );
    }
}

#[tokio::test]
async fn reading_a_rule_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    // `GetTopicRule` reports the rule; it must not re-time, re-mint or otherwise
    // rewrite it, so two reads of one rule are byte-identical.
    for case in cases() {
        let label = case.label;
        let created = emulator
            .create_rule_in(&case.rule_name, &case.payload, case.region, case.tags)
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateTopicRule must succeed, got {created:?}"
        );

        let first = emulator.get_rule(&case.rule_name).await;
        let second = emulator.get_rule(&case.rule_name).await;
        assert_eq!(
            first.status, second.status,
            "case {label:?}: both reads must answer the same status, got {second:?}"
        );
        assert_eq!(
            first.body, second.body,
            "case {label:?}: a read must not change the rule, got {first:?} then {second:?}"
        );
    }
}

#[tokio::test]
async fn a_repeat_create_of_one_name_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    // The rule's name is its identity, so a second create under a name the store
    // holds is refused 409 `ResourceAlreadyExistsException` whatever the second
    // payload says — and the first rule is left exactly as it was.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the same payload again",
            "repeat_same",
            json!({ "sql": "SELECT * FROM 'a'", "actions": [] }),
        ),
        (
            "a different SQL statement",
            "repeat_sql",
            json!({ "sql": "SELECT * FROM 'b'", "actions": [] }),
        ),
        (
            "a different action list",
            "repeat_actions",
            json!({
                "sql": "SELECT * FROM 'a'",
                "actions": [{
                    "lambda": { "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:other" },
                }],
            }),
        ),
        (
            "a payload that disables the rule",
            "repeat_disabled",
            json!({ "sql": "SELECT * FROM 'a'", "actions": [], "ruleDisabled": true }),
        ),
    ];

    for (label, rule_name, second_payload) in rows {
        let first_payload = json!({ "sql": "SELECT * FROM 'a'", "actions": [] });
        let first = emulator.create_rule(rule_name, &first_payload).await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: the first CreateTopicRule must succeed, got {first:?}"
        );
        let before = emulator.get_rule(rule_name).await;

        let second = emulator.create_rule(rule_name, &second_payload).await;
        assert_eq!(
            second.status, 409,
            "case {label:?}: a repeat create must be refused 409, got {second:?}"
        );
        assert_eq!(
            second.error_code.as_deref(),
            Some("ResourceAlreadyExistsException"),
            "case {label:?}: expected ResourceAlreadyExistsException, got {second:?}"
        );
        // The refusal is built from the request: it names the rule that exists and
        // the ARN it was minted with.
        assert_eq!(
            second.text("resourceId").as_deref(),
            Some(rule_name),
            "case {label:?}: the refusal must name the rule that exists, got {second:?}"
        );
        assert_eq!(
            second.text("resourceArn"),
            Some(expected_arn(REGION, rule_name)),
            "case {label:?}: the refusal must name the existing rule's ARN, got {second:?}"
        );

        let after = emulator.get_rule(rule_name).await;
        assert_eq!(
            before.body, after.body,
            "case {label:?}: the refused create must leave the rule untouched, got {after:?}"
        );
    }
}

#[tokio::test]
async fn the_rules_store_is_bounded() {
    let emulator = Emulator::new();
    let payload = json!({ "sql": "SELECT * FROM 'bounded/#'", "actions": [] });

    // The store is one a caller grows, so it has a ceiling: creates up to
    // `MAX_TOPIC_RULES` succeed, and every create past it is refused with the
    // modelled `InvalidRequestException` (400) while the rules already recorded stay
    // readable.
    for index in 0..MAX_TOPIC_RULES {
        let created = emulator
            .create_rule(&format!("bounded_rule_{index}"), &payload)
            .await;
        assert_eq!(
            created.status, 200,
            "case {index}: a create below the ceiling must succeed, got {created:?}"
        );
    }

    for name in ["one_too_many", "another_too_many"] {
        let refused = emulator.create_rule(name, &payload).await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: a create past the ceiling must be refused 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {name:?}: expected InvalidRequestException, got {refused:?}"
        );
        // The refused create recorded nothing under that name.
        let read = emulator.get_rule(name).await;
        assert_eq!(
            read.status, 401,
            "case {name:?}: the refused create must record no rule, got {read:?}"
        );
    }

    // The rules that were recorded are untouched by the refusals.
    for index in [0, MAX_TOPIC_RULES / 2, MAX_TOPIC_RULES - 1] {
        let read = emulator.get_rule(&format!("bounded_rule_{index}")).await;
        assert_eq!(
            read.status, 200,
            "case {index}: a recorded rule must still be readable, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_rule_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // One rule exists, so the refusals below are about the NAME not being in the
    // store rather than the store being empty.
    let created = emulator
        .create_rule(
            "existing_rule",
            &json!({ "sql": "SELECT * FROM 'a'", "actions": [] }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateTopicRule must succeed, got {created:?}"
    );

    // `GetTopicRule` models no not-found error, so an unknown rule name is refused
    // with the modelled `UnauthorizedException` (HTTP 401) — never a 200 carrying no
    // rule, which would claim the rule exists and is empty.
    for rule_name in [
        "a",
        "unknown_rule",
        "existing_rule_2",
        "EXISTING_RULE",
        &"r".repeat(128),
    ] {
        let read = emulator.get_rule(rule_name).await;
        assert_eq!(
            read.status, 401,
            "case {rule_name:?}: an unknown rule name must be refused 401, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("UnauthorizedException"),
            "case {rule_name:?}: expected UnauthorizedException, got {read:?}"
        );
        assert!(
            read.rule().is_none(),
            "case {rule_name:?}: a refusal carries no rule, got {read:?}"
        );
    }

    // The rule that does exist is still readable: the refusals above wrote nothing.
    let read = emulator.get_rule("existing_rule").await;
    assert_eq!(
        read.status, 200,
        "the existing rule must still be readable, got {read:?}"
    );
}
