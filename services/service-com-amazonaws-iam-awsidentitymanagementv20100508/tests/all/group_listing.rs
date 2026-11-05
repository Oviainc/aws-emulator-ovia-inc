// SPDX-License-Identifier: CC0-1.0

//! `CreateGroup` and `ListGroups`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of groups that were
//! PERSISTED by `CreateGroup`, never a handler called directly. The awsQuery harness is
//! the one `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! Each suite is a TABLE whose rows differ along what the rule being asserted generalises
//! over: the name alphabet and case, the path depth, the number of persisted groups, the
//! page size and the path prefix. No single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The fixture group set: names spanning the whole `^[\w+=,.@-]+$` alphabet and both
/// cases (`smithy_query(query="shape", shape="Group")` for `GroupName`), and paths
/// spanning the root, a one-segment path and a nested path, so a rule about paths is
/// exercised at more than one depth.
const GROUPS: &[(&str, &str)] = &[
    ("admins", "/eng/"),
    ("Billing", "/"),
    ("core_7", "/eng/tools/"),
    ("data.x", "/sales/"),
    ("Eng+1", "/eng/"),
    ("finance=2", "/"),
    ("growth@3", "/sales/west/"),
    ("hr-4", "/eng/tools/"),
];

/// The fixture group names in the order the service enumerates them: ascending by the
/// case-folded name, which is the key the store is ordered on — IAM entity names "are not
/// distinguished by case" (`CreateGroup$GroupName`).
fn groups_in_store_order() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = GROUPS.iter().map(|(name, _)| *name).collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// Create one group, asserting the fixture create succeeded.
async fn create_group(router: &MultiServiceRouter, group_name: &str, path: &str) -> String {
    let (status, body) = call(
        router,
        "CreateGroup",
        &[("GroupName", group_name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: the fixture CreateGroup must succeed, got {status}: {body}"
    );
    body
}

/// Create every fixture group, answering the router they were created in.
async fn with_fixture_groups() -> MultiServiceRouter {
    let router = router();
    for (name, path) in GROUPS {
        create_group(&router, name, path).await;
    }
    router
}

/// One group as a response reports it, read off the REQUIRED members of the `Group`
/// shape — `Path`, `GroupName`, `GroupId`, `Arn`, `CreateDate` — each of which every
/// member of the list carries exactly once, so position identifies the group.
#[derive(Debug, PartialEq, Eq)]
struct ListedGroup {
    group_name: String,
    path: String,
    group_id: String,
    arn: String,
    create_date: String,
}

/// The groups a `ListGroups` (or `CreateGroup`) response reports, in document order.
fn listed_groups(body: &str) -> Vec<ListedGroup> {
    let names = elements(body, "GroupName");
    let paths = elements(body, "Path");
    let ids = elements(body, "GroupId");
    let arns = elements(body, "Arn");
    let dates = elements(body, "CreateDate");
    for (label, found) in [
        ("Path", paths.len()),
        ("GroupId", ids.len()),
        ("Arn", arns.len()),
        ("CreateDate", dates.len()),
    ] {
        assert_eq!(
            found,
            names.len(),
            "every listed group must carry one <{label}>: got {found} for {} groups: {body}",
            names.len()
        );
    }
    names
        .into_iter()
        .zip(paths)
        .zip(ids)
        .zip(arns)
        .zip(dates)
        .map(
            |((((group_name, path), group_id), arn), create_date)| ListedGroup {
                group_name,
                path,
                group_id,
                arn,
                create_date,
            },
        )
        .collect()
}

/// RULE: a create mints and persists a resource carrying every required member of its
/// shape, and a later read returns the STORED record — identical id, ARN, path and
/// creation instant — rather than a value recomputed from the reading request.
#[tokio::test]
async fn create_group_persists_every_required_member_and_list_groups_returns_it() {
    let router = router();
    let mut created: Vec<ListedGroup> = Vec::new();

    for (name, path) in GROUPS {
        let case = format!("{name} at {path}");
        let body = create_group(&router, name, path).await;
        let reported = listed_groups(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the create to report exactly one group: {body}"
        );
        let group = reported.into_iter().next().expect("one group");

        assert_eq!(
            group.group_name, *name,
            "case {case:?}: expected the created group to carry the requested name, got {:?}",
            group.group_name
        );
        assert_eq!(
            group.path, *path,
            "case {case:?}: expected Path {path:?}, got {:?}",
            group.path
        );
        assert_eq!(
            group.arn,
            format!("arn:aws:iam::{}:group{path}{name}", account_id()),
            "case {case:?}: expected the modelled IAM group ARN, got {:?}",
            group.arn
        );
        // `Group$GroupId` is `idType`: len 16..128, pattern ^[\w]+$.
        assert!(
            (16..=128).contains(&group.group_id.chars().count()),
            "case {case:?}: expected a GroupId of 16..128 characters, got {:?}",
            group.group_id
        );
        assert!(
            group
                .group_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "case {case:?}: expected a GroupId matching ^[\\w]+$, got {:?}",
            group.group_id
        );
        assert!(
            created.iter().all(|held| held.group_id != group.group_id),
            "case {case:?}: expected a GroupId unique across groups, got the repeat {:?}",
            group.group_id
        );
        assert!(
            ::aws_smithy_types::DateTime::from_str(
                &group.create_date,
                ::aws_smithy_types::date_time::Format::DateTime,
            )
            .is_ok(),
            "case {case:?}: expected an ISO-8601 CreateDate, got {:?}",
            group.create_date
        );

        created.push(group);
    }

    // The listing reports the stored record of every group, not a recomputation: each
    // created group comes back with the identifiers it was minted with.
    let (status, listed) = call(&router, "ListGroups", &[]).await;
    assert_eq!(status, 200, "expected ListGroups 200, got {status}: {listed}");
    let mut reported = listed_groups(&listed);
    reported.sort_by(|left, right| left.group_name.cmp(&right.group_name));
    created.sort_by(|left, right| left.group_name.cmp(&right.group_name));
    assert_eq!(
        reported, created,
        "expected the listing to report the stored record of every created group: {listed}"
    );
}

/// RULE: a resource-listing operation enumerates the persisted resources in a total,
/// stable order and reports `IsTruncated` false when the page holds them all; an account
/// holding none of them answers an empty list rather than an error.
#[tokio::test]
async fn list_groups_enumerates_the_account_and_reports_an_empty_account_as_empty() {
    let empty = router();
    let (status, body) = call(&empty, "ListGroups", &[]).await;
    assert_eq!(
        status, 200,
        "expected an account with no groups to be listed, got {status}: {body}"
    );
    assert!(
        listed_groups(&body).is_empty(),
        "expected no groups for an account holding none, got {body}"
    );
    assert_eq!(
        element(&body, "IsTruncated"),
        "false",
        "expected IsTruncated false for a complete page, got {:?}",
        element(&body, "IsTruncated")
    );

    let router = with_fixture_groups().await;
    let (status, body) = call(&router, "ListGroups", &[]).await;
    assert_eq!(status, 200, "expected ListGroups 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "GroupName"),
        groups_in_store_order(),
        "expected every persisted group in case-folded name order, got {body}"
    );
    assert_eq!(
        element(&body, "IsTruncated"),
        "false",
        "expected IsTruncated false when one page holds them all, got {body}"
    );
    // A read cannot change what the next caller sees.
    let (status, again) = call(&router, "ListGroups", &[]).await;
    assert_eq!(status, 200, "a second listing was refused: {again}");
    assert_eq!(
        elements(&again, "GroupName"),
        groups_in_store_order(),
        "a second listing answered differently: {again}"
    );
}

/// RULE: `PathPrefix` selects the resources whose path starts with it, and defaults to
/// the account root (a slash) when the request omits it.
#[tokio::test]
async fn list_groups_filters_by_path_prefix() {
    let router = with_fixture_groups().await;

    // Rows spanning the prefix depths the fixture holds, plus one that matches nothing.
    let expectations: &[(&str, &[&str])] = &[
        ("/", &["admins", "Billing", "core_7", "data.x", "Eng+1", "finance=2", "growth@3", "hr-4"]),
        ("/eng/", &["admins", "core_7", "Eng+1", "hr-4"]),
        ("/eng/tools/", &["core_7", "hr-4"]),
        ("/sales/", &["data.x", "growth@3"]),
        ("/sales/west/", &["growth@3"]),
        ("/nothing/", &[]),
    ];
    for (prefix, expected) in expectations {
        let (status, body) = call(&router, "ListGroups", &[("PathPrefix", prefix)]).await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200, got {status}: {body}"
        );
        let mut names = elements(&body, "GroupName");
        names.sort_by_key(|name| name.to_ascii_lowercase());
        let mut wanted: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        wanted.sort_by_key(|name| name.to_ascii_lowercase());
        assert_eq!(
            names, wanted,
            "case {prefix:?}: expected the groups under that prefix, got {body}"
        );
    }

    // An omitted prefix lists the same groups as the root prefix does.
    let (_, defaulted) = call(&router, "ListGroups", &[]).await;
    let (_, rooted) = call(&router, "ListGroups", &[("PathPrefix", "/")]).await;
    assert_eq!(
        elements(&defaulted, "GroupName"),
        elements(&rooted, "GroupName"),
        "expected an omitted PathPrefix to default to the account root: {defaulted}"
    );
}

/// RULE: a page past `MaxItems` is truncated and carries a `Marker` that resumes strictly
/// after the last resource reported, so draining the pages enumerates every resource
/// exactly once; a `Marker` this service did not issue is refused rather than read as the
/// start of the collection.
#[tokio::test]
async fn list_groups_pages_with_a_resumable_marker() {
    let router = with_fixture_groups().await;
    let all = groups_in_store_order();

    // Rows spanning page sizes from one at a time to one more than the collection holds.
    for page_size in [1usize, 2, 3, 5, 8, 9] {
        let mut drained: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let size = page_size.to_string();
            let mut params: Vec<(&str, &str)> = vec![("MaxItems", size.as_str())];
            if let Some(token) = marker.as_deref() {
                params.push(("Marker", token));
            }
            let (status, body) = call(&router, "ListGroups", &params).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status}: {body}"
            );
            let names = elements(&body, "GroupName");
            assert!(
                names.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} groups a page, \
                 got {}: {body}",
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
            "case page size {page_size}: expected the pages to enumerate every group \
             exactly once, in order"
        );
    }

    // A marker issued for another collection, and one this service never issued at all,
    // are refused: a token is a position in ONE collection.
    let (_, users_page) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    let foreign: Vec<String> = elements(&users_page, "Marker");
    for bogus in ["not-a-marker", "zz", "0g"]
        .iter()
        .map(|token| token.to_string())
        .chain(foreign)
    {
        let (status, body) = call(&router, "ListGroups", &[("Marker", bogus.as_str())]).await;
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
}

/// RULE: an IAM entity name is unique within the account and names are not distinguished
/// by case, so a second create for a name the account already holds is refused with
/// `EntityAlreadyExists` and persists nothing.
#[tokio::test]
async fn creating_a_group_whose_name_the_account_holds_is_refused() {
    let router = with_fixture_groups().await;
    let before = elements_of_names(&router).await;

    // Rows spanning the casings and paths a repeat create can be spelled with.
    let repeats: &[(&str, &str)] = &[
        ("admins", "/eng/"),
        ("ADMINS", "/eng/"),
        ("Admins", "/"),
        ("billing", "/other/"),
        ("Eng+1", "/eng/"),
    ];
    for (name, path) in repeats {
        let (status, body) = call(
            &router,
            "CreateGroup",
            &[("GroupName", name), ("Path", path)],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {name:?} at {path}: expected 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {name:?} at {path}: expected EntityAlreadyExists, got {:?}",
            error_code(&body)
        );
    }

    assert_eq!(
        elements_of_names(&router).await,
        before,
        "expected a refused create to persist nothing"
    );
}

/// The group names the account holds, as `ListGroups` reports them.
async fn elements_of_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListGroups", &[]).await;
    assert_eq!(status, 200, "expected ListGroups 200, got {status}: {body}");
    elements(&body, "GroupName")
}
