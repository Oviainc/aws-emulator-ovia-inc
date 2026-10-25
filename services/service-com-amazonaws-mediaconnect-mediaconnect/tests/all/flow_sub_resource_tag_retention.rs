// SPDX-License-Identifier: CC0-1.0

//! What a resource's tag map may be changed BY, over the flow hierarchy, end to end over
//! HTTP.
//!
//! `tests/all/flow_sub_resource_tags.rs` pins the write side and the read side of the tag
//! members a create request carries (`AddOutputRequest$OutputTags`,
//! `SetSourceRequest$SourceTags`, `GrantEntitlementRequest$EntitlementTags`, and the flow's
//! own `CreateFlowRequest$FlowTags`): the pairs land on the resource the member tagged and
//! are what the tag operations on that resource's own ARN report and modify. This suite
//! drives the SAME hierarchy — its fixture, its `Resource` rows and its tag helpers, so
//! there is one of each — and pins the other half of that rule: the only two things that
//! MOVE such a map.
//!
//! * The tag operations on the resource's ARN, and its own create's tag member. An
//!   operation whose request models NO tag member cannot change a tag, and none of
//!   `UpdateFlow`, `UpdateFlowOutput`, `UpdateFlowSource` and `UpdateFlowEntitlement`
//!   models one (`smithy_query(query="operation",operation="UpdateFlowOutput")` and its
//!   three siblings list no tag member), so a successful update leaves the addressed
//!   resource's tags exactly as they were — what the create left on it, and what a later
//!   `TagResource` wrote.
//! * The disappearance of the resource: `RemoveFlowSource` detaches a source, after which
//!   its ARN addresses nothing and all three tag operations answer the modelled 404, while
//!   every sibling resource keeps its own map. (`RemoveFlowOutput`, `RevokeFlowEntitlement`
//!   and `DeleteFlow` are pinned in the suite above.)
//!
//! The table spans the class rather than one sub-resource: every ARN-bearing resource kind
//! the fixture holds (flow, output, source, entitlement), each minted both by `CreateFlow`
//! and by the operation that adds one to a live flow, with and without create-time tags,
//! each driven through the update operation that addresses it.

use serde_json::{json, Value};

use crate::flow_lifecycle::Emulator;
use crate::flow_sources::remove_flow_source;
use crate::flow_sub_resource_tags::{
    assert_tagging_is_not_found, fixture, reported_tags, stored, tag, Resource,
};

/// What the description every resource's update sets becomes, so the update is asserted to
/// have DONE something rather than only to have answered 202: a retention assertion a
/// body-discarding handler could satisfy would pin nothing.
const DESCRIPTION: &str = "updated by the retention suite";

/// One update that models no tag member: the URI the model binds the operation that updates
/// that kind of resource to, a body carrying a member that is NOT a tag, and where in the
/// answer that member lands with what value.
struct Update {
    uri: String,
    body: Value,
    /// The chain of response keys the updated member sits under.
    landed_at: Vec<&'static str>,
    landed: Value,
}

/// The update that addresses `arn`.
///
/// The resource kind is read off the ARN's own resource-type segment, so a row added to the
/// shared fixture is routed to its update without a second table to keep in step.
fn update_binding(flow_arn: &str, arn: &str) -> Update {
    let described = json!({ "description": DESCRIPTION });
    match arn.split(':').nth(5).unwrap_or_default() {
        // `UpdateFlow` models no `Description`, so the flow's update carries the member it
        // does model.
        "flow" => Update {
            uri: format!("/v1/flows/{arn}"),
            body: json!({ "sourceMonitoringConfig": { "thumbnailState": "ENABLED" } }),
            landed_at: vec!["flow", "sourceMonitoringConfig", "thumbnailState"],
            landed: json!("ENABLED"),
        },
        "output" => Update {
            uri: format!("/v1/flows/{flow_arn}/outputs/{arn}"),
            body: described,
            landed_at: vec!["output", "description"],
            landed: json!(DESCRIPTION),
        },
        "source" => Update {
            uri: format!("/v1/flows/{flow_arn}/source/{arn}"),
            body: described,
            landed_at: vec!["source", "description"],
            landed: json!(DESCRIPTION),
        },
        "entitlement" => Update {
            uri: format!("/v1/flows/{flow_arn}/entitlements/{arn}"),
            body: described,
            landed_at: vec!["entitlement", "description"],
            landed: json!(DESCRIPTION),
        },
        other => panic!("case {arn:?}: no update operation addresses a {other:?} resource"),
    }
}

/// Send one update and assert it was accepted AND that the member it carried landed, so
/// the tag assertions around it cannot be satisfied by a handler that ignored the body.
async fn assert_update_applied(emulator: &Emulator, case: &str, update: &Update) {
    let (status, code, answered) = emulator
        .send("PUT", &update.uri, Some(update.body.clone()))
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {case:?}: the update must be accepted, got {status} {code:?}: {answered}"
    );
    let mut landed = &answered;
    for key in &update.landed_at {
        let Some(next) = landed.get(key) else {
            panic!(
                "case {case:?}: the answer carries no {key:?} on the way to {:?}: {answered}",
                update.landed_at
            );
        };
        landed = next;
    }
    assert_eq!(
        landed, &update.landed,
        "case {case:?}: the update's own member must land under {:?}, got {answered}",
        update.landed_at
    );
}

/// An update whose request models no tag member leaves the addressed resource's tags
/// exactly as they were: what the create left on it survives the update, and so does what
/// a later `TagResource` wrote.
#[tokio::test]
async fn an_update_that_models_no_tag_member_leaves_the_tags_alone() {
    let emulator = Emulator::new();
    let (flow_arn, mut resources) = fixture(&emulator).await;

    for (index, resource) in resources.iter_mut().enumerate() {
        let Resource { case, arn, tags } = resource;
        let update = update_binding(&flow_arn, arn);

        // The create's tag member is what the read reports before the update runs.
        let before = reported_tags(&emulator, case, arn).await;
        assert_eq!(
            &before, tags,
            "case {case:?}: the create request's tag member is what the read reports, got \
             {before:?}"
        );

        assert_update_applied(&emulator, case, &update).await;

        let after = reported_tags(&emulator, case, arn).await;
        assert_eq!(
            &after, tags,
            "case {case:?}: an update that models no tag member must not move the tags, got \
             {after:?}"
        );

        // The same holds for a map a TAG WRITE left on the resource: it is the tag
        // operations, not the update, that own it.
        let written = format!("ops-{index}");
        let (status, code, body) = tag(
            &emulator,
            arn,
            json!({ "owner": written.clone(), "env": "staging" }),
        )
        .await;
        assert_eq!(
            (status, code.as_deref()),
            (204, None),
            "case {case:?}: tagging expected 204, got {status} {code:?}: {body}"
        );
        tags.insert("owner".to_string(), written);
        tags.insert("env".to_string(), "staging".to_string());

        assert_update_applied(&emulator, case, &update).await;
        let after = reported_tags(&emulator, case, arn).await;
        assert_eq!(
            &after, tags,
            "case {case:?}: the update must not move what the tag write left either, got \
             {after:?}"
        );
    }

    // Every update above addressed ONE resource: each map is still exactly what that
    // resource's own create and tag write left on it.
    for resource in &resources {
        let reported = reported_tags(&emulator, &resource.case, &resource.arn).await;
        assert_eq!(
            reported, resource.tags,
            "case {:?}: another resource's update must not move it, got {reported:?}",
            resource.case
        );
    }
}

/// A source's tags go away with the source: once `RemoveFlowSource` has detached it, its
/// ARN addresses nothing and all three tag operations answer the modelled 404, while every
/// resource the flow still holds keeps its own map.
#[tokio::test]
async fn a_removed_source_takes_its_tags_with_it() {
    let emulator = Emulator::new();
    let (flow_arn, resources) = fixture(&emulator).await;

    // The source the add path minted: the flow keeps the one it was created with, which is
    // what lets RemoveFlowSource accept the call at all.
    let removed = resources
        .iter()
        .find(|resource| resource.case.starts_with("a source AddFlowSources"))
        .expect("the fixture holds a source the add path minted");
    let removed_case = removed.case.clone();
    let removed_arn = removed.arn.clone();

    let before = reported_tags(&emulator, &removed_case, &removed_arn).await;
    assert_eq!(
        before,
        stored(&[("src", "added"), ("team", "video")]),
        "case {removed_case:?}: the added source carries its create's tags, got {before:?}"
    );

    let (status, code, body) = remove_flow_source(&emulator, &flow_arn, &removed_arn).await;
    assert_eq!(
        (status, code.as_deref()),
        (202, None),
        "case {removed_case:?}: removing the source expected 202, got {status} {code:?}: {body}"
    );

    assert_tagging_is_not_found(&emulator, &removed_case, &removed_arn).await;

    for resource in &resources {
        if resource.arn == removed_arn {
            continue;
        }
        let reported = reported_tags(&emulator, &resource.case, &resource.arn).await;
        assert_eq!(
            reported, resource.tags,
            "case {:?}: removing another resource must not move its tags, got {reported:?}",
            resource.case
        );
    }
}
