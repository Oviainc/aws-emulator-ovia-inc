// SPDX-License-Identifier: CC0-1.0

//! What `RunInstances` does with a `TagSpecifications` entry — end to end over the wire
//! the service speaks (`ec2Query`, through the shared [`super::ec2_query`] harness).
//!
//! The rule under test: "the tags to apply to the resources that are created during
//! instance launch" (`RunInstancesRequest$TagSpecifications`) are applied PER RESOURCE
//! TYPE. A specification naming a type the operation documents is served — the launch is
//! not refused for naming one — and its tags land on the resources of that type the launch
//! creates, so the instance carries exactly the `instance` entries and never another
//! type's. A type the operation does not document is refused with EC2's own code, and a
//! refused launch stores nothing.
//!
//! Each row is a distinct input along what the rule generalises over: which documented
//! resource type a specification names, how many specifications one launch carries, which
//! order they are in, how many instances the launch mints, and which undocumented type is
//! refused. No row is the specification.

use std::collections::BTreeMap;

use ec2::state::RUN_INSTANCES_TAGGABLE_RESOURCES;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, elements, error_code};

/// The Region every call here is signed for. An instance's tags are per account, so the
/// Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The image every launch here names, in the long id form this service accepts.
const AMI: &str = "ami-0123456789abcdef0";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// The form fields of one `TagSpecifications` entry at `index` (counting from 1).
fn specification(index: usize, resource_type: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = vec![p(
        &format!("TagSpecification.{index}.ResourceType"),
        resource_type,
    )];
    for (position, (key, value)) in tags.iter().enumerate() {
        let tag = position + 1;
        fields.push(p(&format!("TagSpecification.{index}.Tag.{tag}.Key"), *key));
        fields.push(p(&format!("TagSpecification.{index}.Tag.{tag}.Value"), *value));
    }
    fields
}

/// Launch `count` instances with `extra` parameters, as the HTTP status and body.
async fn launch(
    router: &MultiServiceRouter,
    count: i32,
    extra: Vec<(String, String)>,
) -> (u16, String) {
    let mut params = vec![
        p("ImageId", AMI),
        p("MinCount", count.to_string()),
        p("MaxCount", count.to_string()),
    ];
    params.extend(extra);
    call(router, REGION, "RunInstances", &params).await
}

/// The tags `DescribeInstances` reports for `instance_id`, as key -> value pairs.
async fn reported_tags(router: &MultiServiceRouter, case: &str, instance_id: &str) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading {instance_id} back must succeed, got {status} with body {body}"
    );
    let document = roxmltree::Document::parse(&body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body must be XML ({error}): {body}"));
    document
        .descendants()
        .filter(|node| node.has_tag_name("item"))
        .filter(|node| {
            node.children().any(|child| child.has_tag_name("key"))
                && node.children().any(|child| child.has_tag_name("value"))
        })
        .map(|node| {
            let text = |name: &str| {
                node.children()
                    .find(|child| child.has_tag_name(name))
                    .and_then(|child| child.text())
                    .unwrap_or_default()
                    .to_string()
            };
            (text("key"), text("value"))
        })
        .collect()
}

/// A specification for any resource type the operation documents is SERVED, and only the
/// `instance` entries reach the instance.
#[tokio::test]
async fn a_documented_resource_type_is_served_and_only_instance_tags_reach_the_instance() {
    // Rows: every documented resource type on its own, as the operation's own catalogue
    // lists them — so a type added there is covered here without editing this table.
    for resource_type in RUN_INSTANCES_TAGGABLE_RESOURCES {
        let router = emulator();
        let case = format!("TagSpecification for {resource_type}");
        let (status, body) = launch(
            &router,
            1,
            specification(1, resource_type, &[("purpose", "tagged-on-launch")]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let instances = elements(&body, "instanceId");
        let [instance_id] = instances.as_slice() else {
            panic!("case {case:?}: expected exactly one launched instance, got {instances:?}");
        };

        let tags = reported_tags(&router, &case, instance_id).await;
        let expected: BTreeMap<String, String> = if *resource_type == "instance" {
            [("purpose".to_string(), "tagged-on-launch".to_string())]
                .into_iter()
                .collect()
        } else {
            // The launch mints no resource of that type here, so its tags are carried by
            // nothing — and must not have landed on the instance.
            BTreeMap::new()
        };
        assert_eq!(
            tags, expected,
            "case {case:?}: expected the instance to carry {expected:?}, got {tags:?}"
        );
    }
}

/// One row of the table below: a launch and the tags its instances must end up with.
struct Row {
    /// What makes this row different from the others.
    case: &'static str,
    /// How many instances the launch mints.
    count: i32,
    /// The `TagSpecifications` fields the launch carries.
    params: Vec<(String, String)>,
    /// The tags every launched instance must report back.
    expected: BTreeMap<String, String>,
}

/// Several specifications in one launch: each type's tags stay with its own type, whatever
/// order they arrive in and however many instances the launch mints.
#[tokio::test]
async fn several_specifications_keep_each_types_tags_apart() {
    // Rows: the instance entry first and second, one instance and three, and a launch whose
    // ONLY specification is another type.
    let rows = vec![
        Row {
            case: "instance before volume",
            count: 1,
            params: [
                specification(1, "instance", &[("Name", "first")]),
                specification(2, "volume", &[("Name", "the-root-volume")]),
            ]
            .concat(),
            expected: [("Name".to_string(), "first".to_string())]
                .into_iter()
                .collect(),
        },
        Row {
            case: "volume before instance",
            count: 3,
            params: [
                specification(1, "volume", &[("Name", "the-root-volume")]),
                specification(2, "instance", &[("Name", "second"), ("env", "test")]),
            ]
            .concat(),
            expected: [
                ("Name".to_string(), "second".to_string()),
                ("env".to_string(), "test".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        Row {
            case: "network interface and Spot request only",
            count: 2,
            params: [
                specification(1, "network-interface", &[("Name", "the-primary-eni")]),
                specification(2, "spot-instances-request", &[("Name", "no-such-request")]),
            ]
            .concat(),
            expected: BTreeMap::new(),
        },
    ];

    for Row {
        case,
        count,
        params,
        expected,
    } in rows
    {
        let router = emulator();
        let (status, body) = launch(&router, count, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let launched = elements(&body, "instanceId");
        assert_eq!(
            launched.len(),
            usize::try_from(count).unwrap_or_default(),
            "case {case:?}: expected {count} launched instances, got {launched:?}"
        );
        for instance_id in &launched {
            let tags = reported_tags(&router, case, instance_id).await;
            assert_eq!(
                tags, expected,
                "case {case:?}: expected {instance_id} to carry {expected:?}, got {tags:?}"
            );
        }
    }
}

/// A resource type the operation does not document is refused, and the refused launch
/// stores nothing.
///
/// Two refusals meet here and both are HTTP 400: a value the modelled `ResourceType` enum
/// does not carry is refused by the boundary the model drives (which owns that code), and a
/// value the enum DOES carry but this operation does not document is refused by the
/// operation, with EC2's own `InvalidParameterValue`.
#[tokio::test]
async fn an_undocumented_resource_type_is_refused_and_launches_nothing() {
    // Rows: four resource kinds the modelled enum carries and this operation does not
    // document, then three values outside the enum altogether (a kind of no EC2 resource,
    // the member-cased spelling of a documented type, and the empty string).
    let documented_by_the_enum = ["vpc", "elastic-ip", "image", "subnet"];
    let outside_the_enum = ["not-a-resource-type", "Instance", ""];

    for resource_type in documented_by_the_enum {
        let router = emulator();
        let case = format!("TagSpecification for {resource_type:?}");
        let (status, body) = launch(
            &router,
            1,
            specification(1, resource_type, &[("purpose", "refused")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_nothing_launched(&router, &case).await;
    }

    for resource_type in outside_the_enum {
        let router = emulator();
        let case = format!("TagSpecification for {resource_type:?}");
        let (status, body) = launch(
            &router,
            1,
            specification(1, resource_type, &[("purpose", "refused")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_nothing_launched(&router, &case).await;
    }
}

/// A refused launch must leave the store without an instance.
async fn assert_nothing_launched(router: &MultiServiceRouter, case: &str) {
    let (status, body) = call(router, REGION, "DescribeInstances", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: reading the store back must succeed, got {status} with body {body}"
    );
    let stored = elements(&body, "instanceId");
    assert!(
        stored.is_empty(),
        "case {case:?}: a refused launch must store nothing, got {stored:?}"
    );
}
