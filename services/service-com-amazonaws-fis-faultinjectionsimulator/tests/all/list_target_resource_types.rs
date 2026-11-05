// SPDX-License-Identifier: CC0-1.0

//! AWS FIS `ListTargetResourceTypes`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes. The rule
//! under test is the one FIS states for this operation ("Lists the target resource
//! types"), read as a paginated list over the service's OWN published catalog:
//!
//! * a page reports target resource types FIS publishes, each carrying the id an
//!   experiment template's target and a catalogued action's `resourceType` name, with a
//!   description, and nothing a caller could have created — no API creates, updates or
//!   deletes a resource type;
//! * a page is at most `maxResults` long, a `nextToken` arrives only with a full page, and
//!   following that token enumerates every published type exactly once at any page size —
//!   in one and the same order, whatever the page size and however often the list is read;
//! * a `nextToken` the operation never issued is refused with `ValidationException`
//!   (HTTP 400), and a `maxResults` outside `@range(min: 1, max: 100)` with HTTP 400;
//! * listing changes nothing: the same request answers the same page afterwards.
//!
//! SOURCE for the expected ids below: the AWS FIS actions reference
//! (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
//! publishes the resource types its actions target. They are written out here rather than
//! imported from `src/state.rs`, so a wrong entry in that table fails this suite instead
//! of agreeing with it; most of them are types no issue cited.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_fis_faultinjectionsimulator::state::ACTION_CATALOG;
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `ListTargetResourceTypesMaxResults`'s published bounds (`@range(min: 1, max: 100)`),
/// spelled out here rather than imported from the crate so a wrong bound in the model or
/// the handler fails this suite.
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 100;

/// `TargetResourceTypeId`'s published ceiling (`@length(min: 0, max: 128)`).
const RESOURCE_TYPE_CEILING: usize = 128;

/// `TargetResourceTypeDescription`'s published ceiling (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming this service. The service field is what routes the
/// request to fis.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }

    /// The `targetResourceTypes` list this answer carries.
    fn resource_types(&self, case: &str) -> Vec<Value> {
        self.body["targetResourceTypes"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no targetResourceTypes list in {:?}",
                    self.body
                )
            })
            .clone()
    }
}

/// One `ListTargetResourceTypes` call.
async fn list(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("/targetResourceTypes{query}"))
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(REGION))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET /targetResourceTypes{query} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Walk every page of `ListTargetResourceTypes` at `page_size`, returning the concatenated
/// resource types.
///
/// Asserts the token rule as it goes: a page is never longer than the requested size, and
/// a `nextToken` only ever arrives with a full page.
async fn walk(router: &MultiServiceRouter, page_size: Option<i32>, bound: usize) -> Vec<Value> {
    let case = format!("maxResults={page_size:?}");
    let allowed = page_size
        .map(|size| usize::try_from(size).expect("a positive page size"))
        .unwrap_or(usize::try_from(MAX_RESULTS_CEILING).expect("a positive ceiling"));
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=bound {
        let answer = list(router, page_size, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer.resource_types(&case);
        assert!(
            items.len() <= allowed,
            "case {case:?} page {page}: a page holds at most {allowed} resource types, got {}",
            items.len()
        );
        collected.extend(items.iter().cloned());
        match answer.next_token() {
            None => return collected,
            Some(next) => {
                assert_eq!(
                    items.len(),
                    allowed,
                    "case {case:?} page {page}: a nextToken must only arrive with a full page, \
                     got {} resource types",
                    items.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case {case:?}: pagination did not terminate over {bound} pages");
}

/// The `resourceType` of each listed entry, in the order the pages reported them.
fn ids(case: &str, listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|entry| {
            entry["resourceType"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed resource type carries no resourceType: {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// The published resource types this crate's action table records no action for.
///
/// FIS publishes a target resource type as soon as one of its own actions can target it,
/// and `ACTION_CATALOG` records an action only where the reference named in the module docs
/// states one, so these three are published while no catalogued action targets them. Named
/// here so the cross-table check below stays an assertion rather than an exception.
const UNTARGETED_PUBLISHED_TYPES: &[&str] = &[
    "aws:directconnect:virtual-interface",
    "aws:dsql:cluster",
    "aws:ec2:vpc-endpoint",
];

/// The resource type ids this suite expects the list to publish, by the reference named in
/// the module docs.
///
/// They span eleven AWS services and every family of target the reference documents:
/// compute (`aws:ec2:instance`, `aws:ec2:spot-instance`, `aws:lambda:function`),
/// containers (`aws:ecs:cluster`, `aws:ecs:task`, `aws:eks:cluster`, `aws:eks:nodegroup`,
/// `aws:eks:pod`), storage and databases (`aws:ec2:ebs-volume`, `aws:s3:bucket`,
/// `aws:rds:cluster`, `aws:rds:db`, `aws:dynamodb:global-table`, `aws:dsql:cluster`,
/// `aws:elasticache:replicationgroup`), networking (`aws:ec2:subnet`,
/// `aws:ec2:route-table`, `aws:ec2:transit-gateway`, `aws:ec2:vpc-endpoint`,
/// `aws:directconnect:virtual-interface`), resilience
/// (`aws:arc:zonal-shift-managed-resource`) and identity (`aws:iam:role`).
fn expected_ids() -> BTreeSet<String> {
    [
        "aws:arc:zonal-shift-managed-resource",
        "aws:directconnect:virtual-interface",
        "aws:dsql:cluster",
        "aws:dynamodb:global-table",
        "aws:ec2:autoscaling-group",
        "aws:ec2:ebs-volume",
        "aws:ec2:instance",
        "aws:ec2:route-table",
        "aws:ec2:spot-instance",
        "aws:ec2:subnet",
        "aws:ec2:transit-gateway",
        "aws:ec2:vpc-endpoint",
        "aws:ecs:cluster",
        "aws:ecs:task",
        "aws:eks:cluster",
        "aws:eks:nodegroup",
        "aws:eks:pod",
        "aws:elasticache:replicationgroup",
        "aws:iam:role",
        "aws:lambda:function",
        "aws:rds:cluster",
        "aws:rds:db",
        "aws:s3:bucket",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[tokio::test]
async fn every_page_size_enumerates_every_published_resource_type_exactly_once() {
    let router = emulator();
    let expected = expected_ids();

    // The rows here are page sizes: the published floor, every size up to one past the
    // catalog, the published ceiling, and no `maxResults` at all. Each must enumerate the
    // catalog exactly once, in the same order.
    let mut sizes: Vec<Option<i32>> = (MAX_RESULTS_FLOOR
        ..=i32::try_from(expected.len() + 1).expect("the catalog size fits an i32"))
        .map(Some)
        .collect();
    sizes.push(Some(MAX_RESULTS_CEILING));
    sizes.push(None);

    let mut first_order: Option<Vec<String>> = None;
    for page_size in sizes {
        let case = format!("maxResults={page_size:?}");
        let walked = walk(&router, page_size, expected.len() + 1).await;
        let listed = ids(&case, &walked);
        let unique: BTreeSet<String> = listed.iter().cloned().collect();
        assert_eq!(
            listed.len(),
            unique.len(),
            "case {case:?}: a resource type was served twice: {listed:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case:?}: expected every published resource type exactly once, got {listed:?}"
        );
        match &first_order {
            None => first_order = Some(listed),
            Some(order) => assert_eq!(
                &listed, order,
                "case {case:?}: expected the same order at every page size, got {listed:?} \
                 after {order:?}"
            ),
        }
    }
}

#[tokio::test]
async fn a_listed_resource_type_carries_a_published_id_and_a_description() {
    let router = emulator();
    let expected = expected_ids();
    let listed = walk(&router, Some(3), expected.len() + 1).await;

    for entry in &listed {
        let resource_type = entry["resourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("a listed resource type carries no resourceType: {entry:?}"));
        let case = resource_type;

        // `TargetResourceTypeId` is modelled `@length(0..128)`, `pattern ^[\S]+$`, and the
        // reference publishes every id in the `aws:<service>:<type>` form.
        let segments: Vec<&str> = resource_type.split(':').collect();
        assert!(
            segments.len() == 3 && segments[0] == "aws",
            "case {case:?}: expected an id of the form aws:<service>:<type>, got {segments:?}"
        );
        assert!(
            !resource_type.is_empty()
                && resource_type.chars().count() <= RESOURCE_TYPE_CEILING
                && !resource_type.chars().any(char::is_whitespace),
            "case {case:?}: expected a non-empty whitespace-free id of at most \
             {RESOURCE_TYPE_CEILING} characters, got {resource_type:?}"
        );

        // `TargetResourceTypeDescription` is modelled `@length(0..512)`, `pattern
        // ^[\s\S]+$` — which admits no empty value.
        let description = entry["description"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a description on a listed resource type: {entry:?}")
        });
        assert!(
            !description.is_empty() && description.chars().count() <= DESCRIPTION_CEILING,
            "case {case:?}: expected a non-empty description of at most {DESCRIPTION_CEILING} \
             characters, got {description:?}"
        );

        // A resource type is service-owned: a summary carries nothing else, and in
        // particular nothing a caller could have stored on it.
        let members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected an object, got {entry:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            BTreeSet::from(["resourceType", "description"]),
            "case {case:?}: expected only the modelled summary members, got {members:?}"
        );
    }

    let served: BTreeSet<String> = ids("members", &listed).into_iter().collect();
    assert_eq!(
        served, expected,
        "case members: expected exactly the published resource types, got {served:?}"
    );
}

#[tokio::test]
async fn the_list_publishes_every_resource_type_a_catalogued_action_targets() {
    // The invariant that ties the two service-owned tables together: an action's
    // `ActionTarget$resourceType` names a type an experiment template may target, so every
    // such type has to be one this list publishes.
    //
    // The converse is NOT an invariant of the published data: the type catalog is its own
    // published list, and FIS publishes a type as soon as one of its own actions can target
    // it, so a type whose action `ACTION_CATALOG` records no row for is still published. The
    // types in that position are named below rather than skipped, so adding one is a
    // deliberate edit of this list and not an unnoticed orphan.
    let router = emulator();
    let expected = expected_ids();
    let listed: BTreeSet<String> = ids(
        "action-targets",
        &walk(&router, None, expected.len() + 1).await,
    )
    .into_iter()
    .collect();

    let targeted: BTreeSet<&str> = ACTION_CATALOG
        .iter()
        .flat_map(|action| action.targets.iter().map(|target| target.resource_type))
        .collect();
    for resource_type in &targeted {
        assert!(
            listed.contains(*resource_type),
            "case {resource_type:?}: a resource type a catalogued action targets must be \
             published by ListTargetResourceTypes, got {listed:?}"
        );
    }
    let untargeted: BTreeSet<&str> = listed
        .iter()
        .map(String::as_str)
        .filter(|resource_type| !targeted.contains(resource_type))
        .collect();
    assert_eq!(
        untargeted,
        UNTARGETED_PUBLISHED_TYPES
            .iter()
            .copied()
            .collect::<BTreeSet<&str>>(),
        "case action-targets: expected exactly the published types no catalogued action \
         targets, got {untargeted:?}"
    );
}

#[tokio::test]
async fn a_token_this_operation_never_issued_and_a_max_results_out_of_range_are_refused() {
    let router = emulator();

    // A token the operation did issue is accepted — the rows below are refused for being
    // tokens it never issued, not for being tokens at all.
    let first_page = list(&router, Some(1), None).await;
    let issued = first_page
        .next_token()
        .unwrap_or_else(|| panic!("a full page of one carries a nextToken: {:?}", first_page.body));
    let resumed = list(&router, Some(1), Some(&issued)).await;
    assert_eq!(
        resumed.status, 200,
        "case issued-token: expected HTTP 200, got {} with {:?}",
        resumed.status, resumed.body
    );

    // The marker this operation puts in front of the resource type it resumes at.
    let marker = issued
        .strip_suffix(
            issued
                .rsplit('.')
                .next()
                .expect("the issued token carries a separator"),
        )
        .expect("the issued token ends with the resource type");

    // Each row is a `nextToken` this operation never issued: a plain word, a bare resource
    // type (what the token carries, without the marker), a minted id of either other
    // collection, another list's marker, the marker alone, the marker with a resource type
    // no catalog entry has, the marker with a cased or suffixed published id, and the
    // issued token mangled.
    for (case, bad_token) in [
        ("a-plain-word", "nonsense".to_string()),
        ("a-bare-resource-type", "aws:ec2:instance".to_string()),
        ("an-experiment-template-id", "EXT000000000001".to_string()),
        ("an-experiment-id", "EXP000000000001".to_string()),
        (
            "another-lists-marker",
            "targetAccountConfiguration.000000000001".to_string(),
        ),
        ("the-marker-alone", marker.to_string()),
        (
            "the-marker-with-an-unpublished-resource-type",
            format!("{marker}aws:ec2:placement-group"),
        ),
        (
            "the-marker-with-a-cased-resource-type",
            format!("{marker}AWS:EC2:INSTANCE"),
        ),
        (
            "the-marker-with-a-suffixed-resource-type",
            format!("{marker}aws:ec2:instances"),
        ),
        (
            "the-issued-token-without-its-marker",
            issued[marker.len()..].to_string(),
        ),
        (
            "the-issued-token-with-a-mangled-marker",
            format!("x{issued}"),
        ),
    ] {
        let answer = list(&router, None, Some(&bad_token)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({bad_token:?}): expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?} ({bad_token:?}): expected ValidationException, got {:?}",
            answer.body
        );
    }

    // `maxResults` on each published bound, and one past each of them.
    for (case, max_results, expected_status) in [
        ("at-the-floor", MAX_RESULTS_FLOOR, 200),
        ("at-the-ceiling", MAX_RESULTS_CEILING, 200),
        ("below-the-floor", MAX_RESULTS_FLOOR - 1, 400),
        ("above-the-ceiling", MAX_RESULTS_CEILING + 1, 400),
    ] {
        let answer = list(&router, Some(max_results), None).await;
        assert_eq!(
            answer.status, expected_status,
            "case maxResults-{case}: expected HTTP {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn listing_the_resource_types_changes_nothing() {
    let router = emulator();
    let expected = expected_ids();

    // A read must not change what a later caller sees: the same walk, at three page sizes
    // and after a read at another, answers the same pages.
    for page_size in [Some(MAX_RESULTS_FLOOR), Some(4), None] {
        let case = format!("maxResults={page_size:?}");
        let before = walk(&router, page_size, expected.len() + 1).await;
        let _ = walk(&router, Some(2), expected.len() + 1).await;
        let after = walk(&router, page_size, expected.len() + 1).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected listing twice to answer the same pages, got {after:?} \
             after {before:?}"
        );
    }
}
