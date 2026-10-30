// SPDX-License-Identifier: CC0-1.0

//! AWS FIS `ListActions`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes. The rule
//! under test is the one FIS states for this operation ("Lists the available FIS actions"),
//! read as a paginated list over the service's OWN published catalog:
//!
//! * a page reports actions FIS publishes, each carrying the id an experiment template's
//!   action names, a description, the target resource types it supports keyed by the name a
//!   template refers to them by, a fully qualified ARN naming the region the request was
//!   signed for and the account serving it, and no tags — an action is service-owned, so
//!   nothing a caller
//!   could have created or tagged appears on it;
//! * a page is at most `maxResults` long, a `nextToken` arrives only with a full page, and
//!   following that token enumerates every published action exactly once at any page size —
//!   in one and the same order, whatever the page size and however often the list is read;
//! * a summary carries the `ActionSummary` members the model declares and no others, so an
//!   action's parameter contract is what `GetAction` adds and the two agree on everything
//!   else;
//! * a `nextToken` the operation never issued is refused with `ValidationException`
//!   (HTTP 400), and a `maxResults` outside `@range(min: 1, max: 100)` with HTTP 400;
//! * listing changes nothing: the same request answers the same page afterwards.
//!
//! SOURCE for the expected ids below: the AWS FIS actions reference
//! (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
//! the `Action` shape's own model documentation points at. They are written out here rather
//! than imported from `src/state.rs`, so a wrong entry in that table fails this suite
//! instead of agreeing with it; most of them are actions no issue cited.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `ListActionsMaxResults`'s published bounds (`@range(min: 1, max: 100)`), spelled out
/// here rather than imported from the crate so a wrong bound in the model or the handler
/// fails this suite.
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 100;

/// `ActionId`'s published ceiling (`@length(min: 0, max: 128)`).
const ACTION_ID_CEILING: usize = 128;

/// `ActionDescription`'s published ceiling (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// `ResourceArn`'s published floor (`@length(min: 20, max: 2048)`).
const ARN_FLOOR: usize = 20;

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// The account every ARN this emulator reports names, spelled out here rather than
/// imported so a changed account field fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis; the region field is what a reported ARN names.
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

    /// The `actions` list this answer carries.
    fn actions(&self, case: &str) -> Vec<Value> {
        self.body["actions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no actions list in {:?}", self.body))
            .clone()
    }
}

/// Dispatch one request and read the answer.
async fn call(router: &MultiServiceRouter, uri: &str, region: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(region))
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
                "GET {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One `ListActions` call, signed for `region`.
async fn list_in(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
    region: &str,
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
    call(router, &format!("/actions{query}"), region).await
}

/// One `ListActions` call in the fallback region.
async fn list(
    router: &MultiServiceRouter,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    list_in(router, max_results, next_token, FALLBACK_REGION).await
}

/// One `GetAction` call, so the list can be checked against the full action.
async fn get_action(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, &format!("/actions/{id}"), FALLBACK_REGION).await
}

/// Walk every page of `ListActions` at `page_size`, signed for `region`, returning the
/// concatenated actions.
///
/// Asserts the token rule as it goes: a page is never longer than the requested size, and a
/// `nextToken` only ever arrives with a full page.
async fn walk_in(
    router: &MultiServiceRouter,
    page_size: Option<i32>,
    bound: usize,
    region: &str,
) -> Vec<Value> {
    let case = format!("maxResults={page_size:?}");
    let allowed = page_size
        .map(|size| usize::try_from(size).expect("a positive page size"))
        .unwrap_or(usize::try_from(MAX_RESULTS_CEILING).expect("a positive ceiling"));
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=bound {
        let answer = list_in(router, page_size, token.as_deref(), region).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer.actions(&case);
        assert!(
            items.len() <= allowed,
            "case {case:?} page {page}: a page holds at most {allowed} actions, got {}",
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
                     got {} actions",
                    items.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case {case:?}: pagination did not terminate over {bound} pages");
}

/// [`walk_in`] in the fallback region.
async fn walk(router: &MultiServiceRouter, page_size: Option<i32>, bound: usize) -> Vec<Value> {
    walk_in(router, page_size, bound, FALLBACK_REGION).await
}

/// The `id` of each listed action, in the order the pages reported them.
fn ids(case: &str, listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed action carries no id: {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// The action ids this suite expects the list to publish, by the reference named in the
/// module docs.
///
/// They span fourteen AWS services and every family of fault the reference documents:
/// instance lifecycle (`aws:ec2:stop-instances`, `aws:ec2:terminate-instances`), API faults
/// (`aws:ec2:api-error`, the `aws:fis:inject-api-*` family), container faults (the
/// `aws:ecs:task-*` and `aws:eks:pod-*` families), storage and database faults
/// (`aws:ebs:pause-volume-io`, `aws:rds:failover-db-cluster`,
/// `aws:s3:bucket-pause-replication`), networking (`aws:network:*`), zonal recovery
/// (`aws:arc:start-zonal-autoshift`), observability (`aws:cloudwatch:assert-alarm-state`),
/// orchestration (`aws:ssm:*`) and the parameterised no-op (`aws:fis:wait`).
fn expected_ids() -> BTreeSet<String> {
    [
        "aws:arc:start-zonal-autoshift",
        "aws:cloudwatch:assert-alarm-state",
        "aws:dynamodb:global-table-pause-replication",
        "aws:ebs:pause-volume-io",
        "aws:ec2:api-error",
        "aws:ec2:api-insufficient-instance-capacity-error",
        "aws:ec2:asg-insufficient-instance-capacity-error",
        "aws:ec2:reboot-instances",
        "aws:ec2:send-spot-instance-interruptions",
        "aws:ec2:stop-instances",
        "aws:ec2:terminate-instances",
        "aws:ecs:drain-container-instances",
        "aws:ecs:stop-task",
        "aws:ecs:task-cpu-stress",
        "aws:ecs:task-io-stress",
        "aws:ecs:task-kill-process",
        "aws:ecs:task-network-blackhole-port",
        "aws:ecs:task-network-latency",
        "aws:ecs:task-network-packet-loss",
        "aws:eks:inject-kubernetes-custom-resource",
        "aws:eks:pod-cpu-stress",
        "aws:eks:pod-delete",
        "aws:eks:pod-io-stress",
        "aws:eks:pod-memory-stress",
        "aws:eks:pod-network-blackhole-port",
        "aws:eks:pod-network-latency",
        "aws:eks:pod-network-packet-loss",
        "aws:eks:terminate-nodegroup-instances",
        "aws:elasticache:replicationgroup-interrupt-az-power",
        "aws:fis:inject-api-internal-error",
        "aws:fis:inject-api-throttle-error",
        "aws:fis:inject-api-unavailable-error",
        "aws:fis:wait",
        "aws:lambda:invocation-add-delay",
        "aws:lambda:invocation-error",
        "aws:lambda:invocation-http-integration-response",
        "aws:network:disrupt-connectivity",
        "aws:network:route-table-disrupt-connectivity",
        "aws:network:transit-gateway-disrupt-cross-region-connectivity",
        "aws:rds:failover-db-cluster",
        "aws:rds:reboot-db-instances",
        "aws:s3:bucket-pause-replication",
        "aws:ssm:send-command",
        "aws:ssm:start-automation-execution",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// The page sizes this suite walks the list at: the published floor, a few sizes that do
/// not divide the catalog, the sizes just under, at and just over the catalog's own length,
/// the published ceiling, and no `maxResults` at all.
fn page_sizes(catalog: usize) -> Vec<Option<i32>> {
    let length = i32::try_from(catalog).expect("the catalog size fits an i32");
    vec![
        Some(MAX_RESULTS_FLOOR),
        Some(2),
        Some(3),
        Some(7),
        Some(length - 1),
        Some(length),
        Some(length + 1),
        Some(MAX_RESULTS_CEILING),
        None,
    ]
}

#[tokio::test]
async fn every_page_size_enumerates_every_published_action_exactly_once() {
    let router = emulator();
    let expected = expected_ids();

    let mut first_order: Option<Vec<String>> = None;
    for page_size in page_sizes(expected.len()) {
        let case = format!("maxResults={page_size:?}");
        let walked = walk(&router, page_size, expected.len() + 1).await;
        let listed = ids(&case, &walked);
        let unique: BTreeSet<String> = listed.iter().cloned().collect();
        assert_eq!(
            listed.len(),
            unique.len(),
            "case {case:?}: an action was served twice: {listed:?}"
        );
        assert_eq!(
            unique, expected,
            "case {case:?}: expected every published action exactly once, got {listed:?}"
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
async fn a_listed_action_carries_the_modelled_summary_and_nothing_else() {
    let router = emulator();
    let expected = expected_ids();
    let listed = walk(&router, Some(5), expected.len() + 1).await;

    for entry in &listed {
        let id = entry["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a listed action carries no id: {entry:?}"));
        let case = id;

        // `ActionId` is modelled `@length(0..128)`, `pattern ^[\S]+$`, and the reference
        // publishes every id in the `aws:<service>:<action>` form.
        let segments: Vec<&str> = id.split(':').collect();
        assert!(
            segments.len() == 3 && segments[0] == "aws",
            "case {case:?}: expected an id of the form aws:<service>:<action>, got {segments:?}"
        );
        assert!(
            !id.is_empty()
                && id.chars().count() <= ACTION_ID_CEILING
                && !id.chars().any(char::is_whitespace),
            "case {case:?}: expected a non-empty whitespace-free id of at most \
             {ACTION_ID_CEILING} characters, got {id:?}"
        );

        // An action belongs to the SERVICE, but the ARN reported for one is still complete:
        // it names the region the request was signed for and the serving account.
        let arn = entry["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {entry:?}"));
        assert_eq!(
            arn,
            format!("arn:aws:fis:{FALLBACK_REGION}:{ACCOUNT_ID}:action/{id}"),
            "case {case:?}: expected the action arn to name {FALLBACK_REGION} and account \
             {ACCOUNT_ID}, got {arn:?}"
        );
        assert!(
            arn.chars().count() >= ARN_FLOOR,
            "case {case:?}: expected an arn of at least {ARN_FLOOR} characters, got {arn:?}"
        );

        // `ActionDescription` is modelled `@length(0..512)`, `pattern ^[\s\S]+$` — which
        // admits no empty value.
        let description = entry["description"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: expected a description on a listed action: {entry:?}")
        });
        assert!(
            !description.is_empty() && description.chars().count() <= DESCRIPTION_CEILING,
            "case {case:?}: expected a non-empty description of at most {DESCRIPTION_CEILING} \
             characters, got {description:?}"
        );

        // Each supported target names a resource type under the key an experiment
        // template's action refers to it by. An action that needs no target reports an
        // empty map rather than an absent one: it has a target contract either way.
        let targets = entry["targets"].as_object().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected targets to be a map, got {:?}",
                entry["targets"]
            )
        });
        for (name, target) in targets {
            let resource_type = target["resourceType"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: target {name:?} names no resourceType: {target:?}")
            });
            let segments: Vec<&str> = resource_type.split(':').collect();
            assert!(
                segments.len() == 3 && segments[0] == "aws",
                "case {case:?}: expected target {name:?} to name a resource type of the form \
                 aws:<service>:<type>, got {resource_type:?}"
            );
        }

        // An action is service-owned, so there is nothing for a caller to have tagged.
        assert!(
            entry["tags"]
                .as_object()
                .is_some_and(serde_json::Map::is_empty),
            "case {case:?}: expected no tags on a service-owned action, got {:?}",
            entry["tags"]
        );

        // `ActionSummary` models exactly these five members and NO `parameters`: an
        // action's parameter contract is what `GetAction` adds.
        let members: BTreeSet<&str> = entry
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected an object, got {entry:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            BTreeSet::from(["id", "arn", "description", "targets", "tags"]),
            "case {case:?}: expected only the modelled summary members, got {members:?}"
        );
    }

    let served: BTreeSet<String> = ids("members", &listed).into_iter().collect();
    assert_eq!(
        served, expected,
        "case members: expected exactly the published actions, got {served:?}"
    );
}

#[tokio::test]
async fn a_listed_action_agrees_with_the_action_get_action_reports() {
    // Both operations report the same service-owned catalog, so a summary is the full
    // action minus its parameter contract — for EVERY published action, not a sample.
    let router = emulator();
    let expected = expected_ids();
    let listed = walk(&router, None, expected.len() + 1).await;

    for summary in &listed {
        let id = summary["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a listed action carries no id: {summary:?}"));
        let case = id;
        let answer = get_action(&router, id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetAction to report a listed action, got {} with {:?}",
            answer.status, answer.body
        );
        let action = &answer.body["action"];
        for member in ["id", "arn", "description", "targets", "tags"] {
            assert_eq!(
                &action[member], &summary[member],
                "case {case:?}: expected the listed {member:?} to match GetAction's, got {:?} \
                 and {:?}",
                summary[member], action[member]
            );
        }
        assert!(
            action["parameters"].is_object(),
            "case {case:?}: expected GetAction to add the parameter contract the summary \
             omits, got {:?}",
            action["parameters"]
        );
    }
}

#[tokio::test]
async fn a_listed_action_is_reported_in_the_region_the_request_was_signed_for() {
    let router = emulator();
    let expected = expected_ids();

    // The rows are credential scopes: three partitions' regions and an unusable scope that
    // falls back. Every reported ARN is built from the request, never from one region.
    for (signed, region) in [
        ("eu-central-1", "eu-central-1"),
        ("ap-south-2", "ap-south-2"),
        ("us-gov-east-1", "us-gov-east-1"),
        ("not a region", FALLBACK_REGION),
    ] {
        let listed = walk_in(&router, Some(9), expected.len() + 1, signed).await;
        let served: BTreeSet<String> = ids(signed, &listed).into_iter().collect();
        assert_eq!(
            served, expected,
            "case {signed:?}: expected every published action, got {served:?}"
        );
        for entry in &listed {
            let id = entry["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed action carries no id: {entry:?}"));
            assert_eq!(
                entry["arn"].as_str().unwrap_or_default(),
                format!("arn:aws:fis:{region}:{ACCOUNT_ID}:action/{id}"),
                "case {signed:?}/{id:?}: expected the arn to name {region}, got {:?}",
                entry["arn"]
            );
        }
    }
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

    // The marker this operation puts in front of the action id it resumes at.
    let marker = issued
        .strip_suffix(
            issued
                .split_once('.')
                .expect("the issued token carries a separator")
                .1,
        )
        .expect("the issued token ends with the action id");

    // Each row is a `nextToken` this operation never issued: a plain word, a bare action id
    // (what the token carries, without the marker), a minted id of either caller-owned
    // collection, another list's marker, the marker alone, the marker with an id the catalog
    // does not hold, the marker with a cased or suffixed published id, and the issued token
    // mangled.
    for (case, bad_token) in [
        ("a-plain-word", "nonsense".to_string()),
        ("a-bare-action-id", "aws:fis:wait".to_string()),
        ("an-experiment-template-id", "EXT000000000001".to_string()),
        ("an-experiment-id", "EXP000000000001".to_string()),
        (
            "another-lists-marker",
            "targetResourceType.aws:ec2:instance".to_string(),
        ),
        ("the-marker-alone", marker.to_string()),
        (
            "the-marker-with-an-unpublished-action",
            format!("{marker}aws:ec2:hibernate-instances"),
        ),
        (
            "the-marker-with-a-cased-action",
            format!("{marker}AWS:FIS:WAIT"),
        ),
        (
            "the-marker-with-a-suffixed-action",
            format!("{marker}aws:fis:waits"),
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
async fn listing_the_actions_changes_nothing() {
    let router = emulator();
    let expected = expected_ids();

    // A read must not change what a later caller sees: the same walk, at three page sizes
    // and after a read at another, answers the same pages.
    for page_size in [Some(MAX_RESULTS_FLOOR), Some(6), None] {
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
