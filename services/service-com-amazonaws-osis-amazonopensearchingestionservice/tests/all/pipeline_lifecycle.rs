// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the OpenSearch Ingestion pipeline lifecycle.
//!
//! Every case drives the service the way a client does — one HTTP request per call,
//! through the same `MultiServiceRouter` the emulator boots — and asserts only what the
//! stored state can justify: a pipeline read back through `GetPipeline` exactly as it was
//! created, a page that resumes where the previous one ended, a status a mutation moved
//! and a read reports, and a 404 for a name nothing was ever created under.
//!
//! The rows of each table differ along what the rule generalises over (name length and
//! shape, unit counts, configuration-body size, which optional members are present, page
//! size, status), so no assertion is pinned to one recorded request.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is NOT the emulator default, so an ARN built from the request's own
/// SigV4 credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The transition span the service itself uses, so the transient-status assertions below
/// are tied to the implementation's own constant rather than to a copy of it.
const TRANSITION_SPAN: Duration =
    service_com_amazonaws_osis_amazonopensearchingestionservice::state::TRANSITION_SPAN;

/// Comfortably longer than [`TRANSITION_SPAN`], so a transition the clock owes has come
/// due by the time the next read runs.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// What one call answered: the status, the modelled error code (empty on success) and the
/// parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The `Pipeline` member of the response body (`Null` when the call failed).
    fn pipeline(&self) -> &Value {
        &self.body["Pipeline"]
    }

    /// One member of the returned `Pipeline`.
    fn member(&self, name: &str) -> &Value {
        &self.body["Pipeline"][name]
    }

    fn text(&self, name: &str) -> String {
        self.member(name)
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `{name}` in {:?}", self.body))
            .to_string()
    }

    fn status_value(&self) -> String {
        self.text("Status")
    }
}

fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://osis.{REGION}.amazonaws.com{path}"))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/osis/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

async fn create_pipeline(router: &MultiServiceRouter, request: Value) -> Answer {
    call(
        router,
        "POST",
        "/2022-01-01/osis/createPipeline",
        Some(request),
    )
    .await
}

async fn get_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/2022-01-01/osis/getPipeline/{name}"),
        None,
    )
    .await
}

async fn stop_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/stopPipeline/{name}"),
        None,
    )
    .await
}

async fn start_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "PUT",
        &format!("/2022-01-01/osis/startPipeline/{name}"),
        None,
    )
    .await
}

async fn delete_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/2022-01-01/osis/deletePipeline/{name}"),
        None,
    )
    .await
}

async fn list_pipelines(router: &MultiServiceRouter, query: &str) -> Answer {
    let path = if query.is_empty() {
        "/2022-01-01/osis/listPipelines".to_string()
    } else {
        format!("/2022-01-01/osis/listPipelines?{query}")
    };
    call(router, "GET", &path, None).await
}

/// Create a pipeline and wait out the `CREATING` -> `ACTIVE` transition the clock owes,
/// so the caller starts from a running pipeline.
async fn active_pipeline(router: &MultiServiceRouter, name: &str) -> Answer {
    let created = create_pipeline(
        router,
        json!({
            "PipelineName": name,
            "MinUnits": 1,
            "MaxUnits": 2,
            "PipelineConfigurationBody": "version: \"2\"\nlog-pipeline:\n  source:\n    http:\n",
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: expected 200 from CreatePipeline, got {} / {:?}",
        created.status, created.body
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    let read = get_pipeline(router, name).await;
    assert_eq!(
        read.status_value(),
        "ACTIVE",
        "case {name:?}: the pipeline must settle to ACTIVE, got {:?}",
        read.pipeline()
    );
    read
}

/// A pipeline read within `elapsed` of the call that started a transition must still
/// report the transient status; only a read that took longer than the span may already
/// see it settled.
fn assert_transient_status(case: &str, read: &Answer, expected: &str, elapsed: Duration) {
    let status = read.status_value();
    if status != expected {
        assert!(
            elapsed >= TRANSITION_SPAN,
            "case {case:?}: expected the transient status {expected:?} within the \
             {TRANSITION_SPAN:?} span, got {status:?} after {elapsed:?}"
        );
    }
}

/// Every member a create was given is persisted and read back — across inputs that differ
/// in name length and shape, in unit counts, in configuration-body size and in which
/// optional members are present.
#[tokio::test]
async fn created_pipelines_read_back_exactly_as_created() {
    // `PipelineName` is `@length(3, 28)` / `^[a-z][a-z0-9\-]+$` and `MinUnits`/`MaxUnits`
    // are `@range(1,)` (smithy_query: operation CreatePipeline), so the rows below sit on
    // both published name-length bounds and on the lowest legal unit count.
    let cases = vec![
        (
            "shortest-name-smallest-config",
            json!({
                "PipelineName": "abc",
                "MinUnits": 1,
                "MaxUnits": 1,
                "PipelineConfigurationBody": "x",
            }),
        ),
        (
            "longest-name-every-option",
            json!({
                "PipelineName": "a-pipeline-with-28-chars-abc",
                "MinUnits": 2,
                "MaxUnits": 24,
                "PipelineConfigurationBody": "version: \"2\"\nlog-pipeline:\n  source:\n    http:\n  sink:\n    - opensearch:\n",
                "PipelineRoleArn": "arn:aws:iam::000000000000:role/osis-pipeline",
                "LogPublishingOptions": {
                    "IsLoggingEnabled": true,
                    "CloudWatchLogDestination": { "LogGroup": "/aws/vendedlogs/OpenSearchIngestion/demo" },
                },
                "BufferOptions": { "PersistentBufferEnabled": true },
                "EncryptionAtRestOptions": { "KmsKeyArn": "arn:aws:kms:eu-west-2:000000000000:key/abcd" },
                "Tags": [
                    { "Key": "team", "Value": "search" },
                    { "Key": "cost-center", "Value": "" },
                ],
            }),
        ),
        (
            "vpc-attached",
            json!({
                "PipelineName": "vpc-attached-pipeline",
                "MinUnits": 4,
                "MaxUnits": 4,
                "PipelineConfigurationBody": "version: \"2\"\nentry-pipeline:\n  source:\n    otel_trace_source:\n",
                "VpcOptions": {
                    "SubnetIds": ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"],
                    "SecurityGroupIds": ["sg-0123456789abcdef0"],
                },
                "Tags": [{ "Key": "only", "Value": "one" }],
            }),
        ),
    ];

    let router = new_router();
    for (case, request) in cases {
        let name = request["PipelineName"].as_str().unwrap().to_string();
        let started = std::time::Instant::now();
        let created = create_pipeline(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreatePipeline, got {} / {:?}",
            created.status, created.body
        );

        let arn = format!("arn:aws:osis:{REGION}:{ACCOUNT}:pipeline/{name}");
        assert_eq!(
            created.text("PipelineArn"),
            arn,
            "case {case:?}: PipelineArn must name the pipeline in the signed region, got {:?}",
            created.member("PipelineArn")
        );
        // `PipelineArn` is `@length(46, 76)` (smithy_query: shape PipelineSummary).
        assert!(
            (46..=76).contains(&arn.len()),
            "case {case:?}: PipelineArn must satisfy @length(46, 76), got {} chars in {arn:?}",
            arn.len()
        );
        // A newly created pipeline is not running yet.
        assert_transient_status(case, &created, "CREATING", started.elapsed());

        // One ingestion endpoint per pipeline, derived from the pipeline's own name and
        // the region it lives in.
        let urls = created
            .member("IngestEndpointUrls")
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no IngestEndpointUrls in {:?}", created.body));
        assert_eq!(
            urls.len(),
            1,
            "case {case:?}: expected exactly one ingestion endpoint, got {urls:?}"
        );
        let url = urls[0].as_str().unwrap_or_default().to_string();
        assert!(
            url.starts_with(&format!("{name}-")) && url.ends_with(&format!(".{REGION}.osis.amazonaws.com")),
            "case {case:?}: the ingestion endpoint must be derived from the pipeline name \
             and region, got {url:?}"
        );

        // A VPC interface endpoint exists exactly when the create asked for VPC access.
        let vpc_endpoints = created
            .member("VpcEndpoints")
            .as_array()
            .cloned()
            .unwrap_or_default();
        match request.get("VpcOptions") {
            Some(options) => {
                assert_eq!(
                    vpc_endpoints.len(),
                    1,
                    "case {case:?}: a VPC-attached pipeline needs one VpcEndpoint, got {vpc_endpoints:?}"
                );
                assert_eq!(
                    &vpc_endpoints[0]["VpcOptions"], options,
                    "case {case:?}: the stored VpcOptions did not round-trip, got {:?}",
                    vpc_endpoints[0]
                );
                let endpoint_id = vpc_endpoints[0]["VpcEndpointId"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    endpoint_id.starts_with("vpce-") && endpoint_id.len() == "vpce-".len() + 17,
                    "case {case:?}: expected a vpce- interface endpoint id, got {endpoint_id:?}"
                );
            }
            None => assert!(
                vpc_endpoints.is_empty(),
                "case {case:?}: a public pipeline has no VPC endpoints, got {vpc_endpoints:?}"
            ),
        }

        // Read it back: GetPipeline answers from the stored record, so every member the
        // create was given, and every one the service minted, must match.
        tokio::time::sleep(PAST_TRANSITION).await;
        let read = get_pipeline(&router, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetPipeline, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.status_value(),
            "ACTIVE",
            "case {case:?}: the transition the clock owed was never materialised, got {:?}",
            read.member("Status")
        );
        for member in [
            "PipelineName",
            "MinUnits",
            "MaxUnits",
            "PipelineConfigurationBody",
            "PipelineRoleArn",
            "LogPublishingOptions",
            "BufferOptions",
            "EncryptionAtRestOptions",
        ] {
            assert_eq!(
                read.member(member),
                request.get(member).unwrap_or(&Value::Null),
                "case {case:?}: {member} did not round-trip, got {:?}",
                read.member(member)
            );
        }
        assert_eq!(
            read.member("Tags"),
            request.get("Tags").unwrap_or(&json!([])),
            "case {case:?}: Tags did not round-trip, got {:?}",
            read.member("Tags")
        );
        for member in ["PipelineArn", "IngestEndpointUrls", "VpcEndpoints", "CreatedAt"] {
            assert_eq!(
                read.member(member),
                created.member(member),
                "case {case:?}: {member} changed between create and read, got {:?}",
                read.member(member)
            );
        }
        assert!(
            read.member("CreatedAt").as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: CreatedAt must be a populated timestamp, got {:?}",
            read.member("CreatedAt")
        );
        assert!(
            read.member("LastUpdatedAt").as_f64() >= read.member("CreatedAt").as_f64(),
            "case {case:?}: LastUpdatedAt must not precede CreatedAt, got {:?} / {:?}",
            read.member("LastUpdatedAt"),
            read.member("CreatedAt")
        );
    }
}

/// A name already taken is a conflict, and a request the model cannot refuse on its own —
/// `MaxUnits` below `MinUnits` — is a validation failure, never a second pipeline.
#[tokio::test]
async fn a_create_is_refused_rather_than_duplicating_or_mis_sizing_a_pipeline() {
    let router = new_router();

    // Names differing in length and in hyphenation; each is created once, then again.
    for name in ["dup", "duplicate-pipeline", "d-1-2-3"] {
        let body = json!({
            "PipelineName": name,
            "MinUnits": 1,
            "MaxUnits": 1,
            "PipelineConfigurationBody": "version: \"2\"\n",
        });
        let first = create_pipeline(&router, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: expected 200 from the first CreatePipeline, got {} / {:?}",
            first.status, first.body
        );
        let second = create_pipeline(&router, body).await;
        assert_eq!(
            second.status, 409,
            "case {name:?}: a second create of the same name must be refused, got {} / {:?}",
            second.status, second.body
        );
        assert_eq!(
            second.error_type, "ResourceAlreadyExistsException",
            "case {name:?}: expected ResourceAlreadyExistsException, got {:?}",
            second.error_type
        );
    }

    // MaxUnits below MinUnits, at the bottom of the range and far above it.
    for (min_units, max_units) in [(2, 1), (10, 9), (100, 1)] {
        let case = format!("min={min_units},max={max_units}");
        let refused = create_pipeline(
            &router,
            json!({
                "PipelineName": format!("sized-{min_units}-{max_units}"),
                "MinUnits": min_units,
                "MaxUnits": max_units,
                "PipelineConfigurationBody": "version: \"2\"\n",
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: MaxUnits below MinUnits must be refused, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ValidationException",
            "case {case:?}: expected a ValidationException, got {:?}",
            refused.error_type
        );
    }

    // Names outside `@length(3, 28)` / `^[a-z][a-z0-9\-]+$`: too short, one past the
    // length bound, an upper-case letter, and a leading digit.
    for name in ["ab", &"a".repeat(29), "Bad-Name", "1pipeline"] {
        let refused = create_pipeline(
            &router,
            json!({
                "PipelineName": name,
                "MinUnits": 1,
                "MaxUnits": 1,
                "PipelineConfigurationBody": "version: \"2\"\n",
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: a name outside the modelled constraint must be refused, got {} / {:?}",
            refused.status, refused.body
        );
    }
}

/// `StopPipeline` and `StartPipeline` move a status that later reads report, and refuse a
/// pipeline that is not in a stoppable / startable state.
#[tokio::test]
async fn stop_and_start_move_the_status_every_later_read_reports() {
    let router = new_router();
    let name = "lifecycle-pipeline";
    active_pipeline(&router, name).await;

    let started = std::time::Instant::now();
    let stopped = stop_pipeline(&router, name).await;
    assert_eq!(
        stopped.status, 200,
        "case \"stop an active pipeline\": expected 200, got {} / {:?}",
        stopped.status, stopped.body
    );
    assert_transient_status("stop an active pipeline", &stopped, "STOPPING", started.elapsed());

    // A second stop while the first is still in flight is a conflict, not a no-op.
    let conflict = stop_pipeline(&router, name).await;
    assert_eq!(
        conflict.status, 409,
        "case \"stop a stopping pipeline\": expected 409, got {} / {:?}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error_type, "ConflictException",
        "case \"stop a stopping pipeline\": expected ConflictException, got {:?}",
        conflict.error_type
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    for (case, answer) in [
        ("GetPipeline", get_pipeline(&router, name).await),
        ("StopPipeline", stop_pipeline(&router, name).await),
    ] {
        match case {
            "GetPipeline" => assert_eq!(
                answer.status_value(),
                "STOPPED",
                "case {case:?}: the stop must be visible to a later read, got {:?}",
                answer.pipeline()
            ),
            // Already stopped: not a stoppable state.
            _ => assert_eq!(
                answer.status, 409,
                "case {case:?}: stopping a stopped pipeline must be refused, got {} / {:?}",
                answer.status, answer.body
            ),
        }
    }
    let listed = list_pipelines(&router, "").await;
    assert_eq!(
        listed.body["Pipelines"][0]["Status"],
        json!("STOPPED"),
        "case \"ListPipelines after stop\": the summary must carry the stored status, got {:?}",
        listed.body["Pipelines"]
    );

    let started = std::time::Instant::now();
    let restarted = start_pipeline(&router, name).await;
    assert_eq!(
        restarted.status, 200,
        "case \"start a stopped pipeline\": expected 200, got {} / {:?}",
        restarted.status, restarted.body
    );
    assert_transient_status(
        "start a stopped pipeline",
        &restarted,
        "STARTING",
        started.elapsed(),
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let read = get_pipeline(&router, name).await;
    assert_eq!(
        read.status_value(),
        "ACTIVE",
        "case \"start then read\": the start must be visible to a later read, got {:?}",
        read.pipeline()
    );
    let conflict = start_pipeline(&router, name).await;
    assert_eq!(
        conflict.status, 409,
        "case \"start an active pipeline\": expected 409, got {} / {:?}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error_type, "ConflictException",
        "case \"start an active pipeline\": expected ConflictException, got {:?}",
        conflict.error_type
    );

    // Every name-addressed operation answers 404 for a name nothing was created under.
    for (case, answer) in [
        ("GetPipeline", get_pipeline(&router, "no-such-pipeline").await),
        ("StopPipeline", stop_pipeline(&router, "no-such-pipeline").await),
        ("StartPipeline", start_pipeline(&router, "no-such-pipeline").await),
        ("DeletePipeline", delete_pipeline(&router, "no-such-pipeline").await),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unknown pipeline name must answer 404, got {} / {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_type
        );
    }
}

/// A delete removes the record: the pipeline disappears from every read, and a second
/// delete of the same name is a 404.
#[tokio::test]
async fn delete_removes_the_pipeline_from_every_read() {
    let router = new_router();
    // Three pipelines, so the delete is observable as a removal from a listing that still
    // has other members.
    for ordinal in 0..3 {
        active_pipeline(&router, &format!("deletable-{ordinal}")).await;
    }

    // A pipeline still settling into a status cannot be deleted.
    let created = create_pipeline(
        &router,
        json!({
            "PipelineName": "mid-create-pipeline",
            "MinUnits": 1,
            "MaxUnits": 1,
            "PipelineConfigurationBody": "version: \"2\"\n",
        }),
    )
    .await;
    let elapsed = std::time::Instant::now();
    let refused = delete_pipeline(&router, "mid-create-pipeline").await;
    if elapsed.elapsed() < TRANSITION_SPAN {
        assert_eq!(
            refused.status, 409,
            "case \"delete a CREATING pipeline\": expected 409, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ConflictException",
            "case \"delete a CREATING pipeline\": expected ConflictException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            created.status_value(),
            "CREATING",
            "case \"delete a CREATING pipeline\": the create must report CREATING, got {:?}",
            created.pipeline()
        );
    }

    for ordinal in 0..3 {
        let name = format!("deletable-{ordinal}");
        let deleted = delete_pipeline(&router, &name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: expected 200 from DeletePipeline, got {} / {:?}",
            deleted.status, deleted.body
        );

        let read = get_pipeline(&router, &name).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: a deleted pipeline must be gone from GetPipeline, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type, "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?}",
            read.error_type
        );

        let again = delete_pipeline(&router, &name).await;
        assert_eq!(
            again.status, 404,
            "case {name:?}: a second delete must answer 404, got {} / {:?}",
            again.status, again.body
        );

        let listed = list_pipelines(&router, "").await;
        let names: Vec<String> = listed.body["Pipelines"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["PipelineName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !names.contains(&name),
            "case {name:?}: a deleted pipeline must be gone from ListPipelines, got {names:?}"
        );
    }
}

/// `ListPipelines` honours the `@paginated` contract: whatever the page size, the pages
/// cover every stored pipeline exactly once, in name order.
#[tokio::test]
async fn list_pipelines_pages_cover_every_pipeline_once() {
    let router = new_router();
    // Names created out of order, so a listing that is not sorted by name fails.
    let mut expected = Vec::new();
    for name in ["mango", "apple", "cherry", "banana", "date"] {
        let created = create_pipeline(
            &router,
            json!({
                "PipelineName": name,
                "MinUnits": 1,
                "MaxUnits": 3,
                "PipelineConfigurationBody": "version: \"2\"\n",
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected 200 from CreatePipeline, got {} / {:?}",
            created.status, created.body
        );
        expected.push(name.to_string());
    }
    expected.sort();

    // Page sizes spanning the `@range(1, 100)` on `MaxResults`: both published bounds, a
    // size that divides the set, one that does not, and none at all.
    for case in [None, Some(1), Some(2), Some(5), Some(100)] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = Vec::new();
            if let Some(max_results) = case {
                query.push(format!("maxResults={max_results}"));
            }
            if let Some(token) = &next_token {
                query.push(format!("nextToken={token}"));
            }
            let page = list_pipelines(&router, &query.join("&")).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200 from ListPipelines, got {} / {:?}",
                page.status, page.body
            );
            let items = page.body["Pipelines"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no Pipelines array in {:?}", page.body));
            if let Some(limit) = case {
                assert!(
                    items.len() as i32 <= limit,
                    "case {case:?}: a page exceeded MaxResults, got {} items",
                    items.len()
                );
            }
            for item in items {
                assert!(
                    item["Status"].is_string()
                        && item["CreatedAt"].as_f64().is_some()
                        && item["PipelineArn"].is_string()
                        && item["MinUnits"].as_i64() == Some(1)
                        && item["MaxUnits"].as_i64() == Some(3),
                    "case {case:?}: a summary is missing its stored members, got {item:?}"
                );
                seen.push(item["PipelineName"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            next_token = page.body["NextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len() + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages must cover every pipeline once, in name order"
        );
    }
}

/// A `NextToken` no listing of this service minted is refused, never served as a silent
/// first page; a `MaxResults` outside the modelled range is refused too.
#[tokio::test]
async fn a_pagination_token_this_service_never_minted_is_refused() {
    let router = new_router();
    let created = create_pipeline(
        &router,
        json!({
            "PipelineName": "token-pipeline",
            "MinUnits": 1,
            "MaxUnits": 1,
            "PipelineConfigurationBody": "version: \"2\"\n",
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreatePipeline, got {} / {:?}",
        created.status, created.body
    );

    // Each row is a token that cannot have come from this service: not hex at all, hex of
    // text that is not a cursor, and hex of a cursor-looking string with the wrong prefix.
    for token in [
        "not-a-cursor",
        "6e6f742d612d637572736f72",
        "6f7468657273657276696365",
    ] {
        let refused = list_pipelines(&router, &format!("nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {token:?}: expected 400, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "InvalidPaginationTokenException",
            "case {token:?}: expected InvalidPaginationTokenException, got {:?}",
            refused.error_type
        );
    }

    // One past each published `@range(1, 100)` bound on `MaxResults`.
    for max_results in [0, 101] {
        let refused = list_pipelines(&router, &format!("maxResults={max_results}")).await;
        assert_eq!(
            refused.status, 400,
            "case maxResults={max_results}: a value outside @range(1, 100) must be refused, \
             got {} / {:?}",
            refused.status, refused.body
        );
    }
}
