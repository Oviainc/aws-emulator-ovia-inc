// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of Rekognition's resource tagging over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: RekognitionService.<Operation>`), dispatched through the
//! same [`MultiServiceRouter`] the production emulator mounts.
//!
//! Every assertion goes over HTTP and reads only the status, the wire error code and the
//! JSON body: nothing reaches into the service's state struct, so a response that is not
//! computed from persisted state cannot pass.
//!
//! THE RULE UNDER TEST: a resource's tags are one set stored ON that resource, reached by
//! any ARN that names it — `TagResource` adds the keys it carries and overwrites the ones
//! the resource already has, `UntagResource` drops only the keys it names, and
//! `ListTagsForResource` reports exactly what is left — for every kind of resource this
//! store holds, with one resource's tags never visible on another's.
//!
//! The rows below differ along what the rule generalises over: the KIND of resource whose
//! ARN is tagged (a collection, created by `CreateCollection`; a project, created by
//! `CreateProject`, whose `Tags` member attaches tags at create time), whether its create
//! attached tags at all, which form of its ARN the request names it by (with and without
//! the leading `arn:` label, since `CreateCollection` reports the label-less one), and
//! what one request changes (several new keys, a key already carried, none at all, one of
//! several removed, a key removed twice, a key never set, every key removed). The
//! `env`/`owner` pair of the tag/untag round trip is one row of that table.
//!
//! The collection suite owns the router harness, the dispatch helpers and the reduced
//! reply, so nothing here builds a second one.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_rekognition_rekognitionservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::face_collections::{call, expected_arn, harness, label_less};

/// The region every call here is signed in, and so the region of every ARN minted.
const REGION: &str = "us-east-1";

/// One taggable resource, as this suite holds it: a label for failure messages, a marker
/// that makes its tag VALUES its own (so one resource's tags cannot pass for another's),
/// every ARN form that names it, and the tag set it must be carrying.
struct TaggedResource {
    case: &'static str,
    marker: &'static str,
    arns: Vec<String>,
    expected: BTreeMap<String, String>,
}

impl TaggedResource {
    /// This resource's value for the tag value `base` names: one resource's tag values are
    /// its own, so a value that leaked from another resource is a failure rather than a
    /// match.
    fn value(&self, base: &str) -> String {
        format!("{base}-{}", self.marker)
    }

    /// The ARN form step `step` names this resource by: every form it is reachable
    /// through, in turn.
    fn arn(&self, step: usize) -> &str {
        &self.arns[step % self.arns.len()]
    }
}

/// What one step of the round trip does to a resource.
enum Change {
    /// `TagResource` with these key/value-base pairs.
    Tag(&'static [(&'static str, &'static str)]),
    /// `UntagResource` with these keys.
    Untag(&'static [&'static str]),
}

/// One step of the round trip: what it changes, run against every resource in turn.
struct Step {
    case: &'static str,
    change: Change,
}

/// A tag map as it comes back on the wire.
fn as_json(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The tags `ListTagsForResource` reports for the resource `arn` names.
async fn tags_of(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let reply = call(router, "ListTagsForResource", json!({ "ResourceArn": arn })).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: {arn} must resolve, got {:?}",
        reply.body
    );
    reply.field("Tags").cloned().unwrap_or(json!({}))
}

/// `CreateCollection` for `collection_id` with `tags` attached, as a resource of the
/// table: its ARN as the store holds it, and the label-less form its own answer reports.
async fn collection_resource(
    router: &MultiServiceRouter,
    case: &'static str,
    marker: &'static str,
    collection_id: &str,
    tags: &[(&str, &str)],
) -> TaggedResource {
    let expected: BTreeMap<String, String> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), format!("{value}-{marker}")))
        .collect();
    let mut body = json!({ "CollectionId": collection_id });
    body["Tags"] = as_json(&expected);
    let reply = call(router, "CreateCollection", body).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: creating the collection must succeed, got {:?}",
        reply.body
    );
    let stored = expected_arn(REGION, collection_id);
    assert_eq!(
        reply.string("CollectionArn"),
        Some(label_less(&stored)),
        "case {case:?}: the create must report the ARN a caller then tags by, got {:?}",
        reply.body
    );
    TaggedResource {
        case,
        marker,
        arns: vec![stored.clone(), label_less(&stored)],
        expected,
    }
}

/// `CreateProject` for `project_name` with `tags` attached, as a resource of the table:
/// the ARN the service minted for it, and the same ARN without its label.
async fn project_resource(
    router: &MultiServiceRouter,
    case: &'static str,
    marker: &'static str,
    project_name: &str,
    tags: &[(&str, &str)],
) -> TaggedResource {
    let expected: BTreeMap<String, String> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), format!("{value}-{marker}")))
        .collect();
    let mut body = json!({ "ProjectName": project_name });
    body["Tags"] = as_json(&expected);
    let reply = call(router, "CreateProject", body).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: creating the project must succeed, got {:?}",
        reply.body
    );
    let arn = reply.string("ProjectArn").unwrap_or_else(|| {
        panic!(
            "case {case:?}: a ProjectArn must come back, got {:?}",
            reply.body
        )
    });
    TaggedResource {
        case,
        marker,
        arns: vec![arn.clone(), label_less(&arn)],
        expected,
    }
}

/// Every resource reports its own tag set, through every ARN form that names it.
async fn assert_every_resource(
    router: &MultiServiceRouter,
    step: &str,
    resources: &[TaggedResource],
) {
    for resource in resources {
        for arn in &resource.arns {
            let reported = tags_of(router, resource.case, arn).await;
            assert_eq!(
                reported,
                as_json(&resource.expected),
                "case {step:?}/{}: {arn} must report the tags the resource carries, got {reported:?}",
                resource.case
            );
        }
    }
}

#[tokio::test]
async fn a_resources_tags_are_its_own_set_whatever_kind_of_resource_it_is() {
    let router = harness();
    let mut resources = vec![
        collection_resource(
            &router,
            "a collection created with tags",
            "c1",
            "tagged-collection",
            &[("env", "sim2real"), ("owner", "behavioral")],
        )
        .await,
        collection_resource(
            &router,
            "a collection created with no tags",
            "c2",
            "bare-collection",
            &[],
        )
        .await,
        project_resource(
            &router,
            "a project created with tags",
            "p1",
            "tagged-project",
            &[("team", "vision")],
        )
        .await,
        project_resource(
            &router,
            "a project created with no tags",
            "p2",
            "bare-project",
            &[],
        )
        .await,
    ];

    // What each create attached is the set the round trip starts from.
    assert_every_resource(&router, "the tags the create attached", &resources).await;

    let steps = [
        Step {
            case: "two new keys",
            change: Change::Tag(&[("env", "sim2real"), ("owner", "behavioral")]),
        },
        Step {
            case: "a key the resource already carries",
            change: Change::Tag(&[("env", "prod")]),
        },
        Step {
            case: "no tags at all",
            change: Change::Tag(&[]),
        },
        Step {
            case: "one of several keys removed",
            change: Change::Untag(&["owner"]),
        },
        Step {
            case: "the same key again, beside one never set",
            change: Change::Untag(&["owner", "never-set"]),
        },
        Step {
            case: "no keys at all",
            change: Change::Untag(&[]),
        },
        Step {
            case: "every key the step has added",
            change: Change::Untag(&["env", "owner"]),
        },
        Step {
            case: "a key added back after the set was emptied",
            change: Change::Tag(&[("env", "test")]),
        },
    ];

    for (index, Step { case, change }) in steps.iter().enumerate() {
        for resource in resources.iter_mut() {
            let arn = resource.arn(index).to_string();
            let (operation, mut body) = match change {
                Change::Tag(pairs) => {
                    let tags: BTreeMap<String, String> = pairs
                        .iter()
                        .map(|(key, base)| ((*key).to_string(), resource.value(base)))
                        .collect();
                    ("TagResource", json!({ "Tags": as_json(&tags) }))
                }
                Change::Untag(keys) => ("UntagResource", json!({ "TagKeys": keys })),
            };
            body["ResourceArn"] = json!(arn);
            let reply = call(&router, operation, body).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}/{}: {operation} on {arn} must succeed, got {:?}",
                resource.case, reply.body
            );
            // The same change, applied to the test's own model of the set.
            match change {
                Change::Tag(pairs) => {
                    for (key, base) in pairs.iter() {
                        let value = resource.value(base);
                        resource.expected.insert((*key).to_string(), value);
                    }
                }
                Change::Untag(keys) => {
                    for key in keys.iter() {
                        resource.expected.remove(*key);
                    }
                }
            }
        }
        assert_every_resource(&router, case, &resources).await;
    }

    // A tagged project is still the project `DescribeProjects` reports: tagging changes a
    // resource's tags and nothing else about it.
    let described = call(
        &router,
        "DescribeProjects",
        json!({ "ProjectNames": ["tagged-project"] }),
    )
    .await;
    let descriptions = described.list("ProjectDescriptions");
    assert_eq!(
        descriptions.len(),
        1,
        "a tagged project must still be described, got {:?}",
        described.body
    );
    assert_eq!(
        descriptions[0].get("ProjectArn").and_then(Value::as_str),
        Some(resources[2].arns[0].as_str()),
        "the described project must carry the ARN its create minted, got {:?}",
        described.body
    );
}

#[tokio::test]
async fn an_arn_naming_no_live_resource_is_not_found_by_any_tagging_operation() {
    let router = harness();
    // One live resource of each kind, so a miss is a miss on the ARN and not an empty
    // store.
    collection_resource(
        &router,
        "a live collection",
        "c1",
        "live",
        &[("env", "test")],
    )
    .await;
    let project = project_resource(
        &router,
        "a live project",
        "p1",
        "live-project",
        &[("env", "test")],
    )
    .await;

    // Rows: a collection ARN of a collection never created, the same without its label, a
    // project ARN whose name no project carries, and a stream-processor ARN — a resource
    // kind Rekognition tags that this store does not hold, so it is unknown like any
    // other ARN.
    let stray_collection = expected_arn(REGION, "never-created");
    let stray_project = project.arns[0].replace("live-project", "never-created");
    let stream_processor = format!(
        "arn:aws:rekognition:{REGION}:{}:streamprocessor/never-created",
        engine::emulator_account_id()
    );
    for arn in [
        stray_collection.clone(),
        label_less(&stray_collection),
        stray_project,
        stream_processor,
    ] {
        for (operation, body) in [
            ("ListTagsForResource", json!({})),
            ("TagResource", json!({ "Tags": { "env": "test" } })),
            ("UntagResource", json!({ "TagKeys": ["env"] })),
        ] {
            let mut body = body;
            body["ResourceArn"] = json!(arn);
            let reply = call(&router, operation, body).await;
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ResourceNotFoundException"),
                "case {operation} on {arn}: an ARN naming no live resource must not be \
                 found, got {:?}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_resource_carries_at_most_the_published_number_of_tags() {
    let router = harness();
    let resources = vec![
        collection_resource(&router, "a collection at capacity", "c1", "full", &[]).await,
        project_resource(&router, "a project at capacity", "p1", "full-project", &[]).await,
    ];

    for resource in &resources {
        let case = resource.case;
        let arn = &resource.arns[0];
        // The top of the modelled `TagMap` length range, which is also the number of tags
        // AWS allows on a resource: `state::MAX_TAGS_PER_RESOURCE`.
        let at_capacity: BTreeMap<String, String> = (0..state::MAX_TAGS_PER_RESOURCE)
            .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
            .collect();
        let filled = call(
            &router,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": as_json(&at_capacity) }),
        )
        .await;
        assert_eq!(
            filled.status, 200,
            "case {case:?}: {} tags must be accepted, got {:?}",
            state::MAX_TAGS_PER_RESOURCE, filled.body
        );
        assert_eq!(
            tags_of(&router, case, arn).await,
            as_json(&at_capacity),
            "case {case:?}: a resource at capacity reports every tag it was given"
        );

        // One key past the limit is refused, and the resource's tags are left as they
        // were.
        let over = call(
            &router,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": { "one-too-many": "value" } }),
        )
        .await;
        assert_eq!(
            (over.status, over.error_code().as_str()),
            (400, "ServiceQuotaExceededException"),
            "case {case:?}: a tag past the limit must be refused, got {:?}",
            over.body
        );
        assert_eq!(
            tags_of(&router, case, arn).await,
            as_json(&at_capacity),
            "case {case:?}: a refused request must leave the tags it did not write"
        );

        // A key the resource already carries is an update, not growth, so it is accepted
        // at capacity.
        let mut updated = at_capacity.clone();
        updated.insert("key-000".to_string(), "updated".to_string());
        let update = call(
            &router,
            "TagResource",
            json!({ "ResourceArn": arn, "Tags": { "key-000": "updated" } }),
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: updating a key at capacity must succeed, got {:?}",
            update.body
        );
        assert_eq!(
            tags_of(&router, case, arn).await,
            as_json(&updated),
            "case {case:?}: the updated key must take its new value"
        );
    }
}
