// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAccessPolicies`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rule these suites pin: `ListAccessPolicies` enumerates EVERY access policy
//! the service's state holds, each exactly once, in ONE stable order, cut into pages
//! of at most `maxResults` entries where `nextToken` is present only while another
//! entry follows and resumes the enumeration exactly where the previous page
//! stopped; the identity pair (`identityType` with `identityId` or `iamArn`) and the
//! Monitor-resource pair (`resourceType` with `resourceId`) each NARROW that
//! enumeration to the policies about what they name, half a pair is refused as the
//! bad request it is, and a listed policy carries the values
//! `DescribeAccessPolicy` reports for it — so an update is visible in the listing
//! and the listing answers from stored state rather than from a create's input.
//!
//! The HTTP harness (`call`, `router_with_engine`, `error_code`, `hex_token`,
//! `is_modelled_id`) is the one the asset suite already has, and the portal and
//! project helpers are the ones the access-policy suite has; this suite reuses
//! all of them rather than growing a second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::access_policy_lifecycle::{create_portal, create_project};
use crate::asset_lifecycle::{call, error_code, hex_token, is_modelled_id, router_with_engine};

/// Where every access-policy request goes, as `OPERATIONS.md` reports the routes:
/// `GET /access-policies`, `POST /access-policies`,
/// `GET|PUT /access-policies/{accessPolicyId}`.
const ACCESS_POLICIES: &str = "/access-policies";

/// A well-formed `ID` UUID this suite never stores a Monitor resource under.
const UNKNOWN_RESOURCE_ID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

/// An `IdentityId` no stored policy is about (len 1..256, `^\S+$`).
const UNKNOWN_IDENTITY_ID: &str = "idc-user-nobody-is-about";

/// The IAM Identity Center id of the first user the fixture is about.
const USER_ONE_ID: &str = "94effa50-e1b4-4a3c-9a5a-8bf2e0da6a21";

/// The IAM Identity Center id of the second user.
const USER_TWO_ID: &str = "c7ab1d92-4f0e-4c44-8b64-1f2a3b4c5d6e";

/// The IAM Identity Center id of the one group.
const GROUP_ONE_ID: &str = "eb1b5f70-9b2b-4e1d-bc40-5b5f8f4df0c2";

/// One IAM ARN carried by an `iamUser` identity AND by an `iamRole` identity, so a
/// listing filtered by it has to report both: `iamArn` names an IAM identity, and
/// `Identity` spells an IAM one two ways (smithy_query query="shape"
/// shape="Identity": `iamUser`, `iamRole`).
const SHARED_IAM_ARN: &str = "arn:aws:iam::123456789012:user/floor-one-operator";

/// A second IAM ARN, carried by one `iamUser` identity only.
const OTHER_IAM_ARN: &str = "arn:aws:iam::123456789012:user/floor-two-operator";

/// The `Permission` values. Source: `smithy_query(query="shape",
/// shape="Permission")` — `ADMINISTRATOR` | `VIEWER`, the enum's whole list.
const ADMINISTRATOR: &str = "ADMINISTRATOR";
const VIEWER: &str = "VIEWER";

/// The page size this operation's `maxResults` documents when the caller sends
/// none: "Default: 50".
const DEFAULT_MAX_RESULTS: usize = 50;

/// Which identity a fixture row is about — one per `Identity` member, with the IAM
/// forms split over the two ARNs above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityRef {
    UserOne,
    UserTwo,
    GroupOne,
    IamUserShared,
    IamRoleShared,
    IamUserOther,
}

/// Which Monitor resource a fixture row is about — both `Resource` members, each
/// over two resources, so a filter that names one cannot pass by reporting all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceRef {
    PortalOne,
    PortalTwo,
    ProjectOne,
    ProjectTwo,
}

/// The `accessPolicyIdentity` a row sends: exactly one `Identity` member, which is
/// the rule that shape states.
fn identity_json(identity: IdentityRef) -> Value {
    match identity {
        IdentityRef::UserOne => json!({ "user": { "id": USER_ONE_ID } }),
        IdentityRef::UserTwo => json!({ "user": { "id": USER_TWO_ID } }),
        IdentityRef::GroupOne => json!({ "group": { "id": GROUP_ONE_ID } }),
        IdentityRef::IamUserShared => json!({ "iamUser": { "arn": SHARED_IAM_ARN } }),
        IdentityRef::IamRoleShared => json!({ "iamRole": { "arn": SHARED_IAM_ARN } }),
        IdentityRef::IamUserOther => json!({ "iamUser": { "arn": OTHER_IAM_ARN } }),
    }
}

/// One created access policy: the row it came from, and the id the create minted.
#[derive(Debug)]
struct CreatedPolicy {
    label: &'static str,
    identity: IdentityRef,
    resource: ResourceRef,
    permission: &'static str,
    id: String,
}

/// The two portals, the two projects and the policies about them.
#[derive(Debug)]
struct Fixture {
    portal_one: String,
    portal_two: String,
    project_one: String,
    project_two: String,
    /// The created policies, in the order they were created.
    policies: Vec<CreatedPolicy>,
}

impl Fixture {
    /// The id of one of the fixture's Monitor resources.
    fn resource_id(&self, resource: ResourceRef) -> &str {
        match resource {
            ResourceRef::PortalOne => &self.portal_one,
            ResourceRef::PortalTwo => &self.portal_two,
            ResourceRef::ProjectOne => &self.project_one,
            ResourceRef::ProjectTwo => &self.project_two,
        }
    }

    /// The `accessPolicyResource` a row sends: exactly one `Resource` member.
    fn resource_json(&self, resource: ResourceRef) -> Value {
        let id = self.resource_id(resource);
        match resource {
            ResourceRef::PortalOne | ResourceRef::PortalTwo => json!({ "portal": { "id": id } }),
            ResourceRef::ProjectOne | ResourceRef::ProjectTwo => json!({ "project": { "id": id } }),
        }
    }

    /// The policy a label names.
    fn policy(&self, label: &str) -> &CreatedPolicy {
        self.policies
            .iter()
            .find(|policy| policy.label == label)
            .unwrap_or_else(|| panic!("no fixture policy labelled {label:?}"))
    }
}

/// The fixture rows: every `Identity` member, both `Resource` members, both
/// `Permission` values, and several policies per identity and per resource, so no
/// filter below can pass either by reporting everything or by reporting one row.
fn policy_rows() -> Vec<(&'static str, IdentityRef, ResourceRef, &'static str)> {
    vec![
        (
            "user-one-portal-one",
            IdentityRef::UserOne,
            ResourceRef::PortalOne,
            ADMINISTRATOR,
        ),
        (
            "user-one-portal-two",
            IdentityRef::UserOne,
            ResourceRef::PortalTwo,
            VIEWER,
        ),
        (
            "user-one-project-one",
            IdentityRef::UserOne,
            ResourceRef::ProjectOne,
            VIEWER,
        ),
        (
            "user-two-portal-one",
            IdentityRef::UserTwo,
            ResourceRef::PortalOne,
            VIEWER,
        ),
        (
            "user-two-project-one",
            IdentityRef::UserTwo,
            ResourceRef::ProjectOne,
            ADMINISTRATOR,
        ),
        (
            "group-one-portal-one",
            IdentityRef::GroupOne,
            ResourceRef::PortalOne,
            ADMINISTRATOR,
        ),
        (
            "group-one-portal-two",
            IdentityRef::GroupOne,
            ResourceRef::PortalTwo,
            VIEWER,
        ),
        (
            "group-one-project-two",
            IdentityRef::GroupOne,
            ResourceRef::ProjectTwo,
            VIEWER,
        ),
        (
            "iam-user-shared-portal-two",
            IdentityRef::IamUserShared,
            ResourceRef::PortalTwo,
            ADMINISTRATOR,
        ),
        (
            "iam-role-shared-portal-two",
            IdentityRef::IamRoleShared,
            ResourceRef::PortalTwo,
            ADMINISTRATOR,
        ),
        (
            "iam-role-shared-project-one",
            IdentityRef::IamRoleShared,
            ResourceRef::ProjectOne,
            VIEWER,
        ),
        (
            "iam-user-other-project-two",
            IdentityRef::IamUserOther,
            ResourceRef::ProjectTwo,
            ADMINISTRATOR,
        ),
    ]
}

/// Create the two portals, the two projects and every policy row.
async fn create_fixture(router: &MultiServiceRouter) -> Fixture {
    let portal_one = create_portal(router, "list-access-policies-portal-one").await;
    let portal_two = create_portal(router, "list-access-policies-portal-two").await;
    let project_one = create_project(router, &portal_one, "list-access-policies-project-one").await;
    let project_two = create_project(router, &portal_two, "list-access-policies-project-two").await;

    let mut fixture = Fixture {
        portal_one,
        portal_two,
        project_one,
        project_two,
        policies: Vec::new(),
    };

    for (label, identity, resource, permission) in policy_rows() {
        let body = json!({
            "accessPolicyIdentity": identity_json(identity),
            "accessPolicyResource": fixture.resource_json(resource),
            "accessPolicyPermission": permission,
        });
        let (status, created) = call(router, "POST", ACCESS_POLICIES, Some(body)).await;
        assert_eq!(
            status, 201,
            "case {label:?}: CreateAccessPolicy expected 201, got {status} {created}"
        );
        let id = created["accessPolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_id(&id),
            "case {label:?}: accessPolicyId must be a modelled UUID, got {created}"
        );
        fixture.policies.push(CreatedPolicy {
            label,
            identity,
            resource,
            permission,
            id,
        });
    }

    fixture
}

/// `GET /access-policies`, carrying `query` when it has anything.
async fn list(router: &MultiServiceRouter, query: &[String]) -> (u16, Value) {
    let uri = if query.is_empty() {
        ACCESS_POLICIES.to_string()
    } else {
        format!("{ACCESS_POLICIES}?{}", query.join("&"))
    };
    call(router, "GET", &uri, None).await
}

/// Walk a whole enumeration at `page_size` (or with no `maxResults` at all when it
/// is `None`), returning the ids in the order the pages reported them.
async fn walk(
    router: &MultiServiceRouter,
    label: &str,
    filter: &[String],
    page_size: Option<usize>,
    total: usize,
) -> Vec<String> {
    let case = match page_size {
        Some(size) => format!("{label}, maxResults={size}"),
        None => format!("{label}, no maxResults"),
    };
    let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut query: Vec<String> = filter.to_vec();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }

        let (status, body) = list(router, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {pages} expected 200, got {status} {body}"
        );
        let summaries = body["accessPolicySummaries"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: accessPolicySummaries must be a list, got {body}")
            })
            .clone();
        assert!(
            summaries.len() <= limit,
            "case {case:?}: a page holds at most {limit}, got {} in {body}",
            summaries.len()
        );
        for summary in &summaries {
            seen.push(
                summary["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a summary without an id: {summary}"))
                    .to_string(),
            );
        }
        pages += 1;
        assert!(
            pages <= total + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            // "The token for the next set of results, or null if there are no
            // additional results": a token is issued only while the page it resumes
            // would hold something.
            Some(_) => assert_eq!(
                summaries.len(),
                limit,
                "case {case:?}: a page followed by a nextToken must be full, got {body}"
            ),
            None => break,
        }
    }

    let expected_pages = total.div_ceil(limit).max(1);
    assert_eq!(
        pages, expected_pages,
        "case {case:?}: expected {expected_pages} pages over {total} policies, walked {pages}"
    );
    seen
}

/// One row of the filter table: the query members the request carries, beside the
/// hand-written predicate that says which fixture rows the filter admits.
struct FilterCase {
    label: &'static str,
    query: Vec<String>,
    /// The INDEPENDENT oracle: decided from the fixture row, never from the
    /// listing's own answer.
    admits: fn(IdentityRef, ResourceRef) -> bool,
}

/// The filter rows. They differ along what the rule generalises over: no filter at
/// all, each `identityType` value with the member that value requires, `iamArn`
/// alone, each `resourceType` value, the two filters together, and filters that
/// name an identity or a Monitor resource no stored policy is about.
fn filter_cases(fixture: &Fixture) -> Vec<FilterCase> {
    vec![
        FilterCase {
            label: "no filter",
            query: Vec::new(),
            admits: |_, _| true,
        },
        FilterCase {
            label: "identityType=USER, the first user",
            query: vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_ONE_ID}"),
            ],
            admits: |identity, _| identity == IdentityRef::UserOne,
        },
        FilterCase {
            label: "identityType=USER, the second user",
            query: vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_TWO_ID}"),
            ],
            admits: |identity, _| identity == IdentityRef::UserTwo,
        },
        FilterCase {
            label: "identityType=GROUP",
            query: vec![
                "identityType=GROUP".to_string(),
                format!("identityId={GROUP_ONE_ID}"),
            ],
            admits: |identity, _| identity == IdentityRef::GroupOne,
        },
        FilterCase {
            label: "identityType=IAM, an ARN two identity forms carry",
            query: vec![
                "identityType=IAM".to_string(),
                format!("iamArn={SHARED_IAM_ARN}"),
            ],
            admits: |identity, _| {
                identity == IdentityRef::IamUserShared || identity == IdentityRef::IamRoleShared
            },
        },
        FilterCase {
            label: "identityType=IAM, an ARN one identity carries",
            query: vec![
                "identityType=IAM".to_string(),
                format!("iamArn={OTHER_IAM_ARN}"),
            ],
            admits: |identity, _| identity == IdentityRef::IamUserOther,
        },
        FilterCase {
            label: "iamArn with no identityType",
            query: vec![format!("iamArn={SHARED_IAM_ARN}")],
            admits: |identity, _| {
                identity == IdentityRef::IamUserShared || identity == IdentityRef::IamRoleShared
            },
        },
        FilterCase {
            label: "resourceType=PORTAL, the first portal",
            query: vec![
                "resourceType=PORTAL".to_string(),
                format!("resourceId={}", fixture.portal_one),
            ],
            admits: |_, resource| resource == ResourceRef::PortalOne,
        },
        FilterCase {
            label: "resourceType=PORTAL, the second portal",
            query: vec![
                "resourceType=PORTAL".to_string(),
                format!("resourceId={}", fixture.portal_two),
            ],
            admits: |_, resource| resource == ResourceRef::PortalTwo,
        },
        FilterCase {
            label: "resourceType=PROJECT, the first project",
            query: vec![
                "resourceType=PROJECT".to_string(),
                format!("resourceId={}", fixture.project_one),
            ],
            admits: |_, resource| resource == ResourceRef::ProjectOne,
        },
        FilterCase {
            label: "resourceType=PROJECT, the second project",
            query: vec![
                "resourceType=PROJECT".to_string(),
                format!("resourceId={}", fixture.project_two),
            ],
            admits: |_, resource| resource == ResourceRef::ProjectTwo,
        },
        FilterCase {
            label: "a user and a portal together",
            query: vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_ONE_ID}"),
                "resourceType=PORTAL".to_string(),
                format!("resourceId={}", fixture.portal_one),
            ],
            admits: |identity, resource| {
                identity == IdentityRef::UserOne && resource == ResourceRef::PortalOne
            },
        },
        FilterCase {
            label: "a group and a project together",
            query: vec![
                "identityType=GROUP".to_string(),
                format!("identityId={GROUP_ONE_ID}"),
                "resourceType=PROJECT".to_string(),
                format!("resourceId={}", fixture.project_two),
            ],
            admits: |identity, resource| {
                identity == IdentityRef::GroupOne && resource == ResourceRef::ProjectTwo
            },
        },
        FilterCase {
            label: "an IAM ARN and a portal together",
            query: vec![
                "identityType=IAM".to_string(),
                format!("iamArn={SHARED_IAM_ARN}"),
                "resourceType=PORTAL".to_string(),
                format!("resourceId={}", fixture.portal_two),
            ],
            admits: |identity, resource| {
                (identity == IdentityRef::IamUserShared || identity == IdentityRef::IamRoleShared)
                    && resource == ResourceRef::PortalTwo
            },
        },
        FilterCase {
            label: "a user and a project no policy pairs",
            query: vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_ONE_ID}"),
                "resourceType=PROJECT".to_string(),
                format!("resourceId={}", fixture.project_two),
            ],
            admits: |_, _| false,
        },
        FilterCase {
            label: "an identity no policy is about",
            query: vec![
                "identityType=USER".to_string(),
                format!("identityId={UNKNOWN_IDENTITY_ID}"),
            ],
            admits: |_, _| false,
        },
        FilterCase {
            label: "a Monitor resource no policy is about",
            query: vec![
                "resourceType=PORTAL".to_string(),
                format!("resourceId={UNKNOWN_RESOURCE_ID}"),
            ],
            admits: |_, _| false,
        },
    ]
}

/// The ids a filter row admits, in the fixture's own creation order.
fn expected_ids(fixture: &Fixture, case: &FilterCase) -> Vec<String> {
    fixture
        .policies
        .iter()
        .filter(|policy| (case.admits)(policy.identity, policy.resource))
        .map(|policy| policy.id.clone())
        .collect()
}

/// The ids one full-page call reports for `filter`.
async fn list_ids(router: &MultiServiceRouter, label: &str, filter: &[String]) -> Vec<String> {
    let mut query = filter.to_vec();
    query.push("maxResults=250".to_string());
    let (status, body) = list(router, &query).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected 200, got {status} {body}"
    );
    body["accessPolicySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label:?}: accessPolicySummaries in {body}"))
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `a` and `b` hold the same ids, whatever order each is in.
fn same_set(left: &[String], right: &[String]) -> bool {
    let mut left: Vec<String> = left.to_vec();
    let mut right: Vec<String> = right.to_vec();
    left.sort();
    right.sort();
    left == right
}

#[tokio::test]
async fn list_access_policies_reports_every_stored_policy_once_in_one_stable_order() {
    let (router, _engine) = router_with_engine();
    let fixture = create_fixture(&router).await;
    let total = fixture.policies.len();

    // The reference order: the enumeration one full-page call reports.
    let reference = walk(&router, "every policy", &[], Some(250), total).await;
    let created: Vec<String> = fixture
        .policies
        .iter()
        .map(|policy| policy.id.clone())
        .collect();
    assert!(
        same_set(&reference, &created),
        "the enumeration must cover exactly the stored policies: got {reference:?}, \
         created {created:?}"
    );

    // Rows differ in page size: the published `maxResults` bounds (1 and 250), sizes
    // that divide the fixture and sizes that do not, a size past it, and no
    // `maxResults` at all (the documented default of 50).
    for page_size in [
        Some(1_usize),
        Some(2),
        Some(3),
        Some(5),
        Some(12),
        Some(13),
        Some(250),
        None,
    ] {
        let seen = walk(&router, "every policy", &[], page_size, total).await;
        assert_eq!(
            seen, reference,
            "case {page_size:?}: paging must report the one stable order, got {seen:?}"
        );
    }

    // That one order is by creation instant: the dates the pages report never go
    // backwards.
    let (status, body) = list(&router, &["maxResults=250".to_string()]).await;
    assert_eq!(status, 200, "ListAccessPolicies got {status} {body}");
    let summaries = body["accessPolicySummaries"]
        .as_array()
        .expect("accessPolicySummaries")
        .clone();
    let mut previous = f64::MIN;
    for summary in &summaries {
        let creation = summary["creationDate"].as_f64().unwrap_or_default();
        assert!(
            creation > 0.0,
            "case {:?}: creationDate must be a real instant, got {summary}",
            summary["id"]
        );
        assert!(
            creation >= previous,
            "case {:?}: the enumeration must not go back in time, {creation} follows {previous}",
            summary["id"]
        );
        previous = creation;
    }
}

#[tokio::test]
async fn the_identity_and_resource_filters_narrow_the_enumeration() {
    let (router, _engine) = router_with_engine();
    let fixture = create_fixture(&router).await;

    for case in filter_cases(&fixture) {
        let expected = expected_ids(&fixture, &case);

        // One call, which also shows an empty match is a served empty page rather
        // than a refusal.
        let mut query = case.query.clone();
        query.push("maxResults=250".to_string());
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {body}",
            case.label
        );
        let seen: Vec<String> = body["accessPolicySummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: accessPolicySummaries in {body}", case.label))
            .iter()
            .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            same_set(&seen, &expected),
            "case {:?}: expected the policies {expected:?}, got {seen:?} in {body}",
            case.label
        );
        if expected.is_empty() {
            assert_eq!(
                body["accessPolicySummaries"],
                json!([]),
                "case {:?}: a filter nothing matches lists no policy, got {body}",
                case.label
            );
            assert!(
                body["nextToken"].is_null(),
                "case {:?}: a filter nothing matches issues no nextToken, got {body}",
                case.label
            );
        }

        // The same narrowed enumeration pages: one entry at a time, and all at once.
        for page_size in [Some(1_usize), Some(2), Some(250), None] {
            let paged = walk(
                &router,
                case.label,
                &case.query,
                page_size,
                expected.len(),
            )
            .await;
            assert!(
                same_set(&paged, &expected),
                "case {:?} (maxResults={page_size:?}): expected {expected:?}, got {paged:?}",
                case.label
            );
        }
    }
}

#[tokio::test]
async fn a_listed_policy_carries_what_describe_access_policy_reports() {
    let (router, _engine) = router_with_engine();
    let fixture = create_fixture(&router).await;

    let (status, body) = list(&router, &["maxResults=250".to_string()]).await;
    assert_eq!(
        status, 200,
        "ListAccessPolicies expected 200, got {status} {body}"
    );
    let summaries = body["accessPolicySummaries"]
        .as_array()
        .expect("accessPolicySummaries")
        .clone();
    assert_eq!(
        summaries.len(),
        fixture.policies.len(),
        "every created policy must be listed, got {body}"
    );

    // Each `AccessPolicySummary` member beside the `DescribeAccessPolicy` member
    // that reports the same value (smithy_query query="shape"
    // shape="AccessPolicySummary").
    let members = [
        ("id", "accessPolicyId"),
        ("identity", "accessPolicyIdentity"),
        ("resource", "accessPolicyResource"),
        ("permission", "accessPolicyPermission"),
        ("creationDate", "accessPolicyCreationDate"),
        ("lastUpdateDate", "accessPolicyLastUpdateDate"),
    ];
    for summary in &summaries {
        let id = summary["id"].as_str().unwrap_or_default();
        let (status, described) =
            call(&router, "GET", &format!("{ACCESS_POLICIES}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {id:?}: DescribeAccessPolicy expected 200, got {status} {described}"
        );
        for (listed, describes) in members {
            assert_eq!(
                summary[listed], described[describes],
                "case {id:?}: the summary's {listed} must be what DescribeAccessPolicy reports \
                 as {describes}: {summary} vs {described}"
            );
        }
    }

    // And the row the fixture created is the row the listing reports, member by
    // member — so the summary is read off stored state rather than assembled from
    // something else.
    for policy in &fixture.policies {
        let summary = summaries
            .iter()
            .find(|summary| summary["id"].as_str() == Some(policy.id.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: the policy must be listed", policy.label));
        assert_eq!(
            summary["identity"],
            identity_json(policy.identity),
            "case {:?}: unexpected identity in {summary}",
            policy.label
        );
        assert_eq!(
            summary["resource"],
            fixture.resource_json(policy.resource),
            "case {:?}: unexpected resource in {summary}",
            policy.label
        );
        assert_eq!(
            summary["permission"].as_str().unwrap_or_default(),
            policy.permission,
            "case {:?}: unexpected permission in {summary}",
            policy.label
        );
    }
}

#[tokio::test]
async fn the_listing_follows_an_update_of_a_stored_policy() {
    let (router, _engine) = router_with_engine();
    let fixture = create_fixture(&router).await;

    // Rows differ in what the update changes: the identity (and so which identity
    // filter finds the policy), the Monitor resource, and the permission.
    let rewritten = [
        (
            "user-one-portal-one",
            IdentityRef::UserTwo,
            ResourceRef::ProjectTwo,
            VIEWER,
        ),
        (
            "iam-role-shared-project-one",
            IdentityRef::GroupOne,
            ResourceRef::PortalTwo,
            VIEWER,
        ),
        (
            "group-one-portal-two",
            IdentityRef::IamUserOther,
            ResourceRef::PortalOne,
            ADMINISTRATOR,
        ),
    ];

    for (label, identity, resource, permission) in rewritten {
        let policy = fixture.policy(label);
        let update = json!({
            "accessPolicyIdentity": identity_json(identity),
            "accessPolicyResource": fixture.resource_json(resource),
            "accessPolicyPermission": permission,
        });
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("{ACCESS_POLICIES}/{}", policy.id),
            Some(update.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: UpdateAccessPolicy expected 200, got {status} {updated}"
        );

        // The listing reports what the update wrote.
        let (status, body) = list(&router, &["maxResults=250".to_string()]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: ListAccessPolicies expected 200, got {status} {body}"
        );
        let summary = body["accessPolicySummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: accessPolicySummaries in {body}"))
            .iter()
            .find(|summary| summary["id"].as_str() == Some(policy.id.as_str()))
            .unwrap_or_else(|| panic!("case {label:?}: the updated policy must be listed in {body}"))
            .clone();
        assert_eq!(
            summary["identity"], update["accessPolicyIdentity"],
            "case {label:?}: the listing must report the written identity, got {summary}"
        );
        assert_eq!(
            summary["resource"], update["accessPolicyResource"],
            "case {label:?}: the listing must report the written resource, got {summary}"
        );
        assert_eq!(
            summary["permission"], update["accessPolicyPermission"],
            "case {label:?}: the listing must report the written permission, got {summary}"
        );

        // And the identity filter follows it: the written identity finds the policy,
        // the one it replaced does not.
        let query = match identity {
            IdentityRef::UserTwo => vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_TWO_ID}"),
            ],
            IdentityRef::GroupOne => vec![
                "identityType=GROUP".to_string(),
                format!("identityId={GROUP_ONE_ID}"),
            ],
            IdentityRef::IamUserOther => vec![
                "identityType=IAM".to_string(),
                format!("iamArn={OTHER_IAM_ARN}"),
            ],
            other => panic!("case {label:?}: no filter written for {other:?}"),
        };
        let found = list_ids(&router, label, &query).await;
        assert!(
            found.contains(&policy.id),
            "case {label:?}: the written identity must find the policy, got {found:?}"
        );

        let before = match policy.identity {
            IdentityRef::UserOne => vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_ONE_ID}"),
            ],
            IdentityRef::GroupOne => vec![
                "identityType=GROUP".to_string(),
                format!("identityId={GROUP_ONE_ID}"),
            ],
            IdentityRef::IamRoleShared => vec![
                "identityType=IAM".to_string(),
                format!("iamArn={SHARED_IAM_ARN}"),
            ],
            other => panic!("case {label:?}: no filter written for {other:?}"),
        };
        let stale = list_ids(&router, label, &before).await;
        assert!(
            !stale.contains(&policy.id),
            "case {label:?}: the replaced identity must no longer find the policy, got {stale:?}"
        );
    }
}

#[tokio::test]
async fn listing_access_policies_does_not_change_what_a_later_caller_sees() {
    let (router, engine) = router_with_engine();

    // An empty store lists nothing and issues no token, rather than a fabricated
    // entry or a token that resumes nothing.
    let (status, empty) = list(&router, &[]).await;
    assert_eq!(status, 200, "an empty store expected 200, got {status} {empty}");
    assert_eq!(
        empty["accessPolicySummaries"],
        json!([]),
        "an empty store lists no policy, got {empty}"
    );
    assert!(
        empty["nextToken"].is_null(),
        "an empty store issues no nextToken, got {empty}"
    );

    let fixture = create_fixture(&router).await;
    let held = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .access_policies
        .len();

    // Rows differ in page size and in whether a filter is carried; each read twice.
    let filters = [
        Vec::new(),
        vec![
            "identityType=USER".to_string(),
            format!("identityId={USER_ONE_ID}"),
        ],
        vec![
            "resourceType=PORTAL".to_string(),
            format!("resourceId={}", fixture.portal_one),
        ],
    ];
    for filter in &filters {
        for page_size in [1_usize, 5, 250] {
            let mut query = filter.clone();
            query.push(format!("maxResults={page_size}"));
            let (status, first) = list(&router, &query).await;
            assert_eq!(
                status, 200,
                "case {filter:?} (maxResults={page_size}): expected 200, got {status} {first}"
            );
            let (status, again) = list(&router, &query).await;
            assert_eq!(
                status, 200,
                "case {filter:?} (maxResults={page_size}): the second read expected 200, got \
                 {status} {again}"
            );
            assert_eq!(
                again, first,
                "case {filter:?} (maxResults={page_size}): a read must not change what the next \
                 caller sees, got {again} after {first}"
            );
        }
    }

    // And no read added, removed or moved a stored policy.
    let after = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .access_policies
        .len();
    assert_eq!(
        after, held,
        "the reads must leave the store holding {held} policies, it holds {after}"
    );
}

#[tokio::test]
async fn half_a_filter_pair_is_refused() {
    let (router, engine) = router_with_engine();
    let fixture = create_fixture(&router).await;

    // Rows differ in WHICH half of WHICH pair is missing. The expected message is
    // written out here by hand, as an independent oracle.
    let refusals: Vec<(&str, Vec<String>, String)> = vec![
        (
            "identityId with no identityType",
            vec![format!("identityId={USER_ONE_ID}")],
            "identityType is required when identityId is specified.".to_string(),
        ),
        (
            "identityType=USER with no identityId",
            vec!["identityType=USER".to_string()],
            "identityId is required when identityType is USER.".to_string(),
        ),
        (
            "identityType=GROUP with no identityId",
            vec!["identityType=GROUP".to_string()],
            "identityId is required when identityType is GROUP.".to_string(),
        ),
        (
            "identityType=IAM with no iamArn",
            vec!["identityType=IAM".to_string()],
            "iamArn is required when identityType is IAM.".to_string(),
        ),
        (
            "identityType=IAM carrying only an identityId",
            vec![
                "identityType=IAM".to_string(),
                format!("identityId={USER_ONE_ID}"),
            ],
            "iamArn is required when identityType is IAM.".to_string(),
        ),
        (
            "resourceType=PORTAL with no resourceId",
            vec!["resourceType=PORTAL".to_string()],
            "resourceId is required when resourceType is specified.".to_string(),
        ),
        (
            "resourceType=PROJECT with no resourceId",
            vec!["resourceType=PROJECT".to_string()],
            "resourceId is required when resourceType is specified.".to_string(),
        ),
        (
            "resourceId with no resourceType",
            vec![format!("resourceId={}", fixture.portal_one)],
            "resourceType is required when resourceId is specified.".to_string(),
        ),
        (
            "a stated identity filter beside half a resource pair",
            vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_ONE_ID}"),
                format!("resourceId={}", fixture.portal_one),
            ],
            "resourceType is required when resourceId is specified.".to_string(),
        ),
    ];

    for (label, query, message) in refusals {
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 400,
            "case {label:?}: half a filter pair must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {label:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or_default(),
            message,
            "case {label:?}: unexpected refusal message in {body}"
        );
    }

    // A refused read wrote nothing: every policy is still there.
    let held = service_state(&*engine)
        .expect("the IoT SiteWise store")
        .lock()
        .access_policies
        .len();
    assert_eq!(
        held,
        fixture.policies.len(),
        "the refusals must leave every stored policy in place, the store holds {held}"
    );
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let (router, _engine) = router_with_engine();
    let fixture = create_fixture(&router).await;

    // Rows differ in HOW the token is wrong: not hex at all, an odd-length hex run,
    // hex of a well-formed id no policy carries, and hex of something that is no id.
    let excluded = fixture.policy("iam-user-other-project-two");
    let forged: Vec<(&str, String, Vec<String>)> = vec![
        ("not hex at all", "zzzz".to_string(), Vec::new()),
        ("odd-length hex", "abc".to_string(), Vec::new()),
        (
            "hex of an id no policy carries",
            hex_token(UNKNOWN_RESOURCE_ID),
            Vec::new(),
        ),
        (
            "hex of something that is no id",
            hex_token("iam-user-other-project-two"),
            Vec::new(),
        ),
        // A stored policy's id IS a resume point for the whole enumeration, but not
        // for one the filter excludes it from: that token resumes nothing there.
        (
            "hex of a policy the filter excludes",
            hex_token(&excluded.id),
            vec![
                "identityType=USER".to_string(),
                format!("identityId={USER_ONE_ID}"),
            ],
        ),
    ];

    for (label, token, filter) in forged {
        let mut query = filter.clone();
        query.push(format!("nextToken={token}"));
        let (status, body) = list(&router, &query).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a forged nextToken must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {label:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or_default(),
            "The nextToken is not a token this operation issued.",
            "case {label:?}: unexpected refusal message in {body}"
        );
    }

    // The refusals left the enumeration intact.
    let seen = walk(
        &router,
        "after the forged tokens",
        &[],
        Some(250),
        fixture.policies.len(),
    )
    .await;
    assert_eq!(
        seen.len(),
        fixture.policies.len(),
        "the refusals must not have changed the enumeration, saw {seen:?}"
    );
}

#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let (router, _engine) = router_with_engine();
    create_fixture(&router).await;

    // `maxResults` carries @range 1..250 (smithy_query query="operation"
    // operation="ListAccessPolicies"): each published bound is served and one past it
    // on either side is refused.
    let cases: [(&str, i64, bool); 4] = [
        ("the range minimum", 1, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ];
    for (label, max_results, served) in cases {
        let (status, body) = list(&router, &[format!("maxResults={max_results}")]).await;
        if served {
            assert_eq!(
                status, 200,
                "case {label:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
        }
    }
}
