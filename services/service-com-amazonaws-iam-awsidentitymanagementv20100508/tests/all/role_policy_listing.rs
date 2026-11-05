// SPDX-License-Identifier: CC0-1.0

//! `ListRolePolicies`, end to end over the awsQuery wire.
//!
//! The operation "lists the names of the inline policies that are embedded in the
//! specified IAM role" and "if there are no inline policies embedded with the
//! specified role, the operation returns an empty list"
//! (`smithy_query(query="operation", operation="ListRolePolicies")`), paginated by
//! `MaxItems` and `Marker`. Every assertion here is about what a client on the
//! socket sees after the policies were PERSISTED by `PutRolePolicy`, never about a
//! handler called directly, and each test is a TABLE whose rows differ along what
//! the rule generalises over: the number of embedded policies, the page size, the
//! case and alphabet of the role and policy names, and the role's path.
//!
//! The awsQuery harness (router, call, element, elements, error_code) is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// A trust policy with a single statement — every role this suite creates needs
/// one, and the trust policy is not what is under test here.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The number of names a list answers when the request carries no `MaxItems`:
/// `maxItemsType`'s documentation says "if you do not include this parameter, the
/// number of items defaults to 100".
const DEFAULT_MAX_ITEMS: usize = 100;

/// The document every policy this suite embeds carries: the listing reports NAMES,
/// so the document is not what is under test here.
const EMBEDDED_DOCUMENT: &str =
    r#"{"Statement":[{"Sid":"s","Effect":"Allow","Action":"s3:*"}]}"#;

/// Create a role, failing the test when the create this suite depends on does not
/// succeed.
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
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// Embed one inline policy in `role_name`, failing the test when the put this
/// suite depends on does not succeed.
async fn put_policy(router: &MultiServiceRouter, role_name: &str, policy_name: &str) {
    let (status, body) = call(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", policy_name),
            ("PolicyDocument", EMBEDDED_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}/{policy_name:?}: PutRolePolicy must succeed, got {status}: {body}"
    );
}

/// One `ListRolePolicies` call.
async fn list(router: &MultiServiceRouter, params: &[(&str, &str)]) -> (u16, String) {
    call(router, "ListRolePolicies", params).await
}

/// The policy names one page reports: the list's members, in document order.
fn page_names(body: &str) -> Vec<String> {
    elements(body, "member")
}

/// Page through `role_name`'s inline policies with the given page size, answering
/// every name seen in order. Fails the test on a page that breaks the pagination
/// contract (a page larger than the requested size, a truncated page with no
/// marker, a final page carrying one, or pagination that does not terminate).
async fn page_through(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    page_size: Option<usize>,
    expected_total: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;

    loop {
        let mut params: Vec<(String, String)> = vec![("RoleName".to_string(), role_name.to_string())];
        if let Some(size) = page_size {
            params.push(("MaxItems".to_string(), size.to_string()));
        }
        if let Some(token) = &marker {
            params.push(("Marker".to_string(), token.clone()));
        }
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let (status, body) = list(router, &borrowed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from ListRolePolicies, got {status}: {body}"
        );

        let page = page_names(&body);
        let limit = page_size.unwrap_or(DEFAULT_MAX_ITEMS);
        assert!(
            page.len() <= limit,
            "case {case:?}: expected at most {limit} names on a page, got {}: {page:?}",
            page.len()
        );
        seen.extend(page);

        let truncated = element(&body, "IsTruncated");
        let page_marker = elements(&body, "Marker");
        if truncated == "true" {
            assert_eq!(
                page_marker.len(),
                1,
                "case {case:?}: a truncated page must carry a Marker, got {page_marker:?}"
            );
            marker = Some(page_marker.into_iter().next().expect("a marker"));
        } else {
            assert_eq!(
                truncated, "false",
                "case {case:?}: expected IsTruncated true or false, got {truncated:?}"
            );
            assert!(
                page_marker.is_empty(),
                "case {case:?}: a final page must carry no Marker, got {page_marker:?}"
            );
            break;
        }

        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: pagination did not terminate after {pages} pages, saw {seen:?}"
        );
    }

    seen
}

/// The names in the order the service enumerates them: ascending by the
/// case-folded name, which is the key the store is ordered on.
fn in_store_order(names: &[String]) -> Vec<String> {
    let mut ordered = names.to_vec();
    ordered.sort_by_key(|name| name.to_ascii_lowercase());
    ordered
}

/// One role of the fixture: its name, its path, and the inline policy names
/// embedded in it.
struct RoleCase {
    case: &'static str,
    role_name: &'static str,
    path: &'static str,
    policy_names: &'static [&'static str],
}

/// Rows spanning the counts a listing generalises over — no policy at all, exactly
/// one, a handful, and more than the smallest page sizes — across the root path, a
/// one-segment path and a nested path, with role and policy names drawn from the
/// whole `^[\w+=,.@-]+$` alphabet
/// (`smithy_query(query="operation", operation="ListRolePolicies")`) in mixed case
/// and in an insertion order that is NOT the store's order.
const ROLES: &[RoleCase] = &[
    RoleCase {
        case: "a role holding no inline policy",
        role_name: "Empty",
        path: "/",
        policy_names: &[],
    },
    RoleCase {
        case: "a role holding exactly one",
        role_name: "single_1",
        path: "/eng/",
        policy_names: &["Only.One"],
    },
    RoleCase {
        case: "a role holding a handful, embedded out of order",
        role_name: "handful.2",
        path: "/eng/tools/",
        policy_names: &["zeta@6", "alpha", "Mid+3", "beta_2", "Gamma=4"],
    },
    RoleCase {
        case: "a role holding more than the small page sizes",
        role_name: "many+3",
        path: "/sales/west/",
        policy_names: &[
            "p-01", "p-02", "p-03", "p-04", "p-05", "p-06", "p-07", "p-08", "p-09", "p-10", "p-11",
            "p-12",
        ],
    },
];

/// Create every fixture role and embed its policies, answering the router they
/// live in.
async fn with_fixture_roles() -> MultiServiceRouter {
    let router = router();
    for RoleCase {
        role_name,
        path,
        policy_names,
        ..
    } in ROLES
    {
        create_role(&router, role_name, path).await;
        for policy_name in *policy_names {
            put_policy(&router, role_name, policy_name).await;
        }
    }
    router
}

/// RULE: the operation reports the names of exactly the inline policies embedded
/// in the named role, in one total order, honouring `MaxItems` as a real page size
/// and handing out a `Marker` a follow-up call resumes from without repeating or
/// skipping a name; a role with no embedded policy answers an empty list.
#[tokio::test]
async fn listing_pages_through_every_embedded_policy_exactly_once() {
    let router = with_fixture_roles().await;

    // Rows span the page size: the published minimum, sizes that do and do not
    // divide each set, a size equal to the largest set, the published maximum, and
    // the absent-parameter default. `maxItemsType` is range 1..1000
    // (smithy_query(query="operation", operation="ListRolePolicies")).
    let page_sizes: &[Option<usize>] = &[Some(1), Some(2), Some(3), Some(5), Some(12), Some(1000), None];

    for RoleCase {
        case,
        role_name,
        policy_names,
        ..
    } in ROLES
    {
        let expected = in_store_order(
            &policy_names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
        );
        for page_size in page_sizes {
            let full_case = format!("{case} with MaxItems={page_size:?}");
            let seen =
                page_through(&router, &full_case, role_name, *page_size, expected.len()).await;
            assert_eq!(
                seen, expected,
                "case {full_case:?}: expected every embedded policy exactly once in store \
                 order, got {seen:?}"
            );
        }
    }
}

/// RULE: a role's listing reports only its OWN embedded policies — the same policy
/// name embedded in two roles is reported by each, and a name embedded in one is
/// absent from the other's listing.
#[tokio::test]
async fn a_listing_covers_only_the_named_role() {
    // Rows: two roles sharing one policy name, each also holding a name of its own,
    // plus a third role holding none of them.
    let shared = "Shared";
    let roles = [
        ("first", "/", vec![shared, "first-only"]),
        ("second", "/eng/", vec![shared, "second-only"]),
        ("third", "/sales/", vec![]),
    ];

    let router = router();
    for (role_name, path, policies) in &roles {
        create_role(&router, role_name, path).await;
        for policy_name in policies {
            put_policy(&router, role_name, policy_name).await;
        }
    }

    for (role_name, _, policies) in &roles {
        let (status, body) = list(&router, &[("RoleName", role_name)]).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: expected 200, got {status}: {body}"
        );
        let expected = in_store_order(
            &policies
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
        );
        assert_eq!(
            page_names(&body),
            expected,
            "case {role_name:?}: expected only this role's own policies, got {body}"
        );

        // No other role's own name leaks into this role's listing.
        for (other, _, other_policies) in &roles {
            if other == role_name {
                continue;
            }
            for policy_name in other_policies {
                if *policy_name == shared {
                    continue;
                }
                assert!(
                    !page_names(&body).iter().any(|seen| seen == policy_name),
                    "case {role_name:?}: {policy_name:?} belongs to {other:?}, got {body}"
                );
            }
        }
    }
}

/// RULE: IAM entity and policy names "are not distinguished by case", so a listing
/// reaches the role however the request spells it and reports each policy under the
/// name the account holds — the one the latest put chose.
#[tokio::test]
async fn names_are_matched_without_regard_to_case_and_reported_as_held() {
    let router = router();
    create_role(&router, "CaseRole", "/eng/").await;
    for policy_name in ["Embedded", "Second"] {
        put_policy(&router, "CaseRole", policy_name).await;
    }

    // Rows: the role name as created, folded down, folded up, and inverted.
    for (case, spelling) in [
        ("as created", "CaseRole"),
        ("all lower case", "caserole"),
        ("all upper case", "CASEROLE"),
        ("inverted case", "cASEROLE"),
    ] {
        let (status, body) = list(&router, &[("RoleName", spelling)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a listing must reach the role whatever the case, got {status}: {body}"
        );
        assert_eq!(
            page_names(&body),
            vec!["Embedded".to_string(), "Second".to_string()],
            "case {case:?}: expected the names the account holds, got {body}"
        );
    }

    // A put of an existing name in another case REPLACES that policy rather than
    // adding a second one, and the listing reports the name the latest put chose.
    put_policy(&router, "caserole", "EMBEDDED").await;
    let (status, body) = list(&router, &[("RoleName", "CaseRole")]).await;
    assert_eq!(status, 200, "expected 200 after a repeat put, got {status}: {body}");
    assert_eq!(
        page_names(&body),
        vec!["EMBEDDED".to_string(), "Second".to_string()],
        "a repeat put must replace the name rather than add one, got {body}"
    );
}

/// RULE: the listing is computed from persisted state, so a policy embedded after a
/// listing appears in the next one and a policy deleted before it is gone.
#[tokio::test]
async fn the_listing_follows_the_stores_content() {
    let router = router();
    create_role(&router, "Mutating", "/").await;

    // Rows: the empty role, after each embed, and after each delete.
    let (status, body) = list(&router, &[("RoleName", "Mutating")]).await;
    assert_eq!(status, 200, "expected 200 on an empty role, got {status}: {body}");
    assert!(
        page_names(&body).is_empty(),
        "a role with no embedded policy must answer an empty list, got {body}"
    );

    let embedded = ["a-one", "b-two", "c-three"];
    for (index, policy_name) in embedded.iter().enumerate() {
        put_policy(&router, "Mutating", policy_name).await;
        let (status, body) = list(&router, &[("RoleName", "Mutating")]).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected 200, got {status}: {body}"
        );
        let expected: Vec<String> = embedded[..=index].iter().map(|n| n.to_string()).collect();
        assert_eq!(
            page_names(&body),
            expected,
            "case {policy_name:?}: expected every embed so far, got {body}"
        );
    }

    for (index, policy_name) in embedded.iter().enumerate() {
        let (status, body) = call(
            &router,
            "DeleteRolePolicy",
            &[("RoleName", "Mutating"), ("PolicyName", policy_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: DeleteRolePolicy must succeed, got {status}: {body}"
        );
        let (status, body) = list(&router, &[("RoleName", "Mutating")]).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected 200 after a delete, got {status}: {body}"
        );
        let expected: Vec<String> = embedded[index + 1..].iter().map(|n| n.to_string()).collect();
        assert_eq!(
            page_names(&body),
            expected,
            "case {policy_name:?}: a deleted policy must be gone from the listing, got {body}"
        );
    }
}

/// RULE: a listing of a role the account does not hold references a resource entity
/// that does not exist, so it is refused with the modelled `NoSuchEntityException`
/// (HTTP 404, awsQuery code `NoSuchEntity`).
#[tokio::test]
async fn an_absent_role_is_not_found() {
    // Rows: a role nobody created, a near miss on an existing name, a name spanning
    // the whole alphabet, and a name at each published length bound of
    // `roleNameType` (len 1..64).
    let longest = "r".repeat(64);
    let absent = [
        ("never created", "ghost"),
        ("a near miss on an existing name", "presentx"),
        ("a name spanning the alphabet", "n0_+=,.@-"),
        ("the shortest name the model admits", "r"),
        ("the longest name the model admits", longest.as_str()),
    ];

    let router = router();
    create_role(&router, "present", "/").await;
    put_policy(&router, "present", "Embedded").await;

    for (case, role_name) in absent {
        let (status, body) = list(&router, &[("RoleName", role_name)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a listing of an absent role must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // A name past the published bound is refused before the operation runs.
    let too_long = "r".repeat(65);
    let (status, body) = list(&router, &[("RoleName", too_long.as_str())]).await;
    assert_eq!(
        status, 400,
        "a name past the published length bound must be refused as invalid, got {status}: {body}"
    );
}

/// RULE: a `Marker` this service did not issue for THIS role's inline policies
/// names no position in the store, so the call is refused rather than silently
/// answered from the beginning; a token that does decode names a POSITION, which
/// stays usable even when the policy it was cut at is gone.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_for_the_role_is_refused() {
    let router = router();
    create_role(&router, "Paged", "/").await;
    create_role(&router, "Other", "/eng/").await;
    for policy_name in ["one", "two", "three"] {
        put_policy(&router, "Paged", policy_name).await;
        put_policy(&router, "Other", policy_name).await;
    }

    // A marker issued while paging THIS role's policies.
    let (status, body) = list(&router, &[("RoleName", "Paged"), ("MaxItems", "1")]).await;
    assert_eq!(status, 200, "expected the first page, got {status}: {body}");
    let own_marker = element(&body, "Marker");
    // A marker issued while paging ANOTHER role's policies, and one issued by a
    // different collection entirely.
    let (_, other_body) = list(&router, &[("RoleName", "Other"), ("MaxItems", "1")]).await;
    let other_marker = element(&other_body, "Marker");
    for user_name in ["first-user", "second-user"] {
        let (status, body) = call(&router, "CreateUser", &[("UserName", user_name)]).await;
        assert_eq!(
            status, 200,
            "case {user_name:?}: CreateUser must succeed, got {status}: {body}"
        );
    }
    let (_, users_body) = call(&router, "ListUsers", &[("MaxItems", "1")]).await;
    let users_marker = elements(&users_body, "Marker");
    assert_eq!(
        users_marker.len(),
        1,
        "the user listing must hand out a marker to compare against, got {users_body}"
    );

    let mut refused: Vec<(&str, String)> = vec![
        ("not a token at all", "not-a-marker".to_string()),
        ("two base64 characters", "zz".to_string()),
        ("a token outside the alphabet", "0g".to_string()),
        ("another role's marker", other_marker),
    ];
    if let Some(users_marker) = users_marker.into_iter().next() {
        refused.push(("another collection's marker", users_marker));
    }

    for (case, marker) in &refused {
        let (status, body) = list(
            &router,
            &[("RoleName", "Paged"), ("Marker", marker.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign marker must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the marker to be refused as invalid, got {body}"
        );
    }

    // This role's own marker resumes the page even when the policy it was cut at
    // has since been deleted: the rest of the order follows the position.
    let first = page_names(&body)
        .into_iter()
        .next()
        .expect("the first page carries one name");
    let (status, deleted) = call(
        &router,
        "DeleteRolePolicy",
        &[("RoleName", "Paged"), ("PolicyName", first.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the first policy to be deleted, got {status}: {deleted}"
    );
    let (status, body) = list(
        &router,
        &[("RoleName", "Paged"), ("Marker", own_marker.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected a stale marker to still resume, got {status}: {body}"
    );
    let expected = in_store_order(
        &["one", "two", "three"]
            .into_iter()
            .filter(|name| *name != first)
            .map(|name| name.to_string())
            .collect::<Vec<String>>(),
    );
    assert_eq!(
        page_names(&body),
        expected,
        "expected the rest of the order after the marker's position, got {body}"
    );
}

/// RULE: `MaxItems` outside its published range is refused before the operation
/// runs, so a page size the model forbids never reaches the store.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    create_role(&router, "Sized", "/").await;
    put_policy(&router, "Sized", "Embedded").await;

    // One past each published bound of `maxItemsType` (range 1..1000).
    for out_of_range in ["0", "1001", "-1"] {
        let (status, body) = list(
            &router,
            &[("RoleName", "Sized"), ("MaxItems", out_of_range)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = list(&router, &[("RoleName", "Sized"), ("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            page_names(&body),
            vec!["Embedded".to_string()],
            "case {in_range:?}: expected the embedded policy, got {body}"
        );
    }
}
