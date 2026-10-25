// SPDX-License-Identifier: CC0-1.0

//! The tagging slice over a flow's ARN-BEARING SUB-RESOURCES, end to end over HTTP.
//!
//! `ResourceArn` addresses "a resource ... in the current region", and MediaConnect gives
//! a flow's outputs, sources and entitlements ARNs of their own (`Output$OutputArn`,
//! `Source$SourceArn`, `Entitlement$EntitlementArn`) plus a tag member on each create
//! request that mints one (`AddOutputRequest$OutputTags`, `SetSourceRequest$SourceTags`,
//! `GrantEntitlementRequest$EntitlementTags`). So each of them is a resource the tagging
//! operations address in its own right, with its own tag map. The rules under test:
//!
//! * `ListTagsForResource` on any ARN this service minted and still holds answers 200 with
//!   that resource's own tag map — the pairs its create request's tag member carried, and
//!   an EMPTY map when it carried none;
//! * `TagResource` and `UntagResource` on the same ARN write that same map, and a write to
//!   one resource moves no other resource's tags;
//! * a tag read changes nothing a later caller sees;
//! * a sub-resource's tags go away with the sub-resource (RemoveFlowOutput,
//!   RevokeFlowEntitlement) and with its flow (DeleteFlow), after which its ARN is 404
//!   `NotFoundException` on all three operations, exactly as an ARN this service never
//!   minted is;
//! * a create request's sub-resource tag member is bounded exactly as `FlowTags` and a tag
//!   write are, on the create path and on the add path alike (`CreateFlow`,
//!   `AddFlowOutputs`, `AddFlowSources`, `GrantFlowEntitlements`): a pair past the
//!   documented key/value lengths, or more pairs than a resource keeps, is 400
//!   `BadRequestException` and mints nothing.
//!
//! The table spans the class rather than one sub-resource: every ARN-bearing resource kind
//! (flow, output, source, entitlement), each minted both by `CreateFlow` and by the
//! operation that adds one to an existing flow (`AddFlowOutputs`, `AddFlowSources`,
//! `GrantFlowEntitlements`), with and without create-time tags.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_entitlements::grant;
use crate::flow_lifecycle::{Emulator, REGION};
use crate::flow_outputs::add_flow_outputs;
use crate::flow_sources::{add_flow_sources, failover_active};
use crate::resource_tags::pct;

/// The account id every subscriber list below names — the entitlement's required
/// `Subscribers` member has to name somebody.
const SUBSCRIBER: &str = "111122223333";

/// One answer from the wire: `(status, error code, body)`.
type Reply = (u16, Option<String>, Value);

/// `POST /tags/{ResourceArn}` with the pairs in `tags`.
///
/// `pub(crate)` — like `reported_tags`, `assert_tagging_is_not_found`, `stored`, `Resource`
/// and `fixture` below — because `tests/all/flow_sub_resource_tag_retention.rs` asserts what
/// MOVES these same maps, and so drives the same bindings and the same hierarchy rather
/// than spelling either out a second time.
pub(crate) async fn tag(emulator: &Emulator, resource_arn: &str, tags: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", pct(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
}

/// `GET /tags/{ResourceArn}`.
async fn list_tags(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .send("GET", &format!("/tags/{}", pct(resource_arn)), None)
        .await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=..` — `TagKeys` is bound to `@httpQuery("tagKeys")`,
/// so each key travels as its own query pair.
async fn untag(emulator: &Emulator, resource_arn: &str, keys: &[&str]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .send(
            "DELETE",
            &format!("/tags/{}?{query}", pct(resource_arn)),
            None,
        )
        .await
}

/// The tags `ListTagsForResource` reports for `resource_arn`, asserted to be a 200.
pub(crate) async fn reported_tags(
    emulator: &Emulator,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let (status, code, body) = list_tags(emulator, resource_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "case {case:?}: list tags for {resource_arn} expected 200, got {status} {code:?}: {body}"
    );
    body["tags"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// A tag map as the service stores it, from `(key, value)` pairs.
pub(crate) fn stored(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// One resource under test: how the case is named, the ARN this emulator minted for it,
/// and the tag map its create request left on it.
pub(crate) struct Resource {
    pub(crate) case: String,
    pub(crate) arn: String,
    pub(crate) tags: BTreeMap<String, String>,
}

/// A deterministic string of `length` ASCII characters, for exercising a documented length
/// bound rather than one hand-picked spelling.
fn filler(length: usize, seed: u8) -> String {
    (0..length)
        .map(|index| char::from(b'a' + ((index as u8).wrapping_add(seed) % 26)))
        .collect()
}

/// Create the fixture flow and return every ARN-bearing resource it holds, each with the
/// tags its own create request carried.
///
/// The flow is created with one tagged source, a tagged and an untagged output, and a
/// tagged and an untagged entitlement; then `AddFlowOutputs`, `AddFlowSources` and
/// `GrantFlowEntitlements` add one more of each, so the table covers a sub-resource minted
/// by the create AND one minted by the operation that adds it to a live flow. Source
/// failover is ENABLED because the add gives the flow a second source.
///
/// `pub(crate)` because `tests/all/flow_sub_resource_tag_retention.rs` asserts what MOVES
/// these same maps, and so needs the same hierarchy rather than a second copy of it.
pub(crate) async fn fixture(emulator: &Emulator) -> (String, Vec<Resource>) {
    let (status, code, created) = emulator
        .create_flow(json!({
            "name": "sub-resource-tags",
            "sourceFailoverConfig": failover_active(),
            "sources": [{
                "name": "created-source",
                "protocol": "rtp",
                "whitelistCidr": "10.0.0.0/16",
                "sourceTags": { "src": "created" }
            }],
            "outputs": [
                {
                    "name": "created-output",
                    "protocol": "rtp",
                    "destination": "198.51.100.11",
                    "port": 5000,
                    "outputTags": { "out": "created", "env": "prod" }
                },
                {
                    "name": "bare-output",
                    "protocol": "rtp",
                    "destination": "198.51.100.12",
                    "port": 5001
                }
            ],
            "entitlements": [
                {
                    "name": "created-entitlement",
                    "subscribers": [SUBSCRIBER],
                    "entitlementTags": { "ent": "created" }
                },
                { "name": "bare-entitlement", "subscribers": [SUBSCRIBER] }
            ],
            "flowTags": { "flow": "created" }
        }))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "the fixture flow must be created, got {status} {code:?}: {created}"
    );
    let flow = &created["flow"];
    let flow_arn = flow["flowArn"].as_str().unwrap_or_default().to_string();

    let arn_of = |member: &str, index: usize, arn_member: &str| {
        flow[member][index][arn_member]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    let (status, code, added_outputs) = add_flow_outputs(
        emulator,
        &flow_arn,
        json!([{
            "name": "added-output",
            "protocol": "rtp",
            "destination": "198.51.100.13",
            "port": 5002,
            "outputTags": { "out": "added" }
        }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "adding the fixture output expected 201, got {status} {code:?}: {added_outputs}"
    );

    let (status, code, added_sources) = add_flow_sources(
        emulator,
        &flow_arn,
        json!([{
            "name": "added-source",
            "protocol": "rtp",
            "whitelistCidr": "10.1.0.0/16",
            "sourceTags": { "src": "added", "team": "video" }
        }]),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "adding the fixture source expected 201, got {status} {code:?}: {added_sources}"
    );

    let (status, code, granted) = grant(
        emulator,
        &flow_arn,
        json!({ "entitlements": [{
            "name": "granted-entitlement",
            "subscribers": [SUBSCRIBER],
            "entitlementTags": { "ent": "granted" }
        }] }),
    )
    .await;
    assert_eq!(
        (status, code.as_deref()),
        (201, None),
        "granting the fixture entitlement expected 201, got {status} {code:?}: {granted}"
    );

    let resources = vec![
        Resource {
            case: "the flow itself, created with flowTags".to_string(),
            arn: flow_arn.clone(),
            tags: stored(&[("flow", "created")]),
        },
        Resource {
            case: "an output CreateFlow minted with outputTags".to_string(),
            arn: arn_of("outputs", 0, "outputArn"),
            tags: stored(&[("out", "created"), ("env", "prod")]),
        },
        Resource {
            case: "an output CreateFlow minted with no outputTags".to_string(),
            arn: arn_of("outputs", 1, "outputArn"),
            tags: BTreeMap::new(),
        },
        Resource {
            case: "an output AddFlowOutputs minted with outputTags".to_string(),
            arn: added_outputs["outputs"][0]["outputArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            tags: stored(&[("out", "added")]),
        },
        Resource {
            case: "a source CreateFlow minted with sourceTags".to_string(),
            arn: arn_of("sources", 0, "sourceArn"),
            tags: stored(&[("src", "created")]),
        },
        Resource {
            case: "a source AddFlowSources minted with sourceTags".to_string(),
            arn: added_sources["sources"][0]["sourceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            tags: stored(&[("src", "added"), ("team", "video")]),
        },
        Resource {
            case: "an entitlement CreateFlow granted with entitlementTags".to_string(),
            arn: arn_of("entitlements", 0, "entitlementArn"),
            tags: stored(&[("ent", "created")]),
        },
        Resource {
            case: "an entitlement CreateFlow granted with no entitlementTags".to_string(),
            arn: arn_of("entitlements", 1, "entitlementArn"),
            tags: BTreeMap::new(),
        },
        Resource {
            case: "an entitlement GrantFlowEntitlements granted with entitlementTags"
                .to_string(),
            arn: granted["entitlements"][0]["entitlementArn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            tags: stored(&[("ent", "granted")]),
        },
    ];
    for resource in &resources {
        assert!(
            !resource.arn.is_empty(),
            "case {:?}: the fixture must mint an ARN for it",
            resource.case
        );
    }
    (flow_arn, resources)
}

/// Every ARN this emulator minted in the flow hierarchy carries its OWN tag map: the
/// create request's tag member is what a read reports, a tag write merges into exactly
/// that map, an untag removes from it, and no other resource's map moves.
#[tokio::test]
async fn every_arn_bearing_flow_resource_carries_its_own_tags() {
    let emulator = Emulator::new();
    let (_flow_arn, mut resources) = fixture(&emulator).await;

    // What the create left on each resource is what the read reports — including the
    // EMPTY map of a resource created without tags, which is a 200 and not a 404.
    for resource in &resources {
        let reported = reported_tags(&emulator, &resource.case, &resource.arn).await;
        assert_eq!(
            reported, resource.tags,
            "case {:?}: the create request's tag member is what the read reports, got \
             {reported:?}",
            resource.case
        );
        // A read changes nothing a later caller sees.
        let again = reported_tags(&emulator, &resource.case, &resource.arn).await;
        assert_eq!(
            again, resource.tags,
            "case {:?}: a second read must report the same map, got {again:?}",
            resource.case
        );
    }

    // A tag write merges into the addressed resource's map, and an untag removes from it.
    for (index, resource) in resources.iter_mut().enumerate() {
        let case = resource.case.clone();
        let arn = resource.arn.clone();

        let (status, code, body) = tag(
            &emulator,
            &arn,
            json!({ "owner": format!("ops-{index}"), "env": "staging" }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: tagging expected 204, got {status} {code:?}: {body}"
        );
        resource
            .tags
            .insert("owner".to_string(), format!("ops-{index}"));
        resource
            .tags
            .insert("env".to_string(), "staging".to_string());
        let reported = reported_tags(&emulator, &case, &arn).await;
        assert_eq!(
            reported, resource.tags,
            "case {case:?}: the write merges into this resource's map, got {reported:?}"
        );

        let (status, code, body) = untag(&emulator, &arn, &["env", "absent"]).await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: untagging expected 204, got {status} {code:?}: {body}"
        );
        resource.tags.remove("env");
        let reported = reported_tags(&emulator, &case, &arn).await;
        assert_eq!(
            reported, resource.tags,
            "case {case:?}: the untag removes exactly the named key, got {reported:?}"
        );
    }

    // Every write above addressed ONE resource: each map is still exactly what that
    // resource's own writes left on it.
    for resource in &resources {
        let reported = reported_tags(&emulator, &resource.case, &resource.arn).await;
        assert_eq!(
            reported, resource.tags,
            "case {:?}: another resource's tag writes must not move it, got {reported:?}",
            resource.case
        );
    }
}

/// A sub-resource's tags live and die with the sub-resource: once the output is removed,
/// the entitlement revoked or the flow deleted, its ARN addresses nothing and all three
/// tagging operations answer the modelled 404 — exactly as they do for an ARN of each kind
/// that this service never minted.
#[tokio::test]
async fn a_removed_sub_resource_takes_its_tags_with_it() {
    let emulator = Emulator::new();
    let (flow_arn, resources) = fixture(&emulator).await;

    let output_arn = resources[1].arn.clone();
    let entitlement_arn = resources[6].arn.clone();
    let source_arn = resources[4].arn.clone();

    let (status, code, body) = emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/outputs/{output_arn}"),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "removing the output expected 202, got {status} {code:?}: {body}"
    );
    let (status, code, body) = emulator
        .send(
            "DELETE",
            &format!("/v1/flows/{flow_arn}/entitlements/{entitlement_arn}"),
            None,
        )
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "revoking the entitlement expected 202, got {status} {code:?}: {body}"
    );

    // Everything the flow still holds keeps answering its own tags.
    let reported = reported_tags(&emulator, "the surviving source", &source_arn).await;
    assert_eq!(
        reported,
        stored(&[("src", "created")]),
        "a surviving sub-resource keeps its tags, got {reported:?}"
    );

    let gone = [
        ("the removed output", output_arn),
        ("the revoked entitlement", entitlement_arn),
        (
            "an output ARN this service never minted",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:output:1-0000dead:never-minted"),
        ),
        (
            "a source ARN this service never minted",
            format!("arn:aws:mediaconnect:{REGION}:111122223333:source:1-0000dead:never-minted"),
        ),
        (
            "an entitlement ARN this service never minted",
            format!(
                "arn:aws:mediaconnect:{REGION}:111122223333:entitlement:1-0000dead:never-minted"
            ),
        ),
    ];
    for (case, arn) in &gone {
        assert_tagging_is_not_found(&emulator, case, arn).await;
    }

    // DeleteFlow takes the whole hierarchy's tags with it: "when a resource is deleted, the
    // tags associated with that resource are deleted as well".
    let (status, code, body) = emulator
        .send("DELETE", &format!("/v1/flows/{flow_arn}"), None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "deleting the flow expected 202, got {status} {code:?}: {body}"
    );
    for resource in &resources {
        assert_tagging_is_not_found(&emulator, &resource.case, &resource.arn).await;
    }
}

/// All three tagging operations refuse `arn` with the modelled `NotFoundException` (404) —
/// never a 501 and never a success.
pub(crate) async fn assert_tagging_is_not_found(emulator: &Emulator, case: &str, arn: &str) {
    let calls: Vec<(&str, Reply)> = vec![
        ("TagResource", tag(emulator, arn, json!({ "k": "v" })).await),
        ("ListTagsForResource", list_tags(emulator, arn).await),
        ("UntagResource", untag(emulator, arn, &["k"]).await),
    ];
    for (operation, (status, code, body)) in calls {
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?} / {operation}: expected 404 NotFoundException, got {status} \
             {code:?}: {body}"
        );
    }
}

/// One row of the bounds table: the tag map a sub-resource's create request carries, and
/// the status and error code the service answers for it. The accepted status is 201 for
/// every operation that mints a sub-resource (CreateFlow, AddFlowOutputs, AddFlowSources,
/// GrantFlowEntitlements all answer 201), so one table serves the create path and the add
/// path alike.
struct TagMemberCase {
    case: &'static str,
    tags: Value,
    status: u16,
    code: Option<&'static str>,
}

/// The bounds a tag map a request carries is subject to, as a table: the pair lengths come
/// from the tag member's own documentation (`mc::MAX_TAG_KEY_LENGTH` /
/// `mc::MAX_TAG_VALUE_LENGTH`) and the count from the tags-per-resource bound
/// (`mc::MAX_TAGS_PER_RESOURCE`), each exercised AT the bound and one past it.
fn tag_member_cases() -> Vec<TagMemberCase> {
    let at_key_bound = filler(mc::MAX_TAG_KEY_LENGTH, 3);
    let past_key_bound = filler(mc::MAX_TAG_KEY_LENGTH + 1, 3);
    let at_value_bound = filler(mc::MAX_TAG_VALUE_LENGTH, 7);
    let past_value_bound = filler(mc::MAX_TAG_VALUE_LENGTH + 1, 7);
    let map_of = |count: usize| -> Value {
        json!((0..count)
            .map(|index| (format!("key-{index}"), Value::String(format!("v{index}"))))
            .collect::<serde_json::Map<String, Value>>())
    };

    vec![
        TagMemberCase {
            case: "no tag member at all",
            tags: Value::Null,
            status: 201,
            code: None,
        },
        TagMemberCase {
            case: "an empty tag map",
            tags: json!({}),
            status: 201,
            code: None,
        },
        TagMemberCase {
            case: "a key and a value at the documented maximum lengths",
            tags: json!({ at_key_bound: at_value_bound }),
            status: 201,
            code: None,
        },
        TagMemberCase {
            case: "a key one character past the maximum length",
            tags: json!({ past_key_bound: "v" }),
            status: 400,
            code: Some("BadRequestException"),
        },
        TagMemberCase {
            case: "a value one character past the maximum length",
            tags: json!({ "past-value-bound": past_value_bound }),
            status: 400,
            code: Some("BadRequestException"),
        },
        TagMemberCase {
            case: "an empty key names no tag",
            tags: json!({ "": "v" }),
            status: 400,
            code: Some("BadRequestException"),
        },
        TagMemberCase {
            case: "as many pairs as a resource keeps",
            tags: map_of(mc::MAX_TAGS_PER_RESOURCE),
            status: 201,
            code: None,
        },
        TagMemberCase {
            case: "one pair more than a resource keeps",
            tags: map_of(mc::MAX_TAGS_PER_RESOURCE + 1),
            status: 400,
            code: Some("BadRequestException"),
        },
    ]
}

/// The tag map a case expects a resource to carry, from the map its request carried.
fn expected_tags(tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// A `SetSourceRequest` named `name`, carrying `tags` as its `SourceTags` when there are
/// any.
fn source_request(name: &str, tags: &Value) -> Value {
    let mut source = json!({
        "name": name,
        "protocol": "rtp",
        "whitelistCidr": "10.0.0.0/16"
    });
    if !tags.is_null() {
        source["sourceTags"] = tags.clone();
    }
    source
}

/// An `AddOutputRequest` named `name`, carrying `tags` as its `OutputTags` when there are
/// any.
fn output_request(name: &str, tags: &Value) -> Value {
    let mut output = json!({
        "name": name,
        "protocol": "rtp",
        "destination": "198.51.100.20",
        "port": 5000
    });
    if !tags.is_null() {
        output["outputTags"] = tags.clone();
    }
    output
}

/// A `GrantEntitlementRequest` named `name`, carrying `tags` as its `EntitlementTags` when
/// there are any.
fn entitlement_request(name: &str, tags: &Value) -> Value {
    let mut entitlement = json!({ "name": name, "subscribers": [SUBSCRIBER] });
    if !tags.is_null() {
        entitlement["entitlementTags"] = tags.clone();
    }
    entitlement
}

/// A sub-resource's create-time tag member is bounded exactly as `FlowTags` and a tag write
/// are: a pair at the documented key/value maximum is stored, one past either bound is 400
/// `BadRequestException`, and so is a map with more pairs than a resource keeps. A refused
/// request stores nothing — not even the flow.
#[tokio::test]
async fn create_time_sub_resource_tags_are_bounded_like_a_tag_write() {
    let emulator = Emulator::new();

    for (index, row) in tag_member_cases().into_iter().enumerate() {
        let TagMemberCase {
            case,
            tags,
            status: want_status,
            code: want_code,
        } = row;
        // The same map is put on each of the three sub-resource tag members in turn, so
        // the bound is asserted on every member the rule covers, not only on one.
        for member in ["sourceTags", "outputTags", "entitlementTags"] {
            let bare = Value::Null;
            let (source, output, entitlement) = match member {
                "sourceTags" => (
                    source_request("src", &tags),
                    output_request("out", &bare),
                    entitlement_request("ent", &bare),
                ),
                "outputTags" => (
                    source_request("src", &bare),
                    output_request("out", &tags),
                    entitlement_request("ent", &bare),
                ),
                _ => (
                    source_request("src", &bare),
                    output_request("out", &bare),
                    entitlement_request("ent", &tags),
                ),
            };
            let name = format!("bounded-{member}-{index}");
            let (status, code, body) = emulator
                .create_flow(json!({
                    "name": name,
                    "source": source,
                    "outputs": [output],
                    "entitlements": [entitlement]
                }))
                .await;
            assert_eq!(
                (status, code.as_deref()),
                (want_status, want_code),
                "case {case:?} on {member}: expected {want_status} {want_code:?}, got {status} \
                 {code:?}: {body}"
            );

            if want_status != 201 {
                // A refused create stores nothing: no flow of that name exists, so no
                // sub-resource of it does either.
                let (status, code, listed) = emulator.list_flows("").await;
                assert_eq!(
                    (status, code.as_deref()),
                    (200, None),
                    "case {case:?} on {member}: listing flows expected 200, got {status} \
                     {code:?}: {listed}"
                );
                let names: Vec<&str> = listed["flows"]
                    .as_array()
                    .map(|flows| {
                        flows
                            .iter()
                            .filter_map(|flow| flow["name"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                assert!(
                    !names.contains(&name.as_str()),
                    "case {case:?} on {member}: a refused create stores no flow, got {names:?}"
                );
                continue;
            }
            // An accepted map is what the sub-resource then reports under its own ARN.
            let flow = &body["flow"];
            let arn = match member {
                "sourceTags" => flow["source"]["sourceArn"].as_str(),
                "outputTags" => flow["outputs"][0]["outputArn"].as_str(),
                _ => flow["entitlements"][0]["entitlementArn"].as_str(),
            }
            .unwrap_or_default()
            .to_string();
            let reported = reported_tags(&emulator, case, &arn).await;
            assert_eq!(
                reported,
                expected_tags(&tags),
                "case {case:?} on {member}: the accepted pairs are what the resource carries, \
                 got {reported:?}"
            );
        }
    }
}

/// The same bounds hold on the ADD path, where a sub-resource is minted on a flow that
/// already exists: AddFlowOutputs, AddFlowSources and GrantFlowEntitlements refuse a tag
/// member outside them with 400 `BadRequestException` and mint nothing, and store the pairs
/// of one inside them under the ARN they mint.
#[tokio::test]
async fn added_sub_resource_tags_are_bounded_like_a_tag_write() {
    let emulator = Emulator::new();

    for (index, row) in tag_member_cases().into_iter().enumerate() {
        let TagMemberCase {
            case,
            tags,
            status: want_status,
            code: want_code,
        } = row;
        for operation in ["AddFlowOutputs", "AddFlowSources", "GrantFlowEntitlements"] {
            // One fresh flow per case, so what the flow holds afterwards is only what this
            // call did. Source failover is ENABLED because AddFlowSources gives the flow a
            // second source.
            let (status, code, created) = emulator
                .create_flow(json!({
                    "name": format!("add-bounded-{index}"),
                    "sourceFailoverConfig": failover_active(),
                    "source": source_request("base-source", &Value::Null)
                }))
                .await;
            assert_eq!(
                (status, code.as_deref()),
                (201, None),
                "case {case:?} / {operation}: the fixture flow must be created, got {status} \
                 {code:?}: {created}"
            );
            let flow_arn = created["flow"]["flowArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            let (status, code, body) = match operation {
                "AddFlowOutputs" => {
                    add_flow_outputs(&emulator, &flow_arn, json!([output_request("out", &tags)]))
                        .await
                }
                "AddFlowSources" => {
                    add_flow_sources(&emulator, &flow_arn, json!([source_request("src", &tags)]))
                        .await
                }
                _ => {
                    grant(
                        &emulator,
                        &flow_arn,
                        json!({ "entitlements": [entitlement_request("ent", &tags)] }),
                    )
                    .await
                }
            };
            assert_eq!(
                (status, code.as_deref()),
                (want_status, want_code),
                "case {case:?} / {operation}: expected {want_status} {want_code:?}, got {status} \
                 {code:?}: {body}"
            );

            let (status, code, described) = emulator.describe_flow(&flow_arn).await;
            assert_eq!(
                (status, code.as_deref()),
                (200, None),
                "case {case:?} / {operation}: describing the flow expected 200, got {status} \
                 {code:?}: {described}"
            );
            let flow = &described["flow"];
            let held = |member: &str| flow[member].as_array().map_or(0, Vec::len);
            if want_status != 201 {
                // A refused add mints nothing: the flow still holds only what it was
                // created with (its one base source).
                assert_eq!(
                    (held("outputs"), held("sources"), held("entitlements")),
                    (0, 1, 0),
                    "case {case:?} / {operation}: a refused add stores nothing, got {flow}"
                );
                continue;
            }
            let arn = match operation {
                "AddFlowOutputs" => flow["outputs"][0]["outputArn"].as_str(),
                "AddFlowSources" => flow["sources"][1]["sourceArn"].as_str(),
                _ => flow["entitlements"][0]["entitlementArn"].as_str(),
            }
            .unwrap_or_default()
            .to_string();
            let reported = reported_tags(&emulator, case, &arn).await;
            assert_eq!(
                reported,
                expected_tags(&tags),
                "case {case:?} / {operation}: the accepted pairs are what the minted \
                 sub-resource carries, got {reported:?}"
            );
        }
    }
}
