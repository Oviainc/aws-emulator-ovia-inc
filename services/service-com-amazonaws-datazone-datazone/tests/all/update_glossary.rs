// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateGlossary`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — and reuses the
//! `Emulator` harness from [`crate::harness`], so nothing here reaches into
//! `crate::state` and a handler answering from a hardcoded value would fail.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `UpdateGlossary` is a PATCH of the stored glossary: a member the request carries
//!    replaces the stored value, a member it omits leaves the stored value untouched,
//!    and the answer is what the next `GetGlossary` reports — so a status toggled to
//!    `DISABLED` and back is observable both times.
//! 2. The update is what sets `updatedBy`/`updatedAt`; `createdBy`/`createdAt`,
//!    `owningProjectId` and `usageRestrictions` belong to the create and survive it.
//! 3. A patched name stays unique WITHIN the domain: another glossary of that domain
//!    already carrying the name is a conflict, the glossary re-asserting its own name is
//!    not, and the same name in another domain is a different namespace.
//! 4. The glossary must exist in the named domain, which must itself still be there.
//! 5. A patch outside the modelled constraints is refused, and a refused patch leaves
//!    the stored glossary exactly as it was.

use crate::harness::Emulator;
use serde_json::{json, Value};
// The `GlossaryStatus` members as the service stores them, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    GLOSSARY_STATUS_DISABLED, GLOSSARY_STATUS_ENABLED,
};

/// The principal the harness signs with: `createdBy`/`updatedBy` come from the caller's
/// SigV4 credential scope, never from the request body.
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY";

/// One `UpdateGlossary` case: the glossary the patch is applied to, the patch, and the
/// record the glossary must carry afterwards.
struct PatchCase {
    label: &'static str,
    /// The `CreateGlossary` body, less `owningProjectIdentifier`.
    create: Value,
    /// The `UpdateGlossary` body.
    patch: Value,
    expect_name: String,
    expect_description: Option<String>,
    expect_status: &'static str,
    /// `usageRestrictions` is not an `UpdateGlossary` member, so it must read back
    /// exactly as the create left it.
    expect_usage_restrictions: Option<Value>,
}

/// The cases span what the rule generalises over: which mutable members the patch
/// carries (none, one, all of them), each `GlossaryStatus` member in both directions,
/// and the published bounds of the members' values.
fn patch_cases() -> Vec<PatchCase> {
    vec![
        PatchCase {
            label: "status only: ENABLED toggled to DISABLED",
            create: json!({
                "name": "toggle one",
                "description": "the shared vocabulary",
                "status": "ENABLED",
            }),
            patch: json!({ "status": "DISABLED" }),
            expect_name: "toggle one".to_string(),
            expect_description: Some("the shared vocabulary".to_string()),
            expect_status: GLOSSARY_STATUS_DISABLED,
            expect_usage_restrictions: None,
        },
        PatchCase {
            label: "status only: DISABLED toggled back to ENABLED",
            create: json!({ "name": "toggle two", "status": "DISABLED" }),
            patch: json!({ "status": "ENABLED" }),
            expect_name: "toggle two".to_string(),
            // The create named no description, and the patch names none either.
            expect_description: None,
            expect_status: GLOSSARY_STATUS_ENABLED,
            expect_usage_restrictions: None,
        },
        PatchCase {
            label: "name only, at `GlossaryName`'s 1-character minimum",
            create: json!({ "name": "rename three", "description": "kept" }),
            // `GlossaryName` is 1..256 (smithy_query shape="GlossaryName").
            patch: json!({ "name": "g" }),
            expect_name: "g".to_string(),
            expect_description: Some("kept".to_string()),
            // The create named no status, so the glossary carries the usable member and
            // a patch that names none leaves it there.
            expect_status: GLOSSARY_STATUS_ENABLED,
            expect_usage_restrictions: None,
        },
        PatchCase {
            label: "name only, at `GlossaryName`'s 256-character maximum",
            create: json!({ "name": "rename four", "status": "DISABLED" }),
            patch: json!({ "name": "n".repeat(256) }),
            expect_name: "n".repeat(256),
            expect_description: None,
            // A patch that names no status must not resurrect the default.
            expect_status: GLOSSARY_STATUS_DISABLED,
            expect_usage_restrictions: None,
        },
        PatchCase {
            label: "description only, at `GlossaryDescription`'s 4096-character maximum",
            create: json!({ "name": "describe five", "description": "replaced" }),
            // `GlossaryDescription` is ..4096 (smithy_query shape="GlossaryDescription").
            patch: json!({ "description": "d".repeat(4096) }),
            expect_name: "describe five".to_string(),
            expect_description: Some("d".repeat(4096)),
            expect_status: GLOSSARY_STATUS_ENABLED,
            expect_usage_restrictions: None,
        },
        PatchCase {
            label: "description only, emptied to `GlossaryDescription`'s 0-character minimum",
            create: json!({ "name": "describe six", "description": "replaced" }),
            patch: json!({ "description": "" }),
            expect_name: "describe six".to_string(),
            expect_description: Some(String::new()),
            expect_status: GLOSSARY_STATUS_ENABLED,
            expect_usage_restrictions: None,
        },
        PatchCase {
            label: "every mutable member at once, over a glossary with a usage restriction",
            create: json!({
                "name": "all seven",
                "description": "the old text",
                "status": "ENABLED",
                // `GlossaryUsageRestrictions` is a 1..1 list of
                // `GlossaryUsageRestriction`, whose only member is ASSET_GOVERNED_TERMS
                // (smithy_query shape="GlossaryUsageRestriction").
                "usageRestrictions": ["ASSET_GOVERNED_TERMS"],
            }),
            patch: json!({
                "name": "all seven renamed",
                "description": "the new text",
                "status": "DISABLED",
            }),
            expect_name: "all seven renamed".to_string(),
            expect_description: Some("the new text".to_string()),
            expect_status: GLOSSARY_STATUS_DISABLED,
            expect_usage_restrictions: Some(json!(["ASSET_GOVERNED_TERMS"])),
        },
        PatchCase {
            label: "a patch carrying no mutable member at all",
            create: json!({
                "name": "untouched eight",
                "description": "the only text",
                "status": "DISABLED",
                "usageRestrictions": ["ASSET_GOVERNED_TERMS"],
            }),
            patch: json!({}),
            expect_name: "untouched eight".to_string(),
            expect_description: Some("the only text".to_string()),
            expect_status: GLOSSARY_STATUS_DISABLED,
            expect_usage_restrictions: Some(json!(["ASSET_GOVERNED_TERMS"])),
        },
        PatchCase {
            label: "a patch carrying only a clientToken",
            create: json!({ "name": "token nine", "description": "the only text" }),
            // `ClientToken` is 1..128 of `^[\x21-\x7E]+$` (smithy_query
            // shape="ClientToken").
            patch: json!({ "clientToken": "update-token-9" }),
            expect_name: "token nine".to_string(),
            expect_description: Some("the only text".to_string()),
            expect_status: GLOSSARY_STATUS_ENABLED,
            expect_usage_restrictions: None,
        },
    ]
}

/// Create a domain that has settled to `AVAILABLE` plus one project in it: the
/// prerequisites every glossary needs.
async fn domain_with_project(dz: &Emulator, name: &str) -> (String, String) {
    let domain = dz.available_domain(name).await;
    let project = dz.project(&domain, &format!("{name} owner")).await;
    (domain, project)
}

/// Create one glossary from `body` and answer its id.
async fn create_glossary(dz: &Emulator, domain: &str, project: &str, body: &Value) -> String {
    let mut body = body.clone();
    body["owningProjectIdentifier"] = json!(project);
    let created = dz
        .call("POST", &format!("/v2/domains/{domain}/glossaries"), Some(body))
        .await;
    assert_eq!(
        created.status, 201,
        "creating the glossary to patch: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// Assert that `body` reports the record `case` expects, whichever operation answered it.
fn assert_record(case: &PatchCase, who: &str, body: &Value, domain: &str, project: &str, id: &str) {
    assert_eq!(
        body.get("id").and_then(Value::as_str),
        Some(id),
        "case {:?}: {who} must report the glossary that was patched, got {:?}",
        case.label,
        body.get("id")
    );
    assert_eq!(
        body.get("domainId").and_then(Value::as_str),
        Some(domain),
        "case {:?}: {who} must report the stored domain, got {:?}",
        case.label,
        body.get("domainId")
    );
    assert_eq!(
        body.get("owningProjectId").and_then(Value::as_str),
        Some(project),
        "case {:?}: {who} must report the owning project the create set, got {:?}",
        case.label,
        body.get("owningProjectId")
    );
    assert_eq!(
        body.get("name").and_then(Value::as_str),
        Some(case.expect_name.as_str()),
        "case {:?}: {who} expected name {:?}, got {:?}",
        case.label,
        case.expect_name,
        body.get("name")
    );
    assert_eq!(
        body.get("description").and_then(Value::as_str),
        case.expect_description.as_deref(),
        "case {:?}: {who} expected description {:?}, got {:?}",
        case.label,
        case.expect_description,
        body.get("description")
    );
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some(case.expect_status),
        "case {:?}: {who} expected status {:?}, got {:?}",
        case.label,
        case.expect_status,
        body.get("status")
    );
    assert_eq!(
        body.get("usageRestrictions").cloned(),
        case.expect_usage_restrictions.clone(),
        "case {:?}: {who} expected usageRestrictions {:?}, got {:?}",
        case.label,
        case.expect_usage_restrictions,
        body.get("usageRestrictions")
    );
}

#[tokio::test]
async fn a_patch_replaces_the_members_it_carries_and_persists_the_rest() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "patched-glossaries").await;

    for case in patch_cases() {
        let id = create_glossary(&dz, &domain, &project, &case.create).await;
        let uri = format!("/v2/domains/{domain}/glossaries/{id}");

        // What the create left behind, so the patch can be judged against it.
        let before = dz.call("GET", &uri, None).await;
        assert_eq!(
            before.status, 200,
            "case {:?}: reading the glossary before the patch: expected 200, got {} {:?}",
            case.label, before.status, before.body
        );
        assert!(
            before.body.get("updatedAt").is_none() && before.body.get("updatedBy").is_none(),
            "case {:?}: a glossary nothing has updated must report no updatedAt/updatedBy, got {:?}",
            case.label,
            before.body
        );

        let patched = dz.call("PATCH", &uri, Some(case.patch.clone())).await;
        assert_eq!(
            patched.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.label, patched.status, patched.body
        );
        assert_record(&case, "UpdateGlossary", &patched.body, &domain, &project, &id);

        // The patch is persisted: the next read reports the same record, which is what
        // tells a stored update apart from one computed only for this answer.
        let after = dz.call("GET", &uri, None).await;
        assert_eq!(
            after.status, 200,
            "case {:?}: reading the glossary back: expected 200, got {} {:?}",
            case.label, after.status, after.body
        );
        assert_record(&case, "GetGlossary", &after.body, &domain, &project, &id);

        // `createdBy`/`createdAt` belong to the create, so the patch leaves them alone.
        assert_eq!(
            after.body.get("createdBy").cloned(),
            before.body.get("createdBy").cloned(),
            "case {:?}: the patch must not rewrite createdBy, got {:?}",
            case.label,
            after.body.get("createdBy")
        );
        assert_eq!(
            after.body.get("createdAt").cloned(),
            before.body.get("createdAt").cloned(),
            "case {:?}: the patch must not rewrite createdAt, got {:?}",
            case.label,
            after.body.get("createdAt")
        );
        // The update is what sets `updatedBy`/`updatedAt` — the signing principal, and
        // an instant no earlier than the create's.
        assert_eq!(
            after.s("updatedBy"),
            CALLER,
            "case {:?}: expected the signing principal as updatedBy, got {:?}",
            case.label,
            after.s("updatedBy")
        );
        let created_at = before.body.get("createdAt").and_then(Value::as_f64);
        let updated_at = after.body.get("updatedAt").and_then(Value::as_f64);
        assert!(
            updated_at.is_some() && updated_at >= created_at,
            "case {:?}: expected an updatedAt no earlier than createdAt {created_at:?}, got {updated_at:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_patched_name_stays_unique_within_its_domain() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "renamed-glossaries").await;
    let (other_domain, other_project) = domain_with_project(&dz, "renamed-elsewhere").await;

    for (index, case) in patch_cases().into_iter().enumerate() {
        // A second glossary of this domain holds the name the patch will ask for.
        let taken = format!("taken name {index}");
        create_glossary(&dz, &domain, &project, &json!({ "name": taken })).await;
        let id = create_glossary(&dz, &domain, &project, &case.create).await;
        let uri = format!("/v2/domains/{domain}/glossaries/{id}");
        let created_name = case
            .create
            .get("name")
            .and_then(Value::as_str)
            .expect("every case creates a named glossary")
            .to_string();

        let clash = dz
            .call("PATCH", &uri, Some(json!({ "name": taken })))
            .await;
        assert_eq!(
            (clash.status, clash.error_code.as_str()),
            (409, "ConflictException"),
            "case {:?}: a name another glossary of this domain holds must conflict, got {} {:?}",
            case.label,
            clash.status,
            clash.body
        );
        // The refused patch wrote nothing: the glossary still carries its own name and
        // still reports no update.
        let untouched = dz.call("GET", &uri, None).await;
        assert_eq!(
            untouched.s("name"),
            created_name,
            "case {:?}: a refused patch must leave the stored name alone, got {:?}",
            case.label,
            untouched.s("name")
        );
        assert!(
            untouched.body.get("updatedAt").is_none(),
            "case {:?}: a refused patch must not set updatedAt, got {:?}",
            case.label,
            untouched.body
        );

        // A glossary re-asserting its OWN name is not a clash with itself.
        let same = dz
            .call("PATCH", &uri, Some(json!({ "name": created_name.clone() })))
            .await;
        assert_eq!(
            same.status, 200,
            "case {:?}: re-asserting the glossary's own name must be accepted, got {} {:?}",
            case.label, same.status, same.body
        );
        assert_eq!(
            same.s("name"),
            created_name,
            "case {:?}: expected the name to stand, got {:?}",
            case.label,
            same.s("name")
        );

        // The same name in ANOTHER domain is a different namespace, so a glossary there
        // may take it.
        let elsewhere_id =
            create_glossary(&dz, &other_domain, &other_project, &json!({ "name": "to rename" }))
                .await;
        let elsewhere = dz
            .call(
                "PATCH",
                &format!("/v2/domains/{other_domain}/glossaries/{elsewhere_id}"),
                Some(json!({ "name": created_name.clone() })),
            )
            .await;
        assert_eq!(
            elsewhere.status, 200,
            "case {:?}: a name used in another domain must be accepted here, got {} {:?}",
            case.label, elsewhere.status, elsewhere.body
        );
        assert_eq!(
            elsewhere.s("name"),
            created_name,
            "case {:?}: expected the renamed glossary to carry the name, got {:?}",
            case.label,
            elsewhere.s("name")
        );
    }
}

#[tokio::test]
async fn an_update_needs_the_glossary_to_exist_in_the_named_domain() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "missing-glossaries").await;
    let (other_domain, _) = domain_with_project(&dz, "missing-glossaries-other").await;
    let id = create_glossary(&dz, &domain, &project, &json!({ "name": "the only one" })).await;

    // (label, domain, glossary id, patch, expected status, expected wire error code)
    let cases: Vec<(&str, String, String, Value, u16, &str)> = vec![
        (
            "a glossary id nothing created",
            domain.clone(),
            "nosuchglossary".to_string(),
            json!({ "status": "DISABLED" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the right glossary id under the wrong domain",
            other_domain.clone(),
            id.clone(),
            json!({ "status": "DISABLED" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // update before any lookup.
            "a well-formed domain id nothing created",
            "dzd_nosuchdomain000".to_string(),
            id.clone(),
            json!({ "status": "DISABLED" }),
            403,
            "AccessDeniedException",
        ),
        (
            // `DomainId` is `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query
            // shape="DomainId").
            "a malformed domain identifier",
            "not-a-domain".to_string(),
            id.clone(),
            json!({ "status": "DISABLED" }),
            400,
            "ValidationException",
        ),
        (
            // `GlossaryId` is `^[a-zA-Z0-9_-]{1,36}$` (smithy_query shape="GlossaryId").
            "a glossary identifier one character past the modelled 36 maximum",
            domain.clone(),
            "i".repeat(37),
            json!({ "status": "DISABLED" }),
            400,
            "ValidationException",
        ),
    ];
    for (label, domain_id, glossary_id, patch, status, code) in cases {
        let answer = dz
            .call(
                "PATCH",
                &format!("/v2/domains/{domain_id}/glossaries/{glossary_id}"),
                Some(patch),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // A glossary goes with its domain: once the domain is on its way out it holds no
    // glossary to patch.
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    let gone = dz
        .call(
            "PATCH",
            &format!("/v2/domains/{domain}/glossaries/{id}"),
            Some(json!({ "status": "DISABLED" })),
        )
        .await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a glossary whose domain is gone must not be patchable, got {} {:?}",
        gone.status,
        gone.body
    );
}

#[tokio::test]
async fn a_patch_outside_the_modelled_constraints_is_refused() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "refused-patches").await;
    let create = json!({ "name": "the stored one", "description": "the stored text" });
    let id = create_glossary(&dz, &domain, &project, &create).await;
    let uri = format!("/v2/domains/{domain}/glossaries/{id}");

    let cases: Vec<(&str, Value)> = vec![
        (
            "a name one character past `GlossaryName`'s 256 maximum",
            json!({ "name": "n".repeat(257) }),
        ),
        (
            "a name below `GlossaryName`'s 1-character minimum",
            json!({ "name": "" }),
        ),
        (
            "a description one character past `GlossaryDescription`'s 4096 maximum",
            json!({ "description": "d".repeat(4097) }),
        ),
        (
            // `GlossaryStatus` is DISABLED | ENABLED (smithy_query
            // shape="GlossaryStatus"), so neither a status of another resource's enum
            // nor the right member in the wrong case is one of its values.
            "a status that is not a GlossaryStatus member",
            json!({ "status": "ARCHIVED" }),
        ),
        (
            "a GlossaryStatus member in the wrong case",
            json!({ "status": "enabled" }),
        ),
        (
            // `ClientToken` is 1..128 of `^[\x21-\x7E]+$`.
            "a clientToken below the modelled 1-character minimum",
            json!({ "clientToken": "" }),
        ),
    ];
    for (label, patch) in cases {
        let answer = dz.call("PATCH", &uri, Some(patch)).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
        // Nothing was written: the stored glossary is still what the create left.
        let stored = dz.call("GET", &uri, None).await;
        assert_eq!(
            (stored.s("name"), stored.s("description")),
            ("the stored one".to_string(), "the stored text".to_string()),
            "case {label:?}: a refused patch must leave the stored glossary alone, got {:?}",
            stored.body
        );
        assert!(
            stored.body.get("updatedAt").is_none(),
            "case {label:?}: a refused patch must not set updatedAt, got {:?}",
            stored.body
        );
    }
}
