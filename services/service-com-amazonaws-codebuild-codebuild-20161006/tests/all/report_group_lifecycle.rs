// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CodeBuild's report-group slice, driven over the wire the router
//! serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! Every assertion here is about state: what `CreateReportGroup` persisted is what
//! `BatchGetReportGroups` reports, and nothing is recomputed per call.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::ReportGroup;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState, StoredReportGroup,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so an ARN assertion proves the region came from the request.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDTEST";

/// One router carrying only this service, over one shared engine — the same assembly
/// `core/router` builds, minus the other services.
fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// As [`emulator`], over an engine the caller already holds — so a test can seed or
/// inspect the service's state through the very store the handlers write.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_as(router, ACCESS_KEY_ID, operation, body).await
}

/// As [`call`], signed with `access_key_id` — so the audit test can pick its own records
/// out of a log its siblings also write to.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// A `CreateReportGroup` request body: every member the operation models as required,
/// plus `tags` when the case supplies them.
fn create_report_group_body(
    name: &str,
    report_type: &str,
    export_config: Value,
    tags: Option<Value>,
) -> Value {
    let mut body = json!({
        "name": name,
        "type": report_type,
        "exportConfig": export_config,
    });
    if let (Value::Object(target), Some(tags)) = (&mut body, tags) {
        target.insert("tags".to_string(), tags);
    }
    body
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

fn expected_arn(resource: &str) -> String {
    format!(
        "arn:aws:codebuild:{REGION}:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// RULE: `CreateReportGroup` mints a report group from the request alone — the ARN
/// `arn:aws:codebuild:<region>:<account>:report-group/<name>`, the requested `name`,
/// `type`, `exportConfig` and `tags` kept verbatim, a read-only `status` of `ACTIVE`
/// (the model: "The report group is active"), and `created` == `lastModified` because
/// nothing has changed it yet — and the stored record is exactly what a later read
/// reports.
///
/// The table spans the classes the rule generalises over: both `ReportType` values
/// (`smithy_query(shape="ReportType")`: TEST | CODE_COVERAGE), each
/// `ReportExportConfigType` (S3 | NO_EXPORT) and each `ReportPackagingType`
/// (ZIP | NONE), and tag lists at 0, 1 and the 50 entries `TagList`'s
/// `@length(min: 0, max: 50)` allows.
#[tokio::test]
async fn create_report_group_persists_every_member_it_was_given() {
    let router = emulator();

    let fifty_tags = Value::Array(
        (0..50)
            .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
            .collect(),
    );

    // (case, type, exportConfig, tags)
    let cases: Vec<(&str, &str, Value, Option<Value>)> = vec![
        (
            "test reports, no export",
            "TEST",
            json!({"exportConfigType": "NO_EXPORT"}),
            None,
        ),
        (
            "code coverage reports, no export",
            "CODE_COVERAGE",
            json!({"exportConfigType": "NO_EXPORT"}),
            None,
        ),
        (
            "s3 export, bucket only",
            "TEST",
            json!({"exportConfigType": "S3", "s3Destination": {"bucket": "reports-bucket"}}),
            None,
        ),
        (
            "s3 export, zip packaging under a path",
            "TEST",
            json!({
                "exportConfigType": "S3",
                "s3Destination": {
                    "bucket": "reports-bucket",
                    "path": "junit/",
                    "packaging": "ZIP",
                },
            }),
            None,
        ),
        (
            "s3 export, unpackaged and unencrypted for another bucket owner",
            "CODE_COVERAGE",
            json!({
                "exportConfigType": "S3",
                "s3Destination": {
                    "bucket": "reports-bucket",
                    "bucketOwner": "111122223333",
                    "packaging": "NONE",
                    "encryptionDisabled": true,
                },
            }),
            None,
        ),
        (
            "s3 export with a customer key",
            "TEST",
            json!({
                "exportConfigType": "S3",
                "s3Destination": {
                    "bucket": "reports-bucket",
                    "encryptionKey": "alias/aws/s3",
                    "encryptionDisabled": false,
                },
            }),
            None,
        ),
        (
            "an empty tag list",
            "TEST",
            json!({"exportConfigType": "NO_EXPORT"}),
            Some(json!([])),
        ),
        (
            "one tag",
            "TEST",
            json!({"exportConfigType": "NO_EXPORT"}),
            Some(json!([{"key": "team", "value": "platform"}])),
        ),
        (
            "the maximum of 50 tags",
            "CODE_COVERAGE",
            json!({"exportConfigType": "NO_EXPORT"}),
            Some(fifty_tags),
        ),
    ];

    for (index, (case, report_type, export_config, tags)) in cases.into_iter().enumerate() {
        let name = format!("group-{index}");
        let (status, body) = call(
            &router,
            "CreateReportGroup",
            create_report_group_body(&name, report_type, export_config.clone(), tags.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );

        let created = body["reportGroup"].clone();
        let arn = expected_arn(&format!("report-group/{name}"));
        assert_eq!(
            created["arn"],
            json!(arn),
            "{case}: expected arn {arn}, got {created}"
        );
        assert_eq!(
            created["name"],
            json!(name),
            "{case}: expected name {name}, got {created}"
        );
        assert_eq!(
            created["type"],
            json!(report_type),
            "{case}: expected type {report_type}, got {created}"
        );
        assert_eq!(
            created["exportConfig"], export_config,
            "{case}: expected exportConfig {export_config}, got {created}"
        );
        assert_eq!(
            created["tags"],
            tags.clone().unwrap_or(Value::Null),
            "{case}: expected tags {tags:?}, got {created}"
        );
        assert_eq!(
            created["status"],
            json!("ACTIVE"),
            "{case}: a created report group must be ACTIVE, got {created}"
        );
        assert!(
            created["created"].as_f64().unwrap_or_default() > 0.0,
            "{case}: expected a created timestamp, got {created}"
        );
        assert_eq!(
            created["created"], created["lastModified"],
            "{case}: an unmodified report group's lastModified must equal created, \
             got {created}"
        );

        // The read path reports the stored record, not a recomputed one — and reading
        // it twice changes nothing a later caller sees.
        for read in ["first", "second"] {
            let (status, fetched) = call(
                &router,
                "BatchGetReportGroups",
                json!({"reportGroupArns": [arn]}),
            )
            .await;
            assert_eq!(
                status, 200,
                "{case}: {read} BatchGetReportGroups expected 200, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["reportGroups"][0], created,
                "{case}: {read} BatchGetReportGroups must report the stored report group, \
                 got {fetched}"
            );
            assert_eq!(
                fetched["reportGroupsNotFound"],
                Value::Null,
                "{case}: {read} read of a held ARN must report nothing not found, got {fetched}"
            );
        }
    }
}

/// RULE: a request violating a constraint the model declares is refused with HTTP 400
/// `InvalidInputException`, naming the member and the constraint it failed; one that
/// satisfies every constraint is accepted. The table walks `ReportGroupName`'s
/// published bounds (`smithy_query(shape="ReportGroupName")`: `@length(min: 2,
/// max: 128)`) and one case past each.
#[tokio::test]
async fn a_report_group_name_outside_its_declared_constraints_is_refused() {
    let router = emulator();

    let accepted: Vec<(&str, String)> = vec![
        ("at the minimum length", "ab".to_string()),
        ("at the maximum length", "a".repeat(128)),
    ];
    for (case, name) in accepted {
        let (status, body) = call(
            &router,
            "CreateReportGroup",
            create_report_group_body(&name, "TEST", json!({"exportConfigType": "NO_EXPORT"}), None),
        )
        .await;
        assert_eq!(
            status,
            200,
            "{case} ({} chars): expected 200, got {status} with {body}",
            name.len()
        );
    }

    let refused: Vec<(&str, String)> = vec![
        ("one character, below the minimum length", "a".to_string()),
        ("past the maximum length", "a".repeat(129)),
    ];
    for (case, name) in refused {
        let (status, body) = call(
            &router,
            "CreateReportGroup",
            create_report_group_body(&name, "TEST", json!({"exportConfigType": "NO_EXPORT"}), None),
        )
        .await;
        assert_eq!(
            status,
            400,
            "{case} ({} chars): expected 400, got {status} with {body}",
            name.len()
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "{case}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains("name"),
            "{case}: the refusal must name the member, got {body}"
        );
    }

    // A required member left out, and a `type` outside the modelled enum, are the same
    // class of refusal.
    let malformed: Vec<(&str, Value)> = vec![
        ("no members at all", json!({})),
        (
            "exportConfig left out",
            json!({"name": "no-export-config", "type": "TEST"}),
        ),
        (
            "a type outside ReportType",
            create_report_group_body(
                "bad-type",
                "COVERAGE",
                json!({"exportConfigType": "NO_EXPORT"}),
                None,
            ),
        ),
        (
            "an exportConfigType outside ReportExportConfigType",
            create_report_group_body(
                "bad-export-type",
                "TEST",
                json!({"exportConfigType": "GLACIER"}),
                None,
            ),
        ),
    ];
    for (case, body) in malformed {
        let (status, response) = call(&router, "CreateReportGroup", body).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_type(&response),
            "InvalidInputException",
            "{case}: expected InvalidInputException, got {response}"
        );
    }
}

/// RULE: an `S3` export config has to name the bucket the raw report data is exported
/// to; the model leaves `s3Destination` optional because the same structure also
/// describes a `NO_EXPORT` config, so an `S3` config without a bucket is refused with
/// HTTP 400 `InvalidInputException` and persists nothing.
#[tokio::test]
async fn an_s3_export_config_without_a_bucket_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    let refused: Vec<(&str, Value)> = vec![
        ("no s3Destination at all", json!({"exportConfigType": "S3"})),
        (
            "an s3Destination naming no bucket",
            json!({"exportConfigType": "S3", "s3Destination": {"path": "junit/"}}),
        ),
        (
            "an empty bucket name",
            json!({"exportConfigType": "S3", "s3Destination": {"bucket": ""}}),
        ),
    ];
    for (index, (case, export_config)) in refused.into_iter().enumerate() {
        let (status, body) = call(
            &router,
            "CreateReportGroup",
            create_report_group_body(&format!("no-bucket-{index}"), "TEST", export_config, None),
        )
        .await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "{case}: expected InvalidInputException, got {body}"
        );
    }
    assert!(
        engine
            .state::<CodeBuildState>()
            .lock()
            .report_groups
            .is_empty(),
        "a refused CreateReportGroup must persist nothing"
    );

    // The same config WITH a bucket is accepted, so the refusal is about the missing
    // destination and not about `S3` export at all.
    let (status, body) = call(
        &router,
        "CreateReportGroup",
        create_report_group_body(
            "with-bucket",
            "TEST",
            json!({"exportConfigType": "S3", "s3Destination": {"bucket": "reports-bucket"}}),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "an S3 config naming a bucket: expected 200, got {status} with {body}"
    );
}

/// RULE: a report group name is unique per account, so creating one under a name the
/// account already holds is refused with HTTP 400 `ResourceAlreadyExistsException` and
/// leaves the existing record untouched.
#[tokio::test]
async fn creating_a_report_group_whose_name_is_taken_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    let first = create_report_group_body(
        "taken",
        "TEST",
        json!({"exportConfigType": "NO_EXPORT"}),
        Some(json!([{"key": "team", "value": "platform"}])),
    );
    let (status, body) = call(&router, "CreateReportGroup", first).await;
    assert_eq!(status, 200, "the first create: expected 200, got {body}");
    let created = body["reportGroup"].clone();

    // A second create under the same name, with DIFFERENT members, is still a refusal:
    // the name is what is unique, not the settings.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the same request again",
            create_report_group_body(
                "taken",
                "TEST",
                json!({"exportConfigType": "NO_EXPORT"}),
                Some(json!([{"key": "team", "value": "platform"}])),
            ),
        ),
        (
            "a different type",
            create_report_group_body(
                "taken",
                "CODE_COVERAGE",
                json!({"exportConfigType": "NO_EXPORT"}),
                None,
            ),
        ),
        (
            "a different export config",
            create_report_group_body(
                "taken",
                "TEST",
                json!({"exportConfigType": "S3", "s3Destination": {"bucket": "other-bucket"}}),
                None,
            ),
        ),
    ];
    for (case, body) in cases {
        let (status, response) = call(&router, "CreateReportGroup", body).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_type(&response),
            "ResourceAlreadyExistsException",
            "{case}: expected ResourceAlreadyExistsException, got {response}"
        );
    }

    let (_, fetched) = call(
        &router,
        "BatchGetReportGroups",
        json!({"reportGroupArns": [expected_arn("report-group/taken")]}),
    )
    .await;
    assert_eq!(
        fetched["reportGroups"][0], created,
        "a refused create must leave the existing record untouched, got {fetched}"
    );
    assert_eq!(
        engine.state::<CodeBuildState>().lock().report_groups.len(),
        1,
        "a refused create must not have grown the store"
    );
}

/// RULE: `BatchGetReportGroups` answers position by position for the ARNs it was given —
/// every ARN this account holds in `reportGroups`, in request order, and every other
/// ARN in `reportGroupsNotFound` — and an ARN that is not this account's report group in
/// this region resolves to nothing.
#[tokio::test]
async fn batch_get_report_groups_splits_held_arns_from_the_rest() {
    let router = emulator();

    let mut held = Vec::new();
    for index in 0..3 {
        let name = format!("held-{index}");
        let (status, body) = call(
            &router,
            "CreateReportGroup",
            create_report_group_body(&name, "TEST", json!({"exportConfigType": "NO_EXPORT"}), None),
        )
        .await;
        assert_eq!(status, 200, "creating {name}: expected 200, got {body}");
        held.push(expected_arn(&format!("report-group/{name}")));
    }

    // ARNs that must NOT resolve, each wrong in one way. The lookup compares the
    // STORED arn, so a name that exists under a different region, account or resource
    // type is still not this account's report group.
    let absent: Vec<(&str, String)> = vec![
        (
            "a name nothing was created under",
            expected_arn("report-group/never-created"),
        ),
        (
            "the right name in another region",
            format!(
                "arn:aws:codebuild:eu-west-1:{}:report-group/held-0",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "the right name in another account",
            "arn:aws:codebuild:us-west-2:111122223333:report-group/held-0".to_string(),
        ),
        (
            "the right name under another resource type",
            expected_arn("project/held-0"),
        ),
        ("a bare name rather than an ARN", "held-0".to_string()),
    ];

    for (case, arn) in &absent {
        let (status, body) = call(
            &router,
            "BatchGetReportGroups",
            json!({"reportGroupArns": [arn]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["reportGroupsNotFound"],
            json!([arn]),
            "{case}: expected the ARN in reportGroupsNotFound, got {body}"
        );
        assert_eq!(
            body["reportGroups"],
            Value::Null,
            "{case}: expected no report groups, got {body}"
        );
    }

    // A mixed request: order is the request's, and each ARN lands in exactly one list.
    let requested = vec![
        absent[0].1.clone(),
        held[2].clone(),
        held[0].clone(),
        absent[1].1.clone(),
        held[1].clone(),
    ];
    let (status, body) = call(
        &router,
        "BatchGetReportGroups",
        json!({"reportGroupArns": requested}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a mixed request: expected 200, got {status} with {body}"
    );
    let reported: Vec<String> = body["reportGroups"]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| group["arn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported,
        vec![held[2].clone(), held[0].clone(), held[1].clone()],
        "a mixed request must report held ARNs in request order, got {body}"
    );
    assert_eq!(
        body["reportGroupsNotFound"],
        json!([absent[0].1.clone(), absent[1].1.clone()]),
        "a mixed request must report the rest as not found, got {body}"
    );
}

/// RULE: the report-group store is bounded at its growth site — `CreateReportGroup`
/// answers the account quota it models rather than growing past it.
#[tokio::test]
async fn the_report_group_store_is_bounded_at_its_growth_site() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let handle = engine.state::<CodeBuildState>();
        let mut guard = handle.lock();
        for index in 0..state::MAX_REPORT_GROUPS {
            let name = format!("full-{index:06}");
            guard.report_groups.insert(
                name.clone(),
                StoredReportGroup {
                    report_group: ReportGroup {
                        arn: Some(state::report_group_arn(REGION, &name)),
                        name: Some(name),
                        ..ReportGroup::default()
                    },
                    created_millis: 0,
                    last_modified_millis: 0,
                },
            );
        }
    }

    let router = emulator_on(Arc::clone(&engine));
    let (status, body) = call(
        &router,
        "CreateReportGroup",
        create_report_group_body(
            "one-too-many",
            "TEST",
            json!({"exportConfigType": "NO_EXPORT"}),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "CreateReportGroup at the account quota: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_type(&body),
        "AccountLimitExceededException",
        "CreateReportGroup at the account quota: expected AccountLimitExceededException, \
         got {body}"
    );
    assert_eq!(
        engine.state::<CodeBuildState>().lock().report_groups.len(),
        state::MAX_REPORT_GROUPS,
        "a refused CreateReportGroup must not have grown the store"
    );
}

// ---------------------------------------------------------------------------
// The audit record a state-changing call emits
// ---------------------------------------------------------------------------

/// The access key id the audit test signs with: its own, so its records are
/// identifiable among those the sibling tests in this binary write.
const AUDIT_ACCESS_KEY_ID: &str = "AKIDCODEBUILDAUDIT";

/// Records captured from `tracing`, oldest first.
#[derive(Clone, Default)]
struct CapturedRecords(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// Every record captured since the last drain, clearing the buffer.
    fn drain(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().expect("the audit capture lock"))
    }
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The capture for audit records, installed ONCE as this test binary's global `tracing`
/// subscriber.
///
/// Global rather than scoped on purpose: `tracing` caches per callsite whether any
/// subscriber is interested, the cache is process-wide, and sibling tests run in
/// parallel on other threads — so a thread-local subscriber is raced away as soon as
/// another thread re-evaluates a handler's callsite with no subscriber of its own. The
/// records of a sibling are told apart by the access key id each test signs with.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this test binary installs no other global tracing subscriber");
        captured
    })
}

/// The captured records on the `emulator::audit` target that THIS test's calls wrote.
fn audit_records(captured: &[String]) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with("emulator::audit ")
                && record.contains(&format!("access_key_id={AUDIT_ACCESS_KEY_ID}"))
        })
        .cloned()
        .collect()
}

/// RULE: a state-changing call emits exactly ONE structured record on the
/// `emulator::audit` target, naming the service, the operation, the identity the request
/// presented, the request target, the resource it was about and the outcome — on the
/// refusal paths as well as the successful one — and carries none of the caller's own
/// data.
///
/// The table walks every outcome `CreateReportGroup` can report on a fresh emulator; the
/// quota outcome is the one row that needs a seeded store, so it is asserted where the
/// store is seeded.
#[tokio::test]
async fn a_state_changing_call_emits_one_structured_audit_record() {
    let captured = audit_capture();

    // Harness self-test, so a failure below can never be mistaken for a missing audit
    // record: the capture must receive an event emitted right here.
    tracing::info!(target: "emulator::audit", access_key_id = AUDIT_ACCESS_KEY_ID, "probe");
    let probe = audit_records(&captured.drain());
    assert_eq!(
        probe.len(),
        1,
        "case \"subscriber self-test\": the capture must receive events, got {probe:?}"
    );

    let router = emulator();

    // (case, request, expected status, expected outcome token)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a report group was created",
            create_report_group_body(
                "audited",
                "TEST",
                json!({"exportConfigType": "S3", "s3Destination": {"bucket": "audit-bucket"}}),
                Some(json!([{"key": "owner", "value": "audit-tag-value"}])),
            ),
            200,
            "report-group-created",
        ),
        (
            "the name was already taken",
            create_report_group_body(
                "audited",
                "TEST",
                json!({"exportConfigType": "NO_EXPORT"}),
                None,
            ),
            400,
            "already-exists",
        ),
        (
            "an S3 export config named no bucket",
            create_report_group_body("audited-invalid", "TEST", json!({"exportConfigType": "S3"}), None),
            400,
            "validation-refused",
        ),
    ];

    for (case, body, expected_status, outcome) in rows {
        let name = body["name"].as_str().unwrap_or_default().to_string();
        let (status, response) =
            call_as(&router, AUDIT_ACCESS_KEY_ID, "CreateReportGroup", body).await;
        assert_eq!(
            status, expected_status,
            "{case}: expected {expected_status}, got {status} with {response}"
        );

        let records = audit_records(&captured.drain());
        assert_eq!(
            records.len(),
            1,
            "{case}: expected exactly one audit record, got {records:?}"
        );
        let record = &records[0];
        for expected in [
            "service=codebuild".to_string(),
            "operation=CreateReportGroup".to_string(),
            format!("access_key_id={AUDIT_ACCESS_KEY_ID}"),
            "request_target=POST /".to_string(),
            format!("resource={}", expected_arn(&format!("report-group/{name}"))),
            format!("outcome={outcome}"),
        ] {
            assert!(
                record.contains(&expected),
                "{case}: expected the record to carry {expected:?}, got {record:?}"
            );
        }
        // Metadata only: the caller's own values never reach the log.
        for secret in ["audit-bucket", "audit-tag-value"] {
            assert!(
                !record.contains(secret),
                "{case}: the record must carry none of the caller's data, got {record:?}"
            );
        }
    }

    // A control character in a request-derived value cannot forge a second record.
    let forging = "a\nemulator::audit forged";
    let (status, response) = call_as(
        &router,
        AUDIT_ACCESS_KEY_ID,
        "CreateReportGroup",
        create_report_group_body(forging, "TEST", json!({"exportConfigType": "S3"}), None),
    )
    .await;
    assert_eq!(
        status, 400,
        "a name carrying a newline: expected 400, got {status} with {response}"
    );
    let records = audit_records(&captured.drain());
    assert_eq!(
        records.len(),
        1,
        "a name carrying a newline must still emit exactly one record, got {records:?}"
    );
    assert!(
        !records[0].contains('\n'),
        "a request-derived value must not carry a line break into the log, got {:?}",
        records[0]
    );
}
