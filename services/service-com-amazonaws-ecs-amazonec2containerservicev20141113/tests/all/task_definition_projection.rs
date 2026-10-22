// SPDX-License-Identifier: CC0-1.0

//! The container-definition projection a task definition reads back with.
//!
//! The rule under test: in every response that carries a task definition, each
//! container definition reports the collection members ECS materialises —
//! `portMappings`, `environment`, `mountPoints`, `volumesFrom`, `systemControls` — as a
//! list that is PRESENT (empty when the registration declared none, exactly what was
//! declared otherwise), so a client may index them unconditionally; a member with no
//! empty form, such as the `logConfiguration` structure, stays ABSENT unless it was
//! declared, because nothing is invented for it.
//!
//! Everything goes through the same HTTP front door a client uses, via the harness in
//! `task_lifecycle`.

use serde_json::{json, Value};

use crate::task_lifecycle::{container, ok, service};

/// The image every container here starts from; irrelevant to the rule, so one value.
const IMAGE: &str = "public.ecr.aws/docker/library/nginx:latest";

/// The container-definition collection members the projection materialises, each with
/// one sample value its own shape allows. Source: the `ContainerDefinition` members in
/// `smithy_query(query="shape", shape="com.amazonaws.ecs#ContainerDefinition")` —
/// `portMappings` (PortMappingList), `environment` (EnvironmentVariables),
/// `mountPoints` (MountPointList), `volumesFrom` (VolumeFromList) and `systemControls`
/// (SystemControlList).
fn materialised_members() -> Vec<(&'static str, Value)> {
    vec![
        (
            "portMappings",
            json!([{ "containerPort": 80, "hostPort": 0, "protocol": "tcp" }]),
        ),
        (
            "environment",
            json!([{ "name": "STAGE", "value": "prod" }, { "name": "DEBUG", "value": "" }]),
        ),
        (
            "mountPoints",
            json!([{ "sourceVolume": "data", "containerPath": "/var/data", "readOnly": true }]),
        ),
        (
            "volumesFrom",
            json!([{ "sourceContainer": "app0", "readOnly": false }]),
        ),
        (
            "systemControls",
            json!([{ "namespace": "net.core.somaxconn", "value": "1024" }]),
        ),
    ]
}

/// A member that must be PRESENT as a list — an absent key fails here rather than
/// reading as empty, which is the whole point of the rule.
fn present_list(definition: &Value, member: &str, case: &str) -> Value {
    match definition.get(member) {
        Some(Value::Array(items)) => Value::Array(items.clone()),
        other => panic!(
            "case {case:?}: expected {member} to be PRESENT as a list, got {other:?} in \
             {definition}"
        ),
    }
}

/// The container definitions of a task-definition-carrying response body.
fn container_definitions(body: &Value, case: &str) -> Vec<Value> {
    match body["taskDefinition"]["containerDefinitions"] {
        Value::Array(ref items) => items.clone(),
        ref other => panic!(
            "case {case:?}: expected containerDefinitions to be a list, got {other} in {body}"
        ),
    }
}

#[tokio::test]
async fn container_collections_are_materialised_in_every_task_definition_response() {
    let router = service();
    let members = materialised_members();

    // Each row says, per member of `members`, what the registration DECLARES: `None`
    // omits the member, `Some(value)` sends that literal value — including an explicit
    // `[]`, which must read back the same way an omitted member does.
    let mut rows: Vec<(String, Vec<Option<Value>>)> = vec![
        ("nothing declared".to_string(), vec![None; members.len()]),
        (
            "every collection declared empty".to_string(),
            vec![Some(json!([])); members.len()],
        ),
    ];
    // One row per member, so the rule is exercised for each collection on its own
    // (every other member of that row is omitted).
    for (index, (member, value)) in members.iter().enumerate() {
        let mut declared = vec![None; members.len()];
        declared[index] = Some(value.clone());
        rows.push((format!("only {member} declared"), declared));
    }
    rows.push((
        "every collection declared".to_string(),
        members.iter().map(|(_, value)| Some(value.clone())).collect(),
    ));

    // The count spans one container and several, so the projection cannot be keyed to
    // `containerDefinitions[0]`.
    for (count_index, container_count) in [1_usize, 3].into_iter().enumerate() {
        for (row_index, (row_name, declared)) in rows.iter().enumerate() {
            let case = format!("{row_name} / {container_count} containers");
            let family = format!("projection-{count_index}-{row_index}");

            let containers: Vec<Value> = (0..container_count)
                .map(|slot| {
                    let mut definition = container(&format!("app{slot}"), IMAGE);
                    for ((member, _), value) in members.iter().zip(declared) {
                        if let Some(value) = value {
                            definition[*member] = value.clone();
                        }
                    }
                    definition
                })
                .collect();

            let registered = ok(
                &router,
                "RegisterTaskDefinition",
                json!({ "family": family, "containerDefinitions": containers }),
                &case,
            )
            .await;
            let described = ok(
                &router,
                "DescribeTaskDefinition",
                json!({ "taskDefinition": family }),
                &case,
            )
            .await;

            // The rule holds for the operation that writes the revision and for the
            // one that reads it back — the projection is shared, not per handler.
            for (source, body) in [
                ("RegisterTaskDefinition", &registered),
                ("DescribeTaskDefinition", &described),
            ] {
                let definitions = container_definitions(body, &case);
                assert_eq!(
                    definitions.len(),
                    container_count,
                    "case {case:?}: {source} returned {} container definitions, expected \
                     {container_count}: {body}",
                    definitions.len()
                );
                for (slot, definition) in definitions.iter().enumerate() {
                    for ((member, _), declared_value) in members.iter().zip(declared) {
                        let expected = declared_value.clone().unwrap_or_else(|| json!([]));
                        let actual = present_list(definition, member, &case);
                        assert_eq!(
                            actual, expected,
                            "case {case:?}: {source} container {slot}: expected {member} \
                             {expected}, got {actual} in {definition}"
                        );
                    }
                    // Declared values are reported as stored, so the materialisation
                    // cannot be masking the caller's own input.
                    assert_eq!(
                        definition["name"],
                        json!(format!("app{slot}")),
                        "case {case:?}: {source} container {slot}: expected the stored \
                         name back, got {definition}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn members_with_no_empty_form_stay_absent_unless_declared() {
    let router = service();

    // `logConfiguration` targets the `LogConfiguration` STRUCTURE, which has no empty
    // form: a container that declares none reads back without the key — ECS invents no
    // default log driver. `user` and `hostname` are the same for scalars.
    let declared_log_configuration = json!({
        "logDriver": "awslogs",
        "options": { "awslogs-group": "/ecs/projection", "awslogs-region": "us-west-2" },
    });
    let cases: [(&str, Option<Value>); 2] = [
        ("logConfiguration omitted", None),
        (
            "logConfiguration declared",
            Some(declared_log_configuration.clone()),
        ),
    ];

    for (index, (case, log_configuration)) in cases.into_iter().enumerate() {
        let family = format!("absent-{index}");
        let mut definition = container("app", IMAGE);
        if let Some(ref value) = log_configuration {
            definition["logConfiguration"] = value.clone();
        }

        ok(
            &router,
            "RegisterTaskDefinition",
            json!({ "family": family, "containerDefinitions": [definition] }),
            case,
        )
        .await;
        let described = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;

        let definitions = container_definitions(&described, case);
        let reported = definitions
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected one container, got {described}"));

        match log_configuration {
            None => assert!(
                reported.get("logConfiguration").is_none(),
                "case {case:?}: expected logConfiguration to stay absent, got {reported}"
            ),
            Some(expected) => assert_eq!(
                reported["logConfiguration"], expected,
                "case {case:?}: expected the declared logConfiguration back, got {reported}"
            ),
        }
        for member in ["user", "hostname", "workingDirectory"] {
            assert!(
                reported.get(member).is_none(),
                "case {case:?}: expected the undeclared {member} to stay absent, got \
                 {reported}"
            );
        }
        // The collections are still materialised in the same response.
        for (member, _) in materialised_members() {
            assert_eq!(
                present_list(reported, member, case),
                json!([]),
                "case {case:?}: expected {member} to be [], got {reported}"
            );
        }
    }
}
