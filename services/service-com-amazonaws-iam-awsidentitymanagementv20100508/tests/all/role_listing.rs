// SPDX-License-Identifier: CC0-1.0

//! `ListRoles`, end to end over the awsQuery wire, cross-read through `GetRole`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one
//! shared `InMemoryEngine`, exactly as the production router does, so what is under
//! test is the HTTP behaviour (status, awsQuery error code, XML members) of roles
//! that were PERSISTED by `CreateRole`, never a handler called directly. The
//! awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share, so a request is built and read exactly one way.
//!
//! Each suite is a TABLE whose rows differ along what the rule being asserted
//! generalises over: the page size, the path prefix depth, the name alphabet and
//! case, the number of persisted roles, the tag count and which optional members a
//! role carries. No single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// A trust policy with a single statement — the form `CreateRole`'s own modelled
/// example carries. Every role in this suite is created with it; none of these
/// assertions is about the trust policy itself.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The fixture role set: names spanning the whole `^[\w+=,.@-]+$` alphabet and
/// both cases (`smithy_query(query="shape", shape="Role")` for `RoleName`), and
/// paths spanning the root, a one-segment path and a nested path, so a rule about
/// paths is exercised at more than one depth.
const ROLES: &[(&str, &str)] = &[
    ("alpha", "/eng/"),
    ("Beta", "/"),
    ("gamma_7", "/eng/tools/"),
    ("delta.x", "/sales/"),
    ("Epsilon+1", "/eng/"),
    ("zeta=2", "/"),
    ("eta@3", "/sales/west/"),
    ("theta-4", "/eng/tools/"),
];

/// The fixture role names in the order the service enumerates them: ascending by
/// the case-folded name, which is the key the store is ordered on — IAM entity
/// names "are not distinguished by case" (`CreateRole$RoleName`).
fn roles_in_store_order() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = ROLES.iter().map(|(name, _)| *name).collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// Create one role, asserting the fixture create succeeded.
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: the fixture CreateRole must succeed, got {status}: {body}"
    );
}

/// Create every fixture role, answering the router they were created in.
async fn with_fixture_roles() -> MultiServiceRouter {
    let router = router();
    for (name, path) in ROLES {
        create_role(&router, name, path).await;
    }
    router
}

/// One role as a `ListRoles` page reports it, read off the REQUIRED members of the
/// `Role` shape — `Path`, `RoleName`, `RoleId`, `Arn`, `CreateDate` — each of which
/// every member of the list carries exactly once, so position identifies the role.
#[derive(Debug, PartialEq, Eq)]
struct ListedRole {
    role_name: String,
    path: String,
    role_id: String,
    arn: String,
    create_date: String,
}

/// The roles a `ListRoles` (or `GetRole`) response reports, in document order.
fn listed_roles(body: &str) -> Vec<ListedRole> {
    let names = elements(body, "RoleName");
    let paths = elements(body, "Path");
    let ids = elements(body, "RoleId");
    let arns = elements(body, "Arn");
    let dates = elements(body, "CreateDate");
    for (label, found) in [
        ("Path", paths.len()),
        ("RoleId", ids.len()),
        ("Arn", arns.len()),
        ("CreateDate", dates.len()),
    ] {
        assert_eq!(
            found,
            names.len(),
            "every listed role must carry one <{label}>: got {found} for {} roles: {body}",
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
            |((((role_name, path), role_id), arn), create_date)| ListedRole {
                role_name,
                path,
                role_id,
                arn,
                create_date,
            },
        )
        .collect()
}

/// The role names a `ListRoles` response reports, in document order.
fn listed_names(body: &str) -> Vec<String> {
    elements(body, "RoleName")
}

/// RULE: a resource-listing operation enumerates the persisted resources, reporting
/// for each the record the account holds — the same identifiers a single-resource
/// read of it returns, never a value recomputed from the listing request.
#[tokio::test]
async fn list_roles_reports_the_stored_record_of_every_role() {
    let router = with_fixture_roles().await;

    let (status, body) = call(&router, "ListRoles", &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let listed = listed_roles(&body);
    assert_eq!(
        listed.iter().map(|r| r.role_name.clone()).collect::<Vec<_>>(),
        roles_in_store_order()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        "expected every persisted role once, in store order, got {:?}",
        listed.iter().map(|r| r.role_name.clone()).collect::<Vec<_>>()
    );

    // Each row of this table is one persisted role: what the listing says about it
    // must be what its own read says.
    for role in &listed {
        let case = &role.role_name;
        let (status, fetched) = call(&router, "GetRole", &[("RoleName", case.as_str())]).await;
        assert_eq!(status, 200, "case {case:?}: GetRole must find the role, got {status}: {fetched}");
        let stored = listed_roles(&fetched);
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: expected GetRole to report one role, got {:?}",
            stored
        );
        assert_eq!(
            role, &stored[0],
            "case {case:?}: expected the listing to report the stored record, got {role:?}"
        );
        let expected_path = ROLES
            .iter()
            .find(|(name, _)| name == case)
            .map(|(_, path)| *path)
            .expect("a fixture role");
        assert_eq!(
            role.path, expected_path,
            "case {case:?}: expected path {expected_path:?}, got {:?}",
            role.path
        );
        assert!(
            role.arn.ends_with(&format!("role{}{}", role.path, role.role_name)),
            "case {case:?}: expected the stored role ARN, got {:?}",
            role.arn
        );
    }
}

/// RULE: a listing of an empty collection is an empty list with `IsTruncated`
/// false and no `Marker`, not an error.
#[tokio::test]
async fn listing_an_account_holding_no_roles_is_an_empty_list() {
    let router = router();
    // Rows span the parameters a caller may add to an empty-collection listing.
    let cases: &[&[(&str, &str)]] = &[
        &[],
        &[("MaxItems", "1")],
        &[("MaxItems", "1000")],
        &[("PathPrefix", "/")],
        &[("PathPrefix", "/eng/"), ("MaxItems", "5")],
    ];
    for params in cases {
        let (status, body) = call(&router, "ListRoles", params).await;
        assert_eq!(status, 200, "case {params:?}: expected 200, got {status}: {body}");
        assert_eq!(
            listed_names(&body),
            Vec::<String>::new(),
            "case {params:?}: expected no roles, got {:?}",
            listed_names(&body)
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {params:?}: expected IsTruncated false, got {:?}",
            element(&body, "IsTruncated")
        );
        assert_eq!(
            elements(&body, "Marker"),
            Vec::<String>::new(),
            "case {params:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );
    }
}

/// RULE: a list operation enumerates the persisted entities in one total order,
/// honours `MaxItems` as a real page size, and hands out a `Marker` a follow-up
/// call resumes from without repeating or skipping an entity.
#[tokio::test]
async fn list_roles_pages_through_every_role_exactly_once() {
    let router = with_fixture_roles().await;
    let expected: Vec<String> = roles_in_store_order()
        .into_iter()
        .map(str::to_string)
        .collect();

    // Rows span the page size: the published minimum, sizes that do and do not
    // divide the set, a size equal to the set, the published maximum, and the
    // absent-parameter default. `maxItemsType` is range 1..1000
    // (`smithy_query(query="operation", operation="ListRoles")`).
    let page_sizes: &[Option<i32>] = &[
        Some(1),
        Some(2),
        Some(3),
        Some(expected.len() as i32),
        Some(1000),
        None,
    ];

    for page_size in page_sizes {
        let case = format!("MaxItems={page_size:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut params: Vec<(String, String)> = Vec::new();
            if let Some(size) = page_size {
                params.push(param("MaxItems", &size.to_string()));
            }
            if let Some(token) = &marker {
                params.push(param("Marker", token));
            }
            let (status, body) = call(&router, "ListRoles", &borrowed(&params)).await;
            assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");

            let page = listed_names(&body);
            if let Some(size) = page_size {
                assert!(
                    page.len() <= *size as usize,
                    "case {case:?}: expected at most {size} roles on a page, got {}",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: the pagination never ended, saw {seen:?}"
            );

            if element(&body, "IsTruncated") == "true" {
                marker = Some(element(&body, "Marker"));
            } else {
                assert_eq!(
                    elements(&body, "Marker"),
                    Vec::<String>::new(),
                    "case {case:?}: the last page must carry no Marker, got {:?}",
                    elements(&body, "Marker")
                );
                break;
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every role exactly once in store order, got {seen:?}"
        );
    }
}

/// RULE: `MaxItems` outside its published range is refused before the operation
/// runs, so a page size the model forbids never reaches the store.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = with_fixture_roles().await;
    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = call(&router, "ListRoles", &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = call(&router, "ListRoles", &[("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: a `Marker` this service did not issue for this collection names no
/// position in it, so the call is refused rather than silently answered from the
/// beginning; a token that does decode names a POSITION, which stays usable even
/// when the entity it was cut at is gone.
#[tokio::test]
async fn a_marker_this_collection_did_not_issue_is_refused() {
    let router = with_fixture_roles().await;

    // Rows: a token that is not base64 at all, two that decode to bytes carrying
    // none of this service's marker structure, and one this service DID issue —
    // for another collection, which is a different position space.
    for (status, body) in [
        call(&router, "ListRoles", &[("Marker", "not-a-marker")]).await,
        call(&router, "ListRoles", &[("Marker", "zz")]).await,
        call(&router, "ListRoles", &[("Marker", "0g")]).await,
    ] {
        assert_eq!(status, 400, "expected a foreign marker to be refused, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A marker issued while paging the USER collection is not a position in the
    // role collection, and vice versa.
    for user in ["one", "two"] {
        let (status, body) = call(&router, "CreateUser", &[("UserName", user)]).await;
        assert_eq!(status, 200, "case {user:?}: the fixture CreateUser must succeed: {body}");
    }
    let (status, body) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected a truncated user page, got {status}: {body}");
    let user_marker = element(&body, "Marker");
    let (status, body) = call(&router, "ListRoles", &[("Marker", user_marker.as_str())]).await;
    assert_eq!(status, 400, "expected a user marker to be refused by ListRoles, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected ValidationError, got {:?}",
        error_code(&body)
    );

    let (status, body) = call(&router, "ListRoles", &[("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected a truncated role page, got {status}: {body}");
    let role_marker = element(&body, "Marker");
    let first = element(&body, "RoleName");
    let (status, body) = call(&router, "ListUsers", &[("Marker", role_marker.as_str())]).await;
    assert_eq!(status, 400, "expected a role marker to be refused by ListUsers, got {status}: {body}");

    // A marker cut at a role that is then deleted still resumes from that
    // position: the rest of the order follows it, minus the deleted role.
    let (status, body) = call(&router, "DeleteRole", &[("RoleName", first.as_str())]).await;
    assert_eq!(status, 200, "expected the first role to be deleted, got {status}: {body}");
    let (status, body) = call(&router, "ListRoles", &[("Marker", role_marker.as_str())]).await;
    assert_eq!(status, 200, "expected a stale marker to still resume, got {status}: {body}");
    let expected: Vec<String> = roles_in_store_order()
        .into_iter()
        .filter(|name| *name != first)
        .map(str::to_string)
        .collect();
    assert_eq!(
        listed_names(&body),
        expected,
        "expected the order after the marker's position, got {:?}",
        listed_names(&body)
    );
}

/// RULE: `PathPrefix` selects exactly the resources whose path begins with it, and
/// an absent `PathPrefix` "defaults to a slash (/), listing all roles".
#[tokio::test]
async fn list_roles_filters_by_path_prefix() {
    let router = with_fixture_roles().await;

    // Rows span the prefix depth: the root, a one-segment prefix, a nested prefix,
    // a prefix that selects exactly one role, and one that selects none.
    let cases: &[(&str, &[&str])] = &[
        (
            "/",
            &[
                "alpha",
                "Beta",
                "delta.x",
                "Epsilon+1",
                "eta@3",
                "gamma_7",
                "theta-4",
                "zeta=2",
            ],
        ),
        ("/eng/", &["alpha", "Epsilon+1", "gamma_7", "theta-4"]),
        ("/eng/tools/", &["gamma_7", "theta-4"]),
        ("/sales/", &["delta.x", "eta@3"]),
        ("/sales/west/", &["eta@3"]),
        ("/nobody/", &[]),
    ];

    for (prefix, expected) in cases {
        let (status, body) = call(&router, "ListRoles", &[("PathPrefix", prefix)]).await;
        assert_eq!(status, 200, "case {prefix:?}: expected 200, got {status}: {body}");
        let expected: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            listed_names(&body),
            expected,
            "case {prefix:?}: expected {expected:?}, got {:?}",
            listed_names(&body)
        );
    }

    // The default prefix lists every role, exactly as "/" does.
    let (_, defaulted) = call(&router, "ListRoles", &[]).await;
    let (_, rooted) = call(&router, "ListRoles", &[("PathPrefix", "/")]).await;
    assert_eq!(
        listed_names(&defaulted),
        listed_names(&rooted),
        "expected an absent PathPrefix to default to \"/\", got {:?}",
        listed_names(&defaulted)
    );
}

/// RULE: `PathPrefix` and `MaxItems` compose — a filtered enumeration pages through
/// exactly the matching resources, once each.
#[tokio::test]
async fn a_filtered_listing_pages_through_only_the_matching_roles() {
    let router = with_fixture_roles().await;

    // Rows: one prefix depth per row, each paged at every size that splits it.
    let cases: &[(&str, &[&str])] = &[
        ("/eng/", &["alpha", "Epsilon+1", "gamma_7", "theta-4"]),
        ("/eng/tools/", &["gamma_7", "theta-4"]),
        ("/sales/", &["delta.x", "eta@3"]),
    ];

    for (prefix, expected) in cases {
        for page_size in [1, 2, 3] {
            let case = format!("{prefix} with MaxItems={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            loop {
                let mut params = vec![
                    param("PathPrefix", prefix),
                    param("MaxItems", &page_size.to_string()),
                ];
                if let Some(token) = &marker {
                    params.push(param("Marker", token));
                }
                let (status, body) = call(&router, "ListRoles", &borrowed(&params)).await;
                assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
                seen.extend(listed_names(&body));
                if element(&body, "IsTruncated") == "true" {
                    marker = Some(element(&body, "Marker"));
                } else {
                    break;
                }
            }
            let expected: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
            assert_eq!(
                seen, expected,
                "case {case:?}: expected {expected:?}, got {seen:?}"
            );
        }
    }
}

/// One row of the withheld-attribute table: a role carrying `tags` and whichever
/// optional members the row names.
struct SubsetCase {
    name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    description: Option<&'static str>,
    max_session_duration: Option<&'static str>,
}

/// RULE: an IAM resource-listing operation reports a SUBSET of the resource's
/// attributes — the ones its documentation names are withheld from the listing and
/// available only from the single-resource read — while every other stored
/// attribute is reported.
///
/// `ListRoles`: "This operation does not return the following attributes, even
/// though they are an attribute of the returned object: PermissionsBoundary
/// RoleLastUsed Tags. To view all of the information for a role, see GetRole."
#[tokio::test]
async fn a_listing_withholds_the_attributes_its_documentation_excludes() {
    // Rows span the tag count (none, one, several) and the optional members the
    // role carries, so the rule is asserted on a role that HAS something to
    // withhold and on one that does not.
    let cases = &[
        SubsetCase {
            name: "untagged",
            tags: &[],
            description: None,
            max_session_duration: None,
        },
        SubsetCase {
            name: "one-tag",
            tags: &[("env", "prod")],
            description: Some("a described role"),
            max_session_duration: None,
        },
        SubsetCase {
            name: "three-tags",
            tags: &[("env", "dev"), ("team", "eng"), ("cost", "42")],
            description: Some("another described role"),
            max_session_duration: Some("7200"),
        },
    ];

    for SubsetCase {
        name: case,
        tags,
        description,
        max_session_duration,
    } in cases
    {
        let router = router();
        let role_name = format!("role-{}", case.replace('-', "_"));
        let mut params = vec![
            param("RoleName", &role_name),
            param("AssumeRolePolicyDocument", TRUST_POLICY),
        ];
        if let Some(description) = description {
            params.push(param("Description", description));
        }
        if let Some(duration) = max_session_duration {
            params.push(param("MaxSessionDuration", duration));
        }
        let (status, body) = call(&router, "CreateRole", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );

        if !tags.is_empty() {
            let mut params = vec![param("RoleName", &role_name)];
            for (position, (key, value)) in tags.iter().enumerate() {
                params.push(param(&format!("Tags.member.{}.Key", position + 1), key));
                params.push(param(&format!("Tags.member.{}.Value", position + 1), value));
            }
            let (status, body) = call(&router, "TagRole", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture TagRole must succeed, got {status}: {body}"
            );
        }

        // `GetRole` is the read that reports everything, including the tags.
        let (status, fetched) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(status, 200, "case {case:?}: GetRole must find the role, got {status}: {fetched}");
        let expected_tags: std::collections::BTreeMap<String, String> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags(&fetched),
            expected_tags,
            "case {case:?}: expected GetRole to report the stored tags, got {:?}",
            reported_tags(&fetched)
        );

        // The listing reports the role, WITHOUT the withheld attributes.
        let (status, listed) = call(&router, "ListRoles", &[]).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {listed}");
        assert_eq!(
            listed_names(&listed),
            vec![role_name.clone()],
            "case {case:?}: expected the one persisted role, got {:?}",
            listed_names(&listed)
        );
        // Asserted on the XML ELEMENT the withheld member serializes to, so an
        // unrelated member whose name merely contains one of these words cannot
        // satisfy or break the check.
        for withheld in ["<Tags>", "<PermissionsBoundary>", "<RoleLastUsed>"] {
            assert!(
                !listed.contains(withheld),
                "case {case:?}: the listing must withhold {withheld}, got {listed}"
            );
        }
        if !tags.is_empty() {
            assert!(
                fetched.contains("<Tags>"),
                "case {case:?}: GetRole is the read that reports the tags, got {fetched}"
            );
        }

        // Every other stored attribute IS reported: the required members (asserted
        // against `GetRole` above) plus the optional ones the role carries.
        assert_eq!(
            listed_roles(&listed)[0],
            listed_roles(&fetched)[0],
            "case {case:?}: expected the listing to report the stored record"
        );
        assert_eq!(
            elements(&listed, "AssumeRolePolicyDocument"),
            elements(&fetched, "AssumeRolePolicyDocument"),
            "case {case:?}: expected the stored trust policy in the listing, got {:?}",
            elements(&listed, "AssumeRolePolicyDocument")
        );
        assert_eq!(
            elements(&listed, "Description"),
            elements(&fetched, "Description"),
            "case {case:?}: expected the stored description in the listing, got {:?}",
            elements(&listed, "Description")
        );
        assert_eq!(
            elements(&listed, "MaxSessionDuration"),
            elements(&fetched, "MaxSessionDuration"),
            "case {case:?}: expected the stored MaxSessionDuration in the listing, got {:?}",
            elements(&listed, "MaxSessionDuration")
        );
    }
}

/// RULE: a listing is a REPORT — it changes nothing a later caller sees, however
/// many times and however it is paged.
#[tokio::test]
async fn listing_roles_changes_nothing_a_later_caller_sees() {
    let router = with_fixture_roles().await;
    let (_, before) = call(&router, "ListRoles", &[]).await;
    let baseline = listed_roles(&before);

    for params in [
        vec![param("MaxItems", "1")],
        vec![param("PathPrefix", "/eng/")],
        vec![param("PathPrefix", "/nobody/")],
        vec![param("MaxItems", "3"), param("PathPrefix", "/")],
    ] {
        let (status, body) = call(&router, "ListRoles", &borrowed(&params)).await;
        assert_eq!(status, 200, "case {params:?}: expected 200, got {status}: {body}");
    }

    let (_, after) = call(&router, "ListRoles", &[]).await;
    assert_eq!(
        listed_roles(&after),
        baseline,
        "expected the repeated listings to leave the store alone, got {:?}",
        listed_roles(&after)
    );
}
