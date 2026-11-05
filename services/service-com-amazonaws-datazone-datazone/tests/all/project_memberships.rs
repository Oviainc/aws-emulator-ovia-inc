// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for DataZone project memberships.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the response body as evidence, so a handler that
//! answered from a hardcoded value instead of from the store would fail these tests.
//! The harness itself (`Emulator`, `Answer`) is the one the domain/project suite
//! already owns.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. `CreateProjectMembership` persists the membership the `Member` union names, and
//!    `ListProjectMemberships` reports it back on the matching `MemberDetails` arm with
//!    the designation it was given — for every modelled designation.
//! 2. A member must be a principal the domain can hold a membership for: a group needs
//!    a group profile in the domain, and a domain that holds none refuses the
//!    membership, naming the group and the domain.
//! 3. One principal holds at most one designation in a project: re-adding a member
//!    replaces it.
//! 4. `DeleteProjectMembership` removes exactly that member, and refuses a principal
//!    that holds no membership with the same not-found error an unknown project gets.
//! 5. The listing is scoped to one project, ordered by member, and paginated: a page
//!    walk visits every stored member exactly once, in either `sortOrder`.
//! 6. `userCreatedDesignationId` is required when `designation` is
//!    `USER_CREATED_DESIGNATION` and must not be provided otherwise.
//! 7. The memberships are the ones `ListProjects`'s principal filters select on, and
//!    the ones `CreateProject`'s `membershipAssignments` store.

use crate::harness::{Answer, Emulator};
use serde_json::{json, Value};

/// Every `UserDesignation` the model declares (smithy_query shape="UserDesignation":
/// PROJECT_OWNER | PROJECT_CONTRIBUTOR | PROJECT_CATALOG_VIEWER |
/// PROJECT_CATALOG_CONSUMER | PROJECT_CATALOG_STEWARD | USER_CREATED_DESIGNATION). The
/// last one names a designation resource and is covered by its own case below.
const SERVICE_DESIGNATIONS: [&str; 5] = [
    "PROJECT_OWNER",
    "PROJECT_CONTRIBUTOR",
    "PROJECT_CATALOG_VIEWER",
    "PROJECT_CATALOG_CONSUMER",
    "PROJECT_CATALOG_STEWARD",
];

/// The `UserDesignation` that points at a caller-created designation.
const USER_CREATED_DESIGNATION: &str = "USER_CREATED_DESIGNATION";

/// One requested membership: which arm of the `Member` union, which identifier, which
/// designation.
#[derive(Clone)]
struct MemberCase {
    label: &'static str,
    /// `"user"` or `"group"` — the `MemberDetails` arm the answer must use.
    kind: &'static str,
    identifier: String,
    designation: &'static str,
}

impl MemberCase {
    /// The `Member` union value naming this principal.
    fn member(&self) -> Value {
        if self.kind == "user" {
            json!({ "userIdentifier": self.identifier })
        } else {
            json!({ "groupIdentifier": self.identifier })
        }
    }

    /// The `(kind, identifier, designation)` triple `ListProjectMemberships` must
    /// report for it.
    fn reported(&self) -> (String, String, String) {
        (
            self.kind.to_string(),
            self.identifier.clone(),
            self.designation.to_string(),
        )
    }
}

/// The refusal a membership naming a group gets while its domain holds no group
/// profile for it — built from the request, so it names this group and this domain.
fn group_profile_refusal(group: &str, domain: &str) -> String {
    format!(
        "Group profile '{group}' not found for domain '{domain}', profile creation is only valid for implicit domain"
    )
}

/// The group identifiers the refusal cases use: they differ in form and length, and
/// none of them is a group the domain holds a profile for (no domain does, until
/// `CreateGroupProfile` is implemented).
const GROUP_IDENTIFIERS: [&str; 4] = [
    "analysts",
    "AIDAI23EXAMPLEGROUP",
    "4ce98a60-1111-2222-3333-444455556666",
    "g",
];

/// The rows differ along what the rule generalises over: the designation and the form
/// and length of the member identifier.
fn member_cases() -> Vec<MemberCase> {
    vec![
        MemberCase {
            label: "short name, owner",
            kind: "user",
            identifier: "alice".to_string(),
            designation: SERVICE_DESIGNATIONS[0],
        },
        MemberCase {
            label: "iam principal arn, contributor",
            kind: "user",
            identifier: "arn:aws:iam::123456789012:user/dana".to_string(),
            designation: SERVICE_DESIGNATIONS[1],
        },
        MemberCase {
            label: "uuid-shaped identifier, catalog viewer",
            kind: "user",
            identifier: "4ce98a60-1111-2222-3333-444455556666".to_string(),
            designation: SERVICE_DESIGNATIONS[2],
        },
        MemberCase {
            label: "upper-case identifier, catalog consumer",
            kind: "user",
            identifier: "AIDAI23EXAMPLEUSER".to_string(),
            designation: SERVICE_DESIGNATIONS[3],
        },
        MemberCase {
            label: "one-character identifier, catalog steward",
            kind: "user",
            identifier: "x".to_string(),
            designation: SERVICE_DESIGNATIONS[4],
        },
    ]
}

/// Create a project in `domain` and answer its id.
async fn project(dz: &Emulator, domain: &str, name: &str) -> String {
    let created = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects"),
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "creating project {name:?}: expected 201, got {} {:?}",
        created.status, created.body
    );
    created.s("id")
}

/// The `(kind, identifier, designation)` triples one `ListProjectMemberships` answer
/// reports, in the order the answer lists them.
fn reported_members(answer: &Answer) -> Vec<(String, String, String)> {
    let members = answer
        .body
        .get("members")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    members
        .iter()
        .map(|member| {
            let details = member.get("memberDetails");
            let (kind, identifier) = match details.and_then(|d| d.get("user")) {
                Some(user) => ("user", user.get("userId")),
                None => (
                    "group",
                    details.and_then(|d| d.get("group")).and_then(|g| g.get("groupId")),
                ),
            };
            (
                kind.to_string(),
                identifier
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
                    .to_string(),
                member
                    .get("designation")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
                    .to_string(),
            )
        })
        .collect()
}

/// List every membership of one project in one call (the modelled `maxResults` ceiling
/// is 50, above every count these tests store).
async fn list_members(dz: &Emulator, domain: &str, project: &str) -> Answer {
    dz.call(
        "GET",
        &format!("/v2/domains/{domain}/projects/{project}/memberships"),
        None,
    )
    .await
}

/// Add one member, asserting the modelled 201.
async fn add_member(dz: &Emulator, domain: &str, project: &str, case: &MemberCase) -> Answer {
    let answer = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects/{project}/createMembership"),
            Some(json!({ "member": case.member(), "designation": case.designation })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {:?}: expected 201 from CreateProjectMembership, got {} {:?}",
        case.label, answer.status, answer.body
    );
    answer
}

/// The order `ListProjectMemberships` reports members in: by member identifier (the
/// only name this emulator holds for a member, so the `sortBy=NAME` order), then by
/// kind.
fn in_name_order(mut members: Vec<(String, String, String)>) -> Vec<(String, String, String)> {
    members.sort_by(|a, b| (&a.1, &a.0).cmp(&(&b.1, &b.0)));
    members
}

#[tokio::test]
async fn every_designation_is_persisted_and_reported() {
    let dz = Emulator::new();
    let domain = dz.available_domain("membership-domain").await;
    let holder = project(&dz, &domain, "membership holder").await;
    let bystander = project(&dz, &domain, "bystander").await;

    // A project nobody has been added to reports the required (empty) member list.
    let empty = list_members(&dz, &domain, &holder).await;
    assert_eq!(
        empty.status, 200,
        "expected 200 from ListProjectMemberships, got {} {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.body.get("members"),
        Some(&json!([])),
        "a project with no members must still report the required members list, got {:?}",
        empty.body
    );

    let mut added: Vec<(String, String, String)> = Vec::new();
    for case in member_cases() {
        add_member(&dz, &domain, &holder, &case).await;
        added.push(case.reported());

        let listed = list_members(&dz, &domain, &holder).await;
        assert!(
            reported_members(&listed).contains(&case.reported()),
            "case {:?}: expected {:?} among the reported members, got {:?}",
            case.label,
            case.reported(),
            reported_members(&listed)
        );
        // No member carries a user-created designation id: the designations are a
        // resource this emulator refuses (see the 501 case below).
        for member in listed
            .body
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            assert!(
                member.get("userCreatedDesignationId").is_none(),
                "case {:?}: a membership with a service designation must carry no userCreatedDesignationId, got {member:?}",
                case.label
            );
        }
    }

    // The whole stored set, in the documented order, and nothing else.
    let listed = list_members(&dz, &domain, &holder).await;
    assert_eq!(
        reported_members(&listed),
        in_name_order(added),
        "ListProjectMemberships must report every stored member once, in member order"
    );

    // The listing is scoped to ONE project: the other project in the same domain has
    // no members.
    let other = list_members(&dz, &domain, &bystander).await;
    assert_eq!(
        reported_members(&other),
        Vec::new(),
        "a sibling project must report no members, got {:?}",
        other.body
    );

    // The memberships live with the project: deleting it takes them with it.
    let deleted = dz
        .call(
            "DELETE",
            &format!("/v2/domains/{domain}/projects/{holder}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "expected 204 from DeleteProject, got {} {:?}",
        deleted.status, deleted.body
    );
    let gone = list_members(&dz, &domain, &holder).await;
    assert_eq!(
        (gone.status, gone.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "memberships of a deleted project must be gone, got {} {:?}",
        gone.status,
        gone.body
    );
}

#[tokio::test]
async fn a_group_member_needs_a_group_profile_in_its_domain() {
    let dz = Emulator::new();
    // Two domains, so the refusal is seen to name the domain of the request rather
    // than any fixed value.
    let mut domains = Vec::new();
    for name in ["group-domain-one", "group-domain-two"] {
        let domain = dz.available_domain(name).await;
        let project_id = project(&dz, &domain, "group project").await;
        domains.push((domain, project_id));
    }

    for (domain, project_id) in &domains {
        for group in GROUP_IDENTIFIERS {
            for designation in SERVICE_DESIGNATIONS {
                let answer = dz
                    .call(
                        "POST",
                        &format!("/v2/domains/{domain}/projects/{project_id}/createMembership"),
                        Some(json!({
                            "member": { "groupIdentifier": group },
                            "designation": designation,
                        })),
                    )
                    .await;
                assert_eq!(
                    (answer.status, answer.error_code.as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {group}/{designation}: a group with no profile in the domain must be refused, got {} {:?}",
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.s("message"),
                    group_profile_refusal(group, domain),
                    "case {group}/{designation}: the refusal must name this group and this domain"
                );
            }

            // The same principal rule on the delete path …
            let removed = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/projects/{project_id}/deleteMembership"),
                    Some(json!({ "member": { "groupIdentifier": group } })),
                )
                .await;
            assert_eq!(
                (removed.status, removed.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {group}: removing a group with no profile must be refused, got {} {:?}",
                removed.status,
                removed.body
            );
            assert_eq!(
                removed.s("message"),
                group_profile_refusal(group, domain),
                "case {group}: the delete refusal must name this group and this domain"
            );

            // … and on `CreateProject`'s assignments.
            let created = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/projects"),
                    Some(json!({
                        "name": "group assignment project",
                        "membershipAssignments": [{
                            "member": { "groupIdentifier": group },
                            "designation": SERVICE_DESIGNATIONS[0],
                        }],
                    })),
                )
                .await;
            assert_eq!(
                (created.status, created.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {group}: assigning a group with no profile at create time must be refused, got {} {:?}",
                created.status,
                created.body
            );
            assert_eq!(
                created.s("message"),
                group_profile_refusal(group, domain),
                "case {group}: the assignment refusal must name this group and this domain"
            );
        }

        // Nothing was stored by any refused request.
        assert_eq!(
            reported_members(&list_members(&dz, domain, project_id).await),
            Vec::new(),
            "a refused group membership must not be stored"
        );
    }
}

#[tokio::test]
async fn re_adding_a_member_replaces_its_designation() {
    let dz = Emulator::new();
    let domain = dz.available_domain("replace-domain").await;
    let project_id = project(&dz, &domain, "replace project").await;

    // Each row re-designates the member the previous row added: the pairs differ in
    // both designations, including a row that re-sends the same one.
    let rows = [
        ("user", "alice", SERVICE_DESIGNATIONS[0], SERVICE_DESIGNATIONS[1]),
        ("user", "carol", SERVICE_DESIGNATIONS[2], SERVICE_DESIGNATIONS[4]),
        ("user", "bob", SERVICE_DESIGNATIONS[3], SERVICE_DESIGNATIONS[3]),
    ];
    for (kind, identifier, first, second) in rows {
        for designation in [first, second] {
            add_member(
                &dz,
                &domain,
                &project_id,
                &MemberCase {
                    label: "re-designation",
                    kind,
                    identifier: identifier.to_string(),
                    designation,
                },
            )
            .await;
        }
        let listed = reported_members(&list_members(&dz, &domain, &project_id).await);
        let for_member: Vec<&(String, String, String)> = listed
            .iter()
            .filter(|(k, id, _)| k == kind && id == identifier)
            .collect();
        assert_eq!(
            for_member.len(),
            1,
            "case {kind}/{identifier}: a principal holds exactly one membership, got {for_member:?}"
        );
        assert_eq!(
            for_member[0].2, second,
            "case {kind}/{identifier}: expected the replacing designation {second}, got {:?}",
            for_member[0].2
        );
    }
}

#[tokio::test]
async fn deleting_a_membership_removes_only_that_member_and_then_refuses() {
    let dz = Emulator::new();
    let domain = dz.available_domain("delete-member-domain").await;
    let project_id = project(&dz, &domain, "delete member project").await;

    let cases = member_cases();
    for case in &cases {
        add_member(&dz, &domain, &project_id, case).await;
    }

    let mut expected: Vec<(String, String, String)> =
        cases.iter().map(MemberCase::reported).collect();
    for case in &cases {
        let removed = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/projects/{project_id}/deleteMembership"),
                Some(json!({ "member": case.member() })),
            )
            .await;
        assert_eq!(
            removed.status, 204,
            "case {:?}: expected 204 from DeleteProjectMembership, got {} {:?}",
            case.label, removed.status, removed.body
        );
        expected.retain(|reported| *reported != case.reported());
        let listed = reported_members(&list_members(&dz, &domain, &project_id).await);
        assert_eq!(
            listed,
            in_name_order(expected.clone()),
            "case {:?}: only that member may disappear",
            case.label
        );

        // The membership is gone, so removing it again finds no such resource — and
        // changes nothing.
        let again = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/projects/{project_id}/deleteMembership"),
                Some(json!({ "member": case.member() })),
            )
            .await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: a principal that holds no membership must be refused, got {} {:?}",
            case.label,
            again.status,
            again.body
        );
        assert_eq!(
            reported_members(&list_members(&dz, &domain, &project_id).await),
            in_name_order(expected.clone()),
            "case {:?}: a refused delete must change nothing",
            case.label
        );
    }
}

#[tokio::test]
async fn a_page_walk_visits_every_member_exactly_once_in_either_order() {
    let dz = Emulator::new();
    let domain = dz.available_domain("paged-members-domain").await;
    let project_id = project(&dz, &domain, "paged members").await;

    // Seven members, enough that every page size below needs more than one page.
    let mut stored: Vec<(String, String, String)> = Vec::new();
    for (index, designation) in SERVICE_DESIGNATIONS
        .iter()
        .chain(SERVICE_DESIGNATIONS.iter())
        .take(7)
        .copied()
        .enumerate()
    {
        let case = MemberCase {
            label: "paged member",
            kind: "user",
            identifier: format!("m{index:02}"),
            designation,
        };
        add_member(&dz, &domain, &project_id, &case).await;
        stored.push(case.reported());
    }
    let ascending = in_name_order(stored.clone());
    let descending: Vec<(String, String, String)> =
        ascending.iter().rev().cloned().collect();

    // `MaxResults` is `@range` min 1, max 50 (smithy_query shape="MaxResults"): every
    // page size inside the range, including both published bounds, walks the same
    // sequence.
    for page_size in [1_usize, 2, 3, 7, 50] {
        for (order, expected) in [("ASCENDING", &ascending), ("DESCENDING", &descending)] {
            let mut seen: Vec<(String, String, String)> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut uri = format!(
                    "/v2/domains/{domain}/projects/{project_id}/memberships?sortBy=NAME&sortOrder={order}&maxResults={page_size}"
                );
                if let Some(ref next) = token {
                    uri.push_str(&format!("&nextToken={next}"));
                }
                let answer = dz.call("GET", &uri, None).await;
                assert_eq!(
                    answer.status, 200,
                    "case {order}/{page_size}: expected 200, got {} {:?}",
                    answer.status, answer.body
                );
                let page = reported_members(&answer);
                assert!(
                    page.len() <= page_size,
                    "case {order}/{page_size}: a page must not exceed maxResults, got {page:?}"
                );
                seen.extend(page);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {order}/{page_size}: the page walk does not terminate, saw {seen:?}"
                );
                match answer.body.get("nextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                &seen, expected,
                "case {order}/{page_size}: the page walk must visit every member exactly once, in order"
            );
        }
    }

    // No `sortOrder` is the ascending order, and `sortBy=NAME` is the order the store
    // already keeps.
    for query in ["", "?sortBy=NAME"] {
        let answer = dz
            .call(
                "GET",
                &format!("/v2/domains/{domain}/projects/{project_id}/memberships{query}"),
                None,
            )
            .await;
        assert_eq!(
            reported_members(&answer),
            ascending,
            "case {query:?}: the default order is by member name, ascending"
        );
    }

    // One past each published `maxResults` bound is refused by the modelled range.
    for out_of_range in ["0", "51"] {
        let answer = dz
            .call(
                "GET",
                &format!(
                    "/v2/domains/{domain}/projects/{project_id}/memberships?maxResults={out_of_range}"
                ),
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case maxResults={out_of_range}: expected the modelled range to refuse it, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

/// One request that must be refused, and the refusal it must get.
struct RefusalCase {
    label: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    status: u16,
    /// The wire error code (`__type` / `x-amzn-errortype`).
    code: &'static str,
}

#[tokio::test]
async fn an_unknown_domain_project_member_or_token_is_refused() {
    let dz = Emulator::new();
    let domain = dz.available_domain("refusal-domain").await;
    let project_id = project(&dz, &domain, "refusal project").await;
    let member = json!({ "userIdentifier": "alice" });

    let cases = vec![
        RefusalCase {
            // A permission is held ON a domain, so the caller holds none on an identifier
            // the store serves no domain for: authorization decides these calls before
            // any lookup, whether they list, create or delete.
            label: "list, unknown domain",
            method: "GET",
            uri: format!("/v2/domains/dzd_doesnotexist/projects/{project_id}/memberships"),
            body: None,
            status: 403,
            code: "AccessDeniedException",
        },
        RefusalCase {
            label: "list, unknown project",
            method: "GET",
            uri: format!("/v2/domains/{domain}/projects/nosuchproject/memberships"),
            body: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "list, token this service never issued",
            method: "GET",
            uri: format!(
                "/v2/domains/{domain}/projects/{project_id}/memberships?nextToken=notatoken"
            ),
            body: None,
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            label: "create, unknown domain",
            method: "POST",
            uri: format!("/v2/domains/dzd_doesnotexist/projects/{project_id}/createMembership"),
            body: Some(json!({ "member": member, "designation": SERVICE_DESIGNATIONS[0] })),
            status: 403,
            code: "AccessDeniedException",
        },
        RefusalCase {
            label: "create, unknown project",
            method: "POST",
            uri: format!("/v2/domains/{domain}/projects/nosuchproject/createMembership"),
            body: Some(json!({ "member": member, "designation": SERVICE_DESIGNATIONS[0] })),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "create, empty member identifier",
            method: "POST",
            uri: format!("/v2/domains/{domain}/projects/{project_id}/createMembership"),
            body: Some(
                json!({ "member": { "userIdentifier": "" }, "designation": SERVICE_DESIGNATIONS[0] }),
            ),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            label: "delete, unknown domain",
            method: "POST",
            uri: format!("/v2/domains/dzd_doesnotexist/projects/{project_id}/deleteMembership"),
            body: Some(json!({ "member": member })),
            status: 403,
            code: "AccessDeniedException",
        },
        RefusalCase {
            label: "delete, unknown project",
            method: "POST",
            uri: format!("/v2/domains/{domain}/projects/nosuchproject/deleteMembership"),
            body: Some(json!({ "member": member })),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "delete, empty member identifier",
            method: "POST",
            uri: format!("/v2/domains/{domain}/projects/{project_id}/deleteMembership"),
            body: Some(json!({ "member": { "groupIdentifier": "" } })),
            status: 400,
            code: "ValidationException",
        },
    ];

    for case in cases {
        let answer = dz.call(case.method, &case.uri, case.body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (case.status, case.code),
            "case {:?}: expected {} {}, got {} {:?}",
            case.label,
            case.status,
            case.code,
            answer.status,
            answer.body
        );
    }
}

#[tokio::test]
async fn the_user_created_designation_id_rule_is_enforced() {
    let dz = Emulator::new();
    let domain = dz.available_domain("designation-domain").await;
    let project_id = project(&dz, &domain, "designation project").await;

    // `userCreatedDesignationId` is "Required when designation is
    // USER_CREATED_DESIGNATION; must not be provided otherwise" (smithy_query
    // query="doc" shape="CreateProjectMembershipInput"
    // member="userCreatedDesignationId").
    let mut cases: Vec<(String, Value, u16, &str)> = vec![(
        format!("{USER_CREATED_DESIGNATION} without the id"),
        json!({
            "member": { "userIdentifier": "alice" },
            "designation": USER_CREATED_DESIGNATION,
        }),
        400,
        "ValidationException",
    )];
    for designation in SERVICE_DESIGNATIONS {
        cases.push((
            format!("{designation} with an id"),
            json!({
                "member": { "userIdentifier": "bob" },
                "designation": designation,
                "userCreatedDesignationId": "des-1234",
            }),
            400,
            "ValidationException",
        ));
    }
    // A designation the caller created is a resource only `CreateDesignation` can
    // make, and that operation is still a scaffold, so this membership is refused as
    // "not written yet" rather than stored against a designation that cannot exist.
    cases.push((
        format!("{USER_CREATED_DESIGNATION} with an id"),
        json!({
            "member": { "userIdentifier": "alice" },
            "designation": USER_CREATED_DESIGNATION,
            "userCreatedDesignationId": "des-1234",
        }),
        501,
        "NotImplemented",
    ));

    for (label, body, status, code) in cases {
        let answer = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/projects/{project_id}/createMembership"),
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?}",
            answer.status,
            answer.body
        );
        // A refused request stores nothing.
        assert_eq!(
            reported_members(&list_members(&dz, &domain, &project_id).await),
            Vec::new(),
            "case {label:?}: a refused membership must not be stored"
        );

        // The same rule, through `CreateProject`'s `membershipAssignments`. The
        // assignment carries exactly the members this request carried — an absent
        // `userCreatedDesignationId` stays absent rather than becoming an explicit
        // null.
        let mut assignment = serde_json::Map::new();
        for member in ["member", "designation", "userCreatedDesignationId"] {
            if let Some(value) = body.get(member) {
                assignment.insert(member.to_string(), value.clone());
            }
        }
        let created = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/projects"),
                Some(json!({
                    "name": "assignment project",
                    "membershipAssignments": [Value::Object(assignment)],
                })),
            )
            .await;
        assert_eq!(
            (created.status, created.error_code.as_str()),
            (status, code),
            "case {label:?} as a membershipAssignment: expected {status} {code}, got {} {:?}",
            created.status,
            created.body
        );
    }
}

#[tokio::test]
async fn membership_assignments_are_persisted_and_select_projects_by_member() {
    let dz = Emulator::new();
    let domain = dz.available_domain("assignment-domain").await;

    // Two projects created with different assignments, so each principal belongs to
    // exactly one of them. Each project gets two members, so the assignment list is
    // read as a list and not as one entry.
    let rows = [
        ("alpha project", "u-alpha", "u-alpha-second"),
        ("beta project", "u-beta", "u-beta-second"),
    ];
    let mut projects: Vec<(&str, String, &str, &str)> = Vec::new();
    for (name, first, second) in rows {
        let created = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/projects"),
                Some(json!({
                    "name": name,
                    "membershipAssignments": [
                        { "member": { "userIdentifier": first }, "designation": SERVICE_DESIGNATIONS[0] },
                        { "member": { "userIdentifier": second }, "designation": SERVICE_DESIGNATIONS[1] },
                    ],
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected 201 from CreateProject, got {} {:?}",
            created.status, created.body
        );
        let id = created.s("id");

        // The assignments are memberships: the listing reports them.
        let listed = reported_members(&list_members(&dz, &domain, &id).await);
        assert_eq!(
            listed,
            in_name_order(vec![
                (
                    "user".to_string(),
                    first.to_string(),
                    SERVICE_DESIGNATIONS[0].to_string()
                ),
                (
                    "user".to_string(),
                    second.to_string(),
                    SERVICE_DESIGNATIONS[1].to_string()
                ),
            ]),
            "case {name:?}: CreateProject's membershipAssignments must be stored as memberships"
        );
        projects.push((name, id, first, second));
    }

    /// The project ids `ListProjects` answers for one query.
    async fn listed_projects(dz: &Emulator, domain: &str, query: &str) -> Vec<String> {
        let answer = dz
            .call("GET", &format!("/v2/domains/{domain}/projects{query}"), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "query {query:?}: expected 200 from ListProjects, got {} {:?}",
            answer.status, answer.body
        );
        let mut ids: Vec<String> = answer
            .body
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
            .collect();
        ids.sort();
        ids
    }

    for (name, id, first, second) in &projects {
        for user in [first, second] {
            assert_eq!(
                listed_projects(&dz, &domain, &format!("?userIdentifier={user}")).await,
                vec![id.clone()],
                "case {name:?}/{user}: ListProjects filtered by userIdentifier must report only the projects that user is a member of"
            );
        }
        // A group holds no membership anywhere (no domain holds a group profile), so
        // the group filter selects nothing.
        assert_eq!(
            listed_projects(&dz, &domain, &format!("?groupIdentifier={}", GROUP_IDENTIFIERS[0]))
                .await,
            Vec::<String>::new(),
            "case {name:?}: a group that holds no membership selects no project"
        );
    }
    // A member of one project is not a member of the other, and an unknown principal
    // is a member of nothing.
    assert_eq!(
        listed_projects(&dz, &domain, "?userIdentifier=nobody").await,
        Vec::<String>::new(),
        "an unknown principal is a member of no project"
    );

    // Removing the membership removes the project from the filtered listing: the
    // filter reads the same store the delete writes.
    let (name, id, user, _) = &projects[0];
    let removed = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/projects/{id}/deleteMembership"),
            Some(json!({ "member": { "userIdentifier": user } })),
        )
        .await;
    assert_eq!(
        removed.status, 204,
        "case {name:?}: expected 204 from DeleteProjectMembership, got {} {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        listed_projects(&dz, &domain, &format!("?userIdentifier={user}")).await,
        Vec::<String>::new(),
        "case {name:?}: a removed member belongs to no project"
    );
}
