// SPDX-License-Identifier: CC0-1.0

//! `CreateEnvironment`'s `@idempotent` replay check, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: a second `CreateEnvironment` for a name that is
//! already stored is a REPLAY — answered with the stored ARN, writing nothing — only when
//! it asks for exactly the environment that is stored in every member the create records,
//! the configuration and the tag pairs alike; a second create differing in any of those
//! members is a different request and is refused with HTTP 400 `ValidationException`,
//! leaving the stored environment untouched. No member a create writes may be answered
//! success and then dropped.
//!
//! Each table row varies what the rule generalises over: WHICH recorded member differs (a
//! tag value, a tag key, the `Tags` member's presence, or one of the configuration members
//! `DagS3Path` / `MaxWorkers` / `AirflowVersion` / `EnvironmentClass`), HOW it differs
//! (changed, added, removed, re-cased, omitted) and HOW MANY tags travel with the request
//! (none, one, a handful, the full `TagMap` of `state::MAX_TAGS`). Every refusal row is
//! paired with a read-back of the stored environment, so a refusal is shown to change
//! nothing, and the replay rows pin the other side of the comparison.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/airflow/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region the `AUTHORIZATION` scope above signs for; every ARN this service mints for
/// these requests carries it.
const REGION: &str = "us-east-1";

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The members of the `Environment` a case may move: the tag map plus every configuration
/// member a row changes. Read back on both sides of a refused create, so "nothing was
/// written" is asserted on the values rather than only on the status.
const WATCHED_MEMBERS: &[&str] = &[
    "Tags",
    "DagS3Path",
    "MaxWorkers",
    "AirflowVersion",
    "EnvironmentClass",
];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn arn(&self) -> Value {
        self.json().get("Arn").cloned().unwrap_or(Value::Null)
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn create(&self, name: &str, body: Value) -> Answer {
        self.send("PUT", &format!("/environments/{name}"), Some(body))
            .await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, name: &str) -> Value {
        let answer = self.send("GET", &format!("/environments/{name}"), None).await;
        assert_eq!(
            answer.status,
            200,
            "GetEnvironment({name}) must succeed, got {} {}",
            answer.status,
            answer.text()
        );
        answer
            .json()
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| panic!("GetEnvironment({name}) answered without an Environment"))
    }

    /// The `Tags` map `ListTagsForResource` reports, or `Value::Null` when it reports none.
    async fn tags(&self, arn: &str) -> Value {
        let answer = self
            .send("GET", &format!("/tags/{}", encode_label(arn)), None)
            .await;
        assert_eq!(
            answer.status,
            200,
            "ListTagsForResource({arn}) must succeed, got {} {}",
            answer.status,
            answer.text()
        );
        answer.json().get("Tags").cloned().unwrap_or(Value::Null)
    }

    async fn tag(&self, arn: &str, tags: Value) -> Answer {
        self.send(
            "POST",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({"Tags": tags})),
        )
        .await
    }
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// A create body every member of which satisfies its modelled constraints, carrying the
/// given tag pairs (no `Tags` member at all when `tags` is `None`).
fn create_body(tags: Option<&[(String, String)]>) -> Value {
    let mut body = json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
        "DagS3Path": "dags",
        "NetworkConfiguration": {
            "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            "SecurityGroupIds": ["sg-00000000000000001"],
        },
    });
    if let Some(tags) = tags {
        body.as_object_mut()
            .expect("the create body is an object")
            .insert("Tags".to_string(), tag_map(tags));
    }
    body
}

/// A `TagMap` JSON object of the given pairs. Built rather than written as a `json!`
/// literal because the keys are the table's own values.
fn tag_map(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

fn pairs(literal: &[(&str, &str)]) -> Vec<(String, String)> {
    literal
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `count` distinct tag pairs, for the rows that vary how MANY tags travel with a create
/// rather than which ones.
fn generated_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("Key{index:02}"), format!("value-{index:02}")))
        .collect()
}

/// How a second create of an already stored name differs from the create that stored it,
/// and therefore whether it is a replay.
struct Case {
    /// Names the case in every assertion.
    label: &'static str,
    /// The `Tags` the first create carries; empty means it carries no `Tags` member.
    stored_tags: Vec<(String, String)>,
    /// The `Tags` the second create carries; `None` means it omits the member.
    second_tags: Option<Vec<(String, String)>>,
    /// A configuration member the second create changes, if any.
    config_change: Option<(&'static str, Value)>,
    /// Whether the second create asks for exactly the stored environment.
    replay: bool,
}

/// The table. Rows 1–4 are the replays (what the comparison must ACCEPT, across tag
/// counts); the rest each differ in exactly one recorded member, so the member that makes
/// a create a different request is what varies and not the request's size or shape.
///
/// `AirflowVersion` and `EnvironmentClass` rows take their changed value from the catalogs
/// the service publishes (`state::SUPPORTED_AIRFLOW_VERSIONS`,
/// `state::ENVIRONMENT_CLASSES`), so the second create is a VALID create that differs,
/// never a refusal for some other reason.
fn cases() -> Vec<Case> {
    let changed_version = state::SUPPORTED_AIRFLOW_VERSIONS
        .iter()
        .find(|version| **version != state::LATEST_AIRFLOW_VERSION)
        .copied()
        .expect("the service publishes more than one Apache Airflow version");
    // A class whose default scheduler and web-server counts match the default class's, so
    // the second create differs in `EnvironmentClass` ALONE and stays a valid create.
    let default_units = state::default_unit_count(state::DEFAULT_ENVIRONMENT_CLASS);
    let changed_class = state::ENVIRONMENT_CLASSES
        .iter()
        .find(|class| {
            **class != state::DEFAULT_ENVIRONMENT_CLASS
                && state::default_unit_count(class) == default_units
        })
        .copied()
        .expect("the service publishes more than one environment class");

    vec![
        Case {
            label: "the identical create of an untagged environment",
            stored_tags: Vec::new(),
            second_tags: None,
            config_change: None,
            replay: true,
        },
        Case {
            label: "the identical create of a one-tag environment",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Team", "data")])),
            config_change: None,
            replay: true,
        },
        Case {
            label: "the identical create of a three-tag environment",
            stored_tags: pairs(&[("Team", "data"), ("CostCenter", "42"), ("Env", "Staging")]),
            second_tags: Some(pairs(&[
                ("Env", "Staging"),
                ("Team", "data"),
                ("CostCenter", "42"),
            ])),
            config_change: None,
            replay: true,
        },
        Case {
            label: "the identical create of an environment carrying a full TagMap",
            stored_tags: generated_tags(state::MAX_TAGS),
            second_tags: Some(generated_tags(state::MAX_TAGS)),
            config_change: None,
            replay: true,
        },
        Case {
            label: "one tag's value changed",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Team", "platform")])),
            config_change: None,
            replay: false,
        },
        Case {
            label: "one tag's value re-cased",
            stored_tags: pairs(&[("Team", "Data")]),
            second_tags: Some(pairs(&[("Team", "data")])),
            config_change: None,
            replay: false,
        },
        Case {
            label: "a tag key added beside the stored one",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Team", "data"), ("CostCenter", "42")])),
            config_change: None,
            replay: false,
        },
        Case {
            label: "a tag key dropped from the stored pair set",
            stored_tags: pairs(&[("Team", "data"), ("CostCenter", "42")]),
            second_tags: Some(pairs(&[("Team", "data")])),
            config_change: None,
            replay: false,
        },
        Case {
            label: "a tag key renamed, its value kept",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Squad", "data")])),
            config_change: None,
            replay: false,
        },
        Case {
            label: "the Tags member omitted for a tagged environment",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: None,
            config_change: None,
            replay: false,
        },
        Case {
            label: "a Tags member added for an untagged environment",
            stored_tags: Vec::new(),
            second_tags: Some(pairs(&[("Team", "data")])),
            config_change: None,
            replay: false,
        },
        Case {
            label: "one tag of a full TagMap changed",
            stored_tags: generated_tags(state::MAX_TAGS),
            second_tags: Some({
                let mut tags = generated_tags(state::MAX_TAGS);
                tags[state::MAX_TAGS - 1].1 = "moved".to_string();
                tags
            }),
            config_change: None,
            replay: false,
        },
        Case {
            label: "DagS3Path changed, the tags identical",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Team", "data")])),
            config_change: Some(("DagS3Path", json!("somewhere/else"))),
            replay: false,
        },
        Case {
            label: "MaxWorkers changed on an untagged environment",
            stored_tags: Vec::new(),
            second_tags: None,
            config_change: Some(("MaxWorkers", json!(25))),
            replay: false,
        },
        Case {
            label: "AirflowVersion changed, the tags identical",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Team", "data")])),
            config_change: Some(("AirflowVersion", json!(changed_version))),
            replay: false,
        },
        Case {
            label: "EnvironmentClass changed, the tags identical",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Team", "data")])),
            config_change: Some(("EnvironmentClass", json!(changed_class))),
            replay: false,
        },
        Case {
            label: "a configuration member and the tags both changed",
            stored_tags: pairs(&[("Team", "data")]),
            second_tags: Some(pairs(&[("Team", "platform"), ("CostCenter", "42")])),
            config_change: Some(("DagS3Path", json!("somewhere/else"))),
            replay: false,
        },
    ]
}

impl Case {
    /// The create that stores the environment.
    fn first_body(&self) -> Value {
        create_body((!self.stored_tags.is_empty()).then_some(self.stored_tags.as_slice()))
    }

    /// The second create: the same body, with this row's `Tags` and configuration change.
    fn second_body(&self) -> Value {
        let mut body = create_body(self.second_tags.as_deref());
        if let Some((member, value)) = self.config_change.as_ref() {
            body.as_object_mut()
                .expect("the create body is an object")
                .insert((*member).to_string(), value.clone());
        }
        body
    }

    /// What the tag map of the stored environment reads back as: `TagMap` is
    /// `@length(1, 50)`, so an untagged environment reports no `Tags` member at all.
    fn stored_tags_member(&self) -> Value {
        if self.stored_tags.is_empty() {
            Value::Null
        } else {
            tag_map(&self.stored_tags)
        }
    }
}

/// The watched members of `environment`, so a refused create can be shown to have changed
/// none of them.
fn watched(environment: &Value) -> Vec<(&'static str, Value)> {
    WATCHED_MEMBERS
        .iter()
        .map(|member| {
            (
                *member,
                environment.get(*member).cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_second_create_replays_only_when_every_recorded_member_matches_the_stored_environment() {
    let harness = Harness::new();

    for (index, case) in cases().into_iter().enumerate() {
        let name = format!("ReplayCase{index:02}");
        let arn = format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}");

        let first = harness.create(&name, case.first_body()).await;
        assert_eq!(
            first.status,
            200,
            "case {:?}: the first create must succeed, got {} {}",
            case.label,
            first.status,
            first.text()
        );
        assert_eq!(
            first.arn(),
            json!(arn),
            "case {:?}: the create must answer the ARN of the environment it stored, got {:?}",
            case.label,
            first.arn()
        );
        assert_eq!(
            harness.tags(&arn).await,
            case.stored_tags_member(),
            "case {:?}: the create must store the Tags it carried, got {:?}",
            case.label,
            harness.tags(&arn).await
        );

        let before = watched(&harness.environment(&name).await);

        let second = harness.create(&name, case.second_body()).await;
        if case.replay {
            assert_eq!(
                second.status,
                200,
                "case {:?}: a create asking for exactly the stored environment is a replay \
                 and must succeed, got {} {}",
                case.label,
                second.status,
                second.text()
            );
            assert_eq!(
                second.arn(),
                first.arn(),
                "case {:?}: a replay must answer the stored ARN, got {:?}",
                case.label,
                second.arn()
            );
        } else {
            assert_eq!(
                second.status,
                400,
                "case {:?}: a create of a taken name differing in a recorded member must be \
                 refused, got {} {}",
                case.label,
                second.status,
                second.text()
            );
            assert_eq!(
                second.error_code(),
                "ValidationException",
                "case {:?}: a name clash is a ValidationException, got {:?}",
                case.label,
                second.error_code()
            );
            let message = second.message();
            assert!(
                message.contains(&name) && message.contains("already exists"),
                "case {:?}: the refusal must name the environment whose name is taken, got {message:?}",
                case.label
            );
        }

        // Either way the stored environment is what the FIRST create stored: a replay
        // writes nothing, and a refused create writes nothing.
        let after = watched(&harness.environment(&name).await);
        assert_eq!(
            after,
            before,
            "case {:?}: the second create must leave every recorded member as stored, got {:?} \
             instead of {:?}",
            case.label,
            after,
            before
        );
        assert_eq!(
            harness.tags(&arn).await,
            case.stored_tags_member(),
            "case {:?}: ListTagsForResource must still report the stored tag map, got {:?}",
            case.label,
            harness.tags(&arn).await
        );
    }
}

#[tokio::test]
async fn a_create_matching_the_tag_map_a_later_write_left_is_still_a_replay() {
    let harness = Harness::new();
    let name = "ReplayAfterTagWrite";
    let arn = format!("arn:aws:airflow:{REGION}:{ACCOUNT}:environment/{name}");
    let stored = pairs(&[("Team", "data")]);
    let written = pairs(&[("Team", "data"), ("CostCenter", "42")]);

    assert_eq!(
        harness
            .create(name, create_body(Some(stored.as_slice())))
            .await
            .status,
        200,
        "the create that stores the environment must succeed"
    );
    assert_eq!(
        harness.tag(&arn, tag_map(&written)).await.status,
        200,
        "the tag write must succeed"
    );

    // The comparison is made against what is STORED, not against what the first create
    // carried: the tag map `TagResource` left is the environment's tag map now, so the
    // create that asks for exactly that map is the replay...
    let replay = harness
        .create(name, create_body(Some(written.as_slice())))
        .await;
    assert_eq!(
        replay.status,
        200,
        "a create asking for the tag map now stored must replay, got {} {}",
        replay.status,
        replay.text()
    );
    assert_eq!(
        replay.arn(),
        json!(arn),
        "a replay must answer the stored ARN, got {:?}",
        replay.arn()
    );

    // ...and the create carrying the pairs the environment NO LONGER has is a different
    // create of a taken name, so it is refused rather than answered success with its Tags
    // dropped.
    let refused = harness
        .create(name, create_body(Some(stored.as_slice())))
        .await;
    assert_eq!(
        refused.status,
        400,
        "a create carrying tags the stored environment does not have must be refused, got {} {}",
        refused.status,
        refused.text()
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "a name clash is a ValidationException, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        harness.tags(&arn).await,
        tag_map(&written),
        "the refused create must leave the stored tag map as the tag write left it, got {:?}",
        harness.tags(&arn).await
    );
}
