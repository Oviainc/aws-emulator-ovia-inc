// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the data set lifecycle: `CreateDataSet`, `GetDataSet`,
//! `UpdateDataSet`, `ListDataSets`, `CreateRevision`, `ListDataSetRevisions` and
//! `DeleteDataSet`.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, so the route, the
//! status code, the boundary validator and the serialized body are all exercised.
//!
//! The tables span the classes the rules generalise over — every value of the
//! model's `AssetType` enum, both `Origin` values, page sizes at and past the
//! modelled `MaxResults` bounds — rather than one recorded example.

use std::collections::HashSet;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs with, so the ARNs under test are region-sensitive
/// rather than matching a hardcoded default.
pub(crate) const REGION: &str = "us-west-2";

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// Every value the model's `com.amazonaws.dataexchange#AssetType` enum admits.
/// Source: `smithy_query(query="shape", shape="AssetType")`, mirrored by
/// `crate::ops::types::ASSET_TYPE_VALUES`.
const ASSET_TYPES: &[&str] = &[
    "API_GATEWAY_API",
    "LAKE_FORMATION_DATA_PERMISSION",
    "REDSHIFT_DATA_SHARE",
    "S3_DATA_ACCESS",
    "S3_SNAPSHOT",
];

/// Both values the model's `com.amazonaws.dataexchange#Origin` enum admits.
/// Source: `smithy_query(query="shape", shape="Origin")`.
const ORIGINS: &[&str] = &["OWNED", "ENTITLED"];

/// The published bounds of `com.amazonaws.dataexchange#MaxResults`
/// (`@range(min: 1, max: 200)`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 200;

/// One HTTP answer, reduced to what the assertions read.
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The restJson1 error code, from `x-amzn-errortype` or the body's `__type`.
    pub(crate) error: Option<String>,
    pub(crate) body: Value,
}

impl Answer {
    pub(crate) fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    pub(crate) fn string(&self, name: &str) -> String {
        self.member(name)
            .as_str()
            .unwrap_or_else(|| panic!("member {name} is not a string: {:?}", self.body))
            .to_string()
    }
}

/// A fresh emulator holding only this service, over its own engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    harness().0
}

/// The same emulator, handed back WITH the engine its state lives in — for an
/// assertion about what a call did or did not persist, and for a stored state no
/// implemented operation can reach yet.
pub(crate) fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's, not this service's, concern; the
    // tests assert handler behaviour, so it is switched off explicitly rather than
    // left to depend on the process default.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The same emulator with the S3 service mounted BESIDE this one on the one shared
/// engine — what the assembled router does — handed back with that engine too.
///
/// For the suite that drives both front doors (`tests/all/s3_data_access_jobs.rs`):
/// `StartJob` reads the source of an Amazon S3 data access by calling S3 in process,
/// so S3 has to be there for its answer to be S3's.
pub(crate) fn harness_with_object_storage() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Send one restJson1 request and reduce the answer.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let host = format!("dataexchange.{REGION}.amazonaws.com");
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        // The credential scope is what names the service (so the router claims the
        // request) and the region (so the ARNs carry it).
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=TESTKEY/20240101/{REGION}/dataexchange/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    let payload = match &body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Bytes::from(serde_json::to_vec(value).expect("serializable request body"))
        }
        None => Bytes::new(),
    };
    let response = router
        .dispatch(request.body(payload).expect("a well-formed test request"))
        .await;

    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(short_error_code);
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let error = header_error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(short_error_code)
    });
    Answer {
        status,
        error,
        body,
    }
}

/// `com.amazonaws.dataexchange#ValidationException` / `.../ValidationException:http://..`
/// both reduce to `ValidationException`, the code a client branches on.
fn short_error_code(raw: &str) -> String {
    raw.split(':')
        .next()
        .unwrap_or(raw)
        .rsplit(['#', '/'])
        .next()
        .unwrap_or(raw)
        .to_string()
}

/// The modelled `Id` pattern `^[a-zA-Z0-9]{30,40}$`.
pub(crate) fn is_modelled_id(value: &str) -> bool {
    (30..=40).contains(&value.chars().count())
        && value.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Seconds since the epoch for a `date-time` timestamp member, so two timestamps
/// are compared numerically rather than as text.
///
/// restJson1 renders a timestamp as epoch seconds unless the member carries
/// `@timestampFormat`, so both renderings are accepted here: the assertions are
/// about the instants, not about which of the protocol's encodings carries them.
pub(crate) fn seconds(value: &Value, case: &str) -> f64 {
    if let Some(epoch) = value.as_f64() {
        return epoch;
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: timestamp is neither a number nor a string: {value:?}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case}: timestamp {text:?} is not ISO 8601: {e}"))
        .as_secs_f64()
}

pub(crate) async fn create_data_set(
    router: &MultiServiceRouter,
    name: &str,
    description: &str,
    asset_type: &str,
    tags: Option<Value>,
) -> Answer {
    let mut body = json!({
        "AssetType": asset_type,
        "Description": description,
        "Name": name,
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    call(router, "POST", "/v1/data-sets", Some(body)).await
}

/// RULE: `CreateDataSet` persists the data set and answers out of the stored
/// record, so `GetDataSet` returns the same `Id`, `Arn`, `AssetType`, `Name`,
/// `Description`, `Origin`, `Tags` and timestamps — for EVERY `AssetType` the
/// model admits, with or without tags, whatever the description's size.
#[tokio::test]
async fn create_then_get_round_trips_every_asset_type() {
    let router = emulator();
    let mut minted: HashSet<String> = HashSet::new();

    // Rows differ along asset type (the whole modelled catalog), tag count
    // (none / one / several) and description size (short / long).
    let rows: Vec<(&str, String, Option<Value>)> = ASSET_TYPES
        .iter()
        .enumerate()
        .map(|(index, asset_type)| {
            let description = match index % 2 {
                0 => "a short description".to_string(),
                _ => "d".repeat(4096),
            };
            let tags = match index % 3 {
                0 => None,
                1 => Some(json!({ "team": "data" })),
                _ => Some(json!({ "team": "data", "tier": "gold", "owner": "ops" })),
            };
            (*asset_type, description, tags)
        })
        .collect();

    for (asset_type, description, tags) in &rows {
        let case = format!("asset_type={asset_type} tags={tags:?}");
        let name = format!("set-{asset_type}");
        let created = create_data_set(&router, &name, description, asset_type, tags.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected 201 from CreateDataSet, got {} body {:?}",
            created.status, created.body
        );

        let id = created.string("Id");
        assert!(
            is_modelled_id(&id),
            "case {case}: expected Id to match ^[a-zA-Z0-9]{{30,40}}$, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case}: expected a freshly minted Id, got the already-issued {id:?}"
        );
        assert_eq!(
            created.string("Arn"),
            format!("arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{id}"),
            "case {case}: expected the Arn to be derived from the region, account and Id"
        );
        assert_eq!(
            created.string("Origin"),
            "OWNED",
            "case {case}: expected Origin=OWNED for a data set this account created"
        );
        assert_eq!(
            created.string("AssetType"),
            *asset_type,
            "case {case}: expected the requested AssetType back"
        );
        assert_eq!(
            seconds(created.member("CreatedAt"), &case),
            seconds(created.member("UpdatedAt"), &case),
            "case {case}: expected CreatedAt == UpdatedAt on a freshly created data set"
        );
        match tags {
            None => assert!(
                created.member("Tags").is_null(),
                "case {case}: expected no Tags member for an untagged data set, got {:?}",
                created.member("Tags")
            ),
            Some(expected) => assert_eq!(
                created.member("Tags"),
                expected,
                "case {case}: expected the requested Tags back"
            ),
        }

        let fetched = call(&router, "GET", &format!("/v1/data-sets/{id}"), None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected 200 from GetDataSet, got {} body {:?}",
            fetched.status, fetched.body
        );
        for member in [
            "Arn",
            "AssetType",
            "CreatedAt",
            "Description",
            "Id",
            "Name",
            "Origin",
            "UpdatedAt",
            "Tags",
        ] {
            assert_eq!(
                fetched.member(member),
                created.member(member),
                "case {case}: expected GetDataSet to report the persisted {member}, \
                 got {:?} after a create that answered {:?}",
                fetched.member(member),
                created.member(member)
            );
        }
        assert_eq!(
            fetched.string("Name"),
            name,
            "case {case}: expected the stored Name, got {:?}",
            fetched.member("Name")
        );
        assert_eq!(
            fetched.member("Description").as_str(),
            Some(description.as_str()),
            "case {case}: expected the stored Description of {} characters",
            description.chars().count()
        );
    }
}

/// RULE: a read never changes what a later caller sees — `GetDataSet` repeated
/// reports the identical record.
#[tokio::test]
async fn repeated_get_is_stable() {
    let router = emulator();
    let created = create_data_set(&router, "stable", "unchanged by reads", "S3_SNAPSHOT", None)
        .await;
    let id = created.string("Id");

    let first = call(&router, "GET", &format!("/v1/data-sets/{id}"), None).await;
    let second = call(&router, "GET", &format!("/v1/data-sets/{id}"), None).await;
    assert_eq!(
        (first.status, &first.body),
        (second.status, &second.body),
        "case repeated GetDataSet: expected an identical answer, got {:?} then {:?}",
        first.body,
        second.body
    );
}

/// RULE: `UpdateDataSet` merges only the members the request carries into the
/// stored record and bumps `UpdatedAt`; an absent member keeps its stored value,
/// `CreatedAt` never moves, and `GetDataSet` afterwards shows the merged record.
#[tokio::test]
async fn update_merges_only_the_supplied_members() {
    let router = emulator();

    // Rows differ along WHICH optional members the PATCH carries.
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("name only", Some("renamed"), None),
        ("description only", None, Some("a new description")),
        ("both", Some("renamed-again"), Some("another description")),
        ("neither", None, None),
    ];

    for (case, name, description) in rows {
        let created = create_data_set(&router, "original", "original text", "S3_SNAPSHOT", None)
            .await;
        let id = created.string("Id");
        let created_at = seconds(created.member("CreatedAt"), case);

        let mut patch = json!({});
        if let Some(name) = name {
            patch["Name"] = json!(name);
        }
        if let Some(description) = description {
            patch["Description"] = json!(description);
        }
        let updated = call(
            &router,
            "PATCH",
            &format!("/v1/data-sets/{id}"),
            Some(patch),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateDataSet, got {} body {:?}",
            updated.status, updated.body
        );

        let expected_name = name.unwrap_or("original");
        let expected_description = description.unwrap_or("original text");
        assert_eq!(
            updated.string("Name"),
            expected_name,
            "case {case}: expected Name {expected_name:?}, got {:?}",
            updated.member("Name")
        );
        assert_eq!(
            updated.string("Description"),
            expected_description,
            "case {case}: expected Description {expected_description:?}, got {:?}",
            updated.member("Description")
        );
        assert_eq!(
            seconds(updated.member("CreatedAt"), case),
            created_at,
            "case {case}: expected CreatedAt to be unchanged by an update"
        );
        assert!(
            seconds(updated.member("UpdatedAt"), case) >= created_at,
            "case {case}: expected UpdatedAt at or after CreatedAt, got {:?} vs {:?}",
            updated.member("UpdatedAt"),
            created.member("CreatedAt")
        );

        let fetched = call(&router, "GET", &format!("/v1/data-sets/{id}"), None).await;
        assert_eq!(
            fetched.string("Name"),
            expected_name,
            "case {case}: expected GetDataSet to show the merged Name, got {:?}",
            fetched.member("Name")
        );
        assert_eq!(
            fetched.string("Description"),
            expected_description,
            "case {case}: expected GetDataSet to show the merged Description, got {:?}",
            fetched.member("Description")
        );
        assert_eq!(
            fetched.member("UpdatedAt"),
            updated.member("UpdatedAt"),
            "case {case}: expected the persisted UpdatedAt to be the one UpdateDataSet reported"
        );
    }
}

/// Walk every page of `ListDataSets` with `maxResults=page_size`, returning the
/// ids in the order they were served.
///
/// `maxResults` is sent on every page here because the page SIZE is what this walk
/// varies; a request that omits it is its own case, in
/// `optional_page_size::an_absent_page_size_is_absent_not_the_modelled_default`.
async fn list_all_data_sets(
    router: &MultiServiceRouter,
    page_size: i64,
    case: &str,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..100 {
        let mut query: Vec<String> = vec![format!("maxResults={page_size}")];
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let uri = format!("/v1/data-sets?{}", query.join("&"));
        let answer = call(router, "GET", &uri, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from ListDataSets page {page}, got {} body {:?}",
            answer.status, answer.body
        );
        let entries = answer.member("DataSets").as_array().cloned().unwrap_or_default();
        assert!(
            entries.len() as i64 <= page_size,
            "case {case}: expected at most {page_size} entries on page {page}, got {}",
            entries.len()
        );
        for entry in entries {
            ids.push(
                entry["Id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: a DataSetEntry carries no Id"))
                    .to_string(),
            );
        }
        token = answer.member("NextToken").as_str().map(str::to_string);
        if token.is_none() {
            return ids;
        }
    }
    panic!("case {case}: ListDataSets never stopped handing out a NextToken");
}

/// RULE: a paginated list enumerates exactly the resources state holds, in a
/// stable order, bounded by `maxResults`, and hands out a `NextToken` only while
/// items remain — so walking every page yields each resource exactly once,
/// whatever the page size.
#[tokio::test]
async fn list_data_sets_pages_over_exactly_what_state_holds() {
    let router = emulator();
    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let answer = create_data_set(
            &router,
            &format!("set-{index}"),
            "paged",
            ASSET_TYPES[index % ASSET_TYPES.len()],
            None,
        )
        .await;
        created.push(answer.string("Id"));
    }

    // Rows differ along the page size: the modelled minimum, sizes that do and do
    // not divide the collection, a size larger than it, and the modelled maximum.
    let page_sizes: Vec<i64> = vec![MAX_RESULTS_MIN, 2, 3, 7, 50, MAX_RESULTS_MAX];

    for page_size in page_sizes {
        let case = format!("maxResults={page_size}");
        let served = list_all_data_sets(&router, page_size, &case).await;
        assert_eq!(
            served, created,
            "case {case}: expected every data set in state exactly once, in creation order; \
             got {served:?} for {created:?}"
        );
    }
}

/// RULE: `maxResults` outside the modelled `@range(min: 1, max: 200)` is refused
/// as a 400 `ValidationException`; the bounds themselves are accepted.
#[tokio::test]
async fn list_data_sets_enforces_the_modelled_max_results_range() {
    let router = emulator();
    create_data_set(&router, "bounded", "range", "S3_SNAPSHOT", None).await;

    for accepted in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let answer = call(
            &router,
            "GET",
            &format!("/v1/data-sets?maxResults={accepted}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={accepted}: expected the published bound to be accepted, \
             got {} body {:?}",
            answer.status, answer.body
        );
    }

    for refused in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer = call(
            &router,
            "GET",
            &format!("/v1/data-sets?maxResults={refused}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={refused}: expected 400 one past the published bound, \
             got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ValidationException"),
            "case maxResults={refused}: expected ValidationException, got {:?}",
            answer.error
        );
    }

    // An OMITTED `maxResults` is not a value and is not range-checked at all: see
    // `optional_page_size::an_absent_page_size_is_absent_not_the_modelled_default`,
    // which pins that case across every list operation of this service.
}

/// RULE: `origin` filters the listing to the data sets carrying that `Origin`, and
/// a value outside the modelled `Origin` set is refused rather than answered with
/// an empty page.
#[tokio::test]
async fn list_data_sets_filters_by_origin() {
    let router = emulator();
    let mut owned: Vec<String> = Vec::new();
    for index in 0..3 {
        let answer =
            create_data_set(&router, &format!("owned-{index}"), "o", "S3_SNAPSHOT", None).await;
        owned.push(answer.string("Id"));
    }

    for origin in ORIGINS {
        let answer = call(
            &router,
            "GET",
            &format!("/v1/data-sets?maxResults={MAX_RESULTS_MAX}&origin={origin}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case origin={origin}: expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        let served: Vec<String> = answer
            .member("DataSets")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        // Only CreateDataSet mints data sets here, and those are OWNED, so the
        // ENTITLED page is empty by construction.
        let expected: Vec<String> = if *origin == "OWNED" {
            owned.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            served, expected,
            "case origin={origin}: expected {expected:?}, got {served:?}"
        );
    }

    let answer = call(
        &router,
        "GET",
        &format!("/v1/data-sets?maxResults={MAX_RESULTS_MAX}&origin=NOT_AN_ORIGIN"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case origin=NOT_AN_ORIGIN: expected 400 for a value outside the Origin set, \
         got {} body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ValidationException"),
        "case origin=NOT_AN_ORIGIN: expected ValidationException, got {:?}",
        answer.error
    );
    // The refusal states WHICH parameter is not valid and the set the model admits for
    // it, in the model's own order - so a client can fix the request from the message
    // alone, and the refusal cannot be mistaken for one about another parameter.
    assert_eq!(
        answer.string("Message"),
        format!(
            "The following parameter(s) are not valid: \"origin\" must be one of [{}].",
            service_com_amazonaws_dataexchange_dataexchange::state::ORIGIN_VALUES_IN_MODEL_ORDER
                .join(", ")
        ),
        "case origin=NOT_AN_ORIGIN: expected the parameter refusal to name the admitted \
         set, got {:?}",
        answer.body
    );
}

/// RULE: a `NextToken` the service did not issue is refused as a 400
/// `ValidationException` — never answered with a silently different page.
#[tokio::test]
async fn an_unparsable_next_token_is_refused() {
    let router = emulator();
    let created = create_data_set(&router, "tokens", "t", "S3_SNAPSHOT", None).await;
    let id = created.string("Id");
    call(
        &router,
        "POST",
        &format!("/v1/data-sets/{id}/revisions"),
        Some(json!({})),
    )
    .await;

    // Rows differ along HOW the token is wrong: not hex, hex of the wrong layout,
    // hex of an unknown scheme version, hex with a non-numeric offset, hex naming
    // another account, and a token longer than any this service can issue.
    let tokens = [
        ("not hex at all", "zzzz".to_string()),
        ("hex of a bare word", hex_of("nonsense")),
        (
            "hex of an unknown version",
            hex_of(&format!("9|{ACCOUNT}|0|abc")),
        ),
        (
            "hex with a non-numeric offset",
            hex_of(&format!("1|{ACCOUNT}|x|abc")),
        ),
        (
            "hex naming another account",
            hex_of("1|210987654321|0|abc"),
        ),
        (
            "longer than any issued token",
            hex_of(&format!("1|{ACCOUNT}|0|{}", "a".repeat(4096))),
        ),
    ];

    for (case, token) in tokens {
        for uri in [
            format!("/v1/data-sets?maxResults=10&nextToken={token}"),
            format!("/v1/data-sets/{id}/revisions?maxResults=10&nextToken={token}"),
        ] {
            let answer = call(&router, "GET", &uri, None).await;
            assert_eq!(
                answer.status, 400,
                "case {case} on {uri}: expected 400, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case} on {uri}: expected ValidationException, got {:?}",
                answer.error
            );
            // Every token this service issues carries the account it was issued to, so
            // the refusal of one that does not is about exactly that.
            assert_eq!(
                answer.string("Message"),
                "Invalid NextToken. Account Id does not match Caller's Account Id.",
                "case {case} on {uri}: expected the NextToken refusal, got {:?}",
                answer.body
            );
        }
    }
}

fn hex_of(text: &str) -> String {
    hex::encode(text)
}

/// RULE: a token this service DID issue but whose page no longer exists — because
/// the collection shrank, or because it was minted against a longer one — comes
/// back as an empty final page, never as a panic or a 500.
///
/// The rows differ along how far past the end the cursor's recorded offset sits,
/// including an offset no collection could ever reach.
#[tokio::test]
async fn a_stale_cursor_past_the_end_serves_an_empty_final_page() {
    let router = emulator();
    let data_set = create_data_set(&router, "shrinking", "s", "S3_SNAPSHOT", None).await;
    let data_set_id = data_set.string("Id");
    call(
        &router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions"),
        Some(json!({})),
    )
    .await;

    // A cursor the service's own encoder would produce for this account, whose
    // anchor is gone and whose offset is past the end of the collection.
    for offset in [1_usize, 2, 9_999, usize::MAX] {
        let token = hex_of(&format!(
            "1|{ACCOUNT}|{offset}|00000000000000000000000000000000"
        ));
        for (case, uri) in [
            (
                "data sets",
                format!("/v1/data-sets?maxResults=10&nextToken={token}"),
            ),
            (
                "revisions",
                format!("/v1/data-sets/{data_set_id}/revisions?maxResults=10&nextToken={token}"),
            ),
        ] {
            let answer = call(&router, "GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case} offset={offset}: expected 200 with an empty page, \
                 got {} body {:?}",
                answer.status, answer.body
            );
            let items = answer.member(if case == "data sets" {
                "DataSets"
            } else {
                "Revisions"
            });
            assert_eq!(
                items,
                &json!([]),
                "case {case} offset={offset}: expected an empty page, got {items:?}"
            );
            assert!(
                answer.member("NextToken").is_null(),
                "case {case} offset={offset}: expected no further NextToken, got {:?}",
                answer.member("NextToken")
            );
        }
    }
}

/// RULE: a caller-sized member the model does not bound is still bounded by the
/// store, and a value past that bound is refused with a 400 `ValidationException`
/// naming the member — while the bound itself is accepted.
#[tokio::test]
async fn caller_sized_members_are_bounded() {
    use service_com_amazonaws_dataexchange_dataexchange::state::{
        MAX_DESCRIPTION_CHARS, MAX_NAME_CHARS, MAX_TAGS, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
    };

    let router = emulator();

    // Rows: the member under test, the value at its bound, and the value one past
    // it. Every bound is exercised on both sides.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "Name",
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n".repeat(MAX_NAME_CHARS) }),
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n".repeat(MAX_NAME_CHARS + 1) }),
        ),
        (
            "Description",
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d".repeat(MAX_DESCRIPTION_CHARS), "Name": "n" }),
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d".repeat(MAX_DESCRIPTION_CHARS + 1), "Name": "n" }),
        ),
        (
            "Tags entry count",
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n", "Tags": tag_map(MAX_TAGS) }),
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n", "Tags": tag_map(MAX_TAGS + 1) }),
        ),
        (
            "Tags key length",
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n", "Tags": { "k".repeat(MAX_TAG_KEY_CHARS): "v" } }),
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n", "Tags": { "k".repeat(MAX_TAG_KEY_CHARS + 1): "v" } }),
        ),
        (
            "Tags value length",
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n", "Tags": { "k": "v".repeat(MAX_TAG_VALUE_CHARS) } }),
            json!({ "AssetType": "S3_SNAPSHOT", "Description": "d", "Name": "n", "Tags": { "k": "v".repeat(MAX_TAG_VALUE_CHARS + 1) } }),
        ),
    ];

    for (case, at_bound, past_bound) in rows {
        let accepted = call(&router, "POST", "/v1/data-sets", Some(at_bound)).await;
        assert_eq!(
            accepted.status, 201,
            "case {case} at the bound: expected 201, got {} body {:?}",
            accepted.status, accepted.body
        );

        let refused = call(&router, "POST", "/v1/data-sets", Some(past_bound)).await;
        assert_eq!(
            refused.status, 400,
            "case {case} past the bound: expected 400, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("ValidationException"),
            "case {case} past the bound: expected ValidationException, got {:?}",
            refused.error
        );
    }

    // The same bounds hold on the update path, which replaces what the store holds.
    let id = create_data_set(&router, "updatable", "d", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    for (case, patch) in [
        (
            "Name",
            json!({ "Name": "n".repeat(MAX_NAME_CHARS + 1) }),
        ),
        (
            "Description",
            json!({ "Description": "d".repeat(MAX_DESCRIPTION_CHARS + 1) }),
        ),
    ] {
        let refused = call(
            &router,
            "PATCH",
            &format!("/v1/data-sets/{id}"),
            Some(patch.clone()),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case UpdateDataSet {case} past the bound: expected 400, got {} body {:?}",
            refused.status, refused.body
        );

        // The resource is resolved first, so an unknown id answers 404 even when the
        // body would also have been refused.
        let missing = call(
            &router,
            "PATCH",
            "/v1/data-sets/00000000000000000000000000000000",
            Some(patch),
        )
        .await;
        assert_eq!(
            missing.status, 404,
            "case UpdateDataSet {case} past the bound on an unknown id: expected the \
             resource's 404, got {} body {:?}",
            missing.status, missing.body
        );
    }
}

/// A tag map with `count` distinct keys.
fn tag_map(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), json!("v"));
    }
    Value::Object(map)
}

/// RULE: each counted resource the store accumulates is capped, and the create that
/// would pass the cap is refused with the operation's own modelled error rather
/// than growing the store without bound.
#[tokio::test]
async fn the_counted_resources_are_capped() {
    use service_com_amazonaws_dataexchange_dataexchange::state::{
        MAX_DATA_SETS, MAX_REVISIONS_PER_DATA_SET,
    };

    let router = emulator();

    // Revisions first, on one data set, so the data-set cap is not in the way.
    let data_set_id = create_data_set(&router, "revision-cap", "d", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    for index in 0..MAX_REVISIONS_PER_DATA_SET {
        let answer = call(
            &router,
            "POST",
            &format!("/v1/data-sets/{data_set_id}/revisions"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case revision {index} of {MAX_REVISIONS_PER_DATA_SET}: expected 201, got {}",
            answer.status
        );
    }
    let refused = call(
        &router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions"),
        Some(json!({})),
    )
    .await;
    // `CreateRevision` models no ServiceLimitExceededException, so its own
    // ValidationException is the modelled refusal.
    assert_eq!(
        refused.status, 400,
        "case revision past the cap: expected 400, got {} body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ValidationException"),
        "case revision past the cap: expected ValidationException, got {:?}",
        refused.error
    );

    // One data set already exists, so fill the rest of the cap.
    for index in 1..MAX_DATA_SETS {
        let answer = create_data_set(&router, "filler", "d", "S3_SNAPSHOT", None).await;
        assert_eq!(
            answer.status, 201,
            "case data set {index} of {MAX_DATA_SETS}: expected 201, got {}",
            answer.status
        );
    }
    let refused = create_data_set(&router, "one too many", "d", "S3_SNAPSHOT", None).await;
    assert_eq!(
        refused.status, 402,
        "case data set past the cap: expected the modelled 402, got {} body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error.as_deref(),
        Some("ServiceLimitExceededException"),
        "case data set past the cap: expected ServiceLimitExceededException, got {:?}",
        refused.error
    );
    assert_eq!(
        refused.member("LimitName"),
        &json!("Data sets per account"),
        "case data set past the cap: expected the modelled LimitName, got {:?}",
        refused.member("LimitName")
    );
}

/// The `Origin` this service stamps on a data set it mints must stay a member of
/// the model's own `Origin` value set, so the hand-written constant and the
/// generated catalog cannot drift apart.
#[test]
fn the_owned_origin_constant_is_a_modelled_origin_value() {
    use service_com_amazonaws_dataexchange_dataexchange::ops::types::ORIGIN_VALUES;
    use service_com_amazonaws_dataexchange_dataexchange::state::{
        ORIGIN_OWNED, ORIGIN_VALUES_IN_MODEL_ORDER,
    };

    assert!(
        ORIGIN_VALUES.contains(&ORIGIN_OWNED),
        "case ORIGIN_OWNED: expected {ORIGIN_OWNED:?} to be one of the modelled \
         Origin values {ORIGIN_VALUES:?}"
    );
    assert_eq!(
        ORIGIN_VALUES.len(),
        ORIGINS.len(),
        "case Origin catalog: this suite's table lists {} values but the model has {}: {ORIGIN_VALUES:?}",
        ORIGINS.len(),
        ORIGIN_VALUES.len()
    );
    // The order a refusal quotes the set in is the model's, which the generated catalog
    // (sorted) cannot state - so the two are kept from drifting here: same count, same
    // members.
    assert_eq!(
        ORIGIN_VALUES_IN_MODEL_ORDER.len(),
        ORIGIN_VALUES.len(),
        "case Origin order: the model-order list {ORIGIN_VALUES_IN_MODEL_ORDER:?} and the \
         generated catalog {ORIGIN_VALUES:?} must hold the same values"
    );
    for origin in ORIGIN_VALUES_IN_MODEL_ORDER {
        assert!(
            ORIGIN_VALUES.contains(origin),
            "case Origin order: {origin:?} is not one of the modelled Origin values \
             {ORIGIN_VALUES:?}"
        );
    }
}

/// RULE: `CreateRevision` attaches a revision to an existing data set and
/// `ListDataSetRevisions` reports the stored revisions newest first, bounded by
/// `maxResults` with a working `NextToken`.
#[tokio::test]
async fn revisions_are_listed_newest_first_and_paginate() {
    let router = emulator();
    let data_set = create_data_set(&router, "with-revisions", "r", "S3_SNAPSHOT", None).await;
    let data_set_id = data_set.string("Id");

    // Rows differ along the comment: absent, short, and at the modelled
    // `__stringMin0Max16384` upper length.
    let comments: Vec<Option<String>> = vec![
        None,
        Some("first".to_string()),
        Some("c".repeat(16_384)),
        Some(String::new()),
        Some("last".to_string()),
    ];

    let mut created: Vec<(String, Option<String>)> = Vec::new();
    for comment in &comments {
        let case = format!("comment={:?}", comment.as_ref().map(String::len));
        let mut body = json!({});
        if let Some(comment) = comment {
            body["Comment"] = json!(comment);
        }
        let answer = call(
            &router,
            "POST",
            &format!("/v1/data-sets/{data_set_id}/revisions"),
            Some(body),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case}: expected 201 from CreateRevision, got {} body {:?}",
            answer.status, answer.body
        );
        let revision_id = answer.string("Id");
        assert!(
            is_modelled_id(&revision_id),
            "case {case}: expected a revision Id matching ^[a-zA-Z0-9]{{30,40}}$, \
             got {revision_id:?}"
        );
        assert_eq!(
            answer.string("Arn"),
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}/revisions/{revision_id}"
            ),
            "case {case}: expected the revision Arn to name its data set and itself"
        );
        assert_eq!(
            answer.string("DataSetId"),
            data_set_id,
            "case {case}: expected the revision to report its data set"
        );
        assert_eq!(
            answer.member("Finalized"),
            &json!(false),
            "case {case}: expected Finalized=false on a fresh revision, got {:?}",
            answer.member("Finalized")
        );
        assert_eq!(
            answer.member("Revoked"),
            &json!(false),
            "case {case}: expected Revoked=false on a fresh revision, got {:?}",
            answer.member("Revoked")
        );
        created.push((revision_id, comment.clone()));
    }

    // Newest first: the reverse of the order they were created in.
    let newest_first: Vec<String> = created.iter().rev().map(|(id, _)| id.clone()).collect();

    for page_size in [MAX_RESULTS_MIN, 2, 5, MAX_RESULTS_MAX] {
        let case = format!("revision maxResults={page_size}");
        let mut served: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query: Vec<String> = vec![format!("maxResults={page_size}")];
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let uri = format!(
                "/v1/data-sets/{data_set_id}/revisions?{}",
                query.join("&")
            );
            let answer = call(&router, "GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 from ListDataSetRevisions, got {} body {:?}",
                answer.status, answer.body
            );
            let entries = answer
                .member("Revisions")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() as i64 <= page_size,
                "case {case}: expected at most {page_size} revisions per page, got {}",
                entries.len()
            );
            for entry in entries {
                served.push(entry["Id"].as_str().unwrap_or_default().to_string());
            }
            token = answer.member("NextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            served, newest_first,
            "case {case}: expected the stored revisions newest first, got {served:?} \
             for {newest_first:?}"
        );
    }

    // The comment each revision was created with is the one reported back.
    let listed = call(
        &router,
        "GET",
        &format!("/v1/data-sets/{data_set_id}/revisions?maxResults={MAX_RESULTS_MAX}"),
        None,
    )
    .await;
    let entries = listed
        .member("Revisions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (revision_id, comment) in &created {
        let entry = entries
            .iter()
            .find(|entry| entry["Id"].as_str() == Some(revision_id.as_str()))
            .unwrap_or_else(|| panic!("case {revision_id}: the revision is missing from the list"));
        let expected = comment
            .as_ref()
            .map_or(Value::Null, |text| Value::String(text.clone()));
        assert_eq!(
            entry["Comment"], expected,
            "case {revision_id}: expected the stored Comment, got {:?}",
            entry["Comment"]
        );
    }
}

/// RULE: an operation keyed on a `DataSetId` that names no stored data set answers
/// `ResourceNotFoundException` (404), whatever the operation.
#[tokio::test]
async fn an_unknown_data_set_id_is_a_404_everywhere() {
    let router = emulator();
    // Well-formed per `^[a-zA-Z0-9]{30,40}$` (32 characters) but never created.
    let missing = "00000000000000000000000000000000";

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        ("GetDataSet", "GET", format!("/v1/data-sets/{missing}"), None),
        (
            "UpdateDataSet",
            "PATCH",
            format!("/v1/data-sets/{missing}"),
            Some(json!({ "Name": "renamed" })),
        ),
        (
            "DeleteDataSet",
            "DELETE",
            format!("/v1/data-sets/{missing}"),
            None,
        ),
        (
            "CreateRevision",
            "POST",
            format!("/v1/data-sets/{missing}/revisions"),
            Some(json!({ "Comment": "nope" })),
        ),
        (
            "ListDataSetRevisions",
            "GET",
            format!("/v1/data-sets/{missing}/revisions?maxResults={MAX_RESULTS_MAX}"),
            None,
        ),
    ];

    for (case, method, uri, body) in cases {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404 for an unknown DataSetId, got {} body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.error
        );
    }
}

/// RULE: a `DataSetId` outside the modelled `^[a-zA-Z0-9]{30,40}$` pattern is a 400
/// `ValidationException`, not a 404 — the request is malformed, not the resource
/// absent.
#[tokio::test]
async fn a_malformed_data_set_id_is_a_400() {
    let router = emulator();

    // Rows differ along HOW the id breaks the pattern: too short, too long, and a
    // disallowed character at the published length.
    let ids = [
        ("too short", "a".repeat(29)),
        ("at the minimum length", "a".repeat(30)),
        ("at the maximum length", "a".repeat(40)),
        ("too long", "a".repeat(41)),
        ("a hyphen is not alphanumeric", format!("{}-{}", "a".repeat(16), "b".repeat(15))),
    ];

    for (case, id) in ids {
        let answer = call(&router, "GET", &format!("/v1/data-sets/{id}"), None).await;
        let well_formed = is_modelled_id(&id);
        if well_formed {
            // Inside the pattern: the id is acceptable, so the answer is about the
            // resource, not the request.
            assert_eq!(
                answer.status, 404,
                "case {case}: expected 404 for a well-formed but unknown id {id:?}, \
                 got {} body {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 for {id:?}, got {} body {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {case}: expected ValidationException for {id:?}, got {:?}",
                answer.error
            );
        }
    }
}

/// RULE: `DeleteDataSet` removes the data set and the revisions it owns from state
/// and answers 204 with no body; afterwards the id names nothing, so `GetDataSet`
/// is a 404, `ListDataSets` no longer carries it, and a second delete is a 404 too.
#[tokio::test]
async fn delete_removes_the_data_set_and_its_revisions() {
    let router = emulator();
    let keep = create_data_set(&router, "keep", "k", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let doomed = create_data_set(&router, "doomed", "d", "S3_DATA_ACCESS", None)
        .await
        .string("Id");
    for _ in 0..2 {
        let answer = call(
            &router,
            "POST",
            &format!("/v1/data-sets/{doomed}/revisions"),
            Some(json!({ "Comment": "attached" })),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case setup: expected 201 from CreateRevision, got {} body {:?}",
            answer.status, answer.body
        );
    }

    let deleted = call(&router, "DELETE", &format!("/v1/data-sets/{doomed}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "case delete: expected 204 from DeleteDataSet, got {} body {:?}",
        deleted.status, deleted.body
    );
    assert!(
        deleted.body.is_null(),
        "case delete: expected an empty body on 204, got {:?}",
        deleted.body
    );

    let fetched = call(&router, "GET", &format!("/v1/data-sets/{doomed}"), None).await;
    assert_eq!(
        fetched.status, 404,
        "case get after delete: expected 404, got {} body {:?}",
        fetched.status, fetched.body
    );

    let listed = list_all_data_sets(&router, MAX_RESULTS_MIN, "list after delete").await;
    assert_eq!(
        listed,
        vec![keep.clone()],
        "case list after delete: expected only the surviving data set, got {listed:?}"
    );

    let listed_revisions = call(
        &router,
        "GET",
        &format!("/v1/data-sets/{doomed}/revisions?maxResults={MAX_RESULTS_MAX}"),
        None,
    )
    .await;
    assert_eq!(
        listed_revisions.status, 404,
        "case revisions after delete: expected 404 for a deleted data set, got {} body {:?}",
        listed_revisions.status, listed_revisions.body
    );

    let again = call(&router, "DELETE", &format!("/v1/data-sets/{doomed}"), None).await;
    assert_eq!(
        again.status, 404,
        "case delete twice: expected 404 the second time, got {} body {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error.as_deref(),
        Some("ResourceNotFoundException"),
        "case delete twice: expected ResourceNotFoundException, got {:?}",
        again.error
    );

    // Deleting one data set leaves the other reachable.
    let survivor = call(&router, "GET", &format!("/v1/data-sets/{keep}"), None).await;
    assert_eq!(
        survivor.status, 200,
        "case survivor: expected the untouched data set to still be readable, got {}",
        survivor.status
    );
}
