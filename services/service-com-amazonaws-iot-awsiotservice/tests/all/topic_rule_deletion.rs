// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteTopicRule`: the writer that takes a rule back out
//! of the AWS IoT rules store `CreateTopicRule` grows.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: `DeleteTopicRule` REMOVES the record the store holds under the
//! requested name, so every later reader of the store stops reporting it — `GetTopicRule`
//! of that name refuses, `ListTopicRules` no longer carries it, and a later
//! `CreateTopicRule` may take the name again with a different payload — while every other
//! rule in the store is left exactly as it was. A name the store does not hold is not
//! refused: the operation models no not-found error
//! (`smithy_query(query="operation",operation="DeleteTopicRule")` lists only
//! `ConflictingResourceUpdateException`, `InternalException`, `InvalidRequestException`,
//! `ServiceUnavailableException` and `UnauthorizedException`) and the state the call asks
//! for already holds, so a repeat delete answers exactly as the first one did and writes
//! nothing.
//!
//! The table spans what that rule generalises over — the form and length of a rule name,
//! how many actions the deleted rule carried and of which types, whether it was enabled
//! or disabled, whether it carried an `errorAction`, and whether its SQL names a topic
//! filter at all — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and [`REGION`]: the scope is how the router picks the claiming service. The signature
/// itself is never checked here — the harness turns request authentication off, exactly
/// as an unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
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

    /// The `rules` entries of a `ListTopicRules` response, in the order the page carries
    /// them.
    fn listed(&self) -> Vec<Value> {
        self.body
            .get("rules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The `ruleName` of every entry of a `ListTopicRules` page.
    fn listed_names(&self) -> Vec<String> {
        self.listed()
            .iter()
            .filter_map(|item| item.get("ruleName").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
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

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
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

    async fn create_rule(&self, rule_name: &str, payload: &Value) -> Reply {
        self.call("POST", &format!("/rules/{rule_name}"), Some(payload))
            .await
    }

    async fn get_rule(&self, rule_name: &str) -> Reply {
        self.call("GET", &format!("/rules/{rule_name}"), None).await
    }

    async fn delete_rule(&self, rule_name: &str) -> Reply {
        self.call("DELETE", &format!("/rules/{rule_name}"), None)
            .await
    }

    async fn list_rules(&self) -> Reply {
        self.call("GET", "/rules", None).await
    }
}

/// One rule the store is grown with before it is deleted again: a label, the name it is
/// created under, and the `TopicRulePayload` it carries.
struct Case {
    label: &'static str,
    rule_name: String,
    payload: Value,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over:
///
/// * the form and length of `ruleName` — `RuleName` is modelled
///   `length 1..128, pattern ^[a-zA-Z0-9_]+$`
///   (`smithy_query(query="operation",operation="DeleteTopicRule")`), so the rows include
///   the shortest legal name, the longest, and one using every character class the
///   pattern allows;
/// * how many actions the deleted rule carried and of which type — `ActionList` is
///   modelled `length 0..10` and `Action`'s members are one per destination service
///   (`smithy_query(query="shape",shape="TopicRulePayload")`), so the rows span the empty
///   list the minimum admits, one action, three of three types, and the ten the maximum
///   admits;
/// * whether the rule was enabled or disabled, and whether it carried an `errorAction`,
///   because neither changes what a delete does;
/// * whether the rule's SQL names a topic filter at all, which is what decides whether a
///   listing entry for it carries a `topicPattern`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a rule with the two required members only",
            rule_name: "minimal_rule".to_string(),
            payload: json!({
                "sql": "SELECT * FROM 'sensors/+/telemetry'",
                "actions": [],
            }),
        },
        Case {
            label: "an enabled rule with one lambda action and a description",
            rule_name: "lambda_rule".to_string(),
            payload: json!({
                "sql": "SELECT temperature FROM 'sensors/#' WHERE temperature > 30",
                "description": "forward hot readings",
                "ruleDisabled": false,
                "actions": [{
                    "lambda": { "functionArn": "arn:aws:lambda:us-east-1:000000000000:function:on-hot" },
                }],
            }),
        },
        Case {
            label: "a disabled rule with three action types and an errorAction",
            rule_name: "disabled_fanout_rule".to_string(),
            payload: json!({
                "sql": "SELECT * FROM 'devices/+/events'",
                "ruleDisabled": true,
                "awsIotSqlVersion": "2016-03-23",
                "actions": [
                    {
                        "republish": {
                            "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                            "topic": "alerts/events",
                            "qos": 1,
                        },
                    },
                    {
                        "sqs": {
                            "roleArn": "arn:aws:iam::000000000000:role/iot-sqs",
                            "queueUrl": "https://sqs.us-east-1.amazonaws.com/000000000000/events",
                            "useBase64": false,
                        },
                    },
                    {
                        "kinesis": {
                            "roleArn": "arn:aws:iam::000000000000:role/iot-kinesis",
                            "streamName": "events",
                            "partitionKey": "${topic()}",
                        },
                    },
                ],
                "errorAction": {
                    "republish": {
                        "roleArn": "arn:aws:iam::000000000000:role/iot-republish",
                        "topic": "errors",
                    },
                },
            }),
        },
        Case {
            label: "the ten actions the ActionList maximum admits",
            rule_name: "ten_action_rule".to_string(),
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
        },
        Case {
            label: "a statement that names no topic filter",
            rule_name: "No_Topic_01".to_string(),
            payload: json!({
                "sql": "SELECT 1",
                "actions": [],
            }),
        },
        Case {
            label: "the shortest legal rule name",
            rule_name: "a".to_string(),
            payload: json!({
                "sql": "SELECT * FROM 'short'",
                "actions": [],
            }),
        },
        Case {
            label: "the longest legal rule name",
            rule_name: "r".repeat(128),
            payload: json!({
                "sql": "SELECT * FROM 'long/#'",
                "actions": [],
            }),
        },
    ]
}

/// Create every case's rule, and fail loudly when the store cannot be set up: a delete
/// test says nothing unless the rule it deletes was really there.
async fn create_every_case(emulator: &Emulator) {
    for case in cases() {
        let created = emulator.create_rule(&case.rule_name, &case.payload).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the fixture CreateTopicRule must succeed, got {created:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn deleting_a_rule_removes_it_from_every_reader_of_the_store() {
    let emulator = Emulator::new();
    create_every_case(&emulator).await;

    // Every rule is in the store before the first delete, so what each delete below
    // changes is the rule it names and nothing else.
    let mut remaining: Vec<Value> = emulator.list_rules().await.listed();
    for case in cases() {
        assert!(
            remaining
                .iter()
                .any(|item| item.get("ruleName").and_then(Value::as_str)
                    == Some(case.rule_name.as_str())),
            "case {:?}: the fixture rule must be listed before it is deleted, got {remaining:?}",
            case.label
        );
    }

    for case in cases() {
        let label = case.label;
        let deleted = emulator.delete_rule(&case.rule_name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteTopicRule must succeed, got {deleted:?}"
        );
        // `DeleteTopicRule`'s output shape is `Unit`: a successful delete reports no
        // members at all, and that the rule is gone is read back through the readers.
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {label:?}: DeleteTopicRule models no output members, got {deleted:?}"
        );

        // `GetTopicRule` models no not-found error, so a name the store no longer holds
        // is refused with the modelled `UnauthorizedException` (HTTP 401) — never a 200
        // carrying the rule the delete was supposed to remove.
        let read = emulator.get_rule(&case.rule_name).await;
        assert_eq!(
            read.status, 401,
            "case {label:?}: the deleted rule must no longer be readable, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("UnauthorizedException"),
            "case {label:?}: expected UnauthorizedException, got {read:?}"
        );
        assert!(
            read.rule().is_none(),
            "case {label:?}: a refusal carries no rule, got {read:?}"
        );

        // The listing loses exactly the deleted rule: every other entry is byte-identical
        // to the entry it was before this delete, so no other rule was rewritten,
        // re-timed or dropped.
        remaining.retain(|item| {
            item.get("ruleName").and_then(Value::as_str) != Some(case.rule_name.as_str())
        });
        let listed = emulator.list_rules().await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: ListTopicRules must succeed, got {listed:?}"
        );
        assert_eq!(
            listed.listed(),
            remaining,
            "case {label:?}: expected the listing to lose only the deleted rule, got {listed:?}"
        );

        // Every rule that is still in the store is still readable, with the payload it
        // was created with.
        for other in cases().iter().filter(|other| {
            remaining
                .iter()
                .any(|item| item.get("ruleName").and_then(Value::as_str) == Some(&other.rule_name))
        }) {
            let read = emulator.get_rule(&other.rule_name).await;
            assert_eq!(
                read.status,
                200,
                "case {label:?}: deleting it must leave {:?} readable, got {read:?}",
                other.label
            );
            assert_eq!(
                read.rule_member("sql"),
                other.payload.get("sql"),
                "case {label:?}: {:?} must still report its own SQL, got {read:?}",
                other.label
            );
            assert_eq!(
                read.rule_member("actions"),
                other.payload.get("actions"),
                "case {label:?}: {:?} must still report its own actions, got {read:?}",
                other.label
            );
        }
    }

    // The store is empty once every rule has been deleted: `TopicRuleList` carries no
    // length constraint, so "no rules" is an EMPTY list.
    let listed = emulator.list_rules().await;
    assert_eq!(
        listed.listed_names(),
        Vec::<String>::new(),
        "every rule was deleted, so the listing must be empty, got {listed:?}"
    );
}

#[tokio::test]
async fn deleting_a_name_the_store_does_not_hold_succeeds_and_changes_nothing() {
    let emulator = Emulator::new();
    create_every_case(&emulator).await;

    // One rule is deleted first, so the repeat below is a delete of a name that WAS in
    // the store, next to names that never were.
    let first = emulator.delete_rule("minimal_rule").await;
    assert_eq!(
        first.status, 200,
        "the first DeleteTopicRule must succeed, got {first:?}"
    );

    let before = emulator.list_rules().await.listed();

    // `DeleteTopicRule` models no not-found error and the state it asks for already
    // holds, so each of these answers exactly as a delete of an existing rule does, and
    // writes nothing: a repeat of a delete already performed, a name nobody ever created,
    // one differing from a stored name only in case, and the shortest and longest names
    // the pattern admits.
    let unknown_names: Vec<String> = vec![
        "minimal_rule".to_string(),
        "never_created".to_string(),
        "MINIMAL_RULE".to_string(),
        "lambda_rule_2".to_string(),
        "z".to_string(),
        "q".repeat(128),
    ];
    for name in &unknown_names {
        let deleted = emulator.delete_rule(name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: deleting a name the store does not hold must succeed, got {deleted:?}"
        );
        assert_eq!(
            deleted.body,
            Value::Null,
            "case {name:?}: DeleteTopicRule models no output members, got {deleted:?}"
        );
        assert_eq!(
            deleted.error_code, None,
            "case {name:?}: the delete must report no error, got {deleted:?}"
        );

        let listed = emulator.list_rules().await;
        assert_eq!(
            listed.listed(),
            before,
            "case {name:?}: the delete must leave every rule in the store untouched, got {listed:?}"
        );
    }

    // Every rule the store still holds reports the payload it was created with, member
    // for member.
    for case in cases().iter().filter(|case| case.rule_name != "minimal_rule") {
        let read = emulator.get_rule(&case.rule_name).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: the rule must still be readable, got {read:?}",
            case.label
        );
        assert_eq!(
            read.rule_member("sql"),
            case.payload.get("sql"),
            "case {:?}: the rule must still report its SQL, got {read:?}",
            case.label
        );
        assert_eq!(
            read.rule_member("errorAction"),
            case.payload.get("errorAction"),
            "case {:?}: the rule must still report its errorAction, got {read:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_deleted_name_is_free_for_a_create_that_records_a_different_rule() {
    let emulator = Emulator::new();

    // A delete REMOVES the record rather than hiding it, so the name it freed can be
    // created again — and the rule the second create records is the one a later read
    // reports, not the one the deleted rule carried.
    for case in cases() {
        let label = case.label;
        let created = emulator.create_rule(&case.rule_name, &case.payload).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the first CreateTopicRule must succeed, got {created:?}"
        );

        let deleted = emulator.delete_rule(&case.rule_name).await;
        assert_eq!(
            deleted.status, 200,
            "case {label:?}: DeleteTopicRule must succeed, got {deleted:?}"
        );

        // A payload that differs from the deleted rule's in every member a reader
        // reports, so what the read below reports can only have come from this create.
        let replacement = json!({
            "sql": "SELECT * FROM 'recreated/+/topic'",
            "description": "created again under a freed name",
            "ruleDisabled": true,
            "awsIotSqlVersion": "beta",
            "actions": [{
                "sns": {
                    "targetArn": "arn:aws:sns:us-east-1:000000000000:recreated",
                    "roleArn": "arn:aws:iam::000000000000:role/iot-sns",
                    "messageFormat": "JSON",
                },
            }],
        });
        let recreated = emulator.create_rule(&case.rule_name, &replacement).await;
        assert_eq!(
            recreated.status, 200,
            "case {label:?}: a create under the freed name must succeed, got {recreated:?}"
        );

        let read = emulator.get_rule(&case.rule_name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the re-created rule must be readable, got {read:?}"
        );
        assert_eq!(
            read.rule_member("sql"),
            replacement.get("sql"),
            "case {label:?}: the rule must report the SQL of the second create, got {read:?}"
        );
        assert_eq!(
            read.rule_member("description"),
            replacement.get("description"),
            "case {label:?}: the rule must report the description of the second create, \
             got {read:?}"
        );
        assert_eq!(
            read.rule_member("actions"),
            replacement.get("actions"),
            "case {label:?}: the rule must report the actions of the second create, got {read:?}"
        );
        assert_eq!(
            read.rule_member("ruleDisabled").and_then(Value::as_bool),
            Some(true),
            "case {label:?}: the rule must report the ruleDisabled of the second create, \
             got {read:?}"
        );
        assert_eq!(
            read.rule_member("awsIotSqlVersion").and_then(Value::as_str),
            Some("beta"),
            "case {label:?}: the rule must report the SQL version of the second create, \
             got {read:?}"
        );
        // The errorAction the deleted rule may have carried is gone with it: the
        // re-created rule carries none.
        assert!(
            read.rule_member("errorAction").is_none(),
            "case {label:?}: the re-created rule carries no errorAction, got {read:?}"
        );
    }
}
