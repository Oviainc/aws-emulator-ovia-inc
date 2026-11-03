// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteDomainUnit`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the responses as evidence, so a handler that
//! answered from a hardcoded value instead of from the store fails here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. A domain unit nothing in the domain hangs under is DELETED: the call answers the
//!    modelled 204 with an empty body, the unit stops being listed under its parent, and
//!    its name is free again for a new child of that parent.
//! 2. A domain unit something in the domain still hangs under is refused with
//!    `ConflictException` (HTTP 409) naming the unit the request addressed and the
//!    object that prevented the delete, and the refusal stores nothing: the unit is
//!    still readable afterwards. Once the blocking object is gone, the same delete
//!    succeeds — so a bottom-up teardown works level by level.
//! 3. A unit the service has deleted is no longer a resource anything is held on: every
//!    later call addressing that id — the read and a repeated delete — answers 403
//!    `AccessDeniedException` with the combined wording, consistently across repeats,
//!    while an id the domain never held is still an existence answer (404).
//! 4. A delete scoped to a domain this store serves no record of is denied before any
//!    resource is resolved.
//!
//! The harness (`Emulator`, `Answer`) is the one every suite in this crate drives the
//! service with.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Value};

/// One object of a domain's ownership hierarchy as a deletion refusal names it:
/// `<kind> <identifier>`.
///
/// SOURCE: the recorded `ConflictException` for `DeleteDomainUnit`, which names the unit
/// being deleted and the child unit that prevented it `domainUnit <domainUnitId>`.
fn hierarchy_object(kind: &str, identifier: &str) -> String {
    format!("{kind} {identifier}")
}

/// The `ConflictException` message a delete a stored dependency prevents is refused
/// with: recorded wording, built from the objects the call found, ending in a period and
/// a space.
fn deletion_prevented_message(object: &str, blocker: &str) -> String {
    format!("The following objects could not be deleted: {object} deletion prevented by {blocker}. ")
}

/// The `AccessDeniedException` message a call addressing a domain unit the service has
/// deleted answers: recorded wording, whose only substitutable part is the operation
/// name of the request.
fn not_permitted_or_absent(operation: &str) -> String {
    format!(
        "User is not permitted to perform operation: {operation}, \
         or the requested resource does not exist"
    )
}

/// The `AccessDeniedException` message a call scoped to a domain the store serves no
/// record of answers.
fn not_permitted(operation: &str) -> String {
    format!("User is not permitted to perform operation: {operation}")
}

/// Create a domain unit under `parent`, asserting the create itself succeeded.
async fn create_unit(dz: &Emulator, domain: &str, parent: &str, name: &str, case: &str) -> String {
    let answer = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({ "name": name, "parentDomainUnitIdentifier": parent })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected 201 creating {name:?} under {parent:?}, got {} {:?}",
        answer.status, answer.body
    );
    answer.s("id")
}

/// The ids `ListDomainUnitsForParent` reports for `parent`.
async fn children_of(dz: &Emulator, domain: &str, parent: &str, case: &str) -> Vec<String> {
    let listed = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/domain-units?parentDomainUnitIdentifier={parent}"),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 listing the children of {parent:?}, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("<absent>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One unit to delete when nothing hangs under it. The rows differ along what the rule
/// generalises over: the form and length of the unit's name, how deep under the root
/// unit it hangs, and whether it carries a description.
struct EmptyUnitCase {
    label: &'static str,
    name: String,
    description: Option<String>,
    /// 0 = created directly under the domain's root unit; 2 = two intermediate units
    /// are created first, so the deleted unit is a leaf two levels down.
    depth_under_root: usize,
}

fn empty_unit_cases() -> Vec<EmptyUnitCase> {
    vec![
        EmptyUnitCase {
            label: "a plain name directly under the root unit",
            name: "sales".to_string(),
            description: None,
            depth_under_root: 0,
        },
        EmptyUnitCase {
            label: "a name with spaces and digits one level down",
            name: "Sales Unit 42".to_string(),
            description: Some("the sales organisation".to_string()),
            depth_under_root: 1,
        },
        EmptyUnitCase {
            // `DomainUnitName` is len 1..128 (smithy_query shape="DomainUnitName"):
            // both bounds, exactly.
            label: "a name at the modelled 128-character maximum two levels down",
            name: "n".repeat(128),
            description: Some("d".repeat(2048)),
            depth_under_root: 2,
        },
        EmptyUnitCase {
            label: "a single-character name, the modelled minimum",
            name: "a".to_string(),
            description: Some(String::new()),
            depth_under_root: 0,
        },
    ]
}

#[tokio::test]
async fn deleting_a_domain_unit_nothing_hangs_under_removes_it() {
    let dz = Emulator::new();
    let domain = dz.available_domain("unit-deletion-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    for (row, case) in empty_unit_cases().into_iter().enumerate() {
        // Build the chain this case hangs under. The row number is part of every
        // intermediate name, because a name is unique among ONE parent's children.
        let mut parent = root.clone();
        for step in 0..case.depth_under_root {
            parent = create_unit(
                &dz,
                &domain,
                &parent,
                &format!("ancestor {row} {step}"),
                case.label,
            )
            .await;
        }

        let mut body = json!({ "name": case.name, "parentDomainUnitIdentifier": parent });
        if let Some(description) = &case.description {
            body["description"] = json!(description);
        }
        let created = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/domain-units"),
                Some(body),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201, got {} {:?}",
            case.label, created.status, created.body
        );
        let id = created.s("id");
        assert!(
            children_of(&dz, &domain, &parent, case.label)
                .await
                .contains(&id),
            "case {:?}: the created unit must be listed under {parent:?} before the delete",
            case.label
        );

        let deleted = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/domain-units/{id}"),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected the modelled 204, got {} {:?}",
            case.label, deleted.status, deleted.body
        );
        assert!(
            deleted.body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {:?}: the modelled output has no members, got {:?}",
            case.label,
            deleted.body
        );

        // The delete is observable: the unit is gone from its parent's listing.
        let remaining = children_of(&dz, &domain, &parent, case.label).await;
        assert!(
            !remaining.contains(&id),
            "case {:?}: the deleted unit must not be listed under {parent:?} any more, got {remaining:?}",
            case.label
        );

        // And the name it held is free again under the same parent: the record really
        // went, rather than being hidden from the listing.
        let reused = create_unit(&dz, &domain, &parent, &case.name, case.label).await;
        assert_ne!(
            reused, id,
            "case {:?}: re-creating the name must mint a new unit, got the deleted id",
            case.label
        );
    }
}

/// One object that hangs under the unit being deleted, so the delete is refused. The
/// rows differ along the KIND of the dependency and how many of them there are.
struct BlockedCase {
    label: &'static str,
    /// How many child domain units hang under the unit.
    children: usize,
    /// How many projects name the unit as their `domainUnitId`.
    projects: usize,
}

fn blocked_cases() -> Vec<BlockedCase> {
    vec![
        BlockedCase {
            label: "one child domain unit",
            children: 1,
            projects: 0,
        },
        BlockedCase {
            label: "three child domain units",
            children: 3,
            projects: 0,
        },
        BlockedCase {
            label: "one project assigned to the unit",
            children: 0,
            projects: 1,
        },
        BlockedCase {
            label: "a child domain unit and a project",
            children: 1,
            projects: 1,
        },
    ]
}

#[tokio::test]
async fn a_domain_unit_something_still_hangs_under_is_refused_and_stays_readable() {
    let dz = Emulator::new();
    let domain = dz.available_domain("unit-blocker-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    for (row, case) in blocked_cases().into_iter().enumerate() {
        let parent = create_unit(&dz, &domain, &root, &format!("parent {row}"), case.label).await;

        let mut child_ids: Vec<String> = Vec::new();
        for index in 0..case.children {
            child_ids.push(
                create_unit(
                    &dz,
                    &domain,
                    &parent,
                    &format!("child {row} {index}"),
                    case.label,
                )
                .await,
            );
        }
        let mut project_ids: Vec<String> = Vec::new();
        for index in 0..case.projects {
            let created = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/projects"),
                    Some(json!({
                        "name": format!("project {row} {index}"),
                        "domainUnitId": parent,
                    })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {:?}: expected 201 creating a project in the unit, got {} {:?}",
                case.label, created.status, created.body
            );
            project_ids.push(created.s("id"));
        }

        let refused = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/domain-units/{parent}"),
                None,
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {:?}: expected the modelled conflict, got {} {:?}",
            case.label,
            refused.status,
            refused.body
        );
        // Child units are reported before projects, and each walk is over a map keyed
        // by id, so the object named is the first blocker in that order.
        let expected_blocker = child_ids
            .iter()
            .min()
            .map(|id| hierarchy_object("domainUnit", id))
            .or_else(|| {
                project_ids
                    .iter()
                    .min()
                    .map(|id| hierarchy_object("project", id))
            })
            .expect("every blocked case stores at least one dependency");
        assert_eq!(
            refused.s("message"),
            deletion_prevented_message(&hierarchy_object("domainUnit", &parent), &expected_blocker),
            "case {:?}: unexpected refusal message {:?}",
            case.label,
            refused.s("message")
        );

        // The refusal stored nothing: the unit is still readable, unchanged.
        let still_there = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/domain-units/{parent}"),
                None,
            )
            .await;
        assert_eq!(
            (still_there.status, still_there.s("id").as_str()),
            (200, parent.as_str()),
            "case {:?}: the refused unit must stay readable, got {} {:?}",
            case.label,
            still_there.status,
            still_there.body
        );

        // Bottom-up teardown: once every dependency is gone, the same delete succeeds.
        for child in &child_ids {
            let gone = dz
                .call(
                    "DELETE",
                    &format!("/v2/domains/{domain}/domain-units/{child}"),
                    None,
                )
                .await;
            assert_eq!(
                gone.status, 204,
                "case {:?}: expected 204 deleting the blocking child {child:?}, got {} {:?}",
                case.label, gone.status, gone.body
            );
        }
        for project in &project_ids {
            let gone = dz
                .call(
                    "DELETE",
                    &format!("/v2/domains/{domain}/projects/{project}"),
                    None,
                )
                .await;
            assert_eq!(
                gone.status, 204,
                "case {:?}: expected 204 deleting the blocking project {project:?}, got {} {:?}",
                case.label, gone.status, gone.body
            );
        }
        let now_deleted = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/domain-units/{parent}"),
                None,
            )
            .await;
        assert_eq!(
            now_deleted.status, 204,
            "case {:?}: expected 204 once nothing hangs under the unit, got {} {:?}",
            case.label, now_deleted.status, now_deleted.body
        );
    }
}

#[tokio::test]
async fn the_domain_holds_its_root_unit_so_that_one_cannot_be_deleted() {
    let dz = Emulator::new();

    // The rows differ along what else the domain holds: the rule is about the domain
    // naming the unit in `rootDomainUnitId`, not about what hangs under it.
    for (label, children) in [("an empty root unit", 0usize), ("a root unit with children", 2)] {
        let domain = dz.available_domain(&format!("root-unit-{children}")).await;
        let root = dz
            .call("GET", &format!("/v2/domains/{domain}"), None)
            .await
            .s("rootDomainUnitId");
        for index in 0..children {
            create_unit(&dz, &domain, &root, &format!("child {index}"), label).await;
        }

        let refused = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/domain-units/{root}"),
                None,
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (409, "ConflictException"),
            "case {label:?}: expected the modelled conflict, got {} {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.s("message"),
            deletion_prevented_message(
                &hierarchy_object("domainUnit", &root),
                &hierarchy_object("domain", &domain)
            ),
            "case {label:?}: unexpected refusal message {:?}",
            refused.s("message")
        );
        let still_there = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/domain-units/{root}"),
                None,
            )
            .await;
        assert_eq!(
            (still_there.status, still_there.s("id").as_str()),
            (200, root.as_str()),
            "case {label:?}: the root unit must stay readable, got {} {:?}",
            still_there.status,
            still_there.body
        );
    }
}

#[tokio::test]
async fn a_deleted_domain_unit_is_denied_to_every_later_call() {
    let dz = Emulator::new();
    let domain = dz.available_domain("deleted-unit-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    // The rows differ along how the unit came to be gone: deleted as a leaf, deleted
    // after its own child was deleted first, and deleted two levels down.
    let leaf = create_unit(&dz, &domain, &root, "leaf", "a leaf").await;
    let parent = create_unit(&dz, &domain, &root, "parent", "a former parent").await;
    let child = create_unit(&dz, &domain, &parent, "child", "a former parent").await;
    let deep_parent = create_unit(&dz, &domain, &root, "deep parent", "two levels down").await;
    let deep = create_unit(&dz, &domain, &deep_parent, "deep", "two levels down").await;

    for (label, id) in [
        ("a leaf", leaf.clone()),
        ("a former parent's child", child.clone()),
        ("a former parent", parent.clone()),
        ("a unit two levels down", deep.clone()),
    ] {
        let deleted = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/domain-units/{id}"),
                None,
            )
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {label:?}: expected 204 deleting {id:?}, got {} {:?}",
            deleted.status, deleted.body
        );

        // REPEATED reads answer the same denial every time: the id names nothing the
        // caller holds anything on, and nothing resurrects it.
        for read in 1..=3 {
            let answer = dz
                .call(
                    "GET",
                    &format!("/v2/domains/{domain}/domain-units/{id}"),
                    None,
                )
                .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (403, "AccessDeniedException"),
                "case {label:?}: read {read} of a deleted unit must be denied, got {} {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.s("message"),
                not_permitted_or_absent("GetDomainUnit"),
                "case {label:?}: read {read} carried an unexpected message {:?}",
                answer.s("message")
            );
            assert!(
                !answer.s("message").contains(&id),
                "case {label:?}: the denial must not echo the requested id, got {:?}",
                answer.s("message")
            );
        }

        // And a repeat of the delete itself is denied the same way, naming its own
        // operation.
        let again = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/domain-units/{id}"),
                None,
            )
            .await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {label:?}: a repeated delete must be denied, got {} {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            again.s("message"),
            not_permitted_or_absent("DeleteDomainUnit"),
            "case {label:?}: the repeated delete carried an unexpected message {:?}",
            again.s("message")
        );
    }

    // The control: an id this domain never held is an EXISTENCE answer, not a denial —
    // only an id the service itself removed is one nothing is held on any more.
    for (label, unknown) in [
        ("a short unknown id", "nosuchunit".to_string()),
        // `DomainUnitId` is 1..256 of `^[a-z0-9_\-]+$` (smithy_query
        // shape="DomainUnitId"): the shortest and the longest id the pattern admits.
        ("the shortest id the pattern admits", "u".to_string()),
        ("the longest id the pattern admits", "u".repeat(256)),
        ("digits, hyphens and underscores", "0_unit-9".to_string()),
    ] {
        for method in ["GET", "DELETE"] {
            let answer = dz
                .call(
                    method,
                    &format!("/v2/domains/{domain}/domain-units/{unknown}"),
                    None,
                )
                .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {label:?} via {method}: expected the modelled not-found, got {} {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.s("message"),
                "Domain unit not found",
                "case {label:?} via {method}: unexpected message {:?}",
                answer.s("message")
            );
        }
    }
}

#[tokio::test]
async fn a_delete_scoped_to_a_domain_the_store_does_not_serve_is_denied() {
    let dz = Emulator::new();
    let domain = dz.available_domain("delete-scope-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");
    let unit = create_unit(&dz, &domain, &root, "scoped", "the fixture").await;

    // Domain identifiers no domain in the store carries, each satisfying `DomainId`'s
    // modelled pattern `^dzd[-_][a-zA-Z0-9_-]{1,36}$` (smithy_query shape="DomainId") so
    // that nothing refuses them before the handler runs. The rows differ along which
    // separator and the shortest and longest tail the pattern admits.
    for (label, unserved) in [
        ("an underscore separator", "dzd_nosuchdomain000".to_string()),
        ("a hyphen separator", "dzd-nosuchdomain001".to_string()),
        ("the shortest tail the pattern admits", "dzd_a".to_string()),
        (
            "the longest tail the pattern admits",
            format!("dzd_{}", "d".repeat(36)),
        ),
    ] {
        // The unit identifier is the one the fixture really stores, so the only thing
        // wrong with the request is the domain it is scoped to.
        let answer = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{unserved}/domain-units/{unit}"),
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (403, "AccessDeniedException"),
            "case {label:?}: expected the modelled denial, got {} {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.s("message"),
            not_permitted("DeleteDomainUnit"),
            "case {label:?}: the denial must carry the recorded message, got {:?}",
            answer.s("message")
        );
    }

    // The unit the denied calls addressed is untouched.
    let intact = dz
        .call(
            "GET",
            &format!("/v2/domains/{domain}/domain-units/{unit}"),
            None,
        )
        .await;
    assert_eq!(
        (intact.status, intact.s("id").as_str()),
        (200, unit.as_str()),
        "a denied delete must store nothing, got {} {:?}",
        intact.status,
        intact.body
    );

    // A domain on its way out holds nothing to delete inside it: deleted, then used on
    // the very next call, before the deletion the clock owes has settled.
    let doomed = dz.available_domain("on-its-way-out").await;
    let doomed_root = dz
        .call("GET", &format!("/v2/domains/{doomed}"), None)
        .await
        .s("rootDomainUnitId");
    let doomed_unit = create_unit(&dz, &doomed, &doomed_root, "doomed", "a doomed domain").await;
    let started = dz.call("DELETE", &format!("/v2/domains/{doomed}"), None).await;
    assert_eq!(
        started.status, 202,
        "deleting {doomed:?}: expected 202, got {} {:?}",
        started.status, started.body
    );
    let too_late = dz
        .call(
            "DELETE",
            &format!("/v2/domains/{doomed}/domain-units/{doomed_unit}"),
            None,
        )
        .await;
    assert_eq!(
        (too_late.status, too_late.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a domain that is being deleted\": expected the modelled not-found, got {} {:?}",
        too_late.status,
        too_late.body
    );
    assert_eq!(
        too_late.s("message"),
        "Domain not found",
        "case \"a domain that is being deleted\": unexpected message {:?}",
        too_late.s("message")
    );

    // Once that deletion has completed the identifier names no domain at all, so the
    // same call is decided by authorization instead.
    tokio::time::sleep(SETTLED).await;
    let gone = dz
        .call(
            "DELETE",
            &format!("/v2/domains/{doomed}/domain-units/{doomed_unit}"),
            None,
        )
        .await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (403, "AccessDeniedException"),
        "case \"a domain that is gone\": expected the modelled denial, got {} {:?}",
        gone.status,
        gone.body
    );
}
