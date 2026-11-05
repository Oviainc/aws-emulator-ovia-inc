// SPDX-License-Identifier: CC0-1.0

//! `ListGroupsForUser`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour (status, awsQuery error code, XML members) a client on the socket
//! sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the
//! other suites in this crate share.
//!
//! The rule under test: a read of the collection ONE named IAM entity relates to reports
//! the stored records of exactly the related entities the account holds — names "are not
//! distinguished by case" (`CreateUser$UserName`) — in one total order, paged by
//! `MaxItems` with an opaque `Marker` a follow-up call resumes strictly after, and
//! refuses a name the account holds no such entity under with `NoSuchEntity` (404).
//! Here the entity is a user and the collection is the groups it belongs to, which is
//! the membership `AddUserToGroup` writes read the other way round from `GetGroup`.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: the
//! user's and the groups' name alphabet and path depth, the case the request spells the
//! user's name in, how many groups the user belongs to (none through every fixture
//! group), the page size (both published bounds included), the form of a `Marker` that
//! names no position in this user's memberships, and the identity-based documents the
//! calling principal holds.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// The fixture group set: names spanning the whole `^[\w+=,.@-]+$` alphabet and both
/// cases (`smithy_query(query="operation", operation="CreateGroup")`), and paths spanning
/// the root, a one-segment path and a nested path, so a rule about what a reported group
/// record carries is exercised at more than one depth.
const GROUPS: &[(&str, &str)] = &[
    ("admins", "/eng/"),
    ("Billing", "/"),
    ("core_7", "/eng/tools/"),
    ("data.x", "/sales/"),
    ("eng+1", "/"),
    ("finance=2", "/sales/west/"),
    ("gov@3", "/"),
    ("hr-4", "/eng/"),
];

/// Every fixture group's name.
fn all_groups() -> Vec<&'static str> {
    GROUPS.iter().map(|(name, _)| *name).collect()
}

/// The path the fixture group `group_name` was created under.
fn fixture_path(group_name: &str) -> &'static str {
    GROUPS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(group_name))
        .map(|(_, path)| *path)
        .expect("a fixture group")
}

/// A router holding every fixture group, created through `CreateGroup`.
async fn with_fixture_groups() -> MultiServiceRouter {
    let router = router();
    for (name, path) in GROUPS {
        let (status, body) =
            call(&router, "CreateGroup", &[("GroupName", name), ("Path", path)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the fixture CreateGroup must succeed, got {status}: {body}"
        );
    }
    router
}

/// Create the user `user_name` and add it to each of `groups`, through the operations
/// that write that state (`CreateUser`, `AddUserToGroup`), answering the group names in
/// the order a read of the user's groups must report them: ascending by the case-folded
/// group name, which is the key the store is ordered on — the same order `ListGroups`
/// pages in.
async fn with_memberships(
    router: &MultiServiceRouter,
    user_name: &str,
    path: &str,
    groups: &[&str],
) -> Vec<String> {
    let (status, body) = call(
        router,
        "CreateUser",
        &[("UserName", user_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
    for group in groups {
        let (status, body) = call(
            router,
            "AddUserToGroup",
            &[("GroupName", group), ("UserName", user_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: the fixture AddUserToGroup {group:?} must succeed, \
             got {status}: {body}"
        );
    }
    let mut ordered: Vec<String> = groups.iter().map(|name| name.to_string()).collect();
    ordered.sort_by_key(|name| name.to_ascii_lowercase());
    ordered
}

/// One user whose read must report the groups it belongs to.
struct MembershipCase {
    /// The name the user was created with.
    user_name: &'static str,
    path: &'static str,
    /// The groups the user belongs to.
    groups: Vec<&'static str>,
    /// The case the READ spells the user's name in.
    read_as: &'static str,
}

/// The rows: user names spanning the `^[\w+=,.@-]+$` alphabet and both cases, paths
/// spanning the root and two depths, memberships from none through every fixture group,
/// and reads spelling the name in a different case from the create.
fn membership_cases() -> Vec<MembershipCase> {
    vec![
        MembershipCase {
            user_name: "ada",
            path: "/eng/",
            groups: vec![],
            read_as: "ada",
        },
        MembershipCase {
            user_name: "Bruno",
            path: "/",
            groups: vec!["Billing"],
            read_as: "bruno",
        },
        MembershipCase {
            user_name: "cleo_7",
            path: "/eng/tools/",
            groups: vec!["gov@3", "admins", "core_7"],
            read_as: "CLEO_7",
        },
        MembershipCase {
            user_name: "dmitri.x",
            path: "/sales/",
            groups: vec!["eng+1", "finance=2"],
            read_as: "Dmitri.X",
        },
        MembershipCase {
            user_name: "Enzo+1",
            path: "/",
            groups: all_groups(),
            read_as: "Enzo+1",
        },
    ]
}

/// RULE: the read reports the STORED record of every related entity the named entity
/// holds — and only those — in one total order, names not distinguished by case, and a
/// complete page is not truncated and carries no `Marker`.
#[tokio::test]
async fn list_groups_for_user_reports_the_groups_the_user_belongs_to() {
    for case in membership_cases() {
        let MembershipCase {
            user_name,
            path,
            groups,
            read_as,
        } = case;
        let router = with_fixture_groups().await;
        // A second user belonging to a group this one does not: what the read reports is
        // the addressed user's memberships, never the account's whole group collection.
        with_memberships(&router, "bystander", "/", &["data.x"]).await;
        let expected = with_memberships(&router, user_name, path, &groups).await;

        let (status, body) = call(&router, "ListGroupsForUser", &[("UserName", read_as)]).await;
        assert_eq!(
            status, 200,
            "case {user_name:?} read as {read_as:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            elements(&body, "GroupName"),
            expected,
            "case {user_name:?} read as {read_as:?}: expected the groups the user belongs to, \
             got {body}"
        );

        // Each group comes back as the account holds it, with the ARN its create minted
        // under the path it was created at — not a record recomputed from the read.
        let arns = elements(&body, "Arn");
        for group in &groups {
            let group_arn =
                service_com_amazonaws_iam_awsidentitymanagementv20100508::state::group_arn(
                    fixture_path(group),
                    group,
                );
            assert!(
                arns.contains(&group_arn),
                "case {user_name:?}: expected the group ARN {group_arn:?} among {arns:?}"
            );
        }
        assert_eq!(
            elements(&body, "GroupId").len(),
            groups.len(),
            "case {user_name:?}: every reported group must carry a GroupId: {body}"
        );
        assert_eq!(
            elements(&body, "CreateDate").len(),
            groups.len(),
            "case {user_name:?}: every reported group must carry a CreateDate: {body}"
        );
        assert_eq!(
            elements(&body, "Path").len(),
            groups.len(),
            "case {user_name:?}: every reported group must carry a Path: {body}"
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {user_name:?}: a page holding every membership is not truncated: {body}"
        );
        assert!(
            elements(&body, "Marker").is_empty(),
            "case {user_name:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );

        // A read cannot change what the next caller sees.
        let (status, again) = call(&router, "ListGroupsForUser", &[("UserName", read_as)]).await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: a second read was refused, got {status}: {again}"
        );
        assert_eq!(
            elements(&again, "GroupName"),
            expected,
            "case {user_name:?}: a second read answered a different membership: {again}"
        );
        // The other user's memberships are unchanged by this user's read, and are its own.
        let (status, bystander) =
            call(&router, "ListGroupsForUser", &[("UserName", "bystander")]).await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: the other user's read was refused, got {status}: {bystander}"
        );
        assert_eq!(
            elements(&bystander, "GroupName"),
            vec!["data.x".to_string()],
            "case {user_name:?}: expected the other user's own memberships, got {bystander}"
        );
    }
}

/// RULE: `MaxItems` is a real page size: a page past it is truncated and carries a
/// `Marker` that resumes strictly after the last item reported, so draining the pages
/// enumerates every item exactly once; a `Marker` that names no position in THIS
/// collection is refused rather than read as the start of it.
#[tokio::test]
async fn list_groups_for_user_pages_the_memberships_with_a_resumable_marker() {
    let router = with_fixture_groups().await;
    let all = with_memberships(&router, "paged", "/eng/", &all_groups()).await;
    with_memberships(&router, "other", "/", &["Billing"]).await;

    // Rows spanning page sizes from one at a time, through sizes that do and do not
    // divide the membership, to one more than it holds — and the published range's
    // bounds, 1 and 1000 (`maxItemsType`, range 1..1000).
    for page_size in [1usize, 2, 3, 5, 8, 9, 1000] {
        let mut drained: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let size = page_size.to_string();
            let mut params: Vec<(&str, &str)> =
                vec![("UserName", "paged"), ("MaxItems", size.as_str())];
            if let Some(token) = marker.as_deref() {
                params.push(("Marker", token));
            }
            let (status, body) = call(&router, "ListGroupsForUser", &params).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status}: {body}"
            );
            let names = elements(&body, "GroupName");
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} groups a page, got {}: \
                 {body}",
                names.len()
            );
            drained.extend(names);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case page size {page_size}: paging did not terminate: {body}"
            );
            if element(&body, "IsTruncated") == "true" {
                marker = Some(element(&body, "Marker"));
            } else {
                assert!(
                    elements(&body, "Marker").is_empty(),
                    "case page size {page_size}: a complete page must carry no Marker: {body}"
                );
                break;
            }
        }
        assert_eq!(
            drained, all,
            "case page size {page_size}: expected the pages to enumerate every membership \
             exactly once, in order"
        );
    }

    // A token this service never issued, one it issued for the account's whole group
    // collection, and one it issued while paging ANOTHER user's memberships are all
    // refused: a marker is a position in one collection.
    let (_, groups_page) = call(&router, "ListGroups", &[("MaxItems", "1")]).await;
    let (_, paged_page) = call(
        &router,
        "ListGroupsForUser",
        &[("UserName", "paged"), ("MaxItems", "1")],
    )
    .await;
    let foreign: Vec<String> = elements(&groups_page, "Marker");
    let paged_marker = element(&paged_page, "Marker");
    for bogus in ["not-a-marker", "zz", "0g"]
        .iter()
        .map(|token| token.to_string())
        .chain(foreign)
        .chain([paged_marker])
    {
        let (status, body) = call(
            &router,
            "ListGroupsForUser",
            &[("UserName", "other"), ("Marker", bogus.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected a foreign Marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {bogus:?}: expected ValidationError, got {:?}",
            error_code(&body)
        );
    }

    // The page size is refused outside its published range, at either bound.
    for (case, max_items) in [
        ("one below the minimum", "0"),
        ("one above the maximum", "1001"),
    ] {
        let (status, body) = call(
            &router,
            "ListGroupsForUser",
            &[("UserName", "paged"), ("MaxItems", max_items)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a page size outside the range to be refused, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the query protocol's bound refusal code, got {body}"
        );
    }
}

/// RULE: a name the account holds no entity of that TYPE under refers to no entity, so
/// the read is refused `NoSuchEntity` (404) with the entity type and the name the request
/// carried, and a name that names another type of entity is no exception.
#[tokio::test]
async fn list_groups_for_user_refuses_a_name_the_account_holds_no_user_under() {
    let router = with_fixture_groups().await;
    with_memberships(&router, "ada", "/eng/", &["admins", "Billing"]).await;

    // Rows: a name nothing was ever created under, one differing from a held user by more
    // than case, one that names a GROUP rather than a user, and names spanning the
    // alphabet and both cases.
    for name in [
        "nobody",
        "ada2",
        "Ad",
        "admins",
        "Billing",
        "ghost_7",
        "void.x",
        "NOPE+1",
    ] {
        let (status, body) = call(&router, "ListGroupsForUser", &[("UserName", name)]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            element(&body, "Message"),
            format!("The user with name {name} cannot be found."),
            "case {name:?}: expected the refusal to name the user it could not find, got {:?}",
            element(&body, "Message")
        );
    }

    // The user the account DOES hold is still readable, so the refusals above are about
    // the name and not about the operation.
    let (status, body) = call(&router, "ListGroupsForUser", &[("UserName", "ada")]).await;
    assert_eq!(status, 200, "expected the held user to be read: {body}");
    assert_eq!(
        elements(&body, "GroupName"),
        vec!["admins".to_string(), "Billing".to_string()],
        "expected the held user's memberships: {body}"
    );
}

/// What a signed row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the read.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One signed row: the document the caller holds and the answer the rule requires.
struct SignedCase {
    case: &'static str,
    document: Option<String>,
    expect: Expect,
}

/// RULE: IAM authorizes its own API — a signed read is decided against the calling
/// principal's identity-based policies before anything is reported, a `Deny` about the
/// request refusing it whatever an `Allow` says, an `Allow` with no such `Deny` serving
/// it and a principal with neither being refused HTTP 403 `AccessDenied`, worded from the
/// principal, the action and the entity the request addressed.
#[tokio::test]
async fn a_signed_listing_is_decided_against_the_callers_policies() {
    let router = with_fixture_groups().await;
    // The caller reads its OWN memberships, so one user is both the principal and the
    // resource: the ARN a statement must name is that user's.
    let own = format!("arn:aws:iam::{}:user/", account_id());

    let rows = vec![
        SignedCase {
            case: "no identity-based policy at all",
            document: None,
            expect: Expect::NoAllow,
        },
        SignedCase {
            case: "an Allow naming the action on every resource",
            document: Some(crate::identity_policy_enforcement::statement(
                "Allow",
                "iam:ListGroupsForUser",
                "*",
            )),
            expect: Expect::Served,
        },
        SignedCase {
            case: "an Allow whose action element is a wildcard run",
            document: Some(crate::identity_policy_enforcement::statement(
                "Allow",
                "iam:List*",
                "*",
            )),
            expect: Expect::Served,
        },
        SignedCase {
            case: "an Allow for another action only",
            document: Some(crate::identity_policy_enforcement::statement(
                "Allow",
                "iam:ListGroups",
                "*",
            )),
            expect: Expect::NoAllow,
        },
        SignedCase {
            case: "an Allow scoped to another user's ARN",
            document: Some(crate::identity_policy_enforcement::statement(
                "Allow",
                "iam:ListGroupsForUser",
                &format!("{own}somebody-else"),
            )),
            expect: Expect::NoAllow,
        },
        SignedCase {
            case: "an Allow and a Deny, both about the request",
            document: Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListGroupsForUser","Resource":"*"},{"Effect":"Deny","Action":"iam:ListGroupsForUser","Resource":"*"}]}"#
                    .to_string(),
            ),
            expect: Expect::ExplicitDeny,
        },
    ];

    for (row, case) in rows.into_iter().enumerate() {
        let user_name = format!("group-reader-{row}");
        let policies: Vec<(String, String)> = case
            .document
            .iter()
            .map(|document| ("grant".to_string(), document.clone()))
            .collect();
        let access_key_id =
            crate::identity_policy_enforcement::principal(&router, case.case, &user_name, &policies)
                .await;
        // A membership to report, so a served call is distinguishable from an empty
        // answer.
        let (status, added) = call(
            &router,
            "AddUserToGroup",
            &[("GroupName", "admins"), ("UserName", &user_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture AddUserToGroup must succeed, got {status}: {added}",
            case.case
        );

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListGroupsForUser",
            &[("UserName", user_name.as_str())],
        )
        .await;

        match case.expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the read to be served, got {status}: {body}",
                    case.case
                );
                assert_eq!(
                    elements(&body, "GroupName"),
                    vec!["admins".to_string()],
                    "case {:?}: expected the caller's own membership, got {body}",
                    case.case
                );
            }
            Expect::ExplicitDeny | Expect::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {:?}: expected HTTP 403, got {status}: {body}",
                    case.case
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {:?}: expected the error code AccessDenied, got {body}",
                    case.case
                );
                // The message is built from the request: the principal that signed it,
                // the action it named, the user it addressed — named as the request named
                // it, not by ARN — which of the two refusals this is, and where this one
                // denial's authorization details are readable.
                let reason = if case.expect == Expect::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the iam:ListGroupsForUser action"
                        .to_string()
                };
                assert_access_denied_message(
                    case.case,
                    &element(&body, "Message"),
                    &format!("{own}{user_name}"),
                    "iam:ListGroupsForUser",
                    &format!("user {user_name}"),
                    &reason,
                );
            }
        }
    }
}
