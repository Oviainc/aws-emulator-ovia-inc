// SPDX-License-Identifier: CC0-1.0

//! Which class this service answers a REFUSED create or delete with, and what it says.
//!
//! Four rules, each asserted over a table that spans what it generalises over rather
//! than the one request that exposed it:
//!
//! * a cluster name is a LABEL, not an identity — nothing resolves a cluster by name, so
//!   a second create under a live cluster's name mints a second, independent cluster;
//! * a create that only REFERENCES a resource (the parent it attaches to) is refused on
//!   its input with `ValidationException` (400) naming that resource, while
//!   `ResourceNotFoundException` (404) stays with the request that ADDRESSES a resource
//!   of its own (pinned in `tests/all/not_found_message.rs`);
//! * a name already taken inside the parent that scopes it, and a delete of a resource
//!   that still owns dependents, are refusals of the request's input too — 400, not a
//!   transient 409 a retry could win;
//! * a value that breaks the `@pattern` or `@length` declared on a BODY member never
//!   reaches the operation: the request-body validator refuses it with
//!   `BadRequestException` (400), stating the declared regex or bound and the value that
//!   failed it.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, create_cluster, create_control_panel, create_routing_control, enc, error_code, router,
};

/// The ARN prefix this service mints under:
/// `arn:aws:route53-recovery-control::<account>:...`.
const ARN_PREFIX: &str = "arn:aws:route53-recovery-control::000000000000:";

/// Longer than the span the service gives a `PENDING` -> `DEPLOYED` transition, so a
/// request made after it works on a fully deployed tree.
const PAST_DEPLOYMENT: Duration = Duration::from_millis(900);

/// The regex the model declares for every name and token member in this service's
/// request bodies: `__stringMin1Max64PatternS` / `__stringMin0Max256PatternS`
/// (`smithy_query(query="operation",operation="CreateCluster")`).
const NAME_PATTERN: &str = r"^\S+$";

/// The regex the model declares for the ARN members a request body carries:
/// `__stringMin1Max256PatternAZaZ09`
/// (`smithy_query(query="operation",operation="UpdateCluster")`).
const ARN_PATTERN: &str = r"^[A-Za-z0-9:\/_-]*$";

/// The sentence the request-body validator refuses a value outside a declared regex
/// with, restated here independently of the service's own code.
fn regex_clause(pattern: &str, value: &str) -> String {
    format!(
        "Invalid request body : [ECMA 262 regex \"{pattern}\" does not match input string \
         \"{value}\"]"
    )
}

/// The sentence it refuses a string longer than the declared `@length` maximum with.
fn too_long_clause(value: &str, maximum: usize) -> String {
    format!(
        "Invalid request body : [string \"{value}\" is too long (length: {}, maximum allowed: \
         {maximum})]",
        value.chars().count()
    )
}

/// The sentence it refuses a string shorter than the declared `@length` minimum with.
fn too_short_clause(value: &str, minimum: usize) -> String {
    format!(
        "Invalid request body : [string \"{value}\" is too short (length: {}, required minimum: \
         {minimum})]",
        value.chars().count()
    )
}

/// The sentence a create refuses a parent it cannot attach to with: the kind of resource
/// the request referenced and the ARN it sent.
fn does_not_exist(resource: &str, arn: &str) -> String {
    format!("{resource} {arn} does not exist.")
}

/// The sentence a delete is refused with while the addressed resource still owns
/// dependents.
fn cannot_delete(resource: &str, dependents: &str) -> String {
    format!("Cannot delete a {resource} with {dependents}.")
}

/// The sentence a create is refused with when the name it asks for is already taken
/// inside the parent that scopes it.
fn already_exists(resource: &str, name: &str, parent: &str, parent_arn: &str) -> String {
    format!("{resource} {name} already exists in {parent} {parent_arn}.")
}

/// The sentence a delete of the resource a parent is created with is refused with.
fn cannot_delete_default(resource: &str) -> String {
    format!("Cannot delete a default {resource}.")
}

/// The `message` of an error body, under the `message` jsonName the model gives every
/// error shape in this service (`smithy_query(query="shape",shape="ValidationException")`).
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Every `ClusterArn` `ListClusters` currently serves.
async fn listed_cluster_arns(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "GET", "/cluster?MaxResults=1000", None).await;
    assert_eq!(status, 200, "case list clusters: expected 200, got {status} {body:?}");
    body["Clusters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|cluster| {
            cluster["ClusterArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A `ClusterName` is a label: creating a cluster under a name a live cluster already
/// carries yields a SECOND cluster with its own ARN, separately describable, and both are
/// listed under the shared name.
///
/// Rows differ along the name itself — a plain token, the length maximum, a
/// punctuation-heavy name, one differing from another only in case — and along the rest
/// of the request, so the acceptance does not depend on the two calls being identical:
/// one row re-creates with a different `NetworkType`, another with different `Tags`.
#[tokio::test]
async fn a_cluster_name_is_a_label_and_not_an_identity() {
    let router = router();

    // (case, the name, the second request's NetworkType, the second request's Tags)
    let cases: &[(&str, String, Option<&str>, Option<Value>)] = &[
        ("a plain name", "shared".to_string(), None, None),
        (
            "the length maximum",
            "n".repeat(64),
            Some("DUALSTACK"),
            None,
        ),
        (
            "a punctuation-heavy name",
            "stack-2/blue_green:v1".to_string(),
            None,
            Some(json!({ "stack": "blue" })),
        ),
        (
            "a name differing only in case",
            "SHARED".to_string(),
            Some("IPV4"),
            Some(json!({ "team": "arc" })),
        ),
    ];

    for (case, name, network_type, tags) in cases {
        let first = create_cluster(&router, json!({ "ClusterName": name })).await;
        let mut body = json!({ "ClusterName": name });
        if let Some(network_type) = network_type {
            body["NetworkType"] = json!(network_type);
        }
        if let Some(tags) = tags {
            body["Tags"] = tags.clone();
        }
        let second = create_cluster(&router, body).await;

        let first_arn = first["ClusterArn"].as_str().unwrap_or_default().to_string();
        let second_arn = second["ClusterArn"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            first_arn, second_arn,
            "case {case}: the second cluster must have its own ARN, got {second_arn:?}"
        );
        assert_eq!(
            second["Name"], json!(name),
            "case {case}: expected the requested name, got {:?}",
            second["Name"]
        );

        // Both are independently describable, each answering its own ARN under the one
        // shared name.
        for (which, arn) in [("first", &first_arn), ("second", &second_arn)] {
            let (status, read) = call(&router, "GET", &format!("/cluster/{}", enc(arn)), None).await;
            assert_eq!(
                status, 200,
                "case {case} ({which}): expected DescribeCluster 200, got {status} {read:?}"
            );
            assert_eq!(
                read["Cluster"]["ClusterArn"], json!(arn),
                "case {case} ({which}): expected the requested ARN back, got {:?}",
                read["Cluster"]["ClusterArn"]
            );
            assert_eq!(
                read["Cluster"]["Name"], json!(name),
                "case {case} ({which}): expected the shared name, got {:?}",
                read["Cluster"]["Name"]
            );
        }

        let listed = listed_cluster_arns(&router).await;
        for (which, arn) in [("first", &first_arn), ("second", &second_arn)] {
            assert!(
                listed.contains(arn),
                "case {case} ({which}): expected {arn} among the listed clusters, got {listed:?}"
            );
        }
    }

    // A third create under one of the names is a third cluster: nothing about the count
    // is special.
    let third = create_cluster(&router, json!({ "ClusterName": "shared" })).await;
    let third_arn = third["ClusterArn"].as_str().unwrap_or_default().to_string();
    let listed = listed_cluster_arns(&router).await;
    assert!(
        listed.contains(&third_arn),
        "case a third cluster under one name: expected {third_arn} listed, got {listed:?}"
    );
    assert_eq!(
        listed.len(),
        cases.len() * 2 + 1,
        "case every create accepted: expected {} clusters, got {}",
        cases.len() * 2 + 1,
        listed.len()
    );
}

/// A create that names a parent this service does not hold is refused on its INPUT:
/// `ValidationException` (400), naming the kind of resource the request referenced and
/// the ARN it sent, and nothing is created.
///
/// Rows span the two creates that take a parent, both parent kinds (a cluster and a
/// control panel), and four fillings of the ARN's resource id — the all-zero UUID, a
/// lowercase UUID, an upper-case-hex UUID and a non-UUID tail — all of which satisfy the
/// pattern the model puts on those members, so the refusal is about the store and not
/// about the ARN's text.
#[tokio::test]
async fn a_create_naming_a_parent_that_is_not_stored_is_a_validation_error() {
    let router = router();

    // A real cluster exists throughout, so a refusal is about the ARN the request names
    // and not about an empty store.
    let present = create_cluster(&router, json!({ "ClusterName": "present" })).await;
    let present_arn = present["ClusterArn"].as_str().unwrap_or_default().to_string();

    let resource_ids = [
        "00000000-0000-0000-0000-000000000000",
        "7f9ac0b2-4bd5-4e1d-9c3a-11b0e8a5d6f4",
        "ABCDEF01-2345-6789-ABCD-EF0123456789",
        "named_cluster-1",
    ];

    // (case, path, body, the resource the refusal names, the ARN it echoes)
    let mut cases: Vec<(String, &str, Value, &str, String)> = Vec::new();
    for id in resource_ids {
        let cluster_arn = format!("{ARN_PREFIX}cluster/{id}");
        let panel_arn = format!("{ARN_PREFIX}controlpanel/{id}");
        cases.push((
            format!("CreateControlPanel under cluster/{id}"),
            "/controlpanel",
            json!({ "ClusterArn": cluster_arn, "ControlPanelName": "orphan-panel" }),
            "Cluster",
            cluster_arn.clone(),
        ));
        cases.push((
            format!("CreateRoutingControl under cluster/{id}"),
            "/routingcontrol",
            json!({ "ClusterArn": cluster_arn, "RoutingControlName": "orphan-rc" }),
            "Cluster",
            cluster_arn.clone(),
        ));
        cases.push((
            format!("CreateRoutingControl under controlpanel/{id} of a stored cluster"),
            "/routingcontrol",
            json!({
                "ClusterArn": present_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": "orphan-rc",
            }),
            "Control Panel",
            panel_arn.clone(),
        ));
    }

    for (case, path, body, resource, arn) in &cases {
        let (status, answer) = call(&router, "POST", path, Some(body.clone())).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            actual,
            does_not_exist(resource, arn),
            "case {case}: expected the does-not-exist sentence, got {actual:?}"
        );
    }

    // Nothing was created: the stored cluster still holds only the default control panel
    // it was created with, and no routing control.
    let (status, panels) = call(
        &router,
        "GET",
        &format!("/controlpanels?ClusterArn={}", enc(&present_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list the stored cluster's panels: expected 200, got {status} {panels:?}"
    );
    let panels = panels["ControlPanels"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        panels.len(),
        1,
        "case after {} refused creates: expected only the default control panel, got {panels:?}",
        cases.len()
    );
    assert_eq!(
        panels[0]["RoutingControlCount"], json!(0),
        "case after the refused creates: expected no routing control, got {:?}",
        panels[0]["RoutingControlCount"]
    );
}

/// A name already taken inside the parent that scopes it is refused with
/// `ValidationException` (400), and exactly one resource of that name exists afterwards —
/// including when several creates race for it.
#[tokio::test]
async fn a_name_taken_inside_its_parent_is_a_validation_error_and_one_create_wins() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "racing" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "racing-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    // A routing control created without a `ControlPanelArn` lands in the cluster's
    // default panel, which scopes names exactly as a named panel does.
    let implicit = create_routing_control(
        &router,
        json!({ "ClusterArn": cluster_arn, "RoutingControlName": "implicit" }),
    )
    .await;

    // Rows: one per level this service scopes a name to, including the default panel
    // reached by omitting `ControlPanelArn`; each with the sentence it is refused by.
    let default_panel_arn = implicit["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let taken: &[(&str, &str, Value, String)] = &[
        (
            "a control panel name inside its cluster",
            "/controlpanel",
            json!({ "ClusterArn": cluster_arn, "ControlPanelName": "racing-panel" }),
            already_exists("Control Panel", "racing-panel", "Cluster", &cluster_arn),
        ),
        (
            "a routing control name inside its control panel",
            "/routingcontrol",
            json!({
                "ClusterArn": cluster_arn,
                "ControlPanelArn": panel_arn,
                "RoutingControlName": "first",
            }),
            already_exists("Routing Control", "first", "Control Panel", &panel_arn),
        ),
        (
            "a routing control name inside the default control panel",
            "/routingcontrol",
            json!({ "ClusterArn": cluster_arn, "RoutingControlName": "implicit" }),
            already_exists("Routing Control", "implicit", "Control Panel", &default_panel_arn),
        ),
    ];
    // The middle row needs the name to exist first; the others already do.
    create_routing_control(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": "first",
        }),
    )
    .await;

    for (case, path, body, expected) in taken {
        let (status, answer) = call(&router, "POST", path, Some(body.clone())).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            actual, *expected,
            "case {case}: expected the already-exists sentence, got {actual:?}"
        );
    }

    // Five creates racing for one name: one wins, every loser is refused with the same
    // class, and the panel holds exactly one routing control of that name.
    let contested = json!({
        "ClusterArn": cluster_arn,
        "ControlPanelArn": panel_arn,
        "RoutingControlName": "contested",
    });
    let (a, b, c, d, e) = tokio::join!(
        call(&router, "POST", "/routingcontrol", Some(contested.clone())),
        call(&router, "POST", "/routingcontrol", Some(contested.clone())),
        call(&router, "POST", "/routingcontrol", Some(contested.clone())),
        call(&router, "POST", "/routingcontrol", Some(contested.clone())),
        call(&router, "POST", "/routingcontrol", Some(contested.clone())),
    );
    let outcomes = [a, b, c, d, e];
    let winners = outcomes
        .iter()
        .filter(|(status, _)| *status == 200)
        .count();
    assert_eq!(
        winners, 1,
        "case five creates racing for one name: expected exactly one winner, got {outcomes:?}"
    );
    for (status, answer) in outcomes.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(
            *status, 400,
            "case a loser of the race: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(answer),
            "ValidationException",
            "case a loser of the race: expected ValidationException, got {answer:?}"
        );
    }

    let (status, listed) = call(
        &router,
        "GET",
        &format!("/controlpanel/{}/routingcontrols", enc(&panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list the contested panel: expected 200, got {status} {listed:?}"
    );
    let names: Vec<String> = listed["RoutingControls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rc| rc["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names.iter().filter(|name| *name == "contested").count(),
        1,
        "case after the race: expected one routing control named contested, got {names:?}"
    );
    // The same name under a DIFFERENT parent is a different resource.
    let elsewhere = create_routing_control(
        &router,
        json!({ "ClusterArn": cluster_arn, "RoutingControlName": "contested" }),
    )
    .await;
    assert_ne!(
        elsewhere["ControlPanelArn"], json!(panel_arn),
        "case the same name in the default panel: expected the other panel, got {:?}",
        elsewhere["ControlPanelArn"]
    );
    assert_ne!(
        elsewhere["RoutingControlArn"], implicit["RoutingControlArn"],
        "case the same name in the default panel: expected a new routing control, got {:?}",
        elsewhere["RoutingControlArn"]
    );
}

/// A delete is refused with `ValidationException` (400) while the addressed resource
/// still owns dependents, naming the kind of resource and the kind of dependent — and
/// everything survives, so deleting the dependents first lets it succeed.
///
/// Rows span the dependent kinds (a caller-created control panel, routing controls under
/// a named panel, routing controls under the cluster's default panel) and both deploy
/// states: the refusal is asserted on a `PENDING` tree and again once the whole tree has
/// reached `DEPLOYED`.
#[tokio::test]
async fn a_delete_is_refused_while_dependents_remain() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "dependents" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();
    let panel = create_control_panel(
        &router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": "dependents-panel" }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let control = create_routing_control(
        &router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": "dependent-rc",
        }),
    )
    .await;
    let control_arn = control["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let implicit = create_routing_control(
        &router,
        json!({ "ClusterArn": cluster_arn, "RoutingControlName": "implicit-rc" }),
    )
    .await;
    let implicit_arn = implicit["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let default_panel_arn = implicit["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, path, the expected sentence)
    let refused: Vec<(&str, String, String)> = vec![
        (
            "a cluster that still owns a caller-created control panel",
            format!("/cluster/{}", enc(&cluster_arn)),
            cannot_delete("Cluster", "non-default Control Panels"),
        ),
        (
            "a control panel that still owns a routing control",
            format!("/controlpanel/{}", enc(&panel_arn)),
            cannot_delete("Control Panel", "Routing Controls"),
        ),
    ];

    // Once on the freshly created (PENDING) tree, and once after it has deployed: the
    // refusal is about the dependents, not about a transient status.
    for stage in ["pending", "deployed"] {
        if stage == "deployed" {
            tokio::time::sleep(PAST_DEPLOYMENT).await;
        }
        for (case, path, expected) in &refused {
            let (status, answer) = call(&router, "DELETE", path, None).await;
            assert_eq!(
                status, 400,
                "case {case} ({stage}): expected 400, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case} ({stage}): expected ValidationException, got {answer:?}"
            );
            let actual = message(&answer);
            assert_eq!(
                actual, *expected,
                "case {case} ({stage}): expected the cannot-delete sentence, got {actual:?}"
            );
        }
        // Every resource survived a refused delete.
        for (which, path) in [
            ("cluster", format!("/cluster/{}", enc(&cluster_arn))),
            ("control panel", format!("/controlpanel/{}", enc(&panel_arn))),
            (
                "routing control",
                format!("/routingcontrol/{}", enc(&control_arn)),
            ),
        ] {
            let (status, read) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {which} after a refused delete ({stage}): expected 200, got {status} \
                 {read:?}"
            );
        }
    }

    // Tear down from the leaves up. With the caller-created panel gone, the cluster is
    // still held by the routing control left in its DEFAULT panel — the same rule, a
    // different dependent kind.
    for (case, path) in [
        (
            "the routing control",
            format!("/routingcontrol/{}", enc(&control_arn)),
        ),
        (
            "the caller-created control panel",
            format!("/controlpanel/{}", enc(&panel_arn)),
        ),
    ] {
        let (status, answer) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case delete {case}: expected 200, got {status} {answer:?}"
        );
    }

    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", enc(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case a cluster that still owns a routing control: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case a cluster that still owns a routing control: expected ValidationException, got \
         {answer:?}"
    );
    let actual = message(&answer);
    assert_eq!(
        actual,
        cannot_delete("Cluster", "Routing Controls"),
        "case a cluster that still owns a routing control: expected the cannot-delete sentence, \
         got {actual:?}"
    );

    // The default control panel is part of its cluster, so it is not deletable on its
    // own either — also a refusal of the request, not a conflict.
    let (status, answer) = call(
        &router,
        "DELETE",
        &format!("/controlpanel/{}", enc(&default_panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case the default control panel: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case the default control panel: expected ValidationException, got {answer:?}"
    );
    let actual = message(&answer);
    assert_eq!(
        actual,
        cannot_delete_default("Control Panel"),
        "case the default control panel: expected the cannot-delete-a-default sentence, got \
         {actual:?}"
    );

    // With the last dependent gone the cluster deletes, and its default panel goes with
    // it.
    for (case, path) in [
        (
            "the implicit routing control",
            format!("/routingcontrol/{}", enc(&implicit_arn)),
        ),
        ("the cluster", format!("/cluster/{}", enc(&cluster_arn))),
    ] {
        let (status, answer) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case delete {case}: expected 200, got {status} {answer:?}"
        );
    }
    let (status, gone) = call(
        &router,
        "GET",
        &format!("/controlpanel/{}", enc(&default_panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case the default panel after its cluster: expected 404, got {status} {gone:?}"
    );
}

/// A body value outside the `@pattern` or `@length` the model declares for its member is
/// refused by the request-body validator: `BadRequestException` (400), stating the
/// declared rule and the value that failed it, with nothing created.
///
/// Rows span the two declared patterns (the name/token one and the ARN one), both
/// `@length` bounds, the member kinds a body carries (a plain member, a map value, a
/// member on a create and on an update) and several forms of offending whitespace.
#[tokio::test]
async fn a_body_value_outside_a_declared_string_constraint_is_a_bad_request() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "constrained" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();

    let sixty_five = "a".repeat(65);
    let panel_sixty_five = "p".repeat(65);
    // `Tags`' values are `__stringMin0Max256PatternS` (length 0..256, pattern `^\S+$`),
    // a different `@length` maximum from the name members' 64.
    let two_hundred_fifty_seven = "t".repeat(257);

    // (case, method, path, body, the expected sentence)
    let cases: Vec<(&str, &str, &str, Value, String)> = vec![
        (
            "ClusterName with a space",
            "POST",
            "/cluster",
            json!({ "ClusterName": "has a space" }),
            regex_clause(NAME_PATTERN, "has a space"),
        ),
        (
            "ClusterName with a tab",
            "POST",
            "/cluster",
            json!({ "ClusterName": "has\ta tab" }),
            regex_clause(NAME_PATTERN, "has\ta tab"),
        ),
        (
            "an empty ClusterName",
            "POST",
            "/cluster",
            json!({ "ClusterName": "" }),
            regex_clause(NAME_PATTERN, ""),
        ),
        (
            "ClusterName one past the length maximum",
            "POST",
            "/cluster",
            json!({ "ClusterName": sixty_five }),
            too_long_clause(&sixty_five, 64),
        ),
        (
            "ClientToken with a space",
            "POST",
            "/cluster",
            json!({ "ClusterName": "tokened", "ClientToken": "two words" }),
            regex_clause(NAME_PATTERN, "two words"),
        ),
        (
            "a Tags value with a space",
            "POST",
            "/cluster",
            json!({ "ClusterName": "tagged", "Tags": { "team": "two words" } }),
            regex_clause(NAME_PATTERN, "two words"),
        ),
        (
            "ControlPanelName one past the length maximum",
            "POST",
            "/controlpanel",
            json!({ "ClusterArn": cluster_arn, "ControlPanelName": panel_sixty_five }),
            too_long_clause(&panel_sixty_five, 64),
        ),
        (
            "ControlPanelName with a newline",
            "POST",
            "/controlpanel",
            json!({ "ClusterArn": cluster_arn, "ControlPanelName": "two\nlines" }),
            regex_clause(NAME_PATTERN, "two\nlines"),
        ),
        (
            "RoutingControlName with a space",
            "POST",
            "/routingcontrol",
            json!({ "ClusterArn": cluster_arn, "RoutingControlName": "has a space" }),
            regex_clause(NAME_PATTERN, "has a space"),
        ),
        (
            "a ClusterArn outside the ARN pattern, on an update",
            "PUT",
            "/cluster",
            json!({ "ClusterArn": "not an arn!", "NetworkType": "IPV4" }),
            regex_clause(ARN_PATTERN, "not an arn!"),
        ),
        (
            "a Tags value one past its own length maximum",
            "POST",
            "/cluster",
            json!({ "ClusterName": "tagged", "Tags": { "team": two_hundred_fifty_seven } }),
            too_long_clause(&two_hundred_fifty_seven, 256),
        ),
        (
            // The ARN pattern admits the empty string (`*`), so an empty ARN member is
            // the one body value this service refuses on a length MINIMUM alone.
            "an empty ClusterArn, on an update",
            "PUT",
            "/cluster",
            json!({ "ClusterArn": "", "NetworkType": "IPV4" }),
            too_short_clause("", 1),
        ),
    ];

    for (case, method, path, body, expected) in &cases {
        let (status, answer) = call(&router, method, path, Some(body.clone())).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "BadRequestException",
            "case {case}: expected BadRequestException, got {answer:?}"
        );
        let actual = message(&answer);
        assert_eq!(
            actual, *expected,
            "case {case}: expected the body-validation sentence, got {actual:?}"
        );
    }

    // None of the refused creates stored anything: the one cluster created above is
    // still the only one, and it still holds only its default control panel.
    let listed = listed_cluster_arns(&router).await;
    assert_eq!(
        listed,
        vec![cluster_arn.clone()],
        "case after {} refused requests: expected only the cluster created up front, got \
         {listed:?}",
        cases.len()
    );

    // On the bounds and inside the pattern, the same members are accepted.
    for (case, name) in [
        ("one character", "a".to_string()),
        ("the length maximum", "b".repeat(64)),
        ("every non-space character class", "A-9_:/.~!".to_string()),
    ] {
        let created = create_cluster(&router, json!({ "ClusterName": name })).await;
        assert_eq!(
            created["Name"], json!(name),
            "case ClusterName of {case}: expected it stored, got {:?}",
            created["Name"]
        );
    }
}

/// A BODY value the front door has no clause for keeps the modelled
/// `ValidationException` (400): the door states the classes its validator reports and
/// nothing else, so a request it cannot word whole is left to the service's own refusal
/// rather than reported half in one voice and half in another.
///
/// Rows span the body classes with no clause — a value outside an enumerated set, an
/// absent `@required` member — and the MIXED request, where a member outside its pattern
/// (which the door does state) arrives together with one of them.
#[tokio::test]
async fn a_body_class_the_front_door_has_no_clause_for_keeps_the_modelled_error() {
    let router = router();

    let cluster = create_cluster(&router, json!({ "ClusterName": "modelled" })).await;
    let cluster_arn = cluster["ClusterArn"].as_str().unwrap_or_default().to_string();

    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a NetworkType outside the modelled values",
            "PUT",
            "/cluster",
            json!({ "ClusterArn": cluster_arn, "NetworkType": "IPV6" }),
        ),
        (
            "an absent required body member",
            "POST",
            "/cluster",
            json!({ "Tags": { "team": "arc" } }),
        ),
        (
            "a pattern violation together with an absent required member",
            "POST",
            "/controlpanel",
            json!({ "ControlPanelName": "has a space" }),
        ),
    ];

    for (case, method, path, body) in &cases {
        let (status, answer) = call(&router, method, path, Some(body.clone())).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled ValidationException, got {answer:?}"
        );
    }

    // And nothing was stored by any of them.
    let listed = listed_cluster_arns(&router).await;
    assert_eq!(
        listed,
        vec![cluster_arn.clone()],
        "case after {} refused requests: expected only the cluster created up front, got \
         {listed:?}",
        cases.len()
    );
}
