// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DataZone business-glossary lifecycle.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — and reuses the
//! `Emulator` harness from [`crate::harness`], so nothing here reaches
//! into `crate::state` and a handler answering from a hardcoded value would fail.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateGlossary` mints a glossary under a persisted, available domain and stores
//!    the whole record, so `GetGlossary` reports exactly what was stored.
//! 2. A glossary name is unique WITHIN a domain: the same name in another domain is
//!    accepted, a repeat in the same domain is refused with the modelled conflict.
//! 3. The documented prerequisites are enforced against the store: the domain must
//!    exist and be available, and the owning project must be one of that domain's.
//! 4. A glossary is scoped to its domain and disappears with it.
//! 5. A repeated `clientToken` returns the first glossary instead of a second one.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};
// The `GlossaryStatus` members as the service stores them, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    GLOSSARY_STATUS_DISABLED, GLOSSARY_STATUS_ENABLED,
};

/// `GlossaryId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
/// shape="GlossaryId").
fn is_glossary_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// One `CreateGlossary` request, plus the status the stored glossary must then report.
struct GlossaryCase {
    label: &'static str,
    /// The request body, with `owningProjectIdentifier` filled in per domain.
    body: Value,
    /// The `GlossaryStatus` the glossary must carry afterwards.
    expected_status: &'static str,
}

/// The cases span what the rule generalises over: the name's size and character class,
/// a description present or absent, each member of `GlossaryStatus` plus the request
/// that names none, and `usageRestrictions` present or absent.
fn glossary_cases() -> Vec<GlossaryCase> {
    vec![
        GlossaryCase {
            label: "name only, no status",
            // `GlossaryName` is 1..256 (smithy_query shape="GlossaryName"); this is the
            // minimum length.
            body: json!({ "name": "g" }),
            // A request that names no status still gets one, because `GetGlossaryOutput`
            // reports `status` as required: the glossary just created is usable.
            expected_status: GLOSSARY_STATUS_ENABLED,
        },
        GlossaryCase {
            label: "name and description, explicitly ENABLED",
            body: json!({
                "name": "Business Terms 1",
                "description": "the shared vocabulary",
                // `GlossaryStatus` is DISABLED | ENABLED (smithy_query
                // shape="GlossaryStatus").
                "status": "ENABLED",
            }),
            expected_status: GLOSSARY_STATUS_ENABLED,
        },
        GlossaryCase {
            label: "explicitly DISABLED, with a usage restriction",
            body: json!({
                "name": "retired_terms-2",
                "status": "DISABLED",
                // `GlossaryUsageRestrictions` is a 1..1 list of
                // `GlossaryUsageRestriction`, whose only member is
                // ASSET_GOVERNED_TERMS (smithy_query shape="GlossaryUsageRestriction").
                "usageRestrictions": ["ASSET_GOVERNED_TERMS"],
            }),
            expected_status: GLOSSARY_STATUS_DISABLED,
        },
        GlossaryCase {
            label: "a name at the modelled 256-character maximum and a 4096-character description",
            body: json!({
                "name": "n".repeat(256),
                // `GlossaryDescription` is ..4096 (smithy_query
                // shape="GlossaryDescription"); this is the maximum.
                "description": "d".repeat(4096),
            }),
            expected_status: GLOSSARY_STATUS_ENABLED,
        },
    ]
}

/// Fill a case's body in with the project that owns the glossary.
fn owned_by(body: &Value, project: &str) -> Value {
    let mut body = body.clone();
    body["owningProjectIdentifier"] = json!(project);
    body
}

/// Create an available domain with one project in it, and answer both ids: every
/// glossary case needs a domain that exists and an owning project inside it.
async fn domain_with_project(dz: &Emulator, name: &str) -> (String, String) {
    let domain = dz.available_domain(name).await;
    let project = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": format!("{name} owner") })),
        )
        .await;
    assert_eq!(
        project.status, 201,
        "creating the owning project in {name:?}: expected 201, got {} {:?}",
        project.status, project.body
    );
    (domain, project.s("id"))
}

#[tokio::test]
async fn created_glossaries_are_persisted_scoped_and_reported_by_get() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "glossaries-domain").await;
    let (other_domain, other_project) = domain_with_project(&dz, "other-domain").await;
    let glossaries_uri = format!("/v2/domains/{domain}/glossaries");

    let mut created: Vec<(&'static str, String, Value, &'static str)> = Vec::new();
    for case in glossary_cases() {
        let request = owned_by(&case.body, &project);
        let answer = dz
            .call("POST", &glossaries_uri, Some(request.clone()))
            .await;
        assert_eq!(
            answer.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, answer.status, answer.body
        );
        let id = answer.s("id");
        assert!(
            is_glossary_id(&id),
            "case {:?}: expected an id matching ^[a-zA-Z0-9_-]{{1,36}}$, got {id:?}",
            case.label
        );
        assert_eq!(
            answer.s("domainId"),
            domain,
            "case {:?}: expected the glossary to report its domain, got {:?}",
            case.label,
            answer.s("domainId")
        );
        assert_eq!(
            answer.s("owningProjectId"),
            project,
            "case {:?}: expected the owning project from the request, got {:?}",
            case.label,
            answer.s("owningProjectId")
        );
        assert_eq!(
            answer.s("status"),
            case.expected_status,
            "case {:?}: expected status {:?}, got {:?}",
            case.label,
            case.expected_status,
            answer.s("status")
        );
        assert_eq!(
            answer.body.get("usageRestrictions").cloned(),
            request.get("usageRestrictions").cloned(),
            "case {:?}: expected the requested usageRestrictions, got {:?}",
            case.label,
            answer.body.get("usageRestrictions")
        );
        created.push((case.label, id, request, case.expected_status));
    }

    // Every id is distinct: two glossaries never share one.
    let mut ids: Vec<&String> = created.iter().map(|(_, id, _, _)| id).collect();
    ids.sort();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        distinct,
        "each created glossary must have its own id, got {ids:?}"
    );

    // GetGlossary reports exactly what CreateGlossary stored.
    for (label, id, request, expected_status) in &created {
        let got = dz.call("GET", &format!("{glossaries_uri}/{id}"), None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: expected 200 from GetGlossary, got {} {:?}",
            got.status, got.body
        );
        for member in ["name", "description"] {
            assert_eq!(
                got.body.get(member).and_then(Value::as_str),
                request.get(member).and_then(Value::as_str),
                "case {label:?}: GetGlossary must report the persisted {member}, got {:?}",
                got.body.get(member)
            );
        }
        assert_eq!(
            got.s("id"),
            *id,
            "case {label:?}: GetGlossary must report the glossary that was read"
        );
        assert_eq!(
            got.s("domainId"),
            domain,
            "case {label:?}: GetGlossary must report the stored domain"
        );
        assert_eq!(
            got.s("owningProjectId"),
            project,
            "case {label:?}: GetGlossary must report the stored owning project"
        );
        assert_eq!(
            got.s("status"),
            *expected_status,
            "case {label:?}: expected status {expected_status:?} from GetGlossary, got {:?}",
            got.s("status")
        );
        assert_eq!(
            got.body.get("usageRestrictions").cloned(),
            request.get("usageRestrictions").cloned(),
            "case {label:?}: GetGlossary must report the stored usageRestrictions, got {:?}",
            got.body.get("usageRestrictions")
        );
        // `createdBy` is the CALLER's principal, derived from the request's SigV4
        // credential scope — never an echo of the request body.
        assert_eq!(
            got.s("createdBy"),
            "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY",
            "case {label:?}: expected the signing principal as createdBy, got {:?}",
            got.s("createdBy")
        );
        assert!(
            got.body.get("createdAt").is_some(),
            "case {label:?}: expected createdAt, got {:?}",
            got.body
        );
        // Nothing has updated the glossary, so it reports no update.
        assert!(
            got.body.get("updatedAt").is_none() && got.body.get("updatedBy").is_none(),
            "case {label:?}: a glossary nothing updated must report no updatedAt/updatedBy, got {:?}",
            got.body
        );
    }

    // A glossary is scoped to its domain: the SAME id read through another domain is as
    // absent as one that was never created, and the other domain's glossary is its own.
    let elsewhere = dz
        .call(
            "POST",
            &format!("/v2/domains/{other_domain}/glossaries"),
            Some(json!({ "name": "g", "owningProjectIdentifier": other_project })),
        )
        .await;
    assert_eq!(
        elsewhere.status, 201,
        "a name already used in another domain must still be accepted here, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    for (label, id, _, _) in &created {
        let cross = dz
            .call(
                "GET",
                &format!("/v2/domains/{other_domain}/glossaries/{id}"),
                None,
            )
            .await;
        assert_eq!(
            cross.status, 404,
            "case {label:?}: a glossary of another domain must not be readable here, got {} {:?}",
            cross.status, cross.body
        );
        assert_eq!(
            cross.error_code, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            cross.error_code
        );
    }

    // Deleting the domain takes its glossaries with it.
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    for (label, id, _, _) in &created {
        let gone = dz.call("GET", &format!("{glossaries_uri}/{id}"), None).await;
        assert_eq!(
            gone.status, 404,
            "case {label:?}: a glossary must go with its domain, got {} {:?}",
            gone.status,
            gone.body
        );
    }
}

#[tokio::test]
async fn a_glossary_name_is_unique_within_its_domain() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "unique-names").await;
    let (other_domain, other_project) = domain_with_project(&dz, "unique-names-too").await;

    for case in glossary_cases() {
        let first = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/glossaries"),
                Some(owned_by(&case.body, &project)),
            )
            .await;
        assert_eq!(
            first.status, 201,
            "case {:?}: the first glossary of this name must be created, got {} {:?}",
            case.label, first.status, first.body
        );
        let repeat = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/glossaries"),
                Some(owned_by(&case.body, &project)),
            )
            .await;
        assert_eq!(
            repeat.status, 409,
            "case {:?}: a repeated name in the same domain must conflict, got {} {:?}",
            case.label, repeat.status, repeat.body
        );
        assert_eq!(
            repeat.error_code, "ConflictException",
            "case {:?}: expected ConflictException, got {:?}",
            case.label, repeat.error_code
        );
        // The same name in ANOTHER domain is a different glossary, not a conflict.
        let elsewhere = dz
            .call(
                "POST",
                &format!("/v2/domains/{other_domain}/glossaries"),
                Some(owned_by(&case.body, &other_project)),
            )
            .await;
        assert_eq!(
            elsewhere.status, 201,
            "case {:?}: the same name in another domain must be accepted, got {} {:?}",
            case.label, elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.s("id"),
            first.s("id"),
            "case {:?}: two domains' glossaries of one name must be separate resources",
            case.label
        );
    }
}

#[tokio::test]
async fn a_glossary_needs_a_domain_that_is_available_and_a_project_in_it() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "prerequisites").await;
    let (_other_domain, other_project) = domain_with_project(&dz, "prerequisites-other").await;

    // An unknown domain, and an owning project that is not one of this domain's
    // projects (including one that belongs to a DIFFERENT domain): the operation models
    // no ResourceNotFoundException (smithy_query operation="CreateGlossary"), so each is
    // refused with the access error it does model.
    let denied: Vec<(&str, String, Value)> = vec![
        (
            "a well-formed domain id nothing created",
            "/v2/domains/dzd_nosuchdomain000/glossaries".to_string(),
            json!({ "name": "g", "owningProjectIdentifier": project.clone() }),
        ),
        (
            "an owning project nothing created",
            format!("/v2/domains/{domain}/glossaries"),
            json!({ "name": "g", "owningProjectIdentifier": "nosuchproject" }),
        ),
        (
            "an owning project that belongs to another domain",
            format!("/v2/domains/{domain}/glossaries"),
            json!({ "name": "g", "owningProjectIdentifier": other_project }),
        ),
    ];
    for (label, uri, body) in denied {
        let answer = dz.call("POST", &uri, Some(body)).await;
        assert_eq!(
            answer.status, 403,
            "case {label:?}: expected 403, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "AccessDeniedException",
            "case {label:?}: expected AccessDeniedException, got {:?}",
            answer.error_code
        );
    }

    // A domain that is still being created cannot take a glossary yet: it is reported
    // in a non-terminal status, so the request conflicts with that status.
    let creating = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "still-creating" })))
        .await;
    assert_eq!(
        creating.status, 201,
        "creating the domain: expected 201, got {} {:?}",
        creating.status, creating.body
    );
    let too_early = dz
        .call(
            "POST",
            &format!("/v2/domains/{}/glossaries", creating.s("id")),
            Some(json!({ "name": "g", "owningProjectIdentifier": project })),
        )
        .await;
    assert_eq!(
        too_early.status, 409,
        "a domain that has not settled yet must refuse a glossary, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code, "ConflictException",
        "expected ConflictException, got {:?}",
        too_early.error_code
    );
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_glossary() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "idempotent-glossaries").await;
    let glossaries_uri = format!("/v2/domains/{domain}/glossaries");

    for (index, case) in glossary_cases().into_iter().enumerate() {
        let mut body = owned_by(&case.body, &project);
        // `ClientToken` is 1..128 of `^[\x21-\x7E]+$` (smithy_query shape="ClientToken").
        body["clientToken"] = json!(format!("token-{index}"));
        let first = dz.call("POST", &glossaries_uri, Some(body.clone())).await;
        assert_eq!(
            first.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, first.status, first.body
        );
        let repeat = dz.call("POST", &glossaries_uri, Some(body)).await;
        assert_eq!(
            repeat.status, 201,
            "case {:?}: a repeat carrying the same clientToken must succeed, got {} {:?}",
            case.label, repeat.status, repeat.body
        );
        assert_eq!(
            repeat.s("id"),
            first.s("id"),
            "case {:?}: the repeat must return the first glossary, got {:?}",
            case.label,
            repeat.s("id")
        );
        // And the retry left nothing extra behind: the one glossary still reads back.
        let got = dz
            .call("GET", &format!("{glossaries_uri}/{}", first.s("id")), None)
            .await;
        assert_eq!(
            got.status, 200,
            "case {:?}: the glossary the token created must still be readable, got {} {:?}",
            case.label, got.status, got.body
        );
        assert_eq!(
            got.s("name"),
            first.s("name"),
            "case {:?}: expected the stored name, got {:?}",
            case.label,
            got.s("name")
        );
    }
}

#[tokio::test]
async fn one_token_string_is_remembered_per_operation_not_per_service() {
    let dz = Emulator::new();
    // `clientToken` is CALLER-chosen, so the same string can reach two different
    // creates. Each operation's record must stand on its own: a glossary create must
    // not consume, or overwrite, the record a domain create left under that token.
    let shared = "shared-client-token";
    let domain = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "token-sharer", "clientToken": shared })),
        )
        .await;
    assert_eq!(
        domain.status, 201,
        "creating the domain: expected 201, got {} {:?}",
        domain.status, domain.body
    );
    let domain_id = domain.s("id");
    // Wait out the chosen `LIFECYCLE_SETTLE_MS` span, plus slack, so the domain has
    // settled to AVAILABLE and can take a project and a glossary.
    tokio::time::sleep(SETTLED).await;
    let project = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain_id}/projects"),
            Some(json!({ "name": "token sharer owner" })),
        )
        .await;
    assert_eq!(
        project.status, 201,
        "creating the owning project: expected 201, got {} {:?}",
        project.status, project.body
    );

    let glossaries_uri = format!("/v2/domains/{domain_id}/glossaries");
    let body = json!({
        "name": "shared token glossary",
        "owningProjectIdentifier": project.s("id"),
        "clientToken": shared,
    });
    let glossary = dz.call("POST", &glossaries_uri, Some(body.clone())).await;
    assert_eq!(
        glossary.status, 201,
        "a token another operation used must not block this create, got {} {:?}",
        glossary.status, glossary.body
    );
    // The domain's own record survived: a repeat of the domain create with that token
    // still returns the FIRST domain rather than creating a second one.
    let domain_repeat = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "token-sharer", "clientToken": shared })),
        )
        .await;
    assert_eq!(
        domain_repeat.s("id"),
        domain_id,
        "the domain token must still resolve to the first domain, got {:?}",
        domain_repeat.s("id")
    );
    // And the glossary's record is its own.
    let glossary_repeat = dz.call("POST", &glossaries_uri, Some(body.clone())).await;
    assert_eq!(
        glossary_repeat.s("id"),
        glossary.s("id"),
        "the glossary token must resolve to the first glossary, got {:?}",
        glossary_repeat.s("id")
    );

    // The same token used for a glossary in ANOTHER domain is another record: each
    // domain keeps its own, so neither request loses its replay.
    let (second_domain, second_project) = domain_with_project(&dz, "token-sharer-two").await;
    let elsewhere = dz
        .call(
            "POST",
            &format!("/v2/domains/{second_domain}/glossaries"),
            Some(json!({
                "name": "shared token glossary",
                "owningProjectIdentifier": second_project,
                "clientToken": shared,
            })),
        )
        .await;
    assert_eq!(
        elsewhere.status, 201,
        "the same token in another domain must create that domain's glossary, got {} {:?}",
        elsewhere.status, elsewhere.body
    );
    assert_ne!(
        elsewhere.s("id"),
        glossary.s("id"),
        "the second domain's glossary must be its own resource"
    );
    // The first domain's record is untouched: its replay still answers the first
    // glossary rather than attempting a fresh create.
    let first_again = dz.call("POST", &glossaries_uri, Some(body)).await;
    assert_eq!(
        (first_again.status, first_again.s("id")),
        (201, glossary.s("id")),
        "the first domain's token must still resolve to its own glossary, got {} {:?}",
        first_again.status,
        first_again.body
    );
}
