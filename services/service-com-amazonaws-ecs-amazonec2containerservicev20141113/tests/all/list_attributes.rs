// SPDX-License-Identifier: CC0-1.0

//! `ListAttributes`, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`].
//!
//! The rule under test: `ListAttributes` reports, for the cluster the request names (or
//! the default cluster when it names none), every attribute STORED against each resource
//! of the requested target type, each carrying the full ARN of the resource it is on —
//! narrowed by the exact `attributeName` and `attributeValue` the request asks for and
//! paged in a stable order.
//!
//! The docs that rule comes from:
//!
//! * `ListAttributes`: "When you specify a target type and cluster, ListAttributes
//!   returns a list of attribute objects, one for each attribute on each resource. You
//!   can filter the list of results to a single attribute name to only return results
//!   that have that name. You can also filter the results by attribute name and value."
//! * `ListAttributes$cluster`: "If you do not specify a cluster, the default cluster is
//!   assumed."
//! * `ListAttributes$maxResults`: "This value can be between 1 and 100. If this parameter
//!   isn't used, then ListAttributes returns up to 100 results and a nextToken value if
//!   applicable."
//! * `ListAttributes$attributeValue`: "You must also specify an attribute name to use
//!   this parameter."
//! * `Attribute$targetType`: "This parameter is required if you use the short form ID for
//!   a resource instead of the full ARN."
//! * `DeregisterContainerInstance`: "Deregistering a container instance removes the
//!   instance from a cluster."

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    create_cluster, identity_document, list, ok, refused, service, string, ACCOUNT, SIGNED_REGION,
};

/// One listed attribute, reduced to what a client keys on: the full ARN of the resource
/// it is on, its name and its value (the empty string for an attribute stored without
/// one).
pub(crate) type Listed = (String, String, String);

/// One row of the registration table: the case name, the EC2 instance id the identity
/// document names and the `attributes` the agent reports for it.
type RegistrationRow = (&'static str, &'static str, Value);

/// One row of the filter table: the case name, the `attributeName` and `attributeValue`
/// the request carries, and the indices into the full listing it must answer with.
type FilterRow = (&'static str, Option<&'static str>, Option<&'static str>, Vec<usize>);

/// Register one container instance carrying `attributes` and return its ARN.
pub(crate) async fn register_with_attributes(
    router: &MultiServiceRouter,
    cluster: &str,
    instance_id: &str,
    attributes: &Value,
    case: &str,
) -> String {
    let body = ok(
        router,
        "RegisterContainerInstance",
        json!({
            "cluster": cluster,
            "instanceIdentityDocument": identity_document(instance_id),
            "instanceIdentityDocumentSignature":
                crate::ecs_wire::identity_document_signature(),
            "attributes": attributes,
        }),
        case,
    )
    .await;
    string(&body["containerInstance"], "containerInstanceArn", case)
}

/// The attributes and the `nextToken` one `ListAttributes` call answers with.
///
/// Each entry is checked to carry no `targetType`: the listing reports the full ARN of
/// the target, which is the form `Attribute$targetType` is NOT required for.
pub(crate) async fn list_attributes(
    router: &MultiServiceRouter,
    request: Value,
    case: &str,
) -> (Vec<Listed>, Option<String>) {
    let body = ok(router, "ListAttributes", request, case).await;
    let attributes = list(&body, "attributes", case)
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
        .collect();
    let next_token = body["nextToken"].as_str().map(str::to_string);
    (attributes, next_token)
}

/// Every attribute of a listing, walked page by page with `max_results` per page: the
/// concatenated entries and the number of pages it took.
async fn walk_pages(
    router: &MultiServiceRouter,
    cluster: &str,
    max_results: usize,
    case: &str,
) -> (Vec<Listed>, usize) {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({ "cluster": cluster, "targetType": "container-instance",
                                  "maxResults": max_results });
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (page, next) = list_attributes(router, request, case).await;
        assert!(
            page.len() <= max_results,
            "case {case:?}: expected at most {max_results} entries in a page, got {}",
            page.len()
        );
        pages += 1;
        collected.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            pages < 50,
            "case {case:?}: the walk did not terminate after {pages} pages"
        );
    }
    (collected, pages)
}

/// The registrations the listing tests share. The rows differ along what the rule
/// generalises over: how many attributes a resource carries (none, one, several),
/// whether an attribute has a value at all, and whether two resources share an
/// attribute NAME with different values.
fn registrations() -> [RegistrationRow; 4] {
    [
        (
            "two valued attributes",
            "i-0aaa0000000000001",
            json!([
                { "name": "ecs.os-type", "value": "linux" },
                { "name": "stack", "value": "production" },
            ]),
        ),
        (
            "a valueless attribute beside valued ones",
            "i-0bbb0000000000002",
            json!([
                { "name": "ecs.capability.external" },
                { "name": "ecs.os-type", "value": "windows" },
                { "name": "stack", "value": "staging" },
            ]),
        ),
        ("no attributes at all", "i-0ccc0000000000003", json!([])),
        (
            "an attribute name with a slash and a period",
            "i-0ddd0000000000004",
            json!([{ "name": "fleet.id/west", "value": "a-1" }]),
        ),
    ]
}

/// Register every row of [`registrations`] into `cluster` and return the full listing
/// those registrations imply, in the order `ListAttributes` must report it: by target
/// ARN, then by attribute name.
async fn seed(router: &MultiServiceRouter, cluster: &str, case: &str) -> Vec<Listed> {
    let mut expected: Vec<Listed> = Vec::new();
    for (row, instance_id, attributes) in registrations() {
        let case = format!("{case}: {row}");
        let arn = register_with_attributes(router, cluster, instance_id, &attributes, &case).await;
        for attribute in attributes.as_array().expect("the row is a list") {
            expected.push((
                arn.clone(),
                attribute["name"]
                    .as_str()
                    .expect("the row names the attribute")
                    .to_string(),
                attribute["value"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    expected.sort();
    expected
}

#[tokio::test]
async fn registered_attributes_are_listed_against_their_target_arn_and_filtered() {
    let router = service();
    let case = "filtering";
    create_cluster(&router, "demo", case).await;
    let expected = seed(&router, "demo", case).await;

    let (all, next) = list_attributes(
        &router,
        json!({ "cluster": "demo", "targetType": "container-instance" }),
        case,
    )
    .await;
    assert_eq!(
        all, expected,
        "case {case:?}: expected the stored attributes of the cluster's resources, got {all:?}"
    );
    assert_eq!(
        next, None,
        "case {case:?}: expected no nextToken for a complete listing, got {next:?}"
    );
    // Every entry names its target by full ARN, so a client can go straight from the
    // listing to the resource.
    for (target_id, name, _) in &all {
        assert!(
            target_id.starts_with(&format!(
                "arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:container-instance/demo/"
            )),
            "case {case:?}: expected a container-instance ARN for {name:?}, got {target_id:?}"
        );
    }

    // The indices index into `all`, which is `expected`: the rule is that the filtered
    // listing is a SUBSET of the full one, selected by exact name and value.
    let index_of = |name: &str, value: &str| -> usize {
        all.iter()
            .position(|(_, entry_name, entry_value)| entry_name == name && entry_value == value)
            .unwrap_or_else(|| panic!("the seeded listing holds {name:?}={value:?}"))
    };
    let cases: [FilterRow; 8] = [
        ("no filter", None, None, (0..all.len()).collect()),
        (
            "one name shared by two targets",
            Some("stack"),
            None,
            vec![index_of("stack", "production"), index_of("stack", "staging")],
        ),
        (
            "name and value",
            Some("stack"),
            Some("production"),
            vec![index_of("stack", "production")],
        ),
        (
            "the same name, the other value",
            Some("ecs.os-type"),
            Some("windows"),
            vec![index_of("ecs.os-type", "windows")],
        ),
        (
            "a name no attribute carries",
            Some("ecs.vpc-id"),
            None,
            Vec::new(),
        ),
        (
            "a value no attribute of that name carries",
            Some("stack"),
            Some("development"),
            Vec::new(),
        ),
        (
            "a valueless attribute, by name",
            Some("ecs.capability.external"),
            None,
            vec![index_of("ecs.capability.external", "")],
        ),
        (
            "a valueless attribute never matches a value filter",
            Some("ecs.capability.external"),
            Some("linux"),
            Vec::new(),
        ),
    ];

    for (row, attribute_name, attribute_value, wanted) in cases {
        let mut request = json!({ "cluster": "demo", "targetType": "container-instance" });
        if let Some(attribute_name) = attribute_name {
            request["attributeName"] = json!(attribute_name);
        }
        if let Some(attribute_value) = attribute_value {
            request["attributeValue"] = json!(attribute_value);
        }
        let mut wanted: Vec<Listed> = wanted.into_iter().map(|index| all[index].clone()).collect();
        wanted.sort();
        let (listed, _) = list_attributes(&router, request, row).await;
        assert_eq!(
            listed, wanted,
            "case {row:?}: expected the attributes matching name={attribute_name:?} \
             value={attribute_value:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_pages_in_a_stable_order() {
    let router = service();
    let case = "paging";
    create_cluster(&router, "demo", case).await;
    let expected = seed(&router, "demo", case).await;
    let total = expected.len();

    // `maxResults`: "This value can be between 1 and 100" — both published bounds, and
    // the page sizes between them that split this listing differently: one entry per
    // page, a page that divides it, a page that does not, and pages at and past its
    // length.
    for max_results in [1, 2, 3, total, total + 1, 100] {
        let case = format!("{case}: maxResults={max_results}");
        let (walked, pages) = walk_pages(&router, "demo", max_results, &case).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the whole listing across its pages, got {walked:?}"
        );
        let wanted_pages = total.div_ceil(max_results).max(1);
        assert_eq!(
            pages, wanted_pages,
            "case {case:?}: expected {wanted_pages} pages for {total} attributes, got {pages}"
        );
    }
}

#[tokio::test]
async fn the_default_cluster_is_assumed_and_a_deregistered_resource_drops_out() {
    let router = service();
    let case = "default cluster";
    create_cluster(&router, "default", case).await;
    let attributes = json!([{ "name": "ecs.os-type", "value": "linux" }]);
    let arn =
        register_with_attributes(&router, "default", "i-0eee0000000000005", &attributes, case).await;

    // No `cluster`: "If you do not specify a cluster, the default cluster is assumed."
    let (listed, _) = list_attributes(&router, json!({ "targetType": "container-instance" }), case)
        .await;
    assert_eq!(
        listed,
        vec![(arn.clone(), "ecs.os-type".to_string(), "linux".to_string())],
        "case {case:?}: expected the default cluster's stored attribute, got {listed:?}"
    );

    // The same cluster named by its full ARN resolves to the same listing.
    let (by_arn, _) = list_attributes(
        &router,
        json!({
            "cluster": format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/default"),
            "targetType": "container-instance",
        }),
        case,
    )
    .await;
    assert_eq!(
        by_arn, listed,
        "case {case:?}: expected the cluster ARN to name the same cluster, got {by_arn:?}"
    );

    // "Deregistering a container instance removes the instance from a cluster", so its
    // attributes are no longer attributes of a resource IN the cluster.
    ok(
        &router,
        "DeregisterContainerInstance",
        json!({ "cluster": "default", "containerInstance": arn }),
        case,
    )
    .await;
    let (after, next) =
        list_attributes(&router, json!({ "targetType": "container-instance" }), case).await;
    assert!(
        after.is_empty() && next.is_none(),
        "case {case:?}: expected a deregistered resource to drop out of the listing, \
         got {after:?} with {next:?}"
    );
}

#[tokio::test]
async fn an_unusable_request_is_refused() {
    let router = service();
    let case = "refusals";
    create_cluster(&router, "demo", case).await;
    register_with_attributes(
        &router,
        "demo",
        "i-0fff0000000000006",
        &json!([{ "name": "stack", "value": "production" }]),
        case,
    )
    .await;

    // A cluster that was never created — including the default cluster, which this
    // router has none of.
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
        let mut request = json!({ "targetType": "container-instance" });
        if let Value::String(cluster) = &cluster {
            request["cluster"] = json!(cluster);
        }
        refused(
            &router,
            "ListAttributes",
            request,
            "ClusterNotFoundException",
            row,
        )
        .await;
    }

    // `maxResults`: "This value can be between 1 and 100" — one past each published
    // bound is refused, and each bound itself is accepted above in the paging suite.
    for max_results in [0_i64, -1, 101] {
        let row = format!("maxResults={max_results}");
        refused(
            &router,
            "ListAttributes",
            json!({
                "cluster": "demo",
                "targetType": "container-instance",
                "maxResults": max_results,
            }),
            "InvalidParameterException",
            &row,
        )
        .await;
    }

    // "You must also specify an attribute name to use this parameter."
    refused(
        &router,
        "ListAttributes",
        json!({
            "cluster": "demo",
            "targetType": "container-instance",
            "attributeValue": "production",
        }),
        "InvalidParameterException",
        "a value filter with no name",
    )
    .await;

    // A token this operation never minted names no page.
    for token in ["not-a-token", "|", "|stack", "arn:aws:ecs:::ci/demo/1|"] {
        let row = format!("nextToken={token:?}");
        let body = refused(
            &router,
            "ListAttributes",
            json!({
                "cluster": "demo",
                "targetType": "container-instance",
                "nextToken": token,
            }),
            "ClientException",
            &row,
        )
        .await;
        assert_eq!(
            body["message"].as_str(),
            Some("Invalid nextToken."),
            "case {row:?}: expected the one sentence ECS answers for an unusable token, \
             got {body}"
        );
    }
}
