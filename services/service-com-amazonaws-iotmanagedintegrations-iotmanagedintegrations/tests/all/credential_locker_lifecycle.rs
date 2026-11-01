// SPDX-License-Identifier: CC0-1.0

//! Credential lockers, end to end over the wire: `CreateCredentialLocker` ->
//! `GetCredentialLocker` -> `ListCredentialLockers`.
//!
//! RULE: `ListCredentialLockers` (GET `/credential-lockers`, `@readonly`) reports the
//! credential lockers the caller's region holds, in the order they were created, projecting
//! each STORED record onto a `CredentialLockerSummary`; `MaxResults` cuts the page at that
//! many items and the `NextToken` it answers with resumes exactly where the page stopped,
//! so the token chain reports every record exactly once and the last page carries no token.
//! A `NextToken` this service never issued is the modelled `ValidationException` (HTTP
//! 400), and an empty store answers an empty page with no token.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.credential-locker-quota-exceeded-accepted`) is ONE row of the
//! tables below. The rows differ along what the rule generalises over: the number of stored
//! records, the page size (both modelled bounds and inside them), which members each record
//! carries and in what form, the region the request is signed for, and the form of the
//! rejected `MaxResults` and `NextToken`.
//!
//! `CreateCredentialLocker` (POST `/credential-lockers`, success 201) is what writes those
//! records and `GetCredentialLocker` (GET `/credential-lockers/{Identifier}`) reads one
//! back, so the list is asserted against the records a client really created rather than
//! against a seeded store. Every case drives the real restJson1 request path
//! (`MultiServiceRouter::dispatch`), so the routes, statuses, bodies and modelled errors
//! asserted here are the ones a client sees.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    CredentialLocker, CREDENTIAL_LOCKER_NOT_FOUND_MESSAGE, CREDENTIAL_LOCKER_PAGE_LIMIT,
    CREDENTIAL_LOCKER_RESOURCE_TYPE, MAX_CREDENTIAL_LOCKERS,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The collection route `CreateCredentialLocker` and `ListCredentialLockers` are bound to
/// (`smithy_query(query="operation",operation="ListCredentialLockers")` -> GET
/// `/credential-lockers`).
const COLLECTION: &str = "/credential-lockers";

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";
const QUOTA_EXCEEDED: &str = "ServiceQuotaExceededException";

/// The `@range` bounds of `MaxResults`
/// (`smithy_query(query="operation",operation="ListCredentialLockers")` -> `MaxResults`
/// range 1..1000).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 1000);

/// The `@length` bounds of `CredentialLockerId`, in characters
/// (`smithy_query(query="shape",shape="CredentialLockerId")` -> length 1..64, pattern
/// `^[a-zA-Z0-9]*$`).
const CREDENTIAL_LOCKER_ID_BOUNDS: (usize, usize) = (1, 64);

/// The `@length` bounds of `CredentialLockerArn`
/// (`smithy_query(query="shape",shape="CredentialLockerArn")` -> length 32..1011).
const CREDENTIAL_LOCKER_ARN_BOUNDS: (usize, usize) = (32, 1011);

/// The `@length` maximum of `CredentialLockerName`
/// (`smithy_query(query="shape",shape="CredentialLockerName")` -> length 1..128, pattern
/// `^[A-Za-z0-9-_ ]+$`).
const CREDENTIAL_LOCKER_NAME_MAX_LENGTH: usize = 128;

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

    fn seconds(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }

    /// The `Items` of a list answer.
    fn items(&self) -> Vec<&Value> {
        self.member("Items")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default()
    }

    /// The ids a list answer reports, in the order it reported them.
    fn item_ids(&self) -> Vec<String> {
        self.items()
            .iter()
            .map(|item| {
                item.get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The tags a read reports, on either casing of the member.
    fn tags(&self) -> Option<BTreeMap<String, String>> {
        let value = self.member("Tags").or_else(|| self.member("tags"))?;
        let pairs = value.as_object()?;
        Some(
            pairs
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect(),
        )
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the caller keeps a handle to, so a test can inspect the
/// service's store directly.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"
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

/// `CreateCredentialLocker` with `body`, signed for `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "POST", COLLECTION, Some(body)).await
}

/// [`create_in`] in [`REGION`].
async fn create(router: &MultiServiceRouter, body: Value) -> Reply {
    create_in(router, REGION, body).await
}

/// `GetCredentialLocker` for `id`, signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, id: &str) -> Reply {
    call_in(router, region, "GET", &format!("{COLLECTION}/{id}"), None).await
}

/// [`get_in`] in [`REGION`].
async fn get(router: &MultiServiceRouter, id: &str) -> Reply {
    get_in(router, REGION, id).await
}

/// `ListCredentialLockers` with the raw `query` (empty for none), signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> Reply {
    call_in(router, region, "GET", &format!("{COLLECTION}{query}"), None).await
}

/// [`list_in`] in [`REGION`].
async fn list(router: &MultiServiceRouter, query: &str) -> Reply {
    list_in(router, REGION, query).await
}

/// `DeleteCredentialLocker` for `id`, signed for [`REGION`].
async fn delete(router: &MultiServiceRouter, id: &str) -> Reply {
    call_in(
        router,
        REGION,
        "DELETE",
        &format!("{COLLECTION}/{id}"),
        None,
    )
    .await
}

/// `ListTagsForResource`: GET `/tags/{ResourceArn}`. `ResourceArn` is a NON-greedy
/// `@httpLabel`, so the ARN fills ONE path segment and every byte outside the unreserved
/// set is percent-encoded, which is what an SDK puts on the wire.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Reply {
    let label: String = arn
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect();
    call_in(router, REGION, "GET", &format!("/tags/{label}"), None).await
}

/// Assert one create was accepted, and answer the `Id` it minted — checking the answer
/// against the members the model declares for it.
fn accepted_id(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {:?}",
        reply.status, reply.body
    );
    let id = reply.string("Id").unwrap_or_default().to_string();
    let (min, max) = CREDENTIAL_LOCKER_ID_BOUNDS;
    assert!(
        (min..=max).contains(&id.chars().count()) && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected a CredentialLockerId of {min}..={max} alphanumerics, got \
         {id:?} in body {:?}",
        reply.body
    );
    assert!(
        reply.seconds("CreatedAt").is_some_and(|secs| secs > 0.0),
        "case {case:?}: expected a CreatedAt timestamp, got {:?}",
        reply.member("CreatedAt")
    );
    assert_arn(case, reply.string("Arn"), &id);
    id
}

/// Assert an `Arn` is the credential-locker ARN of `id`: the modelled `CredentialLockerArn`
/// pattern, with the resource segment naming this id.
fn assert_arn(case: &str, arn: Option<&str>, id: &str) {
    let arn = arn.unwrap_or_default();
    let (min, max) = CREDENTIAL_LOCKER_ARN_BOUNDS;
    assert!(
        (min..=max).contains(&arn.len())
            && arn.starts_with("arn:aws:iotmanagedintegrations:")
            && arn.ends_with(&format!(":credential-locker/{id}")),
        "case {case:?}: expected a CredentialLockerArn of {min}..={max} characters naming \
         credential-locker/{id}, got {arn:?}"
    );
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

/// Assert `reply` is a 200 list answer whose items are exactly `expected`, in order.
fn assert_lists(case: &str, reply: &Reply, expected: &[String]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.item_ids(),
        expected,
        "case {case:?}: expected the stored records {expected:?} in insertion order, got \
         body {:?}",
        reply.body
    );
}

/// RULE: a create stores every member it was given and the reads project the STORED record
/// — `GetCredentialLocker` reports all of them, `ListCredentialLockers` the ones
/// `CredentialLockerSummary` declares — so what a create wrote comes back on both paths.
#[tokio::test]
async fn a_create_stores_the_record_and_the_reads_project_it() {
    let router = emulator();

    // The rows differ along which optional members the create carries and along the form
    // and length of each one: no member at all, names differing in form inside the
    // `CredentialLockerName` pattern (`^[A-Za-z0-9-_ ]+$`, length 1..128) including both
    // length bounds, tags alone, and every member at once.
    let cases: Vec<(&str, Value)> = vec![
        ("no member at all", json!({})),
        ("name at the modelled minimum length", json!({"Name": "L"})),
        ("name with spaces", json!({"Name": "My Credential Locker"})),
        (
            "name with every admitted punctuation",
            json!({"Name": "locker-1_of 2"}),
        ),
        (
            "name at the modelled maximum length",
            json!({"Name": "n".repeat(CREDENTIAL_LOCKER_NAME_MAX_LENGTH)}),
        ),
        ("tags only", json!({"Tags": {"env": "test", "owner": "fixer"}})),
        (
            "every member at once",
            json!({"Name": "Full", "Tags": {"only": "one"}, "ClientToken": "token-full"}),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut created: Vec<(String, String, Value)> = Vec::new();
    for (case, body) in &cases {
        let reply = create(&router, body.clone()).await;
        let id = accepted_id(case, &reply);
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: every create mints its own identifier, but {id:?} was minted twice"
        );

        // The get reports every member the record stores.
        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from the get, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("Name").map(str::to_string),
            body.get("Name").and_then(Value::as_str).map(str::to_string),
            "case {case:?}: expected the get to report the stored Name, got {:?}",
            read.member("Name")
        );
        // The tag map is part of the record, so the get reports it whatever it holds: the
        // pairs the create stored, or the EMPTY map for a locker the create tagged with
        // nothing.
        let stored_tags = body.get("Tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            read.member("Tags"),
            Some(&stored_tags),
            "case {case:?}: expected the get to report the stored Tags {stored_tags:?}, got {:?}",
            read.member("Tags")
        );
        assert_eq!(
            read.string("Id"),
            Some(id.as_str()),
            "case {case:?}: expected the get to report the stored Id, got {:?}",
            read.member("Id")
        );
        assert_eq!(
            read.seconds("CreatedAt"),
            reply.seconds("CreatedAt"),
            "case {case:?}: expected the get to report the CreatedAt the create minted, \
             got {:?} after {:?}",
            read.member("CreatedAt"),
            reply.member("CreatedAt")
        );
        assert_arn(case, read.string("Arn"), &id);

        // The one tag map the resource carries is the one the ARN-addressed tagging
        // surface reads, so a locker's tags resolve through its own ARN too.
        let arn = read.string("Arn").unwrap_or_default().to_string();
        let tagged = list_tags(&router, &arn).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected HTTP 200 reading the tags of {arn:?}, got {} with body {:?}",
            tagged.status, tagged.body
        );
        // The two readers report the same PAIRS. Their reporting of an EMPTY map differs:
        // the locker's own read always carries the member (asserted above), so the
        // comparison is over the pairs each reports.
        let own = read
            .tags()
            .unwrap_or_else(|| panic!("case {case:?}: the locker read reports no Tags member"));
        assert_eq!(
            tagged.tags().unwrap_or_default(),
            own,
            "case {case:?}: expected the tagging surface to report the record's own tag map, \
             got {:?} against {own:?}",
            tagged.tags()
        );

        created.push((case.to_string(), id, body.clone()));
    }

    // One list answer must carry the same projection of every record, in insertion order.
    let listed = list(&router, "").await;
    let expected: Vec<String> = created.iter().map(|(_, id, _)| id.clone()).collect();
    assert_lists("every record", &listed, &expected);
    for (item, (case, id, body)) in listed.items().iter().zip(created.iter()) {
        // The members `CredentialLockerSummary` declares, each carrying the stored value.
        assert_eq!(
            item.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: expected the item to report the stored Id, got {item:?}"
        );
        assert_eq!(
            item.get("Name").and_then(Value::as_str),
            body.get("Name").and_then(Value::as_str),
            "case {case:?}: expected the item to report the stored Name, got {item:?}"
        );
        assert!(
            item.get("CreatedAt")
                .and_then(Value::as_f64)
                .is_some_and(|secs| secs > 0.0),
            "case {case:?}: expected the item to report a CreatedAt, got {item:?}"
        );
        assert_arn(case, item.get("Arn").and_then(Value::as_str), id);
        assert!(
            item.get("Tags").is_none(),
            "case {case:?}: CredentialLockerSummary declares no Tags member, got {item:?}"
        );
    }

    // A list is a pure read: repeating it reports the same page.
    let relisted = list(&router, "").await;
    assert_lists("every record, reread", &relisted, &expected);
}

/// RULE: a list reports the stored records in insertion order, `MaxResults` cuts the page at
/// that many items, and the `NextToken` it answers with resumes exactly where the page
/// stopped — so the token chain reports every record exactly once and the last page carries
/// no token. An empty store answers an empty page with no token.
#[tokio::test]
async fn a_list_pages_over_the_store_in_insertion_order() {
    let router = emulator();

    let empty = list(&router, "").await;
    assert_lists("empty store", &empty, &[]);
    assert!(
        empty.member("NextToken").is_none(),
        "case \"empty store\": an empty store answers no NextToken, got {:?}",
        empty.member("NextToken")
    );

    // Seven records, so the page sizes below divide the store evenly and unevenly.
    let mut inserted: Vec<String> = Vec::new();
    for n in 0..7u32 {
        let reply = create(&router, json!({"Name": format!("locker-{n}")})).await;
        inserted.push(accepted_id(&format!("seed-{n}"), &reply));
    }

    let unpaged = list(&router, "").await;
    assert_lists("no MaxResults", &unpaged, &inserted);
    assert!(
        unpaged.member("NextToken").is_none(),
        "case \"no MaxResults\": a complete page carries no NextToken, got {:?}",
        unpaged.member("NextToken")
    );

    // Each page size: walking the token chain must visit every id exactly once, in
    // insertion order, and the last page must carry no token. 1 and 1000 are the modelled
    // bounds of `MaxResults`.
    let (min_results, max_results) = MAX_RESULTS_BOUNDS;
    for page_size in [min_results, 2, 3, 7, 8, max_results] {
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
            let page_ids = page.item_ids();
            assert!(
                page_ids.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: a page holds at most MaxResults items, got {} in {page_ids:?}",
                page_ids.len()
            );
            assert!(
                page_ids.len() <= CREDENTIAL_LOCKER_PAGE_LIMIT,
                "case {case:?}: a page holds at most {CREDENTIAL_LOCKER_PAGE_LIMIT} items, got \
                 {} in {page_ids:?}",
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
            "case {case:?}: the token chain reports every record exactly once, in insertion order"
        );
    }
}

/// RULE: a `MaxResults` outside its modelled `@range` is the modelled 400
/// `ValidationException`, while every value inside the range is accepted and bounds the
/// page.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    for n in 0..3u32 {
        let reply = create(&router, json!({"Name": format!("locker-{n}")})).await;
        accepted_id(&format!("seed-{n}"), &reply);
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
        assert!(
            reply.member("Items").is_none(),
            "case {case:?}: a refused list reports no Items, got {:?}",
            reply.body
        );
    }

    // Inside the range, both bounds included, the call is answered.
    for value in [min_results, min_results + 1, 2, 100, max_results] {
        let case = format!("MaxResults={value}");
        let reply = list(&router, &format!("?MaxResults={value}")).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.items().is_empty()
                && reply.items().len() <= usize::try_from(value).unwrap_or(usize::MAX),
            "case {case:?}: expected between 1 and {value} items, got {:?}",
            reply.body
        );
    }
}

/// RULE: a `NextToken` this service never issued is the modelled 400 rather than a page, so
/// a client cannot page a store it was never given a cursor into.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let router = emulator();
    for n in 0..3u32 {
        let reply = create(&router, json!({"Name": format!("locker-{n}")})).await;
        accepted_id(&format!("seed-{n}"), &reply);
    }

    // The rows differ along the form of the token: not a cursor at all, a short one, one of
    // the right length that is not hex, and a well-formed cursor beyond anything this store
    // has minted. Every one matches the modelled `NextToken` pattern, so the boundary
    // validator passes it and the handler is what must refuse it.
    for token in [
        "nonsense",
        "deadbeef",
        "zzzzzzzzzzzzzzzz",
        "00000000ffffffff",
        "=",
    ] {
        let case = format!("NextToken={token}");
        let reply = list(&router, &format!("?NextToken={token}")).await;
        assert_refused(&case, &reply, 400, VALIDATION);
    }

    // A token the service DID issue still pages, so the refusals above changed nothing.
    let first = list(&router, "?MaxResults=1").await;
    let token = first
        .string("NextToken")
        .expect("a store of three records answers a NextToken for a page of one")
        .to_string();
    let second = list(&router, &format!("?MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        second.status, 200,
        "case \"issued token\": expected HTTP 200, got {} with body {:?}",
        second.status, second.body
    );
}

/// RULE: a credential locker is regional — only the region whose endpoint created it lists
/// or reads it — and an identifier a region holds no locker under is the modelled 404
/// naming the resource type and the identifier the request carried.
#[tokio::test]
async fn a_locker_is_only_visible_in_the_region_that_created_it() {
    let router = emulator();

    // The rows differ along the region the create was signed for.
    let regions = ["us-east-1", "eu-central-1", "ap-southeast-2"];
    let mut per_region: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let reply = create_in(&router, region, json!({"Name": "Shared Name"})).await;
        let id = accepted_id(&format!("create in {region}"), &reply);
        assert!(
            reply.string("Arn").unwrap_or_default().contains(region),
            "case {region:?}: expected the ARN to carry the signing region, got {:?}",
            reply.string("Arn")
        );
        per_region.push((region, id));
    }

    for (region, id) in &per_region {
        // The region that created it lists and reads exactly its own record.
        let listed = list_in(&router, region, "").await;
        assert_lists(region, &listed, std::slice::from_ref(id));
        let read = get_in(&router, region, id).await;
        assert_eq!(
            read.status, 200,
            "case {region:?}: expected HTTP 200 reading its own record, got {} with body {:?}",
            read.status, read.body
        );

        // No other region resolves it.
        for (other, _) in per_region.iter().filter(|(other, _)| other != region) {
            let case = format!("{id} from {other}");
            let elsewhere = get_in(&router, other, id).await;
            assert_refused(&case, &elsewhere, 404, NOT_FOUND);
            assert_eq!(
                elsewhere.string("ResourceType"),
                Some(CREDENTIAL_LOCKER_RESOURCE_TYPE),
                "case {case:?}: expected the 404 to name the resource type \
                 {CREDENTIAL_LOCKER_RESOURCE_TYPE:?}, got {:?}",
                elsewhere.string("ResourceType")
            );
            assert_eq!(
                elsewhere.string("ResourceId"),
                Some(id.as_str()),
                "case {case:?}: expected the 404 to report the identifier the request carried, \
                 got {:?}",
                elsewhere.string("ResourceId")
            );
        }
    }

    // An identifier nothing minted, in several forms the `CredentialLockerId` pattern
    // admits, including one at the modelled maximum length.
    for id in [
        "a".to_string(),
        "NoSuchLocker1".to_string(),
        "0123456789".to_string(),
        "b".repeat(CREDENTIAL_LOCKER_ID_BOUNDS.1),
    ] {
        let read = get(&router, &id).await;
        assert_refused(&format!("never minted {id}"), &read, 404, NOT_FOUND);
        // Every credential-locker 404 carries the one reason this resource's paths word,
        // whichever identifier was asked for.
        assert_eq!(
            read.message(),
            CREDENTIAL_LOCKER_NOT_FOUND_MESSAGE,
            "case {id:?}: expected the modelled reason \
             {CREDENTIAL_LOCKER_NOT_FOUND_MESSAGE:?}, got {:?}",
            read.message()
        );
    }
}

/// RULE: a retry carrying the `ClientToken` of a create that already succeeded answers with
/// that create's record and performs no further action, so the store still holds one locker
/// for it.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_locker() {
    // The rows differ along the token's form (the `ClientToken` pattern is
    // `^[a-zA-Z0-9=_-]+$`, length 1..64) and along the body it created.
    let cases: Vec<(&str, Value)> = vec![
        ("t", json!({})),
        ("token-1", json!({"Name": "Second"})),
        ("TOKEN_2=", json!({"Name": "Third", "Tags": {"k": "v"}})),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            json!({"Name": "Fourth"}),
        ),
    ];

    for (token, body) in cases {
        let router = emulator();
        let mut request = body.clone();
        request["ClientToken"] = json!(token);

        let first = create(&router, request.clone()).await;
        let id = accepted_id(token, &first);

        for attempt in 2..=3 {
            let case = format!("{token}/attempt {attempt}");
            let retry = create(&router, request.clone()).await;
            let replayed = accepted_id(&case, &retry);
            assert_eq!(
                replayed, id,
                "case {case:?}: a replayed ClientToken answers with the first identifier {id:?}, \
                 got {replayed:?}"
            );
            assert_eq!(
                retry.seconds("CreatedAt"),
                first.seconds("CreatedAt"),
                "case {case:?}: a replay reports the original CreatedAt, got {:?} after {:?}",
                retry.member("CreatedAt"),
                first.member("CreatedAt")
            );
        }

        // It performed no further action: the store holds exactly the first record.
        let listed = list(&router, "").await;
        assert_lists(token, &listed, std::slice::from_ref(&id));
    }
}

/// RULE: a delete removes exactly the locker its `Identifier` named, so the reads stop
/// reporting it while every other record keeps its place in the list, and an identifier the
/// region holds no locker under is the modelled 404.
#[tokio::test]
async fn a_delete_removes_exactly_the_locker_it_names() {
    // The rows differ along which record of the store is deleted — the first, one in the
    // middle, the last and (over the whole table) every one of them — so a delete cannot be
    // mistaken for "drop the head" or "truncate the tail".
    for victim in 0..4usize {
        let router = emulator();
        let mut inserted: Vec<String> = Vec::new();
        for n in 0..4u32 {
            let reply = create(&router, json!({"Name": format!("locker-{n}")})).await;
            inserted.push(accepted_id(&format!("seed-{n}"), &reply));
        }

        let case = format!("delete index {victim}");
        let id = inserted[victim].clone();
        let removed = delete(&router, &id).await;
        assert_eq!(
            removed.status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {} with body {:?}",
            removed.status, removed.body
        );

        // The reads stop reporting it, and the records around it keep their order.
        let gone = get(&router, &id).await;
        assert_refused(&case, &gone, 404, NOT_FOUND);
        let expected: Vec<String> = inserted
            .iter()
            .filter(|held| *held != &id)
            .cloned()
            .collect();
        assert_lists(&case, &list(&router, "").await, &expected);

        // Deleting it again finds nothing: a delete is not a toggle.
        let again = delete(&router, &id).await;
        assert_refused(&format!("{case}, repeated"), &again, 404, NOT_FOUND);
        assert_eq!(
            again.message(),
            CREDENTIAL_LOCKER_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the modelled reason \
             {CREDENTIAL_LOCKER_NOT_FOUND_MESSAGE:?}, got {:?}",
            again.message()
        );
        assert_eq!(
            again.string("ResourceType"),
            Some(CREDENTIAL_LOCKER_RESOURCE_TYPE),
            "case {case:?}: expected the 404 to name the resource type \
             {CREDENTIAL_LOCKER_RESOURCE_TYPE:?}, got {:?}",
            again.string("ResourceType")
        );
        assert_eq!(
            again.string("ResourceId"),
            Some(id.as_str()),
            "case {case:?}: expected the 404 to report the identifier the request carried, \
             got {:?}",
            again.string("ResourceId")
        );
    }
}

/// RULE: the store is bounded — at `MAX_CREDENTIAL_LOCKERS` records a create is refused with
/// the modelled quota error rather than growing the store, and the reads still report
/// exactly what is stored.
#[tokio::test]
async fn a_create_past_the_maximum_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Seed the store to its cap directly: driving the bound through the wire would be
    // thousands of requests to assert one refusal.
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        for n in 0..MAX_CREDENTIAL_LOCKERS {
            let id = format!("seeded{n}");
            let sequence = store.take_sequence();
            store.credential_lockers.insert(
                id.clone(),
                CredentialLocker {
                    id: id.clone(),
                    arn: format!("arn:aws:iotmanagedintegrations:{REGION}:000000000000:credential-locker/{id}"),
                    region: REGION.to_string(),
                    name: Some(format!("seeded locker {n}")),
                    tags: std::collections::HashMap::new(),
                    created_at: std::time::SystemTime::now(),
                    sequence,
                },
            );
            store.credential_locker_order.push(id);
        }
    }

    let refused = create(&router, json!({"Name": "one too many"})).await;
    assert_refused("at-the-maximum", &refused, 402, QUOTA_EXCEEDED);
    assert!(
        refused.message().contains(&MAX_CREDENTIAL_LOCKERS.to_string()),
        "case \"at-the-maximum\": expected the refusal to state the bound it enforced, got {:?}",
        refused.message()
    );
    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .credential_lockers
            .len(),
        MAX_CREDENTIAL_LOCKERS,
        "case \"at-the-maximum\": a refused create must not have grown the store"
    );

    // The list still pages the records that ARE stored, bounded by one page.
    let page = list(&router, "").await;
    assert_eq!(
        page.status, 200,
        "case \"at-the-maximum\": expected HTTP 200 from the list, got {} with body {:?}",
        page.status, page.body
    );
    assert_eq!(
        page.items().len(),
        CREDENTIAL_LOCKER_PAGE_LIMIT,
        "case \"at-the-maximum\": expected a full page of {CREDENTIAL_LOCKER_PAGE_LIMIT} items, \
         got {}",
        page.items().len()
    );
    assert!(
        page.member("NextToken").is_some(),
        "case \"at-the-maximum\": a store past one page answers a NextToken, got body {:?}",
        page.body
    );
}
