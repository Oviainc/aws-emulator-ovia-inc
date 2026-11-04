// SPDX-License-Identifier: CC0-1.0

//! `UpdateRoutingControl`, end to end over HTTP.
//!
//! The rule under test: `UpdateRoutingControl` writes the name the request carries onto
//! the stored routing control the ARN identifies, leaves every other member of the record
//! alone, and answers with the record a later read and a later list answer with; a name
//! another routing control of the same control panel already holds is refused on the
//! request's input, and an ARN no routing control is stored under is refused with the
//! modelled not-found error.
//!
//! The tables span what that rule generalises over: the name's value, length (both
//! published `@length` bounds of `RoutingControlName` and one step outside each) and
//! character makeup, the no-op rename onto the name the control already carries, a
//! control in the cluster's default panel and one in a caller-created panel, every
//! resource level of the hierarchy for the not-found rule, and the same name taken in the
//! same panel versus in another one.
//!
//! The not-found refusal is asserted for all THREE operations that address a routing
//! control by ARN, because each of the three words it as its own recorded refusal does:
//! the update and the read name the control and its control panel by the ids the ARN
//! nests, each in its own words, while the delete states the service's shared sentence
//! (`tests/all/not_found_message.rs` pins that one, for every operation that states it).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The HTTP harness every suite in this target shares, and the facts about this service's
// resources the lifecycle suite already states.
use super::common::{
    call, create_cluster, create_control_panel, create_routing_control, enc, error_code, router,
};
use super::recovery_control_lifecycle::{
    ACCOUNT, ARN_PREFIX, PAST_DEPLOYMENT, STATUS_DEPLOYED, STATUS_PENDING,
};

/// The published `@length` maximum of `RoutingControlName`
/// (`smithy_query(query="operation",operation="UpdateRoutingControl")` ->
/// `__stringMin1Max64PatternS`, len 1..64).
const NAME_MAX_LENGTH: usize = 64;

/// Its published `@length` minimum, from the same member.
const NAME_MIN_LENGTH: usize = 1;

/// Call `UpdateRoutingControl` for `routing_control_arn` and `routing_control_name`.
async fn update_routing_control(
    router: &MultiServiceRouter,
    routing_control_arn: &str,
    routing_control_name: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        "/routingcontrol",
        Some(json!({
            "RoutingControlArn": routing_control_arn,
            "RoutingControlName": routing_control_name,
        })),
    )
    .await
}

/// `DescribeRoutingControl` for `routing_control_arn`, as the stored `RoutingControl`
/// body.
async fn describe_routing_control(router: &MultiServiceRouter, routing_control_arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/routingcontrol/{}", enc(routing_control_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case describe {routing_control_arn}: expected DescribeRoutingControl 200, got {status} \
         {body:?}"
    );
    body["RoutingControl"].clone()
}

/// The `(ARN, Name)` pairs `ListRoutingControls` reports for `panel_arn`.
async fn list_routing_controls(
    router: &MultiServiceRouter,
    panel_arn: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/controlpanel/{}/routingcontrols", enc(panel_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case list {panel_arn}: expected ListRoutingControls 200, got {status} {body:?}"
    );
    body["RoutingControls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|rc| {
            (
                rc["RoutingControlArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                rc["Name"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `message` of an error body, under the `message` jsonName the model gives this
/// service's error shapes.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The sentence `DescribeRoutingControl` refuses an ARN that addresses no routing control
/// with, restated here independently of the service's own code: fixed text, with the id
/// path the ARN nests (its control panel's id, then the control's own) naming the control,
/// and the control panel's id naming the panel.
fn describe_not_found_sentence(routing_control_arn: &str) -> String {
    let (panel_id, control_id) = nested_ids(routing_control_arn);
    format!("Routing control {panel_id}{control_id} does not exist in control panel {panel_id}")
}

/// The sentence `UpdateRoutingControl` refuses such an ARN with, restated the same way:
/// the control's own id, the fixed text, then its control panel's id.
fn update_not_found_sentence(routing_control_arn: &str) -> String {
    let (panel_id, control_id) = nested_ids(routing_control_arn);
    format!("{control_id}not found in {panel_id}")
}

/// The sentence `DeleteRoutingControl` refuses such an ARN with: the service's shared
/// not-found sentence, which `tests/all/not_found_message.rs` pins for every operation
/// that states it.
fn shared_not_found_sentence(routing_control_arn: &str) -> String {
    format!("No record corresponding to routing control Arn({routing_control_arn})")
}

/// The `(control panel id, routing control id)` a routing control's ARN nests.
fn nested_ids(routing_control_arn: &str) -> (&str, &str) {
    let resource = routing_control_arn
        .rsplit_once(':')
        .map_or(routing_control_arn, |(_, resource)| resource);
    let segments: Vec<&str> = resource.split('/').collect();
    (
        segments.get(1).copied().unwrap_or_default(),
        segments.get(3).copied().unwrap_or_default(),
    )
}

/// Every operation that ADDRESSES a routing control by ARN, each as
/// `(operation, status, body, the sentence it refuses an unresolvable ARN with)`: the
/// update, the read and the delete.
async fn addressing_calls(
    router: &MultiServiceRouter,
    routing_control_arn: &str,
) -> Vec<(&'static str, u16, Value, String)> {
    let (update_status, update_body) =
        update_routing_control(router, routing_control_arn, "renamed").await;
    let (describe_status, describe_body) = call(
        router,
        "GET",
        &format!("/routingcontrol/{}", enc(routing_control_arn)),
        None,
    )
    .await;
    let (delete_status, delete_body) = call(
        router,
        "DELETE",
        &format!("/routingcontrol/{}", enc(routing_control_arn)),
        None,
    )
    .await;
    vec![
        (
            "UpdateRoutingControl",
            update_status,
            update_body,
            update_not_found_sentence(routing_control_arn),
        ),
        (
            "DescribeRoutingControl",
            describe_status,
            describe_body,
            describe_not_found_sentence(routing_control_arn),
        ),
        (
            "DeleteRoutingControl",
            delete_status,
            delete_body,
            shared_not_found_sentence(routing_control_arn),
        ),
    ]
}

/// One cluster with a caller-created control panel: `(cluster ARN, panel ARN)`.
async fn cluster_with_panel(router: &MultiServiceRouter, label: &str) -> (String, String) {
    let cluster = create_cluster(router, json!({ "ClusterName": format!("{label}-cluster") })).await;
    let cluster_arn = cluster["ClusterArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let panel = create_control_panel(
        router,
        json!({ "ClusterArn": cluster_arn, "ControlPanelName": format!("{label}-panel") }),
    )
    .await;
    let panel_arn = panel["ControlPanelArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (cluster_arn, panel_arn)
}

/// Create one routing control in `panel_arn` and return its body.
async fn control_in(
    router: &MultiServiceRouter,
    cluster_arn: &str,
    panel_arn: &str,
    name: &str,
) -> Value {
    create_routing_control(
        router,
        json!({
            "ClusterArn": cluster_arn,
            "ControlPanelArn": panel_arn,
            "RoutingControlName": name,
        }),
    )
    .await
}

/// RULE: the update writes the requested name onto the addressed control, changes nothing
/// else, and a later read and a later list answer the written name.
///
/// Rows differ along the name's value, its length — both published bounds of
/// `RoutingControlName` — and its character makeup, and include the rename onto the name
/// the control already carries.
#[tokio::test]
async fn update_writes_the_name_and_describe_and_list_answer_it() {
    let router = router();
    let (cluster_arn, panel_arn) = cluster_with_panel(&router, "written").await;

    // Every row's name is non-whitespace printable ASCII, which is what the declared
    // pattern (`^\S+$`) accepts; the two length rows sit exactly on the published bounds.
    let longest = "x".repeat(NAME_MAX_LENGTH);
    let shortest = "y".repeat(NAME_MIN_LENGTH);
    let cases: Vec<(&str, String)> = vec![
        ("a one-word name", "renamed".to_string()),
        ("the length minimum", shortest),
        ("the length maximum", longest),
        ("digits only", "1234567890".to_string()),
        ("dashes and underscores", "re-named_control-2".to_string()),
        ("mixed case", "ReNamedControl".to_string()),
        (
            "punctuation the pattern allows",
            "renamed.control+1".to_string(),
        ),
    ];

    for (case, target) in &cases {
        let created = control_in(
            &router,
            &cluster_arn,
            &panel_arn,
            &format!("initial-{}", case.replace(' ', "-")),
        )
        .await;
        let arn = created["RoutingControlArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, answer) = update_routing_control(&router, &arn, target).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateRoutingControl 200, got {status} {answer:?}"
        );
        let updated = answer["RoutingControl"].clone();
        assert_eq!(
            updated["Name"], json!(target),
            "case {case}: expected the requested name {target:?}, got {:?}",
            updated["Name"]
        );
        for member in ["RoutingControlArn", "ControlPanelArn", "Owner"] {
            assert_eq!(
                updated[member], created[member],
                "case {case}: UpdateRoutingControl must not change {member}, got {:?} after {:?}",
                updated[member], created[member]
            );
        }
        assert_eq!(
            updated["Owner"], json!(ACCOUNT),
            "case {case}: expected owner {ACCOUNT}, got {:?}",
            updated["Owner"]
        );

        // The response came from the stored record, so the read answers the same body.
        let read = describe_routing_control(&router, &arn).await;
        for member in ["RoutingControlArn", "ControlPanelArn", "Owner", "Name"] {
            assert_eq!(
                read[member], updated[member],
                "case {case}: DescribeRoutingControl must answer the written {member}, got {:?} \
                 after {:?}",
                read[member], updated[member]
            );
        }

        // A second update with the SAME name is a no-op that still answers the record.
        let (status, again) = update_routing_control(&router, &arn, target).await;
        assert_eq!(
            status, 200,
            "case {case} repeated: expected UpdateRoutingControl 200, got {status} {again:?}"
        );
        assert_eq!(
            again["RoutingControl"]["Name"], json!(target),
            "case {case} repeated: expected the name to stand, got {:?}",
            again["RoutingControl"]["Name"]
        );
    }

    // The list reports the written name for every control, and no old name survives.
    let listed = list_routing_controls(&router, &panel_arn).await;
    let names: Vec<&str> = listed.iter().map(|(_, name)| name.as_str()).collect();
    for (case, target) in &cases {
        assert!(
            names.contains(&target.as_str()),
            "case {case}: ListRoutingControls must report the written name {target:?}, got \
             {names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name.starts_with("initial-")),
        "case the whole panel: no control may keep the name it was created with, got {names:?}"
    );
}

/// RULE, on the clock the rename restarts: a rename re-deploys the control, so it reads
/// `PENDING` straight after the call and reaches `DEPLOYED` on its own; a request naming
/// the name the control already carries changes nothing, so it does not put a deployed
/// control back into `PENDING`.
#[tokio::test]
async fn a_changing_rename_redeploys_the_control_and_a_no_op_does_not() {
    let router = router();
    let (cluster_arn, panel_arn) = cluster_with_panel(&router, "redeploy").await;

    // (case, the name to create with, the name to request)
    let cases: &[(&str, &str, &str)] = &[
        ("a different name", "before", "after"),
        ("the same name again", "steady", "steady"),
        // Case differs, so the stored value differs: this is a change.
        ("the same letters in another case", "casing", "CASING"),
    ];

    for (case, created_with, target) in cases {
        let created = control_in(&router, &cluster_arn, &panel_arn, created_with).await;
        let arn = created["RoutingControlArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            created["Status"], json!(STATUS_PENDING),
            "case {case}: a freshly created control is {STATUS_PENDING}, got {:?}",
            created["Status"]
        );

        // Wait the first deployment out, so the status the update leaves is the only
        // thing the next read can be reporting.
        std::thread::sleep(PAST_DEPLOYMENT);
        let before = describe_routing_control(&router, &arn).await;
        assert_eq!(
            before["Status"], json!(STATUS_DEPLOYED),
            "case {case}: the created control must be {STATUS_DEPLOYED} before the update, got \
             {:?}",
            before["Status"]
        );

        let (status, answer) = update_routing_control(&router, &arn, target).await;
        assert_eq!(
            status, 200,
            "case {case}: expected UpdateRoutingControl 200, got {status} {answer:?}"
        );
        let expected_status = if created_with == target {
            STATUS_DEPLOYED
        } else {
            STATUS_PENDING
        };
        assert_eq!(
            answer["RoutingControl"]["Status"], json!(expected_status),
            "case {case}: expected {expected_status} right after the update, got {:?}",
            answer["RoutingControl"]["Status"]
        );

        // Nothing writes the transition: waiting the span out is what makes it due.
        std::thread::sleep(PAST_DEPLOYMENT);
        let after = describe_routing_control(&router, &arn).await;
        assert_eq!(
            after["Status"], json!(STATUS_DEPLOYED),
            "case {case}: expected {STATUS_DEPLOYED} once the deployment is due, got {:?}",
            after["Status"]
        );
        assert_eq!(
            after["Name"], json!(target),
            "case {case}: the written name must survive the deployment, got {:?}",
            after["Name"]
        );
    }
}

/// RULE, on scope: an update names exactly one routing control, so every other stored
/// control — in the same panel, in the cluster's default panel and in another cluster —
/// keeps the name it had.
#[tokio::test]
async fn an_update_touches_only_the_control_it_names() {
    let router = router();
    let (cluster_arn, panel_arn) = cluster_with_panel(&router, "scoped").await;
    let (other_cluster_arn, other_panel_arn) = cluster_with_panel(&router, "neighbour").await;

    let mut controls: Vec<Value> = Vec::new();
    for n in 0..3 {
        controls.push(control_in(&router, &cluster_arn, &panel_arn, &format!("same-panel-{n}")).await);
    }
    // The cluster's DEFAULT control panel, reached by omitting `ControlPanelArn`.
    controls.push(
        create_routing_control(
            &router,
            json!({ "ClusterArn": cluster_arn, "RoutingControlName": "default-panel" }),
        )
        .await,
    );
    controls.push(
        control_in(
            &router,
            &other_cluster_arn,
            &other_panel_arn,
            "other-cluster",
        )
        .await,
    );

    let target_index = 1usize;
    let target_arn = controls[target_index]["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, answer) = update_routing_control(&router, &target_arn, "only-this-one").await;
    assert_eq!(
        status, 200,
        "case rename one of five: expected 200, got {status} {answer:?}"
    );

    for (n, control) in controls.iter().enumerate() {
        let arn = control["RoutingControlArn"].as_str().unwrap_or_default();
        let read = describe_routing_control(&router, arn).await;
        let expected = if n == target_index {
            json!("only-this-one")
        } else {
            control["Name"].clone()
        };
        assert_eq!(
            read["Name"], expected,
            "case control {n} after the update: expected name {expected:?}, got {:?}",
            read["Name"]
        );
    }
}

/// RULE, on the name's scope: a name another routing control of the SAME control panel
/// holds is refused on the request's input (400 `ValidationException`) and writes nothing,
/// while the same name in another panel is free — and a control may always be renamed to
/// the name it already carries.
#[tokio::test]
async fn renaming_onto_a_name_its_own_panel_already_holds_is_refused() {
    let router = router();
    let (cluster_arn, panel_arn) = cluster_with_panel(&router, "collision").await;
    let (other_cluster_arn, other_panel_arn) = cluster_with_panel(&router, "free").await;

    let taken = control_in(&router, &cluster_arn, &panel_arn, "taken").await;
    let taken_name = taken["Name"].as_str().unwrap_or_default().to_string();
    let mover = control_in(&router, &cluster_arn, &panel_arn, "mover").await;
    let mover_arn = mover["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let elsewhere = control_in(
        &router,
        &other_cluster_arn,
        &other_panel_arn,
        "elsewhere",
    )
    .await;
    let elsewhere_arn = elsewhere["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The refusal wording this service states for a name already taken inside the parent
    // that scopes it, restated here independently of the service's own code.
    let expected_message =
        format!("Routing Control {taken_name} already exists in Control Panel {panel_arn}.");

    let (status, answer) = update_routing_control(&router, &mover_arn, &taken_name).await;
    assert_eq!(
        status, 400,
        "case a name the same panel holds: expected 400, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case a name the same panel holds: expected ValidationException, got {answer:?}"
    );
    let actual = message(&answer);
    assert_eq!(
        actual, expected_message,
        "case a name the same panel holds: expected {expected_message:?}, got {actual:?}"
    );
    let read = describe_routing_control(&router, &mover_arn).await;
    assert_eq!(
        read["Name"], json!("mover"),
        "case a name the same panel holds: a refused update must not write, got {:?}",
        read["Name"]
    );

    // The same name in ANOTHER control panel is not taken, so that rename stands.
    let (status, answer) = update_routing_control(&router, &elsewhere_arn, &taken_name).await;
    assert_eq!(
        status, 200,
        "case the same name in another panel: expected 200, got {status} {answer:?}"
    );
    assert_eq!(
        answer["RoutingControl"]["Name"], json!(taken_name),
        "case the same name in another panel: expected the requested name, got {:?}",
        answer["RoutingControl"]["Name"]
    );

    // And the holder of the name may be renamed to the name it already carries.
    let taken_arn = taken["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let (status, answer) = update_routing_control(&router, &taken_arn, &taken_name).await;
    assert_eq!(
        status, 200,
        "case its own name: expected 200, got {status} {answer:?}"
    );
    assert_eq!(
        answer["RoutingControl"]["Name"], json!(taken_name),
        "case its own name: expected the name to stand, got {:?}",
        answer["RoutingControl"]["Name"]
    );
}

/// RULE, on an ARN that addresses nothing: every operation that ADDRESSES a routing
/// control by ARN answers `ResourceNotFoundException` (404) with the sentence its own
/// recorded refusal states — the update and the read naming the control and its control
/// panel by the ids that ARN carries, each in its own words, the delete stating the
/// service's shared sentence; an ARN that nests no routing control has no control panel to
/// name, so all three fall back to that shared sentence.
///
/// Rows span those three operations, the ARN's own variable parts and the resource level
/// the ARN names.
#[tokio::test]
async fn addressing_an_unknown_routing_control_is_not_found() {
    let router = router();
    let (cluster_arn, panel_arn) = cluster_with_panel(&router, "missing").await;

    // A real control exists, so a refusal is about the ARN the request names and not
    // about an empty store.
    let existing = control_in(&router, &cluster_arn, &panel_arn, "present").await;
    let existing_arn = existing["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let unstored_panel = format!("{ARN_PREFIX}controlpanel/00000000-0000-0000-0000-000000000000");
    let deleted_arn = {
        let doomed = control_in(&router, &cluster_arn, &panel_arn, "doomed").await;
        let arn = doomed["RoutingControlArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/routingcontrol/{}", enc(&arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case prepare a deleted control: expected DeleteRoutingControl 200, got {status} \
             {body:?}"
        );
        arn
    };

    // Rows: a routing-control ARN under a panel that exists and one under a panel that
    // does not, two id fillings, and the ARN of a control this service really minted and
    // then deleted — every one of them nests a routing control, so each operation answers
    // it with its own sentence.
    let nested: Vec<(&str, String)> = vec![
        (
            "a routing control that was never created, under a stored panel",
            format!("{panel_arn}/routingcontrol/11111111-1111-1111-1111-111111111111"),
        ),
        (
            "a routing control under a panel nothing is stored under",
            format!("{unstored_panel}/routingcontrol/ABCDEF01-2345-6789-ABCD-EF0123456789"),
        ),
        (
            "a routing control whose id is no UUID",
            format!("{panel_arn}/routingcontrol/named_control-1"),
        ),
        ("a deleted control's ARN", deleted_arn),
    ];

    for (case, arn) in &nested {
        for (op, status, answer, expected) in addressing_calls(&router, arn).await {
            assert_eq!(
                status, 404,
                "case {op} with {case}: expected 404, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {op} with {case}: expected ResourceNotFoundException, got {answer:?}"
            );
            let actual = message(&answer);
            assert_eq!(
                actual, expected,
                "case {op} with {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // Rows: an ARN of every OTHER resource level, and a value that is no ARN at all —
    // none of them nests a routing control, so none of them can be answered with a
    // control panel's id.
    let other: Vec<(&str, String)> = vec![
        ("a control-panel ARN", panel_arn.clone()),
        ("a cluster ARN", cluster_arn.clone()),
        (
            "a safety-rule ARN",
            format!("{panel_arn}/safetyrule/22222222-2222-2222-2222-222222222222"),
        ),
        ("a value that is not an ARN at all", "not-an-arn".to_string()),
    ];

    for (case, arn) in &other {
        for (op, status, answer, _) in addressing_calls(&router, arn).await {
            assert_eq!(
                status, 404,
                "case {op} with {case}: expected 404, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {op} with {case}: expected ResourceNotFoundException, got {answer:?}"
            );
            let actual = message(&answer);
            let expected = shared_not_found_sentence(arn);
            assert_eq!(
                actual, expected,
                "case {op} with {case}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The control that does exist still carries the name it was created with.
    let read = describe_routing_control(&router, &existing_arn).await;
    assert_eq!(
        read["Name"], json!("present"),
        "case the existing control: a refused update must change nothing, got {:?}",
        read["Name"]
    );
}

/// RULE, at the name's declared boundary: a `RoutingControlName` outside the published
/// `@length` bounds or the declared pattern never reaches this operation — the
/// request-body validator refuses it with `BadRequestException` (400) — and the control
/// keeps the name it had.
#[tokio::test]
async fn a_name_outside_the_declared_constraints_is_refused() {
    let router = router();
    let (cluster_arn, panel_arn) = cluster_with_panel(&router, "bounds").await;

    let created = control_in(&router, &cluster_arn, &panel_arn, "unchanged").await;
    let arn = created["RoutingControlArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows: one step past each published bound, and whitespace, which the declared
    // pattern (`^\S+$`) excludes at any length.
    let one_past_maximum = "z".repeat(NAME_MAX_LENGTH + 1);
    let cases: Vec<(&str, String)> = vec![
        ("one below the length minimum", String::new()),
        ("one past the length maximum", one_past_maximum),
        ("a name with a space", "two words".to_string()),
        ("a name with a tab", "two\twords".to_string()),
    ];

    for (case, name) in &cases {
        let (status, answer) = update_routing_control(&router, &arn, name).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "BadRequestException",
            "case {case}: expected BadRequestException, got {answer:?}"
        );
        let read = describe_routing_control(&router, &arn).await;
        assert_eq!(
            read["Name"], json!("unchanged"),
            "case {case}: a refused update must not write, got {:?}",
            read["Name"]
        );
    }

    // The bounds themselves are accepted, so the rows above refuse only what is outside
    // them.
    for (case, name) in [
        ("the length minimum", "m".repeat(NAME_MIN_LENGTH)),
        ("the length maximum", "m".repeat(NAME_MAX_LENGTH)),
    ] {
        let (status, answer) = update_routing_control(&router, &arn, &name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {answer:?}"
        );
        assert_eq!(
            answer["RoutingControl"]["Name"], json!(name),
            "case {case}: expected the requested name, got {:?}",
            answer["RoutingControl"]["Name"]
        );
    }
}
