// SPDX-License-Identifier: CC0-1.0

//! `DeleteAttributes`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: `DeleteAttributes` removes from the resource each entry names —
//! by `targetId`, inside the cluster the request names (or the default cluster when it
//! names none) — the attribute that entry names by `name`, reports the attributes it
//! actually removed against the full ARN of the resource they were on, and refuses, with
//! nothing removed, a request whose cluster is not there (`ClusterNotFoundException`) or
//! whose entry names a resource that cluster does not hold
//! (`TargetNotFoundException`).
//!
//! The docs that rule comes from:
//!
//! * `DeleteAttributes`: "Deletes one or more custom attributes from an Amazon ECS
//!   resource."
//! * `DeleteAttributes$attributes`: "The attributes to delete from your resource. You can
//!   specify up to 10 attributes for each request. For custom attributes, specify the
//!   attribute name and target ID, but don't specify the value."
//! * `DeleteAttributes$cluster`: "If you do not specify a cluster, the default cluster is
//!   assumed."
//! * `DeleteAttributesResponse$attributes`: "A list of attribute objects that were
//!   successfully deleted from your resource."
//! * `Attribute$targetId`: "The ID of the target. You can specify the short form ID for a
//!   resource or the full Amazon Resource Name (ARN)."
//! * `Attribute$targetType`: "This parameter is required if you use the short form ID for
//!   a resource instead of the full ARN."
//! * `TargetNotFoundException`: "The specified target wasn't found."
//! * `ContainerInstance$version`: "Every time a container instance experiences a change
//!   ... the version counter is incremented."
//! * `DeregisterContainerInstance`: "Deregistering a container instance removes the
//!   instance from a cluster."

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, count, list, ok, refusal, refused, service, ACCOUNT, SIGNED_REGION,
};
use crate::list_attributes::{list_attributes, register_with_attributes, Listed};

/// The cluster every case works in.
const CLUSTER: &str = "demo";

/// The EC2 instance the fixture registers its container instance for.
const INSTANCE_ID: &str = "i-0aaa0000000000001";

/// The sentence a `TargetNotFoundException` carries, whichever resource the entry named:
/// the condition is a property of the request, so the text is the same for every entry
/// that meets it.
const TARGET_NOT_FOUND: &str = "The specified target wasn't found.";

/// One row of the deletion table: the case name, the entries it sends (built from the
/// target's full ARN and its short form id) and the names of the attributes the call must
/// report as deleted.
type DeletionRow = (&'static str, fn(&str, &str) -> Value, &'static [&'static str]);

/// One row of the refusal table: the case name, the ONE `attributes` entry it sends, and
/// the error code and sentence it must be refused with.
type RefusalRow = (&'static str, Value, &'static str, &'static str);

/// The attributes a fresh fixture stores against its container instance. The rows differ
/// along what the rule generalises over: a plain name, a dotted name, a name carrying a
/// slash, an attribute stored with NO value, and a value carrying punctuation.
fn seeded() -> Value {
    json!([
        { "name": "stack", "value": "production" },
        { "name": "ecs.os-type", "value": "linux" },
        { "name": "fleet.id/west", "value": "a-1" },
        { "name": "ecs.capability.external" },
        { "name": "tier", "value": "gpu@1" },
    ])
}

/// The name and value of every attribute [`seeded`] stores, an attribute stored without a
/// value reading as the empty string.
fn seeded_pairs() -> Vec<(String, String)> {
    seeded()
        .as_array()
        .expect("the seed is a list")
        .iter()
        .map(|attribute| {
            (
                attribute["name"]
                    .as_str()
                    .expect("each seeded attribute is named")
                    .to_string(),
                attribute["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The value [`seeded`] stores against `name`.
fn seeded_value(name: &str) -> String {
    seeded_pairs()
        .into_iter()
        .find(|(seeded_name, _)| seeded_name == name)
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("the seed holds an attribute named {name:?}"))
}

/// The listing [`seeded`] implies for the resource at `arn`, minus the names in
/// `without`, in the order `ListAttributes` reports it.
fn seeded_listing(arn: &str, without: &[&str]) -> Vec<Listed> {
    let mut expected: Vec<Listed> = seeded_pairs()
        .into_iter()
        .filter(|(name, _)| !without.contains(&name.as_str()))
        .map(|(name, value)| (arn.to_string(), name, value))
        .collect();
    expected.sort();
    expected
}

/// The short form id of a container instance: the last segment of its ARN
/// (`arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<id>`) — the other form
/// `Attribute$targetId` documents beside the full ARN.
fn short_id(arn: &str) -> &str {
    arn.rsplit('/')
        .next()
        .expect("a container-instance ARN ends in its id")
}

/// A router holding one cluster whose single container instance carries [`seeded`], and
/// that instance's ARN.
async fn fixture(case: &str) -> (MultiServiceRouter, String) {
    let router = service();
    create_cluster(&router, CLUSTER, case).await;
    let arn = register_with_attributes(&router, CLUSTER, INSTANCE_ID, &seeded(), case).await;
    (router, arn)
}

/// The attributes one `DeleteAttributes` call reports as deleted, each reduced to the
/// target ARN, name and value a client keys on — the same shape
/// [`crate::list_attributes`] reduces a listing to, so the two can be compared.
///
/// Every entry is checked to carry no `targetType`: the response names its target by full
/// ARN, which is the form `Attribute$targetType` is NOT required beside.
async fn delete_attributes(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> Vec<Listed> {
    let body = ok(router, "DeleteAttributes", request, case).await;
    list(&body, "attributes", case)
        .iter()
        .map(|attribute| {
            assert!(
                attribute.get("targetType").and_then(Value::as_str).is_none(),
                "case {case:?}: expected no targetType beside a full-ARN targetId, \
                 got {attribute}"
            );
            let target_id = attribute["targetId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a targetId, got {attribute}"))
                .to_string();
            let name = attribute["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a name, got {attribute}"))
                .to_string();
            let value = attribute["value"].as_str().unwrap_or_default().to_string();
            (target_id, name, value)
        })
        .collect()
}

/// The attribute names and the `version` `DescribeContainerInstances` reports for the
/// instance at `arn` — the resource read back through its own reader.
async fn described(router: &MultiServiceRouter, arn: &str, case: &str) -> (Vec<String>, i64) {
    let body = ok(
        router,
        "DescribeContainerInstances",
        json!({ "cluster": CLUSTER, "containerInstances": [arn] }),
        case,
    )
    .await;
    let instances = list(&body, "containerInstances", case);
    let instance = instances
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected the described instance, got {body}"));
    let mut names: Vec<String> = list(instance, "attributes", case)
        .iter()
        .map(|attribute| {
            attribute["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a name, got {attribute}"))
                .to_string()
        })
        .collect();
    names.sort();
    (names, count(instance, "version", case))
}

#[tokio::test]
async fn the_named_attributes_are_deleted_from_their_target_and_reported() {
    // Each row names its target one of the two documented ways, and differs along what
    // the rule generalises over: how many attributes one request deletes, whether the
    // attribute has a value, whether the entry carries a value of its own, whether the
    // name is one the resource carries, and a repeated entry.
    let rows: [DeletionRow; 8] = [
        (
            "one attribute, target by full ARN",
            |arn, _| json!([{ "name": "stack", "targetId": arn }]),
            &["stack"],
        ),
        (
            "one attribute, target by short form id and targetType",
            |_, short| {
                json!([{
                    "name": "ecs.os-type",
                    "targetId": short,
                    "targetType": "container-instance",
                }])
            },
            &["ecs.os-type"],
        ),
        (
            "an attribute stored without a value",
            |arn, _| json!([{ "name": "ecs.capability.external", "targetId": arn }]),
            &["ecs.capability.external"],
        ),
        (
            "a name carrying a slash and a period",
            |arn, _| json!([{ "name": "fleet.id/west", "targetId": arn }]),
            &["fleet.id/west"],
        ),
        (
            "three attributes in one request, named both ways",
            |arn, short| {
                json!([
                    { "name": "stack", "targetId": arn },
                    { "name": "tier", "targetId": short, "targetType": "container-instance" },
                    { "name": "ecs.capability.external", "targetId": arn },
                ])
            },
            &["stack", "tier", "ecs.capability.external"],
        ),
        (
            "a name the resource does not carry deletes nothing",
            |arn, _| json!([{ "name": "ecs.vpc-id", "targetId": arn }]),
            &[],
        ),
        (
            "an entry carrying a value selects by name alone",
            |arn, _| json!([{ "name": "stack", "value": "staging", "targetId": arn }]),
            &["stack"],
        ),
        (
            "the same entry twice deletes it once",
            |arn, _| {
                json!([
                    { "name": "tier", "targetId": arn },
                    { "name": "tier", "targetId": arn },
                ])
            },
            &["tier"],
        ),
    ];

    for (case, build, deleted_names) in rows {
        let (router, arn) = fixture(case).await;
        let entries = build(&arn, short_id(&arn));

        let mut reported = delete_attributes(
            &router,
            json!({ "cluster": CLUSTER, "attributes": entries }),
            case,
        )
        .await;
        reported.sort();
        let mut expected: Vec<Listed> = deleted_names
            .iter()
            .map(|name| (arn.clone(), name.to_string(), seeded_value(name)))
            .collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the attributes this call removed, got {reported:?}"
        );

        // What the resource carries afterwards is the seeded set minus exactly those.
        let (listed, next) = list_attributes(
            &router,
            json!({ "cluster": CLUSTER, "targetType": "container-instance" }),
            case,
        )
        .await;
        let remaining = seeded_listing(&arn, deleted_names);
        assert_eq!(
            listed, remaining,
            "case {case:?}: expected the stored attributes minus {deleted_names:?}, \
             got {listed:?}"
        );
        assert_eq!(
            next, None,
            "case {case:?}: expected no nextToken for a complete listing, got {next:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_attribute_is_gone_from_the_resource_and_a_second_delete_removes_nothing() {
    let case = "round trip";
    let (router, arn) = fixture(case).await;
    let (names_before, version_before) = described(&router, &arn, case).await;
    assert!(
        names_before.contains(&"stack".to_string()),
        "case {case:?}: expected the seeded attribute on the resource, got {names_before:?}"
    );

    let request = json!({
        "cluster": CLUSTER,
        "attributes": [{ "name": "stack", "targetId": arn }],
    });
    let deleted = delete_attributes(&router, request.clone(), case).await;
    assert_eq!(
        deleted,
        vec![(arn.clone(), "stack".to_string(), "production".to_string())],
        "case {case:?}: expected the deleted attribute reported against its target, \
         got {deleted:?}"
    );

    // The resource read back through its own reader no longer carries it, and the change
    // moved its version counter.
    let (names_after, version_after) = described(&router, &arn, case).await;
    assert!(
        !names_after.contains(&"stack".to_string()),
        "case {case:?}: expected the attribute to be gone from the resource, \
         got {names_after:?}"
    );
    assert_eq!(
        version_after,
        version_before + 1,
        "case {case:?}: expected the version counter to move once, got {version_after}"
    );

    // The same call again: there is nothing left to remove, so it removes nothing, reports
    // nothing and leaves the resource — version included — as it stands.
    let again = delete_attributes(&router, request, case).await;
    assert!(
        again.is_empty(),
        "case {case:?}: expected a second delete to report nothing deleted, got {again:?}"
    );
    let (names_again, version_again) = described(&router, &arn, case).await;
    assert_eq!(
        (names_again, version_again),
        (names_after, version_after),
        "case {case:?}: expected a call that removed nothing to change nothing"
    );
}

#[tokio::test]
async fn up_to_ten_attributes_go_in_one_request_and_one_more_is_refused() {
    let case = "ten per request";
    let router = service();
    create_cluster(&router, CLUSTER, case).await;
    // "You can specify up to 10 attributes for each request": the published bound itself
    // is accepted, and one past it is refused.
    let stored: Vec<Value> = (1..=10)
        .map(|index| json!({ "name": format!("probe-{index}"), "value": format!("v{index}") }))
        .collect();
    let arn = register_with_attributes(
        &router,
        CLUSTER,
        "i-0bbb0000000000002",
        &json!(stored),
        case,
    )
    .await;

    let entries: Vec<Value> = (1..=10)
        .map(|index| json!({ "name": format!("probe-{index}"), "targetId": arn }))
        .collect();
    let deleted = delete_attributes(
        &router,
        json!({ "cluster": CLUSTER, "attributes": entries }),
        case,
    )
    .await;
    let mut expected: Vec<Listed> = (1..=10)
        .map(|index| (arn.clone(), format!("probe-{index}"), format!("v{index}")))
        .collect();
    expected.sort();
    let mut reported = deleted;
    reported.sort();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected all ten attributes deleted in one request, got {reported:?}"
    );
    let (listed, _) = list_attributes(
        &router,
        json!({ "cluster": CLUSTER, "targetType": "container-instance" }),
        case,
    )
    .await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected the resource to carry no attribute afterwards, got {listed:?}"
    );

    let too_many: Vec<Value> = (1..=11)
        .map(|index| json!({ "name": format!("probe-{index}"), "targetId": arn }))
        .collect();
    let (code, message) = refusal(
        &router,
        "DeleteAttributes",
        json!({ "cluster": CLUSTER, "attributes": too_many }),
        case,
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            "attributes can have at most 10 items."
        ),
        "case {case:?}: expected eleven attributes to be refused on the documented bound"
    );
}

#[tokio::test]
async fn an_unusable_request_is_refused_and_deletes_nothing() {
    let case = "refusals";
    let (router, arn) = fixture(case).await;

    // A resource of ANOTHER cluster, and one this cluster no longer holds: neither is a
    // target of a request against `CLUSTER`.
    create_cluster(&router, "other", case).await;
    let other_arn = register_with_attributes(
        &router,
        "other",
        "i-0ccc0000000000003",
        &json!([{ "name": "stack", "value": "staging" }]),
        case,
    )
    .await;
    let deregistered = register_with_attributes(
        &router,
        CLUSTER,
        "i-0ddd0000000000004",
        &json!([{ "name": "stack", "value": "draining" }]),
        case,
    )
    .await;
    ok(
        &router,
        "DeregisterContainerInstance",
        json!({ "cluster": CLUSTER, "containerInstance": deregistered }),
        case,
    )
    .await;

    // A cluster that is not there — including the default cluster, which this router has
    // none of.
    let missing: [(&str, Value); 3] = [
        ("a cluster that was never created", json!("no-such-cluster")),
        (
            "a cluster ARN that names no cluster",
            json!(format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/no-such-cluster"
            )),
        ),
        ("the assumed default cluster", Value::Null),
    ];
    for (row, cluster) in missing {
        let mut request = json!({ "attributes": [{ "name": "stack", "targetId": arn }] });
        if let Value::String(cluster) = &cluster {
            request["cluster"] = json!(cluster);
        }
        refused(
            &router,
            "DeleteAttributes",
            request,
            "ClusterNotFoundException",
            row,
        )
        .await;
    }

    let unusable: [RefusalRow; 7] = [
        (
            "a short form id nothing minted",
            json!({
                "name": "stack",
                "targetId": "0".repeat(32),
                "targetType": "container-instance",
            }),
            "TargetNotFoundException",
            TARGET_NOT_FOUND,
        ),
        (
            "a container-instance ARN of another cluster",
            json!({ "name": "stack", "targetId": other_arn }),
            "TargetNotFoundException",
            TARGET_NOT_FOUND,
        ),
        (
            "a container-instance ARN this cluster no longer holds",
            json!({ "name": "stack", "targetId": deregistered }),
            "TargetNotFoundException",
            TARGET_NOT_FOUND,
        ),
        (
            "a container-instance ARN nothing minted",
            json!({
                "name": "stack",
                "targetId": format!(
                    "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/{CLUSTER}/{}",
                    "f".repeat(32)
                ),
            }),
            "TargetNotFoundException",
            TARGET_NOT_FOUND,
        ),
        (
            "an entry that names no target",
            json!({ "name": "stack" }),
            "InvalidParameterException",
            "attributes.targetId is required to name the resource the attribute is on",
        ),
        (
            "an entry whose targetId is blank",
            json!({ "name": "stack", "targetId": "   ", "targetType": "container-instance" }),
            "InvalidParameterException",
            "attributes.targetId is required to name the resource the attribute is on",
        ),
        (
            "a short form id with no targetType",
            json!({ "name": "stack", "targetId": short_id(&arn) }),
            "InvalidParameterException",
            "attributes.targetType is required when targetId is the short form id of a resource",
        ),
    ];
    for (row, entry, expected_code, expected_message) in unusable {
        let (code, message) = refusal(
            &router,
            "DeleteAttributes",
            json!({ "cluster": CLUSTER, "attributes": [entry] }),
            row,
        )
        .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            (expected_code, expected_message),
            "case {row:?}: expected the documented refusal for this entry"
        );
    }

    // A request whose entries are not ALL usable deletes none of them.
    refused(
        &router,
        "DeleteAttributes",
        json!({
            "cluster": CLUSTER,
            "attributes": [
                { "name": "stack", "targetId": arn },
                { "name": "tier", "targetId": other_arn },
            ],
        }),
        "TargetNotFoundException",
        "one usable entry beside an unusable one",
    )
    .await;

    // Every refusal above left the store as it was.
    let (listed, _) = list_attributes(
        &router,
        json!({ "cluster": CLUSTER, "targetType": "container-instance" }),
        case,
    )
    .await;
    assert_eq!(
        listed,
        seeded_listing(&arn, &[]),
        "case {case:?}: expected a refused request to delete nothing, got {listed:?}"
    );
}
