// SPDX-License-Identifier: CC0-1.0

//! The Kinesis Video Stream POOL lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the wire: the suite builds the real
//! `MultiServiceRouter`, mounts this service on one shared engine and dispatches
//! `http::Request`s, so what it pins is the emulator's HTTP behaviour and not an internal
//! helper. The [`Emulator`] harness is `pub(crate)` because the sibling suite
//! `media_insights_pipeline_status` drives the same wire surface and must not build a
//! second copy of it.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a create MINTS and PERSISTS a pool — its own `PoolId`, an ARN derived from the signed
//!   region and that id, the caller's `StreamConfiguration` stored as submitted — and
//!   every later read renders from the stored record;
//! * a pool is identified by its NAME or its ARN, and an identifier that resolves to no
//!   stored pool is this service's modelled 404 `NotFoundException`;
//! * a `PoolName` another live pool already carries is the modelled 409
//!   `ConflictException`, however many callers ask for it at once: of N concurrent creates
//!   of one name exactly one is served;
//! * a delete is observable by every later read and by the listing, and deleting twice is
//!   the same 404 as deleting something that never existed;
//! * `CREATING` is a non-terminal status: a caller reading straight back still sees it,
//!   and once the span has elapsed every read sees `ACTIVE`;
//! * an update applies the retention it carries, is observable by a later read, and puts
//!   the pool back into the transient `UPDATING` status;
//! * a repeated `ClientRequestToken` answers with the pool the first create minted;
//! * the listing pages in creation order and refuses a token it did not issue.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::{
    POOL_STATUS_ACTIVE, POOL_STATUS_CREATING, POOL_STATUS_UPDATING, POOL_TRANSIENT_TO_ACTIVE,
};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite signs for. The ARN a create derives must carry it, so it is
/// deliberately NOT the emulator's `us-east-1` default — and deliberately not always the
/// `Region` of the stream configuration either, which is the caller's own value.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical placeholder account every emulated ARN carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The collection route the pool operations hang off, from
/// `smithy_query(query="digest")`: `POST`/`GET /media-pipeline-kinesis-video-stream-pools`
/// and `GET`/`PUT`/`DELETE …/{Identifier}`.
const POOLS: &str = "/media-pipeline-kinesis-video-stream-pools";

/// The `ErrorCode` value the modelled `NotFoundException` carries in its body `Code`
/// member. Source: `smithy_query(query="shape",shape="ErrorCode")` on this service's model
/// — `BadRequest | Forbidden | NotFound | ResourceLimitExceeded | ServiceFailure |
/// ServiceUnavailable | Throttling`.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// The wire error-shape names restJson1 resolves from `x-amzn-errortype`.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";
const CONFLICT_EXCEPTION: &str = "ConflictException";
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// One request/response round trip through the router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The engine the service is mounted on, kept so a suite can set up the part of the
    /// store no modelled operation of this service writes (the account's voice-analytics
    /// enrolment) instead of building a second engine, which would be a private store.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// This emulator's store, reached exactly as a handler reaches it.
    pub(crate) fn store(
        &self,
    ) -> engine::ServiceState<
        service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::ComAmazonawsChimesdkmediapipelinesChimesdkmediapipelinesserviceState,
    > {
        service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::service_state(&*self.engine)
            .expect("the in-memory engine serves this service's store")
    }

    /// Dispatch one request and return what the rows assert on (`Value::Null` for an empty
    /// body, which is what a 204 carries).
    pub(crate) async fn call(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<Value>,
    ) -> Response {
        let payload = body.map_or_else(Vec::new, |b| {
            serde_json::to_vec(&b).expect("a JSON value serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://media-pipelines-chime.{REGION}.amazonaws.com{path_and_query}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTSUITE/20261021/{REGION}/chime/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {path_and_query} answered {status} with a non-JSON body: {e}: {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        Response {
            status,
            error_type,
            body,
        }
    }

    async fn create_pool(&self, input: Value) -> Response {
        self.call("POST", POOLS, Some(input)).await
    }

    async fn get_pool(&self, identifier: &str) -> Response {
        self.call("GET", &format!("{POOLS}/{}", encode_label(identifier)), None)
            .await
    }

    async fn update_pool(&self, identifier: &str, input: Value) -> Response {
        self.call(
            "PUT",
            &format!("{POOLS}/{}", encode_label(identifier)),
            Some(input),
        )
        .await
    }

    async fn delete_pool(&self, identifier: &str) -> Response {
        self.call(
            "DELETE",
            &format!("{POOLS}/{}", encode_label(identifier)),
            None,
        )
        .await
    }

    async fn list_pools(&self, query: &str) -> Response {
        self.call("GET", &format!("{POOLS}{query}"), None).await
    }
}

/// A non-greedy `@httpLabel` as a client sends it: the characters an ARN carries that are
/// path separators or delimiters are percent-encoded.
pub(crate) fn encode_label(label: &str) -> String {
    label
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

#[derive(Debug)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Response {
    /// The wire error shape name, read from `x-amzn-errortype` (which restJson1 resolves
    /// the shape from) and falling back to the body's `__type`.
    pub(crate) fn error_code(&self) -> String {
        self.error_type
            .clone()
            .or_else(|| {
                self.body
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    pub(crate) fn str_at(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Whether `id` is a 36-character GUID, which also satisfies the model's
/// `KinesisVideoStreamPoolId` (len 1..256, pattern `^[0-9a-zA-Z._-]+$`) and the
/// `GuidString` every minted id of this service carries.
pub(crate) fn is_guid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| group.len() == *len)
        && groups
            .iter()
            .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Parse a modelled `Iso8601Timestamp` as the service renders it, returning epoch seconds.
/// Parsing is what makes the assertion sound: it fails both on the epoch NUMBER the
/// protocol default would emit and on any other text.
pub(crate) fn iso8601_seconds(case: &str, value: &str) -> f64 {
    aws_smithy_types::DateTime::from_str(value, aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {value:?} is not an ISO-8601 date-time: {e}"))
        .as_secs_f64()
}

/// The longest `PoolName` the model allows (len 1..128, pattern `^[0-9a-zA-Z._-]+$`).
fn longest_pool_name() -> String {
    "p".repeat(128)
}

/// The create inputs the lifecycle rules are asserted over. The rows differ along what the
/// rules generalise over: the NAME's form and length across the modelled pattern and both
/// of its length bounds, the stream configuration's `Region`, and whether the optional
/// `DataRetentionInHours` and `Tags` are present.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "the shortest name the model allows, retention at its minimum",
            json!({
                "PoolName": "p",
                "StreamConfiguration": { "Region": REGION, "DataRetentionInHours": 0 },
            }),
        ),
        (
            "a name carrying every class its pattern allows, with tags",
            json!({
                "PoolName": "Pool.1_test-2",
                "StreamConfiguration": { "Region": "af-south-1", "DataRetentionInHours": 2 },
                "Tags": [{ "Key": "team", "Value": "voice" }],
            }),
        ),
        (
            "the longest name the model allows, no retention at all",
            json!({
                "PoolName": longest_pool_name(),
                "StreamConfiguration": { "Region": "eu-west-2" },
            }),
        ),
        (
            "a numeric name, a month of retention",
            json!({
                "PoolName": "1234567890",
                "StreamConfiguration": { "Region": "ap-southeast-1", "DataRetentionInHours": 744 },
            }),
        ),
    ]
}

/// RULE: a create mints and persists a pool, every later read of either identifying form
/// renders from the stored record, and a delete is observable by every later read.
#[tokio::test]
async fn a_pool_is_created_read_listed_and_deleted_by_name_or_arn() {
    for (case, input) in create_rows() {
        let emulator = Emulator::new();
        let created = emulator.create_pool(input.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected the modelled 201, got {} body {:?}",
            created.status, created.body
        );

        let pool = "/KinesisVideoStreamPoolConfiguration";
        let name = created.str_at(&format!("{pool}/PoolName"));
        let id = created.str_at(&format!("{pool}/PoolId"));
        let arn = created.str_at(&format!("{pool}/PoolArn"));
        assert_eq!(
            name,
            input["PoolName"].as_str().unwrap_or_default(),
            "case {case:?}: the pool must carry the name the create asked for"
        );
        assert!(
            is_guid(&id),
            "case {case:?}: expected a minted pool id, got {id:?}"
        );
        assert_eq!(
            arn,
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline-kinesis-video-stream-pool/{id}"),
            "case {case:?}: the ARN must be derived from the SIGNED region and the minted id"
        );
        assert_eq!(
            created.str_at(&format!("{pool}/PoolStatus")),
            POOL_STATUS_CREATING,
            "case {case:?}: a freshly created pool is in the transient status: {:?}",
            created.body
        );
        assert_eq!(
            created.body.pointer(&format!("{pool}/StreamConfiguration")),
            Some(&input["StreamConfiguration"]),
            "case {case:?}: the caller's stream configuration must be reported as submitted"
        );
        let created_at = created.str_at(&format!("{pool}/CreatedTimestamp"));
        let updated_at = created.str_at(&format!("{pool}/UpdatedTimestamp"));
        assert_eq!(
            iso8601_seconds(case, &created_at),
            iso8601_seconds(case, &updated_at),
            "case {case:?}: a pool nothing has updated carries one instant on both members"
        );

        // Both identifying forms render the same stored record.
        for (form, identifier) in [("name", name.clone()), ("ARN", arn.clone())] {
            let read = emulator.get_pool(&identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} / by {form}: expected 200, got {} body {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.body.pointer(pool),
                created.body.pointer(pool),
                "case {case:?} / by {form}: a read must render the stored record"
            );
        }

        // The listing summarises exactly the three members the model gives the summary.
        let listed = emulator.list_pools("").await;
        assert_eq!(
            listed.body.pointer("/KinesisVideoStreamPools"),
            Some(&json!([{ "PoolName": name, "PoolId": id, "PoolArn": arn }])),
            "case {case:?}: expected the one pool summarised, got {:?}",
            listed.body
        );

        // A delete is observable by every later read, and is 404 the second time.
        let deleted = emulator.delete_pool(&name).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: expected the modelled 204, got {} body {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            listed_names(&emulator.list_pools("").await),
            BTreeSet::new(),
            "case {case:?}: a deleted pool must leave the listing"
        );
        for (operation, response) in [
            ("GetMediaPipelineKinesisVideoStreamPool", emulator.get_pool(&name).await),
            ("DeleteMediaPipelineKinesisVideoStreamPool", emulator.delete_pool(&arn).await),
        ] {
            assert_not_found(&format!("{case} / {operation} after the delete"), &response);
        }
    }
}

/// Every `PoolName` the listing reports.
fn listed_names(response: &Response) -> BTreeSet<String> {
    response
        .body
        .pointer("/KinesisVideoStreamPools")
        .and_then(Value::as_array)
        .map(|pools| {
            pools
                .iter()
                .map(|pool| {
                    pool["PoolName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `response` is this service's modelled 404 `NotFoundException`, in the restJson1
/// error framing: the status, the error-shape name, and the body members the shape
/// declares.
fn assert_not_found(case: &str, response: &Response) {
    assert_eq!(
        response.status, 404,
        "case {case:?}: expected the modelled 404, got {} body {:?}",
        response.status, response.body
    );
    assert_eq!(
        response.error_code(),
        NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the modelled error shape, got {:?}",
        response.body
    );
    assert_eq!(
        response.str_at("/Code"),
        ERROR_CODE_NOT_FOUND,
        "case {case:?}: expected the modelled ErrorCode, got {:?}",
        response.body
    );
    assert!(
        !response.str_at("/Message").is_empty(),
        "case {case:?}: the refusal must say what was not found: {:?}",
        response.body
    );
    assert!(
        !response.str_at("/RequestId").is_empty(),
        "case {case:?}: every error of this service carries a RequestId: {:?}",
        response.body
    );
}

/// RULE: a pool name is unique among live pools — a second create under a live name is the
/// modelled 409 `ConflictException`, and the name is free again once the pool is deleted.
#[tokio::test]
async fn a_pool_name_already_in_use_is_the_modelled_conflict() {
    for (case, input) in create_rows() {
        let emulator = Emulator::new();
        assert_eq!(emulator.create_pool(input.clone()).await.status, 201);
        let name = input["PoolName"].as_str().unwrap_or_default().to_string();

        // A second create of the same name, with a DIFFERENT stream configuration, so the
        // refusal is about the name and not about the request being a replay.
        let mut again = input.clone();
        again["StreamConfiguration"] = json!({ "Region": "us-east-1", "DataRetentionInHours": 1 });
        let refused = emulator.create_pool(again.clone()).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected the modelled 409, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            CONFLICT_EXCEPTION,
            "case {case:?}: expected the modelled error shape, got {:?}",
            refused.body
        );
        assert!(
            !refused.str_at("/Message").is_empty(),
            "case {case:?}: the refusal must say what clashed: {:?}",
            refused.body
        );

        // Nothing was stored for the refused create: the first pool is untouched.
        let held = emulator.get_pool(&name).await;
        assert_eq!(
            held.body
                .pointer("/KinesisVideoStreamPoolConfiguration/StreamConfiguration"),
            Some(&input["StreamConfiguration"]),
            "case {case:?}: a refused create must not overwrite the pool that holds the name"
        );

        // Deleting frees the name.
        assert_eq!(emulator.delete_pool(&name).await.status, 204);
        assert_eq!(
            emulator.create_pool(again).await.status,
            201,
            "case {case:?}: a name is free again once its pool is deleted"
        );
    }
}

/// How many callers race to create one pool name. More than two, so the test is about ANY
/// number of concurrent creates rather than a single pair.
const RACERS: usize = 5;

/// RULE (the uniqueness rule, reached concurrently): of N callers creating one pool name at
/// the same instant exactly ONE is served and every other is refused the modelled 409 —
/// the lookup and the insert happen under one lock — and exactly one pool exists after.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_of_several_concurrent_creates_of_one_name_is_served() {
    let emulator = Arc::new(Emulator::new());
    let name = "concurrent-pool";

    let mut racers = Vec::new();
    for racer in 0..RACERS {
        let emulator = Arc::clone(&emulator);
        racers.push(tokio::spawn(async move {
            (
                racer,
                emulator
                    .create_pool(json!({
                        "PoolName": name,
                        // A per-racer retention, so the winner's record identifies which
                        // create was served rather than being the same value either way.
                        "StreamConfiguration": {
                            "Region": REGION,
                            "DataRetentionInHours": racer as i64 + 1,
                        },
                    }))
                    .await,
            )
        }));
    }

    let mut winners = Vec::new();
    let mut conflicts = Vec::new();
    for racer in racers {
        let (index, reply) = racer.await.expect("the racing create task ran");
        match reply.status {
            201 => winners.push((index, reply)),
            409 => {
                assert_eq!(
                    reply.error_code(),
                    CONFLICT_EXCEPTION,
                    "racer {index}: a refused concurrent create is the modelled conflict, got {:?}",
                    reply.body
                );
                conflicts.push(index);
            }
            other => panic!(
                "racer {index}: expected 201 or the modelled 409, got {other} body {:?}",
                reply.body
            ),
        }
    }
    assert_eq!(
        winners.len(),
        1,
        "expected exactly 1 winner of {RACERS} concurrent creates, got {} (conflicts: {conflicts:?})",
        winners.len()
    );
    assert_eq!(
        conflicts.len(),
        RACERS - 1,
        "expected every other racer refused, got {conflicts:?}"
    );

    // One pool exists, and it is the winner's record.
    let listed = emulator.list_pools("").await;
    assert_eq!(
        listed_names(&listed),
        BTreeSet::from([name.to_string()]),
        "expected exactly one pool to exist, got {:?}",
        listed.body
    );
    let (_, winner) = &winners[0];
    assert_eq!(
        emulator
            .get_pool(name)
            .await
            .body
            .pointer("/KinesisVideoStreamPoolConfiguration"),
        winner.body.pointer("/KinesisVideoStreamPoolConfiguration"),
        "the stored pool must be the one the served create reported"
    );
}

/// The identifier forms that name no pool. They differ along what the resolution
/// generalises over: a name nothing was created under, this service's own ARN form for
/// such a name, an ARN of another resource type in the same namespace, a pool ID (which is
/// NOT one of the two identifying forms the model documents), and a value that is no ARN
/// at all.
fn unknown_identifiers(live_pool_id: &str) -> Vec<(&'static str, String)> {
    vec![
        ("a well-formed name nothing was created under", "no-such-pool".to_string()),
        (
            "this service's own ARN form for a name nothing holds",
            format!(
                "arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline-kinesis-video-stream-pool/no-such-pool"
            ),
        ),
        (
            "an ARN of another resource type in the same namespace",
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e"),
        ),
        (
            "the pool ID of a LIVE pool, which the model does not make an identifier",
            live_pool_id.to_string(),
        ),
        ("a value that is no ARN and no stored name", "nope".to_string()),
    ]
}

/// RULE: an identifier that resolves to no stored pool is the modelled 404
/// `NotFoundException`, on every operation that resolves one.
#[tokio::test]
async fn an_identifier_that_names_no_pool_is_the_modelled_not_found() {
    let emulator = Emulator::new();
    // One live pool, so the refusals are about resolution and not about an empty store.
    let created = emulator
        .create_pool(json!({
            "PoolName": "live-pool",
            "StreamConfiguration": { "Region": REGION, "DataRetentionInHours": 3 },
        }))
        .await;
    assert_eq!(created.status, 201, "the live pool: {:?}", created.body);
    let live_id = created.str_at("/KinesisVideoStreamPoolConfiguration/PoolId");

    for (case, identifier) in unknown_identifiers(&live_id) {
        for (operation, response) in [
            (
                "GetMediaPipelineKinesisVideoStreamPool",
                emulator.get_pool(&identifier).await,
            ),
            (
                "UpdateMediaPipelineKinesisVideoStreamPool",
                emulator
                    .update_pool(
                        &identifier,
                        json!({ "StreamConfiguration": { "DataRetentionInHours": 5 } }),
                    )
                    .await,
            ),
            (
                "DeleteMediaPipelineKinesisVideoStreamPool",
                emulator.delete_pool(&identifier).await,
            ),
        ] {
            assert_not_found(&format!("{operation} on {case}"), &response);
        }
    }

    // None of those refusals touched the live pool.
    assert_eq!(
        emulator.get_pool("live-pool").await.body.pointer("/KinesisVideoStreamPoolConfiguration"),
        created.body.pointer("/KinesisVideoStreamPoolConfiguration"),
        "a refused call must leave its neighbours alone"
    );
}

/// RULE: a pool the service returns in a NON-TERMINAL status carries the transition the
/// clock owes — a caller reading straight back still sees `CREATING`, and once the span has
/// elapsed every read sees `ACTIVE` — and an update applies the retention it carries and
/// puts the pool back into the transient `UPDATING` status.
#[tokio::test]
async fn the_transient_pool_statuses_move_to_active_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let created = emulator
        .create_pool(json!({
            "PoolName": "clock-pool",
            "StreamConfiguration": { "Region": REGION, "DataRetentionInHours": 1 },
        }))
        .await;
    assert_eq!(created.status, 201, "{:?}", created.body);
    let created_at = created.str_at("/KinesisVideoStreamPoolConfiguration/CreatedTimestamp");

    // Straight back: the transient status is still observable.
    assert_eq!(
        emulator
            .get_pool("clock-pool")
            .await
            .str_at("/KinesisVideoStreamPoolConfiguration/PoolStatus"),
        POOL_STATUS_CREATING,
        "a read taken straight after the create must still see the transient status"
    );

    // Past the span the transition is due in — derived from the span itself, not a magic
    // number, so the test follows the constant if it ever changes.
    tokio::time::sleep(POOL_TRANSIENT_TO_ACTIVE + Duration::from_millis(200)).await;
    let settled = emulator.get_pool("clock-pool").await;
    assert_eq!(
        settled.str_at("/KinesisVideoStreamPoolConfiguration/PoolStatus"),
        POOL_STATUS_ACTIVE,
        "the owed transition must have been materialised: {:?}",
        settled.body
    );
    let settled_at = settled.str_at("/KinesisVideoStreamPoolConfiguration/UpdatedTimestamp");
    assert!(
        iso8601_seconds("the settled pool", &settled_at)
            > iso8601_seconds("the settled pool", &created_at),
        "expected UpdatedTimestamp after CreatedTimestamp {created_at:?}, got {settled_at:?}"
    );

    // An update is applied to the stored record and reported by a later read; the pool
    // goes back into a transient status while the change is being applied.
    for (case, identifier) in [
        ("by name", "clock-pool".to_string()),
        (
            "by ARN",
            settled.str_at("/KinesisVideoStreamPoolConfiguration/PoolArn"),
        ),
    ] {
        let retention = if case == "by name" { 12 } else { 24 };
        let updated = emulator
            .update_pool(
                &identifier,
                json!({ "StreamConfiguration": { "DataRetentionInHours": retention } }),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected the modelled 200, got {} body {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.str_at("/KinesisVideoStreamPoolConfiguration/PoolStatus"),
            POOL_STATUS_UPDATING,
            "case {case:?}: an updating pool is in the transient status: {:?}",
            updated.body
        );
        let read_back = emulator.get_pool("clock-pool").await;
        assert_eq!(
            read_back
                .body
                .pointer("/KinesisVideoStreamPoolConfiguration/StreamConfiguration/DataRetentionInHours"),
            Some(&json!(retention)),
            "case {case:?}: the update must be observable by a later read: {:?}",
            read_back.body
        );
        assert_eq!(
            read_back.body.pointer("/KinesisVideoStreamPoolConfiguration/StreamConfiguration/Region"),
            Some(&json!(REGION)),
            "case {case:?}: an update that names only the retention leaves the rest alone"
        );
    }
}

/// RULE: a repeated `ClientRequestToken` is a replay of the create it first served, so it
/// answers with that pool rather than minting a second one or refusing the name; a
/// different token under the same name is a new create of a name in use.
#[tokio::test]
async fn a_repeated_client_request_token_answers_with_the_pool_the_first_create_minted() {
    for (case, token) in [
        ("the shortest token the model allows", "ab".to_string()),
        ("the longest token the model allows", "t".repeat(64)),
    ] {
        let emulator = Emulator::new();
        let input = json!({
            "PoolName": "idempotent-pool",
            "StreamConfiguration": { "Region": REGION, "DataRetentionInHours": 6 },
            "ClientRequestToken": token,
        });
        let first = emulator.create_pool(input.clone()).await;
        assert_eq!(first.status, 201, "case {case:?}: {:?}", first.body);
        let replay = emulator.create_pool(input).await;
        assert_eq!(
            replay.status, 201,
            "case {case:?}: a replay is served, not refused: {:?}",
            replay.body
        );
        assert_eq!(
            replay.body.pointer("/KinesisVideoStreamPoolConfiguration"),
            first.body.pointer("/KinesisVideoStreamPoolConfiguration"),
            "case {case:?}: a replay must answer with the pool the first create minted"
        );
        assert_eq!(
            listed_names(&emulator.list_pools("").await).len(),
            1,
            "case {case:?}: a replay must not mint a second pool"
        );

        let refused = emulator
            .create_pool(json!({
                "PoolName": "idempotent-pool",
                "StreamConfiguration": { "Region": REGION },
                // A DIFFERENT token of the same (modelled) length, so the refusal is
                // about the name in use and not about the token's length.
                "ClientRequestToken": format!("x{}", &token[1..]),
            }))
            .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: a different token under a live name is a conflict: {:?}",
            refused.body
        );
    }
}

/// RULE: the listing reports the pools in creation order, one page at a time, with a
/// `NextToken` present only while records remain — and a token it did not issue is the
/// modelled 400 `BadRequestException`.
#[tokio::test]
async fn the_listing_pages_in_creation_order_and_refuses_a_token_it_did_not_issue() {
    let emulator = Emulator::new();
    let names: Vec<String> = (0..5).map(|index| format!("page-pool-{index}")).collect();
    for name in &names {
        let created = emulator
            .create_pool(json!({
                "PoolName": name,
                "StreamConfiguration": { "Region": REGION },
            }))
            .await;
        assert_eq!(created.status, 201, "{name}: {:?}", created.body);
    }

    // Each page size the rows span, including one that divides the count and one that
    // does not, and one past the count.
    for page_size in [1, 2, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?max-results={page_size}");
        loop {
            let page = emulator.list_pools(&query).await;
            assert_eq!(
                page.status, 200,
                "page size {page_size}: expected 200, got {} body {:?}",
                page.status, page.body
            );
            let items = page
                .body
                .pointer("/KinesisVideoStreamPools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                items.len() <= page_size,
                "page size {page_size}: a page must not exceed the asked-for size, got {}",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|pool| pool["PoolName"].as_str().unwrap_or_default().to_string()),
            );
            match page.body.get("NextToken").and_then(Value::as_str) {
                Some(token) => query = format!("?max-results={page_size}&next-token={token}"),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "page size {page_size}: paging must report every pool once, in creation order"
        );
    }

    let refused = emulator.list_pools("?next-token=not-a-token-this-service-issued").await;
    assert_eq!(
        refused.status, 400,
        "expected the modelled 400 for a token this service did not issue, got {} body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        BAD_REQUEST_EXCEPTION,
        "expected the modelled error shape, got {:?}",
        refused.body
    );
}
