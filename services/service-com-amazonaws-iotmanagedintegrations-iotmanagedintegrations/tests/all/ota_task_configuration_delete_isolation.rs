// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteOtaTaskConfiguration` touches, and what it must leave alone, end to end
//! over the wire.
//!
//! RULE under test: a delete of an OTA task configuration removes EXACTLY the one record
//! the request's `Identifier` names in the region the request is signed for — it answers
//! the modelled 204 with no payload, every later read of that identifier is the modelled
//! `ResourceNotFoundException`, and every OTHER stored configuration, in that region and
//! in any other, still projects the record its create stored, member for member. A delete
//! aimed at a region whose endpoint holds no record under that identifier removes nothing
//! at all.
//!
//! The recorded unit this was derived from deletes the single configuration a store holds;
//! that is ONE row here. The rows differ along what the rule generalises over: how many
//! records the store holds when the delete runs, which members each one carried, the
//! region each belongs to, and the order the records are removed in.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses and bodies asserted here are the ones a client sees. The suite
//! carries its own request harness, as each suite in this crate does, so a case reads as
//! one HTTP call.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The collection route the OTA-task-configuration operations are bound to
/// (`smithy_query(query="operation",operation="CreateOtaTaskConfiguration,DeleteOtaTaskConfiguration")`
/// -> POST `/ota-task-configurations`, DELETE `/ota-task-configurations/{Identifier}`).
const COLLECTION: &str = "/ota-task-configurations";

/// The modelled success status of `DeleteOtaTaskConfiguration` (`@http` code 204), and the
/// modelled error status and code of an identifier no record is held under.
const DELETE_STATUS: u16 = 204;
const NOT_FOUND_STATUS: u16 = 404;
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The regions the table spreads its records over: each is a separate endpoint holding its
/// own records, since an OTA task configuration is a regional resource.
const REGIONS: &[&str] = &["us-east-1", "us-west-2", "eu-central-1"];

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    /// The answer's payload length in bytes, so a modelled empty body can be asserted as
    /// empty rather than as "parsed to null".
    body_len: usize,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }
}

/// A router holding one `iotmanagedintegrations` service over `engine`: the same assembly
/// `core/router` performs, with one service mounted. The caller keeps the engine handle, so
/// a case can check the store itself and not only what the wire reports.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names the region its
/// records belong to.
async fn call(
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
    let body_len = bytes.len();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body_len,
        body,
    }
}

/// `CreateOtaTaskConfiguration` in `region`, asserting the modelled 201 and answering the
/// minted `OtaTaskConfigurationId`.
async fn create(router: &MultiServiceRouter, case: &str, region: &str, body: Value) -> String {
    let reply = call(router, region, "POST", COLLECTION, Some(body)).await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201 from CreateOtaTaskConfiguration, got {} with body {:?}",
        reply.status, reply.body
    );
    reply
        .string("TaskConfigurationId")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a TaskConfigurationId, got body {:?}",
                reply.body
            )
        })
        .to_string()
}

/// `GetOtaTaskConfiguration` for `identifier` through `region`'s endpoint.
async fn read(router: &MultiServiceRouter, region: &str, identifier: &str) -> Reply {
    call(
        router,
        region,
        "GET",
        &format!("{COLLECTION}/{identifier}"),
        None,
    )
    .await
}

/// `DeleteOtaTaskConfiguration` for `identifier` through `region`'s endpoint.
async fn delete(router: &MultiServiceRouter, region: &str, identifier: &str) -> Reply {
    call(
        router,
        region,
        "DELETE",
        &format!("{COLLECTION}/{identifier}"),
        None,
    )
    .await
}

/// One seeded configuration: the region it belongs to, the id it is held under, and the
/// whole body a read of it reported while it was stored — the projection every later
/// assertion compares against.
struct Seeded {
    case: String,
    region: String,
    id: String,
    projection: Value,
}

/// The table of configurations each case seeds: the rows differ in which members the create
/// carried and how deeply `PushConfig` nests, and are spread over [`REGIONS`] so the same
/// store holds records belonging to several endpoints. The first row is the recorded shape
/// of request this issue was filed about.
fn seed_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "name, description and a push config",
            json!({
                "Name": "fleet lamps config",
                "Description": "rollout for the lamp fleet",
                "PushConfig": {"TimeoutConfig": {"InProgressTimeoutInMinutes": 60}},
            }),
        ),
        ("every member absent", json!({})),
        ("name only", json!({"Name": "ota config one"})),
        (
            "description only",
            json!({"Description": "fleet rollout stage one"}),
        ),
        (
            "an empty push config structure",
            json!({"Name": "empty push config", "PushConfig": {}}),
        ),
        (
            "a client token, so the delete must forget it too",
            json!({"Name": "tokened config", "ClientToken": "deleteisolation1"}),
        ),
        (
            "every branch of the push config at once",
            json!({
                "Name": "whole push config",
                "Description": "abort rollout and timeout together",
                "PushConfig": {
                    "AbortConfig": {
                        "AbortConfigCriteriaList": [{
                            "Action": "CANCEL",
                            "FailureType": "FAILED",
                            "MinNumberOfExecutedThings": 2,
                            "ThresholdPercentage": 75.5,
                        }],
                    },
                    "RolloutConfig": {
                        "MaximumPerMinute": 25,
                        "ExponentialRolloutRate": {
                            "BaseRatePerMinute": 10,
                            "IncrementFactor": 1.5,
                            "RateIncreaseCriteria": {
                                "numberOfNotifiedThings": 20,
                                "numberOfSucceededThings": 10,
                            },
                        },
                    },
                    "TimeoutConfig": {"InProgressTimeoutInMinutes": 10_080},
                },
            }),
        ),
    ]
}

/// Seed the whole table, spread over [`REGIONS`], and record what a read of each one
/// reports while it is stored.
async fn seed(router: &MultiServiceRouter) -> Vec<Seeded> {
    let mut seeded = Vec::new();
    for (row, (case, request)) in seed_requests().into_iter().enumerate() {
        let region = REGIONS[row % REGIONS.len()];
        let case = format!("{region}/{case}");
        let id = create(router, &case, region, request).await;
        let stored = read(router, region, &id).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: expected HTTP 200 from GetOtaTaskConfiguration while the record is \
             stored, got {} with body {:?}",
            stored.status, stored.body
        );
        seeded.push(Seeded {
            case,
            region: region.to_string(),
            id,
            projection: stored.body,
        });
    }
    seeded
}

/// Assert `reply` is the modelled refusal of an identifier this endpoint holds no record
/// under.
fn assert_absent(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, NOT_FOUND_STATUS,
        "case {case:?}: expected HTTP {NOT_FOUND_STATUS} for an identifier this endpoint holds \
         no record under, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(NOT_FOUND),
        "case {case:?}: expected the modelled {NOT_FOUND} code, got {:?}",
        reply.error_type
    );
}

/// Assert every record in `survivors` still projects exactly what it projected when it was
/// seeded.
async fn assert_survivors_untouched(router: &MultiServiceRouter, case: &str, survivors: &[&Seeded]) {
    for survivor in survivors {
        let still_there = read(router, &survivor.region, &survivor.id).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: the untouched record {:?} must still read back, got {} with body {:?}",
            survivor.case, still_there.status, still_there.body
        );
        assert_eq!(
            still_there.body, survivor.projection,
            "case {case:?}: the untouched record {:?} must project exactly what it stored \
             (expected {:?}), got {:?}",
            survivor.case, survivor.projection, still_there.body
        );
    }
}

/// RULE (see the module docs): a delete removes exactly the record its `Identifier` names in
/// the request's region, answers the modelled 204 with no payload, and leaves every other
/// stored configuration — in that region and in any other — projecting what it stored.
#[tokio::test]
async fn a_delete_removes_only_the_configuration_it_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let seeded = seed(&router).await;
    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .ota_task_configurations
            .len(),
        seeded.len(),
        "every create must have persisted exactly one record"
    );

    // The records are removed one at a time, so every row runs against a store holding a
    // different number of records: the first delete sees the whole table, the last sees one.
    for (position, target) in seeded.iter().enumerate() {
        let case = format!("{} (with {} stored)", target.case, seeded.len() - position);

        let removed = delete(&router, &target.region, &target.id).await;
        assert_eq!(
            removed.status, DELETE_STATUS,
            "case {case:?}: expected HTTP {DELETE_STATUS} from DeleteOtaTaskConfiguration, got {} \
             with body {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.body_len, 0,
            "case {case:?}: the modelled success answers no payload, got {} bytes: {:?}",
            removed.body_len, removed.body
        );

        // The named record is gone, from the store and from every endpoint.
        assert!(
            !engine
                .state::<ServiceState>()
                .lock()
                .ota_task_configurations
                .contains_key(&target.id),
            "case {case:?}: the delete must have removed the record under {:?}",
            target.id
        );
        for region in REGIONS {
            assert_absent(
                &format!("{case}/read through {region}"),
                &read(&router, region, &target.id).await,
            );
        }

        // Nothing else moved: every record not yet deleted still projects what it stored.
        let survivors: Vec<&Seeded> = seeded.iter().skip(position + 1).collect();
        assert_survivors_untouched(&router, &case, &survivors).await;
        assert_eq!(
            engine
                .state::<ServiceState>()
                .lock()
                .ota_task_configurations
                .len(),
            survivors.len(),
            "case {case:?}: the delete must have removed exactly one record"
        );
    }
}

/// RULE, the regional half: a delete reaching an endpoint that holds no record under the
/// identifier removes NOTHING — neither the record another region holds under it nor any
/// other — and is the modelled 404.
#[tokio::test]
async fn a_delete_through_another_regions_endpoint_removes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let seeded = seed(&router).await;

    for target in &seeded {
        // Every endpoint except the one that created the record, plus a region the test
        // never used at all.
        let elsewhere: Vec<&str> = REGIONS
            .iter()
            .copied()
            .chain(std::iter::once("ap-southeast-2"))
            .filter(|region| *region != target.region)
            .collect();
        for region in elsewhere {
            let case = format!("{} deleted through {region}", target.case);
            assert_absent(&case, &delete(&router, region, &target.id).await);
            // The refused delete removed nothing at all: every seeded record, the target
            // included, still projects what it stored.
            let survivors: Vec<&Seeded> = seeded.iter().collect();
            assert_survivors_untouched(&router, &case, &survivors).await;
        }
    }

    assert_eq!(
        engine
            .state::<ServiceState>()
            .lock()
            .ota_task_configurations
            .len(),
        seeded.len(),
        "no out-of-region delete may remove a record"
    );
}
