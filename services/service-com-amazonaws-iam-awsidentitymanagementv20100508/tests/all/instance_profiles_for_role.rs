// SPDX-License-Identifier: CC0-1.0

//! `ListInstanceProfilesForRole`, end to end over the awsQuery wire, cross-read through
//! `GetInstanceProfile` and against the bindings `AddRoleToInstanceProfile` persisted.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of bindings that were
//! PERSISTED by `AddRoleToInstanceProfile`, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in this
//! crate share, so a request is built and read exactly one way.
//!
//! Each suite is a TABLE whose rows differ along what the rule being asserted generalises
//! over: the number of profiles bound to the role (none, one, several), the name alphabet
//! and case, the profile path depth, the page size, and whether the role exists at all. No
//! single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::user_lifecycle::{account_id, call, call_signed_by, element, elements, error_code, router};

/// A trust policy with a single statement, so `CreateRole` accepts every fixture role (the
/// EC2 service principal is the one an instance profile's role carries).
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The fixture roles: names spanning the `^[\w+=,.@-]+$` alphabet and both cases
/// (`smithy_query(query="operation", operation="ListInstanceProfilesForRole")` for
/// `RoleName`). The last one is bound to no profile at all.
const ROLES: &[&str] = &["alpha-role", "Beta_Role", "gamma.role@3", "unbound-role"];

/// The fixture instance profiles: the name, the path and the role the fixture binds into
/// the profile, if any. The paths span the account root, a one-segment path and a nested
/// path; the bindings span a role holding several profiles, one holding two, one holding a
/// single profile, a profile holding no role, and a binding REQUESTED in a different case
/// from the role's own spelling.
const PROFILES: &[(&str, &str, Option<&str>)] = &[
    ("p-one", "/", Some("alpha-role")),
    ("P-two", "/eng/", Some("alpha-role")),
    ("p-three", "/eng/tools/", Some("ALPHA-ROLE")),
    ("p-four", "/", Some("Beta_Role")),
    ("p-five", "/sales/", None),
    ("p_six+7", "/", Some("gamma.role@3")),
    ("p-seven", "/eng/", Some("alpha-role")),
    ("p-eight", "/eng/tools/", Some("beta_role")),
];

/// The fixture profile names bound to `role_name`, in the order the service enumerates
/// them: ascending by the case-folded profile name, which is the key the instance profile
/// collection is ordered on — IAM entity names "are not distinguished by case"
/// (`CreateInstanceProfile$InstanceProfileName`). The role a binding named is matched the
/// same way, so the case a binding was requested in does not change which role it is.
fn bound_profiles(role_name: &str) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = PROFILES
        .iter()
        .filter(|(_, _, bound)| bound.is_some_and(|bound| bound.eq_ignore_ascii_case(role_name)))
        .map(|(name, _, _)| *name)
        .collect();
    names.sort_by_key(|name| name.to_ascii_lowercase());
    names
}

/// The path the fixture created the profile `name` under.
fn fixture_path(name: &str) -> &'static str {
    PROFILES
        .iter()
        .find(|(profile, _, _)| *profile == name)
        .map(|(_, path, _)| *path)
        .expect("a fixture profile")
}

/// Create every fixture role and profile and persist every fixture binding, answering the
/// router they were created in.
async fn with_fixtures() -> MultiServiceRouter {
    let router = router();
    for role in ROLES {
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
            "case {role:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );
    }
    for (name, path, bound) in PROFILES {
        let (status, body) = call(
            &router,
            "CreateInstanceProfile",
            &[("InstanceProfileName", name), ("Path", path)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the fixture CreateInstanceProfile must succeed, got {status}: {body}"
        );
        if let Some(role) = bound {
            let (status, body) = call(
                &router,
                "AddRoleToInstanceProfile",
                &[("InstanceProfileName", name), ("RoleName", role)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: the fixture AddRoleToInstanceProfile must succeed, got \
                 {status}: {body}"
            );
        }
    }
    router
}

/// The profile names a response reports, in document order.
fn listed_names(body: &str) -> Vec<String> {
    elements(body, "InstanceProfileName")
}

/// The profile names a response reports, as owned `String`s for comparison with a fixture
/// row.
fn expected_names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// RULE: an operation that lists the resources associated with one entity reports exactly
/// the persisted resources whose stored association names that entity — each as the record
/// the account holds — and nothing else, in the collection's one stable order.
#[tokio::test]
async fn reports_exactly_the_profiles_bound_to_the_named_role() {
    let router = with_fixtures().await;

    // Rows: one per fixture role, spanning the number of profiles bound to it (several,
    // two, one, none).
    for role in ROLES {
        let expected = bound_profiles(role);
        let (status, body) =
            call(&router, "ListInstanceProfilesForRole", &[("RoleName", role)]).await;
        assert_eq!(
            status, 200,
            "case {role:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            listed_names(&body),
            expected_names(&expected),
            "case {role:?}: expected {expected:?}, got {:?}",
            listed_names(&body)
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {role:?}: a complete page is not truncated, got {:?}",
            element(&body, "IsTruncated")
        );
        assert_eq!(
            elements(&body, "Marker"),
            Vec::<String>::new(),
            "case {role:?}: an untruncated page carries no Marker, got {:?}",
            elements(&body, "Marker")
        );
        // The role each reported profile carries is the one asked about, reported under
        // the name the ACCOUNT holds it as rather than the case a binding was requested
        // in.
        assert_eq!(
            elements(&body, "RoleName"),
            expected_names(&vec![*role; expected.len()]),
            "case {role:?}: expected every reported profile to carry the named role, got \
             {:?}",
            elements(&body, "RoleName")
        );

        // Each listed profile must be the record its own single-resource read reports.
        for name in &expected {
            let (status, fetched) = call(
                &router,
                "GetInstanceProfile",
                &[("InstanceProfileName", name)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: GetInstanceProfile must find the profile, got {status}: \
                 {fetched}"
            );
            for member in ["InstanceProfileId", "Arn", "Path", "CreateDate"] {
                let stored = elements(&fetched, member);
                let listed = elements(&body, member);
                assert!(
                    stored.iter().all(|value| listed.contains(value)),
                    "case {name:?}: expected the listing to report the stored <{member}> \
                     {stored:?}, got {listed:?}"
                );
            }
            let path = fixture_path(name);
            assert!(
                elements(&body, "Arn")
                    .contains(&format!(
                        "arn:aws:iam::{}:instance-profile{path}{name}",
                        account_id()
                    )),
                "case {name:?}: expected the stored instance profile ARN, got {:?}",
                elements(&body, "Arn")
            );
        }
    }
}

/// RULE: a listing of an entity's associations is an empty list with `IsTruncated` false
/// and no `Marker` when the entity has none — "if there are none, the operation returns an
/// empty list" — not an error and not the whole collection.
#[tokio::test]
async fn a_role_holding_no_instance_profile_lists_an_empty_page() {
    let router = with_fixtures().await;

    // Rows span the parameters a caller may add to such a listing, and the published
    // bounds of `maxItemsType` (range 1..1000).
    let cases: &[&[(&str, &str)]] = &[
        &[("RoleName", "unbound-role")],
        &[("RoleName", "unbound-role"), ("MaxItems", "1")],
        &[("RoleName", "unbound-role"), ("MaxItems", "1000")],
        &[("RoleName", "UNBOUND-ROLE"), ("MaxItems", "7")],
    ];
    for params in cases {
        let (status, body) = call(&router, "ListInstanceProfilesForRole", params).await;
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

/// RULE: a paginated listing honours `MaxItems` as a real page size and hands out a
/// `Marker` a follow-up call resumes from, so paging reports each associated resource
/// exactly once whatever the page size.
#[tokio::test]
async fn pages_through_every_bound_profile_exactly_once() {
    let router = with_fixtures().await;

    // Rows: each fixture role crossed with page sizes spanning the published minimum,
    // sizes that do and do not divide its set, a size equal to the set, the published
    // maximum, and the absent-parameter default (100).
    for role in ROLES {
        let expected = expected_names(&bound_profiles(role));
        let page_sizes: &[Option<i32>] = &[
            Some(1),
            Some(2),
            Some(3),
            Some(expected.len().max(1) as i32),
            Some(1000),
            None,
        ];
        for page_size in page_sizes {
            let case = format!("{role} with MaxItems={page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            let mut pages = 0;

            loop {
                let mut params = vec![param("RoleName", role)];
                if let Some(size) = page_size {
                    params.push(param("MaxItems", &size.to_string()));
                }
                if let Some(token) = &marker {
                    params.push(param("Marker", token));
                }
                let (status, body) =
                    call(&router, "ListInstanceProfilesForRole", &borrowed(&params)).await;
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
                "case {case:?}: expected every bound profile exactly once in store order, \
                 got {seen:?}"
            );
        }
    }
}

/// RULE: `MaxItems` outside its published range is refused before the operation runs, so a
/// page size the model forbids never reaches the store; the refusal is HTTP 400
/// `ValidationError` and its message names the member and the bound it broke.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = with_fixtures().await;

    // One past each published bound of `maxItemsType` (range 1..1000), with the bound
    // clause each one breaks.
    for (out_of_range, requirement) in [
        ("0", "Member must have value greater than or equal to 1"),
        ("-1", "Member must have value greater than or equal to 1"),
        ("1001", "Member must have value less than or equal to 1000"),
    ] {
        let (status, body) = call(
            &router,
            "ListInstanceProfilesForRole",
            &[("RoleName", "alpha-role"), ("MaxItems", out_of_range)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {out_of_range:?}: expected the query protocol's bound refusal code, got \
             {body}"
        );
        let expected = format!(
            "1 validation error detected: Value at 'maxItems' failed to satisfy constraint: \
             {requirement}"
        );
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {out_of_range:?}: expected {expected:?}, got {:?}",
            element(&body, "Message")
        );
        assert_eq!(
            listed_names(&body),
            Vec::<String>::new(),
            "case {out_of_range:?}: a refused listing reports no profile, got {body}"
        );
    }

    // And each bound itself is accepted.
    for in_range in ["1", "1000"] {
        let (status, body) = call(
            &router,
            "ListInstanceProfilesForRole",
            &[("RoleName", "alpha-role"), ("MaxItems", in_range)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected 200, got {status}: {body}"
        );
    }
}

/// RULE: a request naming an entity the account does not hold "referenced a resource
/// entity that does not exist", which is HTTP 404 `NoSuchEntity` naming that entity —
/// never the empty list an entity that exists with no association answers with.
#[tokio::test]
async fn a_name_the_account_holds_no_role_under_is_refused() {
    let router = with_fixtures().await;

    // A role deleted after its profiles were unbound is a name the account no longer
    // holds, so it is refused exactly as one that never existed is.
    let (status, body) = call(&router, "DeleteRole", &[("RoleName", "unbound-role")]).await;
    assert_eq!(
        status, 200,
        "the fixture DeleteRole must succeed, got {status}: {body}"
    );

    // Rows: a name never created, a name that only looks like a fixture role, a name
    // spanning the rest of the accepted alphabet, and the deleted role.
    for missing in ["nobody", "alpha-role-2", "Zeta=9", "unbound-role"] {
        let (status, body) = call(
            &router,
            "ListInstanceProfilesForRole",
            &[("RoleName", missing)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {missing:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {missing:?}: expected NoSuchEntity, got {:?}",
            error_code(&body)
        );
        let expected = format!("The role with name {missing} cannot be found.");
        assert_eq!(
            element(&body, "Message"),
            expected,
            "case {missing:?}: expected {expected:?}, got {:?}",
            element(&body, "Message")
        );
        assert_eq!(
            listed_names(&body),
            Vec::<String>::new(),
            "case {missing:?}: a refused listing reports no profile, got {body}"
        );
    }
}

/// RULE: IAM entity names "are not distinguished by case", so a request naming a role in
/// any case names the one role the account holds, and reports the same associations.
#[tokio::test]
async fn the_role_name_is_resolved_without_regard_to_case() {
    let router = with_fixtures().await;

    // Rows: each fixture role in its own spelling, upper case and lower case.
    for role in ROLES {
        let expected = expected_names(&bound_profiles(role));
        for spelling in [
            (*role).to_string(),
            role.to_ascii_uppercase(),
            role.to_ascii_lowercase(),
        ] {
            let (status, body) = call(
                &router,
                "ListInstanceProfilesForRole",
                &[("RoleName", spelling.as_str())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {spelling:?}: expected 200, got {status}: {body}"
            );
            assert_eq!(
                listed_names(&body),
                expected,
                "case {spelling:?}: expected {expected:?}, got {:?}",
                listed_names(&body)
            );
        }
    }
}

/// RULE: a `Marker` is a position in ONE collection — the associations of the entity it was
/// issued for — so a token this service did not issue for that collection is refused rather
/// than silently answered from the beginning.
#[tokio::test]
async fn a_marker_this_collection_did_not_issue_is_refused() {
    let router = with_fixtures().await;

    // Rows: a token that is not base64 at all, and two that decode to bytes carrying none
    // of this service's marker structure.
    for foreign in ["not-a-marker", "zz", "0g"] {
        let (status, body) = call(
            &router,
            "ListInstanceProfilesForRole",
            &[("RoleName", "alpha-role"), ("Marker", foreign)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {foreign:?}: expected a foreign marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {foreign:?}: expected the marker to be refused as invalid, got {:?}",
            error_code(&body)
        );
    }

    // A marker issued while paging ONE role's profiles names no position in another
    // role's, and one issued while paging the account's whole instance profile collection
    // names no position in either.
    let (status, body) = call(
        &router,
        "ListInstanceProfilesForRole",
        &[("RoleName", "alpha-role"), ("MaxItems", "1")],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected a truncated page for the role, got {status}: {body}"
    );
    let alpha_marker = element(&body, "Marker");
    let (_, body) = call(&router, "ListInstanceProfiles", &[("MaxItems", "1")]).await;
    let collection_marker = element(&body, "Marker");

    for (case, role, marker) in [
        ("another role's marker", "Beta_Role", alpha_marker.as_str()),
        (
            "the whole collection's marker",
            "alpha-role",
            collection_marker.as_str(),
        ),
    ] {
        let (status, body) = call(
            &router,
            "ListInstanceProfilesForRole",
            &[("RoleName", role), ("Marker", marker)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the foreign marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected ValidationError, got {:?}",
            error_code(&body)
        );
    }

    // And the token the role's own listing issued does resume that listing.
    let (status, body) = call(
        &router,
        "ListInstanceProfilesForRole",
        &[
            ("RoleName", "alpha-role"),
            ("Marker", alpha_marker.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected the next page, got {status}: {body}");
    let rest = expected_names(&bound_profiles("alpha-role"))
        .split_off(1)
        .to_vec();
    assert_eq!(
        listed_names(&body),
        rest,
        "expected the page to resume after the first profile, got {:?}",
        listed_names(&body)
    );
}

/// RULE: a listing reads the association as it is PERSISTED now, so binding and unbinding
/// a resource changes what the next listing reports — and the resource keeps its stored
/// record either way.
#[tokio::test]
async fn the_listing_follows_the_persisted_binding() {
    let router = with_fixtures().await;

    // Rows: a profile moved out of the role's set, then back in, then into another role's
    // set — each step read back through the listing of both roles involved.
    let (status, body) = call(
        &router,
        "RemoveRoleFromInstanceProfile",
        &[("InstanceProfileName", "p-one"), ("RoleName", "alpha-role")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture RemoveRoleFromInstanceProfile must succeed, got {status}: {body}"
    );
    let (_, body) = call(
        &router,
        "ListInstanceProfilesForRole",
        &[("RoleName", "alpha-role")],
    )
    .await;
    let mut without = expected_names(&bound_profiles("alpha-role"));
    without.retain(|name| name != "p-one");
    assert_eq!(
        listed_names(&body),
        without,
        "expected the unbound profile to leave the role's set, got {:?}",
        listed_names(&body)
    );

    // The same profile bound to a DIFFERENT role is reported by that role's listing and by
    // no other.
    let (status, body) = call(
        &router,
        "AddRoleToInstanceProfile",
        &[("InstanceProfileName", "p-one"), ("RoleName", "gamma.role@3")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture AddRoleToInstanceProfile must succeed, got {status}: {body}"
    );
    let (_, body) = call(
        &router,
        "ListInstanceProfilesForRole",
        &[("RoleName", "gamma.role@3")],
    )
    .await;
    let mut gamma = expected_names(&bound_profiles("gamma.role@3"));
    gamma.push("p-one".to_string());
    gamma.sort_by_key(|name| name.to_ascii_lowercase());
    assert_eq!(
        listed_names(&body),
        gamma,
        "expected the rebound profile in the other role's set, got {:?}",
        listed_names(&body)
    );
    let (_, body) = call(
        &router,
        "ListInstanceProfilesForRole",
        &[("RoleName", "alpha-role")],
    )
    .await;
    assert_eq!(
        listed_names(&body),
        without,
        "expected the rebound profile to stay out of the first role's set, got {:?}",
        listed_names(&body)
    );
}

/// RULE: an IAM resource-listing operation reports a SUBSET of a resource's attributes —
/// the tags are withheld from the listing and available only from the single-resource read
/// — while every other stored attribute, including the association the listing is about, is
/// reported.
#[tokio::test]
async fn the_listing_withholds_the_tags_a_resource_read_reports() {
    // Rows span the tag count: none, one, several.
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("untagged", &[]),
        ("one-tag", &[("env", "prod")]),
        (
            "three-tags",
            &[("env", "dev"), ("team", "eng"), ("cost", "42")],
        ),
    ];

    for (case, tags) in cases {
        let router = router();
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", "tagged-case-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );
        let profile_name = format!("profile-{case}");
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
        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[
                ("InstanceProfileName", profile_name.as_str()),
                ("RoleName", "tagged-case-role"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture binding must succeed, got {status}: {body}"
        );

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

        let (status, listed) = call(
            &router,
            "ListInstanceProfilesForRole",
            &[("RoleName", "tagged-case-role")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {listed}"
        );
        assert_eq!(
            listed_names(&listed),
            vec![profile_name.clone()],
            "case {case:?}: expected the one bound profile, got {:?}",
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
        for member in ["InstanceProfileId", "Arn", "Path", "CreateDate", "RoleName"] {
            assert_eq!(
                elements(&listed, member),
                elements(&fetched, member),
                "case {case:?}: expected the listing to report the stored <{member}>, got {:?}",
                elements(&listed, member)
            );
        }
    }
}

/// RULE: a listing is a REPORT — it changes nothing a later caller sees, however many times
/// and however it is paged.
#[tokio::test]
async fn listing_a_roles_instance_profiles_changes_nothing_a_later_caller_sees() {
    let router = with_fixtures().await;
    let (_, before) = call(&router, "ListInstanceProfiles", &[]).await;
    let baseline = listed_names(&before);
    let bound_before: Vec<Vec<String>> = {
        let mut all = Vec::new();
        for role in ROLES {
            let (_, body) =
                call(&router, "ListInstanceProfilesForRole", &[("RoleName", role)]).await;
            all.push(listed_names(&body));
        }
        all
    };

    // Rows span the ways the listing can be driven: every role, paged one at a time and
    // whole.
    for role in ROLES {
        for params in [
            vec![param("RoleName", role)],
            vec![param("RoleName", role), param("MaxItems", "1")],
            vec![param("RoleName", role), param("MaxItems", "1000")],
        ] {
            let (status, body) =
                call(&router, "ListInstanceProfilesForRole", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {params:?}: expected 200, got {status}: {body}"
            );
        }
    }

    let (_, after) = call(&router, "ListInstanceProfiles", &[]).await;
    assert_eq!(
        listed_names(&after),
        baseline,
        "expected the repeated listings to leave the collection alone, got {:?}",
        listed_names(&after)
    );
    for (position, role) in ROLES.iter().enumerate() {
        let (_, body) = call(&router, "ListInstanceProfilesForRole", &[("RoleName", role)]).await;
        assert_eq!(
            listed_names(&body),
            bound_before[position],
            "case {role:?}: expected the repeated listings to leave the bindings alone, got \
             {:?}",
            listed_names(&body)
        );
    }
}

/// RULE: IAM decides a call against the signing principal's identity-based policies before
/// the operation reports anything — an `Allow` about the request serves it, a `Deny` about
/// it refuses it whatever any `Allow` says, and a principal with neither is refused with
/// HTTP 403 `AccessDenied`. The resource the decision is made on is the entity the request
/// addresses, so a grant scoped to another entity does not serve it.
#[tokio::test]
async fn the_listing_is_decided_against_the_callers_identity_policies() {
    let allow_all = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListInstanceProfilesForRole","Resource":"*"}]}"#;
    let deny_all = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListInstanceProfilesForRole","Resource":"*"}]}"#;
    let allow_alpha = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListInstanceProfilesForRole","Resource":"arn:aws:iam::{}:role/alpha-role"}}]}}"#,
        account_id()
    );

    // Rows: the documents in force, the role the request lists, and the status the rule
    // requires — a grant, no grant at all, a grant overridden by a deny, a grant scoped to
    // the role asked about, and that same grant asked for another role.
    let cases: &[(&str, &[&str], &str, u16)] = &[
        ("an Allow naming the action", &[allow_all], "alpha-role", 200),
        ("no document at all", &[], "alpha-role", 403),
        (
            "a Deny over the Allow",
            &[allow_all, deny_all],
            "alpha-role",
            403,
        ),
        (
            "an Allow scoped to the role asked about",
            &[allow_alpha.as_str()],
            "alpha-role",
            200,
        ),
        (
            "an Allow scoped to another role",
            &[allow_alpha.as_str()],
            "Beta_Role",
            403,
        ),
    ];

    for (row, (case, documents, role, expected)) in cases.iter().enumerate() {
        let router = with_fixtures().await;
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

        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListInstanceProfilesForRole",
            &[("RoleName", role)],
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
                "case {case:?}: expected the served listing to report the bound profiles, got \
                 {body}"
            );
        }
    }

    // A caller that may not read one role's profiles learns that it may not, never whether
    // that role exists: the decision comes before the lookup.
    let router = with_fixtures().await;
    let (status, body) = call(&router, "CreateUser", &[("UserName", "unprivileged")]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateUser must succeed, got {status}: {body}"
    );
    let (status, key) = call(
        &router,
        "CreateAccessKey",
        &[("UserName", "unprivileged")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    let access_key_id = element(&key, "AccessKeyId");
    for missing in ["nobody", "alpha-role"] {
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListInstanceProfilesForRole",
            &[("RoleName", missing)],
        )
        .await;
        assert_eq!(
            status, 403,
            "case {missing:?}: expected the refusal to precede the lookup, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {missing:?}: expected AccessDenied, got {:?}",
            error_code(&body)
        );
    }
}
