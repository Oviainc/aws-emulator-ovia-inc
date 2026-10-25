// SPDX-License-Identifier: CC0-1.0

//! How the ARN-addressed tagging surface RESOLVES the `resourceArn` it is handed.
//!
//! The rule under test, on `TagResource`, `ListTagsForResource` and `UntagResource` —
//! the three operations that address a resource by `resourceArn`: an environment is a
//! REGIONAL resource that ceases to exist once it reaches `DELETED`, so an ARN resolves
//! only to an environment this service holds LIVE in the Region the request was signed
//! for. Any other ARN names no resource of the endpoint it was sent to, and draws the
//! tagging surface's own not-found refusal (HTTP 400 `ResourceNotFoundException`, the
//! fixed resource-agnostic message, the request's `resourceArn` echoed — the body
//! `tests/all/tagging_not_found.rs` owns) while writing nothing.
//!
//! The same rule the id-addressed operations of this crate apply
//! (`tests/all/environment_lifecycle.rs`,
//! `an_environment_is_only_visible_at_the_region_it_was_created_in`), on the surface
//! that addresses the resource by ARN instead.
//!
//! The table spans Regions (every row a different endpoint, so the resolution cannot be
//! a special case of one) and the lifecycle states an environment passes through, so
//! what separates an addressable environment from one that is gone is asserted on both
//! axes. Every test drives the service the way a client does, through the same
//! `MultiServiceRouter` the production router mounts.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice as evs;

use crate::environment_lifecycle::{
    call_in_region, emulator, error_type, settle, Emulator, ACCOUNT,
};

/// The fixed, resource-agnostic text the tagging surface answers a missing resource
/// with (`crate::scaffolded::TAGGING_RESOURCE_NOT_FOUND_MESSAGE`); the ARN travels in
/// the `resourceArn` member instead. Owned by `tests/all/tagging_not_found.rs`.
const NOT_FOUND_MESSAGE: &str = "Resource cannot be found.";

/// The endpoints the Region table spans, one per AWS area so no two share a prefix.
/// Each is a three-segment Region token, the form the modelled `Arn` pattern
/// `^arn:aws:evs:[a-z]{2}-[a-z]+-[0-9]:[0-9]{12}:environment/[a-zA-Z0-9_-]+$` admits
/// (`smithy_query(query="shape",shape="Arn")`), so every ARN below reaches the handler
/// rather than the boundary validator.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-2", "sa-east-1"];

/// The three ARN-addressed tagging operations, each with the rest of its request body —
/// the operations the model binds `resourceArn` to
/// (`smithy_query(query="operation",operation="TagResource,ListTagsForResource,UntagResource")`).
fn tagging_operations() -> Vec<(&'static str, Value)> {
    vec![
        ("TagResource", json!({ "tags": { "team": "platform" } })),
        ("ListTagsForResource", json!({})),
        ("UntagResource", json!({ "tagKeys": ["team"] })),
    ]
}

/// One tagging request: the ARN it names, plus the rest of the operation's body.
fn tagging_request(resource_arn: &str, extra: &Value) -> Value {
    let mut request = json!({ "resourceArn": resource_arn });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            request[name] = value.clone();
        }
    }
    request
}

/// Create an environment at `region`'s endpoint carrying `tags`, and return its
/// `(environmentId, environmentArn)`.
async fn create_tagged_in_region(
    router: &Emulator,
    region: &str,
    name: &str,
    tags: Value,
) -> (String, String) {
    let mut request = router.create_request(Some(name));
    request["tags"] = tags;
    let (status, body) = call_in_region(router, region, "CreateEnvironment", &request).await;
    assert_eq!(
        status, 200,
        "create {name:?} in {region}: expected 200, got {status} {body}"
    );
    let environment = &body["environment"];
    let id = environment["environmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name:?} in {region}: no environmentId in {body}"))
        .to_string();
    let arn = environment["environmentArn"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name:?} in {region}: no environmentArn in {body}"))
        .to_string();
    assert_eq!(
        arn,
        format!("arn:aws:evs:{region}:{ACCOUNT}:environment/{id}"),
        "create {name:?} in {region}: the ARN must name the Region the caller addressed, \
         got {arn}"
    );
    (id, arn)
}

/// Assert that `operation` on `resource_arn`, sent to `region`'s endpoint, is refused
/// the way the tagging surface refuses an ARN that names no resource of that endpoint.
async fn assert_refused(
    router: &Emulator,
    case: &str,
    region: &str,
    operation: &str,
    extra: &Value,
    resource_arn: &str,
) {
    let request = tagging_request(resource_arn, extra);
    let (status, body) = call_in_region(router, region, operation, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(&body).rsplit(['#', '/']).next().unwrap_or(""),
        "ResourceNotFoundException",
        "case {case:?}: expected a ResourceNotFoundException, got {}",
        error_type(&body)
    );
    assert_eq!(
        body["message"],
        json!(NOT_FOUND_MESSAGE),
        "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {}",
        body["message"]
    );
    assert_eq!(
        body["resourceArn"],
        json!(resource_arn),
        "case {case:?}: expected resourceArn {resource_arn:?}, got {}",
        body["resourceArn"]
    );
}

/// The tags `ListTagsForResource` reports for `resource_arn` at `region`'s endpoint.
async fn reported_tags(router: &Emulator, case: &str, region: &str, resource_arn: &str) -> Value {
    let (status, body) = call_in_region(
        router,
        region,
        "ListTagsForResource",
        &json!({ "resourceArn": resource_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} {body}"
    );
    body["tags"].clone()
}

/// The stored tag map and `modifiedAt` of one environment, read from this service's own
/// store — the only way to see the tags of an environment the tagging surface no longer
/// resolves (the modelled `Environment` shape carries no tags member).
fn stored_tags_and_modified_at(
    router: &Emulator,
    environment_id: &str,
) -> (BTreeMap<String, String>, i64) {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    let environment = store
        .environments
        .get(environment_id)
        .unwrap_or_else(|| panic!("the store holds {environment_id}"));
    (environment.tags.clone(), environment.modified_at_ms)
}

/// The `environmentState` the store holds for one environment, for an assertion that
/// names the state the row set up.
fn stored_state(router: &Emulator, environment_id: &str) -> String {
    let state = evs::state::service_state(&*router.engine)
        .expect("the in-memory engine carries this service's store");
    let store = state.lock();
    store
        .environments
        .get(environment_id)
        .map(|environment| environment.state.clone())
        .unwrap_or_else(|| panic!("the store holds {environment_id}"))
}

#[tokio::test]
async fn a_resource_arn_resolves_only_at_the_endpoint_of_its_own_region() {
    // One emulator holding one environment per endpoint, each tagged with the Region it
    // was created in: so a hit is a hit on the RIGHT record and a miss is a miss on the
    // lookup rather than on an empty store.
    let router = emulator();
    let mut environments: Vec<(&str, String, String)> = Vec::new();
    for region in REGIONS {
        let (id, arn) =
            create_tagged_in_region(&router, region, "region-scoped", json!({ "home": region }))
                .await;
        environments.push((region, id, arn));
    }

    for (home, id, arn) in &environments {
        let before = stored_tags_and_modified_at(&router, id);

        // Sent to any OTHER endpoint, the ARN names no resource: all three operations
        // refuse it, whatever that endpoint itself holds.
        for (foreign, _, _) in environments.iter().filter(|(region, _, _)| region != home) {
            for (operation, extra) in tagging_operations() {
                let case = format!("{operation} on the {home} ARN at the {foreign} endpoint");
                assert_refused(&router, &case, foreign, operation, &extra, arn).await;
            }
        }

        // Nothing those refusals named changed: the stored tag map is the one the create
        // gave the environment, and `modifiedAt` has not moved.
        let after = stored_tags_and_modified_at(&router, id);
        assert_eq!(
            after.0,
            BTreeMap::from([("home".to_string(), (*home).to_string())]),
            "case the {home} environment after the refusals: a refused tagging call must \
             write no tag, got {:?}",
            after.0
        );
        assert_eq!(
            after.1, before.1,
            "case the {home} environment after the refusals: a refused tagging call must \
             not move modifiedAt, got {} after {}",
            after.1, before.1
        );

        // And at its OWN endpoint the very same ARN resolves: the refusals above are the
        // Region scoping, not a blanket refusal.
        let case = format!("the {home} ARN at the {home} endpoint");
        let tags = reported_tags(&router, &case, home, arn).await;
        assert_eq!(
            tags,
            json!({ "home": home }),
            "case {case:?}: expected the tags the create gave it, got {tags}"
        );
        let (status, body) = call_in_region(
            &router,
            home,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": { "team": "platform" } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource expected 200, got {status} {body}"
        );
        let tags = reported_tags(&router, &case, home, arn).await;
        assert_eq!(
            tags,
            json!({ "home": home, "team": "platform" }),
            "case {case:?}: the tag written at its own endpoint must be reported, got {tags}"
        );
        let (status, body) = call_in_region(
            &router,
            home,
            "UntagResource",
            &json!({ "resourceArn": arn, "tagKeys": ["team"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource expected 200, got {status} {body}"
        );
        let tags = reported_tags(&router, &case, home, arn).await;
        assert_eq!(
            tags,
            json!({ "home": home }),
            "case {case:?}: the untagged key must be gone, got {tags}"
        );
    }
}

#[tokio::test]
async fn a_resource_arn_stops_resolving_once_its_environment_reaches_deleted() {
    // Each row: a point in the environment's lifecycle, how to reach it, and whether the
    // tagging surface still resolves its ARN there. Only `DELETED` is gone: "The
    // resource might not be specified correctly, or it may have a `state` of DELETED"
    // (`ResourceNotFoundException` doc) — an environment on its way out is still a
    // resource of the endpoint.
    let rows: Vec<(&str, bool, bool, bool, &str)> = vec![
        ("straight after the create", false, false, false, "CREATING"),
        ("once the create transition settled", true, false, false, "CREATED"),
        ("straight after the delete", true, true, false, "DELETING"),
        ("once the delete transition settled", true, true, true, "DELETED"),
    ];

    for (case, settle_create, delete, settle_delete, expected_state) in rows {
        // A fresh emulator per row: settling the clock settles every environment the
        // store holds, so each row drives its own.
        let router = emulator();
        let (id, arn) =
            create_tagged_in_region(&router, REGIONS[0], "deleted-state", json!({ "home": "a" }))
                .await;
        if settle_create {
            settle(&router);
        }
        if delete {
            let (status, body) = call_in_region(
                &router,
                REGIONS[0],
                "DeleteEnvironment",
                &json!({ "environmentId": id }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteEnvironment expected 200, got {status} {body}"
            );
        }
        if settle_delete {
            settle(&router);
        }
        assert_eq!(
            stored_state(&router, &id),
            expected_state,
            "case {case:?}: the row must set up a {expected_state} environment, got {}",
            stored_state(&router, &id)
        );

        let before = stored_tags_and_modified_at(&router, &id);
        if expected_state == "DELETED" {
            // Gone for addressing purposes: every one of the three operations refuses
            // the ARN, and the stored tag map and `modifiedAt` are untouched.
            for (operation, extra) in tagging_operations() {
                let case = format!("{operation} {case}");
                assert_refused(&router, &case, REGIONS[0], operation, &extra, &arn).await;
            }
            let after = stored_tags_and_modified_at(&router, &id);
            assert_eq!(
                after, before,
                "case {case:?}: a refused tagging call must neither write a tag nor move \
                 modifiedAt, got {after:?} after {before:?}"
            );
        } else {
            // Still a resource of this endpoint: the tags read back, a write lands and
            // an untag removes it again.
            let tags = reported_tags(&router, case, REGIONS[0], &arn).await;
            assert_eq!(
                tags,
                json!({ "home": "a" }),
                "case {case:?}: expected the tags the create gave it, got {tags}"
            );
            for (operation, extra) in tagging_operations() {
                let (status, body) =
                    call_in_region(&router, REGIONS[0], operation, &tagging_request(&arn, &extra))
                        .await;
                assert_eq!(
                    status, 200,
                    "case {operation} {case}: expected 200, got {status} {body}"
                );
            }
            let tags = reported_tags(&router, case, REGIONS[0], &arn).await;
            assert_eq!(
                tags,
                json!({ "home": "a" }),
                "case {case:?}: the tag written and then removed must leave the create's \
                 tags, got {tags}"
            );
        }
    }
}
