// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the report-definition lifecycle: `PutReportDefinition`,
//! `DescribeReportDefinitions`, `ModifyReportDefinition`, `TagResource`,
//! `ListTagsForResource`, `UntagResource` and `DeleteReportDefinition`, driven over the
//! wire through the same `MultiServiceRouter::dispatch` the served emulator uses.
//!
//! The rule under test throughout: every response is computed from the persisted store,
//! so a definition reads back member for member as it was supplied, an update is visible
//! to the next read, and a delete is visible to every later read. Each test asserts that
//! over a TABLE of inputs spanning what the rule generalises over (member values, the
//! modelled enum catalogs, list sizes, which optional members are present, report count),
//! not just one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice as cur;

use cur::ops::types::ReportDefinition;
use cur::state::{StoredReport, MAX_REPORTS};
use cur::ComAmazonawsCostandusagereportserviceAwsorigamiservicegatewayserviceState as ReportStore;

/// One emulator: the shared engine plus a router carrying only this service.
///
/// `pub(crate)` so the sibling suites of this test target (see `tests/all.rs`) drive the
/// service through the same wire harness instead of each rebuilding one.
pub(crate) struct Cur {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Cur {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These requests are unsigned; signature verification is not what is under test.
        router.set_request_authentication(false);
        router.register_service(cur::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Call one operation on the awsJson1_1 wire: `POST /` with
    /// `X-Amz-Target: AWSOrigamiServiceGatewayService.<OperationName>`. The SigV4
    /// credential scope names the `cur` signing name, which is how the router picks
    /// this service.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSOrigamiServiceGatewayService.{operation}"),
            )
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-east-1/cur/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    pub(crate) async fn put(&self, definition: &Value, tags: Option<&Value>) -> (u16, Value) {
        let mut body = json!({ "ReportDefinition": definition });
        if let Some(tags) = tags {
            body["Tags"] = tags.clone();
        }
        self.call("PutReportDefinition", &body).await
    }

    pub(crate) async fn describe(&self, page: &Value) -> (u16, Value) {
        self.call("DescribeReportDefinitions", page).await
    }

    /// Every definition the account holds, across as many pages as it takes.
    pub(crate) async fn describe_all(&self) -> Vec<Value> {
        let mut page = json!({});
        let mut seen = Vec::new();
        loop {
            let (status, body) = self.describe(&page).await;
            assert_eq!(status, 200, "DescribeReportDefinitions: got {body:?}");
            seen.extend(
                body["ReportDefinitions"]
                    .as_array()
                    .expect("ReportDefinitions is a list")
                    .iter()
                    .cloned(),
            );
            match body["NextToken"].as_str() {
                Some(token) => page = json!({ "NextToken": token }),
                None => return seen,
            }
        }
    }

    pub(crate) async fn tags_of(&self, report_name: &str) -> (u16, Value) {
        self.call(
            "ListTagsForResource",
            &json!({ "ReportName": report_name }),
        )
        .await
    }

    /// Replace the store's contents, so a test can set up a report count
    /// `PutReportDefinition` would refuse (its five-report cap is itself under test).
    pub(crate) fn seed(&self, reports: Vec<StoredReport>) {
        self.engine.state::<ReportStore>().lock().reports = reports;
    }
}

/// The required members of a report definition, as the model lists them.
pub(crate) fn minimal_definition(report_name: &str) -> Value {
    json!({
        "ReportName": report_name,
        "TimeUnit": "DAILY",
        "Format": "textORcsv",
        "Compression": "GZIP",
        "AdditionalSchemaElements": ["RESOURCES"],
        "S3Bucket": "my-bucket",
        "S3Prefix": "cur/",
        "S3Region": "us-east-1",
    })
}

/// A stored report for seeding, with every required member filled.
pub(crate) fn stored(report_name: &str) -> StoredReport {
    StoredReport::new(
        ReportDefinition {
            report_name: report_name.to_string(),
            time_unit: "DAILY".to_string(),
            format: "textORcsv".to_string(),
            compression: "GZIP".to_string(),
            additional_schema_elements: vec!["RESOURCES".to_string()],
            s3_bucket: "paging-bucket".to_string(),
            s3_prefix: "cur/".to_string(),
            s3_region: "us-east-1".to_string(),
            ..Default::default()
        },
        &[],
    )
    .expect("no tags, so the per-report tag limit cannot be crossed")
}

/// The wire error code (`__type`) of an error response.
pub(crate) fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or("<no __type>")
}

/// Definitions spanning the class: which optional members are present, how long the
/// lists are, which enum values and which name spelling. Every row is a different point
/// along what the round-trip rule generalises over.
fn definition_table() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "required members only, no tags",
            minimal_definition("minimal-report"),
            json!([]),
        ),
        (
            "every optional member present",
            json!({
                "ReportName": "full-report",
                "TimeUnit": "HOURLY",
                "Format": "Parquet",
                "Compression": "Parquet",
                "AdditionalSchemaElements": ["RESOURCES", "SPLIT_COST_ALLOCATION_DATA"],
                "S3Bucket": "full.bucket-name",
                "S3Prefix": "teams/finance/cur/",
                "S3Region": "eu-west-1",
                "AdditionalArtifacts": ["ATHENA", "QUICKSIGHT", "REDSHIFT"],
                "RefreshClosedReports": true,
                "ReportVersioning": "OVERWRITE_REPORT",
                "BillingViewArn": "arn:aws:billing::123456789012:billingview/123456789012",
                "ReportStatus": { "lastDelivery": "20240101T000000Z", "lastStatus": "SUCCESS" },
            }),
            json!([{ "Key": "team", "Value": "fin" }, { "Key": "cost-center", "Value": "42" }]),
        ),
        (
            "empty S3 prefix and empty artifact list",
            json!({
                "ReportName": "empty-prefix-report",
                "TimeUnit": "MONTHLY",
                "Format": "textORcsv",
                "Compression": "ZIP",
                "AdditionalSchemaElements": [],
                "S3Bucket": "bucket_with_underscores",
                "S3Prefix": "",
                "S3Region": "ap-southeast-2",
                "AdditionalArtifacts": [],
                "RefreshClosedReports": false,
            }),
            json!([{ "Key": "blank", "Value": "" }]),
        ),
        (
            "name using the punctuation the ReportName pattern allows",
            json!({
                "ReportName": "report!name-with_punctuation.v1*()",
                "TimeUnit": "DAILY",
                "Format": "textORcsv",
                "Compression": "GZIP",
                "AdditionalSchemaElements": ["MANUAL_DISCOUNT_COMPATIBILITY"],
                "S3Bucket": "edge-bucket",
                "S3Prefix": "a/b/c/",
                "S3Region": "us-west-2",
                "ReportVersioning": "CREATE_NEW_REPORT",
            }),
            json!([{ "Key": "unicode-value", "Value": "équipe" }]),
        ),
        (
            "report status reported as an error, single artifact",
            json!({
                "ReportName": "status-report",
                "TimeUnit": "HOURLY",
                "Format": "textORcsv",
                "Compression": "GZIP",
                "AdditionalSchemaElements": ["RESOURCES"],
                "S3Bucket": "status-bucket",
                "S3Prefix": "status/",
                "S3Region": "sa-east-1",
                "AdditionalArtifacts": ["REDSHIFT"],
                "ReportStatus": { "lastStatus": "ERROR_NO_BUCKET" },
            }),
            json!([]),
        ),
    ]
}

#[tokio::test]
async fn a_put_definition_reads_back_member_for_member_through_describe() {
    for (case, definition, tags) in definition_table() {
        let cur = Cur::new();

        let (status, body) = cur.put(&definition, Some(&tags)).await;
        assert_eq!(status, 200, "case {case:?}: PutReportDefinition got {body:?}");

        let listed = cur.describe_all().await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected exactly one definition, got {listed:?}"
        );
        assert_eq!(
            listed[0], definition,
            "case {case:?}: expected the definition as supplied, got {:?}",
            listed[0]
        );

        let report_name = definition["ReportName"].as_str().expect("a report name");
        let (status, body) = cur.tags_of(report_name).await;
        assert_eq!(status, 200, "case {case:?}: ListTagsForResource got {body:?}");
        let mut expected: Vec<Value> = tags.as_array().expect("a tag list").clone();
        expected.sort_by_key(|tag| tag["Key"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            body["Tags"].as_array().cloned().unwrap_or_default(),
            expected,
            "case {case:?}: expected exactly the tags supplied, got {:?}",
            body["Tags"]
        );
    }
}

#[tokio::test]
async fn every_value_in_the_modelled_enum_catalogs_round_trips() {
    // The catalogs come from the service's own model, generated into
    // `ops::types` (`smithy_query(shape="TimeUnit,ReportFormat,CompressionFormat,
    // SchemaElement,AdditionalArtifact,ReportVersioning,AWSRegion")`), so this covers
    // every value the wire accepts rather than the ones the issue happened to cite.
    let catalogs: Vec<(&str, &[&str])> = vec![
        ("TimeUnit", cur::ops::types::TIME_UNIT_VALUES),
        ("Format", cur::ops::types::REPORT_FORMAT_VALUES),
        ("Compression", cur::ops::types::COMPRESSION_FORMAT_VALUES),
        ("S3Region", cur::ops::types::A_W_S_REGION_VALUES),
        ("ReportVersioning", cur::ops::types::REPORT_VERSIONING_VALUES),
    ];

    for (member, values) in catalogs {
        assert!(
            !values.is_empty(),
            "case {member:?}: the model's value catalog must not be empty"
        );
        for value in values {
            let cur = Cur::new();
            let mut definition = minimal_definition("catalog-report");
            definition[member] = json!(value);

            let (status, body) = cur.put(&definition, None).await;
            assert_eq!(
                status, 200,
                "case {member:?}={value:?}: PutReportDefinition got {body:?}"
            );

            let listed = cur.describe_all().await;
            assert_eq!(
                listed.first(), Some(&definition),
                "case {member:?}={value:?}: expected the definition as supplied, got {listed:?}"
            );
        }
    }

    // The list-valued members take any subset of their catalog, so each catalog is also
    // tested as a whole list and as each single element.
    let list_catalogs: Vec<(&str, &[&str])> = vec![
        (
            "AdditionalSchemaElements",
            cur::ops::types::SCHEMA_ELEMENT_VALUES,
        ),
        (
            "AdditionalArtifacts",
            cur::ops::types::ADDITIONAL_ARTIFACT_VALUES,
        ),
    ];
    for (member, values) in list_catalogs {
        for list in std::iter::once(values.to_vec()).chain(values.iter().map(|v| vec![*v])) {
            let cur = Cur::new();
            let mut definition = minimal_definition("list-catalog-report");
            definition[member] = json!(list);

            let (status, body) = cur.put(&definition, None).await;
            assert_eq!(
                status, 200,
                "case {member:?}={list:?}: PutReportDefinition got {body:?}"
            );

            let listed = cur.describe_all().await;
            assert_eq!(
                listed.first(), Some(&definition),
                "case {member:?}={list:?}: expected the definition as supplied, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_the_account_already_holds_is_refused_as_a_duplicate() {
    // Rows differ in the name's spelling and in whether the second request carries the
    // same body as the first: a duplicate is a duplicate by name alone.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "identical definition",
            minimal_definition("same-report"),
            minimal_definition("same-report"),
        ),
        (
            "same name, different members",
            minimal_definition("busy-report"),
            json!({
                "ReportName": "busy-report",
                "TimeUnit": "MONTHLY",
                "Format": "Parquet",
                "Compression": "Parquet",
                "AdditionalSchemaElements": ["RESOURCES"],
                "S3Bucket": "other-bucket",
                "S3Prefix": "other/",
                "S3Region": "eu-central-1",
            }),
        ),
        (
            "name using the pattern's punctuation",
            minimal_definition("odd!name_(1)"),
            minimal_definition("odd!name_(1)"),
        ),
    ];

    for (case, first, second) in cases {
        let cur = Cur::new();
        let (status, body) = cur.put(&first, None).await;
        assert_eq!(status, 200, "case {case:?}: the first put got {body:?}");

        let (status, body) = cur.put(&second, None).await;
        assert_eq!(status, 400, "case {case:?}: the second put got {body:?}");
        assert_eq!(
            error_code(&body),
            "DuplicateReportNameException",
            "case {case:?}: expected DuplicateReportNameException, got {body:?}"
        );

        // The refusal left the stored definition untouched.
        let listed = cur.describe_all().await;
        assert_eq!(
            listed, vec![first.clone()],
            "case {case:?}: expected only the first definition, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_account_holds_at_most_the_modelled_report_limit() {
    let cur = Cur::new();
    for index in 0..MAX_REPORTS {
        let definition = minimal_definition(&format!("report-{index}"));
        let (status, body) = cur.put(&definition, None).await;
        assert_eq!(status, 200, "case report-{index}: got {body:?}");
    }

    let over_limit = minimal_definition("one-too-many");
    let (status, body) = cur.put(&over_limit, None).await;
    assert_eq!(status, 400, "case over the limit: got {body:?}");
    assert_eq!(
        error_code(&body),
        "ReportLimitReachedException",
        "case over the limit: expected ReportLimitReachedException, got {body:?}"
    );
    assert_eq!(
        cur.describe_all().await.len(),
        MAX_REPORTS,
        "case over the limit: the refused put must not have been stored"
    );

    // The limit counts what is stored, so deleting one frees exactly one slot.
    let (status, body) = cur
        .call(
            "DeleteReportDefinition",
            &json!({ "ReportName": "report-0" }),
        )
        .await;
    assert_eq!(status, 200, "case delete to free a slot: got {body:?}");

    let (status, body) = cur.put(&over_limit, None).await;
    assert_eq!(status, 200, "case put after a delete: got {body:?}");
    let names: Vec<String> = cur
        .describe_all()
        .await
        .iter()
        .map(|definition| {
            definition["ReportName"]
                .as_str()
                .expect("a report name")
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "report-1".to_string(),
            "report-2".to_string(),
            "report-3".to_string(),
            "report-4".to_string(),
            "one-too-many".to_string()
        ],
        "case put after a delete: expected creation order with the deleted report gone, got {names:?}"
    );
}

#[tokio::test]
async fn modify_replaces_the_stored_definition() {
    // Rows differ in which members change, including a rename and adding or dropping an
    // optional member: the replacement is wholesale, whatever changed.
    let cases: Vec<(&str, Value)> = vec![
        (
            "one enum member changes",
            json!({
                "ReportName": "base-report",
                "TimeUnit": "HOURLY",
                "Format": "textORcsv",
                "Compression": "GZIP",
                "AdditionalSchemaElements": ["RESOURCES"],
                "S3Bucket": "my-bucket",
                "S3Prefix": "cur/",
                "S3Region": "us-east-1",
            }),
        ),
        (
            "destination and schema elements change",
            json!({
                "ReportName": "base-report",
                "TimeUnit": "DAILY",
                "Format": "Parquet",
                "Compression": "Parquet",
                "AdditionalSchemaElements": ["RESOURCES", "SPLIT_COST_ALLOCATION_DATA"],
                "S3Bucket": "another-bucket",
                "S3Prefix": "moved/",
                "S3Region": "eu-north-1",
            }),
        ),
        (
            "optional members added",
            json!({
                "ReportName": "base-report",
                "TimeUnit": "DAILY",
                "Format": "textORcsv",
                "Compression": "GZIP",
                "AdditionalSchemaElements": ["RESOURCES"],
                "S3Bucket": "my-bucket",
                "S3Prefix": "cur/",
                "S3Region": "us-east-1",
                "AdditionalArtifacts": ["ATHENA"],
                "RefreshClosedReports": true,
                "ReportVersioning": "CREATE_NEW_REPORT",
            }),
        ),
        (
            "renamed",
            json!({
                "ReportName": "renamed-report",
                "TimeUnit": "MONTHLY",
                "Format": "textORcsv",
                "Compression": "ZIP",
                "AdditionalSchemaElements": ["RESOURCES"],
                "S3Bucket": "my-bucket",
                "S3Prefix": "cur/",
                "S3Region": "us-east-1",
            }),
        ),
    ];

    for (case, replacement) in cases {
        let cur = Cur::new();
        let base = minimal_definition("base-report");
        let tags = json!([{ "Key": "team", "Value": "fin" }]);
        let (status, body) = cur.put(&base, Some(&tags)).await;
        assert_eq!(status, 200, "case {case:?}: the put got {body:?}");
        // A second report pins that modify touches only the one it names.
        let bystander = minimal_definition("bystander-report");
        let (status, body) = cur.put(&bystander, None).await;
        assert_eq!(status, 200, "case {case:?}: the bystander put got {body:?}");

        let (status, body) = cur
            .call(
                "ModifyReportDefinition",
                &json!({ "ReportName": "base-report", "ReportDefinition": replacement }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: ModifyReportDefinition got {body:?}");

        let listed = cur.describe_all().await;
        assert_eq!(
            listed,
            vec![replacement.clone(), bystander.clone()],
            "case {case:?}: expected the replacement in place and the bystander untouched, got {listed:?}"
        );

        // Tags belong to the report, not to its name, so a rename keeps them.
        let new_name = replacement["ReportName"].as_str().expect("a report name");
        let (status, body) = cur.tags_of(new_name).await;
        assert_eq!(status, 200, "case {case:?}: ListTagsForResource got {body:?}");
        assert_eq!(
            body["Tags"],
            tags,
            "case {case:?}: expected the tags to survive the modify, got {:?}",
            body["Tags"]
        );
    }
}

#[tokio::test]
async fn modify_refuses_a_name_the_account_does_not_hold_or_a_colliding_rename() {
    let cur = Cur::new();
    let (status, body) = cur.put(&minimal_definition("held-report"), None).await;
    assert_eq!(status, 200, "case setup: got {body:?}");
    let (status, body) = cur.put(&minimal_definition("other-report"), None).await;
    assert_eq!(status, 200, "case setup: got {body:?}");

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "unknown report name",
            "absent-report",
            minimal_definition("absent-report"),
        ),
        (
            "unknown name, body naming a held report",
            "absent-report",
            minimal_definition("held-report"),
        ),
        (
            "rename onto a name another report holds",
            "held-report",
            minimal_definition("other-report"),
        ),
    ];

    for (case, report_name, definition) in cases {
        let (status, body) = cur
            .call(
                "ModifyReportDefinition",
                &json!({ "ReportName": report_name, "ReportDefinition": definition }),
            )
            .await;
        assert_eq!(status, 400, "case {case:?}: got {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    let listed = cur.describe_all().await;
    assert_eq!(
        listed,
        vec![
            minimal_definition("held-report"),
            minimal_definition("other-report")
        ],
        "a refused modify must change nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn tags_merge_by_key_and_untag_removes_only_the_named_keys() {
    #[derive(Debug)]
    enum Step {
        Tag(Value),
        Untag(Value),
    }

    // Rows differ in the operation, the number of keys and whether a key is already
    // attached: a tag write merges by key, an untag removes by key.
    let steps: Vec<(&str, Step, Value)> = vec![
        (
            "a new key is added",
            Step::Tag(json!([{ "Key": "env", "Value": "dev" }])),
            json!([{ "Key": "env", "Value": "dev" }, { "Key": "team", "Value": "fin" }]),
        ),
        (
            "an attached key takes the new value",
            Step::Tag(json!([{ "Key": "team", "Value": "ops" }])),
            json!([{ "Key": "env", "Value": "dev" }, { "Key": "team", "Value": "ops" }]),
        ),
        (
            "several keys at once, one of them empty-valued",
            Step::Tag(json!([
                { "Key": "owner", "Value": "alice" },
                { "Key": "note", "Value": "" }
            ])),
            json!([
                { "Key": "env", "Value": "dev" },
                { "Key": "note", "Value": "" },
                { "Key": "owner", "Value": "alice" },
                { "Key": "team", "Value": "ops" }
            ]),
        ),
        (
            "an unattached key is nothing to remove",
            Step::Untag(json!(["never-attached"])),
            json!([
                { "Key": "env", "Value": "dev" },
                { "Key": "note", "Value": "" },
                { "Key": "owner", "Value": "alice" },
                { "Key": "team", "Value": "ops" }
            ]),
        ),
        (
            "two attached keys are removed",
            Step::Untag(json!(["env", "note"])),
            json!([{ "Key": "owner", "Value": "alice" }, { "Key": "team", "Value": "ops" }]),
        ),
        (
            "removing the rest leaves no tags",
            Step::Untag(json!(["owner", "team"])),
            json!([]),
        ),
    ];

    let cur = Cur::new();
    let (status, body) = cur
        .put(
            &minimal_definition("tagged-report"),
            Some(&json!([{ "Key": "team", "Value": "fin" }])),
        )
        .await;
    assert_eq!(status, 200, "case setup: got {body:?}");

    for (case, step, expected) in steps {
        let (status, body) = match &step {
            Step::Tag(tags) => {
                cur.call(
                    "TagResource",
                    &json!({ "ReportName": "tagged-report", "Tags": tags }),
                )
                .await
            }
            Step::Untag(keys) => {
                cur.call(
                    "UntagResource",
                    &json!({ "ReportName": "tagged-report", "TagKeys": keys }),
                )
                .await
            }
        };
        assert_eq!(status, 200, "case {case:?}: {step:?} got {body:?}");

        let (status, body) = cur.tags_of("tagged-report").await;
        assert_eq!(status, 200, "case {case:?}: ListTagsForResource got {body:?}");
        assert_eq!(
            body["Tags"], expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            body["Tags"]
        );
    }
}

#[tokio::test]
async fn every_tag_operation_refuses_a_report_the_account_does_not_hold() {
    let cur = Cur::new();
    let (status, body) = cur.put(&minimal_definition("held-report"), None).await;
    assert_eq!(status, 200, "case setup: got {body:?}");

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "ListTagsForResource",
            "ListTagsForResource",
            json!({ "ReportName": "absent-report" }),
        ),
        (
            "TagResource",
            "TagResource",
            json!({ "ReportName": "absent-report", "Tags": [{ "Key": "k", "Value": "v" }] }),
        ),
        (
            "UntagResource",
            "UntagResource",
            json!({ "ReportName": "absent-report", "TagKeys": ["k"] }),
        ),
        (
            "ListTagsForResource with a name differing only in case",
            "ListTagsForResource",
            json!({ "ReportName": "HELD-REPORT" }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = cur.call(operation, &request).await;
        assert_eq!(status, 400, "case {case:?}: got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn describe_pages_over_every_stored_report_exactly_once() {
    // Rows differ in how many reports are stored relative to the page size and in
    // whether the request names MaxResults: paging covers the store exactly once either
    // way. Counts above the five-report cap are seeded directly, because the cap is
    // PutReportDefinition's rule, not Describe's.
    for stored_count in [0_usize, 1, 4, 5, 6, 11] {
        for max_results in [None, Some(5)] {
            let cur = Cur::new();
            let names: Vec<String> = (0..stored_count)
                .map(|index| format!("paged-report-{index}"))
                .collect();
            cur.seed(names.iter().map(|name| stored(name.as_str())).collect());

            let case = format!("{stored_count} stored, MaxResults={max_results:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut request = json!({});
                if let Some(max) = max_results {
                    request["MaxResults"] = json!(max);
                }
                if let Some(token) = &token {
                    request["NextToken"] = json!(token);
                }

                let (status, body) = cur.describe(&request).await;
                assert_eq!(status, 200, "case {case:?}: got {body:?}");
                pages += 1;

                let page = body["ReportDefinitions"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: ReportDefinitions is a list"))
                    .clone();
                token = body["NextToken"].as_str().map(str::to_string);

                if token.is_some() {
                    assert_eq!(
                        page.len(),
                        5,
                        "case {case:?}: a page carrying a NextToken must be full, got {}",
                        page.len()
                    );
                }
                seen.extend(page.iter().map(|definition| {
                    definition["ReportName"]
                        .as_str()
                        .expect("a report name")
                        .to_string()
                }));
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= stored_count + 1,
                    "case {case:?}: paging did not terminate after {pages} pages"
                );
            }

            assert_eq!(
                seen, names,
                "case {case:?}: expected every stored report exactly once in creation order, got {seen:?}"
            );
            // A page is issued for an empty store too, and a token is issued only when
            // reports remain, so an exact multiple of the page size needs no extra page.
            let expected_pages = stored_count.div_ceil(5).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} pages, got {pages}"
            );
        }
    }
}

#[tokio::test]
async fn describe_refuses_a_next_token_it_did_not_issue() {
    let cur = Cur::new();
    cur.seed(vec![stored("first-report"), stored("second-report")]);

    // Rows span the ways a token can fail to name a stored report while still satisfying
    // the modelled NextToken pattern `^[A-Za-z0-9_\.\-=]*$`.
    let cases: Vec<(&str, String)> = vec![
        ("not an encoded name at all", "zzz".to_string()),
        ("an odd number of hex digits", "6669727".to_string()),
        (
            "a well-formed token naming a report the account does not hold",
            hex::encode("ghost-report"),
        ),
    ];

    for (case, token) in cases {
        let (status, body) = cur.describe(&json!({ "NextToken": token })).await;
        assert_eq!(status, 400, "case {case:?}: token {token:?} got {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // A token the service itself issued resumes where the previous page ended.
    cur.seed((0..7).map(|index| stored(&format!("r{index}"))).collect());
    let (_, first) = cur.describe(&json!({ "MaxResults": 5 })).await;
    let token = first["NextToken"].as_str().expect("a NextToken").to_string();
    let (status, second) = cur.describe(&json!({ "NextToken": token })).await;
    assert_eq!(status, 200, "case resumed page: got {second:?}");
    let resumed: Vec<&str> = second["ReportDefinitions"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|definition| definition["ReportName"].as_str().expect("a name"))
        .collect();
    assert_eq!(
        resumed,
        vec!["r5", "r6"],
        "case resumed page: expected the reports after the first page, got {resumed:?}"
    );
}

#[tokio::test]
async fn a_deleted_report_is_gone_from_every_later_read() {
    let cur = Cur::new();
    let kept = minimal_definition("kept-report");
    let doomed = minimal_definition("doomed-report");
    let (status, body) = cur.put(&kept, None).await;
    assert_eq!(status, 200, "case setup: got {body:?}");
    let (status, body) = cur
        .put(&doomed, Some(&json!([{ "Key": "team", "Value": "fin" }])))
        .await;
    assert_eq!(status, 200, "case setup: got {body:?}");

    let (status, body) = cur
        .call(
            "DeleteReportDefinition",
            &json!({ "ReportName": "doomed-report" }),
        )
        .await;
    assert_eq!(status, 200, "case delete: got {body:?}");
    let message = body["ResponseMessage"].as_str().unwrap_or_default();
    assert!(
        message.contains("doomed-report"),
        "case delete: expected a ResponseMessage naming the report, got {body:?}"
    );

    let listed = cur.describe_all().await;
    assert_eq!(
        listed,
        vec![kept.clone()],
        "case delete: expected only the kept report, got {listed:?}"
    );

    let (status, body) = cur.tags_of("doomed-report").await;
    assert_eq!(status, 400, "case tags after delete: got {body:?}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case tags after delete: expected ResourceNotFoundException, got {body:?}"
    );

    // Deleting it again is not refused — the operation models no not-found error, and its
    // ResponseMessage is modelled as "whether the deletion was successful or not".
    let (status, body) = cur
        .call(
            "DeleteReportDefinition",
            &json!({ "ReportName": "doomed-report" }),
        )
        .await;
    assert_eq!(status, 200, "case repeated delete: got {body:?}");
    assert_ne!(
        body["ResponseMessage"].as_str().unwrap_or_default(),
        message,
        "case repeated delete: the message must report that nothing was deleted, got {body:?}"
    );
    assert_eq!(
        cur.describe_all().await,
        vec![kept],
        "case repeated delete: the store must be unchanged"
    );

    // The name is free again once the report is gone.
    let (status, body) = cur.put(&doomed, None).await;
    assert_eq!(
        status, 200,
        "case recreate after delete: got {body:?}"
    );
    let (status, body) = cur.tags_of("doomed-report").await;
    assert_eq!(status, 200, "case tags after recreate: got {body:?}");
    assert_eq!(
        body["Tags"].as_array().map(Vec::len).unwrap_or_default(),
        0,
        "case tags after recreate: a recreated report starts untagged, got {body:?}"
    );
}
