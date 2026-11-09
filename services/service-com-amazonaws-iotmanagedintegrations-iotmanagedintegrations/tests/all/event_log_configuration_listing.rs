// SPDX-License-Identifier: CC0-1.0

//! Listing event log configurations, end to end over the wire:
//! `CreateEventLogConfiguration` -> `ListEventLogConfigurations` ->
//! `UpdateEventLogConfiguration`/`DeleteEventLogConfiguration` -> `ListEventLogConfigurations`.
//!
//! RULE: `ListEventLogConfigurations` (GET `/event-log-configurations`, `@readonly`) reports
//! the event log configurations the caller's region holds, in the order they were created,
//! projecting each STORED record onto an `EventLogConfigurationSummary` — the same target and
//! level a read of that one configuration reports, with `unknown` on `ResourceId` when the
//! target names no resource. `MaxResults` cuts the page at that many items and the
//! `NextToken` it answers with resumes exactly where the page stopped, so the token chain
//! reports every record exactly once and the last page carries no token. A region holding
//! none answers an empty page with no token, a `NextToken` this service never issued is the
//! modelled `ValidationException` (HTTP 400), and the list follows every later write: an
//! updated level and a deleted record show up on the next call.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.event-log-configuration-lifecycle-and-duplicate-conflict`) is
//! ONE row of the tables below. The rows differ along what the rule generalises over: the
//! number of stored records, the page size (both modelled bounds of `MaxResults` and values
//! inside them), the target each record addresses (the account wildcard, a whole resource
//! type, a resource inside a type), the `LogLevel` stored for it, the region the request is
//! signed for, and the form of a rejected `MaxResults` or `NextToken`.
//!
//! `CreateEventLogConfiguration` (POST `/event-log-configurations`, success 201) is what
//! writes those records, so the list is asserted against records a client really created
//! rather than against a seeded store. Every case drives the real restJson1 request path
//! (`MultiServiceRouter::dispatch`), so the routes, statuses, bodies and modelled errors
//! asserted here are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    EVENT_LOG_CONFIGURATION_PAGE_LIMIT, LOG_LEVEL_VALUES,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The collection route `CreateEventLogConfiguration` and `ListEventLogConfigurations` are
/// bound to (`smithy_query(query="operation",operation="ListEventLogConfigurations")` -> GET
/// `/event-log-configurations`).
const COLLECTION: &str = "/event-log-configurations";

/// The wire error code the model's 400 `@httpError` shape is rendered under.
const VALIDATION: &str = "ValidationException";

/// The `@range` bounds of `MaxResults`
/// (`smithy_query(query="operation",operation="ListEventLogConfigurations")` -> `MaxResults`
/// range 1..1000).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 1000);

/// Every `ResourceType` an event log configuration can address, read off the
/// `SmartHomeResourceType` `@pattern`
/// (`smithy_query(query="shape",shape="SmartHomeResourceType")` ->
/// `^[*]$|^(managed-thing|credential-locker|provisioning-profile|ota-task|account-association)$`):
/// the account wildcard, then the five resource types it names.
const RESOURCE_TYPES: &[&str] = &[
    "*",
    "managed-thing",
    "credential-locker",
    "provisioning-profile",
    "ota-task",
    "account-association",
];

/// What a read reports on `ResourceId` for a configuration whose target names no resource,
/// written out here rather than imported so the test pins the value a client reads and not
/// the implementation's own constant.
const UNSCOPED_RESOURCE_ID: &str = "unknown";

/// The `@length` bounds of `LogConfigurationId`, in characters
/// (`smithy_query(query="shape",shape="LogConfigurationId")` -> length 1..200, pattern
/// `^[A-Za-z0-9]+$`).
const LOG_CONFIGURATION_ID_BOUNDS: (usize, usize) = (1, 200);

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// The `Id` a create answered with.
    fn id(&self) -> Option<&str> {
        self.string("Id")
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }

    /// The `EventLogConfigurationList` of a list answer — the member the model names for
    /// this operation's items.
    fn configurations(&self) -> Vec<&Value> {
        self.member("EventLogConfigurationList")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default()
    }

    /// The ids a list answer reports, in the order it reported them.
    fn configuration_ids(&self) -> Vec<String> {
        self.configurations()
            .iter()
            .map(|item| {
                item.get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The summary reported for `id`, if the page carries one.
    fn configuration(&self, id: &str) -> Option<&Value> {
        self.configurations()
            .into_iter()
            .find(|item| item.get("Id").and_then(Value::as_str) == Some(id))
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// `CreateEventLogConfiguration` with `body`, signed for `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "POST", COLLECTION, Some(body)).await
}

/// [`create_in`] in [`REGION`].
async fn create(router: &MultiServiceRouter, body: Value) -> Reply {
    create_in(router, REGION, body).await
}

/// `ListEventLogConfigurations` with the raw `query` (empty for none), signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> Reply {
    call_in(router, region, "GET", &format!("{COLLECTION}{query}"), None).await
}

/// [`list_in`] in [`REGION`].
async fn list(router: &MultiServiceRouter, query: &str) -> Reply {
    list_in(router, REGION, query).await
}

/// `UpdateEventLogConfiguration`: PATCH `/event-log-configurations/{Id}`, signed for
/// [`REGION`].
async fn update(router: &MultiServiceRouter, id: &str, level: &str) -> Reply {
    call_in(
        router,
        REGION,
        "PATCH",
        &format!("{COLLECTION}/{id}"),
        Some(json!({"EventLogLevel": level})),
    )
    .await
}

/// `DeleteEventLogConfiguration`: DELETE `/event-log-configurations/{Id}`, signed for
/// [`REGION`].
async fn delete(router: &MultiServiceRouter, id: &str) -> Reply {
    call_in(router, REGION, "DELETE", &format!("{COLLECTION}/{id}"), None).await
}

/// The create request body that configures `level` for the target `resource_type`/
/// `resource_id`.
fn request(resource_type: &str, resource_id: Option<&str>, level: &str) -> Value {
    let mut body = json!({"ResourceType": resource_type, "EventLogLevel": level});
    if let Some(resource_id) = resource_id {
        body["ResourceId"] = json!(resource_id);
    }
    body
}

/// Assert one create was accepted, and answer the `LogConfigurationId` it minted.
fn accepted_id(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {:?}",
        reply.status, reply.body
    );
    let id = reply.id().unwrap_or_default().to_string();
    let (min, max) = LOG_CONFIGURATION_ID_BOUNDS;
    assert!(
        (min..=max).contains(&id.chars().count()) && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected a LogConfigurationId of {min}..={max} alphanumerics, \
         got {id:?} in body {:?}",
        reply.body
    );
    id
}

/// Assert one answer is the modelled refusal `status`/`code` carrying a reason.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?}",
        reply.error_type
    );
    assert!(
        !reply.message().is_empty(),
        "case {case:?}: expected the modelled Message member to carry a reason, got body {:?}",
        reply.body
    );
}

/// Assert one list answer reports exactly `expected` ids, in that order.
fn assert_lists(case: &str, reply: &Reply, expected: &[String]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.member("EventLogConfigurationList").is_some(),
        "case {case:?}: a list answer always carries EventLogConfigurationList, got body {:?}",
        reply.body
    );
    let reported = reply.configuration_ids();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the configurations {expected:?}, got {reported:?} in body {:?}",
        reply.body
    );
}

/// Assert the summary a list reports for `id` carries exactly the target and level stored for
/// it — the same projection a read of that one configuration reports.
fn assert_summary(
    case: &str,
    reply: &Reply,
    id: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    level: &str,
) {
    let summary = reply.configuration(id).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected a summary for {id:?} in body {:?}",
            reply.body
        )
    });
    let string = |member: &str| summary.get(member).and_then(Value::as_str);
    assert_eq!(
        string("ResourceType"),
        Some(resource_type),
        "case {case:?}: expected ResourceType {resource_type:?}, got {:?} in {summary:?}",
        string("ResourceType")
    );
    // A target that names no resource is reported through the placeholder.
    let expected_resource_id = resource_id.unwrap_or(UNSCOPED_RESOURCE_ID);
    assert_eq!(
        string("ResourceId"),
        Some(expected_resource_id),
        "case {case:?}: expected ResourceId {expected_resource_id:?}, got {:?} in {summary:?}",
        string("ResourceId")
    );
    assert_eq!(
        string("EventLogLevel"),
        Some(level),
        "case {case:?}: expected EventLogLevel {level:?}, got {:?} in {summary:?}",
        string("EventLogLevel")
    );
}

/// A `ResourceId` of exactly `length` characters the `SmartHomeResourceId` `@pattern`
/// (`^[a-zA-Z0-9+*]*$`) admits, so a case can sit on the modelled `@length` bound.
fn resource_id_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// The targets and levels the tables below store, one per row: the account wildcard, each
/// resource type as a whole, and several resource-specific targets whose ids differ in form
/// and length. Every row names a DISTINCT target, so none is the duplicate a create refuses.
fn seed_targets() -> Vec<(&'static str, Option<String>, &'static str)> {
    let mut rows: Vec<(&'static str, Option<String>, &'static str)> = Vec::new();
    for resource_type in RESOURCE_TYPES {
        rows.push((resource_type, None, ""));
    }
    for resource_id in [
        "thing1".to_string(),
        "ThingTWO".to_string(),
        "thing+three".to_string(),
        "*".to_string(),
        "t".to_string(),
        // The modelled `@length` maximum of `SmartHomeResourceId`.
        resource_id_of_length(200),
    ] {
        rows.push(("managed-thing", Some(resource_id), ""));
    }
    // Every `LogLevel` the model declares is exercised, cycling over the rows.
    rows.into_iter()
        .enumerate()
        .map(|(row, (resource_type, resource_id, _))| {
            (
                resource_type,
                resource_id,
                LOG_LEVEL_VALUES[row % LOG_LEVEL_VALUES.len()],
            )
        })
        .collect()
}

/// RULE: a list reports every configuration the caller's region holds, in creation order,
/// projecting each stored record onto its summary — and it is a pure read, so repeating it
/// reports the same page.
#[tokio::test]
async fn a_list_reports_every_stored_configuration_in_creation_order() {
    let router = emulator();

    let empty = list(&router, "").await;
    assert_lists("empty store", &empty, &[]);
    assert!(
        empty.member("NextToken").is_none(),
        "case \"empty store\": an empty store answers no NextToken, got {:?}",
        empty.member("NextToken")
    );

    let rows = seed_targets();
    let mut inserted: Vec<String> = Vec::new();
    for (resource_type, resource_id, level) in &rows {
        let case = format!("{resource_type}/{resource_id:?}/{level}");
        let created = create(&router, request(resource_type, resource_id.as_deref(), level)).await;
        inserted.push(accepted_id(&case, &created));
    }

    let listed = list(&router, "").await;
    assert_lists("every record", &listed, &inserted);
    assert!(
        listed.member("NextToken").is_none(),
        "case \"every record\": a complete page carries no NextToken, got {:?}",
        listed.member("NextToken")
    );
    for (id, (resource_type, resource_id, level)) in inserted.iter().zip(rows.iter()) {
        assert_summary(
            &format!("{resource_type}/{resource_id:?}/{level}"),
            &listed,
            id,
            resource_type,
            resource_id.as_deref(),
            level,
        );
    }

    // Every summary names its own configuration: no record is reported twice.
    let unique: BTreeSet<String> = inserted.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        inserted.len(),
        "case \"every record\": each create minted its own identifier, got {inserted:?}"
    );

    // A list is a pure read: repeating it reports the same page.
    let relisted = list(&router, "").await;
    assert_lists("every record, reread", &relisted, &inserted);
}

/// RULE: `MaxResults` cuts the page at that many items and the `NextToken` the page answers
/// with resumes exactly where it stopped — so the token chain reports every configuration
/// exactly once, in creation order, and the last page carries no token.
#[tokio::test]
async fn a_list_pages_over_the_store_with_max_results_and_next_token() {
    let router = emulator();

    // Nine records, so the page sizes below divide the store evenly and unevenly.
    let mut inserted: Vec<String> = Vec::new();
    for (row, (resource_type, resource_id, level)) in seed_targets().iter().take(9).enumerate() {
        let created = create(&router, request(resource_type, resource_id.as_deref(), level)).await;
        inserted.push(accepted_id(&format!("seed-{row}"), &created));
    }

    let (min_results, max_results) = MAX_RESULTS_BOUNDS;
    // Each bound of `MaxResults` and several values inside it, including one that divides the
    // store exactly and one past its size.
    for page_size in [min_results, 2, 3, 9, 10, max_results] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= inserted.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            let query = match &token {
                None => format!("?MaxResults={page_size}"),
                Some(next) => format!("?MaxResults={page_size}&NextToken={next}"),
            };
            let page = list(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {:?}",
                page.status, page.body
            );
            let page_ids = page.configuration_ids();
            assert!(
                page_ids.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: a page holds at most MaxResults items, got {} in {page_ids:?}",
                page_ids.len()
            );
            assert!(
                page_ids.len() <= EVENT_LOG_CONFIGURATION_PAGE_LIMIT,
                "case {case:?}: a page holds at most {EVENT_LOG_CONFIGURATION_PAGE_LIMIT} items, \
                 got {} in {page_ids:?}",
                page_ids.len()
            );
            seen.extend(page_ids);
            match page.string("NextToken") {
                Some(next) => {
                    assert!(
                        !next.is_empty()
                            && next
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '=' | '_' | '-')),
                        "case {case:?}: NextToken must match ^[a-zA-Z0-9=_-]+$, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, inserted,
            "case {case:?}: the token chain reports every configuration exactly once, in \
             creation order"
        );
    }
}

/// RULE: a `MaxResults` outside its modelled `@range` is the modelled 400
/// `ValidationException`, and so is a `NextToken` this service never issued — never a page
/// and never an empty answer.
#[tokio::test]
async fn a_max_results_or_next_token_outside_the_contract_is_refused() {
    let router = emulator();
    let mut inserted: Vec<String> = Vec::new();
    for (row, (resource_type, resource_id, level)) in seed_targets().iter().take(3).enumerate() {
        let created = create(&router, request(resource_type, resource_id.as_deref(), level)).await;
        inserted.push(accepted_id(&format!("seed-{row}"), &created));
    }

    let (min_results, max_results) = MAX_RESULTS_BOUNDS;
    // Each bound, and one step past it on both sides; plus a negative value and the largest
    // 32-bit integer, which are outside the range by any reading.
    for value in [
        min_results - 1,
        max_results + 1,
        -1,
        i64::from(i32::MAX),
        max_results * 2,
    ] {
        let case = format!("MaxResults={value}");
        let reply = list(&router, &format!("?MaxResults={value}")).await;
        assert_refused(&case, &reply, 400, VALIDATION);
    }
    for value in [min_results, max_results] {
        let case = format!("MaxResults={value}");
        let reply = list(&router, &format!("?MaxResults={value}")).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a MaxResults on the modelled bound is accepted, got {} with body {:?}",
            reply.status, reply.body
        );
    }

    // Tokens that match the modelled `NextToken` pattern — so the boundary validator admits
    // them — but that this service cannot have issued: a word, a cursor past everything it
    // has minted, and one of the wrong width.
    for token in ["notatoken", "ffffffffffffffff", "0", "00000000000000000000"] {
        let case = format!("NextToken={token}");
        let reply = list(&router, &format!("?NextToken={token}")).await;
        assert_refused(&case, &reply, 400, VALIDATION);
    }

    // A token this service DID issue still pages, so the refusals above are about the token
    // and not about paging.
    let first = list(&router, "?MaxResults=1").await;
    let token = first
        .string("NextToken")
        .expect("a store of three records answers a NextToken for a page of one")
        .to_string();
    let second = list(&router, &format!("?MaxResults=1&NextToken={token}")).await;
    assert_lists("the issued token", &second, &inserted[1..2]);
}

/// RULE: a configuration is regional, so a list reports only the records the region it is
/// signed for holds — and a write in one region changes no other region's page.
#[tokio::test]
async fn a_list_reports_only_the_signing_region() {
    let router = emulator();

    // The rows differ along the region and along how many records it holds.
    let regions: Vec<(&str, usize)> = vec![("us-east-1", 3), ("eu-central-1", 1), ("ap-south-1", 2)];
    let mut by_region: Vec<(&str, Vec<String>)> = Vec::new();
    for (region, count) in &regions {
        let mut ids = Vec::new();
        for (row, (resource_type, resource_id, level)) in
            seed_targets().iter().take(*count).enumerate()
        {
            let created = create_in(
                &router,
                region,
                request(resource_type, resource_id.as_deref(), level),
            )
            .await;
            ids.push(accepted_id(&format!("{region}/seed-{row}"), &created));
        }
        by_region.push((region, ids));
    }

    for (region, ids) in &by_region {
        let listed = list_in(&router, region, "").await;
        assert_lists(region, &listed, ids);
    }

    // A region that holds none answers an empty page, not another region's records.
    let elsewhere = list_in(&router, "us-west-2", "").await;
    assert_lists("a region holding none", &elsewhere, &[]);
}

/// RULE: the list follows the store, so a level an update changed and a record a delete
/// removed show up on the next call — nothing is cached and nothing is synthesised.
#[tokio::test]
async fn a_list_follows_later_writes() {
    let router = emulator();

    let rows = seed_targets();
    let seeded = &rows[..4];
    let mut inserted: Vec<String> = Vec::new();
    for (row, (resource_type, resource_id, level)) in seeded.iter().enumerate() {
        let created = create(&router, request(resource_type, resource_id.as_deref(), level)).await;
        inserted.push(accepted_id(&format!("seed-{row}"), &created));
    }

    // Each stored record is updated to a level it did not carry; the next list reports the
    // new one.
    for (id, (resource_type, resource_id, level)) in inserted.iter().zip(seeded.iter()) {
        let next_level = LOG_LEVEL_VALUES
            .iter()
            .find(|candidate| *candidate != level)
            .expect("the model declares more than one LogLevel");
        let updated = update(&router, id, next_level).await;
        assert_eq!(
            updated.status, 200,
            "case {id:?}: expected the update to be accepted, got {} with body {:?}",
            updated.status, updated.body
        );
        let listed = list(&router, "").await;
        assert_summary(
            &format!("{resource_type}/{resource_id:?} updated to {next_level}"),
            &listed,
            id,
            resource_type,
            resource_id.as_deref(),
            next_level,
        );
    }

    // Deleting a record removes it from the page and leaves the others in creation order.
    let mut remaining = inserted.clone();
    while !remaining.is_empty() {
        let removed = remaining.remove(0);
        let deleted = delete(&router, &removed).await;
        assert_eq!(
            deleted.status, 200,
            "case {removed:?}: expected the delete to be accepted, got {} with body {:?}",
            deleted.status, deleted.body
        );
        let listed = list(&router, "").await;
        assert_lists(&format!("after deleting {removed}"), &listed, &remaining);
    }

    // An emptied store answers an empty page with no token.
    let emptied = list(&router, "").await;
    assert_lists("the emptied store", &emptied, &[]);
    assert!(
        emptied.member("NextToken").is_none(),
        "case \"the emptied store\": an empty page carries no NextToken, got {:?}",
        emptied.member("NextToken")
    );
}
