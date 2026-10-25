// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resolution `StartCostEstimation` performs: every entry of
//! the `CostEstimationResourceCollectionFilter` a request submits has to select at
//! least one resource the account holds, or there is nothing to price and the start is
//! refused with the modelled `ResourceNotFoundException` (HTTP 404), creating no
//! estimation.
//!
//! The suites here drive the service through the shared [`Emulator`] harness in
//! `configuration_lifecycle` and the cost-estimation calls in `cost_estimation`, so
//! the assertions are about the wire behaviour rather than handler internals.

use serde_json::{json, Value};
use std::time::Duration;

use service_com_amazonaws_devopsguru_capstonecontrolplaneservice::state::{
    COST_ESTIMATION_DURATION, MONITORED_STACK_RESOURCE_TYPE, MONITORED_TAG_RESOURCE_TYPE,
};

use crate::configuration_lifecycle::{cloud_formation, tags, Emulator};
use crate::cost_estimation::{
    add_coverage, get_cost_estimation, reported_status, reported_total_cost, reports_an_estimation,
    start_cost_estimation, MONEY_EPSILON,
};

/// `ServiceResourceCost.Type` / `ResourceNotFoundException.ResourceType` for the stack
/// half of a collection, read from the service crate so the wire value a row expects
/// cannot drift from the one the handler reports.
const STACK_TYPE: &str = MONITORED_STACK_RESOURCE_TYPE;
/// The same for the tag half.
const TAG_TYPE: &str = MONITORED_TAG_RESOURCE_TYPE;

/// What one row of the resolution table expects back.
#[derive(Debug)]
enum Expect {
    /// The scope selects a resource the account holds, so the start is accepted.
    Accepted,
    /// The scope selects none, so it is refused with `ResourceNotFoundException`
    /// naming this `ResourceType`, a `ResourceId` carrying this value, and a message
    /// saying which half of the collection could not be resolved.
    NotFound {
        resource_type: &'static str,
        id_contains: &'static str,
        message_names: &'static str,
    },
    /// The entry is outside the shape the model allows, so the request never reaches
    /// resolution: it is refused with `ValidationException` (400).
    Invalid,
}

struct ScopeCase {
    name: &'static str,
    /// The resources the account holds, each declared with one `ADD`
    /// `UpdateResourceCollection` before the start.
    held: Vec<Value>,
    /// The collection the start submits.
    collection: Value,
    expect: Expect,
}

/// RULE: a cost estimate prices resources, so every entry of the collection a start
/// submits has to select at least one resource the account holds — a stack name is
/// matched exactly, a tag entry by its app-boundary key AND one of its values, both
/// case-sensitively and both exactly, because nothing in the model expands one value
/// into others — and an entry that selects none names nothing to price: the start is
/// refused with `ResourceNotFoundException` (HTTP 404) naming the resource that was not
/// found, and no estimation is created.
///
/// The rows differ along which half of the collection is named, whether the account
/// holds a resource the entry selects (not at all, under another key, under the same
/// key with another value, exactly, in different letter case, as one of several), and
/// the form of the value named (a literal, the empty tag value, no value at all, and
/// the `*` the member patterns admit, which is matched as the value it is).
#[tokio::test]
async fn a_start_resolves_every_entry_of_its_collection_against_the_account() {
    let cases = vec![
        ScopeCase {
            name: "a tag no resource of the account carries",
            held: vec![],
            collection: tags(&[("Devops-guru-cost", &["never-applied"])]),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "never-applied",
            },
        },
        ScopeCase {
            name: "a value held under another app boundary key",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: tags(&[("Devops-guru-other", &["production"])]),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "Devops-guru-other",
            },
        },
        ScopeCase {
            name: "a value nothing carries under the held key",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: tags(&[("Devops-guru-app", &["staging"])]),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "staging",
            },
        },
        ScopeCase {
            name: "the tag key and value the account holds",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: tags(&[("Devops-guru-app", &["production"])]),
            expect: Expect::Accepted,
        },
        ScopeCase {
            name: "one of the several values the held key carries",
            held: vec![tags(&[("Devops-guru-app", &["production", "staging"])])],
            collection: tags(&[("Devops-guru-app", &["staging"])]),
            expect: Expect::Accepted,
        },
        ScopeCase {
            // Tag values are case-sensitive: see
            // `smithy_query(query="doc",shape="TagCostEstimationResourceCollectionFilter",member="TagValues")`.
            name: "a held tag value in different letter case",
            held: vec![tags(&[("Devops-guru-app", &["Production"])])],
            collection: tags(&[("Devops-guru-app", &["production"])]),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "production",
            },
        },
        ScopeCase {
            // "Omitting the tag value is the same as using an empty string"
            // (`TagCostEstimationResourceCollectionFilter.TagValues` documentation),
            // and an empty value is a value, not a wildcard.
            name: "the empty tag value, which the account holds",
            held: vec![tags(&[("Devops-guru-app", &[""])])],
            collection: tags(&[("Devops-guru-app", &[""])]),
            expect: Expect::Accepted,
        },
        ScopeCase {
            name: "the empty tag value against a key holding another value",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: tags(&[("Devops-guru-app", &[""])]),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "Devops-guru-app",
            },
        },
        ScopeCase {
            // A value is a value even when it is the empty string, so an entry naming
            // NO value selects nothing at all — and the model does not let one reach
            // the handler: `CostEstimationTagValues` is fixed at one item
            // (`smithy_query(query="shape",shape="CostEstimationTagValues")`).
            name: "a held key with no tag value at all",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: json!({
                "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": [] }],
            }),
            expect: Expect::Invalid,
        },
        ScopeCase {
            // `*` is a value `TagValue`'s pattern admits as a whole value
            // (`smithy_query(query="shape",shape="CostEstimationTagValues")`); nothing
            // in the model says it stands for the other values, so it selects the
            // resource carrying it and no other.
            name: "the `*` tag value where the account holds another value",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: tags(&[("Devops-guru-app", &["*"])]),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "*",
            },
        },
        ScopeCase {
            name: "the `*` tag value, which the account holds",
            held: vec![tags(&[("Devops-guru-app", &["*"])])],
            collection: tags(&[("Devops-guru-app", &["*"])]),
            expect: Expect::Accepted,
        },
        ScopeCase {
            name: "the `*` tag value under a key the account does not hold",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: tags(&[("Devops-guru-absent", &["*"])]),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "Devops-guru-absent",
            },
        },
        ScopeCase {
            name: "a stack the account does not hold",
            held: vec![],
            collection: cloud_formation(&["absent-stack"]),
            expect: Expect::NotFound {
                resource_type: STACK_TYPE,
                message_names: "stack name",
                id_contains: "absent-stack",
            },
        },
        ScopeCase {
            name: "a stack other than the one the account holds",
            held: vec![cloud_formation(&["held-stack"])],
            collection: cloud_formation(&["other-stack"]),
            expect: Expect::NotFound {
                resource_type: STACK_TYPE,
                message_names: "stack name",
                id_contains: "other-stack",
            },
        },
        ScopeCase {
            name: "one of the several stacks the account holds",
            held: vec![cloud_formation(&["held-stack", "spare-stack"])],
            collection: cloud_formation(&["spare-stack"]),
            expect: Expect::Accepted,
        },
        ScopeCase {
            // `StackName`'s pattern admits `*` too
            // (`smithy_query(query="shape",shape="CostEstimationStackNames")`), and it
            // is matched as the name it is here as well.
            name: "a `*` stack name where the account holds a named stack",
            held: vec![cloud_formation(&["held-stack"])],
            collection: cloud_formation(&["*"]),
            expect: Expect::NotFound {
                resource_type: STACK_TYPE,
                message_names: "stack name",
                id_contains: "*",
            },
        },
        ScopeCase {
            name: "a `*` stack name, which the account holds",
            held: vec![cloud_formation(&["*"])],
            collection: cloud_formation(&["*"]),
            expect: Expect::Accepted,
        },
        ScopeCase {
            name: "both halves, each selecting a held resource",
            held: vec![
                cloud_formation(&["held-stack"]),
                tags(&[("Devops-guru-app", &["production"])]),
            ],
            collection: json!({
                "CloudFormation": { "StackNames": ["held-stack"] },
                "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": ["production"] }],
            }),
            expect: Expect::Accepted,
        },
        ScopeCase {
            name: "both halves where only the tag half selects a held resource",
            held: vec![tags(&[("Devops-guru-app", &["production"])])],
            collection: json!({
                "CloudFormation": { "StackNames": ["absent-stack"] },
                "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": ["production"] }],
            }),
            expect: Expect::NotFound {
                resource_type: STACK_TYPE,
                message_names: "stack name",
                id_contains: "absent-stack",
            },
        },
        ScopeCase {
            name: "both halves where only the stack half selects a held resource",
            held: vec![cloud_formation(&["held-stack"])],
            collection: json!({
                "CloudFormation": { "StackNames": ["held-stack"] },
                "Tags": [{ "AppBoundaryKey": "Devops-guru-app", "TagValues": ["production"] }],
            }),
            expect: Expect::NotFound {
                resource_type: TAG_TYPE,
                message_names: "tag",
                id_contains: "production",
            },
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        for held in &case.held {
            add_coverage(&emulator, held.clone()).await;
        }

        let reply = start_cost_estimation(&emulator, &case.collection, None).await;
        let read = get_cost_estimation(&emulator, None).await;

        match case.expect {
            Expect::Accepted => {
                assert_eq!(
                    reply.status, 200,
                    "case {:?}: expected the start to be accepted with 200, got {} {}",
                    case.name, reply.status, reply.raw
                );
                assert!(
                    reports_an_estimation(&read),
                    "case {:?}: expected the created estimation to read back, got {}",
                    case.name,
                    read.raw
                );
                assert_eq!(
                    read.at("/ResourceCollection"),
                    Some(&case.collection),
                    "case {:?}: expected the submitted collection to round-trip, got {}",
                    case.name,
                    read.raw
                );
            }
            Expect::NotFound {
                resource_type,
                id_contains,
                message_names,
            } => {
                assert_eq!(
                    reply.status, 404,
                    "case {:?}: expected HTTP 404, got {} {}",
                    case.name, reply.status, reply.raw
                );
                assert!(
                    reply.names_error("ResourceNotFoundException"),
                    "case {:?}: expected ResourceNotFoundException, got {}",
                    case.name,
                    reply.raw
                );
                assert_eq!(
                    reply.at("/ResourceType").and_then(Value::as_str),
                    Some(resource_type),
                    "case {:?}: expected the refusal to name resource type {resource_type}, got {}",
                    case.name,
                    reply.raw
                );
                let resource_id = reply
                    .at("/ResourceId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    resource_id.contains(id_contains),
                    "case {:?}: expected the refused ResourceId to name {id_contains:?}, got {resource_id:?} in {}",
                    case.name,
                    reply.raw
                );
                let message = reply
                    .at("/Message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert!(
                    message.contains(message_names),
                    "case {:?}: expected the refusal to say the {message_names} could not be resolved, got {message:?} in {}",
                    case.name,
                    reply.raw
                );
                assert_eq!(
                    reported_status(&read),
                    None,
                    "case {:?}: a refused start must create no estimation, got {}",
                    case.name,
                    read.raw
                );
                assert!(
                    (reported_total_cost(&read) - 0.0).abs() < MONEY_EPSILON,
                    "case {:?}: expected TotalCost 0 with no estimation, got {}",
                    case.name,
                    read.raw
                );
            }
            Expect::Invalid => {
                assert_eq!(
                    reply.status, 400,
                    "case {:?}: expected HTTP 400, got {} {}",
                    case.name, reply.status, reply.raw
                );
                assert!(
                    reply.names_error("ValidationException"),
                    "case {:?}: expected ValidationException, got {}",
                    case.name,
                    reply.raw
                );
                assert_eq!(
                    reported_status(&read),
                    None,
                    "case {:?}: a refused start must create no estimation, got {}",
                    case.name,
                    read.raw
                );
            }
        }
    }
}

/// RULE: resolution is against the resources the account holds AT THE TIME OF THE
/// START, so one and the same request is refused with `ResourceNotFoundException`
/// (404) while the account holds nothing its collection selects, accepted once that
/// resource is recorded, and refused again after it is removed; a retry under the
/// `ClientToken` that already started an estimation is answered from that estimation
/// whatever the account holds by then; and because a refused start creates nothing, the
/// estimate an earlier accepted start created is still the one `GetCostEstimation`
/// reports.
///
/// The steps differ along the order of the calls relative to the `ADD` and the
/// `REMOVE`, along whether the start repeats a token that already started an
/// estimation, and along whether an estimate already exists when the start is refused.
#[tokio::test]
async fn a_scope_resolves_against_the_resources_held_when_the_start_is_made() {
    let emulator = Emulator::new();
    let collection = tags(&[("Devops-guru-cost", &["estimated"])]);

    // Nothing is held yet: there is no resource to price.
    let before = start_cost_estimation(&emulator, &collection, Some("scopetokenone")).await;
    assert_eq!(
        before.status, 404,
        "case {:?}: expected HTTP 404 before the resource is held, got {} {}",
        "start before the resource is held", before.status, before.raw
    );
    let empty = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        reported_status(&empty),
        None,
        "case {:?}: a refused start must leave no estimation behind, got {}",
        "read after the refused start",
        empty.raw
    );

    // The same request, once the account holds a resource the collection selects.
    add_coverage(&emulator, tags(&[("Devops-guru-cost", &["estimated"])])).await;
    let accepted = start_cost_estimation(&emulator, &collection, Some("scopetokentwo")).await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected the same start to be accepted once the resource is held, got {} {}",
        "start after the ADD", accepted.status, accepted.raw
    );
    let created = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        created.at("/ResourceCollection"),
        Some(&collection),
        "case {:?}: expected the accepted collection to be reported, got {}",
        "read after the ADD",
        created.raw
    );

    // The resource is given up again, so the same collection selects nothing once more.
    let removed = emulator
        .update_resource_collection("REMOVE", tags(&[("Devops-guru-cost", &["estimated"])]))
        .await;
    assert_eq!(
        removed.status, 200,
        "case {:?}: expected the REMOVE to be accepted, got {} {}",
        "remove the held resource", removed.status, removed.raw
    );

    // A RETRY of the request that was already accepted: it names the estimation its
    // token started, so it is answered from that estimation rather than resolved again.
    let retry = start_cost_estimation(&emulator, &collection, Some("scopetokentwo")).await;
    assert_eq!(
        retry.status, 200,
        "case {:?}: expected the retry to be answered from the estimation its token started, got {} {}",
        "retry under the accepted token after the REMOVE", retry.status, retry.raw
    );
    let retried = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        retried.at("/ResourceCollection"),
        created.at("/ResourceCollection"),
        "case {:?}: a retry must not change the estimation it names, got {}",
        "read after the retry",
        retried.raw
    );

    // Let that estimation finish, so the next start is refused for its scope rather
    // than for the estimation still being created.
    tokio::time::sleep(COST_ESTIMATION_DURATION + Duration::from_millis(250)).await;

    // A NEW request for the same collection, under its own token: nothing selects a
    // held resource any more, so it is refused.
    let after = start_cost_estimation(&emulator, &collection, Some("scopetokenthree")).await;
    assert_eq!(
        after.status, 404,
        "case {:?}: expected HTTP 404 once the resource is given up, got {} {}",
        "start after the REMOVE", after.status, after.raw
    );
    let settled = get_cost_estimation(&emulator, None).await;
    assert_eq!(
        settled.at("/ResourceCollection"),
        created.at("/ResourceCollection"),
        "case {:?}: a refused start must leave the existing estimate alone, got {}",
        "read after the REMOVE",
        settled.raw
    );
    assert_eq!(
        reported_status(&settled),
        Some("COMPLETED"),
        "case {:?}: expected the earlier estimate to still be reported, got {}",
        "read after the REMOVE",
        settled.raw
    );
}
