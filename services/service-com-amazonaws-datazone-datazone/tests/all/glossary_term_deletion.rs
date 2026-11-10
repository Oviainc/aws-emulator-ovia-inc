// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteGlossaryTerm`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the responses as evidence, so a handler that
//! answered from a hardcoded value instead of from the store fails here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A stored glossary term goes exactly when no other object of its domain still NAMES
//!    it (`smithy_query(query="doc",shape="DeleteGlossaryTerm")`: "The term must not be
//!    linked to other assets or child terms"). Then the call answers 204 with no members,
//!    the term stops being readable, a second delete of the same identifier is a miss,
//!    and the glossary it was an entry of stops being blocked by it.
//! 2. A term another object names is refused with the modelled `ConflictException`, whose
//!    message names the KIND of object that is linked, and the refusal stores NOTHING:
//!    the term, the object that named it and every other term of the domain are readable
//!    afterwards exactly as before. A link the term HOLDS — its own `termRelations` —
//!    is not a link held ON it and blocks nothing.
//! 3. The `GlossaryTermStatus` the term reports does not gate the removal: a term reached
//!    through an `UpdateGlossaryTerm` to either member of that enum deletes the same way.
//! 4. A delete is scoped to the domain in the request path and addresses a term by the
//!    identifier that READS it, so a term of another domain, or of no domain, is a
//!    `ResourceNotFoundException` naming the KIND and never the caller's identifier, while
//!    an identifier this store serves no domain for is denied.

use crate::harness::Emulator;
use serde_json::{json, Value};
// The `GlossaryTermStatus` members as the service stores them, so a drift between the
// constant and the value on the wire fails here.
use service_com_amazonaws_datazone_datazone::state::{
    GLOSSARY_STATUS_DISABLED, GLOSSARY_TERM_STATUS_DISABLED, GLOSSARY_TERM_STATUS_ENABLED,
};

/// The `ResourceNotFoundException` message the service answers for a business glossary
/// term it cannot find: a fixed sentence naming the resource by its API TYPE name rather
/// than the caller's identifier (`crate::state::GLOSSARY_TERM_NOT_FOUND_MESSAGE`).
const TERM_NOT_FOUND: &str =
    "The given GlossaryTerm doesn't exist. Try creating GlossaryTerm before accessing it.";

/// The same, for a domain the service cannot find.
const DOMAIN_NOT_FOUND: &str = "Domain not found";

/// The `ConflictException` message a term another object is still linked to is refused
/// with: the service's wording for a delete a stored dependency prevents in this resource
/// family, with the kind of linked object read from the store
/// (`crate::state::glossary_term_linked_message`).
fn linked_message(kind: &str) -> String {
    format!("Glossary term can't be deleted while having {kind} attached")
}

/// What a case expects `DeleteGlossaryTerm` to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// 204, and the term stops being readable.
    Removed,
    /// 409, because an object of this kind still names the term.
    LinkedTo(&'static str),
}

/// How the domain is arranged around the term before it is deleted. The rows span what
/// the rule generalises over: no link at all, a link the term HOLDS, a link held on it by
/// each of the three stored object kinds that can name a `GlossaryTermId`, and each
/// member of `GlossaryTermStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Nothing in the domain names the term.
    Unlinked,
    /// Nothing names it, and an `UpdateGlossaryTerm` moved it to this
    /// `GlossaryTermStatus` first.
    UnlinkedInStatus(&'static str),
    /// The term's OWN `termRelations` names another term through this member.
    RelatesToAnother(&'static str),
    /// Another term of the domain names it through this `termRelations` member.
    NamedByTerm(&'static str),
    /// An asset of the domain attaches it in `glossaryTerms`.
    AttachedByAsset,
    /// An asset attached it at its first revision and a later revision dropped it.
    AttachedByEarlierAssetRevision,
    /// A project of the domain attaches it in `glossaryTerms`.
    AttachedByProject,
}

struct DeleteCase {
    label: &'static str,
    setup: Setup,
    expected: Expected,
}

fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "nothing names the term",
            setup: Setup::Unlinked,
            expected: Expected::Removed,
        },
        DeleteCase {
            // `GlossaryTermStatus` is DISABLED | ENABLED (smithy_query
            // shape="GlossaryTermStatus"); both members are covered.
            label: "nothing names the term, disabled by a later update",
            setup: Setup::UnlinkedInStatus(GLOSSARY_TERM_STATUS_DISABLED),
            expected: Expected::Removed,
        },
        DeleteCase {
            label: "nothing names the term, re-enabled by a later update",
            setup: Setup::UnlinkedInStatus(GLOSSARY_TERM_STATUS_ENABLED),
            expected: Expected::Removed,
        },
        DeleteCase {
            // `TermRelations` has `isA` and `classifies` (smithy_query
            // shape="TermRelations"); a link the term holds is covered through each.
            label: "the term's own isA names another term",
            setup: Setup::RelatesToAnother("isA"),
            expected: Expected::Removed,
        },
        DeleteCase {
            label: "the term's own classifies names another term",
            setup: Setup::RelatesToAnother("classifies"),
            expected: Expected::Removed,
        },
        DeleteCase {
            label: "another term's isA names it",
            setup: Setup::NamedByTerm("isA"),
            expected: Expected::LinkedTo("glossary terms"),
        },
        DeleteCase {
            label: "another term's classifies names it",
            setup: Setup::NamedByTerm("classifies"),
            expected: Expected::LinkedTo("glossary terms"),
        },
        DeleteCase {
            label: "an asset attaches it",
            setup: Setup::AttachedByAsset,
            expected: Expected::LinkedTo("assets"),
        },
        DeleteCase {
            label: "an earlier asset revision attaches it",
            setup: Setup::AttachedByEarlierAssetRevision,
            expected: Expected::LinkedTo("assets"),
        },
        DeleteCase {
            label: "a project attaches it",
            setup: Setup::AttachedByProject,
            expected: Expected::LinkedTo("projects"),
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

/// Create one DISABLED glossary in the domain: DISABLED is what lets `DeleteGlossary`
/// take it once its terms are gone, which is how a case proves the term really left the
/// store.
async fn disabled_glossary(dz: &Emulator, domain: &str, project: &str, name: &str) -> String {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/glossaries"),
            Some(json!({
                "name": name,
                "owningProjectIdentifier": project,
                "status": GLOSSARY_STATUS_DISABLED,
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating glossary {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// Create one glossary term from `body` (which must carry at least a name), and answer
/// its id.
async fn create_term(dz: &Emulator, domain: &str, label: &str, body: Value) -> String {
    let created = dz
        .call("POST", &format!("/v2/domains/{domain}/glossary-terms"), Some(body))
        .await;
    assert_eq!(
        created.status, 201,
        "case {label:?}: creating the term: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// Arrange one case's domain and answer the id of the term the case deletes.
async fn term_for(
    dz: &Emulator,
    domain: &str,
    project: &str,
    glossary: &str,
    case: &DeleteCase,
) -> String {
    let label = case.label;
    match case.setup {
        Setup::Unlinked => {
            create_term(
                dz,
                domain,
                label,
                json!({ "glossaryIdentifier": glossary, "name": label }),
            )
            .await
        }
        Setup::UnlinkedInStatus(status) => {
            let term = create_term(
                dz,
                domain,
                label,
                json!({ "glossaryIdentifier": glossary, "name": label }),
            )
            .await;
            let updated = dz
                .call(
                    "PATCH",
                    &format!("/v2/domains/{domain}/glossary-terms/{term}"),
                    Some(json!({ "status": status })),
                )
                .await;
            assert_eq!(
                (updated.status, updated.s("status").as_str()),
                (200, status),
                "case {label:?}: moving the term to {status:?}: got {} {:?}",
                updated.status,
                updated.body
            );
            term
        }
        Setup::RelatesToAnother(relation) => {
            let other = create_term(
                dz,
                domain,
                label,
                json!({ "glossaryIdentifier": glossary, "name": format!("{label} target") }),
            )
            .await;
            create_term(
                dz,
                domain,
                label,
                json!({
                    "glossaryIdentifier": glossary,
                    "name": label,
                    "termRelations": { relation: [other] },
                }),
            )
            .await
        }
        Setup::NamedByTerm(relation) => {
            let term = create_term(
                dz,
                domain,
                label,
                json!({ "glossaryIdentifier": glossary, "name": label }),
            )
            .await;
            create_term(
                dz,
                domain,
                label,
                json!({
                    "glossaryIdentifier": glossary,
                    "name": format!("{label} holder"),
                    "termRelations": { relation: [term.clone()] },
                }),
            )
            .await;
            term
        }
        Setup::AttachedByAsset | Setup::AttachedByEarlierAssetRevision => {
            let term = create_term(
                dz,
                domain,
                label,
                json!({ "glossaryIdentifier": glossary, "name": label }),
            )
            .await;
            let asset = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/assets"),
                    Some(json!({
                        "name": format!("{label} asset"),
                        "typeIdentifier": "amazon.datazone.RelationalTable",
                        "owningProjectIdentifier": project,
                        "glossaryTerms": [term.clone()],
                    })),
                )
                .await;
            assert_eq!(
                asset.status, 201,
                "case {label:?}: creating the asset: expected 201, got {} {:?}",
                asset.status, asset.body
            );
            if case.setup == Setup::AttachedByEarlierAssetRevision {
                // A revision that names no `glossaryTerms` attaches none, so the term is
                // left behind by the NEWEST revision while the first one still records
                // it — and a revision is a record this store still serves.
                let asset_id = asset.s("id");
                let revised = dz
                    .call(
                        "POST",
                        &format!("/v2/domains/{domain}/assets/{asset_id}/revisions"),
                        Some(json!({ "name": format!("{label} asset v2") })),
                    )
                    .await;
                assert_eq!(
                    revised.status, 200,
                    "case {label:?}: recording a second revision: expected 200, got {} {:?}",
                    revised.status, revised.body
                );
                assert!(
                    revised.body.get("glossaryTerms").is_none(),
                    "case {label:?}: the newest revision must attach no term, got {:?}",
                    revised.body
                );
            }
            term
        }
        Setup::AttachedByProject => {
            let term = create_term(
                dz,
                domain,
                label,
                json!({ "glossaryIdentifier": glossary, "name": label }),
            )
            .await;
            let created = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/projects"),
                    Some(json!({
                        "name": format!("{label} project"),
                        "glossaryTerms": [term.clone()],
                    })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {label:?}: creating the project: expected 201, got {} {:?}",
                created.status, created.body
            );
            term
        }
    }
}

#[tokio::test]
async fn a_glossary_term_is_deleted_only_while_nothing_else_names_it() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "term-deletes").await;
    let glossary = disabled_glossary(&dz, &domain, &project, "deletion glossary").await;

    // A term of the SAME glossary that no case touches: every delete, refused or not,
    // must leave it exactly where it is, so a handler that cleared the map instead of
    // one entry fails here.
    let bystander = create_term(
        &dz,
        &domain,
        "bystander",
        json!({ "glossaryIdentifier": glossary, "name": "bystander" }),
    )
    .await;

    for case in delete_cases() {
        let term = term_for(&dz, &domain, &project, &glossary, &case).await;
        let uri = format!("/v2/domains/{domain}/glossary-terms/{term}");
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
                    "case {:?}: a deleted term must not be readable, got {} {:?}",
                    case.label,
                    gone.status,
                    gone.body
                );
                assert_eq!(
                    gone.s("message"),
                    TERM_NOT_FOUND,
                    "case {:?}: expected {TERM_NOT_FOUND:?}, got {:?}",
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
            Expected::LinkedTo(kind) => {
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
                    linked_message(kind),
                    "case {:?}: expected the linked kind to be named in the refusal, got {:?}",
                    case.label,
                    answer.s("message")
                );
                // A refused delete stores NOTHING: the term is still readable, as the
                // term it was.
                let still = dz.call("GET", &uri, None).await;
                assert_eq!(
                    still.status, 200,
                    "case {:?}: a refused delete must leave the term readable, got {} {:?}",
                    case.label, still.status, still.body
                );
                assert_eq!(
                    still.s("id"),
                    term,
                    "case {:?}: expected the same term, got {:?}",
                    case.label,
                    still.s("id")
                );
            }
        }
    }

    // Nothing above touched the bystander.
    let untouched = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/glossary-terms/{bystander}"),
            None,
        )
        .await;
    assert_eq!(
        untouched.status, 200,
        "a term no delete addressed must still be readable, got {} {:?}",
        untouched.status, untouched.body
    );
}

#[tokio::test]
async fn a_glossary_is_deletable_once_its_terms_are_gone() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "term-then-glossary").await;
    let glossary = disabled_glossary(&dz, &domain, &project, "emptied glossary").await;
    let glossary_uri = format!("/v2/domains/{domain}/glossaries/{glossary}");

    // Several terms, so the glossary is blocked until the LAST one goes: the block is a
    // property of the terms the store holds, not of one call having happened.
    let mut terms = Vec::new();
    for index in 0..3 {
        terms.push(
            create_term(
                &dz,
                &domain,
                "emptying",
                json!({
                    "glossaryIdentifier": glossary,
                    "name": format!("emptying term {index}"),
                }),
            )
            .await,
        );
    }

    for (index, term) in terms.iter().enumerate() {
        let blocked = dz.call("DELETE", &glossary_uri, None).await;
        assert_eq!(
            (blocked.status, blocked.error_code.as_str()),
            (409, "ConflictException"),
            "case {index}: a glossary with {} term(s) left must be refused, got {} {:?}",
            terms.len() - index,
            blocked.status,
            blocked.body
        );
        let removed = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/glossary-terms/{term}"),
                None,
            )
            .await;
        assert_eq!(
            removed.status, 204,
            "case {index}: deleting term {term}: expected 204, got {} {:?}",
            removed.status, removed.body
        );
        // A 204 carries no members: the operation models no output members at all.
        assert!(
            removed.body.is_null()
                || removed
                    .body
                    .as_object()
                    .is_some_and(|members| members.is_empty()),
            "case {index}: expected a response with no members, got {:?}",
            removed.body
        );
    }

    let emptied = dz.call("DELETE", &glossary_uri, None).await;
    assert_eq!(
        emptied.status, 204,
        "a glossary whose terms are all deleted must go: got {} {:?}",
        emptied.status, emptied.body
    );
}

#[tokio::test]
async fn a_delete_addresses_a_term_of_the_domain_in_the_request_path() {
    let dz = Emulator::new();
    let (domain, project) = domain_with_project(&dz, "term-delete-scoping").await;
    let glossary = disabled_glossary(&dz, &domain, &project, "scoping glossary").await;
    let (other_domain, other_project) =
        domain_with_project(&dz, "term-delete-scoping-other").await;
    let other_glossary =
        disabled_glossary(&dz, &other_domain, &other_project, "other glossary").await;
    let elsewhere = create_term(
        &dz,
        &other_domain,
        "another domain's term",
        json!({ "glossaryIdentifier": other_glossary, "name": "term of another domain" }),
    )
    .await;

    // A term nothing created, and one created in ANOTHER domain: each is a miss in THIS
    // domain, reported by the kind alone.
    let misses: Vec<(&str, String)> = vec![
        (
            "a well-formed term id nothing created",
            format!("/v2/domains/{domain}/glossary-terms/nosuchterm"),
        ),
        (
            "a term that belongs to another domain",
            format!("/v2/domains/{domain}/glossary-terms/{elsewhere}"),
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
            TERM_NOT_FOUND,
            "case {label:?}: expected {TERM_NOT_FOUND:?}, got {:?}",
            answer.s("message")
        );
    }
    // And the other domain's term is untouched by the call that named it here.
    let intact = dz
        .call(
            "GET",
            &format!("/v2/domains/{other_domain}/glossary-terms/{elsewhere}"),
            None,
        )
        .await;
    assert_eq!(
        intact.status, 200,
        "the other domain's term must survive a delete scoped to this one, got {} {:?}",
        intact.status, intact.body
    );

    // An identifier this store serves no domain for: a permission is held ON a domain,
    // so the caller holds none and the request is denied rather than reported as a miss.
    let denied = dz
        .call(
            "DELETE",
            "/v2/domains/dzd_nosuchdomain000/glossary-terms/anyterm",
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
        "User is not permitted to perform operation: DeleteGlossaryTerm",
        "expected the operation name in the denial, got {:?}",
        denied.s("message")
    );

    // A domain on its way out holds nothing to delete: its terms are about to go with
    // it, so a term of it reads as a missing DOMAIN rather than a missing term.
    let doomed_term = create_term(
        &dz,
        &domain,
        "doomed domain's term",
        json!({ "glossaryIdentifier": glossary, "name": "term of a doomed domain" }),
    )
    .await;
    let deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        deleted.status, 202,
        "deleting the domain: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    let orphan = dz
        .call(
            "DELETE",
            &format!("/v2/domains/{domain}/glossary-terms/{doomed_term}"),
            None,
        )
        .await;
    assert_eq!(
        (orphan.status, orphan.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a domain being deleted must hold no term to delete, got {} {:?}",
        orphan.status,
        orphan.body
    );
    assert_eq!(
        orphan.s("message"),
        DOMAIN_NOT_FOUND,
        "expected {DOMAIN_NOT_FOUND:?}, got {:?}",
        orphan.s("message")
    );
}
