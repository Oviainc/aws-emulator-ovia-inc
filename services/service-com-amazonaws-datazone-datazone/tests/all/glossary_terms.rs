// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone business glossary TERMS.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router, with only the response body as evidence — and reuses the
//! `Emulator` harness from [`crate::harness`], so nothing here reaches into
//! `crate::state` and a handler answering from a hardcoded value would fail.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateGlossaryTerm` mints a term under a persisted glossary of a persisted,
//!    available domain and stores the WHOLE record, so `GetGlossaryTerm` reports exactly
//!    what was stored (and reports no update until something updates the term).
//! 2. A term name is unique WITHIN ITS GLOSSARY: the same name under another glossary —
//!    of the same domain or another — is accepted, a repeat under the same glossary is
//!    refused with the modelled conflict.
//! 3. The documented prerequisites are enforced against the store: the domain must exist
//!    and be available, the glossary must be one of that domain's, and every term a
//!    `termRelations` names must be a term of the same domain.
//! 4. A term is scoped to its domain and disappears with it.
//! 5. A repeated `clientToken` returns the first term instead of a second one.
//! 6. `usageRestrictions` is the OWNING GLOSSARY's restriction, not anything the term
//!    carries of its own.

use crate::harness::Emulator;
use serde_json::{json, Value};
// The `GlossaryTermStatus` members as the service stores them, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    GLOSSARY_TERM_STATUS_DISABLED, GLOSSARY_TERM_STATUS_ENABLED,
};

/// `GlossaryTermId`'s modelled pattern, `^[a-zA-Z0-9_-]{1,36}$` (smithy_query
/// shape="GlossaryTermId").
fn is_glossary_term_id(id: &str) -> bool {
    (1..=36).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The only member of `GlossaryUsageRestriction` (smithy_query
/// shape="GlossaryUsageRestriction": ASSET_GOVERNED_TERMS).
const ASSET_GOVERNED_TERMS: &str = "ASSET_GOVERNED_TERMS";

/// One `CreateGlossaryTerm` request, plus the status the stored term must then report.
struct TermCase {
    label: &'static str,
    /// The request body, with `glossaryIdentifier` filled in per glossary.
    body: Value,
    /// The `GlossaryTermStatus` the term must carry afterwards.
    expected_status: &'static str,
}

/// The cases span what the rule generalises over: the name's size and character class,
/// each description present or absent and at its modelled maximum, each member of
/// `GlossaryTermStatus` plus the request that names none, and `termRelations` absent,
/// one-sided and two-sided over a varying number of related terms.
///
/// `related` are ids of terms already stored in the same domain, which is what a
/// relation has to name.
fn term_cases(related: &[String]) -> Vec<TermCase> {
    vec![
        TermCase {
            label: "name only, no status",
            // `GlossaryTermName` is 1..256 (smithy_query shape="GlossaryTermName"); this
            // is the minimum length.
            body: json!({ "name": "t" }),
            // A request that names no status still gets one, because both
            // `CreateGlossaryTermOutput` and `GetGlossaryTermOutput` report `status` as
            // required: the term just created is usable.
            expected_status: GLOSSARY_TERM_STATUS_ENABLED,
        },
        TermCase {
            label: "both descriptions, explicitly ENABLED",
            body: json!({
                "name": "Revenue 1",
                "shortDescription": "money taken in",
                "longDescription": "the income a business takes in from its operations",
                // `GlossaryTermStatus` is ENABLED | DISABLED (smithy_query
                // shape="GlossaryTermStatus").
                "status": "ENABLED",
            }),
            expected_status: GLOSSARY_TERM_STATUS_ENABLED,
        },
        TermCase {
            label: "explicitly DISABLED, short description only",
            body: json!({
                "name": "retired_term-2",
                "status": "DISABLED",
                "shortDescription": "s",
            }),
            expected_status: GLOSSARY_TERM_STATUS_DISABLED,
        },
        TermCase {
            label: "name and both descriptions at their modelled maxima",
            body: json!({
                "name": "n".repeat(256),
                // `ShortDescription` is ..1024 and `LongDescription` ..4096 (smithy_query
                // shape="ShortDescription,LongDescription"); these are the maxima.
                "shortDescription": "s".repeat(1024),
                "longDescription": "l".repeat(4096),
            }),
            expected_status: GLOSSARY_TERM_STATUS_ENABLED,
        },
        TermCase {
            label: "an isA relation naming one stored term",
            body: json!({
                "name": "narrower concept",
                // `TermRelations` carries `isA` and `classifies`, each a 1..10 list of
                // term ids (smithy_query shape="TermRelations").
                "termRelations": { "isA": [related[0]] },
            }),
            expected_status: GLOSSARY_TERM_STATUS_ENABLED,
        },
        TermCase {
            label: "isA and classifies relations naming two stored terms",
            body: json!({
                "name": "both relations",
                "termRelations": {
                    "isA": [related[0]],
                    "classifies": [related[0], related[1]],
                },
            }),
            expected_status: GLOSSARY_TERM_STATUS_ENABLED,
        },
    ]
}

/// Fill a case's body in with the glossary the term is an entry of.
fn under(body: &Value, glossary: &str) -> Value {
    let mut body = body.clone();
    body["glossaryIdentifier"] = json!(glossary);
    body
}

/// Create an available domain, a project in it and one glossary owned by that project,
/// answering the domain and glossary ids: every term case needs a glossary that exists.
///
/// `usage_restrictions` is passed through to `CreateGlossary`, so a caller can ask for
/// the restricted glossary a term's `usageRestrictions` must then report.
async fn domain_with_glossary(
    dz: &Emulator,
    name: &str,
    usage_restrictions: Option<&str>,
) -> (String, String) {
    let domain = dz.available_domain(name).await;
    let project = dz.project(&domain, &format!("{name} owner")).await;
    let glossary = glossary_in(dz, &domain, &project, &format!("{name} glossary"), usage_restrictions).await;
    (domain, glossary)
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

/// Create one term and answer its id, asserting only that the create succeeded: the
/// fixture a relation case needs.
async fn term_in(dz: &Emulator, domain: &str, glossary: &str, name: &str) -> String {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/glossary-terms"),
            Some(json!({ "glossaryIdentifier": glossary, "name": name })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating term {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

#[tokio::test]
async fn created_terms_are_persisted_scoped_and_reported_by_get() {
    let dz = Emulator::new();
    let (domain, glossary) = domain_with_glossary(&dz, "terms-domain", None).await;
    // A SECOND glossary in the same domain, restricted: the same term names go under it,
    // which is what makes the name rule per-glossary rather than per-domain, and its
    // terms must report the glossary's own usage restriction.
    let project = dz.project(&domain, "restricted owner").await;
    let restricted = glossary_in(
        &dz,
        &domain,
        &project,
        "restricted glossary",
        Some(ASSET_GOVERNED_TERMS),
    )
    .await;
    let (other_domain, _other_glossary) = domain_with_glossary(&dz, "other-terms-domain", None).await;
    let terms_uri = format!("/v2/domains/{domain}/glossary-terms");

    // The terms a relation case names: created first, in the same domain.
    let related = vec![
        term_in(&dz, &domain, &glossary, "base concept a").await,
        term_in(&dz, &domain, &glossary, "base concept b").await,
    ];

    // (label, id, request body, expected status, expected usageRestrictions)
    let mut created: Vec<(String, String, Value, &'static str, Option<Value>)> = Vec::new();
    for case in term_cases(&related) {
        for (glossary_label, glossary_id, restrictions) in [
            ("unrestricted glossary", &glossary, None),
            (
                "restricted glossary",
                &restricted,
                Some(json!([ASSET_GOVERNED_TERMS])),
            ),
        ] {
            let label = format!("{} in the {glossary_label}", case.label);
            let request = under(&case.body, glossary_id);
            let answer = dz.call("POST", &terms_uri, Some(request.clone())).await;
            assert_eq!(
                answer.status, 201,
                "case {label:?}: expected 201, got {} {:?}",
                answer.status, answer.body
            );
            let id = answer.s("id");
            assert!(
                is_glossary_term_id(&id),
                "case {label:?}: expected an id matching ^[a-zA-Z0-9_-]{{1,36}}$, got {id:?}"
            );
            assert_eq!(
                answer.s("domainId"),
                domain,
                "case {label:?}: expected the term to report its domain, got {:?}",
                answer.s("domainId")
            );
            assert_eq!(
                &answer.s("glossaryId"),
                glossary_id,
                "case {label:?}: expected the glossary from the request, got {:?}",
                answer.s("glossaryId")
            );
            assert_eq!(
                answer.s("status"),
                case.expected_status,
                "case {label:?}: expected status {:?}, got {:?}",
                case.expected_status,
                answer.s("status")
            );
            for member in ["name", "shortDescription", "longDescription"] {
                assert_eq!(
                    answer.body.get(member).and_then(Value::as_str),
                    request.get(member).and_then(Value::as_str),
                    "case {label:?}: expected the requested {member} back, got {:?}",
                    answer.body.get(member)
                );
            }
            assert_eq!(
                answer.body.get("termRelations").cloned(),
                request.get("termRelations").cloned(),
                "case {label:?}: expected the requested termRelations, got {:?}",
                answer.body.get("termRelations")
            );
            assert_eq!(
                answer.body.get("usageRestrictions").cloned(),
                restrictions.clone(),
                "case {label:?}: usageRestrictions must be the owning glossary's, got {:?}",
                answer.body.get("usageRestrictions")
            );
            created.push((label, id, request, case.expected_status, restrictions));
        }
    }

    // Every id is distinct: two terms never share one.
    let mut ids: Vec<&String> = created.iter().map(|(_, id, _, _, _)| id).collect();
    ids.sort();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        distinct,
        "each created term must have its own id, got {ids:?}"
    );

    // GetGlossaryTerm reports exactly what CreateGlossaryTerm stored.
    for (label, id, request, expected_status, restrictions) in &created {
        let got = dz.call("GET", &format!("{terms_uri}/{id}"), None).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: expected 200 from GetGlossaryTerm, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.s("id"),
            *id,
            "case {label:?}: GetGlossaryTerm must report the term that was read"
        );
        assert_eq!(
            got.s("domainId"),
            domain,
            "case {label:?}: GetGlossaryTerm must report the stored domain, got {:?}",
            got.s("domainId")
        );
        assert_eq!(
            got.body.get("glossaryId").and_then(Value::as_str),
            request.get("glossaryIdentifier").and_then(Value::as_str),
            "case {label:?}: GetGlossaryTerm must report the stored glossary, got {:?}",
            got.body.get("glossaryId")
        );
        for member in ["name", "shortDescription", "longDescription"] {
            assert_eq!(
                got.body.get(member).and_then(Value::as_str),
                request.get(member).and_then(Value::as_str),
                "case {label:?}: GetGlossaryTerm must report the persisted {member}, got {:?}",
                got.body.get(member)
            );
        }
        assert_eq!(
            got.body.get("termRelations").cloned(),
            request.get("termRelations").cloned(),
            "case {label:?}: GetGlossaryTerm must report the persisted termRelations, got {:?}",
            got.body.get("termRelations")
        );
        assert_eq!(
            got.s("status"),
            *expected_status,
            "case {label:?}: expected status {expected_status:?} from GetGlossaryTerm, got {:?}",
            got.s("status")
        );
        assert_eq!(
            got.body.get("usageRestrictions").cloned(),
            restrictions.clone(),
            "case {label:?}: GetGlossaryTerm must report the glossary's usageRestrictions, got {:?}",
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
        // Nothing has updated the term, so it reports no update.
        assert!(
            got.body.get("updatedAt").is_none() && got.body.get("updatedBy").is_none(),
            "case {label:?}: a term nothing updated must report no updatedAt/updatedBy, got {:?}",
            got.body
        );
    }

    // A term is scoped to its domain: the SAME id read through another domain is as
    // absent as one that was never created.
    for (label, id, _, _, _) in &created {
        let cross = dz
            .call(
                "GET",
                &format!("/v2/domains/{other_domain}/glossary-terms/{id}"),
                None,
            )
            .await;
        assert_eq!(
            cross.status, 404,
            "case {label:?}: a term of another domain must not be readable here, got {} {:?}",
            cross.status, cross.body
        );
        assert_eq!(
            cross.error_code, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            cross.error_code
        );
    }

    // Deleting the domain takes its terms with it.
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    for (label, id, _, _, _) in &created {
        let gone = dz.call("GET", &format!("{terms_uri}/{id}"), None).await;
        assert_eq!(
            gone.status, 404,
            "case {label:?}: a term must go with its domain, got {} {:?}",
            gone.status, gone.body
        );
    }
}

#[tokio::test]
async fn a_term_name_is_unique_within_its_glossary() {
    let dz = Emulator::new();
    let (domain, glossary) = domain_with_glossary(&dz, "unique-term-names", None).await;
    let project = dz.project(&domain, "second owner").await;
    let sibling = glossary_in(&dz, &domain, &project, "sibling glossary", None).await;
    let (other_domain, other_glossary) =
        domain_with_glossary(&dz, "unique-term-names-too", None).await;
    let terms_uri = format!("/v2/domains/{domain}/glossary-terms");
    let related = vec![
        term_in(&dz, &domain, &glossary, "relation target a").await,
        term_in(&dz, &domain, &glossary, "relation target b").await,
    ];

    for case in term_cases(&related) {
        let first = dz
            .call("POST", &terms_uri, Some(under(&case.body, &glossary)))
            .await;
        assert_eq!(
            first.status, 201,
            "case {:?}: the first term of this name must be created, got {} {:?}",
            case.label, first.status, first.body
        );
        let repeat = dz
            .call("POST", &terms_uri, Some(under(&case.body, &glossary)))
            .await;
        assert_eq!(
            repeat.status, 409,
            "case {:?}: a repeated name in the same glossary must conflict, got {} {:?}",
            case.label, repeat.status, repeat.body
        );
        assert_eq!(
            repeat.error_code, "ConflictException",
            "case {:?}: expected ConflictException, got {:?}",
            case.label, repeat.error_code
        );
        // The same name under ANOTHER glossary of the SAME domain is a different term.
        let sibling_term = dz
            .call("POST", &terms_uri, Some(under(&case.body, &sibling)))
            .await;
        assert_eq!(
            sibling_term.status, 201,
            "case {:?}: the same name under another glossary must be accepted, got {} {:?}",
            case.label, sibling_term.status, sibling_term.body
        );
        assert_ne!(
            sibling_term.s("id"),
            first.s("id"),
            "case {:?}: two glossaries' terms of one name must be separate resources",
            case.label
        );
        // And so is the same name in another DOMAIN's glossary. A relation names terms of
        // this domain, so the relation-carrying cases are sent there without one.
        let mut elsewhere_body = under(&case.body, &other_glossary);
        elsewhere_body
            .as_object_mut()
            .expect("the case body is an object")
            .remove("termRelations");
        let elsewhere = dz
            .call(
                "POST",
                &format!("/v2/domains/{other_domain}/glossary-terms"),
                Some(elsewhere_body),
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
            "case {:?}: two domains' terms of one name must be separate resources",
            case.label
        );
    }
}

#[tokio::test]
async fn a_term_needs_an_available_domain_a_stored_glossary_and_stored_relations() {
    let dz = Emulator::new();
    let (domain, glossary) = domain_with_glossary(&dz, "term-prerequisites", None).await;
    let (other_domain, other_glossary) =
        domain_with_glossary(&dz, "term-prerequisites-other", None).await;
    let stored_term = term_in(&dz, &domain, &glossary, "a stored term").await;
    let foreign_term = term_in(&dz, &other_domain, &other_glossary, "a foreign term").await;

    // A call scoped to a domain the store serves no record of is denied before any
    // lookup: a permission is held ON a domain, so the caller holds none there.
    let denied = dz
        .call(
            "POST",
            "/v2/domains/dzd_nosuchdomain000/glossary-terms",
            Some(json!({ "glossaryIdentifier": glossary.clone(), "name": "t" })),
        )
        .await;
    assert_eq!(
        (denied.status, denied.error_code.as_str(), denied.s("message")),
        (
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: CreateGlossaryTerm".to_string()
        ),
        "a term scoped to a domain nothing created must be denied, got {} {:?}",
        denied.status,
        denied.body
    );

    // Each of these names something the SERVED domain does not hold, so each is refused
    // with the `ResourceNotFoundException` this operation models (smithy_query
    // operation="CreateGlossaryTerm").
    let absent: Vec<(&str, String, Value)> = vec![
        (
            "a glossary nothing created",
            format!("/v2/domains/{domain}/glossary-terms"),
            json!({ "glossaryIdentifier": "nosuchglossary", "name": "t" }),
        ),
        (
            "a glossary that belongs to another domain",
            format!("/v2/domains/{domain}/glossary-terms"),
            json!({ "glossaryIdentifier": other_glossary.clone(), "name": "t" }),
        ),
        (
            "an isA relation naming a term nothing created",
            format!("/v2/domains/{domain}/glossary-terms"),
            json!({
                "glossaryIdentifier": glossary.clone(),
                "name": "t",
                "termRelations": { "isA": ["nosuchterm"] },
            }),
        ),
        (
            "a classifies relation naming a term of another domain",
            format!("/v2/domains/{domain}/glossary-terms"),
            json!({
                "glossaryIdentifier": glossary.clone(),
                "name": "t",
                "termRelations": { "classifies": [foreign_term.clone()] },
            }),
        ),
        (
            "a relation whose second entry names no stored term",
            format!("/v2/domains/{domain}/glossary-terms"),
            json!({
                "glossaryIdentifier": glossary.clone(),
                "name": "t",
                "termRelations": { "isA": [stored_term.clone(), "nosuchterm"] },
            }),
        ),
    ];
    for (label, uri, body) in absent {
        let answer = dz.call("POST", &uri, Some(body)).await;
        assert_eq!(
            answer.status, 404,
            "case {label:?}: expected 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code
        );
    }

    // A term the refused requests would have created is not in the store: the name they
    // all used is still free.
    let free = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/glossary-terms"),
            Some(json!({ "glossaryIdentifier": glossary, "name": "t" })),
        )
        .await;
    assert_eq!(
        free.status, 201,
        "a refused request must have persisted nothing, got {} {:?}",
        free.status, free.body
    );

    // A domain that is still being created cannot take a term yet: it is reported in a
    // non-terminal status, so the request conflicts with that status.
    let creating = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "still-creating-terms" })),
        )
        .await;
    assert_eq!(
        creating.status, 201,
        "creating the domain: expected 201, got {} {:?}",
        creating.status, creating.body
    );
    let too_early = dz
        .call(
            "POST",
            &format!("/v2/domains/{}/glossary-terms", creating.s("id")),
            Some(json!({ "glossaryIdentifier": "anyglossary", "name": "t" })),
        )
        .await;
    assert_eq!(
        too_early.status, 409,
        "a domain that has not settled yet must refuse a term, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code, "ConflictException",
        "expected ConflictException, got {:?}",
        too_early.error_code
    );
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_term() {
    let dz = Emulator::new();
    let (domain, glossary) = domain_with_glossary(&dz, "idempotent-terms", None).await;
    let terms_uri = format!("/v2/domains/{domain}/glossary-terms");
    let related = vec![
        term_in(&dz, &domain, &glossary, "token relation a").await,
        term_in(&dz, &domain, &glossary, "token relation b").await,
    ];

    for (index, case) in term_cases(&related).into_iter().enumerate() {
        let mut body = under(&case.body, &glossary);
        // `ClientToken` is 1..128 of `^[\x21-\x7E]+$` (smithy_query shape="ClientToken").
        body["clientToken"] = json!(format!("term-token-{index}"));
        let first = dz.call("POST", &terms_uri, Some(body.clone())).await;
        assert_eq!(
            first.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, first.status, first.body
        );
        let repeat = dz.call("POST", &terms_uri, Some(body)).await;
        assert_eq!(
            repeat.status, 201,
            "case {:?}: a repeat carrying the same clientToken must succeed, got {} {:?}",
            case.label, repeat.status, repeat.body
        );
        assert_eq!(
            repeat.s("id"),
            first.s("id"),
            "case {:?}: the repeat must return the first term, got {:?}",
            case.label,
            repeat.s("id")
        );
        // And the retry left nothing extra behind: the one term still reads back with the
        // name the first call stored.
        let got = dz
            .call("GET", &format!("{terms_uri}/{}", first.s("id")), None)
            .await;
        assert_eq!(
            got.status, 200,
            "case {:?}: the term the token created must still be readable, got {} {:?}",
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
