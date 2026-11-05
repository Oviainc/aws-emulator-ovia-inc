// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `AddEntityOwner`, `ListEntityOwners` and
//! `RemoveEntityOwner`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail here. The
//! harness (`Emulator`, `Answer`) is the one the other suites already define.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `ListEntityOwners` reports exactly the owners the named entity currently holds —
//!    the owner its create recorded, plus every owner `AddEntityOwner` added and none
//!    `RemoveEntityOwner` took away — each rendered as the `OwnerPropertiesOutput`
//!    member its kind names (`user` carries `userId`, `group` carries `groupId`), and
//!    the same set `GetDomainUnit` reports for that entity.
//! 2. An entity's owners are a SET: the model marks both writes `@idempotent`, so
//!    adding an owner the entity already has, or removing one it does not have, leaves
//!    the entity exactly as it was and still succeeds. A user and a group that share an
//!    identifier are two different owners.
//! 3. A page holds at most `maxResults` owners in one stable order, and following
//!    `nextToken` walks the owners exactly once: the pages concatenated are the whole
//!    owner set, with no duplicate and nothing missing, for every page size in the
//!    modelled range and for a request naming none.
//! 4. A reporting call changes nothing a later caller sees, and a refused call writes
//!    nothing: an `entityType` the model does not declare, an unknown domain or entity,
//!    a `nextToken` this service never issued and a `maxResults` outside the modelled
//!    range are refused — as a 400 `ValidationException` by the listing, which models
//!    no `ResourceNotFoundException`, and as a 404 by the two writes, which do.

use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};

/// One owner, as the two owner unions both spell it: the member (`user` or `group`) and
/// the identifier it carries (smithy_query shape="OwnerProperties,OwnerPropertiesOutput").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Owner {
    kind: &'static str,
    id: String,
}

impl Owner {
    fn user(id: &str) -> Self {
        Self {
            kind: "user",
            id: id.to_string(),
        }
    }

    fn group(id: &str) -> Self {
        Self {
            kind: "group",
            id: id.to_string(),
        }
    }

    /// The request body `AddEntityOwner` and `RemoveEntityOwner` take for this owner:
    /// the `OwnerProperties` union, whose `user` member carries `userIdentifier` and
    /// whose `group` member carries `groupIdentifier`.
    fn body(&self) -> Value {
        if self.kind == "user" {
            json!({ "owner": { "user": { "userIdentifier": self.id } } })
        } else {
            json!({ "owner": { "group": { "groupIdentifier": self.id } } })
        }
    }
}

/// Every owner in an `owners` list, in the order it was reported. Reads both the
/// `OwnerPropertiesOutput` union `ListEntityOwners` reports and the
/// `DomainUnitOwnerProperties` union `GetDomainUnit` reports: `user`/`userId` and
/// `group`/`groupId` in each.
fn owners_of(body: &Value) -> Vec<Owner> {
    body.get("owners")
        .and_then(Value::as_array)
        .map(|owners| {
            owners
                .iter()
                .map(|owner| {
                    let read = |member: &str, id_member: &str| {
                        owner
                            .get(member)
                            .and_then(|properties| properties.get(id_member))
                            .and_then(Value::as_str)
                    };
                    if let Some(id) = read("user", "userId") {
                        Owner::user(id)
                    } else if let Some(id) = read("group", "groupId") {
                        Owner::group(id)
                    } else {
                        Owner {
                            kind: "unrecognised",
                            id: owner.to_string(),
                        }
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The domain's root domain unit, the entity every case hangs its own unit under.
async fn root_unit(dz: &Emulator, domain: &str) -> String {
    dz.call("GET", &format!("/v2/domains/{domain}"), None)
        .await
        .s("rootDomainUnitId")
}

/// Create a domain unit under `parent` and answer its id.
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
        "case {label:?}: expected 201 creating domain unit {name:?}, got {} {:?}",
        answer.status, answer.body
    );
    answer.s("id")
}

/// One `ListEntityOwners` call, with `query` appended to the URI.
async fn list(dz: &Emulator, domain: &str, entity_type: &str, entity: &str, query: &str) -> Answer {
    dz.call(
        "GET",
        &format!("/v2/domains/{domain}/entities/{entity_type}/{entity}/owners{query}"),
        None,
    )
    .await
}

/// One `AddEntityOwner` call.
async fn add(dz: &Emulator, domain: &str, entity_type: &str, entity: &str, o: &Owner) -> Answer {
    dz.call(
        "POST",
        &format!("/v2/domains/{domain}/entities/{entity_type}/{entity}/addOwner"),
        Some(o.body()),
    )
    .await
}

/// One `RemoveEntityOwner` call.
async fn remove(dz: &Emulator, domain: &str, entity_type: &str, entity: &str, o: &Owner) -> Answer {
    dz.call(
        "POST",
        &format!("/v2/domains/{domain}/entities/{entity_type}/{entity}/removeOwner"),
        Some(o.body()),
    )
    .await
}

/// `GetDomainUnit`, the reader a write is proved against.
async fn get_unit(dz: &Emulator, domain: &str, unit: &str) -> Answer {
    dz.call(
        "GET",
        &format!("/v2/domains/{domain}/domain-units/{unit}"),
        None,
    )
    .await
}

/// The one `DataZoneEntityType` member (smithy_query shape="DataZoneEntityType":
/// DOMAIN_UNIT), so an entity owners are kept on is a domain unit.
const DOMAIN_UNIT: &str = "DOMAIN_UNIT";

/// One owner set to put on an entity of its own. The rows differ along what the rule
/// generalises over: the owner kind, how many owners there are, and the form of the
/// identifier — every alternative of `UserIdentifier`'s pattern
/// `(^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-…$|^[a-zA-Z_0-9+=,.@-]+$|^arn:aws:iam::\d{12}:.+$)`
/// and of `GroupIdentifier`'s (smithy_query shape="UserIdentifier,GroupIdentifier").
struct OwnerCase {
    label: &'static str,
    owners: Vec<Owner>,
}

fn owner_cases() -> Vec<OwnerCase> {
    vec![
        OwnerCase {
            label: "no owner added: the entity keeps the one its create recorded",
            owners: Vec::new(),
        },
        OwnerCase {
            label: "one user, named the way an SSO identifier is",
            owners: vec![Owner::user("11111111-2222-3333-4444-555555555555")],
        },
        OwnerCase {
            label: "one group, named by a plain identifier",
            owners: vec![Owner::group("analytics-stewards")],
        },
        OwnerCase {
            label: "a user and a group that SHARE an identifier: two different owners",
            owners: vec![
                Owner::user("shared.identifier@example.com"),
                Owner::group("shared.identifier@example.com"),
            ],
        },
        OwnerCase {
            label: "five owners spanning every identifier form the patterns allow",
            owners: vec![
                Owner::user("0123456789-AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE"),
                Owner::user("arn:aws:iam::123456789012:role/DataZoneOwner"),
                Owner::user("data_steward+42"),
                Owner::group("99999999-8888-7777-6666-555555555555"),
                Owner::group("Finance.Owners"),
            ],
        },
    ]
}

#[tokio::test]
async fn the_listing_reports_exactly_the_owners_the_entity_holds() {
    let dz = Emulator::new();
    let domain = dz.available_domain("entity-owners-domain").await;
    let root = root_unit(&dz, &domain).await;

    for (index, case) in owner_cases().into_iter().enumerate() {
        let unit = create_unit(&dz, &domain, &root, &format!("unit {index}"), case.label).await;
        // The owner every entity starts with is the principal that created it, which
        // `GetDomainUnit` names in `createdBy` — read from the store, never assumed.
        let created = get_unit(&dz, &domain, &unit).await;
        let creator = created.s("createdBy");
        assert_ne!(
            creator, "<absent>",
            "case {:?}: GetDomainUnit must report who created the entity, got {:?}",
            case.label, created.body
        );

        let mut expected = vec![Owner::user(&creator)];
        for owner in &case.owners {
            let added = add(&dz, &domain, DOMAIN_UNIT, &unit, owner).await;
            assert_eq!(
                added.status, 201,
                "case {:?}: expected 201 adding {owner:?}, got {} {:?}",
                case.label, added.status, added.body
            );
            expected.push(owner.clone());
        }

        let listed = list(&dz, &domain, DOMAIN_UNIT, &unit, "").await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.label, listed.status, listed.body
        );
        let reported = owners_of(&listed.body);
        let mut sorted_reported = reported.clone();
        sorted_reported.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_reported, sorted_expected,
            "case {:?}: expected exactly the entity's owners, got {reported:?}",
            case.label
        );
        assert!(
            listed.body.get("nextToken").is_none_or(Value::is_null),
            "case {:?}: {} owners fit in one page, so no nextToken is offered, got {:?}",
            case.label,
            expected.len(),
            listed.body.get("nextToken")
        );

        // RULE 1: the entity's own reader reports the same owner set, so the listing
        // reads the stored entity rather than a second copy of it.
        let via_get = get_unit(&dz, &domain, &unit).await;
        let mut from_reader = owners_of(&via_get.body);
        from_reader.sort();
        assert_eq!(
            from_reader, sorted_expected,
            "case {:?}: GetDomainUnit must report the same owners as the listing, got {:?}",
            case.label, via_get.body
        );

        // RULE 4: listing twice reports the same owners in the same order, so the read
        // changed nothing and the order is stable.
        let again = list(&dz, &domain, DOMAIN_UNIT, &unit, "").await;
        assert_eq!(
            owners_of(&again.body),
            reported,
            "case {:?}: a second listing must report the same owners in the same order, got {:?}",
            case.label,
            again.body
        );
    }
}

#[tokio::test]
async fn adding_and_removing_an_owner_is_a_set_operation() {
    let dz = Emulator::new();
    let domain = dz.available_domain("entity-owners-set-domain").await;
    let root = root_unit(&dz, &domain).await;

    for (index, case) in owner_cases().into_iter().enumerate() {
        if case.owners.is_empty() {
            continue;
        }
        let unit = create_unit(&dz, &domain, &root, &format!("set unit {index}"), case.label).await;
        let creator = Owner::user(&get_unit(&dz, &domain, &unit).await.s("createdBy"));

        // Adding each owner TWICE records it once: the second call is an accepted
        // repeat, not a second owner.
        for owner in &case.owners {
            for attempt in 1..=2 {
                let added = add(&dz, &domain, DOMAIN_UNIT, &unit, owner).await;
                assert_eq!(
                    added.status, 201,
                    "case {:?}: expected 201 on add attempt {attempt} for {owner:?}, got {} {:?}",
                    case.label, added.status, added.body
                );
            }
        }
        let mut expected = vec![creator.clone()];
        expected.extend(case.owners.iter().cloned());
        expected.sort();
        let mut got = owners_of(&list(&dz, &domain, DOMAIN_UNIT, &unit, "").await.body);
        got.sort();
        assert_eq!(
            got, expected,
            "case {:?}: a repeated add must record each owner exactly once, got {got:?}",
            case.label
        );

        // Removing an owner the entity never had changes nothing, and succeeds.
        let never_added = Owner::group("never-an-owner-of-this-entity");
        let removed_absent = remove(&dz, &domain, DOMAIN_UNIT, &unit, &never_added).await;
        assert_eq!(
            removed_absent.status, 204,
            "case {:?}: removing an owner the entity never had must succeed, got {} {:?}",
            case.label, removed_absent.status, removed_absent.body
        );
        let mut unchanged = owners_of(&list(&dz, &domain, DOMAIN_UNIT, &unit, "").await.body);
        unchanged.sort();
        assert_eq!(
            unchanged, expected,
            "case {:?}: removing a non-owner must leave the owner set alone, got {unchanged:?}",
            case.label
        );

        // Removing each added owner twice leaves the entity with the owner its create
        // recorded, and only that one.
        for owner in &case.owners {
            for attempt in 1..=2 {
                let answer = remove(&dz, &domain, DOMAIN_UNIT, &unit, owner).await;
                assert_eq!(
                    answer.status, 204,
                    "case {:?}: expected 204 on remove attempt {attempt} for {owner:?}, got {} {:?}",
                    case.label, answer.status, answer.body
                );
            }
        }
        let left = owners_of(&list(&dz, &domain, DOMAIN_UNIT, &unit, "").await.body);
        assert_eq!(
            left,
            vec![creator.clone()],
            "case {:?}: every added owner was removed, so only the creator is left, got {left:?}",
            case.label
        );
        let mut via_get = owners_of(&get_unit(&dz, &domain, &unit).await.body);
        via_get.sort();
        assert_eq!(
            via_get,
            vec![creator],
            "case {:?}: GetDomainUnit must agree with the listing after the removes, got {via_get:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn following_next_token_walks_every_owner_exactly_once() {
    let dz = Emulator::new();
    let domain = dz.available_domain("entity-owners-paging-domain").await;
    let root = root_unit(&dz, &domain).await;
    let unit = create_unit(&dz, &domain, &root, "paged unit", "paging").await;
    let creator = Owner::user(&get_unit(&dz, &domain, &unit).await.s("createdBy"));

    // Enough owners that even a request naming no page size has to offer a token: the
    // modelled `maxResults` ceiling is 25, and the creator is an owner too.
    let mut all = vec![creator];
    for index in 0..30 {
        // Alternating kinds, so the walk has to carry both union members.
        let owner = if index % 2 == 0 {
            Owner::user(&format!("paged-user-{index:02}"))
        } else {
            Owner::group(&format!("paged-group-{index:02}"))
        };
        let added = add(&dz, &domain, DOMAIN_UNIT, &unit, &owner).await;
        assert_eq!(
            added.status, 201,
            "case \"paging\": expected 201 adding {owner:?}, got {} {:?}",
            added.status, added.body
        );
        all.push(owner);
    }
    all.sort();
    let owner_count = all.len();

    // Both published bounds of `MaxResultsForListDomains` (`@range` 1..25) and sizes in
    // between: ones that divide the set and ones that do not.
    for page_size in [1usize, 2, 7, 10, 25] {
        let mut walked: Vec<Owner> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(t) => format!("?maxResults={page_size}&nextToken={t}"),
                None => format!("?maxResults={page_size}"),
            };
            let page = list(&dz, &domain, DOMAIN_UNIT, &unit, &query).await;
            assert_eq!(
                page.status, 200,
                "case page_size={page_size} page {pages}: expected 200, got {} {:?}",
                page.status, page.body
            );
            let owners = owners_of(&page.body);
            assert!(
                owners.len() <= page_size,
                "case page_size={page_size} page {pages}: a page must not exceed maxResults, got {} owners",
                owners.len()
            );
            assert!(
                !owners.is_empty(),
                "case page_size={page_size} page {pages}: a token must not lead to an empty page, got {:?}",
                page.body
            );
            walked.extend(owners);
            pages += 1;
            assert!(
                pages <= owner_count + 1,
                "case page_size={page_size}: the walk did not terminate after {pages} pages"
            );
            match page.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let expected_pages = owner_count.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page_size={page_size}: expected {expected_pages} pages for {owner_count} owners, got {pages}"
        );
        let mut sorted_walk = walked.clone();
        sorted_walk.sort();
        sorted_walk.dedup();
        assert_eq!(
            sorted_walk.len(),
            walked.len(),
            "case page_size={page_size}: the pages must not report an owner twice, got {walked:?}"
        );
        assert_eq!(
            sorted_walk, all,
            "case page_size={page_size}: the pages concatenated must be every owner exactly once, got {walked:?}"
        );
    }

    // A request naming no page size gets the modelled ceiling and a token for the rest.
    let defaulted = list(&dz, &domain, DOMAIN_UNIT, &unit, "").await;
    assert_eq!(
        owners_of(&defaulted.body).len(),
        25,
        "case \"no maxResults\": expected the modelled ceiling of 25 owners, got {:?}",
        defaulted.body
    );
    let next = defaulted
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !next.is_empty(),
        "case \"no maxResults\": {owner_count} owners do not fit one page, so a nextToken is required, got {:?}",
        defaulted.body
    );
    let last = list(
        &dz,
        &domain,
        DOMAIN_UNIT,
        &unit,
        &format!("?nextToken={next}"),
    )
    .await;
    assert_eq!(
        owners_of(&last.body).len(),
        owner_count - 25,
        "case \"no maxResults, second page\": expected the remaining {} owners, got {:?}",
        owner_count - 25,
        last.body
    );
    assert!(
        last.body.get("nextToken").is_none_or(Value::is_null),
        "case \"no maxResults, second page\": the last page offers no token, got {:?}",
        last.body.get("nextToken")
    );
}

#[tokio::test]
async fn a_bad_entity_type_identifier_token_or_page_size_is_refused() {
    let dz = Emulator::new();
    let domain = dz.available_domain("entity-owners-refusals").await;
    let root = root_unit(&dz, &domain).await;
    let unit = create_unit(&dz, &domain, &root, "refusal unit", "refusals").await;
    let owner = Owner::user("refusals-owner");
    let added = add(&dz, &domain, DOMAIN_UNIT, &unit, &owner).await;
    assert_eq!(
        added.status, 201,
        "setting up the refusal cases: expected 201 adding {owner:?}, got {} {:?}",
        added.status, added.body
    );
    let before = owners_of(&list(&dz, &domain, DOMAIN_UNIT, &unit, "").await.body);

    // A call scoped to a domain the store serves no record of is DENIED, whatever else
    // it names: a permission is held ON a domain, so the caller holds none there and
    // authorization decides the call before any lookup — for the listing and for both
    // writes alike.
    for (label, operation, answer) in [
        (
            "list in a domain that is not stored",
            "ListEntityOwners",
            list(&dz, "dzd_nosuchdomain000", DOMAIN_UNIT, &unit, "").await,
        ),
        (
            "add into a domain that is not stored",
            "AddEntityOwner",
            add(
                &dz,
                "dzd_nosuchdomain000",
                DOMAIN_UNIT,
                &unit,
                &Owner::user("x"),
            )
            .await,
        ),
        (
            "remove from a domain that is not stored",
            "RemoveEntityOwner",
            remove(&dz, "dzd_nosuchdomain000", DOMAIN_UNIT, &unit, &owner).await,
        ),
    ] {
        assert_eq!(
            (answer.status, answer.error_code.as_str(), answer.s("message")),
            (
                403,
                "AccessDeniedException",
                format!("User is not permitted to perform operation: {operation}")
            ),
            "case {label:?}: expected the modelled denial, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // Inside a domain the caller may act in, the listing models no
    // `ResourceNotFoundException`, so every one of these is bad input: an unknown
    // identifier, an entityType the model does not declare, a token this service never
    // issued, and a page size one past each published bound of the modelled `maxResults`
    // range (1..25).
    let list_cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "an entity that is not stored in this domain",
            domain.clone(),
            DOMAIN_UNIT,
            "",
        ),
        (
            "an entityType the model does not declare",
            domain.clone(),
            "PROJECT",
            "",
        ),
        (
            "a nextToken this service never issued",
            domain.clone(),
            DOMAIN_UNIT,
            "?nextToken=notatoken",
        ),
        (
            "maxResults below the modelled range",
            domain.clone(),
            DOMAIN_UNIT,
            "?maxResults=0",
        ),
        (
            "maxResults above the modelled range",
            domain.clone(),
            DOMAIN_UNIT,
            "?maxResults=26",
        ),
    ];
    for (label, in_domain, entity_type, query) in list_cases {
        // The "entity that is not stored" row is the only one that needs a bad entity
        // id; every other row keeps the stored one so exactly one thing is wrong.
        let entity = if label.starts_with("an entity") {
            "nosuchentity"
        } else {
            unit.as_str()
        };
        let answer = list(&dz, &in_domain, entity_type, entity, query).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // Both writes DO model `ResourceNotFoundException`, so an entity that names nothing
    // in a domain the caller may act in is 404 there; an entityType the model does not
    // declare is still 400.
    for (label, in_domain, entity_type, entity, status, code) in [
        (
            "add onto an entity that is not stored",
            domain.as_str(),
            DOMAIN_UNIT,
            "nosuchentity",
            404,
            "ResourceNotFoundException",
        ),
        (
            "add with an entityType the model does not declare",
            domain.as_str(),
            "PROJECT",
            unit.as_str(),
            400,
            "ValidationException",
        ),
    ] {
        let answer = add(&dz, in_domain, entity_type, entity, &Owner::user("x")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
    }
    for (label, in_domain, entity_type, entity, status, code) in [
        (
            "remove from an entity that is not stored",
            domain.as_str(),
            DOMAIN_UNIT,
            "nosuchentity",
            404,
            "ResourceNotFoundException",
        ),
        (
            "remove with an entityType the model does not declare",
            domain.as_str(),
            "PROJECT",
            unit.as_str(),
            400,
            "ValidationException",
        ),
    ] {
        let answer = remove(&dz, in_domain, entity_type, entity, &owner).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // Every refusal left the owner set exactly as it was.
    let after = owners_of(&list(&dz, &domain, DOMAIN_UNIT, &unit, "").await.body);
    assert_eq!(
        after, before,
        "the refused calls must not have changed the entity's owners, got {after:?}"
    );
}

#[tokio::test]
async fn an_owner_cannot_be_added_while_the_domain_is_still_being_created() {
    let dz = Emulator::new();
    // NOT settled: the call below lands while the domain is still `CREATING`, which is
    // a state the domain cannot take a write in.
    let created = dz
        .call(
            "POST",
            "/v2/domains",
            Some(json!({ "name": "entity-owners-creating-domain" })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 creating the domain, got {} {:?}",
        created.status, created.body
    );
    let domain = created.s("id");
    let root = created.s("rootDomainUnitId");
    assert_eq!(
        created.s("status"),
        "CREATING",
        "the domain is reported in progress before it settles, got {:?}",
        created.body
    );

    let answer = add(&dz, &domain, DOMAIN_UNIT, &root, &Owner::user("early-owner")).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (409, "ConflictException"),
        "case \"a domain that is still CREATING\": expected a 409 ConflictException, got {} {:?}",
        answer.status,
        answer.body
    );

    // Once the clock settles the domain, the same request is accepted and the owner is
    // reported by the listing.
    tokio::time::sleep(crate::harness::SETTLED).await;
    let accepted = add(&dz, &domain, DOMAIN_UNIT, &root, &Owner::user("early-owner")).await;
    assert_eq!(
        accepted.status, 201,
        "case \"the settled domain\": expected 201, got {} {:?}",
        accepted.status, accepted.body
    );
    let listed = owners_of(&list(&dz, &domain, DOMAIN_UNIT, &root, "").await.body);
    assert!(
        listed.contains(&Owner::user("early-owner")),
        "case \"the settled domain\": expected the added owner in the listing, got {listed:?}"
    );
}
