// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateGlossaryTerm`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — and reuses the
//! `Emulator` harness from [`crate::harness`], so nothing here reaches into
//! `crate::state` and a handler answering from a hardcoded value would fail.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `UpdateGlossaryTerm` is a PATCH of the stored term: a member the request carries
//!    replaces the stored value, a member it omits leaves the stored value untouched,
//!    and the answer is what the next `GetGlossaryTerm` reports — so a status toggled to
//!    `DISABLED` and back is observable both times.
//! 2. The update is what sets `updatedBy`/`updatedAt`; `createdBy`/`createdAt` belong to
//!    the create and survive it.
//! 3. A patched name stays unique WITHIN the glossary the term then belongs to: another
//!    term of that glossary already carrying the name is a conflict, the term
//!    re-asserting its own name is not, and the same name under another glossary is a
//!    different term.
//! 4. The term must exist in the named domain, a `glossaryIdentifier` must name a
//!    glossary of that domain, and every term a `termRelations` names must be a term of
//!    the same domain.
//! 5. A patch outside the modelled constraints is refused, and a refused patch leaves
//!    the stored term exactly as it was.

use crate::harness::Emulator;
use serde_json::{json, Value};
// The `GlossaryTermStatus` members as the service stores them, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    GLOSSARY_TERM_STATUS_DISABLED, GLOSSARY_TERM_STATUS_ENABLED,
};

/// The principal the harness signs with: `createdBy`/`updatedBy` come from the caller's
/// SigV4 credential scope, never from the request body.
const CALLER: &str = "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY";

/// The only member of `GlossaryUsageRestriction` (smithy_query
/// shape="GlossaryUsageRestriction": ASSET_GOVERNED_TERMS).
const ASSET_GOVERNED_TERMS: &str = "ASSET_GOVERNED_TERMS";

/// One `UpdateGlossaryTerm` case: the term the patch is applied to, the patch, and the
/// record the term must carry afterwards.
struct PatchCase {
    label: String,
    /// The `CreateGlossaryTerm` body, less `glossaryIdentifier`.
    create: Value,
    /// The `UpdateGlossaryTerm` body.
    patch: Value,
    expect_name: String,
    expect_short_description: Option<String>,
    expect_long_description: Option<String>,
    expect_status: &'static str,
    /// The `termRelations` the term must report, absent when it has no relation.
    expect_term_relations: Option<Value>,
    /// The glossary the term must be an entry of once the patch has been applied.
    expect_glossary: String,
    /// `usageRestrictions` is the OWNING GLOSSARY's, so it follows `expect_glossary`.
    expect_usage_restrictions: Option<Value>,
}

/// The cases span what the rule generalises over: which patchable members the request
/// carries (none, one, all of them), each `GlossaryTermStatus` member in both
/// directions, the published bounds of the string members, `termRelations` one-sided and
/// two-sided over a varying number of related terms, and a `glossaryIdentifier` that
/// re-points the term at another glossary of the domain.
///
/// `glossary` is the glossary every case creates its term under, `restricted` a second
/// glossary of the same domain carrying a usage restriction, and `related` the ids of
/// terms already stored in that domain, which is what a relation has to name.
fn patch_cases(glossary: &str, restricted: &str, related: &[String]) -> Vec<PatchCase> {
    let unrestricted = |case: PatchCase| PatchCase {
        expect_glossary: glossary.to_string(),
        expect_usage_restrictions: None,
        ..case
    };
    let moved = |case: PatchCase| PatchCase {
        expect_glossary: restricted.to_string(),
        expect_usage_restrictions: Some(json!([ASSET_GOVERNED_TERMS])),
        ..case
    };
    // The members a row leaves unsaid; `unrestricted`/`moved`, which every row below
    // goes through, fill the glossary in.
    let base = || PatchCase {
        label: String::new(),
        create: json!({}),
        patch: json!({}),
        expect_name: String::new(),
        expect_short_description: None,
        expect_long_description: None,
        expect_status: GLOSSARY_TERM_STATUS_ENABLED,
        expect_term_relations: None,
        expect_glossary: String::new(),
        expect_usage_restrictions: None,
    };
    vec![
        unrestricted(PatchCase {
            label: "status only: ENABLED toggled to DISABLED".to_string(),
            create: json!({
                "name": "toggle one",
                "status": "ENABLED",
                "shortDescription": "money taken in",
            }),
            // `GlossaryTermStatus` is DISABLED | ENABLED (smithy_query
            // shape="GlossaryTermStatus").
            patch: json!({ "status": "DISABLED" }),
            expect_name: "toggle one".to_string(),
            expect_short_description: Some("money taken in".to_string()),
            expect_status: GLOSSARY_TERM_STATUS_DISABLED,
            ..base()
        }),
        unrestricted(PatchCase {
            label: "status only: DISABLED toggled back to ENABLED".to_string(),
            create: json!({ "name": "toggle two", "status": "DISABLED" }),
            patch: json!({ "status": "ENABLED" }),
            expect_name: "toggle two".to_string(),
            expect_status: GLOSSARY_TERM_STATUS_ENABLED,
            ..base()
        }),
        unrestricted(PatchCase {
            label: "name only, at `GlossaryTermName`'s 1-character minimum".to_string(),
            create: json!({ "name": "rename three", "longDescription": "kept" }),
            // `GlossaryTermName` is 1..256 (smithy_query shape="GlossaryTermName").
            patch: json!({ "name": "t" }),
            expect_name: "t".to_string(),
            expect_long_description: Some("kept".to_string()),
            // The create named no status, so the term carries the usable member and a
            // patch that names none leaves it there.
            expect_status: GLOSSARY_TERM_STATUS_ENABLED,
            ..base()
        }),
        unrestricted(PatchCase {
            label: "name only, at `GlossaryTermName`'s 256-character maximum".to_string(),
            create: json!({ "name": "rename four", "status": "DISABLED" }),
            patch: json!({ "name": "n".repeat(256) }),
            expect_name: "n".repeat(256),
            // A patch that names no status must not resurrect the default.
            expect_status: GLOSSARY_TERM_STATUS_DISABLED,
            ..base()
        }),
        unrestricted(PatchCase {
            label: "both descriptions, at their modelled maxima".to_string(),
            create: json!({
                "name": "describe five",
                "shortDescription": "replaced",
                "longDescription": "replaced",
            }),
            // `ShortDescription` is ..1024 and `LongDescription` ..4096 (smithy_query
            // shape="ShortDescription,LongDescription"); these are the maxima.
            patch: json!({
                "shortDescription": "s".repeat(1024),
                "longDescription": "l".repeat(4096),
            }),
            expect_name: "describe five".to_string(),
            expect_short_description: Some("s".repeat(1024)),
            expect_long_description: Some("l".repeat(4096)),
            ..base()
        }),
        unrestricted(PatchCase {
            label: "both descriptions, emptied to their 0-character minimum".to_string(),
            create: json!({
                "name": "describe six",
                "shortDescription": "replaced",
                "longDescription": "replaced",
            }),
            patch: json!({ "shortDescription": "", "longDescription": "" }),
            expect_name: "describe six".to_string(),
            expect_short_description: Some(String::new()),
            expect_long_description: Some(String::new()),
            ..base()
        }),
        unrestricted(PatchCase {
            label: "termRelations replaced by a one-sided isA naming one stored term"
                .to_string(),
            create: json!({
                "name": "relate seven",
                // `TermRelations` carries `isA` and `classifies`, each a list of term
                // ids targeting `GlossaryTerms` (smithy_query shape="TermRelations").
                "termRelations": { "classifies": [related[0]] },
            }),
            patch: json!({ "termRelations": { "isA": [related[0]] } }),
            expect_name: "relate seven".to_string(),
            expect_term_relations: Some(json!({ "isA": [related[0]] })),
            ..base()
        }),
        unrestricted(PatchCase {
            label: "termRelations added two-sided over two stored terms".to_string(),
            create: json!({ "name": "relate eight" }),
            patch: json!({
                "termRelations": {
                    "isA": [related[0]],
                    "classifies": [related[0], related[1]],
                },
            }),
            expect_name: "relate eight".to_string(),
            expect_term_relations: Some(json!({
                "isA": [related[0]],
                "classifies": [related[0], related[1]],
            })),
            ..base()
        }),
        moved(PatchCase {
            label: "glossaryIdentifier only: the term becomes an entry of another glossary"
                .to_string(),
            create: json!({ "name": "moved nine", "shortDescription": "kept" }),
            patch: json!({ "glossaryIdentifier": restricted }),
            expect_name: "moved nine".to_string(),
            expect_short_description: Some("kept".to_string()),
            ..base()
        }),
        moved(PatchCase {
            label: "every patchable member at once".to_string(),
            create: json!({
                "name": "all ten",
                "shortDescription": "the old short text",
                "longDescription": "the old long text",
                "status": "ENABLED",
                "termRelations": { "isA": [related[1]] },
            }),
            patch: json!({
                "glossaryIdentifier": restricted,
                "name": "all ten renamed",
                "shortDescription": "the new short text",
                "longDescription": "the new long text",
                "status": "DISABLED",
                "termRelations": { "classifies": [related[0]] },
            }),
            expect_name: "all ten renamed".to_string(),
            expect_short_description: Some("the new short text".to_string()),
            expect_long_description: Some("the new long text".to_string()),
            expect_status: GLOSSARY_TERM_STATUS_DISABLED,
            expect_term_relations: Some(json!({ "classifies": [related[0]] })),
            ..base()
        }),
        unrestricted(PatchCase {
            label: "a patch carrying no patchable member at all".to_string(),
            create: json!({
                "name": "untouched eleven",
                "shortDescription": "the only short text",
                "longDescription": "the only long text",
                "status": "DISABLED",
                "termRelations": { "isA": [related[0], related[1]] },
            }),
            patch: json!({}),
            expect_name: "untouched eleven".to_string(),
            expect_short_description: Some("the only short text".to_string()),
            expect_long_description: Some("the only long text".to_string()),
            expect_status: GLOSSARY_TERM_STATUS_DISABLED,
            expect_term_relations: Some(json!({ "isA": [related[0], related[1]] })),
            ..base()
        }),
    ]
}

/// Create one glossary in an existing domain and answer its id.
async fn glossary_in(
    dz: &Emulator,
    domain: &str,
    project: &str,
    name: &str,
    usage_restrictions: Option<&str>,
) -> String {
    let mut body = json!({ "name": name, "owningProjectIdentifier": project });
    if let Some(restriction) = usage_restrictions {
        body["usageRestrictions"] = json!([restriction]);
    }
    let created = dz
        .call("POST", &format!("/v2/domains/{domain}/glossaries"), Some(body))
        .await;
    assert_eq!(
        created.status, 201,
        "creating glossary {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// Create one term from `body` under `glossary` and answer its id.
async fn term_in(dz: &Emulator, domain: &str, glossary: &str, body: &Value) -> String {
    let mut body = body.clone();
    body["glossaryIdentifier"] = json!(glossary);
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/glossary-terms"),
            Some(body.clone()),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating the term to patch {body:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// Create one named term under `glossary` and answer its id: the fixture a relation or a
/// name-clash case needs.
async fn named_term(dz: &Emulator, domain: &str, glossary: &str, name: &str) -> String {
    term_in(dz, domain, glossary, &json!({ "name": name })).await
}

/// An available domain with a project, an unrestricted glossary, a second glossary
/// carrying a usage restriction, and two terms a relation can name.
async fn fixture(dz: &Emulator, name: &str) -> (String, String, String, Vec<String>) {
    let domain = dz.available_domain(name).await;
    let project = dz.project(&domain, &format!("{name} owner")).await;
    let glossary = glossary_in(dz, &domain, &project, &format!("{name} glossary"), None).await;
    let restricted = glossary_in(
        dz,
        &domain,
        &project,
        &format!("{name} restricted glossary"),
        Some(ASSET_GOVERNED_TERMS),
    )
    .await;
    let related = vec![
        named_term(dz, &domain, &glossary, "base concept a").await,
        named_term(dz, &domain, &glossary, "base concept b").await,
    ];
    (domain, glossary, restricted, related)
}

/// Assert that `body` reports the record `case` expects, whichever operation answered it.
fn assert_record(case: &PatchCase, who: &str, body: &Value, domain: &str, id: &str) {
    assert_eq!(
        body.get("id").and_then(Value::as_str),
        Some(id),
        "case {:?}: {who} must report the term that was patched, got {:?}",
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
        body.get("glossaryId").and_then(Value::as_str),
        Some(case.expect_glossary.as_str()),
        "case {:?}: {who} expected glossaryId {:?}, got {:?}",
        case.label,
        case.expect_glossary,
        body.get("glossaryId")
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
        body.get("shortDescription").and_then(Value::as_str),
        case.expect_short_description.as_deref(),
        "case {:?}: {who} expected shortDescription {:?}, got {:?}",
        case.label,
        case.expect_short_description,
        body.get("shortDescription")
    );
    assert_eq!(
        body.get("longDescription").and_then(Value::as_str),
        case.expect_long_description.as_deref(),
        "case {:?}: {who} expected longDescription {:?}, got {:?}",
        case.label,
        case.expect_long_description,
        body.get("longDescription")
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
        body.get("termRelations").cloned(),
        case.expect_term_relations.clone(),
        "case {:?}: {who} expected termRelations {:?}, got {:?}",
        case.label,
        case.expect_term_relations,
        body.get("termRelations")
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
    let (domain, glossary, restricted, related) = fixture(&dz, "patched-terms").await;

    for case in patch_cases(&glossary, &restricted, &related) {
        let id = term_in(&dz, &domain, &glossary, &case.create).await;
        let uri = format!("/v2/domains/{domain}/glossary-terms/{id}");

        // What the create left behind, so the patch can be judged against it.
        let before = dz.call("GET", &uri, None).await;
        assert_eq!(
            before.status, 200,
            "case {:?}: reading the term before the patch: expected 200, got {} {:?}",
            case.label, before.status, before.body
        );
        assert!(
            before.body.get("updatedAt").is_none() && before.body.get("updatedBy").is_none(),
            "case {:?}: a term nothing has updated must report no updatedAt/updatedBy, got {:?}",
            case.label,
            before.body
        );

        let patched = dz.call("PATCH", &uri, Some(case.patch.clone())).await;
        assert_eq!(
            patched.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.label, patched.status, patched.body
        );
        assert_record(&case, "UpdateGlossaryTerm", &patched.body, &domain, &id);

        // The patch is persisted: the next read reports the same record, which is what
        // tells a stored update apart from one computed only for this answer.
        let after = dz.call("GET", &uri, None).await;
        assert_eq!(
            after.status, 200,
            "case {:?}: reading the term back: expected 200, got {} {:?}",
            case.label, after.status, after.body
        );
        assert_record(&case, "GetGlossaryTerm", &after.body, &domain, &id);

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
async fn a_patched_name_stays_unique_within_its_glossary() {
    let dz = Emulator::new();
    let (domain, glossary, restricted, related) = fixture(&dz, "renamed-terms").await;

    for (index, case) in patch_cases(&glossary, &restricted, &related)
        .into_iter()
        .enumerate()
    {
        // Another term of the SAME glossary holds the name the patch will ask for, and a
        // term of a SIBLING glossary holds a second name, which the patch may take.
        let taken = format!("taken name {index}");
        let free_elsewhere = format!("sibling name {index}");
        named_term(&dz, &domain, &glossary, &taken).await;
        named_term(&dz, &domain, &restricted, &free_elsewhere).await;
        let id = term_in(&dz, &domain, &glossary, &case.create).await;
        let uri = format!("/v2/domains/{domain}/glossary-terms/{id}");
        let created_name = case
            .create
            .get("name")
            .and_then(Value::as_str)
            .expect("every case creates a named term")
            .to_string();

        let clash = dz.call("PATCH", &uri, Some(json!({ "name": taken }))).await;
        assert_eq!(
            (clash.status, clash.error_code.as_str()),
            (409, "ConflictException"),
            "case {:?}: a name another term of this glossary holds must conflict, got {} {:?}",
            case.label,
            clash.status,
            clash.body
        );
        // The refused patch wrote nothing: the term still carries its own name and still
        // reports no update.
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

        // A term re-asserting its OWN name is not a clash with itself.
        let same = dz
            .call("PATCH", &uri, Some(json!({ "name": created_name.clone() })))
            .await;
        assert_eq!(
            same.status, 200,
            "case {:?}: re-asserting the term's own name must be accepted, got {} {:?}",
            case.label, same.status, same.body
        );
        assert_eq!(
            same.s("name"),
            created_name,
            "case {:?}: expected the name to stand, got {:?}",
            case.label,
            same.s("name")
        );

        // A name only ANOTHER glossary's term holds is free here: the scope is the
        // glossary, not the domain.
        let accepted = dz
            .call(
                "PATCH",
                &uri,
                Some(json!({ "name": free_elsewhere.clone() })),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: a name used in another glossary must be accepted here, got {} {:?}",
            case.label, accepted.status, accepted.body
        );

        // Re-pointing the term at THAT glossary is then refused, because the name it
        // carries is already taken where it would land.
        let into_clash = dz
            .call(
                "PATCH",
                &uri,
                Some(json!({ "glossaryIdentifier": restricted.clone() })),
            )
            .await;
        assert_eq!(
            (into_clash.status, into_clash.error_code.as_str()),
            (409, "ConflictException"),
            "case {:?}: a move into a glossary that already holds this name must conflict, got {} {:?}",
            case.label,
            into_clash.status,
            into_clash.body
        );
        let still_here = dz.call("GET", &uri, None).await;
        assert_eq!(
            still_here.s("glossaryId"),
            glossary,
            "case {:?}: a refused move must leave the stored glossary alone, got {:?}",
            case.label,
            still_here.s("glossaryId")
        );

        // The same move carrying a name that IS free there is accepted, and the name the
        // term leaves behind in its old glossary is no obstacle to it.
        let moved_name = format!("moved name {index}");
        named_term(&dz, &domain, &glossary, &moved_name).await;
        let into_free = dz
            .call(
                "PATCH",
                &uri,
                Some(json!({ "glossaryIdentifier": restricted.clone(), "name": moved_name })),
            )
            .await;
        assert_eq!(
            into_free.status, 200,
            "case {:?}: a move carrying a name free in the target glossary must be accepted, got {} {:?}",
            case.label, into_free.status, into_free.body
        );
        assert_eq!(
            (into_free.s("glossaryId"), into_free.s("name")),
            (restricted.clone(), moved_name.clone()),
            "case {:?}: expected the term to report its new glossary and name, got {:?}",
            case.label,
            into_free.body
        );
    }
}

#[tokio::test]
async fn an_update_needs_the_term_the_glossary_and_the_relations_to_be_stored() {
    let dz = Emulator::new();
    let (domain, glossary, _restricted, _related) = fixture(&dz, "missing-terms").await;
    let (other_domain, other_glossary, _, _) = fixture(&dz, "missing-terms-other").await;
    let id = named_term(&dz, &domain, &glossary, "the only one").await;
    let foreign_term = named_term(&dz, &other_domain, &other_glossary, "a foreign term").await;

    // (label, domain, term id, patch, expected status, expected wire error code)
    let cases: Vec<(&str, String, String, Value, u16, &str)> = vec![
        (
            "a term id nothing created",
            domain.clone(),
            "nosuchterm".to_string(),
            json!({ "status": "DISABLED" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the right term id under the wrong domain",
            other_domain.clone(),
            id.clone(),
            json!({ "status": "DISABLED" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed domain id nothing created",
            "dzd_nosuchdomain000".to_string(),
            id.clone(),
            json!({ "status": "DISABLED" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a glossaryIdentifier nothing created",
            domain.clone(),
            id.clone(),
            json!({ "glossaryIdentifier": "nosuchglossary" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a glossaryIdentifier that belongs to another domain",
            domain.clone(),
            id.clone(),
            json!({ "glossaryIdentifier": other_glossary.clone() }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an isA relation naming a term nothing created",
            domain.clone(),
            id.clone(),
            json!({ "termRelations": { "isA": ["nosuchterm"] } }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a classifies relation naming a term of another domain",
            domain.clone(),
            id.clone(),
            json!({ "termRelations": { "classifies": [foreign_term.clone()] } }),
            404,
            "ResourceNotFoundException",
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
            // `GlossaryTermId` is `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
            // shape="GlossaryTermId").
            "a term identifier one character past the modelled 36 maximum",
            domain.clone(),
            "i".repeat(37),
            json!({ "status": "DISABLED" }),
            400,
            "ValidationException",
        ),
    ];
    for (label, domain_id, term_id, patch, status, code) in cases {
        let answer = dz
            .call(
                "PATCH",
                &format!("/v2/domains/{domain_id}/glossary-terms/{term_id}"),
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

    // Every refusal above left the stored term exactly as the create did.
    let stored = dz
        .call("GET", &format!("/v2/domains/{domain}/glossary-terms/{id}"), None)
        .await;
    assert_eq!(
        (stored.s("name"), stored.s("glossaryId")),
        ("the only one".to_string(), glossary.clone()),
        "a refused patch must leave the stored term alone, got {:?}",
        stored.body
    );
    assert!(
        stored.body.get("updatedAt").is_none(),
        "a refused patch must not set updatedAt, got {:?}",
        stored.body
    );

    // A term goes with its domain: once the domain is on its way out it holds no term to
    // patch.
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    let gone = dz
        .call(
            "PATCH",
            &format!("/v2/domains/{domain}/glossary-terms/{id}"),
            Some(json!({ "status": "DISABLED" })),
        )
        .await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a term whose domain is gone must not be patchable, got {} {:?}",
        gone.status,
        gone.body
    );
}

#[tokio::test]
async fn a_patch_outside_the_modelled_constraints_is_refused() {
    let dz = Emulator::new();
    let (domain, glossary, _restricted, related) = fixture(&dz, "refused-term-patches").await;
    let id = term_in(
        &dz,
        &domain,
        &glossary,
        &json!({
            "name": "the stored one",
            "shortDescription": "the stored short text",
            "longDescription": "the stored long text",
            "status": "ENABLED",
        }),
    )
    .await;
    let uri = format!("/v2/domains/{domain}/glossary-terms/{id}");

    let cases: Vec<(&str, Value)> = vec![
        (
            "a name one character past `GlossaryTermName`'s 256 maximum",
            json!({ "name": "n".repeat(257) }),
        ),
        (
            "a name below `GlossaryTermName`'s 1-character minimum",
            json!({ "name": "" }),
        ),
        (
            "a shortDescription one character past `ShortDescription`'s 1024 maximum",
            json!({ "shortDescription": "s".repeat(1025) }),
        ),
        (
            "a longDescription one character past `LongDescription`'s 4096 maximum",
            json!({ "longDescription": "l".repeat(4097) }),
        ),
        (
            // `GlossaryTermStatus` is DISABLED | ENABLED (smithy_query
            // shape="GlossaryTermStatus"), so neither a status of another resource's
            // enum nor the right member in the wrong case is one of its values.
            "a status that is not a GlossaryTermStatus member",
            json!({ "status": "ARCHIVED" }),
        ),
        (
            "a GlossaryTermStatus member in the wrong case",
            json!({ "status": "enabled" }),
        ),
        (
            // `GlossaryTerms`, the list each `TermRelations` member targets, is 1..20
            // (smithy_query shape="GlossaryTerms"), so an empty one is below its
            // minimum. The members themselves carry a tighter 1..10 (smithy_query
            // shape="TermRelations"); the bound asserted here is the list shape's own,
            // which is the one the request boundary enforces.
            "an isA relation below the modelled 1-entry minimum",
            json!({ "termRelations": { "isA": [] } }),
        ),
        (
            // `GlossaryTerms` is a list of 1..20 `GlossaryTermId` (smithy_query
            // shape="GlossaryTerms"), so this is one entry past that maximum.
            "a classifies relation one entry past the modelled 20 maximum",
            json!({ "termRelations": { "classifies": vec![related[0].clone(); 21] } }),
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
        // Nothing was written: the stored term is still what the create left.
        let stored = dz.call("GET", &uri, None).await;
        assert_eq!(
            (
                stored.s("name"),
                stored.s("shortDescription"),
                stored.s("longDescription"),
                stored.s("status"),
            ),
            (
                "the stored one".to_string(),
                "the stored short text".to_string(),
                "the stored long text".to_string(),
                GLOSSARY_TERM_STATUS_ENABLED.to_string(),
            ),
            "case {label:?}: a refused patch must leave the stored term alone, got {:?}",
            stored.body
        );
        assert!(
            stored.body.get("updatedAt").is_none(),
            "case {label:?}: a refused patch must not set updatedAt, got {:?}",
            stored.body
        );
    }
}
