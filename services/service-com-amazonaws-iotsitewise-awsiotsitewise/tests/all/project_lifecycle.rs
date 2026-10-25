// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise Monitor project slice (`CreateProject` ->
//! `DescribeProject` -> `UpdateProject`), driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a created project is persisted in the portal the request named and reads back
//!   exactly as it was created, with the ARN form `CreateProject` documents;
//! * an update REPLACES the project's caller-owned members — its name and its
//!   description — with what the request states, so an omitted optional description
//!   clears the stored one, and moves the project's last-update instant to the
//!   instant of the call while leaving its id, its ARN, the portal it lives in, its
//!   tags and its creation date exactly as they were;
//! * an update leaves every other project alone, and a read never changes what the
//!   next caller sees;
//! * an id that resolves to no stored project — or to no stored portal, for a
//!   create — is a modelled 404 rather than a fabricated answer;
//! * a value past a modelled length bound is refused before anything is written.
//!
//! The one refusal these suites do NOT exercise is the `MAX_PROJECTS` ceiling: the
//! bound is 1000 projects and nothing evicts one, so covering it would mean a
//! thousand creates in a single test — the same reason the sibling portal suite
//! records for its own ceiling.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `is_modelled_id`)
//! is the one the asset suite already has, and the portal a project lives in is
//! created through the helper the access-policy suite already has; this suite
//! reuses both rather than growing a second copy.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{service_state, TAG_KEY_MAX_LENGTH};

use crate::access_policy_lifecycle::create_portal;
use crate::asset_lifecycle::{call, error_code, is_modelled_id, router_with_engine, REGION};

/// Where every project request goes, as `OPERATIONS.md` reports the routes:
/// `POST /projects`, `GET /projects/{projectId}`, `PUT /projects/{projectId}`.
const PROJECTS: &str = "/projects";

/// The modelled `Name` maximum length, which `projectName` targets
/// (smithy_query query="operation" operation="UpdateProject": `projectName`
/// (Name, len 1..256)).
const NAME_MAX_LENGTH: usize = 256;

/// The modelled `Description` maximum length, which `projectDescription` targets
/// (same source: `projectDescription` (Description, len 1..2048)).
const DESCRIPTION_MAX_LENGTH: usize = 2048;

/// Long enough that the clock the handlers read moves between two calls, so a
/// last-update instant that the update is supposed to advance can be asserted to
/// have advanced.
const CLOCK_GAP: Duration = Duration::from_millis(5);

/// One row of the update table: the description the create stored, beside the name
/// and description the update then states.
#[derive(Debug)]
struct UpdateCase {
    label: &'static str,
    created_description: Option<String>,
    updated_name: String,
    updated_description: Option<String>,
}

/// The rows. They differ along every dimension `UpdateProject`'s input has: whether
/// the stored project had a description at all, whether the update states one
/// (added, replaced, cleared, absent throughout), and the size and form of the two
/// values it carries — one character, both published maxima, non-ASCII text, and
/// punctuation and spaces.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "description-added-to-a-project-created-without-one",
            created_description: None,
            updated_name: "the-renamed-project".to_string(),
            updated_description: Some("a description this project had none of".to_string()),
        },
        UpdateCase {
            label: "description-replaced",
            created_description: Some("the description the create stored".to_string()),
            updated_name: "the-twice-written-project".to_string(),
            updated_description: Some("the description the update stored".to_string()),
        },
        UpdateCase {
            label: "description-cleared-by-an-update-that-states-none",
            created_description: Some("the description that is about to go".to_string()),
            updated_name: "the-undescribed-project".to_string(),
            updated_description: None,
        },
        UpdateCase {
            label: "no-description-on-either-side",
            created_description: None,
            updated_name: "the-never-described-project".to_string(),
            updated_description: None,
        },
        UpdateCase {
            label: "name-and-description-at-the-minimum-length",
            created_description: Some("the description the create stored".to_string()),
            updated_name: "p".to_string(),
            updated_description: Some("d".to_string()),
        },
        UpdateCase {
            label: "name-and-description-at-the-maximum-length",
            created_description: None,
            updated_name: "n".repeat(NAME_MAX_LENGTH),
            updated_description: Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
        },
        UpdateCase {
            label: "non-ascii-name-and-description",
            created_description: Some("la première description".to_string()),
            updated_name: "proyecto-ñandú-日本語".to_string(),
            updated_description: Some("descripción actualizada — 更新済み".to_string()),
        },
        UpdateCase {
            label: "name-with-spaces-and-punctuation",
            created_description: Some("floor one, line two".to_string()),
            updated_name: "Floor 1 / Line 2 (v2) — monitor".to_string(),
            updated_description: Some("Floor 1 / Line 2 (v2): everything at once".to_string()),
        },
    ]
}

/// The body a create sends.
fn create_body(portal_id: &str, name: &str, description: Option<&str>) -> Value {
    let mut body = json!({ "portalId": portal_id, "projectName": name });
    if let Some(description) = description {
        body["projectDescription"] = Value::String(description.to_string());
    }
    body
}

/// The body an update sends.
fn update_body(name: &str, description: Option<&str>) -> Value {
    let mut body = json!({ "projectName": name });
    if let Some(description) = description {
        body["projectDescription"] = Value::String(description.to_string());
    }
    body
}

/// `DescribeProject` on `id`.
async fn describe_project(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("{PROJECTS}/{id}"), None).await
}

/// The tags the store holds for the project `id`, as sorted pairs, or `None` when
/// the store holds no such project.
///
/// Read from the store rather than from the wire because `CreateProject` carries
/// tags but no project reader reports them, so this is the only place they are
/// visible.
fn stored_tags(engine: &engine::InMemoryEngine, id: &str) -> Option<Vec<(String, String)>> {
    let state = service_state(engine).expect("the iotsitewise store");
    let tags = state.lock().projects.get(id).map(|project| {
        let mut pairs: Vec<(String, String)> = project
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        pairs.sort();
        pairs
    });
    tags
}

/// How many projects the store holds.
fn stored_project_count(engine: &engine::InMemoryEngine) -> usize {
    let state = service_state(engine).expect("the iotsitewise store");
    let count = state.lock().projects.len();
    count
}

/// The names of every project the store holds — the only way to see that a
/// refused create persisted nothing at all.
fn stored_project_names(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = service_state(engine).expect("the iotsitewise store");
    let names: Vec<String> = state
        .lock()
        .projects
        .values()
        .map(|project| project.name.clone())
        .collect();
    names
}

/// What a project's description must read back as: the string the write stated, or
/// nothing at all when it stated none.
fn assert_description(case: &str, described: &Value, expected: Option<&str>) {
    match expected {
        Some(expected) => assert_eq!(
            described["projectDescription"].as_str().unwrap_or("<absent>"),
            expected,
            "case {case:?}: projectDescription must read back as the write stated it, \
             got {described}"
        ),
        None => assert!(
            described
                .get("projectDescription")
                .is_none_or(Value::is_null),
            "case {case:?}: a project no write gave a description reports none, got {described}"
        ),
    }
}

#[tokio::test]
async fn an_update_replaces_the_projects_caller_owned_members_for_every_input_shape() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-project-portal").await;

    for case in update_cases() {
        let created_name = format!("created-{}", case.label);
        let (status, created) = call(
            &router,
            "POST",
            PROJECTS,
            Some(create_body(
                &portal_id,
                &created_name,
                case.created_description.as_deref(),
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {:?}: CreateProject expected 201, got {status} {created}",
            case.label
        );

        let id = created["projectId"].as_str().unwrap_or("").to_string();
        assert!(
            is_modelled_id(&id),
            "case {:?}: projectId must be a modelled UUID, got {created}",
            case.label
        );
        let expected_arn = format!(
            "arn:aws:iotsitewise:{REGION}:{}:project/{id}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            created["projectArn"].as_str().unwrap_or(""),
            expected_arn,
            "case {:?}: unexpected projectArn in {created}",
            case.label
        );

        // The create round-trips through the reader before the update touches it,
        // so what the update changed is measured against what was really stored.
        let (status, before) = describe_project(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeProject expected 200, got {status} {before}",
            case.label
        );
        assert_eq!(
            before["projectName"].as_str().unwrap_or(""),
            created_name,
            "case {:?}: DescribeProject reported another name in {before}",
            case.label
        );
        assert_eq!(
            before["portalId"].as_str().unwrap_or(""),
            portal_id,
            "case {:?}: a project lives in the portal the create named, got {before}",
            case.label
        );
        assert_description(case.label, &before, case.created_description.as_deref());
        let creation_date = before["projectCreationDate"].as_f64().unwrap_or(0.0);
        let last_update_before = before["projectLastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            creation_date > 0.0,
            "case {:?}: projectCreationDate must be a real instant, got {before}",
            case.label
        );
        assert_eq!(
            creation_date, last_update_before,
            "case {:?}: an untouched project's lastUpdateDate is its creationDate, got {before}",
            case.label
        );

        // Tags are asserted against the store: `CreateProject` carries them but no
        // project reader reports them, so this is the only place they are visible.
        let tags_before = stored_tags(&engine, &id)
            .unwrap_or_else(|| panic!("case {:?}: project {id} was not persisted", case.label));

        tokio::time::sleep(CLOCK_GAP).await;

        let (status, answer) = call(
            &router,
            "PUT",
            &format!("{PROJECTS}/{id}"),
            Some(update_body(
                &case.updated_name,
                case.updated_description.as_deref(),
            )),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: UpdateProject expected 200, got {status} {answer}",
            case.label
        );

        let (status, after) = describe_project(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeProject after the update expected 200, got {status} {after}",
            case.label
        );
        assert_eq!(
            after["projectName"].as_str().unwrap_or(""),
            case.updated_name,
            "case {:?}: the project's name must be the one the update stated, got {after}",
            case.label
        );
        assert_description(case.label, &after, case.updated_description.as_deref());

        // The update owns the name, the description and the last-update instant, and
        // nothing else on the record.
        assert_eq!(
            after["projectId"], before["projectId"],
            "case {:?}: an update does not re-mint the project id, got {after}",
            case.label
        );
        assert_eq!(
            after["projectArn"], before["projectArn"],
            "case {:?}: an update does not re-mint the project ARN, got {after}",
            case.label
        );
        assert_eq!(
            after["portalId"], before["portalId"],
            "case {:?}: an update does not move the project between portals, got {after}",
            case.label
        );
        assert_eq!(
            after["projectCreationDate"].as_f64().unwrap_or(0.0),
            creation_date,
            "case {:?}: an update does not change when the project was created, got {after}",
            case.label
        );
        let last_update_after = after["projectLastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            last_update_after > last_update_before,
            "case {:?}: the update must move the last-update instant past {last_update_before}, \
             got {after}",
            case.label
        );

        let tags_after = stored_tags(&engine, &id)
            .unwrap_or_else(|| panic!("case {:?}: project {id} vanished on update", case.label));
        assert_eq!(
            tags_after, tags_before,
            "case {:?}: an update does not touch the project's tags, got {tags_after:?}",
            case.label
        );

        // Reading does not change what the next caller sees.
        let (status, again) = describe_project(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: second DescribeProject expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again, after,
            "case {:?}: DescribeProject answered differently on the second read: {again}",
            case.label
        );
    }
}

#[tokio::test]
async fn the_tags_a_create_carries_are_persisted_on_the_project() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-tagged-project-portal").await;

    // The rows differ along the number of tags, from none at all to several.
    let cases: Vec<(&str, Value)> = vec![
        ("no tags", json!(null)),
        ("one tag", json!({ "stage": "test" })),
        (
            "three tags",
            json!({ "team": "ops", "env": "edge", "owner": "sitewise" }),
        ),
    ];

    for (case, tags) in cases {
        let mut body = create_body(&portal_id, &format!("project-{case}"), None);
        if !tags.is_null() {
            body["tags"] = tags.clone();
        }
        let (status, created) = call(&router, "POST", PROJECTS, Some(body)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateProject expected 201, got {status} {created}"
        );
        let id = created["projectId"].as_str().unwrap_or("").to_string();

        let mut expected: Vec<(String, String)> = tags
            .as_object()
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| {
                        (key.clone(), value.as_str().unwrap_or_default().to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        expected.sort();

        let stored = stored_tags(&engine, &id)
            .unwrap_or_else(|| panic!("case {case:?}: project {id} was not persisted"));
        assert_eq!(
            stored, expected,
            "case {case:?}: the tags the create carried must be persisted, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_outside_the_modelled_bounds_is_refused_by_the_handler() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-tag-key-bounds-portal").await;

    // `TagMap`'s KEYS target `TagKey`, a bound the framework's constraint walk does
    // not reach — it validates a map's values only — so this is the HANDLER's own
    // refusal and its error code is pinned. The rows walk the bound: both published
    // ends, and one character past each.
    let cases: Vec<(&str, String, bool)> = vec![
        ("the empty key, one below the minimum", String::new(), false),
        ("a key at the minimum length", "k".to_string(), true),
        (
            "a key at the maximum length",
            "k".repeat(TAG_KEY_MAX_LENGTH),
            true,
        ),
        (
            "a key one past the maximum length",
            "k".repeat(TAG_KEY_MAX_LENGTH + 1),
            false,
        ),
    ];

    for (case, key, accepted) in cases {
        let name = format!("tag-key-{}", key.len());
        let mut body = create_body(&portal_id, &name, None);
        body["tags"] = json!({ key.clone(): "value" });
        let (status, answer) = call(&router, "POST", PROJECTS, Some(body)).await;

        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: a key inside the bound is accepted, got {status} {answer}"
            );
            let id = answer["projectId"].as_str().unwrap_or("").to_string();
            let stored = stored_tags(&engine, &id)
                .unwrap_or_else(|| panic!("case {case:?}: project {id} was not persisted"));
            assert_eq!(
                stored,
                vec![(key.clone(), "value".to_string())],
                "case {case:?}: the accepted key must be persisted, got {stored:?}"
            );
        } else {
            // The operation models no `ValidationException`, so its refusal of a
            // malformed request is the `InvalidRequestException` it does model.
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {case:?}: a key outside the bound must be refused, got {status} {answer}"
            );
            let stored_names: Vec<String> = stored_project_names(&engine);
            assert!(
                !stored_names.contains(&name),
                "case {case:?}: a refused create must persist nothing, got {stored_names:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_naming_a_project_that_was_never_created_is_refused() {
    let (router, _engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-only-project-portal").await;

    let (status, created) = call(
        &router,
        "POST",
        PROJECTS,
        Some(create_body(&portal_id, "the-only-project", Some("as created"))),
    )
    .await;
    assert_eq!(
        status, 201,
        "the only project: expected 201, got {status} {created}"
    );
    let id = created["projectId"].as_str().unwrap_or("").to_string();
    let (_, before) = describe_project(&router, &id).await;

    // Well-formed ids no create ever minted: one all-f, one that differs from the
    // stored id in a single nibble.
    let mut neighbour: Vec<char> = id.chars().collect();
    let at = neighbour
        .iter()
        .position(|ch| *ch != '-' && *ch != '0')
        .expect("a non-zero character in the id");
    neighbour[at] = '0';
    let neighbour: String = neighbour.into_iter().collect();

    for case in ["ffffffff-ffff-ffff-ffff-ffffffffffff", neighbour.as_str()] {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        for (operation, method) in [("UpdateProject", "PUT"), ("DescribeProject", "GET")] {
            let body = (method == "PUT").then(|| update_body("whatever", None));
            let (status, answer) =
                call(&router, method, &format!("{PROJECTS}/{case}"), body).await;
            assert_eq!(
                (status, error_code(&answer)),
                (404, "ResourceNotFoundException"),
                "case {case:?}: {operation} on an absent project must be a modelled 404, \
                 got {status} {answer}"
            );
            // The refusal names the identifier the REQUEST supplied, not the one
            // project that does exist.
            let message = answer["message"].as_str().unwrap_or("");
            assert!(
                message.contains(case) && !message.contains(&id),
                "case {case:?}: {operation} must name the requested id only, got {answer}"
            );
        }
    }

    // The refusals wrote nothing: the stored project reads back as it was created.
    let (status, after) = describe_project(&router, &id).await;
    assert_eq!(
        status, 200,
        "the stored project after the refusals: got {status} {after}"
    );
    assert_eq!(
        after, before,
        "a refused update must leave the stored project exactly as it was, got {after}"
    );
}

#[tokio::test]
async fn a_create_naming_a_portal_that_does_not_exist_is_refused() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-real-portal").await;

    for case in ["ffffffff-ffff-ffff-ffff-ffffffffffff", "00000000-0000-0000-0000-000000000001"] {
        assert!(
            is_modelled_id(case),
            "case {case:?}: the probe id must itself be modelled"
        );
        assert_ne!(
            case, portal_id,
            "case {case:?}: the probe must not be the portal that exists"
        );
        let (status, answer) = call(
            &router,
            "POST",
            PROJECTS,
            Some(create_body(case, "project-in-no-portal", None)),
        )
        .await;
        assert_eq!(
            (status, error_code(&answer)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a create in an absent portal must be a modelled 404, \
             got {status} {answer}"
        );
    }

    let stored = stored_project_count(&engine);
    assert_eq!(
        stored, 0,
        "a refused create must persist nothing, the store holds {stored} projects"
    );
}

#[tokio::test]
async fn updating_one_project_leaves_every_other_project_alone() {
    let (router, _engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-shared-portal").await;

    // Three projects of the same portal, all created under one name and one
    // description so that only the update can tell them apart afterwards.
    let mut ids: Vec<String> = Vec::new();
    for case in ["first", "second", "third"] {
        let (status, created) = call(
            &router,
            "POST",
            PROJECTS,
            Some(create_body(
                &portal_id,
                "the-shared-project-name",
                Some("the shared description"),
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateProject expected 201, got {status} {created}"
        );
        ids.push(created["projectId"].as_str().unwrap_or("").to_string());
    }
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        ids.len(),
        "three projects sharing a name are still three resources, got {ids:?}"
    );

    let mut before: Vec<Value> = Vec::new();
    for id in &ids {
        let (status, described) = describe_project(&router, id).await;
        assert_eq!(status, 200, "project {id}: got {status} {described}");
        before.push(described);
    }

    let (status, answer) = call(
        &router,
        "PUT",
        &format!("{PROJECTS}/{}", ids[1]),
        Some(update_body("only-the-second-was-renamed", None)),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateProject on the second project: got {status} {answer}"
    );

    for (at, id) in ids.iter().enumerate() {
        let (status, after) = describe_project(&router, id).await;
        assert_eq!(status, 200, "project {id}: got {status} {after}");
        if at == 1 {
            assert_eq!(
                after["projectName"].as_str().unwrap_or(""),
                "only-the-second-was-renamed",
                "project {id} is the one the update named, got {after}"
            );
            assert_description("the updated project", &after, None);
        } else {
            assert_eq!(
                after, before[at],
                "project {id} was not named by the update, so nothing about it changed: {after}"
            );
        }
    }
}

#[tokio::test]
async fn a_project_value_outside_a_modelled_length_bound_is_refused() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-bounds-portal").await;

    let (status, created) = call(
        &router,
        "POST",
        PROJECTS,
        Some(create_body(&portal_id, "the-bounds-project", Some("as created"))),
    )
    .await;
    assert_eq!(
        status, 201,
        "the bounds project: expected 201, got {status} {created}"
    );
    let id = created["projectId"].as_str().unwrap_or("").to_string();
    let (_, before) = describe_project(&router, &id).await;

    // The rows walk both published bounds of the two constrained members: the
    // maximum itself is accepted by the table above, so these are the values one
    // character past each bound, plus the empty string one character below the
    // minimum, and an absent required member.
    let cases: Vec<(&str, Value)> = vec![
        (
            "projectName one past the maximum length",
            update_body(&"n".repeat(NAME_MAX_LENGTH + 1), None),
        ),
        (
            "projectName one below the minimum length",
            update_body("", None),
        ),
        (
            "projectDescription one past the maximum length",
            update_body("fine-name", Some(&"d".repeat(DESCRIPTION_MAX_LENGTH + 1))),
        ),
        (
            "projectDescription one below the minimum length",
            update_body("fine-name", Some("")),
        ),
        ("no projectName at all", json!({})),
    ];

    for (case, body) in cases {
        // Only the STATUS is asserted: every row is refused by the boundary
        // validator before the handler runs, and the error CODE that walk renders is
        // the subject of its own issue — the same reason the portal suite records.
        let (status, answer) = call(&router, "PUT", &format!("{PROJECTS}/{id}"), Some(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the model expected 400, got {status} {answer}"
        );
    }

    // Every refusal left the stored project exactly as it was.
    let (status, after) = describe_project(&router, &id).await;
    assert_eq!(status, 200, "the stored project: got {status} {after}");
    assert_eq!(
        after, before,
        "a refused update must write nothing, got {after}"
    );

    let stored = stored_project_count(&engine);
    assert_eq!(
        stored, 1,
        "the refusals must not add a project, the store holds {stored}"
    );
}
