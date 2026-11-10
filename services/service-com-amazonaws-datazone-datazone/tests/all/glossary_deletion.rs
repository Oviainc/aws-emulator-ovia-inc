// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteGlossary`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the responses as evidence, so a handler that
//! answered from a hardcoded value instead of from the store fails here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. The documented prerequisites (`smithy_query(query="doc",shape="DeleteGlossary")`)
//!    decide the answer: a stored glossary goes only while it is in DISABLED state and
//!    no stored glossary term is an entry of it. Then the call answers 204 and the
//!    glossary stops being readable, so a second delete of the same identifier is a
//!    miss.
//! 2. A glossary that fails either prerequisite is refused with the modelled
//!    `ConflictException` and the refusal stores NOTHING: the glossary, its terms and
//!    every other glossary of the domain are readable afterwards exactly as before.
//!    How the glossary reached DISABLED — the create naming the status or a later
//!    `UpdateGlossary` — makes no difference, and neither does how many terms it holds.
//! 3. A delete is scoped to the domain in the request path and addresses a glossary by
//!    the identifier that READS it, so a glossary of another domain, or of no domain, is
//!    a `ResourceNotFoundException` naming the KIND and never the caller's identifier,
//!    while an identifier this store serves no domain for is denied.

use crate::harness::Emulator;
use serde_json::json;
// The `GlossaryStatus` members as the service stores them, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    GLOSSARY_STATUS_DISABLED, GLOSSARY_STATUS_ENABLED,
};

/// The `ResourceNotFoundException` message the service answers for a business glossary it
/// cannot find: a fixed sentence naming the resource by its API TYPE name rather than the
/// caller's identifier (`crate::state::GLOSSARY_NOT_FOUND_MESSAGE`).
const GLOSSARY_NOT_FOUND: &str =
    "The given Glossary doesn't exist. Try creating Glossary before accessing it.";

/// The same, for a domain the service cannot find.
const DOMAIN_NOT_FOUND: &str = "Domain not found";

/// The `ConflictException` message a glossary that still has terms filed under it is
/// refused with: the service's own wording
/// (`crate::state::GLOSSARY_HAS_TERMS_MESSAGE`), a fixed sentence naming neither the
/// glossary nor the term.
const GLOSSARY_HAS_TERMS: &str = "Glossary can't be deleted while having glossary terms attached";

/// The `ConflictException` message a glossary that is not DISABLED yet is refused with,
/// in that same form with the `GlossaryStatus` read from the store
/// (`crate::state::glossary_not_disabled_message`).
fn not_disabled_message(status: &str) -> String {
    format!("Glossary can't be deleted while it is in {status} state")
}

/// What a case expects `DeleteGlossary` to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// 204, and the glossary stops being readable.
    Removed,
    /// 409, because the glossary is still in this `GlossaryStatus`.
    StillInStatus(&'static str),
    /// 409, because a stored term is still an entry of the glossary.
    StillHasTerms,
}

/// One glossary to delete: how it was created, what moved it since, how many terms are
/// filed under it, and what the delete must then answer.
struct DeleteCase {
    label: &'static str,
    /// The `status` the `CreateGlossary` request names, or `None` to let the create
    /// default it.
    created_status: Option<&'static str>,
    /// The `GlossaryStatus` an `UpdateGlossary` moves the glossary to before the delete,
    /// when one does.
    updated_status: Option<&'static str>,
    /// How many glossary terms are filed under the glossary before the delete.
    terms: usize,
    expected: Expected,
}

/// The cases span what the rule generalises over: each member of `GlossaryStatus` plus
/// the create that names none, the status reached by the create and the status reached by
/// a later update, and a term count of none, one and several.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "created DISABLED, no terms",
            // `GlossaryStatus` is DISABLED | ENABLED (smithy_query
            // shape="GlossaryStatus").
            created_status: Some(GLOSSARY_STATUS_DISABLED),
            updated_status: None,
            terms: 0,
            expected: Expected::Removed,
        },
        DeleteCase {
            label: "created ENABLED, disabled by a later update, no terms",
            created_status: Some(GLOSSARY_STATUS_ENABLED),
            updated_status: Some(GLOSSARY_STATUS_DISABLED),
            terms: 0,
            expected: Expected::Removed,
        },
        DeleteCase {
            label: "created with no status, no terms",
            created_status: None,
            updated_status: None,
            terms: 0,
            expected: Expected::StillInStatus(GLOSSARY_STATUS_ENABLED),
        },
        DeleteCase {
            label: "created ENABLED, no terms",
            created_status: Some(GLOSSARY_STATUS_ENABLED),
            updated_status: None,
            terms: 0,
            expected: Expected::StillInStatus(GLOSSARY_STATUS_ENABLED),
        },
        DeleteCase {
            label: "created DISABLED, re-enabled by a later update, no terms",
            created_status: Some(GLOSSARY_STATUS_DISABLED),
            updated_status: Some(GLOSSARY_STATUS_ENABLED),
            terms: 0,
            expected: Expected::StillInStatus(GLOSSARY_STATUS_ENABLED),
        },
        DeleteCase {
            label: "created ENABLED, one term",
            created_status: Some(GLOSSARY_STATUS_ENABLED),
            updated_status: None,
            terms: 1,
            expected: Expected::StillInStatus(GLOSSARY_STATUS_ENABLED),
        },
        DeleteCase {
            label: "created DISABLED, one term",
            created_status: Some(GLOSSARY_STATUS_DISABLED),
            updated_status: None,
            terms: 1,
            expected: Expected::StillHasTerms,
        },
        DeleteCase {
            label: "created ENABLED, disabled by a later update, two terms",
            created_status: Some(GLOSSARY_STATUS_ENABLED),
            updated_status: Some(GLOSSARY_STATUS_DISABLED),
            terms: 2,
            expected: Expected::StillHasTerms,
        },
        DeleteCase {
            label: "created DISABLED, three terms",
            created_status: Some(GLOSSARY_STATUS_DISABLED),
            updated_status: None,
            terms: 3,
            expected: Expected::StillHasTerms,
        },
    ]
}

/// Create an available domain with one project in it: a glossary needs a domain that
/// exists and an owning project inside it.
async fn domain_with_project(dz: &Emulator, name: &str) -> (String, String) {
    let domain = dz.available_domain(name).await;
    let project = dz.project(&domain, &format!("{name} owner")).await;
    (domain, project)
}

/// Create one glossary, move it if the case says so, file its terms under it, and answer
/// the glossary id together with the ids of the terms.
async fn glossary_for(
    dz: &Emulator,
    domain: &str,
    project: &str,
    name: &str,
    case: &DeleteCase,
) -> (String, Vec<String>) {
    let mut body = json!({ "name": name, "owningProjectIdentifier": project });
    if let Some(status) = case.created_status {
        body["status"] = json!(status);
    }
    let created = dz
        .call("POST", &format!("/v2/domains/{domain}/glossaries"), Some(body))
        .await;
    assert_eq!(
        created.status, 201,
        "case {:?}: creating the glossary: expected 201, got {} {:?}",
        case.label, created.status, created.body
    );
    let glossary = created.s("id");

    if let Some(status) = case.updated_status {
        let updated = dz
            .call(
                "PATCH",
                &format!("/v2/domains/{domain}/glossaries/{glossary}"),
                Some(json!({ "status": status })),
            )
            .await;
        assert_eq!(
            (updated.status, updated.s("status").as_str()),
            (200, status),
            "case {:?}: moving the glossary to {status:?}: got {} {:?}",
            case.label,
            updated.status,
            updated.body
        );
    }

    let mut terms = Vec::new();
    for index in 0..case.terms {
        let term = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/glossary-terms"),
                Some(json!({
                    "glossaryIdentifier": glossary,
                    "name": format!("{name} term {index}"),
                })),
            )
            .await;
        assert_eq!(
            term.status, 201,
            "case {:?}: creating term {index}: expected 201, got {} {:?}",
            case.label, term.status, term.body
        );
        terms.push(term.s("id"));
    }
    (glossary, terms)
}

#[tokio::test]
async fn a_glossary_is_deleted_only_while_disabled_and_free_of_terms() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "glossary-deletes").await;

    // A glossary of the SAME domain that no case touches: every delete, refused or not,
    // must leave it exactly where it is.
    let (bystander, _) = glossary_for(
        &dz,
        &domain,
        &project,
        "bystander",
        &DeleteCase {
            label: "bystander",
            created_status: Some(GLOSSARY_STATUS_DISABLED),
            updated_status: None,
            terms: 0,
            expected: Expected::Removed,
        },
    )
    .await;

    for case in delete_cases() {
        let (glossary, terms) =
            glossary_for(&dz, &domain, &project, case.label, &case).await;
        let uri = format!("/v2/domains/{domain}/glossaries/{glossary}");
        let answer = dz.call("DELETE", &uri, None).await;

        match case.expected {
            Expected::Removed => {
                assert_eq!(
                    answer.status, 204,
                    "case {:?}: expected 204, got {} {:?}",
                    case.label, answer.status, answer.body
                );
                // The record is gone from the store: the reader misses it, and so does
                // every repeat of the delete.
                let gone = dz.call("GET", &uri, None).await;
                assert_eq!(
                    (gone.status, gone.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {:?}: a deleted glossary must not be readable, got {} {:?}",
                    case.label,
                    gone.status,
                    gone.body
                );
                assert_eq!(
                    gone.s("message"),
                    GLOSSARY_NOT_FOUND,
                    "case {:?}: expected {GLOSSARY_NOT_FOUND:?}, got {:?}",
                    case.label,
                    gone.s("message")
                );
                let repeat = dz.call("DELETE", &uri, None).await;
                assert_eq!(
                    (repeat.status, repeat.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {:?}: a second delete must be a miss, got {} {:?}",
                    case.label,
                    repeat.status,
                    repeat.body
                );
            }
            Expected::StillInStatus(status) => {
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (409, "ConflictException"),
                    "case {:?}: expected 409 ConflictException, got {} {:?}",
                    case.label,
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.s("message"),
                    not_disabled_message(status),
                    "case {:?}: expected the status to be named in the refusal, got {:?}",
                    case.label,
                    answer.s("message")
                );
            }
            Expected::StillHasTerms => {
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (409, "ConflictException"),
                    "case {:?}: expected 409 ConflictException, got {} {:?}",
                    case.label,
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.s("message"),
                    GLOSSARY_HAS_TERMS,
                    "case {:?}: expected {GLOSSARY_HAS_TERMS:?}, got {:?}",
                    case.label,
                    answer.s("message")
                );
            }
        }

        if case.expected != Expected::Removed {
            // A refused delete stores NOTHING: the glossary is still readable, in the
            // status it had, and every term filed under it is still readable too.
            let still = dz.call("GET", &uri, None).await;
            assert_eq!(
                still.status, 200,
                "case {:?}: a refused delete must leave the glossary readable, got {} {:?}",
                case.label, still.status, still.body
            );
            assert_eq!(
                still.s("id"),
                glossary,
                "case {:?}: expected the same glossary, got {:?}",
                case.label,
                still.s("id")
            );
            for term in &terms {
                let term_read = dz
                    .call(
                        "GET",
                        &format!("/v2/domains/{domain}/glossary-terms/{term}"),
                        None,
                    )
                    .await;
                assert_eq!(
                    term_read.status, 200,
                    "case {:?}: term {term} must survive a refused delete, got {} {:?}",
                    case.label, term_read.status, term_read.body
                );
            }
        }
    }

    // Nothing above touched the bystander.
    let untouched = dz
        .call("GET", &format!("/v2/domains/{domain}/glossaries/{bystander}"), None)
        .await;
    assert_eq!(
        untouched.status, 200,
        "a glossary no delete addressed must still be readable, got {} {:?}",
        untouched.status, untouched.body
    );
}

#[tokio::test]
async fn a_delete_addresses_a_glossary_of_the_domain_in_the_request_path() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "delete-scoping").await;
    let (other_domain, other_project) = domain_with_project(&dz, "delete-scoping-other").await;

    let disabled = DeleteCase {
        label: "scoping",
        created_status: Some(GLOSSARY_STATUS_DISABLED),
        updated_status: None,
        terms: 0,
        expected: Expected::Removed,
    };
    let (elsewhere, _) = glossary_for(
        &dz,
        &other_domain,
        &other_project,
        "glossary of another domain",
        &disabled,
    )
    .await;

    // A glossary nothing created, and one created in ANOTHER domain: each is a miss in
    // THIS domain, reported by the kind alone.
    let misses: Vec<(&str, String)> = vec![
        (
            "a well-formed glossary id nothing created",
            format!("/v2/domains/{domain}/glossaries/nosuchglossary"),
        ),
        (
            "a glossary that belongs to another domain",
            format!("/v2/domains/{domain}/glossaries/{elsewhere}"),
        ),
    ];
    for (label, uri) in misses {
        let answer = dz.call("DELETE", &uri, None).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: expected 404 ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.s("message"),
            GLOSSARY_NOT_FOUND,
            "case {label:?}: expected {GLOSSARY_NOT_FOUND:?}, got {:?}",
            answer.s("message")
        );
    }
    // And the other domain's glossary is untouched by the call that named it here.
    let intact = dz
        .call(
            "GET",
            &format!("/v2/domains/{other_domain}/glossaries/{elsewhere}"),
            None,
        )
        .await;
    assert_eq!(
        intact.status, 200,
        "the other domain's glossary must survive a delete scoped to this one, got {} {:?}",
        intact.status, intact.body
    );

    // An identifier this store serves no domain for: a permission is held ON a domain,
    // so the caller holds none and the request is denied rather than reported as a miss.
    let denied = dz
        .call(
            "DELETE",
            "/v2/domains/dzd_nosuchdomain000/glossaries/anyglossary",
            None,
        )
        .await;
    assert_eq!(
        (denied.status, denied.error_code.as_str()),
        (403, "AccessDeniedException"),
        "an unknown domain must be denied, got {} {:?}",
        denied.status,
        denied.body
    );
    assert_eq!(
        denied.s("message"),
        "User is not permitted to perform operation: DeleteGlossary",
        "expected the operation name in the denial, got {:?}",
        denied.s("message")
    );

    // A domain on its way out holds nothing to delete: its glossaries are about to go
    // with it, so a glossary of it reads as a missing DOMAIN rather than a missing
    // glossary.
    let (doomed, doomed_project) = domain_with_project(&dz, "delete-scoping-doomed").await;
    let (glossary, _) = glossary_for(
        &dz,
        &doomed,
        &doomed_project,
        "glossary of a doomed domain",
        &disabled,
    )
    .await;
    let deleted = dz.call("DELETE", &format!("/v2/domains/{doomed}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    let orphan = dz
        .call("DELETE", &format!("/v2/domains/{doomed}/glossaries/{glossary}"), None)
        .await;
    assert_eq!(
        (orphan.status, orphan.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a domain being deleted must hold no glossary to delete, got {} {:?}",
        orphan.status,
        orphan.body
    );
    assert_eq!(
        orphan.s("message"),
        DOMAIN_NOT_FOUND,
        "expected {DOMAIN_NOT_FOUND:?}, got {:?}",
        orphan.s("message")
    );
    // A 204 carries no members: the operation models no output members at all.
    let (spare, _) = glossary_for(&dz, &domain, &project, "spare", &disabled).await;
    let removed = dz
        .call("DELETE", &format!("/v2/domains/{domain}/glossaries/{spare}"), None)
        .await;
    assert_eq!(
        removed.status, 204,
        "expected 204, got {} {:?}",
        removed.status, removed.body
    );
    assert!(
        removed.body.as_object().is_some_and(|members| members.is_empty()),
        "expected a response with no members, got {:?}",
        removed.body
    );
}
