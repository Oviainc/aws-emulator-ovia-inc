// SPDX-License-Identifier: CC0-1.0

//! The AWS IoT RULES ENGINE, end to end over the wire through the real
//! `MultiServiceRouter`, with the IoT data plane, Amazon Kinesis and IAM mounted on the
//! same engine — so a rule's effect is observed where a client observes it: as records a
//! stream's own `GetShardIterator` + `GetRecords` serve.
//!
//! The RULE asserted here: a message published to a topic that an ENABLED topic rule's
//! `FROM`-clause topic filter covers has each of that rule's actions performed for the
//! message its statement selects — for a `kinesis` action, one `kinesis:PutRecord` of the
//! selected message onto the stream the action names, under the partition key it names —
//! and the action is performed only where its `roleArn` both admits the IoT service
//! principal and carries a grant for that call on that stream, with NOTHING written
//! otherwise. The publisher is answered 200 either way, which is why every assertion below
//! is made on the stream.
//!
//! The tables span the class rather than one recorded case:
//!
//! * the first varies what decides WHETHER and WHAT is delivered — the form of the topic
//!   filter (exact, single-level `+`, multi-level `#`, one that does not cover the topic),
//!   the rule's enablement, a `WHERE` condition that holds and one that does not, a
//!   projection naming attributes instead of `*`, a non-JSON payload, two actions on one
//!   rule, and a `partitionKey` the action does not name;
//! * the second varies what the action's ROLE says about the write — the grant, another
//!   action, another stream's ARN, a wildcard grant, no inline policy at all, a trust policy
//!   naming another service principal, and a permissions boundary that covers the write and
//!   one that caps it away;
//! * the third is the permission boundary as a DIFFERENTIAL, the shape a client verifies it
//!   with: two sibling rules on one stream, one permitted and one not, with the permitted
//!   leg's message delivered and the unpermitted leg's never — over several message pairs,
//!   so neither leg can be right by accident.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    rules_engine::KINESIS_PUT_RECORD_ACTION, SERVICE_PRINCIPAL,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    ACTIVATION_DELAY, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call below is signed for.
const REGION: &str = "us-east-1";

/// The stream every rule of these cases writes to, and the role name of the role that is
/// allowed to write it.
const STREAM: &str = "telemetry";
const WRITER_ROLE: &str = "rule-stream-writer";

/// One shard, because what these cases assert is WHICH records a stream holds and under
/// which partition key — the shard a key maps to is Kinesis's own rule, asserted by that
/// service's tests.
const SHARD_COUNT: i64 = 1;

/// The emulator: the IoT control plane, the IoT data plane, Kinesis and IAM on ONE engine —
/// the shape `core/router/src/lib.rs` mounts them in.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    Emulator { router }
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`: the scope is
/// how the router picks the claiming service. Signature verification is off in this
/// harness, so the value itself is never checked.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One HTTP answer: the status and the body, read as JSON where there is one.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

impl Emulator {
    async fn send(
        &self,
        method: &str,
        uri: &str,
        signing_name: &str,
        content_type: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Answer {
        let mut request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("authorization", authorization(signing_name))
            .header("content-type", content_type);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request"),
            )
            .await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    /// `CreateTopicRule`: the restJson1 `POST /rules/{ruleName}` of the IoT control plane,
    /// whose body IS the `topicRulePayload` (the member is the operation's HTTP payload).
    async fn create_topic_rule(&self, case: &str, rule_name: &str, payload: &Value) {
        let answer = self
            .send(
                "POST",
                &format!("/rules/{rule_name}"),
                "iot",
                "application/json",
                &[],
                serde_json::to_vec(payload).expect("the payload serializes"),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: CreateTopicRule({rule_name}) answered {} {}",
            answer.status,
            answer.text()
        );
    }

    /// `Publish` of the IoT data plane: `POST /topics/{topic}` at QoS 1, the call the
    /// rules engine runs on. An accepted publish is a 200 whatever the rules did with it.
    async fn publish(&self, case: &str, topic: &str, payload: &[u8]) {
        let answer = self
            .send(
                "POST",
                &format!("/topics/{}?qos=1", encode_component(topic)),
                "iotdata",
                "application/octet-stream",
                &[],
                payload.to_vec(),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: Publish({topic}) answered {} {}",
            answer.status,
            answer.text()
        );
    }

    /// One Kinesis call (`Kinesis_20131202`, awsJson1_1), answered with its body.
    async fn kinesis(&self, case: &str, operation: &str, body: &Value) -> Value {
        let answer = self
            .send(
                "POST",
                "/",
                "kinesis",
                "application/x-amz-json-1.1",
                &[("x-amz-target", &format!("Kinesis_20131202.{operation}"))],
                serde_json::to_vec(body).expect("the request serializes"),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: {operation} answered {} {}",
            answer.status,
            answer.text()
        );
        answer.json()
    }

    /// One IAM call over the `awsQuery` protocol, answered with its XML body.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let answer = self
            .send(
                "POST",
                "/",
                "iam",
                "application/x-www-form-urlencoded",
                &[],
                body.into_bytes(),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: {action} answered {} {}",
            answer.status,
            answer.text()
        );
        answer.text()
    }
}

/// Percent-encode `value` for one URI component: a `{topic}` path label carries no `/` of
/// its own, so a topic name's separators travel percent-encoded — which is how an SDK sends
/// a topic name holding slashes.
fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let character = *byte as char;
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | '~') {
            encoded.push(character);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Percent-encode one `awsQuery` form value.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The text of the first `<tag>` of an XML body.
fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// One record a shard served: its partition key and its data as text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    key: String,
    data: String,
}

/// A stream of [`SHARD_COUNT`] shards, past the transition its creation armed; answered with
/// the ARN a role's grant names it by.
async fn given_stream(emulator: &Emulator, case: &str, stream: &str) -> String {
    emulator
        .kinesis(
            case,
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": SHARD_COUNT }),
        )
        .await;
    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;
    let described = emulator
        .kinesis(case, "DescribeStream", &json!({ "StreamName": stream }))
        .await;
    assert_eq!(
        described["StreamDescription"]["StreamStatus"],
        json!("ACTIVE"),
        "case {case:?}: the stream must be active before a rule writes to it, got {described}"
    );
    described["StreamDescription"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: DescribeStream reported no StreamARN"))
        .to_string()
}

/// Everything `stream` holds, read from its shard's oldest retained record.
async fn records(emulator: &Emulator, case: &str, stream: &str) -> Vec<Held> {
    let described = emulator
        .kinesis(case, "DescribeStream", &json!({ "StreamName": stream }))
        .await;
    let shards = described["StreamDescription"]["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut held = Vec::new();
    for shard in shards {
        let shard_id = shard["ShardId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a shard carries no ShardId: {shard}"));
        let iterator = emulator
            .kinesis(
                case,
                "GetShardIterator",
                &json!({
                    "StreamName": stream,
                    "ShardId": shard_id,
                    "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                }),
            )
            .await["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ShardIterator for {shard_id}"))
            .to_string();
        let served = emulator
            .kinesis(
                case,
                "GetRecords",
                &json!({ "ShardIterator": iterator, "Limit": 100 }),
            )
            .await;
        for record in served["Records"].as_array().cloned().unwrap_or_default() {
            let encoded = record["Data"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a record carries no Data: {record}"));
            let data = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                panic!("case {case:?}: a record's Data is not base64 ({error}): {record}")
            });
            held.push(Held {
                key: record["PartitionKey"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a record carries no PartitionKey: {record}")
                    })
                    .to_string(),
                data: String::from_utf8_lossy(&data).to_string(),
            });
        }
    }
    held
}

/// A trust policy admitting one service principal, written as a client sends it.
fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy allowing one action on one resource.
fn identity_policy(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
    .to_string()
}

/// A role trusting `principal`, carrying an inline policy allowing `allowed` on `resource`
/// when the case names one; answered with the ARN IAM persisted.
async fn given_role(
    emulator: &Emulator,
    case: &str,
    name: &str,
    principal: &str,
    allowed: Option<(&str, &str)>,
) -> String {
    let trust = trust_policy(principal);
    let created = emulator
        .iam(
            case,
            "CreateRole",
            &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
        )
        .await;
    let arn = xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole reported no Arn: {created}"));
    if let Some((action, resource)) = allowed {
        let document = identity_policy(action, resource);
        emulator
            .iam(
                case,
                "PutRolePolicy",
                &[
                    ("RoleName", name),
                    ("PolicyName", "rule-action"),
                    ("PolicyDocument", &document),
                ],
            )
            .await;
    }
    arn
}

/// The role every case that is not ABOUT the role uses: one the IoT rules engine may assume
/// and that grants the stream write on the stream the actions name.
async fn given_writer_role(emulator: &Emulator, case: &str, stream_arn: &str) -> String {
    given_role(
        emulator,
        case,
        WRITER_ROLE,
        SERVICE_PRINCIPAL,
        Some((KINESIS_PUT_RECORD_ACTION, stream_arn)),
    )
    .await
}

/// One `kinesis` action of a rule, as a client writes it.
fn kinesis_action(role_arn: &str, partition_key: Option<&str>) -> Value {
    let mut action = json!({ "roleArn": role_arn, "streamName": STREAM });
    if let Some(key) = partition_key {
        action["partitionKey"] = json!(key);
    }
    json!({ "kinesis": action })
}

/// Whether a record's data is the message `expected`: the same text, or the same JSON
/// DOCUMENT where both are documents — a projected document's members are a set, and the
/// order a JSON object writes them in carries no meaning.
fn data_matches(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    match (
        serde_json::from_str::<Value>(expected),
        serde_json::from_str::<Value>(actual),
    ) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => false,
    }
}

/// One row of the routing table: the rule the case stores, the message it publishes, and the
/// records the stream must then hold.
///
/// A row is built by [`Row::new`] with the defaults every row shares — an enabled rule of one
/// `kinesis` action keyed [`KEY`], publishing [`MESSAGE`], expected to deliver that message
/// once — and each builder below changes exactly what its case varies.
struct Row {
    case: &'static str,
    /// The rule's `sql`.
    sql: &'static str,
    /// The topic the message is published to.
    topic: &'static str,
    /// Whether the rule is created disabled.
    disabled: bool,
    /// How many `kinesis` actions the rule carries, and the partition key of each (`None` for
    /// an action naming none).
    keys: Vec<Option<&'static str>>,
    /// The message payload, as published.
    payload: &'static str,
    /// The records the stream must hold afterwards, as (partition key, data) pairs.
    expected: Vec<(Option<&'static str>, &'static str)>,
}

/// The message the rows publish unless one names its own: a document whose members the
/// projection and condition rows select over.
const MESSAGE: &str = r#"{"marker":"m-1","temp":42,"state":{"reported":{"ok":true}}}"#;

/// The partition key the actions name unless a row names another.
const KEY: &str = "by-device";

impl Row {
    fn new(case: &'static str, sql: &'static str, topic: &'static str) -> Self {
        Self {
            case,
            sql,
            topic,
            disabled: false,
            keys: vec![Some(KEY)],
            payload: MESSAGE,
            expected: vec![(Some(KEY), MESSAGE)],
        }
    }

    /// The rule does not deliver this message at all.
    fn delivers_nothing(mut self) -> Self {
        self.expected = Vec::new();
        self
    }

    /// The rule delivers `selected` rather than the message as published.
    fn delivers(mut self, selected: &'static str) -> Self {
        self.expected = vec![(Some(KEY), selected)];
        self
    }

    /// The rule is created disabled.
    fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }

    /// The message published is `payload`, and is what the rule delivers.
    fn publishing(mut self, payload: &'static str) -> Self {
        self.payload = payload;
        self.expected = vec![(Some(KEY), payload)];
        self
    }

    /// The rule carries one `kinesis` action per key in `keys`, each delivering the message
    /// under that key.
    fn keyed(mut self, keys: Vec<Option<&'static str>>) -> Self {
        self.expected = keys.iter().map(|key| (*key, self.payload)).collect();
        self.keys = keys;
        self
    }
}

fn rows() -> Vec<Row> {
    vec![
        Row::new(
            "an exact topic filter and SELECT *",
            "SELECT * FROM 'sensors/one'",
            "sensors/one",
        ),
        Row::new(
            "a single-level wildcard covering the topic",
            "SELECT * FROM 'sensors/+/reading'",
            "sensors/two/reading",
        ),
        Row::new(
            "a multi-level wildcard covering the topic",
            "SELECT * FROM 'sensors/#'",
            "sensors/three/deep/reading",
        ),
        Row::new(
            "a topic filter that does not cover the published topic",
            "SELECT * FROM 'sensors/one'",
            "sensors/other",
        )
        .delivers_nothing(),
        Row::new(
            "a disabled rule whose filter does cover the topic",
            "SELECT * FROM 'sensors/one'",
            "sensors/one",
        )
        .disabled()
        .delivers_nothing(),
        Row::new(
            "a condition that holds for the message",
            "SELECT * FROM 'sensors/one' WHERE temp > 40",
            "sensors/one",
        ),
        Row::new(
            "a condition that does not hold for the message",
            "SELECT * FROM 'sensors/one' WHERE temp > 100",
            "sensors/one",
        )
        .delivers_nothing(),
        Row::new(
            "a condition over a nested attribute and a string comparison",
            "SELECT * FROM 'sensors/one' WHERE state.reported.ok = true AND marker = 'm-1'",
            "sensors/one",
        ),
        Row::new(
            "a condition over an attribute the message does not carry",
            "SELECT * FROM 'sensors/one' WHERE missing = 1",
            "sensors/one",
        )
        .delivers_nothing(),
        Row::new(
            "a condition naming a function of the AWS IoT SQL library",
            "SELECT * FROM 'sensors/one' WHERE startswith(marker, 'm')",
            "sensors/one",
        )
        // An expression this engine does not compute: the rule's actions are not performed,
        // rather than performed on a condition nobody evaluated.
        .delivers_nothing(),
        Row::new(
            "a select clause naming attributes instead of the whole message",
            "SELECT marker, temp AS celsius FROM 'sensors/one'",
            "sensors/one",
        )
        .delivers(r#"{"marker":"m-1","celsius":42}"#),
        Row::new(
            "a select clause naming a nested attribute",
            "SELECT state.reported.ok FROM 'sensors/one'",
            "sensors/one",
        )
        .delivers(r#"{"ok":true}"#),
        Row::new(
            "a select clause naming an attribute the message does not carry",
            "SELECT marker, missing FROM 'sensors/one'",
            "sensors/one",
        )
        .delivers(r#"{"marker":"m-1"}"#),
        Row::new(
            "a payload that is not a JSON document, selected whole",
            "SELECT * FROM 'sensors/one'",
            "sensors/one",
        )
        .publishing("not a document"),
        Row::new(
            "two kinesis actions on one rule",
            "SELECT * FROM 'sensors/one'",
            "sensors/one",
        )
        .keyed(vec![Some("first-key"), Some("second-key")]),
        Row::new(
            "an action naming no partitionKey",
            "SELECT * FROM 'sensors/one'",
            "sensors/one",
        )
        .keyed(vec![None]),
        Row::new(
            "an action whose partitionKey is a substitution template",
            "SELECT * FROM 'sensors/one'",
            "sensors/one",
        )
        // A key this engine does not render: no record, rather than one keyed on the
        // template's literal text.
        .keyed(vec![Some("${newuuid()}")])
        .delivers_nothing(),
        Row::new(
            "an action whose partitionKey is the empty string",
            "SELECT * FROM 'sensors/one'",
            "sensors/one",
        )
        // `PartitionKey` is `length 1..256`, so an empty key is not a value Kinesis takes:
        // no record, rather than one under a key this emulator chose.
        .keyed(vec![Some("")])
        .delivers_nothing(),
        Row::new(
            "a statement whose FROM clause names no topic filter",
            "SELECT * FROM sensors",
            "sensors",
        )
        .delivers_nothing(),
    ]
}

/// Every enabled rule whose topic filter covers the published topic performs its `kinesis`
/// actions for the message its statement selects, and no other rule performs anything.
#[tokio::test]
async fn a_published_message_is_written_to_the_stream_of_every_rule_that_selects_it() {
    for row in rows() {
        let emulator = emulator();
        let stream_arn = given_stream(&emulator, row.case, STREAM).await;
        let role_arn = given_writer_role(&emulator, row.case, &stream_arn).await;
        let actions: Vec<Value> = row
            .keys
            .iter()
            .map(|key| kinesis_action(&role_arn, *key))
            .collect();
        emulator
            .create_topic_rule(
                row.case,
                "to_the_stream",
                &json!({
                    "sql": row.sql,
                    "awsIotSqlVersion": "2016-03-23",
                    "ruleDisabled": row.disabled,
                    "actions": actions,
                }),
            )
            .await;

        emulator
            .publish(row.case, row.topic, row.payload.as_bytes())
            .await;

        let held = records(&emulator, row.case, STREAM).await;
        assert_eq!(
            held.len(),
            row.expected.len(),
            "case {:?}: expected {} record(s) on the stream, got {held:?}",
            row.case,
            row.expected.len()
        );
        for (key, data) in &row.expected {
            let matching: Vec<&Held> = held
                .iter()
                .filter(|record| {
                    data_matches(data, &record.data) && key.is_none_or(|named| record.key == named)
                })
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "case {:?}: expected exactly one record carrying {data:?} under the key \
                 {key:?}, got {held:?}",
                row.case
            );
            // An action that names no partition key still writes one, because Kinesis takes
            // no record without it: the key is then the record's own rather than empty.
            if key.is_none() {
                assert!(
                    !matching[0].key.is_empty(),
                    "case {:?}: a record written for an action naming no partitionKey must \
                     still carry one, got {held:?}",
                    row.case
                );
            }
        }
    }
}

/// One row of the role table: what the action's role says about the write, and whether the
/// record is written at all.
struct RoleRow {
    case: &'static str,
    /// The service principal the role's trust policy admits.
    principal: &'static str,
    /// The `(action, resource)` its inline policy allows, or `None` for a role carrying no
    /// inline policy at all.
    allowed: Option<(&'static str, &'static str)>,
    /// The one action the role's PERMISSIONS BOUNDARY allows, or `None` for a role carrying
    /// no boundary. "A permissions boundary sets the maximum permissions that an
    /// identity-based policy can grant", so a boundary that does not cover the write caps it
    /// away however the role's own policies read.
    boundary: Option<&'static str>,
    /// Whether the record reaches the stream.
    written: bool,
}

impl RoleRow {
    /// A row whose role trusts the IoT service principal, allows `allowed` on the stream the
    /// action writes (`None` for no inline policy) and carries no boundary.
    fn new(case: &'static str, allowed: Option<(&'static str, &'static str)>, written: bool) -> Self {
        Self {
            case,
            principal: SERVICE_PRINCIPAL,
            allowed,
            boundary: None,
            written,
        }
    }
}

/// Cap the role `name` by a permissions boundary allowing `action` on every resource: a
/// managed policy of this account, attached as the role's boundary the way a client does it
/// (`CreatePolicy` + `PutRolePermissionsBoundary`).
async fn given_boundary(emulator: &Emulator, case: &str, name: &str, action: &str) {
    let document = identity_policy(action, "*");
    let created = emulator
        .iam(
            case,
            "CreatePolicy",
            &[
                ("PolicyName", "rule-action-boundary"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
    let arn = xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy reported no Arn: {created}"));
    emulator
        .iam(
            case,
            "PutRolePermissionsBoundary",
            &[("RoleName", name), ("PermissionsBoundary", &arn)],
        )
        .await;
}

/// The ARN of a stream this account does not hold, which a grant may name instead of the
/// stream the action writes: built the way every emulated ARN is, so no literal from any one
/// run appears here.
fn other_stream_arn(stream_arn: &str) -> String {
    stream_arn.replace(STREAM, "another-stream")
}

/// A `kinesis` action is performed only where its role admits the IoT service principal and
/// grants `kinesis:PutRecord` on the stream it names; otherwise nothing is written.
#[tokio::test]
async fn a_kinesis_actions_role_decides_whether_the_record_is_written() {
    let rows: Vec<RoleRow> = vec![
        RoleRow::new(
            "the role grants the stream write on that stream",
            Some((KINESIS_PUT_RECORD_ACTION, "")),
            true,
        ),
        RoleRow::new(
            "the role grants every kinesis action on every resource",
            Some(("kinesis:*", "*")),
            true,
        ),
        RoleRow::new("the role carries no inline policy at all", None, false),
        RoleRow::new(
            "the role's policy names another action on that stream",
            Some(("kinesis:DescribeStream", "")),
            false,
        ),
        RoleRow::new(
            "the role's policy grants the write on another stream",
            Some((KINESIS_PUT_RECORD_ACTION, "other")),
            false,
        ),
        RoleRow {
            case: "the role does not trust the IoT service principal",
            principal: "lambda.amazonaws.com",
            allowed: Some((KINESIS_PUT_RECORD_ACTION, "")),
            boundary: None,
            written: false,
        },
        RoleRow {
            case: "a permissions boundary that covers the stream write",
            principal: SERVICE_PRINCIPAL,
            allowed: Some((KINESIS_PUT_RECORD_ACTION, "")),
            boundary: Some("kinesis:*"),
            written: true,
        },
        RoleRow {
            case: "a permissions boundary that caps the stream write away",
            principal: SERVICE_PRINCIPAL,
            allowed: Some((KINESIS_PUT_RECORD_ACTION, "")),
            boundary: Some("kinesis:DescribeStream"),
            written: false,
        },
    ];

    for row in rows {
        let emulator = emulator();
        let stream_arn = given_stream(&emulator, row.case, STREAM).await;
        let other = other_stream_arn(&stream_arn);
        // An empty resource in the row means the stream the action writes; `other` means a
        // stream of this account the action does not write.
        let allowed = row.allowed.map(|(action, resource)| {
            let resource = match resource {
                "" => stream_arn.as_str(),
                "other" => other.as_str(),
                literal => literal,
            };
            (action, resource)
        });
        let role_arn = given_role(&emulator, row.case, WRITER_ROLE, row.principal, allowed).await;
        if let Some(action) = row.boundary {
            given_boundary(&emulator, row.case, WRITER_ROLE, action).await;
        }
        emulator
            .create_topic_rule(
                row.case,
                "to_the_stream",
                &json!({
                    "sql": "SELECT * FROM 'sensors/one'",
                    "actions": [kinesis_action(&role_arn, Some("by-device"))],
                }),
            )
            .await;

        emulator
            .publish(row.case, "sensors/one", MESSAGE.as_bytes())
            .await;

        let held = records(&emulator, row.case, STREAM).await;
        if row.written {
            assert_eq!(
                held,
                vec![Held {
                    key: "by-device".to_string(),
                    data: MESSAGE.to_string(),
                }],
                "case {:?}: expected the selected message on the stream",
                row.case
            );
        } else {
            assert!(
                held.is_empty(),
                "case {:?}: expected no record for a write the role does not grant, got \
                 {held:?}",
                row.case
            );
        }
    }
}

/// The permission boundary as a client verifies it: two sibling rules on ONE stream, one
/// whose role grants the stream write and one whose role grants nothing, with each leg
/// publishing its own marker.
///
/// The permitted leg's marker must appear — without it the case could not tell an enforced
/// boundary from a rules engine that never ran — and the unpermitted leg's must never
/// appear, however many messages are published.
#[tokio::test]
async fn only_the_permitted_sibling_rules_messages_reach_the_stream() {
    let emulator = emulator();
    let case = "sibling rules on one stream";
    let stream_arn = given_stream(&emulator, case, STREAM).await;
    let permitted = given_role(
        &emulator,
        case,
        "permitted-rule-role",
        SERVICE_PRINCIPAL,
        Some((KINESIS_PUT_RECORD_ACTION, &stream_arn)),
    )
    .await;
    let unpermitted = given_role(
        &emulator,
        case,
        "unpermitted-rule-role",
        SERVICE_PRINCIPAL,
        None,
    )
    .await;
    emulator
        .create_topic_rule(
            case,
            "permitted_leg",
            &json!({
                "sql": "SELECT * FROM 'boundary/permitted'",
                "actions": [kinesis_action(&permitted, Some("sim-partition"))],
            }),
        )
        .await;
    emulator
        .create_topic_rule(
            case,
            "unpermitted_leg",
            &json!({
                "sql": "SELECT * FROM 'boundary/unpermitted'",
                "actions": [kinesis_action(&unpermitted, Some("sim-partition"))],
            }),
        )
        .await;

    // Several message pairs, so neither leg can be right by accident: every permitted
    // marker lands, every unpermitted one never does.
    let markers = ["alpha", "beta", "gamma"];
    for marker in markers {
        let good = json!({ "marker": format!("good-{marker}") }).to_string();
        let bad = json!({ "marker": format!("bad-{marker}") }).to_string();
        emulator
            .publish(case, "boundary/permitted", good.as_bytes())
            .await;
        emulator
            .publish(case, "boundary/unpermitted", bad.as_bytes())
            .await;
    }

    let held = records(&emulator, case, STREAM).await;
    let delivered: Vec<String> = held.iter().map(|record| record.data.clone()).collect();
    for marker in markers {
        let good = json!({ "marker": format!("good-{marker}") }).to_string();
        let bad = json!({ "marker": format!("bad-{marker}") }).to_string();
        assert!(
            delivered.contains(&good),
            "case {case:?}: the permitted sibling rule must deliver its message for \
             {marker:?}, got {held:?}"
        );
        assert!(
            !delivered.contains(&bad),
            "case {case:?}: a kinesis action whose role grants no {KINESIS_PUT_RECORD_ACTION} \
             on this stream must deliver nothing for {marker:?}, got {held:?}"
        );
    }
    assert_eq!(
        held.len(),
        markers.len(),
        "case {case:?}: expected one record per permitted publish and no others, got {held:?}"
    );
}
