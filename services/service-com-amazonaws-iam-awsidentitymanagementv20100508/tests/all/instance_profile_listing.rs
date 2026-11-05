// SPDX-License-Identifier: CC0-1.0

//! `ListInstanceProfiles`, end to end over the awsQuery wire, cross-read through
//! `GetInstanceProfile`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of instance profiles that
//! were PERSISTED by `CreateInstanceProfile`, never a handler called directly. The
//! awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in
//! this crate share, so a request is built and read exactly one way.
//!
//! Each suite is a TABLE whose rows differ along what the rule being asserted generalises
//! over: the page size, the path prefix depth, the name alphabet and case, the number of
//! persisted profiles, the tag count and whether a role is bound. No single request is
//! the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{account_id, call, call_signed_by, element, elements, error_code, router};

/// A trust policy with a single statement, so `CreateRole` accepts the one role this
/// suite binds into a profile (the EC2 service principal is the one an instance
/// profile's role carries).
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The fixture profile set: names spanning the whole `^[\w+=,.@-]+$` alphabet and both
/// cases (`smithy_query(query="operation", operation="CreateInstanceProfile")` for
/// `InstanceProfileName`), and paths spanning the root, a one-segment path and a nested
/// path, so a rule about paths is exercised at more than one depth.
const PROFILES: &[(&str, &str)] = &[
    ("alpha", "/eng/"),
    ("Beta", "/"),
    ("gamma_7", "/eng/tools/"),
    ("delta.x", "/sales/"),
    ("Epsilon+1", "/eng/"),
    ("zeta=2", "/"),
    ("eta@3", "/sales/west/"),
    ("theta-4", "/eng/tools/"),
];

/// The fixture profile names in the order the service enumerates them: ascending by the
/// case-folded name, which is the key the store is ordered on — IAM entity names "are not
/// distinguished by case" (`CreateInstanceProfile$InstanceProfileName`).
fn profiles_in_store_order() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = PROFILES.iter().map(|(name, _)| *name).collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// Create one instance profile, asserting the fixture create succeeded.
async fn create_profile(router: &MultiServiceRouter, name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateInstanceProfile",
        &[("InstanceProfileName", name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture CreateInstanceProfile must succeed, got {status}: {body}"
    );
}

/// Create every fixture profile, answering the router they were created in.
async fn with_fixture_profiles() -> MultiServiceRouter {
    let router = router();
    for (name, path) in PROFILES {
        create_profile(&router, name, path).await;
    }
    router
}

/// One instance profile as a `ListInstanceProfiles` page reports it, read off the
/// REQUIRED members of the `InstanceProfile` shape — `Path`, `InstanceProfileName`,
/// `InstanceProfileId`, `Arn`, `CreateDate` — each of which every member of the list
/// carries exactly once, so position identifies the profile.
#[derive(Debug, PartialEq, Eq)]
struct ListedProfile {
    instance_profile_name: String,
    path: String,
    instance_profile_id: String,
    arn: String,
    create_date: String,
}

/// The profiles a `ListInstanceProfiles` (or `GetInstanceProfile`) response reports, in
/// document order. Valid only for profiles holding no role, because a bound role carries
/// its own `Path`, `Arn` and `CreateDate`.
fn listed_profiles(body: &str) -> Vec<ListedProfile> {
    let names = elements(body, "InstanceProfileName");
    let paths = elements(body, "Path");
    let ids = elements(body, "InstanceProfileId");
    let arns = elements(body, "Arn");
    let dates = elements(body, "CreateDate");
    for (label, found) in [
        ("Path", paths.len()),
        ("InstanceProfileId", ids.len()),
        ("Arn", arns.len()),
        ("CreateDate", dates.len()),
    ] {
        assert_eq!(
            found,
            names.len(),
            "every listed profile must carry one <{label}>: got {found} for {} profiles: \
             {body}",
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
            |((((instance_profile_name, path), instance_profile_id), arn), create_date)| {
                ListedProfile {
                    instance_profile_name,
                    path,
                    instance_profile_id,
                    arn,
                    create_date,
                }
            },
        )
        .collect()
}

/// The profile names a `ListInstanceProfiles` response reports, in document order.
fn listed_names(body: &str) -> Vec<String> {
    elements(body, "InstanceProfileName")
}

/// RULE: a resource-listing operation enumerates the persisted resources, reporting for
/// each the record the account holds — the same identifiers a single-resource read of it
/// returns, never a value recomputed from the listing request.
#[tokio::test]
async fn list_instance_profiles_reports_the_stored_record_of_every_profile() {
    let router = with_fixture_profiles().await;

    let (status, body) = call(&router, "ListInstanceProfiles", &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let listed = listed_profiles(&body);
    let names: Vec<String> = listed
        .iter()
        .map(|p| p.instance_profile_name.clone())
        .collect();
    assert_eq!(
        names,
        profiles_in_store_order()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        "expected every persisted profile once, in store order, got {names:?}"
    );

    // Each row of this table is one persisted profile: what the listing says about it
    // must be what its own read says.
    for profile in &listed {
        let case = &profile.instance_profile_name;
        let (status, fetched) = call(
            &router,
            "GetInstanceProfile",
            &[("InstanceProfileName", case.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetInstanceProfile must find the profile, got {status}: {fetched}"
        );
        let stored = listed_profiles(&fetched);
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: expected the read to report one profile, got {stored:?}"
        );
        assert_eq!(
            profile, &stored[0],
            "case {case:?}: expected the listing to report the stored record, got {profile:?}"
        );
        let expected_path = PROFILES
            .iter()
            .find(|(name, _)| name == case)
            .map(|(_, path)| *path)
            .expect("a fixture profile");
        assert_eq!(
            profile.path, expected_path,
            "case {case:?}: expected path {expected_path:?}, got {:?}",
            profile.path
        );
        assert_eq!(
            profile.arn,
            format!(
                "arn:aws:iam::{}:instance-profile{}{}",
                account_id(),
                profile.path,
                profile.instance_profile_name
            ),
            "case {case:?}: expected the stored instance profile ARN, got {:?}",
            profile.arn
        );
    }
}

/// RULE: a listing of an empty collection is an empty list with `IsTruncated` false and
/// no `Marker`, not an error.
#[tokio::test]
async fn listing_an_account_holding_no_instance_profiles_is_an_empty_list() {
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
        let (status, body) = call(&router, "ListInstanceProfiles", params).await;
        assert_eq!(
            status, 200,
            "case {params:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            listed_names(&body),
            Vec::<String>::new(),
            "case {params:?}: expected no profiles, got {:?}",
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

/// RULE: a list operation enumerates the persisted entities in one total order, honours
/// `MaxItems` as a real page size, and hands out a `Marker` a follow-up call resumes from
/// without repeating or skipping an entity.
#[tokio::test]
async fn list_instance_profiles_pages_through_every_profile_exactly_once() {
    let router = with_fixture_profiles().await;
    let expected: Vec<String> = profiles_in_store_order()
        .into_iter()
        .map(str::to_string)
        .collect();

    // Rows span the page size: the published minimum, sizes that do and do not divide
    // the set, a size equal to the set, the published maximum, and the absent-parameter
    // default. `maxItemsType` is range 1..1000
    // (`smithy_query(query="operation", operation="ListInstanceProfiles")`).
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
            let (status, body) =
                call(&router, "ListInstanceProfiles", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );

            let page = listed_names(&body);
            if let Some(size) = page_size {
                assert!(
                    page.len() <= *size as usize,
                    "case {case:?}: expected at most {size} profiles on a page, got {}",
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
            "case {case:?}: expected every profile exactly once in store order, got {seen:?}"
        );
    }
}

/// RULE: `MaxItems` outside its published range is refused before the operation runs, so
/// a page size the model forbids never reaches the store; the refusal is HTTP 400
/// `ValidationError` and its message names the member and the bound it broke.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = with_fixture_profiles().await;
    // One past each published bound of `maxItemsType` (range 1..1000), with the bound
    // clause each one breaks.
    for (out_of_range, requirement) in [
        ("0", "Member must have value greater than or equal to 1"),
        ("-1", "Member must have value greater than or equal to 1"),
        ("1001", "Member must have value less than or equal to 1000"),
    ] {
        let (status, body) =
            call(&router, "ListInstanceProfiles", &[("MaxItems", out_of_range)]).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {out_of_range:?}: expected the query protocol's bound refusal code, got {body}"
        );
        let expected =
            format!("1 validation error detected: Value at 'maxItems' failed to satisfy constraint: {requirement}");
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {out_of_range:?}: expected {expected:?}, got {:?}",
            element(&body, "Message")
        );
        // Nothing was listed and nothing was persisted by the refused call.
        assert_eq!(
            listed_names(&body),
            Vec::<String>::new(),
            "case {out_of_range:?}: a refused listing reports no profile, got {body}"
        );
    }
    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) =
            call(&router, "ListInstanceProfiles", &[("MaxItems", in_range)]).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: a `Marker` this service did not issue for this collection names no position in
/// it, so the call is refused rather than silently answered from the beginning; a token
/// that does decode names a POSITION, which stays usable even when the entity it was cut
/// at is gone.
#[tokio::test]
async fn a_marker_this_collection_did_not_issue_is_refused() {
    let router = with_fixture_profiles().await;

    // Rows: a token that is not base64 at all, and two that decode to bytes carrying
    // none of this service's marker structure.
    for (status, body) in [
        call(
            &router,
            "ListInstanceProfiles",
            &[("Marker", "not-a-marker")],
        )
        .await,
        call(&router, "ListInstanceProfiles", &[("Marker", "zz")]).await,
        call(&router, "ListInstanceProfiles", &[("Marker", "0g")]).await,
    ] {
        assert_eq!(
            status, 400,
            "expected a foreign marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A marker issued while paging the ROLE collection is not a position in the instance
    // profile collection, and vice versa.
    for role in ["one", "two"] {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role:?}: the fixture CreateRole must succeed: {body}"
        );
    }
    let (status, body) = call(&router, "ListRoles", &[("MaxItems", "1")]).await;
    assert_eq!(
        status, 200,
        "expected a truncated role page, got {status}: {body}"
    );
    let role_marker = element(&body, "Marker");
    let (status, body) = call(
        &router,
        "ListInstanceProfiles",
        &[("Marker", role_marker.as_str())],
    )
    .await;
    assert_eq!(
        status, 400,
        "expected a role marker to be refused by ListInstanceProfiles, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationError",
        "expected ValidationError, got {:?}",
        error_code(&body)
    );

    let (status, body) = call(&router, "ListInstanceProfiles", &[("MaxItems", "1")]).await;
    assert_eq!(
        status, 200,
        "expected a truncated profile page, got {status}: {body}"
    );
    let profile_marker = element(&body, "Marker");
    let first = element(&body, "InstanceProfileName");
    let (status, body) = call(&router, "ListRoles", &[("Marker", profile_marker.as_str())]).await;
    assert_eq!(
        status, 400,
        "expected a profile marker to be refused by ListRoles, got {status}: {body}"
    );

    // A marker cut at a profile that is then deleted still resumes from that position:
    // the rest of the order follows it, minus the deleted profile.
    let (status, body) = call(
        &router,
        "DeleteInstanceProfile",
        &[("InstanceProfileName", first.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the first profile to be deleted, got {status}: {body}"
    );
    let (status, body) = call(
        &router,
        "ListInstanceProfiles",
        &[("Marker", profile_marker.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected a stale marker to still resume, got {status}: {body}"
    );
    let expected: Vec<String> = profiles_in_store_order()
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

/// RULE: `PathPrefix` selects exactly the resources whose path begins with it, and an
/// absent `PathPrefix` "defaults to a slash (/), listing all instance profiles".
#[tokio::test]
async fn list_instance_profiles_filters_by_path_prefix() {
    let router = with_fixture_profiles().await;

    // Rows span the prefix depth: the root, a one-segment prefix, a nested prefix, a
    // prefix that selects exactly one profile, a partial segment that still matches by
    // prefix, and one that selects none.
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
        ("/sal", &["delta.x", "eta@3"]),
        ("/nobody/", &[]),
    ];

    for (prefix, expected) in cases {
        let (status, body) = call(
            &router,
            "ListInstanceProfiles",
            &[("PathPrefix", prefix)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {prefix:?}: expected 200, got {status}: {body}"
        );
        let expected: Vec<String> = expected.iter().map(|n| (*n).to_string()).collect();
        assert_eq!(
            listed_names(&body),
            expected,
            "case {prefix:?}: expected {expected:?}, got {:?}",
            listed_names(&body)
        );
    }

    // The default prefix lists every profile, exactly as "/" does.
    let (_, defaulted) = call(&router, "ListInstanceProfiles", &[]).await;
    let (_, rooted) = call(&router, "ListInstanceProfiles", &[("PathPrefix", "/")]).await;
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
async fn a_filtered_listing_pages_through_only_the_matching_profiles() {
    let router = with_fixture_profiles().await;

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
                let (status, body) =
                    call(&router, "ListInstanceProfiles", &borrowed(&params)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status}: {body}"
                );
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

/// One row of the withheld-attribute table: a profile carrying the tags the row names,
/// with or without a role bound to it.
struct SubsetCase {
    name: &'static str,
    tags: &'static [(&'static str, &'static str)],
    bind_role: bool,
}

/// RULE: an IAM resource-listing operation reports a SUBSET of the resource's attributes
/// — the ones its documentation names are withheld from the listing and available only
/// from the single-resource read — while every other stored attribute is reported.
///
/// `ListInstanceProfiles`: "this operation does not return tags, even though they are an
/// attribute of the returned object. To view all of the information for an instance
/// profile, see GetInstanceProfile." The bound role is NOT among them, so it stays.
#[tokio::test]
async fn a_listing_withholds_the_attributes_its_documentation_excludes() {
    // Rows span the tag count (none, one, several) and whether a role is bound, so the
    // rule is asserted on a profile that HAS something to withhold and on one that does
    // not.
    let cases = &[
        SubsetCase {
            name: "untagged-no-role",
            tags: &[],
            bind_role: false,
        },
        SubsetCase {
            name: "one-tag-no-role",
            tags: &[("env", "prod")],
            bind_role: false,
        },
        SubsetCase {
            name: "one-tag-with-role",
            tags: &[("env", "dev")],
            bind_role: true,
        },
        SubsetCase {
            name: "three-tags-with-role",
            tags: &[("env", "dev"), ("team", "eng"), ("cost", "42")],
            bind_role: true,
        },
    ];

    for SubsetCase {
        name: case,
        tags,
        bind_role,
    } in cases
    {
        let router = router();
        let profile_name = format!("profile-{}", case.replace('-', "_"));
        let mut params = vec![param("InstanceProfileName", &profile_name)];
        for (position, (key, value)) in tags.iter().enumerate() {
            params.push(param(&format!("Tags.member.{}.Key", position + 1), key));
            params.push(param(&format!("Tags.member.{}.Value", position + 1), value));
        }
        let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture create must succeed, got {status}: {body}"
        );

        if *bind_role {
            let (status, body) = call(
                &router,
                "CreateRole",
                &[
                    ("RoleName", "bound-role"),
                    ("AssumeRolePolicyDocument", TRUST_POLICY),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture CreateRole must succeed, got {status}: {body}"
            );
            let (status, body) = call(
                &router,
                "AddRoleToInstanceProfile",
                &[
                    ("InstanceProfileName", profile_name.as_str()),
                    ("RoleName", "bound-role"),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture binding must succeed, got {status}: {body}"
            );
        }

        // `GetInstanceProfile` is the read that reports everything, including the tags.
        let (status, fetched) = call(
            &router,
            "GetInstanceProfile",
            &[("InstanceProfileName", profile_name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the read must find the profile, got {status}: {fetched}"
        );
        let expected_tags: std::collections::BTreeMap<String, String> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags(&fetched),
            expected_tags,
            "case {case:?}: expected the read to report the stored tags, got {:?}",
            reported_tags(&fetched)
        );

        let (status, listed) = call(&router, "ListInstanceProfiles", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            listed_names(&listed),
            vec![profile_name.clone()],
            "case {case:?}: expected the one persisted profile, got {:?}",
            listed_names(&listed)
        );
        // Asserted on the XML ELEMENT the withheld member serializes to, so an unrelated
        // member whose name merely contains the word cannot satisfy or break the check.
        assert!(
            !listed.contains("<Tags>"),
            "case {case:?}: the listing must withhold <Tags>, got {listed}"
        );
        if !tags.is_empty() {
            assert!(
                fetched.contains("<Tags>"),
                "case {case:?}: the read is what reports the tags, got {fetched}"
            );
        }

        // Every other stored attribute IS reported, including the bound role, which the
        // listing resolves from the role the account holds.
        let expected_roles: Vec<String> = if *bind_role {
            vec!["bound-role".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            elements(&listed, "RoleName"),
            expected_roles,
            "case {case:?}: expected the listing to report {expected_roles:?}, got {:?}",
            elements(&listed, "RoleName")
        );
        for member in ["InstanceProfileId", "Arn", "Path", "CreateDate"] {
            assert_eq!(
                elements(&listed, member),
                elements(&fetched, member),
                "case {case:?}: expected the listing to report the stored <{member}>, got \
                 {:?}",
                elements(&listed, member)
            );
        }
    }
}

/// RULE: a listing is a REPORT — it changes nothing a later caller sees, however many
/// times and however it is paged.
#[tokio::test]
async fn listing_instance_profiles_changes_nothing_a_later_caller_sees() {
    let router = with_fixture_profiles().await;
    let (_, before) = call(&router, "ListInstanceProfiles", &[]).await;
    let baseline = listed_profiles(&before);

    for params in [
        vec![param("MaxItems", "1")],
        vec![param("PathPrefix", "/eng/")],
        vec![param("PathPrefix", "/nobody/")],
        vec![param("MaxItems", "3"), param("PathPrefix", "/")],
    ] {
        let (status, body) = call(&router, "ListInstanceProfiles", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {params:?}: expected 200, got {status}: {body}"
        );
    }

    let (_, after) = call(&router, "ListInstanceProfiles", &[]).await;
    assert_eq!(
        listed_profiles(&after),
        baseline,
        "expected the repeated listings to leave the store alone, got {:?}",
        listed_profiles(&after)
    );
}

/// RULE: IAM decides a call against the signing principal's identity-based policies
/// before the operation reports anything — an `Allow` about the request serves it, a
/// `Deny` about it refuses it whatever any `Allow` says, and a principal with neither is
/// refused with HTTP 403 `AccessDenied`.
#[tokio::test]
async fn listing_instance_profiles_is_decided_against_the_callers_identity_policies() {
    let allow_all = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListInstanceProfiles","Resource":"*"}]}"#;
    let deny_all = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListInstanceProfiles","Resource":"*"}]}"#;
    let allow_eng = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListInstanceProfiles","Resource":"arn:aws:iam::{}:instance-profile/eng/*"}}]}}"#,
        account_id()
    );

    // Rows: the documents in force, the path prefix the request lists, and the status the
    // rule requires — a grant, no grant at all, a grant overridden by a deny, and a grant
    // scoped to one path prefix asked for both inside and outside that scope.
    let cases: &[(&str, &[&str], Option<&str>, u16)] = &[
        ("an Allow naming the action", &[allow_all], None, 200),
        ("no document at all", &[], None, 403),
        (
            "a Deny over the Allow",
            &[allow_all, deny_all],
            None,
            403,
        ),
        (
            "an Allow scoped to the prefix the request lists",
            &[allow_eng.as_str()],
            Some("/eng/"),
            200,
        ),
        (
            "an Allow scoped to a prefix the request does not list",
            &[allow_eng.as_str()],
            None,
            403,
        ),
    ];

    for (row, (case, documents, path_prefix, expected)) in cases.iter().enumerate() {
        let router = with_fixture_profiles().await;
        let user_name = format!("caller{row}");
        let (status, body) = call(&router, "CreateUser", &[("UserName", &user_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
        );
        for (position, document) in documents.iter().enumerate() {
            let policy_name = format!("policy{position}");
            let (status, body) = call(
                &router,
                "PutUserPolicy",
                &[
                    ("UserName", &user_name),
                    ("PolicyName", &policy_name),
                    ("PolicyDocument", document),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture PutUserPolicy must succeed, got {status}: {body}"
            );
        }
        let (status, key) = call(&router, "CreateAccessKey", &[("UserName", &user_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
        );
        let access_key_id = element(&key, "AccessKeyId");

        let params: Vec<(&str, &str)> = match path_prefix {
            Some(prefix) => vec![("PathPrefix", *prefix)],
            None => Vec::new(),
        };
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListInstanceProfiles",
            &params,
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status}: {body}"
        );
        if *expected == 403 {
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}",
                error_code(&body)
            );
            assert_eq!(
                listed_names(&body),
                Vec::<String>::new(),
                "case {case:?}: a refused listing reports no profile, got {body}"
            );
        } else {
            assert!(
                !listed_names(&body).is_empty(),
                "case {case:?}: expected the served listing to report the persisted \
                 profiles, got {body}"
            );
        }
    }
}
