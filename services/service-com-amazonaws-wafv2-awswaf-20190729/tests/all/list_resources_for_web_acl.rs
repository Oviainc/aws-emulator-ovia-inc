// SPDX-License-Identifier: CC0-1.0

//! What `ListResourcesForWebACL` reports, end to end over the wire.
//!
//! The rule: the call reports the ARNs of the protected resources the stored
//! bindings give the web ACL the request names, narrowed to the one requested
//! `ResourceType` (the load balancer kind when the request names none) — so a web
//! ACL that no binding names reports an EMPTY `ResourceArns` list for every kind,
//! rather than a refusal or an absent member, and the report leaves every binding
//! exactly as it found it.
//!
//! The tables span the class rather than the one recorded call: one row per
//! `ResourceType` the model declares
//! (`smithy_query(query="shape",shape="ResourceType")` → eight kinds) plus the row
//! where the request names no type at all, crossed with web ACLs that differ by
//! scope and by rule count, and against a store that does hold bindings — of
//! another web ACL. The harness is the one in [`crate::web_acl_lifecycle`], so
//! every call goes through the router's front door.

use serde_json::{json, Value};

use crate::web_acl_associations::{listed_resources, web_acl_for_resource, PROTECTED_RESOURCES};
use crate::web_acl_lifecycle::{create_web_acl, rule, sqli_statement, Waf};

/// Every `ResourceType` a listing can name, and the row for a request that names
/// none. Taken from the catalogue in [`PROTECTED_RESOURCES`], which is the
/// `ResourceType` enum's values joined to the ARN form of each kind.
fn resource_type_rows() -> Vec<Option<&'static str>> {
    PROTECTED_RESOURCES
        .iter()
        .map(|(_, _, resource_type)| Some(*resource_type))
        .chain([None])
        .collect()
}

/// A web ACL nothing is bound to reports an empty list for every resource type —
/// whatever its scope, whatever its rules, and with another web ACL's bindings in
/// the store.
#[tokio::test]
async fn an_unassociated_web_acl_reports_an_empty_list_for_every_resource_type() {
    let waf = Waf::new();

    // One other web ACL holds a binding of every documented kind, so an empty
    // answer below is about the web ACL that was NAMED and not about an empty
    // store.
    let (_busy_id, busy_arn, _busy_token) =
        create_web_acl(&waf, "REGIONAL", "bound-acl", json!([])).await;
    for (case, resource_arn, _) in PROTECTED_RESOURCES {
        waf.ok(
            "AssociateWebACL",
            json!({ "ResourceArn": resource_arn, "WebACLArn": busy_arn }),
        )
        .await;
        let bound = web_acl_for_resource(&waf, resource_arn).await;
        assert!(
            bound.is_some(),
            "case {case:?}: the other web ACL must hold its binding, got {bound:?}"
        );
    }

    // The subjects: a web ACL differs by scope and by how much it carries, and none
    // of them is bound to anything.
    let subjects: &[(&str, &str, &str, Value)] = &[
        (
            "a REGIONAL web ACL with no rules",
            "REGIONAL",
            "quiet-regional-acl",
            json!([]),
        ),
        (
            "a REGIONAL web ACL with a rule tree",
            "REGIONAL",
            "quiet-regional-rules-acl",
            json!([rule("block-sqli", sqli_statement())]),
        ),
        (
            // A distribution is bound through its own configuration, so this
            // service holds no binding that could name a CLOUDFRONT web ACL.
            "a CLOUDFRONT web ACL",
            "CLOUDFRONT",
            "quiet-global-acl",
            json!([]),
        ),
    ];

    for (case, scope, name, rules) in subjects {
        let (_id, web_acl_arn, _token) = create_web_acl(&waf, scope, name, rules.clone()).await;
        for resource_type in resource_type_rows() {
            let row = format!("{case} listing {resource_type:?}");
            // `listed_resources` panics unless the answer carries a `ResourceArns`
            // ARRAY, so an empty listing answering a MISSING member fails here
            // rather than reading as empty.
            let listed = listed_resources(&waf, &web_acl_arn, resource_type).await;
            assert!(
                listed.is_empty(),
                "case {row:?}: an unassociated web ACL must report no resource, got {listed:?}"
            );
        }
    }

    // Nothing the listings above did took the other web ACL's bindings away.
    for (case, resource_arn, resource_type) in PROTECTED_RESOURCES {
        let listed = listed_resources(&waf, &busy_arn, Some(resource_type)).await;
        assert_eq!(
            listed,
            vec![(*resource_arn).to_string()],
            "case {case:?}: {resource_type} must still list {resource_arn}, got {listed:?}"
        );
    }
}

/// A listing reports the bindings of the web ACL it names, of the kind it names,
/// and repeating it answers the same thing: a report changes nothing a later caller
/// sees.
#[tokio::test]
async fn a_listing_reports_one_web_acls_bindings_of_one_kind_and_changes_nothing() {
    let waf = Waf::new();
    let (_first_id, first_arn, _first_token) =
        create_web_acl(&waf, "REGIONAL", "reporting-acl", json!([])).await;
    let (_second_id, second_arn, _second_token) =
        create_web_acl(&waf, "REGIONAL", "unreported-acl", json!([])).await;

    // Two bindings of two different kinds on the first web ACL: the load balancer
    // the call defaults to, and one other kind.
    let load_balancer = PROTECTED_RESOURCES
        .iter()
        .find(|(_, _, resource_type)| *resource_type == "APPLICATION_LOAD_BALANCER")
        .expect("the catalogue holds the load balancer kind");
    let other = PROTECTED_RESOURCES
        .iter()
        .find(|(_, _, resource_type)| *resource_type != "APPLICATION_LOAD_BALANCER")
        .expect("the catalogue holds another kind");
    for (_, resource_arn, _) in [load_balancer, other] {
        waf.ok(
            "AssociateWebACL",
            json!({ "ResourceArn": resource_arn, "WebACLArn": first_arn }),
        )
        .await;
    }

    // Each kind reports its own resource, the defaulted request reports the load
    // balancer, and every other kind reports nothing.
    for resource_type in resource_type_rows() {
        let row = format!("listing {resource_type:?}");
        let expected: Vec<String> = match resource_type {
            None => vec![load_balancer.1.to_string()],
            Some(named) if named == load_balancer.2 => vec![load_balancer.1.to_string()],
            Some(named) if named == other.2 => vec![other.1.to_string()],
            Some(_) => Vec::new(),
        };
        // Three reads in a row: a report is repeatable, so the second and third
        // answer what the first did.
        for attempt in 0..3 {
            let listed = listed_resources(&waf, &first_arn, resource_type).await;
            assert_eq!(
                listed, expected,
                "case {row:?} read {attempt}: expected {expected:?}, got {listed:?}"
            );
        }
        // The web ACL that was not named reports nothing, whatever is bound to the
        // other one.
        let unreported = listed_resources(&waf, &second_arn, resource_type).await;
        assert!(
            unreported.is_empty(),
            "case {row:?}: the web ACL that was not named must report nothing, \
             got {unreported:?}"
        );
    }

    // The association reads still see what they saw before any listing ran.
    for (case, resource_arn, _) in [load_balancer, other] {
        let bound = web_acl_for_resource(&waf, resource_arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the resource must still report a web ACL"));
        assert_eq!(
            bound["ARN"].as_str(),
            Some(first_arn.as_str()),
            "case {case:?}: expected the web ACL {first_arn} after the listings, got {bound}"
        );
    }
}
