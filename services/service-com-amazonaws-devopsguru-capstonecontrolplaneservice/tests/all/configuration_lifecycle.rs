// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the client-driven DevOps Guru configuration lifecycle:
//! resource-collection coverage, notification channels and the service integration.
//!
//! Every test drives the service the way a client does — a buffered HTTP request
//! through the same [`MultiServiceRouter`] the emulator boots — so the assertions are
//! about the wire behaviour, not about handler internals.
//!
//! Each test states the RULE it pins and exercises it over a set of inputs that
//! differ along what the rule generalises over (value, count, repetition, order,
//! form, collection type), with the request from the issue report as one row.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    ComAmazonawsDevopsguruCapstonecontrolplaneserviceState, InsightRecord, MAX_COVERED_STACK_NAMES,
    MAX_COVERED_TAG_KEYS, MAX_NOTIFICATION_CHANNELS, MAX_TAG_VALUES_PER_KEY, PAGE_SIZE,
};

/// Host that routes a request to this service: `resolve_route_key` falls back to the
/// `Host` header's first label, which is this service's endpoint prefix.
const HOST: &str = "devops-guru.us-east-1.amazonaws.com";

/// One reply, as a client sees it.
///
/// `pub(crate)` because the other suites in this test crate drive the service through
/// the same harness rather than standing up a second one.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Value,
    /// The error SHAPE NAME the reply carries, from the `x-amzn-errortype` header or
    /// the body's `__type`, with any namespace/URL prefix stripped. Empty on a success.
    error_name: String,
    pub(crate) raw: String,
}

impl Reply {
    /// Whether the rendered error IS `code` — the parsed error name, not a substring of
    /// the body, so an error name quoted inside a message cannot satisfy it.
    pub(crate) fn names_error(&self, code: &str) -> bool {
        self.error_name == code
    }

    pub(crate) fn at(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer)
    }

    pub(crate) fn strings_at(&self, pointer: &str) -> Vec<String> {
        self.at(pointer)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A fresh emulator holding only this service, over its own engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The one engine the registered service reads and writes, kept so a suite can
    /// seed the records DevOps Guru's own analysis would write — no modelled
    /// operation mints an insight, so this is the only way to drive the insight
    /// reads over a non-empty store.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process-wide switch.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_devopsguru_capstonecontrolplaneservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Put `records` in the store's `insights` map, as the analysis that produces them
    /// would. Keyed by `InsightRecord::id`, which is the map's key.
    pub(crate) fn seed_insights(&self, records: Vec<InsightRecord>) {
        let state = self
            .engine
            .state::<ComAmazonawsDevopsguruCapstonecontrolplaneserviceState>();
        let mut guard = state.lock();
        for record in records {
            guard.insights.insert(record.id.clone(), record);
        }
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", HOST)
            .header("content-type", "application/json")
            .body(Bytes::from(payload))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let raw = format!("{errortype} {}", String::from_utf8_lossy(&bytes));
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        // restJson1 names the error in the `x-amzn-errortype` header, the body's
        // `__type`, or both; either may carry a `namespace#Name` or URL prefix.
        let declared = if errortype.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            errortype
        };
        let error_name = declared
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        Reply {
            status,
            body,
            error_name,
            raw,
        }
    }

    pub(crate) async fn update_resource_collection(
        &self,
        action: &str,
        collection: Value,
    ) -> Reply {
        self.call(
            "PUT",
            "/resource-collections",
            Some(&json!({ "Action": action, "ResourceCollection": collection })),
        )
        .await
    }

    pub(crate) async fn get_resource_collection(
        &self,
        collection_type: &str,
        next: Option<&str>,
    ) -> Reply {
        let path = match next {
            Some(token) => format!("/resource-collections/{collection_type}?NextToken={token}"),
            None => format!("/resource-collections/{collection_type}"),
        };
        self.call("GET", &path, None).await
    }

    async fn add_notification_channel(&self, config: Value) -> Reply {
        self.call("PUT", "/channels", Some(&json!({ "Config": config })))
            .await
    }

    async fn list_notification_channels(&self, next: Option<&str>) -> Reply {
        let body = match next {
            Some(token) => json!({ "NextToken": token }),
            None => json!({}),
        };
        self.call("POST", "/channels", Some(&body)).await
    }

    async fn remove_notification_channel(&self, id: &str) -> Reply {
        self.call("DELETE", &format!("/channels/{id}"), None).await
    }

    async fn describe_service_integration(&self) -> Reply {
        self.call("GET", "/service-integrations", None).await
    }

    async fn update_service_integration(&self, integration: Value) -> Reply {
        self.call(
            "PUT",
            "/service-integrations",
            Some(&json!({ "ServiceIntegration": integration })),
        )
        .await
    }

    /// Every covered stack name, walking `GetResourceCollection`'s pages.
    pub(crate) async fn all_stack_names(&self) -> Vec<String> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let reply = self
                .get_resource_collection("AWS_CLOUD_FORMATION", token.as_deref())
                .await;
            assert_eq!(
                reply.status, 200,
                "walking AWS_CLOUD_FORMATION must succeed, got {}",
                reply.raw
            );
            all.extend(reported_stack_names(&reply));
            token = reply
                .at("/NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                return all;
            }
        }
    }

    /// Every covered tag collection, walking `GetResourceCollection`'s pages.
    pub(crate) async fn all_tag_collections(&self) -> Vec<(String, Vec<String>)> {
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let reply = self
                .get_resource_collection("AWS_TAGS", token.as_deref())
                .await;
            assert_eq!(
                reply.status, 200,
                "walking AWS_TAGS must succeed, got {}",
                reply.raw
            );
            all.extend(reported_tag_collections(&reply));
            token = reply
                .at("/NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                return all;
            }
        }
    }
}

/// `NotificationChannelId`'s pattern, from
/// `smithy_query(query="shape",shape="NotificationChannelId")`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, length 36..36.
fn is_modeled_channel_id(value: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let parts: Vec<&str> = value.split('-').collect();
    value.len() == 36
        && parts.len() == groups.len()
        && parts.iter().zip(groups).all(|(part, width)| {
            part.len() == width
                && part
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
}

/// The `ResourceCollection.CloudFormation.StackNames` a read reported.
fn reported_stack_names(reply: &Reply) -> Vec<String> {
    reply.strings_at("/ResourceCollection/CloudFormation/StackNames")
}

/// The `ResourceCollection.Tags` a read reported, as (AppBoundaryKey, TagValues).
fn reported_tag_collections(reply: &Reply) -> Vec<(String, Vec<String>)> {
    reply
        .at("/ResourceCollection/Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag.get("AppBoundaryKey")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        tag.get("TagValues")
                            .and_then(Value::as_array)
                            .map(|values| {
                                values
                                    .iter()
                                    .map(|v| v.as_str().unwrap_or_default().to_string())
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn cloud_formation(stack_names: &[&str]) -> Value {
    json!({ "CloudFormation": { "StackNames": stack_names } })
}

pub(crate) fn tags(entries: &[(&str, &[&str])]) -> Value {
    let tags: Vec<Value> = entries
        .iter()
        .map(|(key, values)| json!({ "AppBoundaryKey": key, "TagValues": values }))
        .collect();
    json!({ "Tags": tags })
}

/// One step of a coverage case: an `Action` plus the filter it carries.
type Step<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, &'a [&'a str])]);

struct CoverageCase<'a> {
    name: &'a str,
    steps: &'a [Step<'a>],
    expect_stacks: &'a [&'a str],
    expect_tags: &'a [(&'a str, &'a [&'a str])],
}

/// RULE: resource-collection coverage is a SET per collection kind — `ADD` unions the
/// submitted stack names and tag key/values into it, `REMOVE` subtracts them, and a
/// read reports each covered member exactly once in a stable order. Repeating a
/// request therefore changes nothing, and removing something uncovered is a no-op.
///
/// The rows differ along value, count, repetition, order, overlap and which half of
/// the filter they touch; the issue's single `ADD ["my-stack"]` is the first row.
#[tokio::test]
async fn coverage_is_a_set_that_add_unions_and_remove_subtracts() {
    let cases = [
        CoverageCase {
            name: "one stack added",
            steps: &[("ADD", &["my-stack"], &[])],
            expect_stacks: &["my-stack"],
            expect_tags: &[],
        },
        CoverageCase {
            name: "the same stack added twice",
            steps: &[("ADD", &["my-stack"], &[]), ("ADD", &["my-stack"], &[])],
            expect_stacks: &["my-stack"],
            expect_tags: &[],
        },
        CoverageCase {
            name: "a stack repeated inside one request",
            steps: &[("ADD", &["alpha", "alpha", "alpha"], &[])],
            expect_stacks: &["alpha"],
            expect_tags: &[],
        },
        CoverageCase {
            name: "overlapping adds union, and are reported sorted",
            steps: &[
                ("ADD", &["zeta", "alpha"], &[]),
                ("ADD", &["middle", "alpha"], &[]),
            ],
            expect_stacks: &["alpha", "middle", "zeta"],
            expect_tags: &[],
        },
        CoverageCase {
            name: "remove subtracts only the names it carries",
            steps: &[
                ("ADD", &["alpha", "beta", "gamma"], &[]),
                ("REMOVE", &["beta"], &[]),
            ],
            expect_stacks: &["alpha", "gamma"],
            expect_tags: &[],
        },
        CoverageCase {
            name: "removing an uncovered name is a no-op",
            steps: &[("ADD", &["alpha"], &[]), ("REMOVE", &["beta"], &[])],
            expect_stacks: &["alpha"],
            expect_tags: &[],
        },
        CoverageCase {
            name: "removing everything empties the coverage",
            steps: &[
                ("ADD", &["alpha", "beta"], &[]),
                ("REMOVE", &["beta", "alpha"], &[]),
            ],
            expect_stacks: &[],
            expect_tags: &[],
        },
        CoverageCase {
            name: "an empty stack list changes nothing",
            steps: &[("ADD", &["alpha"], &[]), ("ADD", &[], &[])],
            expect_stacks: &["alpha"],
            expect_tags: &[],
        },
        CoverageCase {
            name: "tag values union under one app boundary key",
            steps: &[
                ("ADD", &[], &[("Devops-guru-app", &["x", "y"])]),
                ("ADD", &[], &[("Devops-guru-app", &["y", "z"])]),
            ],
            expect_stacks: &[],
            expect_tags: &[("Devops-guru-app", &["x", "y", "z"])],
        },
        CoverageCase {
            name: "several app boundary keys are kept apart",
            steps: &[(
                "ADD",
                &[],
                &[
                    ("Devops-guru-rds", &["prod"]),
                    ("Devops-guru-app", &["prod", "test"]),
                ],
            )],
            expect_stacks: &[],
            expect_tags: &[
                ("Devops-guru-app", &["prod", "test"]),
                ("Devops-guru-rds", &["prod"]),
            ],
        },
        CoverageCase {
            name: "a key left with no values stops being covered",
            steps: &[
                (
                    "ADD",
                    &[],
                    &[
                        ("Devops-guru-app", &["x"]),
                        ("Devops-guru-rds", &["q"]),
                    ],
                ),
                ("REMOVE", &[], &[("Devops-guru-app", &["x"])]),
            ],
            expect_stacks: &[],
            expect_tags: &[("Devops-guru-rds", &["q"])],
        },
        CoverageCase {
            name: "stack coverage and tag coverage are independent",
            steps: &[
                ("ADD", &["alpha"], &[("Devops-guru-app", &["x"])]),
                ("REMOVE", &["alpha"], &[]),
            ],
            expect_stacks: &[],
            expect_tags: &[("Devops-guru-app", &["x"])],
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for (action, stacks, tag_entries) in case.steps {
            let mut collection = json!({});
            if !stacks.is_empty() || tag_entries.is_empty() {
                collection["CloudFormation"] = cloud_formation(stacks)["CloudFormation"].clone();
            }
            if !tag_entries.is_empty() {
                collection["Tags"] = tags(tag_entries)["Tags"].clone();
            }
            let reply = emulator.update_resource_collection(action, collection).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: {action} must succeed, got status {} body {}",
                case.name, reply.status, reply.raw
            );
        }

        let cfn = emulator
            .get_resource_collection("AWS_CLOUD_FORMATION", None)
            .await;
        assert_eq!(
            cfn.status, 200,
            "case {:?}: AWS_CLOUD_FORMATION read must succeed, got {}",
            case.name, cfn.raw
        );
        let actual_stacks = reported_stack_names(&cfn);
        assert_eq!(
            actual_stacks, case.expect_stacks,
            "case {:?}: expected stack names {:?}, got {actual_stacks:?}",
            case.name, case.expect_stacks
        );

        let tag_reply = emulator.get_resource_collection("AWS_TAGS", None).await;
        assert_eq!(
            tag_reply.status, 200,
            "case {:?}: AWS_TAGS read must succeed, got {}",
            case.name, tag_reply.raw
        );
        let actual_tags = reported_tag_collections(&tag_reply);
        let expect_tags: Vec<(String, Vec<String>)> = case
            .expect_tags
            .iter()
            .map(|(key, values)| {
                (
                    (*key).to_string(),
                    values.iter().map(|v| (*v).to_string()).collect(),
                )
            })
            .collect();
        assert_eq!(
            actual_tags, expect_tags,
            "case {:?}: expected tag collections {expect_tags:?}, got {actual_tags:?}",
            case.name
        );
    }
}

/// The same RULE as above, checked as an INVARIANT against a model rather than against
/// hand-written rows: a long, deterministic walk of interleaved ADD/REMOVE requests is
/// applied both to the service and to a `BTreeSet`/`BTreeMap` model, and after EVERY
/// step the read must report exactly what the model holds.
///
/// The walk is generated by fixed arithmetic rather than by a random generator, so the
/// case list is bounded, reproducible and needs nothing on disk; the step index plays
/// the role of the seed and is named in every assertion.
#[tokio::test]
async fn every_step_of_an_add_remove_walk_matches_the_set_model() {
    // Stack names must match `StackName`'s pattern `^[a-zA-Z*]+[a-zA-Z0-9-]*$`
    // (smithy_query(query="shape",shape="StackNames")).
    const STACKS: [&str; 7] = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta",
    ];
    // Tag keys and values are plain text, within `AppBoundaryKey` / `TagValue`.
    const KEYS: [&str; 3] = ["Devops-guru-app", "Devops-guru-rds", "Devops-guru-lambda"];
    const VALUES: [&str; 4] = ["prod", "test", "stage", "dev"];
    const STEPS: usize = 120;

    let emulator = Emulator::new();
    let mut model_stacks: BTreeSet<String> = BTreeSet::new();
    let mut model_tags: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for step in 0..STEPS {
        // Deterministic schedule: two of every five steps remove, the rest add, and
        // the members touched sweep the pools at co-prime strides so adds and removes
        // overlap, miss, and repeat.
        let action = if step % 5 < 2 { "REMOVE" } else { "ADD" };
        let stack_a = STACKS[(step * 3) % STACKS.len()];
        let stack_b = STACKS[(step * 5 + 1) % STACKS.len()];
        let key = KEYS[(step * 2) % KEYS.len()];
        let value_a = VALUES[(step * 3) % VALUES.len()];
        let value_b = VALUES[(step * 7 + 2) % VALUES.len()];

        let collection = json!({
            "CloudFormation": { "StackNames": [stack_a, stack_b] },
            "Tags": [{ "AppBoundaryKey": key, "TagValues": [value_a, value_b] }],
        });
        let reply = emulator.update_resource_collection(action, collection).await;
        assert_eq!(
            reply.status, 200,
            "step {step}: {action} must succeed, got status {} body {}",
            reply.status, reply.raw
        );

        if action == "ADD" {
            model_stacks.insert(stack_a.to_string());
            model_stacks.insert(stack_b.to_string());
            let values = model_tags.entry(key.to_string()).or_default();
            values.insert(value_a.to_string());
            values.insert(value_b.to_string());
        } else {
            model_stacks.remove(stack_a);
            model_stacks.remove(stack_b);
            if let Some(values) = model_tags.get_mut(key) {
                values.remove(value_a);
                values.remove(value_b);
                if values.is_empty() {
                    model_tags.remove(key);
                }
            }
        }

        let expect_stacks: Vec<String> = model_stacks.iter().cloned().collect();
        let actual_stacks = reported_stack_names(
            &emulator
                .get_resource_collection("AWS_CLOUD_FORMATION", None)
                .await,
        );
        assert_eq!(
            actual_stacks, expect_stacks,
            "step {step} ({action}): expected stack names {expect_stacks:?}, got {actual_stacks:?}"
        );

        let expect_tags: Vec<(String, Vec<String>)> = model_tags
            .iter()
            .map(|(key, values)| (key.clone(), values.iter().cloned().collect()))
            .collect();
        let actual_tags =
            reported_tag_collections(&emulator.get_resource_collection("AWS_TAGS", None).await);
        assert_eq!(
            actual_tags, expect_tags,
            "step {step} ({action}): expected tag collections {expect_tags:?}, got {actual_tags:?}"
        );
    }
}

/// RULE: a read of one `ResourceCollectionType` reports only the coverage that type
/// names, and nothing the model does not bind to it.
///
/// The row set is the whole `ResourceCollectionType` catalog — `AWS_CLOUD_FORMATION |
/// AWS_SERVICE | AWS_TAGS`, from
/// `smithy_query(query="shape",shape="ResourceCollectionType")`, mirrored by
/// `RESOURCE_COLLECTION_TYPE_VALUES` in `ops/types/shared_types.rs` — including
/// `AWS_SERVICE`, which the issue report does not cite and which
/// `ResourceCollectionFilter` binds no member for.
#[tokio::test]
async fn each_collection_type_reports_only_its_own_half_of_the_coverage() {
    let emulator = Emulator::new();
    let added = emulator
        .update_resource_collection(
            "ADD",
            json!({
                "CloudFormation": { "StackNames": ["my-stack", "other-stack"] },
                "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": ["prod"] }],
            }),
        )
        .await;
    assert_eq!(added.status, 200, "the ADD must succeed, got {}", added.raw);

    for collection_type in ["AWS_CLOUD_FORMATION", "AWS_SERVICE", "AWS_TAGS"] {
        let reply = emulator.get_resource_collection(collection_type, None).await;
        assert_eq!(
            reply.status, 200,
            "case {collection_type:?}: the read must succeed, got {}",
            reply.raw
        );
        assert!(
            reply.at("/ResourceCollection").is_some(),
            "case {collection_type:?}: expected a ResourceCollection in the reply, got {}",
            reply.raw
        );

        let stacks = reported_stack_names(&reply);
        let tag_collections = reported_tag_collections(&reply);
        match collection_type {
            "AWS_CLOUD_FORMATION" => {
                assert_eq!(
                    stacks,
                    vec!["my-stack".to_string(), "other-stack".to_string()],
                    "case {collection_type:?}: expected the covered stack names, got {stacks:?}"
                );
                assert!(
                    reply.at("/ResourceCollection/Tags").is_none(),
                    "case {collection_type:?}: expected no Tags member, got {}",
                    reply.raw
                );
            }
            "AWS_TAGS" => {
                assert_eq!(
                    tag_collections,
                    vec![("Devops-guru-app".to_string(), vec!["prod".to_string()])],
                    "case {collection_type:?}: expected the covered tag collections, got {tag_collections:?}"
                );
                assert!(
                    reply.at("/ResourceCollection/CloudFormation").is_none(),
                    "case {collection_type:?}: expected no CloudFormation member, got {}",
                    reply.raw
                );
            }
            // `ResourceCollectionFilter` binds no member for service coverage.
            _ => {
                assert!(
                    reply.at("/ResourceCollection/CloudFormation").is_none()
                        && reply.at("/ResourceCollection/Tags").is_none(),
                    "case {collection_type:?}: expected an empty ResourceCollection, got {}",
                    reply.raw
                );
            }
        }
    }
}

/// RULE: a read never changes what a later caller sees — `GetResourceCollection`,
/// `ListNotificationChannels` and `DescribeServiceIntegration` are reporting
/// operations, so repeating them reports the same thing.
#[tokio::test]
async fn repeating_a_read_reports_the_same_state() {
    let emulator = Emulator::new();
    emulator
        .update_resource_collection("ADD", cloud_formation(&["my-stack"]))
        .await;
    emulator
        .add_notification_channel(
            json!({ "Sns": { "TopicArn": "arn:aws:sns:us-east-1:123456789012:guru-alerts" } }),
        )
        .await;

    let reads: [(&str, &str, Option<Value>); 3] = [
        ("GetResourceCollection", "GET", None),
        ("ListNotificationChannels", "POST", Some(json!({}))),
        ("DescribeServiceIntegration", "GET", None),
    ];
    let paths = [
        "/resource-collections/AWS_CLOUD_FORMATION",
        "/channels",
        "/service-integrations",
    ];

    for ((name, method, body), path) in reads.iter().zip(paths) {
        let first = emulator.call(method, path, body.as_ref()).await;
        let second = emulator.call(method, path, body.as_ref()).await;
        let third = emulator.call(method, path, body.as_ref()).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the read must succeed, got {}",
            first.raw
        );
        assert_eq!(
            (first.status, &first.body),
            (second.status, &second.body),
            "case {name:?}: a second read reported something else: {} then {}",
            first.raw,
            second.raw
        );
        assert_eq!(
            (second.status, &second.body),
            (third.status, &third.body),
            "case {name:?}: a third read reported something else: {} then {}",
            second.raw,
            third.raw
        );
    }
}

/// RULE: `AddNotificationChannel` mints a fresh `NotificationChannelId` and stores the
/// submitted `Config` under it, so `ListNotificationChannels` reports that id with
/// exactly the config submitted; the SNS topic identifies the channel, so a second add
/// of the same topic is a `ConflictException`; `RemoveNotificationChannel` deletes it,
/// after which the id names nothing and a repeat delete is a `ResourceNotFoundException`.
///
/// The rows differ in the FORM of `Config.Filters` — absent, one list, the other list,
/// both, and empty lists — which is what the round-trip generalises over.
#[tokio::test]
async fn a_channel_round_trips_its_config_and_its_topic_is_unique() {
    let cases: [(&str, Value); 5] = [
        ("no filters", json!(null)),
        ("severities only", json!({ "Severities": ["HIGH"] })),
        (
            "message types only",
            json!({ "MessageTypes": ["NEW_INSIGHT", "CLOSED_INSIGHT"] }),
        ),
        (
            "both filter lists",
            json!({ "Severities": ["HIGH", "MEDIUM"], "MessageTypes": ["SEVERITY_UPGRADED"] }),
        ),
        (
            "empty filter lists",
            json!({ "Severities": [], "MessageTypes": [] }),
        ),
    ];

    for (name, filters) in cases {
        let emulator = Emulator::new();
        let topic_arn = "arn:aws:sns:us-east-1:123456789012:guru-alerts";
        let mut config = json!({ "Sns": { "TopicArn": topic_arn } });
        if !filters.is_null() {
            config["Filters"] = filters.clone();
        }

        let added = emulator.add_notification_channel(config.clone()).await;
        assert_eq!(
            added.status, 200,
            "case {name:?}: the add must succeed, got {}",
            added.raw
        );
        let id = added
            .at("/Id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modeled_channel_id(&id),
            "case {name:?}: expected a 36-char lowercase-hex channel id, got {id:?}"
        );

        let listed = emulator.list_notification_channels(None).await;
        assert_eq!(
            listed.status, 200,
            "case {name:?}: the list must succeed, got {}",
            listed.raw
        );
        let expected_channels = json!([{ "Id": id, "Config": config }]);
        let actual_channels = listed.at("/Channels").cloned().unwrap_or(Value::Null);
        assert_eq!(
            actual_channels, expected_channels,
            "case {name:?}: expected the submitted channel back, got {actual_channels}"
        );

        // The topic, not the request, is the channel's identity.
        let conflict = emulator.add_notification_channel(config.clone()).await;
        assert_eq!(
            conflict.status, 409,
            "case {name:?}: a second add of the same topic must conflict, got {}",
            conflict.raw
        );
        assert!(
            conflict.names_error("ConflictException"),
            "case {name:?}: expected a ConflictException, got {}",
            conflict.raw
        );

        // An unknown but well-formed id names no channel.
        let unknown = "00000000-0000-4000-8000-000000000000";
        let missing = emulator.remove_notification_channel(unknown).await;
        assert_eq!(
            missing.status, 404,
            "case {name:?}: removing an unknown id must be a 404, got {}",
            missing.raw
        );
        assert!(
            missing.names_error("ResourceNotFoundException"),
            "case {name:?}: expected a ResourceNotFoundException, got {}",
            missing.raw
        );

        let removed = emulator.remove_notification_channel(&id).await;
        assert_eq!(
            removed.status, 200,
            "case {name:?}: the remove must succeed, got {}",
            removed.raw
        );

        let after = emulator.list_notification_channels(None).await;
        assert_eq!(
            after.status, 200,
            "case {name:?}: the list after removal must succeed, got {}",
            after.raw
        );
        let remaining = after.at("/Channels").cloned().unwrap_or(json!([]));
        assert_eq!(
            remaining,
            json!([]),
            "case {name:?}: expected no channels after removal, got {remaining}"
        );

        let again = emulator.remove_notification_channel(&id).await;
        assert_eq!(
            again.status, 404,
            "case {name:?}: a repeat remove must be a 404, got {}",
            again.raw
        );
        assert!(
            again.names_error("ResourceNotFoundException"),
            "case {name:?}: expected a ResourceNotFoundException, got {}",
            again.raw
        );
    }
}

/// RULE: every channel added is listed exactly once, each under its own id, and a
/// paginated read walks the whole collection — the pages concatenated are the whole
/// set, in order, with no repeats and no gaps.
///
/// The row count spans the page boundary: `PAGE_SIZE + 3` channels, so the walk takes
/// more than one page.
#[tokio::test]
async fn listing_walks_every_channel_exactly_once_across_pages() {
    let emulator = Emulator::new();
    let total = PAGE_SIZE + 3;

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for index in 0..total {
        let reply = emulator
            .add_notification_channel(json!({
                "Sns": { "TopicArn": format!("arn:aws:sns:us-east-1:123456789012:guru-topic-{index:04}") }
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "channel {index}: the add must succeed, got {}",
            reply.raw
        );
        let id = reply
            .at("/Id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            minted.insert(id.clone()),
            "channel {index}: id {id:?} was minted twice"
        );
    }

    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let reply = emulator.list_notification_channels(token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "page {pages}: the list must succeed, got {}",
            reply.raw
        );
        let page: Vec<String> = reply
            .at("/Channels")
            .and_then(Value::as_array)
            .map(|channels| {
                channels
                    .iter()
                    .map(|c| {
                        c.get("Id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            page.len() <= PAGE_SIZE,
            "page {pages}: expected at most {PAGE_SIZE} channels, got {}",
            page.len()
        );
        walked.extend(page);
        pages += 1;
        token = reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            pages < 10,
            "page {pages}: the walk did not terminate after {} ids",
            walked.len()
        );
    }

    assert!(
        pages > 1,
        "expected more than one page for {total} channels, got {pages}"
    );
    let expected: Vec<String> = minted.iter().cloned().collect();
    assert_eq!(
        walked, expected,
        "the walk must report every id exactly once in order: expected {} ids, got {:?}",
        expected.len(),
        walked.len()
    );
}

/// RULE: a request a rule refuses is answered with the modelled client error and its
/// status, never with a success and never with a 501.
///
/// The rows differ in WHICH rule is broken: an enum value outside its catalog, a
/// required nested member left out, and four pagination tokens that name no page of
/// the collection being read — one collection's cursor replayed against another, an
/// `AWS_CLOUD_FORMATION` cursor replayed against `AWS_TAGS` and against the channel
/// list (the per-type and per-collection scoping), an offset hand-edited off a page
/// boundary, and a token rewritten outside the modelled lowercase-hex form.
#[tokio::test]
async fn requests_that_break_a_rule_are_refused_with_a_client_error() {
    let emulator = Emulator::new();

    // A `NextToken` this service did issue, but for another collection.
    for index in 0..(PAGE_SIZE + 1) {
        emulator
            .add_notification_channel(json!({
                "Sns": { "TopicArn": format!("arn:aws:sns:us-east-1:123456789012:guru-topic-{index:04}") }
            }))
            .await;
    }
    let channels_token = emulator
        .list_notification_channels(None)
        .await
        .at("/NextToken")
        .and_then(Value::as_str)
        .expect("a second page of channels must offer a NextToken")
        .to_string();

    // A `NextToken` this service issued for ONE ResourceCollectionType, which is what
    // the per-type scoping of a GetResourceCollection cursor has to refuse elsewhere.
    let mut covered = 0;
    while covered < PAGE_SIZE + 1 {
        let batch: Vec<String> = (covered..(covered + 100).min(PAGE_SIZE + 1))
            .map(|index| format!("stack-{index:05}"))
            .collect();
        covered += batch.len();
        emulator
            .update_resource_collection("ADD", json!({ "CloudFormation": { "StackNames": batch } }))
            .await;
    }
    let cloud_formation_token = emulator
        .get_resource_collection("AWS_CLOUD_FORMATION", None)
        .await
        .at("/NextToken")
        .and_then(Value::as_str)
        .expect("a second page of stack names must offer a NextToken")
        .to_string();

    let cases: Vec<(&str, Reply)> = vec![
        (
            "Action outside UpdateResourceCollectionAction",
            emulator
                .update_resource_collection("DELETE", cloud_formation(&["my-stack"]))
                .await,
        ),
        (
            "Config.Sns with no TopicArn",
            emulator.add_notification_channel(json!({ "Sns": {} })).await,
        ),
        (
            "a channels NextToken replayed against GetResourceCollection",
            emulator
                .get_resource_collection("AWS_CLOUD_FORMATION", Some(&channels_token))
                .await,
        ),
        (
            "an AWS_CLOUD_FORMATION NextToken replayed against AWS_TAGS",
            emulator
                .get_resource_collection("AWS_TAGS", Some(&cloud_formation_token))
                .await,
        ),
        (
            "an AWS_CLOUD_FORMATION NextToken replayed against ListNotificationChannels",
            emulator
                .list_notification_channels(Some(&cloud_formation_token))
                .await,
        ),
        (
            "a NextToken whose offset was hand-edited off a page boundary",
            {
                // The digest binds the token to its collection but is not a secret, so
                // the offset group is the part a caller can plausibly edit.
                let edited = format!("00000005{}", &channels_token[8..]);
                emulator.list_notification_channels(Some(&edited)).await
            },
        ),
        (
            "a NextToken rewritten in uppercase hex",
            emulator
                .list_notification_channels(Some(&channels_token.to_uppercase()))
                .await,
        ),
    ];

    for (name, reply) in cases {
        assert_eq!(
            reply.status, 400,
            "case {name:?}: expected a 400 refusal, got status {} body {}",
            reply.status, reply.raw
        );
        assert!(
            reply.names_error("ValidationException"),
            "case {name:?}: expected a ValidationException, got {}",
            reply.raw
        );
    }
}

/// RULE: `DescribeServiceIntegration` reports the persisted `ServiceIntegrationConfig`
/// — opted out of every integration before the first update — and
/// `UpdateServiceIntegration` replaces only the members its request supplies, leaving
/// the rest as stored.
///
/// The rows are applied in order to one account, and each names the WHOLE expected
/// config, so a row that touches one integration also pins that the others survived.
/// They differ in which members are supplied, including the KMS integration's three.
#[tokio::test]
async fn service_integration_defaults_to_opted_out_and_updates_only_what_it_names() {
    let emulator = Emulator::new();
    let disabled_kms = json!({ "OptInStatus": "DISABLED" });

    let steps: Vec<(&str, Option<Value>, Value)> = vec![
        (
            "before any update",
            None,
            json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": disabled_kms.clone(),
            }),
        ),
        (
            "OpsCenter opted in",
            Some(json!({ "OpsCenter": { "OptInStatus": "ENABLED" } })),
            json!({
                "OpsCenter": { "OptInStatus": "ENABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "DISABLED" },
                "KMSServerSideEncryption": disabled_kms.clone(),
            }),
        ),
        (
            "log anomaly detection opted in, OpsCenter untouched",
            Some(json!({ "LogsAnomalyDetection": { "OptInStatus": "ENABLED" } })),
            json!({
                "OpsCenter": { "OptInStatus": "ENABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "ENABLED" },
                "KMSServerSideEncryption": disabled_kms.clone(),
            }),
        ),
        (
            "a customer-managed key, the other two untouched",
            Some(json!({
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": "alias/devops-guru"
                }
            })),
            json!({
                "OpsCenter": { "OptInStatus": "ENABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "ENABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": "alias/devops-guru"
                },
            }),
        ),
        (
            "OpsCenter opted back out, the key kept",
            Some(json!({ "OpsCenter": { "OptInStatus": "DISABLED" } })),
            json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "ENABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": "alias/devops-guru"
                },
            }),
        ),
        (
            "an update carrying no member changes nothing",
            Some(json!({})),
            json!({
                "OpsCenter": { "OptInStatus": "DISABLED" },
                "LogsAnomalyDetection": { "OptInStatus": "ENABLED" },
                "KMSServerSideEncryption": {
                    "OptInStatus": "ENABLED",
                    "Type": "CUSTOMER_MANAGED_KEY",
                    "KMSKeyId": "alias/devops-guru"
                },
            }),
        ),
    ];

    for (name, update, expected) in steps {
        if let Some(update) = update {
            let reply = emulator.update_service_integration(update).await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}: the update must succeed, got {}",
                reply.raw
            );
        }
        let described = emulator.describe_service_integration().await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: the describe must succeed, got {}",
            described.raw
        );
        let actual = described
            .at("/ServiceIntegration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            actual, expected,
            "case {name:?}: expected service integration {expected}, got {actual}"
        );
    }
}

/// RULE: every caller-driven store carries a maximum entry count, and an `ADD` that
/// would take it past that cap is refused with the operation's modelled client error
/// and stores NOTHING — the store is left exactly as it was, not half-applied.
///
/// The rows are the three coverage caps in `crate::state`
/// (`MAX_COVERED_STACK_NAMES`, `MAX_COVERED_TAG_KEYS`, `MAX_TAG_VALUES_PER_KEY`), which
/// differ in which collection grows and in whether one request or a stream of them
/// reaches the cap. Each row fills to exactly the cap, checks that succeeded, then
/// submits one more member.
#[tokio::test]
async fn an_add_past_a_coverage_cap_is_refused_and_stores_nothing() {
    // `UpdateStackNames` and `UpdateTagValues` are both capped at 100 entries per
    // request (smithy_query), so filling a larger cap takes several requests.
    const PER_REQUEST: usize = 100;

    for kind in ["stack names", "app boundary keys", "tag values per key"] {
        let emulator = Emulator::new();

        match kind {
            "stack names" => {
                let mut filled = 0;
                while filled < MAX_COVERED_STACK_NAMES {
                    let batch: Vec<String> = (filled
                        ..(filled + PER_REQUEST).min(MAX_COVERED_STACK_NAMES))
                        .map(|index| format!("stack-{index:05}"))
                        .collect();
                    let reply = emulator
                        .update_resource_collection(
                            "ADD",
                            json!({ "CloudFormation": { "StackNames": batch } }),
                        )
                        .await;
                    assert_eq!(
                        reply.status, 200,
                        "case {kind:?}: filling to the cap must succeed at {filled}, got {}",
                        reply.raw
                    );
                    filled += batch.len();
                }
                let at_cap = emulator.all_stack_names().await;
                assert_eq!(
                    at_cap.len(),
                    MAX_COVERED_STACK_NAMES,
                    "case {kind:?}: expected {MAX_COVERED_STACK_NAMES} covered names, got {}",
                    at_cap.len()
                );

                let over = emulator
                    .update_resource_collection(
                        "ADD",
                        json!({ "CloudFormation": { "StackNames": ["one-stack-too-many"] } }),
                    )
                    .await;
                assert_eq!(
                    over.status, 400,
                    "case {kind:?}: the over-cap add must be refused, got status {} body {}",
                    over.status, over.raw
                );
                assert!(
                    over.names_error("ValidationException"),
                    "case {kind:?}: expected a ValidationException, got {}",
                    over.raw
                );
                let after = emulator.all_stack_names().await;
                assert_eq!(
                    after, at_cap,
                    "case {kind:?}: the refused add changed the store: {} names before, {} after",
                    at_cap.len(),
                    after.len()
                );
            }
            "app boundary keys" => {
                let batch: Vec<Value> = (0..MAX_COVERED_TAG_KEYS)
                    .map(|index| {
                        json!({
                            "AppBoundaryKey": format!("Devops-guru-key-{index:05}"),
                            "TagValues": ["prod"],
                        })
                    })
                    .collect();
                let reply = emulator
                    .update_resource_collection("ADD", json!({ "Tags": batch }))
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {kind:?}: filling to the cap must succeed, got {}",
                    reply.raw
                );
                let at_cap = emulator.all_tag_collections().await;
                assert_eq!(
                    at_cap.len(),
                    MAX_COVERED_TAG_KEYS,
                    "case {kind:?}: expected {MAX_COVERED_TAG_KEYS} covered keys, got {}",
                    at_cap.len()
                );

                let over = emulator
                    .update_resource_collection(
                        "ADD",
                        json!({ "Tags": [{
                            "AppBoundaryKey": "Devops-guru-one-key-too-many",
                            "TagValues": ["prod"],
                        }] }),
                    )
                    .await;
                assert_eq!(
                    over.status, 400,
                    "case {kind:?}: the over-cap add must be refused, got status {} body {}",
                    over.status, over.raw
                );
                assert!(
                    over.names_error("ValidationException"),
                    "case {kind:?}: expected a ValidationException, got {}",
                    over.raw
                );
                let after = emulator.all_tag_collections().await;
                assert_eq!(
                    after, at_cap,
                    "case {kind:?}: the refused add changed the store: {} keys before, {} after",
                    at_cap.len(),
                    after.len()
                );
            }
            _ => {
                let key = "Devops-guru-app";
                let mut filled = 0;
                while filled < MAX_TAG_VALUES_PER_KEY {
                    let batch: Vec<String> = (filled
                        ..(filled + PER_REQUEST).min(MAX_TAG_VALUES_PER_KEY))
                        .map(|index| format!("value-{index:05}"))
                        .collect();
                    let reply = emulator
                        .update_resource_collection(
                            "ADD",
                            json!({ "Tags": [{ "AppBoundaryKey": key, "TagValues": batch }] }),
                        )
                        .await;
                    assert_eq!(
                        reply.status, 200,
                        "case {kind:?}: filling to the cap must succeed at {filled}, got {}",
                        reply.raw
                    );
                    filled += batch.len();
                }
                let at_cap = emulator.all_tag_collections().await;
                assert_eq!(
                    at_cap.first().map(|(_, values)| values.len()),
                    Some(MAX_TAG_VALUES_PER_KEY),
                    "case {kind:?}: expected {MAX_TAG_VALUES_PER_KEY} values under {key:?}, got {at_cap:?}"
                );

                let over = emulator
                    .update_resource_collection(
                        "ADD",
                        json!({ "Tags": [{
                            "AppBoundaryKey": key,
                            "TagValues": ["one-value-too-many"],
                        }] }),
                    )
                    .await;
                assert_eq!(
                    over.status, 400,
                    "case {kind:?}: the over-cap add must be refused, got status {} body {}",
                    over.status, over.raw
                );
                assert!(
                    over.names_error("ValidationException"),
                    "case {kind:?}: expected a ValidationException, got {}",
                    over.raw
                );
                let after = emulator.all_tag_collections().await;
                assert_eq!(
                    after, at_cap,
                    "case {kind:?}: the refused add changed the store: {at_cap:?} before, {after:?} after"
                );
            }
        }
    }
}

/// RULE (same bound, the other store): past `MAX_NOTIFICATION_CHANNELS` a further
/// `AddNotificationChannel` is refused with the `ServiceQuotaExceededException` (HTTP
/// 402) the model declares for it, and no channel is stored.
#[tokio::test]
async fn adding_a_channel_past_the_cap_is_a_service_quota_exceeded() {
    let emulator = Emulator::new();

    for index in 0..MAX_NOTIFICATION_CHANNELS {
        let reply = emulator
            .add_notification_channel(json!({
                "Sns": { "TopicArn": format!("arn:aws:sns:us-east-1:123456789012:guru-topic-{index:05}") }
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "channel {index}: filling to the cap must succeed, got {}",
            reply.raw
        );
    }

    let over = emulator
        .add_notification_channel(json!({
            "Sns": { "TopicArn": "arn:aws:sns:us-east-1:123456789012:guru-one-topic-too-many" }
        }))
        .await;
    assert_eq!(
        over.status, 402,
        "the over-cap add must be refused with 402, got status {} body {}",
        over.status, over.raw
    );
    assert!(
        over.names_error("ServiceQuotaExceededException"),
        "expected a ServiceQuotaExceededException, got {}",
        over.raw
    );
    assert!(
        over.at("/Id").is_none(),
        "the refused add must mint no id, got {}",
        over.raw
    );

    // And the refused add stored nothing: the walk still finds exactly the cap.
    let mut walked = 0usize;
    let mut token: Option<String> = None;
    loop {
        let reply = emulator.list_notification_channels(token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "the list must succeed, got {}",
            reply.raw
        );
        walked += reply
            .at("/Channels")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        token = reply
            .at("/NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, MAX_NOTIFICATION_CHANNELS,
        "expected exactly {MAX_NOTIFICATION_CHANNELS} stored channels, got {walked}"
    );
}
