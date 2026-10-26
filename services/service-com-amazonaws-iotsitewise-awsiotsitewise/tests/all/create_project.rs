// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateProject`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a create mints a fresh project in the portal the request names, persists
//!   every member the request carried and answers with the minted id and the ARN
//!   form the model documents, so `DescribeProject` reads back exactly what the
//!   create stored and `ListTagsForResource` on the minted ARN reports exactly
//!   the tags it carried;
//! * a created project starts un-updated: its last-update instant is its
//!   creation instant, and reading it never changes what the next caller sees;
//! * each create is its own resource — two identical requests mint two
//!   projects, each in the portal its own request named;
//! * a request outside the model is refused before anything is written, and a
//!   refusal persists nothing.
//!
//! The rows span what the create rule generalises over: whether the optional
//! members are present at all, the size of each value (both published length
//! bounds of `projectName` and `projectDescription`), its form (ASCII,
//! non-ASCII, punctuation and spaces) and the NUMBER of tags, from none to the
//! published `TagMap` maximum.
//!
//! The sibling `project_lifecycle` suite pins `UpdateProject` over the same
//! slice, and the refusal of a create naming a portal that does not exist; this
//! one is about what an ACCEPTED create stores and about the create's own
//! bounds over `POST /projects`, which that suite walks over `PUT` only.
//!
//! Every helper and bound these suites need already exists: the HTTP harness
//! (`call`, `router_with_engine`, `is_modelled_id`, `REGION`) comes from the
//! asset suite, the portal helper from the access-policy suite, `list_tags`
//! from the resource-tags suite, and the project route, the two modelled length
//! maxima, the create body, the reader and the description assertion from the
//! sibling `project_lifecycle` suite — reused rather than copied, so the two
//! project suites cannot drift apart.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::access_policy_lifecycle::create_portal;
use crate::asset_lifecycle::{call, is_modelled_id, router_with_engine, REGION};
use crate::project_lifecycle::{
    assert_description, create_body, describe_project, stored_project_count,
    DESCRIPTION_MAX_LENGTH, NAME_MAX_LENGTH, PROJECTS,
};
use crate::resource_tags::list_tags;

/// The modelled `TagMap` maximum entry count (smithy_query query="operation"
/// operation="CreateProject": `tags` (TagMap, len 1..50)).
const TAGS_MAX_ENTRIES: usize = 50;

/// One row of the create table: what the request carries.
#[derive(Debug)]
struct CreateCase {
    label: &'static str,
    name: String,
    description: Option<String>,
    tags: Option<BTreeMap<String, String>>,
}

/// `TAGS_MAX_ENTRIES` distinct tags — the published ceiling of the map.
fn tags_at_the_maximum() -> BTreeMap<String, String> {
    (0..TAGS_MAX_ENTRIES)
        .map(|at| (format!("key-{at:02}"), format!("value-{at:02}")))
        .collect()
}

/// A tag set from `(key, value)` pairs.
fn tags_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The rows. They differ along every dimension the create's input has: whether
/// it carries a description at all, whether it carries tags and how many, and
/// the size and form of the two text members — one character, both published
/// maxima, non-ASCII text, and punctuation and spaces.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "required-members-only",
            name: "the-bare-project".to_string(),
            description: None,
            tags: None,
        },
        CreateCase {
            label: "name-and-description-at-the-minimum-length",
            name: "p".to_string(),
            description: Some("d".to_string()),
            tags: None,
        },
        CreateCase {
            label: "name-and-description-at-the-maximum-length",
            name: "n".repeat(NAME_MAX_LENGTH),
            description: Some("d".repeat(DESCRIPTION_MAX_LENGTH)),
            tags: None,
        },
        CreateCase {
            label: "non-ascii-name-and-description",
            name: "proyecto-ñandú-日本語".to_string(),
            description: Some("descripción du projet — 現場".to_string()),
            tags: None,
        },
        CreateCase {
            label: "name-with-spaces-and-punctuation",
            name: "Floor 1 / Line 2 (v1) — monitor".to_string(),
            description: Some("Floor 1 / Line 2 (v1): everything at once".to_string()),
            tags: None,
        },
        CreateCase {
            label: "one-tag",
            name: "the-tagged-project".to_string(),
            description: None,
            tags: Some(tags_of(&[("stage", "test")])),
        },
        CreateCase {
            label: "several-tags-beside-a-description",
            name: "the-many-tagged-project".to_string(),
            description: Some("three tags and a description".to_string()),
            tags: Some(tags_of(&[
                ("team", "ops"),
                ("env", "edge"),
                ("owner", "sitewise"),
            ])),
        },
        CreateCase {
            label: "tags-at-the-published-maximum-count",
            name: "the-fully-tagged-project".to_string(),
            description: None,
            tags: Some(tags_at_the_maximum()),
        },
    ]
}

/// The body one row sends: the sibling suite's create body, plus the tags only
/// a create can carry.
fn body_of(portal_id: &str, case: &CreateCase) -> Value {
    let mut body = create_body(portal_id, &case.name, case.description.as_deref());
    if let Some(tags) = case.tags.as_ref() {
        body["tags"] = json!(tags);
    }
    body
}

/// The tags `ListTagsForResource` reports for `arn`, asserting a 200.
///
/// A resource carrying no tag answers with an empty map, so an absent member is
/// read as one rather than panicked on.
async fn reported_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = list_tags(router, arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource on the minted ARN expected 200, got {status} {body}"
    );
    body.get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_create_persists_every_member_it_carries_for_every_input_shape() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-create-project-portal").await;

    let mut minted: BTreeMap<String, &'static str> = BTreeMap::new();

    for (at, case) in create_cases().iter().enumerate() {
        let (status, created) =
            call(&router, "POST", PROJECTS, Some(body_of(&portal_id, case))).await;
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
        if let Some(earlier) = minted.insert(id.clone(), case.label) {
            panic!(
                "case {:?}: the create re-used the id {id} already minted for case {earlier:?}",
                case.label
            );
        }

        let expected_arn = format!(
            "arn:aws:iotsitewise:{REGION}:{}:project/{id}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            created["projectArn"].as_str().unwrap_or(""),
            expected_arn,
            "case {:?}: projectArn must be the documented form, got {created}",
            case.label
        );

        // The round trip: the reader answers from what the create stored.
        let (status, described) = describe_project(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeProject expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["projectName"].as_str().unwrap_or(""),
            case.name,
            "case {:?}: projectName must read back as the create stated it, got {described}",
            case.label
        );
        assert_eq!(
            described["portalId"].as_str().unwrap_or(""),
            portal_id,
            "case {:?}: the project must live in the portal the create named, got {described}",
            case.label
        );
        assert_eq!(
            described["projectId"].as_str().unwrap_or(""),
            id,
            "case {:?}: DescribeProject must report the minted id, got {described}",
            case.label
        );
        assert_eq!(
            described["projectArn"].as_str().unwrap_or(""),
            expected_arn,
            "case {:?}: DescribeProject must report the minted ARN, got {described}",
            case.label
        );
        assert_description(case.label, &described, case.description.as_deref());

        let creation_date = described["projectCreationDate"].as_f64().unwrap_or(0.0);
        let last_update_date = described["projectLastUpdateDate"].as_f64().unwrap_or(0.0);
        assert!(
            creation_date > 0.0,
            "case {:?}: projectCreationDate must be a real instant, got {described}",
            case.label
        );
        assert_eq!(
            creation_date, last_update_date,
            "case {:?}: a project no update has touched reports its creation instant as its \
             last-update instant, got {described}",
            case.label
        );

        let reported = reported_tags(&router, &expected_arn, case.label).await;
        let expected_tags = case.tags.clone().unwrap_or_default();
        assert_eq!(
            reported, expected_tags,
            "case {:?}: the tags the create carried must be the tags the resource holds, \
             got {reported:?}",
            case.label
        );

        // A read changes nothing a later caller sees.
        let (status, again) = describe_project(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the second DescribeProject expected 200, got {status} {again}",
            case.label
        );
        assert_eq!(
            again, described,
            "case {:?}: DescribeProject answered differently on the second read: {again}",
            case.label
        );

        let stored = stored_project_count(&engine);
        assert_eq!(
            stored,
            at + 1,
            "case {:?}: each accepted create adds exactly one project, the store holds {stored}",
            case.label
        );
    }
}

#[tokio::test]
async fn identical_creates_mint_distinct_projects_in_the_portal_each_one_names() {
    let (router, engine) = router_with_engine();

    // Three portals, so the portal a project lands in can only come from the
    // request that created it.
    let mut portal_ids: Vec<String> = Vec::new();
    for name in ["portal-west", "portal-east", "portal-north"] {
        portal_ids.push(create_portal(&router, name).await);
    }

    // One request, sent twice per portal: a create carries no natural key, so
    // each call is its own resource even when the body is byte-identical.
    let shared = CreateCase {
        label: "the-shared-request",
        name: "the-shared-project-name".to_string(),
        description: Some("the shared description".to_string()),
        tags: Some(tags_of(&[("stage", "shared")])),
    };

    let mut by_portal: Vec<(String, Vec<String>)> = Vec::new();
    for portal_id in &portal_ids {
        let mut ids: Vec<String> = Vec::new();
        for attempt in ["first", "second"] {
            let (status, created) =
                call(&router, "POST", PROJECTS, Some(body_of(portal_id, &shared))).await;
            assert_eq!(
                status, 201,
                "case {attempt:?} in portal {portal_id}: expected 201, got {status} {created}"
            );
            ids.push(created["projectId"].as_str().unwrap_or("").to_string());
        }
        by_portal.push((portal_id.clone(), ids));
    }

    let all: Vec<&str> = by_portal
        .iter()
        .flat_map(|(_, ids)| ids.iter().map(String::as_str))
        .collect();
    let distinct = all
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<&str>>()
        .len();
    assert_eq!(
        distinct,
        all.len(),
        "every create is its own project, got the ids {all:?}"
    );

    for (portal_id, ids) in &by_portal {
        for id in ids {
            let (status, described) = describe_project(&router, id).await;
            assert_eq!(
                status, 200,
                "project {id} of portal {portal_id}: got {status} {described}"
            );
            assert_eq!(
                described["portalId"].as_str().unwrap_or(""),
                portal_id.as_str(),
                "project {id} must report the portal its own create named, got {described}"
            );
        }
    }

    let stored = stored_project_count(&engine);
    assert_eq!(
        stored,
        all.len(),
        "the store must hold one project per accepted create, it holds {stored}"
    );
}

#[tokio::test]
async fn a_create_outside_the_model_is_refused_and_persists_nothing() {
    let (router, engine) = router_with_engine();
    let portal_id = create_portal(&router, "the-create-bounds-portal").await;

    // The rows walk both published bounds of the two constrained text members —
    // the maxima themselves are accepted by the table above, so these are the
    // values one character past each and the empty string one below each
    // minimum — plus each required member absent, a `portalId` outside its
    // modelled length, and a character the members' pattern
    // `^[^\u0000-\u001F\u007F]+$` excludes.
    let cases: Vec<(&str, Value)> = vec![
        (
            "projectName one past the maximum length",
            create_body(&portal_id, &"n".repeat(NAME_MAX_LENGTH + 1), None),
        ),
        (
            "projectName one below the minimum length",
            create_body(&portal_id, "", None),
        ),
        (
            "projectDescription one past the maximum length",
            create_body(
                &portal_id,
                "fine-name",
                Some(&"d".repeat(DESCRIPTION_MAX_LENGTH + 1)),
            ),
        ),
        (
            "projectDescription one below the minimum length",
            create_body(&portal_id, "fine-name", Some("")),
        ),
        ("no projectName at all", json!({ "portalId": portal_id })),
        ("no portalId at all", json!({ "projectName": "fine-name" })),
        (
            "a portalId outside the modelled length",
            create_body("not-a-uuid", "fine-name", None),
        ),
        (
            "a control character in projectName, which its pattern excludes",
            create_body(&portal_id, "bad\u{1}name", None),
        ),
        (
            "DEL in projectDescription, which its pattern excludes",
            create_body(&portal_id, "fine-name", Some("north\u{7f}line")),
        ),
    ];

    for (case, body) in cases {
        // Only the STATUS is asserted: every row is refused by the boundary
        // validator before the handler runs, and the error CODE that walk
        // renders is the subject of its own issue — the same reason the sibling
        // project and portal suites record. The refusals the HANDLER itself
        // renders have their error code pinned by the sibling suites, on the
        // tag-key bound and on a portal that does not exist.
        let (status, answer) = call(&router, "POST", PROJECTS, Some(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the model expected 400, got {status} {answer}"
        );
    }

    // A refused create minted nothing: the store is as empty as it was.
    let stored = stored_project_count(&engine);
    assert_eq!(
        stored, 0,
        "a refused create must persist nothing, the store holds {stored} projects"
    );
}
