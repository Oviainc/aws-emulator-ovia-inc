// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the DataZone domain-unit hierarchy.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here. The
//! harness (`Emulator`, `Answer`) is the one the domain/project suite already defines.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateDomainUnit` persists the whole record under the parent the request names,
//!    and a later `GetDomainUnit` reports what was stored.
//! 2. `ancestorDomainUnitIds` is the chain of parents of the created unit, nearest
//!    first and the domain's root unit last — so its length is the unit's depth.
//! 3. A domain's root unit exists from the moment the domain does: it is readable at
//!    `rootDomainUnitId`, carries the domain's name and has no parent.
//! 4. A repeated `clientToken` answers with the unit the first request created.
//! 5. A request naming a domain or a parent that is not stored, or a domain that is
//!    not `AVAILABLE`, is refused with the modelled error for that kind of failure.

use crate::domain_project_lifecycle::is_domain_unit_id;
use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};

/// The members of a response body that is a list of domain unit ids.
fn id_list(answer: &Answer, member: &str) -> Vec<String> {
    answer
        .body
        .get(member)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or("<not a string>").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `userId` of every `owners` entry that is a `user`, in order
/// (smithy_query shape="DomainUnitOwnerProperties": `user` | `group`).
fn owner_user_ids(answer: &Answer) -> Vec<String> {
    answer
        .body
        .get("owners")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|owner| {
                    owner
                        .get("user")
                        .and_then(|user| user.get("userId"))
                        .and_then(Value::as_str)
                        .unwrap_or("<absent>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One `CreateDomainUnit` request. The rows differ along what the rule generalises
/// over: the form and length of the name, whether a description is present and how
/// long it is, and how deep under the root unit the new unit hangs.
struct UnitCase {
    label: &'static str,
    name: String,
    description: Option<String>,
    /// 0 = created directly under the domain's root unit, so `ancestorDomainUnitIds`
    /// is `[root]`; 3 = three intermediate units are created first.
    depth_under_root: usize,
}

fn unit_cases() -> Vec<UnitCase> {
    vec![
        UnitCase {
            label: "name only, directly under the root unit",
            name: "sales".to_string(),
            description: None,
            depth_under_root: 0,
        },
        UnitCase {
            label: "name with spaces and digits, one level down",
            name: "Sales Unit 42".to_string(),
            description: Some("the sales organisation".to_string()),
            depth_under_root: 1,
        },
        UnitCase {
            label: "name of word characters, hyphens and underscores, two levels down",
            name: "eu-west_sub 1".to_string(),
            description: Some(String::new()),
            depth_under_root: 2,
        },
        UnitCase {
            // `DomainUnitName` is len 1..128 and `DomainUnitDescription` len ..2048
            // (smithy_query operation="CreateDomainUnit"): both bounds, exactly.
            label: "name at the modelled 128-character maximum, three levels down",
            name: "n".repeat(128),
            description: Some("d".repeat(2048)),
            depth_under_root: 3,
        },
        UnitCase {
            label: "single-character name, the modelled minimum",
            name: "a".to_string(),
            description: Some("short".to_string()),
            depth_under_root: 0,
        },
    ]
}

/// Create a domain unit, asserting the create itself succeeded.
async fn create_unit(dz: &Emulator, domain: &str, parent: &str, name: &str, label: &str) -> String {
    let answer = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({ "name": name, "parentDomainUnitIdentifier": parent })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {label:?}: expected 201 creating {name:?} under {parent:?}, got {} {:?}",
        answer.status, answer.body
    );
    answer.s("id")
}

#[tokio::test]
async fn created_domain_units_are_persisted_and_reported_by_get() {
    let dz = Emulator::new();
    let domain = dz.available_domain("unit-hierarchy-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    let mut minted: Vec<(&'static str, String)> = Vec::new();

    for (row, case) in unit_cases().into_iter().enumerate() {
        // Build the chain this case hangs under: `depth_under_root` intermediate units,
        // each the child of the one before. The row number is part of every
        // intermediate name, because a name is unique among ONE parent's children and
        // the first step of every chain hangs under the same root unit.
        let mut chain: Vec<String> = vec![root.clone()];
        for step in 0..case.depth_under_root {
            let parent = chain.last().expect("the chain always holds the root").clone();
            let name = format!("ancestor {row} {step}");
            chain.push(create_unit(&dz, &domain, &parent, &name, case.label).await);
        }
        let parent = chain.last().expect("the chain always holds the root").clone();

        let mut body = json!({
            "name": case.name,
            "parentDomainUnitIdentifier": parent,
        });
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
            is_domain_unit_id(&id),
            "case {:?}: expected an id matching ^[a-z0-9_-]+$ of length 1..256, got {id:?}",
            case.label
        );
        assert_eq!(
            created.s("domainId"),
            domain,
            "case {:?}: the unit must report the domain it was created in, got {:?}",
            case.label,
            created.s("domainId")
        );
        assert_eq!(
            created.s("name"),
            case.name,
            "case {:?}: the stored name must come back unchanged, got {:?}",
            case.label,
            created.s("name")
        );
        assert_eq!(
            created.s("parentDomainUnitId"),
            parent,
            "case {:?}: expected the parent the request named, got {:?}",
            case.label,
            created.s("parentDomainUnitId")
        );
        match &case.description {
            Some(description) => assert_eq!(
                created.s("description"),
                *description,
                "case {:?}: the stored description must come back unchanged, got {:?}",
                case.label,
                created.s("description")
            ),
            None => assert!(
                created.body.get("description").is_none(),
                "case {:?}: a unit created without a description must report none, got {:?}",
                case.label,
                created.body.get("description")
            ),
        }

        // RULE 2: the ancestors are the chain of parents, nearest first, root last.
        let expected_ancestors: Vec<String> = chain.iter().rev().cloned().collect();
        assert_eq!(
            id_list(&created, "ancestorDomainUnitIds"),
            expected_ancestors,
            "case {:?}: expected the parent chain nearest-first, got {:?}",
            case.label,
            id_list(&created, "ancestorDomainUnitIds")
        );

        // The creator owns what it created, as one `user` owner.
        assert_eq!(
            owner_user_ids(&created),
            vec!["arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY".to_string()],
            "case {:?}: expected the calling principal as the only owner, got {:?}",
            case.label,
            created.body.get("owners")
        );
        assert!(
            created.body.get("createdAt").is_some(),
            "case {:?}: expected a createdAt, got {:?}",
            case.label,
            created.body
        );
        assert_eq!(
            created.s("createdBy"),
            "arn:aws:iam::000000000000:user/AKIAEXAMPLETESTKEY",
            "case {:?}: expected the calling principal as createdBy, got {:?}",
            case.label,
            created.s("createdBy")
        );

        // RULE 1: the read reports the stored record, member for member.
        let got = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/domain-units/{id}"),
                None,
            )
            .await;
        assert_eq!(
            got.status, 200,
            "case {:?}: expected 200 from GetDomainUnit, got {} {:?}",
            case.label, got.status, got.body
        );
        for member in ["id", "domainId", "name", "parentDomainUnitId", "createdBy"] {
            assert_eq!(
                got.s(member),
                created.s(member),
                "case {:?}: GetDomainUnit must report the stored {member}, got {:?}",
                case.label,
                got.s(member)
            );
        }
        assert_eq!(
            owner_user_ids(&got),
            owner_user_ids(&created),
            "case {:?}: GetDomainUnit must report the stored owners, got {:?}",
            case.label,
            got.body.get("owners")
        );
        assert!(
            got.body.get("lastUpdatedAt").is_none() && got.body.get("lastUpdatedBy").is_none(),
            "case {:?}: nothing updated the unit, so neither lastUpdated member is reported, got {:?}",
            case.label,
            got.body
        );

        minted.push((case.label, id));
    }

    // Distinct creates never share an id.
    for (index, (label_a, id_a)) in minted.iter().enumerate() {
        for (label_b, id_b) in minted.iter().skip(index + 1) {
            assert_ne!(
                id_a, id_b,
                "cases {label_a:?} and {label_b:?}: two domain units were given the same id {id_a:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_domains_root_unit_exists_from_the_moment_the_domain_does() {
    let dz = Emulator::new();
    // `DomainName` declares no pattern while `DomainUnitName` is `^[\w -]+$`
    // (smithy_query shape="DomainUnitName"), so the root unit's name is the domain's
    // name with every character outside that class replaced.
    let cases = [
        ("plain name", "root unit domain", "root unit domain"),
        ("dots and a bang", "my.domain!", "my_domain_"),
        ("word characters only", "A_b-9", "A_b-9"),
    ];

    for (label, domain_name, expected_unit_name) in cases {
        let domain = dz.available_domain(domain_name).await;
        let described = dz.call("GET", &format!("/v2/domains/{domain}"), None).await;
        let root = described.s("rootDomainUnitId");
        assert!(
            is_domain_unit_id(&root),
            "case {label:?}: expected a rootDomainUnitId matching ^[a-z0-9_-]+$, got {root:?}"
        );

        let got = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/domain-units/{root}"),
                None,
            )
            .await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: the root unit must be readable, got {} {:?}",
            got.status, got.body
        );
        assert_eq!(
            got.s("id"),
            root,
            "case {label:?}: expected the root unit's own id, got {:?}",
            got.s("id")
        );
        assert_eq!(
            got.s("name"),
            expected_unit_name,
            "case {label:?}: unexpected root unit name {:?}",
            got.s("name")
        );
        assert!(
            got.body.get("parentDomainUnitId").is_none(),
            "case {label:?}: the root unit has no parent, got {:?}",
            got.body.get("parentDomainUnitId")
        );

        // A unit created directly under it has exactly the root as its ancestor.
        let child = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/domain-units"),
                Some(json!({ "name": "child", "parentDomainUnitIdentifier": root })),
            )
            .await;
        assert_eq!(
            id_list(&child, "ancestorDomainUnitIds"),
            vec![root.clone()],
            "case {label:?}: expected the root as the only ancestor, got {:?}",
            id_list(&child, "ancestorDomainUnitIds")
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_returns_the_first_domain_unit() {
    let dz = Emulator::new();
    let domain = dz.available_domain("idempotent-unit-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    // The rows differ along what the token has to survive: a bare repeat, a repeat
    // with a different name, and a distinct token that must NOT replay.
    let first = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({
                "name": "tokened",
                "parentDomainUnitIdentifier": root,
                "clientToken": "token-one",
            })),
        )
        .await;
    let first_id = first.s("id");

    for (label, body) in [
        (
            "same token, same request",
            json!({ "name": "tokened", "parentDomainUnitIdentifier": root, "clientToken": "token-one" }),
        ),
        (
            "same token, different name",
            json!({ "name": "renamed", "parentDomainUnitIdentifier": root, "clientToken": "token-one" }),
        ),
    ] {
        let replay = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/domain-units"),
                Some(body),
            )
            .await;
        assert_eq!(
            replay.s("id"),
            first_id,
            "case {label:?}: a repeated clientToken must answer with the first unit, got {:?}",
            replay.s("id")
        );
        assert_eq!(
            replay.s("name"),
            "tokened",
            "case {label:?}: the replay reports the STORED unit, not the repeated request, got {:?}",
            replay.s("name")
        );
    }

    // A fresh token creates a second unit — under its own name, since the name of the
    // first one is taken among the root unit's children.
    let other = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({
                "name": "tokened twice",
                "parentDomainUnitIdentifier": root,
                "clientToken": "token-two",
            })),
        )
        .await;
    assert_ne!(
        other.s("id"),
        first_id,
        "case \"a different token\": a fresh token must create a second unit, got {:?}",
        other.s("id")
    );
}

#[tokio::test]
async fn a_domain_unit_needs_a_stored_domain_an_available_domain_and_a_stored_parent() {
    let dz = Emulator::new();
    let domain = dz.available_domain("refusal-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    // The two rows that do not depend on any domain's status run first, so the two
    // that DO are asserted immediately after the call that puts their domain in that
    // status — the emulator leaves a domain CREATING or DELETING only until the clock
    // settles it, so nothing slow may run in between.
    let cases = [
        (
            "unknown domain",
            "/v2/domains/dzd_nosuchdomain000/domain-units".to_string(),
            json!({ "name": "orphan", "parentDomainUnitIdentifier": root }),
            400u16,
            "ValidationException",
        ),
        (
            "unknown parent in a stored domain",
            format!("/v2/domains/{domain}/domain-units"),
            json!({ "name": "orphan", "parentDomainUnitIdentifier": "nosuchparent" }),
            400,
            "ValidationException",
        ),
    ];

    for (label, uri, body, status, code) in cases {
        let answer = dz.call("POST", &uri, Some(body)).await;
        assert_eq!(
            answer.status, status,
            "case {label:?}: expected {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, code,
            "case {label:?}: expected the {code} error code, got {:?}",
            answer.error_code
        );
    }

    // A domain on its way out: deleted, and used on the very next call, before the
    // deletion the clock owes has settled, so it is still stored and reports DELETING.
    let doomed_domain = dz.available_domain("on-its-way-out").await;
    let doomed_root = dz
        .call("GET", &format!("/v2/domains/{doomed_domain}"), None)
        .await
        .s("rootDomainUnitId");
    let deleted = dz
        .call("DELETE", &format!("/v2/domains/{doomed_domain}"), None)
        .await;
    assert_eq!(
        deleted.status, 202,
        "deleting {doomed_domain:?}: expected 202, got {} {:?}",
        deleted.status, deleted.body
    );
    let too_late = dz
        .call(
            "POST",
            &format!("/v2/domains/{doomed_domain}/domain-units"),
            Some(json!({ "name": "too late", "parentDomainUnitIdentifier": doomed_root })),
        )
        .await;
    assert_eq!(
        too_late.status, 409,
        "case \"a domain that is being deleted\": expected 409, got {} {:?}",
        too_late.status, too_late.body
    );
    assert_eq!(
        too_late.error_code, "ConflictException",
        "case \"a domain that is being deleted\": expected ConflictException, got {:?}",
        too_late.error_code
    );

    // And one the clock has not settled yet: created, then used on the very next call.
    let creating = dz
        .call("POST", "/v2/domains", Some(json!({ "name": "still-creating" })))
        .await;
    let too_early = dz
        .call(
            "POST",
            &format!("/v2/domains/{}/domain-units", creating.s("id")),
            Some(json!({
                "name": "too early",
                "parentDomainUnitIdentifier": creating.s("rootDomainUnitId"),
            })),
        )
        .await;
    assert_eq!(
        too_early.status, 409,
        "case \"a domain that has not finished being created\": expected 409, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code, "ConflictException",
        "case \"a domain that has not finished being created\": expected ConflictException, got {:?}",
        too_early.error_code
    );

    // Reading a unit a stored domain does not hold is a not-found; reading a stored
    // unit through a domain this store has no record of is decided by authorization
    // first, before the unit is looked for at all.
    for (label, uri, status, code) in [
        (
            "unknown unit in a stored domain",
            format!("/v2/domains/{domain}/domain-units/nosuchunit"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stored unit addressed through an unknown domain",
            format!("/v2/domains/dzd_nosuchdomain000/domain-units/{root}"),
            403,
            "AccessDeniedException",
        ),
    ] {
        let answer = dz.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, status,
            "case {label:?}: expected {status}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, code,
            "case {label:?}: expected {code}, got {:?}",
            answer.error_code
        );
    }
}

#[tokio::test]
async fn the_domain_unit_store_is_bounded_and_refuses_with_the_modelled_quota_error() {
    let dz = Emulator::new();
    let domain = dz.available_domain("bounded-unit-domain").await;
    let root = dz
        .call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId");

    // The service's own ceiling, so this suite tests the boundary the handler
    // enforces; the root unit the domain was created with is the first unit of it.
    let ceiling = service_com_amazonaws_datazone_datazone::state::MAX_DOMAIN_UNITS_PER_DOMAIN;
    for index in 1..ceiling {
        create_unit(
            &dz,
            &domain,
            &root,
            &format!("unit {index}"),
            "filling the store",
        )
        .await;
    }

    let refused = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/domain-units"),
            Some(json!({ "name": "one too many", "parentDomainUnitIdentifier": root })),
        )
        .await;
    assert_eq!(
        refused.status, 402,
        "case \"one past the ceiling\": expected 402, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code, "ServiceQuotaExceededException",
        "case \"one past the ceiling\": expected ServiceQuotaExceededException, got {:?}",
        refused.error_code
    );
}
