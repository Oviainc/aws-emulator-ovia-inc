// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT rules store's ENABLEMENT pair:
//! `EnableTopicRule` and `DisableTopicRule`, proved through the `GetTopicRule`
//! reader.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what
//! an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: enabling or disabling a topic rule is a ONE-MEMBER update of
//! the record the store already holds — it sets `ruleDisabled` to `false`
//! (`POST /rules/{ruleName}/enable`) or `true` (`POST /rules/{ruleName}/disable`) and
//! leaves every other member of the rule exactly as it was — so it is idempotent, the
//! two calls in either order are each other's inverse, and a rule name the store does
//! not hold is refused with the operation's modelled `UnauthorizedException` (401,
//! `smithy_query(query="operation",operation="EnableTopicRule,DisableTopicRule")`
//! lists no not-found error) while writing nothing. Both operations model `Unit`
//! output, so a success is an empty 200.
//!
//! The table spans what that rule generalises over — the rule's starting
//! `ruleDisabled` value (absent, `false`, `true`), the form and length of the rule
//! name, how many actions of which types the rule carries, which optional payload
//! members it carries, the region the call is signed for, and the SEQUENCE of
//! enable/disable calls applied (a single call, a repeat of one call, and alternating
//! runs) — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing
/// name and `region`: the scope is how the router picks the claiming service, and how
/// the service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
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
    /// The `rule` member of a `GetTopicRule` response.
    fn rule(&self) -> Option<&Value> {
        self.body.get("rule")
    }

    /// A member of the reported `TopicRule`, or `None` when the rule or the member is
    /// absent.
    fn rule_member(&self, name: &str) -> Option<&Value> {
        self.rule().and_then(|rule| rule.get(name))
    }

    /// What the read rule reports for `ruleDisabled`.
    fn rule_disabled(&self) -> Option<bool> {
        self.rule_member("ruleDisabled").and_then(Value::as_bool)
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
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

    async fn create_rule_in(&self, rule_name: &str, payload: &Value, region: &str) -> Reply {
        self.call("POST", &format!("/rules/{rule_name}"), Some(payload), region)
            .await
    }

    async fn enable_rule_in(&self, rule_name: &str, region: &str) -> Reply {
        self.call("POST", &format!("/rules/{rule_name}/enable"), None, region)
            .await
    }

    async fn disable_rule_in(&self, rule_name: &str, region: &str) -> Reply {
        self.call("POST", &format!("/rules/{rule_name}/disable"), None, region)
            .await
    }

    async fn get_rule_in(&self, rule_name: &str, region: &str) -> Reply {
        self.call("GET", &format!("/rules/{rule_name}"), None, region)
            .await
    }
}

/// Which of the pair a step applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Enable,
    Disable,
}

impl Step {
    /// What `ruleDisabled` must report once this step has been applied.
    fn disabled_after(self) -> bool {
        self == Step::Disable
    }
}

/// One rule the table creates, and the sequence of enable/disable calls applied to
/// it.
struct Case {
    label: &'static str,
    rule_name: String,
    region: &'static str,
    /// The `TopicRulePayload` the rule is created with.
    payload: Value,
    /// What `ruleDisabled` reports right after the create: the payload's value, or
    /// `false` when the payload carried none.
    created_disabled: bool,
    /// The calls applied in order; `ruleDisabled` is asserted after each one.
    steps: Vec<Step>,
}

/// The table every case below is built from. It spans what the rule generalises over:
///
/// * the rule's starting `ruleDisabled` — absent (an enabled rule), explicit `false`,
///   explicit `true` (`smithy_query(query="shape",shape="TopicRulePayload")` models
///   the member optional);
/// * the sequence applied — a single enable, a single disable, the same call twice
///   (idempotence), and alternating runs in both orders (each call is the other's
///   inverse);
/// * the form and length of `ruleName` — `RuleName` is modelled
///   `length 1..128, pattern ^[a-zA-Z0-9_]+$`
///   (`smithy_query(query="operation",operation="EnableTopicRule")`), so the rows
///   include the shortest legal name, the longest, and one using every character
///   class the pattern allows;
/// * what else the rule carries — zero actions, one action, several actions of
///   different `Action` member types, a `description`, a non-default
///   `awsIotSqlVersion` and an `errorAction` — all of which the pair must leave
///   untouched;
/// * the region the call is signed for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a rule created without ruleDisabled, then enabled",
            rule_name: "enable_default_rule".to_string(),
            region: REGION,
            payload: json!({ "sql": "SELECT * FROM 'sensors/+/telemetry'", "actions": [] }),
            created_disabled: false,
            steps: vec![Step::Enable],
        },
        Case {
            label: "a rule created disabled, then enabled",
            rule_name: "disabled_then_enabled".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT temperature FROM 'sensors/#' WHERE temperature > 30",
                "description": "fan out hot readings",
                "ruleDisabled": true,
                "actions": [{
                    "republish": {
                        "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                        "topic": "alerts/hot",
                        "qos": 1,
                    },
                }],
            }),
            created_disabled: true,
            steps: vec![Step::Enable],
        },
        Case {
            label: "a rule created explicitly enabled, then disabled",
            rule_name: "enabled_then_disabled".to_string(),
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
            created_disabled: false,
            steps: vec![Step::Disable],
        },
        Case {
            label: "enabling an already enabled rule twice is idempotent",
            rule_name: "idempotent_enable".to_string(),
            region: REGION,
            payload: json!({ "sql": "SELECT * FROM 'idem/enable'", "actions": [] }),
            created_disabled: false,
            steps: vec![Step::Enable, Step::Enable, Step::Enable],
        },
        Case {
            label: "disabling an already disabled rule twice is idempotent",
            rule_name: "idempotent_disable".to_string(),
            region: REGION,
            payload: json!({ "sql": "SELECT * FROM 'idem/disable'", "actions": [], "ruleDisabled": true }),
            created_disabled: true,
            steps: vec![Step::Disable, Step::Disable, Step::Disable],
        },
        Case {
            label: "alternating disable/enable runs on a rule with an errorAction",
            rule_name: "Rule_01_aZ".to_string(),
            region: REGION,
            payload: json!({
                "sql": "SELECT * FROM 'devices/+/shadow'",
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
            created_disabled: false,
            steps: vec![
                Step::Disable,
                Step::Enable,
                Step::Disable,
                Step::Disable,
                Step::Enable,
            ],
        },
        Case {
            label: "alternating enable/disable runs, shortest legal name, another region",
            rule_name: "a".to_string(),
            region: "eu-west-1",
            payload: json!({
                "sql": "SELECT * FROM 'eu/#'",
                "ruleDisabled": true,
                "actions": [{
                    "lambda": { "functionArn": "arn:aws:lambda:eu-west-1:000000000000:function:eu" },
                }],
            }),
            created_disabled: true,
            steps: vec![Step::Enable, Step::Enable, Step::Disable, Step::Enable],
        },
        Case {
            label: "the longest legal rule name",
            rule_name: "r".repeat(128),
            region: REGION,
            payload: json!({ "sql": "SELECT * FROM 'long/#'", "actions": [] }),
            created_disabled: false,
            steps: vec![Step::Disable, Step::Enable],
        },
    ]
}

/// The ARN a rule created in `region` must carry.
fn expected_arn(region: &str, rule_name: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:rule/{rule_name}")
}

#[tokio::test]
async fn enable_and_disable_flip_only_rule_disabled_on_the_stored_rule() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let name = case.rule_name.as_str();
        let region = case.region;

        let created = emulator.create_rule_in(name, &case.payload, region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateTopicRule must succeed, got {created:?}"
        );

        // The rule as the store holds it right after the create: every later
        // assertion compares against this, with `ruleDisabled` substituted.
        let after_create = emulator.get_rule_in(name, region).await;
        assert_eq!(
            after_create.status, 200,
            "case {label:?}: GetTopicRule must find the created rule, got {after_create:?}"
        );
        assert_eq!(
            after_create.rule_disabled(),
            Some(case.created_disabled),
            "case {label:?}: expected a created ruleDisabled of {}, got {after_create:?}",
            case.created_disabled
        );
        let baseline = after_create
            .rule()
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: the read reported no rule: {after_create:?}"));

        for (index, step) in case.steps.iter().copied().enumerate() {
            let applied = match step {
                Step::Enable => emulator.enable_rule_in(name, region).await,
                Step::Disable => emulator.disable_rule_in(name, region).await,
            };
            assert_eq!(
                applied.status, 200,
                "case {label:?} step {index} ({step:?}): must answer 200, got {applied:?}"
            );
            // Both operations model `Unit` output: a success carries no members.
            assert_eq!(
                applied.body,
                Value::Null,
                "case {label:?} step {index} ({step:?}): the output shape is Unit, got {applied:?}"
            );

            let read = emulator.get_rule_in(name, region).await;
            assert_eq!(
                read.status, 200,
                "case {label:?} step {index} ({step:?}): the rule must still be readable, got {read:?}"
            );
            assert_eq!(
                read.rule_disabled(),
                Some(step.disabled_after()),
                "case {label:?} step {index} ({step:?}): expected ruleDisabled {}, got {read:?}",
                step.disabled_after()
            );
            assert_eq!(
                read.body.get("ruleArn").and_then(Value::as_str),
                Some(expected_arn(region, name).as_str()),
                "case {label:?} step {index} ({step:?}): the rule's ARN must not change, got {read:?}"
            );

            // Only `ruleDisabled` moved: the whole rule equals the created one with
            // that single member substituted — same SQL, actions, description, SQL
            // version, errorAction and createdAt.
            let mut expected = baseline.clone();
            expected["ruleDisabled"] = Value::Bool(step.disabled_after());
            assert_eq!(
                read.rule(),
                Some(&expected),
                "case {label:?} step {index} ({step:?}): expected only ruleDisabled to change \
                 (wanted {expected}), got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn enabling_or_disabling_one_rule_leaves_every_other_rule_alone() {
    let emulator = Emulator::new();
    let payload = json!({ "sql": "SELECT * FROM 'neighbours/#'", "actions": [] });

    // Three rules in distinct starting states, so a write to one cannot be mistaken
    // for a write to the store as a whole.
    for (name, disabled) in [
        ("neighbour_enabled", false),
        ("neighbour_disabled", true),
        ("neighbour_target", false),
    ] {
        let mut body = payload.clone();
        body["ruleDisabled"] = Value::Bool(disabled);
        let created = emulator.create_rule_in(name, &body, REGION).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreateTopicRule must succeed, got {created:?}"
        );
    }

    let before_enabled = emulator.get_rule_in("neighbour_enabled", REGION).await;
    let before_disabled = emulator.get_rule_in("neighbour_disabled", REGION).await;

    for step in [Step::Disable, Step::Enable] {
        let applied = match step {
            Step::Enable => emulator.enable_rule_in("neighbour_target", REGION).await,
            Step::Disable => emulator.disable_rule_in("neighbour_target", REGION).await,
        };
        assert_eq!(
            applied.status, 200,
            "case {step:?}: the call must succeed, got {applied:?}"
        );

        let after_enabled = emulator.get_rule_in("neighbour_enabled", REGION).await;
        let after_disabled = emulator.get_rule_in("neighbour_disabled", REGION).await;
        assert_eq!(
            before_enabled.body, after_enabled.body,
            "case {step:?}: an untargeted enabled rule must be untouched, got {after_enabled:?}"
        );
        assert_eq!(
            before_disabled.body, after_disabled.body,
            "case {step:?}: an untargeted disabled rule must be untouched, got {after_disabled:?}"
        );
    }
}

#[tokio::test]
async fn a_rule_the_store_does_not_hold_cannot_be_enabled_or_disabled() {
    let emulator = Emulator::new();

    // One rule exists, so the refusals below are about the NAME not being in the
    // store rather than the store being empty.
    let created = emulator
        .create_rule_in(
            "existing_rule",
            &json!({ "sql": "SELECT * FROM 'a'", "actions": [] }),
            REGION,
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the setup CreateTopicRule must succeed, got {created:?}"
    );
    let before = emulator.get_rule_in("existing_rule", REGION).await;

    // Neither operation models a not-found error, so an unknown rule name is refused
    // with the modelled `UnauthorizedException` (401) — never an empty 200, which
    // would claim a rule was flipped when none exists.
    for rule_name in [
        "a",
        "unknown_rule",
        "existing_rule_2",
        "EXISTING_RULE",
        &"r".repeat(128),
    ] {
        for step in [Step::Enable, Step::Disable] {
            let refused = match step {
                Step::Enable => emulator.enable_rule_in(rule_name, REGION).await,
                Step::Disable => emulator.disable_rule_in(rule_name, REGION).await,
            };
            assert_eq!(
                refused.status, 401,
                "case {rule_name:?} ({step:?}): an unknown rule must be refused 401, got {refused:?}"
            );
            assert_eq!(
                refused.error_code.as_deref(),
                Some("UnauthorizedException"),
                "case {rule_name:?} ({step:?}): expected UnauthorizedException, got {refused:?}"
            );

            // The refusal recorded nothing under that name: the rule is still absent.
            let read = emulator.get_rule_in(rule_name, REGION).await;
            assert_eq!(
                read.status, 401,
                "case {rule_name:?} ({step:?}): the refusal must record no rule, got {read:?}"
            );
        }
    }

    // The rule that does exist is untouched by the refusals.
    let after = emulator.get_rule_in("existing_rule", REGION).await;
    assert_eq!(
        before.body, after.body,
        "the refusals must leave the existing rule alone, got {after:?}"
    );
}
