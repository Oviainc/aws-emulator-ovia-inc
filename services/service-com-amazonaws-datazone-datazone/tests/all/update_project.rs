// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateProject`.
//!
//! The rule under test: an update replaces exactly the members the request carries,
//! leaves every omitted member at its stored value, refreshes `lastUpdatedAt` while
//! never rewriting `createdAt`/`createdBy`, and reports the whole project as it is now
//! stored — so a later `GetProject` answers the same thing.
//!
//! Each rule is asserted over a TABLE of patches that differ in which members they
//! carry, in value size (both published bounds and one past them), in collection count
//! and in whether a collection is replaced or cleared. Nothing reaches into
//! `crate::state`: every assertion is made on a response body, so a handler answering
//! from anything but the store would fail here.

use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The project every case starts from: every member `UpdateProject` can change is set,
/// so a patch that omits one proves the stored value survives.
fn baseline() -> Value {
    json!({
        "name": "baseline project",
        "description": "baseline description",
        // `projectCategory` is NOT an UpdateProject input member (smithy_query
        // operation="UpdateProject"), so it must survive every patch.
        "projectCategory": "ANALYTICS",
        "domainUnitId": "unit-base_1",
        "glossaryTerms": ["base_a", "base_b", "base_c"],
        "resourceTags": { "env": "dev", "owner": "team" },
    })
}

/// The project state a case expects after its patch, in the form the wire reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Expected {
    name: String,
    description: Option<String>,
    glossary_terms: Vec<String>,
    domain_unit_id: Option<String>,
    resource_tags: BTreeMap<String, String>,
}

impl Expected {
    /// The baseline project as stored by the create above.
    fn baseline() -> Self {
        Self {
            name: "baseline project".to_string(),
            description: Some("baseline description".to_string()),
            glossary_terms: vec![
                "base_a".to_string(),
                "base_b".to_string(),
                "base_c".to_string(),
            ],
            domain_unit_id: Some("unit-base_1".to_string()),
            resource_tags: [("env", "dev"), ("owner", "team")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// `glossaryTerms` is reported only when the project has terms: the modelled list
    /// has `min: 1` (smithy_query operation="UpdateProject"), so an empty one is absent.
    fn glossary_json(&self) -> Option<Value> {
        (!self.glossary_terms.is_empty()).then(|| json!(self.glossary_terms))
    }

    /// `resourceTags` on the wire: a list of `ResourceTag`, one per stored key in key
    /// order, each tagged with its `ResourceTagSource` (PROJECT | PROJECT_PROFILE —
    /// smithy_query shape="ResourceTagSource"); a tag the request carried is the
    /// project's own, so PROJECT.
    fn tags_json(&self) -> Value {
        Value::Array(
            self.resource_tags
                .iter()
                .map(|(key, value)| json!({ "key": key, "value": value, "source": "PROJECT" }))
                .collect(),
        )
    }
}

/// One row of the table: the patch body, and what the project must look like after it.
struct UpdateCase {
    label: &'static str,
    patch: Value,
    expect: fn(&mut Expected),
}

fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "an empty patch changes nothing",
            patch: json!({}),
            expect: |_| {},
        },
        UpdateCase {
            label: "name only: every other member survives",
            patch: json!({ "name": "renamed project" }),
            expect: |e| e.name = "renamed project".to_string(),
        },
        UpdateCase {
            label: "description only",
            patch: json!({ "description": "a replacement description" }),
            expect: |e| e.description = Some("a replacement description".to_string()),
        },
        UpdateCase {
            label: "description emptied (Description has no minimum length)",
            patch: json!({ "description": "" }),
            expect: |e| e.description = Some(String::new()),
        },
        UpdateCase {
            label: "glossaryTerms replaced by a shorter list",
            patch: json!({ "glossaryTerms": ["only_one"] }),
            expect: |e| e.glossary_terms = vec!["only_one".to_string()],
        },
        UpdateCase {
            label: "glossaryTerms replaced at the modelled maximum of 20",
            patch: json!({ "glossaryTerms": (0..20).map(|i| format!("term_{i}")).collect::<Vec<_>>() }),
            expect: |e| e.glossary_terms = (0..20).map(|i| format!("term_{i}")).collect(),
        },
        UpdateCase {
            label: "resourceTags replaced by a different set of keys",
            patch: json!({ "resourceTags": { "stage": "prod", "cost": "42", "alpha": "x" } }),
            expect: |e| {
                e.resource_tags = [("alpha", "x"), ("cost", "42"), ("stage", "prod")]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
            },
        },
        UpdateCase {
            label: "resourceTags cleared by an empty map",
            patch: json!({ "resourceTags": {} }),
            expect: |e| e.resource_tags = BTreeMap::new(),
        },
        UpdateCase {
            label: "resourceTags at the modelled maximum of 25 entries",
            patch: json!({
                "resourceTags": (0..25)
                    .map(|i| (format!("k{i:02}"), Value::String(i.to_string())))
                    .collect::<serde_json::Map<String, Value>>()
            }),
            expect: |e| {
                e.resource_tags = (0..25).map(|i| (format!("k{i:02}"), i.to_string())).collect();
            },
        },
        UpdateCase {
            label: "domainUnitId only",
            patch: json!({ "domainUnitId": "unit-moved_9" }),
            expect: |e| e.domain_unit_id = Some("unit-moved_9".to_string()),
        },
        UpdateCase {
            // `ProjectName` is 1..64 of `^[\w -]+$` (smithy_query operation="UpdateProject").
            label: "a name at the modelled 1-character minimum",
            patch: json!({ "name": "p" }),
            expect: |e| e.name = "p".to_string(),
        },
        UpdateCase {
            label: "a name at the modelled 64-character maximum",
            patch: json!({ "name": "n".repeat(64) }),
            expect: |e| e.name = "n".repeat(64),
        },
        UpdateCase {
            // `Description` is ..2048 (smithy_query operation="UpdateProject").
            label: "a description at the modelled 2048-character maximum",
            patch: json!({ "description": "d".repeat(2048) }),
            expect: |e| e.description = Some("d".repeat(2048)),
        },
        UpdateCase {
            // `DomainUnitId` is 1..256 of `^[a-z0-9_\-]+$`.
            label: "a domainUnitId at the modelled 256-character maximum",
            patch: json!({ "domainUnitId": "u".repeat(256) }),
            expect: |e| e.domain_unit_id = Some("u".repeat(256)),
        },
        UpdateCase {
            label: "every updatable member at once",
            patch: json!({
                "name": "all_at once-1",
                "description": "all at once",
                "domainUnitId": "unit_all-1",
                "glossaryTerms": ["g_1", "g_2"],
                "resourceTags": { "one": "1" },
            }),
            expect: |e| {
                e.name = "all_at once-1".to_string();
                e.description = Some("all at once".to_string());
                e.domain_unit_id = Some("unit_all-1".to_string());
                e.glossary_terms = vec!["g_1".to_string(), "g_2".to_string()];
                e.resource_tags = [("one", "1")]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
            },
        },
    ]
}

/// Assert one answer reports exactly `expect`, plus the members an update never
/// changes: the project's identity, its category, and who/when created it.
fn assert_project(answer: &Answer, label: &str, source: &str, expect: &Expected, created: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {label:?} ({source}): expected 200, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.s("name"),
        expect.name,
        "case {label:?} ({source}): expected name {:?}, got {:?}",
        expect.name,
        answer.s("name")
    );
    assert_eq!(
        answer.body.get("description").and_then(Value::as_str),
        expect.description.as_deref(),
        "case {label:?} ({source}): expected description {:?}, got {:?}",
        expect.description,
        answer.body.get("description")
    );
    assert_eq!(
        answer.body.get("glossaryTerms").cloned(),
        expect.glossary_json(),
        "case {label:?} ({source}): expected glossaryTerms {:?}, got {:?}",
        expect.glossary_json(),
        answer.body.get("glossaryTerms")
    );
    assert_eq!(
        answer.body.get("domainUnitId").and_then(Value::as_str),
        expect.domain_unit_id.as_deref(),
        "case {label:?} ({source}): expected domainUnitId {:?}, got {:?}",
        expect.domain_unit_id,
        answer.body.get("domainUnitId")
    );
    assert_eq!(
        answer
            .body
            .get("resourceTags")
            .cloned()
            .unwrap_or(Value::Null),
        expect.tags_json(),
        "case {label:?} ({source}): expected resourceTags {:?}, got {:?}",
        expect.tags_json(),
        answer.body.get("resourceTags")
    );
    for member in ["id", "domainId", "projectCategory", "createdBy"] {
        assert_eq!(
            answer.s(member),
            created.s(member),
            "case {label:?} ({source}): an update must not change {member}, got {:?} (was {:?})",
            answer.s(member),
            created.s(member)
        );
    }
    assert_eq!(
        answer.body.get("createdAt").cloned(),
        created.body.get("createdAt").cloned(),
        "case {label:?} ({source}): an update must not rewrite createdAt, got {:?} (was {:?})",
        answer.body.get("createdAt"),
        created.body.get("createdAt")
    );
    assert_eq!(
        answer.s("projectStatus"),
        "ACTIVE",
        "case {label:?} ({source}): expected ACTIVE, got {:?}",
        answer.s("projectStatus")
    );
    assert!(
        answer.body.get("lastUpdatedAt").is_some(),
        "case {label:?} ({source}): expected lastUpdatedAt, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_update_replaces_the_members_it_carries_and_keeps_the_rest() {
    let dz = Emulator::new();
    let domain = dz.available_domain("update-project-domain").await;
    let projects_uri = format!("/v2/domains/{domain}/projects");

    for case in update_cases() {
        // Each case gets its own project, so one patch cannot mask another.
        let created = dz.call("POST", &projects_uri, Some(baseline())).await;
        assert_eq!(
            created.status, 201,
            "case {:?}: the baseline project must be created, got {} {:?}",
            case.label, created.status, created.body
        );
        let id = created.s("id");
        let project_uri = format!("{projects_uri}/{id}");

        let mut expect = Expected::baseline();
        (case.expect)(&mut expect);

        let updated = dz
            .call("PATCH", &project_uri, Some(case.patch.clone()))
            .await;
        assert_project(&updated, case.label, "UpdateProject", &expect, &created);

        // The update is PERSISTED, not just echoed: the next read reports it.
        let read_back = dz.call("GET", &project_uri, None).await;
        assert_project(&read_back, case.label, "GetProject", &expect, &created);
    }
}

#[tokio::test]
async fn repeated_updates_compose_and_each_one_moves_last_updated_at() {
    let dz = Emulator::new();
    let domain = dz.available_domain("update-project-compose").await;
    let projects_uri = format!("/v2/domains/{domain}/projects");
    let created = dz.call("POST", &projects_uri, Some(baseline())).await;
    let project_uri = format!("{projects_uri}/{}", created.s("id"));

    // Three patches, each carrying a different single member: the project ends up
    // holding all three changes, so no patch undoes an earlier one.
    let first = dz
        .call("PATCH", &project_uri, Some(json!({ "name": "step one" })))
        .await;
    // A full second between writes, so the two instants differ in every timestamp
    // precision the wire format can use.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let second = dz
        .call(
            "PATCH",
            &project_uri,
            Some(json!({ "description": "step two" })),
        )
        .await;
    let third = dz
        .call(
            "PATCH",
            &project_uri,
            Some(json!({ "glossaryTerms": ["step_three"] })),
        )
        .await;

    let mut expect = Expected::baseline();
    expect.name = "step one".to_string();
    expect.description = Some("step two".to_string());
    expect.glossary_terms = vec!["step_three".to_string()];
    let read_back = dz.call("GET", &project_uri, None).await;
    assert_project(&read_back, "three composed patches", "GetProject", &expect, &created);
    for (label, answer) in [("first", &first), ("second", &second), ("third", &third)] {
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // `lastUpdatedAt` tracks the write that last touched the project.
    assert_ne!(
        first.body.get("lastUpdatedAt"),
        second.body.get("lastUpdatedAt"),
        "case \"second patch\": expected lastUpdatedAt to move with the write, got {:?} then {:?}",
        first.body.get("lastUpdatedAt"),
        second.body.get("lastUpdatedAt")
    );
    assert_eq!(
        read_back.body.get("lastUpdatedAt"),
        third.body.get("lastUpdatedAt"),
        "case \"read back\": a read must report the stored lastUpdatedAt, got {:?} (update said {:?})",
        read_back.body.get("lastUpdatedAt"),
        third.body.get("lastUpdatedAt")
    );
}

#[tokio::test]
async fn an_update_is_scoped_to_its_domain_and_refuses_what_the_model_rejects() {
    let dz = Emulator::new();
    let domain = dz.available_domain("update-scope-domain").await;
    let other_domain = dz.available_domain("update-scope-other").await;
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(baseline()),
        )
        .await;
    let id = created.s("id");

    // (label, domain, project id, patch, expected status, expected wire error code)
    let cases: Vec<(&str, String, String, Value, u16, &str)> = vec![
        (
            "a project that does not exist",
            domain.clone(),
            "doesnotexist".to_string(),
            json!({ "name": "x" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the right project id under the wrong domain",
            other_domain.clone(),
            id.clone(),
            json!({ "name": "x" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // update before any lookup.
            "a domain that does not exist",
            "dzd_doesnotexist".to_string(),
            id.clone(),
            json!({ "name": "x" }),
            403,
            "AccessDeniedException",
        ),
        (
            // `DomainId` is `^dzd[-_][a-zA-Z0-9_-]{1,36}$`.
            "a malformed domain identifier",
            "not-a-domain".to_string(),
            id.clone(),
            json!({ "name": "x" }),
            400,
            "ValidationException",
        ),
        (
            "a name one character past the modelled 64 maximum",
            domain.clone(),
            id.clone(),
            json!({ "name": "n".repeat(65) }),
            400,
            "ValidationException",
        ),
        (
            "a name below the modelled 1-character minimum",
            domain.clone(),
            id.clone(),
            json!({ "name": "" }),
            400,
            "ValidationException",
        ),
        (
            "a name outside `^[\\w -]+$`",
            domain.clone(),
            id.clone(),
            json!({ "name": "no/slashes" }),
            400,
            "ValidationException",
        ),
        (
            "a description one character past the modelled 2048 maximum",
            domain.clone(),
            id.clone(),
            json!({ "description": "d".repeat(2049) }),
            400,
            "ValidationException",
        ),
        (
            "glossaryTerms one entry past the modelled 20 maximum",
            domain.clone(),
            id.clone(),
            json!({ "glossaryTerms": (0..21).map(|i| format!("t{i}")).collect::<Vec<_>>() }),
            400,
            "ValidationException",
        ),
        (
            "a domainUnitId outside `^[a-z0-9_\\-]+$`",
            domain.clone(),
            id.clone(),
            json!({ "domainUnitId": "UPPER" }),
            400,
            "ValidationException",
        ),
        (
            "a domainUnitId one character past the modelled 256 maximum",
            domain.clone(),
            id.clone(),
            json!({ "domainUnitId": "u".repeat(257) }),
            400,
            "ValidationException",
        ),
    ];

    for (label, domain_id, project_id, patch, status, code) in cases {
        let answer = dz
            .call(
                "PATCH",
                &format!("/v2/domains/{domain_id}/projects/{project_id}"),
                Some(patch),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }

    // A refused update leaves the stored project exactly as it was.
    let read_back = dz
        .call("GET", &format!("/v2/domains/{domain}/projects/{id}"), None)
        .await;
    assert_project(
        &read_back,
        "after every refusal",
        "GetProject",
        &Expected::baseline(),
        &created,
    );
}

#[tokio::test]
async fn an_update_that_needs_environments_or_project_profiles_is_still_a_501() {
    let dz = Emulator::new();
    let domain = dz.available_domain("update-not-implemented").await;
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(baseline()),
        )
        .await;
    let project_uri = format!("/v2/domains/{domain}/projects/{}", created.s("id"));

    // Environment configuration and project profiles are separate resources this
    // emulator does not serve yet, so a patch asking for them is refused honestly
    // rather than accepted and dropped.
    let cases: Vec<(&str, Value)> = vec![
        (
            "projectProfileVersion",
            json!({ "projectProfileVersion": "latest" }),
        ),
        (
            "userParameters",
            json!({ "userParameters": [{ "environmentId": "env-1" }] }),
        ),
        (
            "environmentDeploymentDetails",
            json!({ "environmentDeploymentDetails": { "overallDeploymentStatus": "SUCCESSFUL" } }),
        ),
    ];
    for (label, patch) in cases {
        let answer = dz.call("PATCH", &project_uri, Some(patch)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (501, "NotImplemented"),
            "case {label:?}: expected 501 NotImplemented, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }

    // The refusals persisted nothing.
    let read_back = dz.call("GET", &project_uri, None).await;
    assert_project(
        &read_back,
        "after every 501",
        "GetProject",
        &Expected::baseline(),
        &created,
    );
}
