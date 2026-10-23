// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the data-export lifecycle over the awsJson1_1 wire:
//! `CreateExport` -> `GetExport` / `ListExports` / `ListTagsForResource` ->
//! `UpdateExport` -> `DeleteExport`.
//!
//! Every assertion goes through the router, so it exercises exactly what a client sees:
//! `POST /` with `X-Amz-Target: AWSBillingAndCostManagementDataExports.<Op>` and a JSON
//! body of modelled member names (`OPERATIONS.md`, "Wire protocol").
//!
//! The rule under test is a round trip through persisted state: whatever `CreateExport`
//! accepted is what every later reader reports, until `UpdateExport` replaces it or
//! `DeleteExport` removes it. It is asserted over a TABLE of export definitions that
//! differ along what the rule generalises over — name length, an absent vs present
//! description, absent vs present table configurations and bucket owner, each published
//! value of every `S3OutputConfigurations` enum, and tag counts from zero to three —
//! because the rule is about persistence, not about any one definition.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::{
    COMPRESSION_OPTION_VALUES, FORMAT_OPTION_VALUES, FREQUENCY_OPTION_VALUES,
    OVERWRITE_OPTION_VALUES, S3_OUTPUT_TYPE_VALUES,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    MAX_EXPORTS, TABLE_CATALOG,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::EmulatedService;

/// This service's SigV4 signing name — the routing key the router reads out of the
/// request's credential scope to pick the claiming service (`OPERATIONS.md`).
const SIGNING_NAME: &str = "bcm-data-exports";

/// The region the test signs for, and therefore the region segment of every ARN the
/// service mints for these requests.
const REGION: &str = "us-east-1";

/// The access-key id these requests present unless a suite varies it: the AWS
/// documentation's own example id.
const DEFAULT_ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// `Export.Name`: `length 1..128`, `pattern ^[0-9A-Za-z\-_]+$`
/// (smithy_query shape `Export`, member `Name`).
const NAME_MAX_LEN: usize = 128;

/// The HTTP status this service answers the export-not-found condition with: the status
/// the wire recordings carry for an export ARN that resolves to nothing, which is the one
/// [`crate::export_lifecycle`]'s readers must see rather than the error shape's modelled
/// `@httpError 404`.
///
/// `pub(crate)` so every suite in this target asserts the one status.
pub(crate) const NOT_FOUND_STATUS: u16 = 400;

/// An in-process client: the one shared engine, this service mounted on a router, and a
/// signed-looking request per call.
///
/// `pub(crate)` so the other suites in this test target drive the service through the
/// SAME harness instead of each growing its own copy of it.
pub(crate) struct Client {
    router: MultiServiceRouter,
}

impl Client {
    pub(crate) fn new() -> Self {
        Self::with_engine(Arc::new(engine::InMemoryEngine::new()))
    }

    /// An in-process client over an engine the CALLER holds, so a suite that has to
    /// inspect or seed this service's store — a record no operation of this service
    /// writes, such as a permission grant or a recorded execution — reads the SAME store
    /// the handlers do.
    pub(crate) fn with_engine(engine: Arc<engine::InMemoryEngine>) -> Self {
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here; routing and the handlers are.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body`, signed for [`REGION`], and return the HTTP status
    /// and the parsed JSON body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in_region(REGION, operation, body).await
    }

    /// Call `operation` with a credential scope naming `region`, which is the region
    /// segment of any ARN the service mints for the call.
    pub(crate) async fn call_in_region(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        self.call_signed(DEFAULT_ACCESS_KEY_ID, region, operation, body)
            .await
    }

    /// Call `operation` with a credential scope naming `access_key_id` and `region`.
    async fn call_signed(
        &self,
        access_key_id: &str,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSBillingAndCostManagementDataExports.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20261021/{region}/{SIGNING_NAME}/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|e| {
                panic!(
                    "{operation}: response body is not JSON ({e}): {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, parsed)
    }

    /// Call `operation`, requiring a 200, and return the response body.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(status, 200, "{operation}: expected HTTP 200, got {status} with {parsed}");
        parsed
    }

    /// Call `operation` in `region`, requiring a 200, and return the response body.
    async fn ok_in_region(&self, region: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in_region(region, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} in {region}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }
}

/// The wire error code of a modelled failure: awsJson carries it in `__type`, optionally
/// namespace-qualified.
pub(crate) fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("an error response must carry __type, got {body}"));
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// One row of the table: an export definition, in the members the model names.
#[derive(Debug, Clone)]
pub(crate) struct Case {
    label: String,
    name: String,
    description: Option<String>,
    query_statement: String,
    /// The catalog tables [`Case::query_statement`] reads, which are the tables whose
    /// properties the stored definition carries a configuration for.
    queried_tables: Vec<&'static str>,
    table_configurations: Option<Value>,
    bucket_owner: Option<&'static str>,
    /// `OutputType`, `Format`, `Compression`, `Overwrite` — each a published value of its
    /// own enum.
    output: (&'static str, &'static str, &'static str, &'static str),
    frequency: &'static str,
    tags: Vec<(String, String)>,
}

impl Case {
    /// The `Export` structure this case submits.
    fn export(&self) -> Value {
        let mut export = Map::new();
        export.insert("Name".to_string(), json!(self.name));
        if let Some(description) = &self.description {
            export.insert("Description".to_string(), json!(description));
        }
        let mut data_query = Map::new();
        data_query.insert("QueryStatement".to_string(), json!(self.query_statement));
        if let Some(table_configurations) = &self.table_configurations {
            data_query.insert(
                "TableConfigurations".to_string(),
                table_configurations.clone(),
            );
        }
        export.insert("DataQuery".to_string(), Value::Object(data_query));

        let (output_type, format, compression, overwrite) = self.output;
        let mut s3 = Map::new();
        s3.insert("S3Bucket".to_string(), json!(format!("{}-bucket", self.name)));
        if let Some(owner) = self.bucket_owner {
            s3.insert("S3BucketOwner".to_string(), json!(owner));
        }
        s3.insert("S3Prefix".to_string(), json!("exports/daily"));
        s3.insert("S3Region".to_string(), json!(REGION));
        s3.insert(
            "S3OutputConfigurations".to_string(),
            json!({
                "OutputType": output_type,
                "Format": format,
                "Compression": compression,
                "Overwrite": overwrite,
            }),
        );
        export.insert(
            "DestinationConfigurations".to_string(),
            json!({ "S3Destination": Value::Object(s3) }),
        );
        export.insert(
            "RefreshCadence".to_string(),
            json!({ "Frequency": self.frequency }),
        );
        Value::Object(export)
    }

    /// The `ResourceTags` this case submits, if any.
    fn resource_tags(&self) -> Value {
        Value::Array(
            self.tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        )
    }

    pub(crate) fn create_request(&self) -> Value {
        json!({ "Export": self.export(), "ResourceTags": self.resource_tags() })
    }

    /// Give this row another export name, so one row can be created more than once.
    pub(crate) fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    /// How many tags this row submits with its create.
    pub(crate) fn tag_count(&self) -> usize {
        self.tags.len()
    }

    /// The catalog tables this row's query reads.
    pub(crate) fn queried_tables(&self) -> &[&'static str] {
        &self.queried_tables
    }

    /// The `Export` structure an UPDATE submits: as [`Case::export`], plus a
    /// `TableConfigurations` entry for every queried table that publishes table
    /// properties.
    ///
    /// `UpdateExport` "[updates] an existing data export by overwriting all export
    /// parameters. All export parameters must be specified in the UpdateExport request"
    /// (smithy_query operation `UpdateExport`), so an update does not inherit the stored
    /// definition's per-table properties and must restate them.
    pub(crate) fn update_export(&self) -> Value {
        let mut export = self.export();
        let data_query = export
            .as_object_mut()
            .expect("the Export structure is a JSON object")
            .get_mut("DataQuery")
            .and_then(Value::as_object_mut)
            .expect("the DataQuery structure is a JSON object");
        if let Some(restated) =
            restated_table_configurations(&self.queried_tables, self.table_configurations.as_ref())
        {
            data_query.insert("TableConfigurations".to_string(), restated);
        }
        export
    }

    /// The `Export` every reader must report for this case: what it submitted, plus the
    /// server-assigned `ExportArn`, and with `DataQuery.TableConfigurations` at the
    /// configuration IN FORCE.
    fn stored_export(&self, arn: &str) -> Value {
        stored_definition(&self.export(), arn, &self.queried_tables)
    }
}

/// `export` as a reader reports it once stored under `arn`: the submitted definition with
/// the server-assigned `ExportArn` and `DataQuery.TableConfigurations` at the
/// configuration IN FORCE for the tables its query names.
pub(crate) fn stored_definition(
    export: &Value,
    arn: &str,
    queried_tables: &[&'static str],
) -> Value {
    let mut stored = export.clone();
    let object = stored
        .as_object_mut()
        .expect("the Export structure is a JSON object");
    object.insert("ExportArn".to_string(), json!(arn));
    let submitted = object
        .get("DataQuery")
        .and_then(|query| query.get("TableConfigurations"))
        .cloned();
    let data_query = object
        .get_mut("DataQuery")
        .and_then(Value::as_object_mut)
        .expect("the DataQuery structure is a JSON object");
    match effective_table_configurations(queried_tables, submitted.as_ref()) {
        Some(effective) => {
            data_query.insert("TableConfigurations".to_string(), effective);
        }
        None => {
            data_query.remove("TableConfigurations");
        }
    }
    stored
}

/// The table configuration IN FORCE for a definition whose query names `tables` and which
/// submitted `requested`: for every one of those tables, and every table the request
/// configures, each property that table publishes — at the requested value where the
/// request names it and at the published default otherwise. `None` when no named table
/// publishes a property.
///
/// Computed from the service's own published catalog ([`TABLE_CATALOG`]) rather than
/// restated here, so the expectation covers every table and property the catalog carries
/// — including ones no row of this table names.
pub(crate) fn effective_table_configurations(
    tables: &[&'static str],
    requested: Option<&Value>,
) -> Option<Value> {
    let mut configured: Vec<&str> = tables.to_vec();
    for name in requested
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.keys())
    {
        if !configured.contains(&name.as_str()) {
            configured.push(name);
        }
    }

    let mut effective = Map::new();
    for table in configured {
        let spec = TABLE_CATALOG
            .iter()
            .find(|candidate| candidate.name == table)
            .unwrap_or_else(|| panic!("{table} is not a published catalog table"));
        if spec.properties.is_empty() {
            continue;
        }
        let mut properties = Map::new();
        for property in spec.properties {
            // A property the catalog publishes no default for assumes nothing: it is in
            // force only for a request that names it, which the loop below adds.
            if let Some(default) = property.default_value() {
                properties.insert(property.name.to_string(), json!(default));
            }
        }
        for (name, value) in requested
            .and_then(|configs| configs.get(table))
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            properties.insert(name.clone(), value.clone());
        }
        effective.insert(table.to_string(), Value::Object(properties));
    }
    (!effective.is_empty()).then_some(Value::Object(effective))
}

/// The `TableConfigurations` an UPDATE has to restate for a definition whose query names
/// `tables` and which submitted `requested`: the submitted entry for each table that
/// publishes any table property, and an empty entry for one the request left unsaid.
/// `None` when no named table publishes a property, so there is nothing to restate.
///
/// Which tables publish properties is read from the service's own published catalog
/// ([`TABLE_CATALOG`]) rather than restated here, so a table the rows never name is
/// covered too.
pub(crate) fn restated_table_configurations(
    tables: &[&'static str],
    requested: Option<&Value>,
) -> Option<Value> {
    let mut restated = requested
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for table in tables {
        let spec = TABLE_CATALOG
            .iter()
            .find(|candidate| candidate.name == *table)
            .unwrap_or_else(|| panic!("{table} is not a published catalog table"));
        if spec.properties.is_empty() || restated.contains_key(*table) {
            continue;
        }
        restated.insert((*table).to_string(), json!({}));
    }
    (!restated.is_empty()).then_some(Value::Object(restated))
}

/// The table: one row per shape of export definition the rule has to hold for.
///
/// The enum rows walk the published value lists the service's own generated catalogs
/// carry (`COMPRESSION_OPTION_VALUES`, `FORMAT_OPTION_VALUES`, `S3_OUTPUT_TYPE_VALUES`,
/// `OVERWRITE_OPTION_VALUES`, `FREQUENCY_OPTION_VALUES` in `ops::types::shared_types`),
/// so a value the issue never cited is covered too.
pub(crate) fn cases() -> Vec<Case> {
    let base_output = (
        S3_OUTPUT_TYPE_VALUES[0],
        FORMAT_OPTION_VALUES[0],
        COMPRESSION_OPTION_VALUES[0],
        OVERWRITE_OPTION_VALUES[0],
    );
    let frequency = FREQUENCY_OPTION_VALUES[0];

    let mut cases = vec![
        // The leanest definition the model allows: no Description, no
        // TableConfigurations, no S3BucketOwner, no tags.
        Case {
            label: "minimal".to_string(),
            name: "minimal_export".to_string(),
            description: None,
            query_statement: "SELECT line_item_usage_account_id FROM COST_AND_USAGE_REPORT"
                .to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            table_configurations: None,
            bucket_owner: None,
            output: base_output,
            frequency,
            tags: Vec::new(),
        },
        // Every optional member present, several tags, a query configuration naming each
        // property of the queried table at a value that is not its default.
        Case {
            label: "fully-populated".to_string(),
            name: "Fully-Populated_0".to_string(),
            description: Some("every optional member present".to_string()),
            query_statement: "SELECT bill_payer_account_id, line_item_unblended_cost FROM COST_AND_USAGE_REPORT WHERE line_item_usage_start_date > '2026-01-01'".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            table_configurations: Some(json!({
                "COST_AND_USAGE_REPORT": {
                    "TIME_GRANULARITY": "DAILY",
                    "INCLUDE_RESOURCES": "TRUE",
                    "INCLUDE_SPLIT_COST_ALLOCATION_DATA": "TRUE",
                    "INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY": "TRUE",
                },
            })),
            bucket_owner: Some("123456789012"),
            output: base_output,
            frequency,
            tags: vec![
                ("env".to_string(), "dev".to_string()),
                ("owner".to_string(), "finops".to_string()),
                // The model allows an empty tag VALUE (ResourceTagValue len 0..256).
                ("note".to_string(), String::new()),
            ],
        },
        // A query on another catalog table, with that table's own property configured.
        Case {
            label: "recommendations table".to_string(),
            name: "recommendations_export".to_string(),
            description: Some("a query on a second catalog table".to_string()),
            query_statement:
                "SELECT recommendation_id FROM COST_OPTIMIZATION_RECOMMENDATIONS".to_string(),
            queried_tables: vec!["COST_OPTIMIZATION_RECOMMENDATIONS"],
            table_configurations: Some(json!({
                "COST_OPTIMIZATION_RECOMMENDATIONS": { "INCLUDE_ALL_RECOMMENDATIONS": "TRUE" },
            })),
            bucket_owner: None,
            output: base_output,
            frequency,
            tags: vec![("team".to_string(), "billing".to_string())],
        },
        // A query on a table whose only published property is the free-form billing
        // view, so the stored configuration is that one property at its default.
        Case {
            label: "table with one free-form property".to_string(),
            name: "focus_export".to_string(),
            description: None,
            query_statement: "SELECT BilledCost, ChargePeriodStart FROM FOCUS_1_0_AWS".to_string(),
            queried_tables: vec!["FOCUS_1_0_AWS"],
            table_configurations: None,
            bucket_owner: None,
            output: base_output,
            frequency,
            tags: Vec::new(),
        },
        // The published bounds of Export.Name's length constraint (1..128).
        Case {
            label: "name at the minimum length".to_string(),
            name: "a".to_string(),
            description: Some(String::new()),
            query_statement: "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            table_configurations: Some(json!({})),
            bucket_owner: None,
            output: base_output,
            frequency,
            tags: vec![("single".to_string(), "tag".to_string())],
        },
        Case {
            label: "name at the maximum length".to_string(),
            name: "N".repeat(NAME_MAX_LEN),
            description: Some("x".repeat(1024)),
            query_statement: "SELECT product_region_code FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            table_configurations: None,
            bucket_owner: Some("000000000000"),
            output: base_output,
            frequency,
            tags: Vec::new(),
        },
    ];

    // One row per published value of each S3OutputConfigurations enum, so each modelled
    // value round-trips and not just the one the issue used.
    for (index, compression) in COMPRESSION_OPTION_VALUES.iter().enumerate() {
        cases.push(Case {
            label: format!("compression {compression}"),
            name: format!("compression-{index}"),
            description: None,
            query_statement: "SELECT line_item_product_code FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            table_configurations: None,
            bucket_owner: None,
            output: (
                S3_OUTPUT_TYPE_VALUES[index % S3_OUTPUT_TYPE_VALUES.len()],
                FORMAT_OPTION_VALUES[index % FORMAT_OPTION_VALUES.len()],
                compression,
                OVERWRITE_OPTION_VALUES[index % OVERWRITE_OPTION_VALUES.len()],
            ),
            frequency,
            tags: vec![(format!("key-{index}"), format!("value-{index}"))],
        });
    }
    for (index, output_type) in S3_OUTPUT_TYPE_VALUES.iter().enumerate() {
        cases.push(Case {
            label: format!("output type {output_type}"),
            name: format!("output_type_{index}"),
            description: Some(format!("output type {output_type}")),
            query_statement: "SELECT line_item_operation FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            table_configurations: None,
            bucket_owner: None,
            output: (
                output_type,
                FORMAT_OPTION_VALUES[index % FORMAT_OPTION_VALUES.len()],
                COMPRESSION_OPTION_VALUES[index % COMPRESSION_OPTION_VALUES.len()],
                OVERWRITE_OPTION_VALUES[index % OVERWRITE_OPTION_VALUES.len()],
            ),
            frequency,
            tags: Vec::new(),
        });
    }
    cases
}

/// Check `arn` against the modelled `Arn` constraints — `length 20..2048` and
/// `pattern ^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`
/// (smithy_query shape `Export`, member `ExportArn`) — piecewise, and that its resource
/// portion is `expected_resource`.
fn assert_modelled_arn(case: &str, arn: &str, expected_resource: &str) {
    assert!(
        (20..=2048).contains(&arn.len()),
        "case {case:?}: expected an ARN of length 20..2048, got {} in {arn:?}",
        arn.len()
    );
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected six colon-separated ARN fields, got {arn:?}"
    );
    assert_eq!(fields[0], "arn", "case {case:?}: got {arn:?}");
    let partition_suffix = fields[1]
        .strip_prefix("aws")
        .unwrap_or_else(|| panic!("case {case:?}: partition must start with aws, got {arn:?}"));
    assert!(
        partition_suffix.chars().all(is_arn_lower_segment_char),
        "case {case:?}: partition must match aws[-a-z0-9]*, got {arn:?}"
    );
    assert_eq!(
        fields[2], SIGNING_NAME,
        "case {case:?}: expected the bcm-data-exports namespace, got {arn:?}"
    );
    assert!(
        fields[3].chars().all(is_arn_lower_segment_char),
        "case {case:?}: region must match [-a-z0-9]*, got {arn:?}"
    );
    assert!(
        fields[4].len() == 12 && fields[4].chars().all(|c| c.is_ascii_digit()),
        "case {case:?}: account must match [0-9]{{12}}, got {arn:?}"
    );
    assert!(
        fields[5]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | ':' | '_')),
        "case {case:?}: resource must match [-a-zA-Z0-9/:_]+, got {arn:?}"
    );
    assert_eq!(
        fields[5], expected_resource,
        "case {case:?}: unexpected ARN resource in {arn:?}"
    );
}

fn is_arn_lower_segment_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

/// A timestamp member of `ExportStatus`, as a comparable instant.
///
/// Every `ExportStatus` timestamp is modelled `@timestampFormat("date-time")`, so the ONLY
/// wire form accepted here is the RFC 3339 string — an epoch number, which is what the
/// protocol default would write, is a failure. The form itself is covered row by row in
/// `status_timestamp_wire_form.rs`; this helper refuses the wrong one so a suite reading an
/// instant cannot pass on it.
///
/// `pub(crate)` so the wire-form suite reads an instant through the same reader.
pub(crate) fn timestamp(case: &str, status: &Value, member: &str) -> DateTime {
    let raw = status
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: ExportStatus.{member} is absent from {status}"));
    let Value::String(text) = raw else {
        panic!(
            "case {case:?}: ExportStatus.{member} is @timestampFormat(\"date-time\"), so it must \
             be an RFC 3339 string, got {raw}"
        )
    };
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case:?}: ExportStatus.{member} is not an RFC 3339 instant ({e}): {text}")
    })
}

fn as_pair(instant: DateTime) -> (i64, u32) {
    (instant.secs(), instant.subsec_nanos())
}

/// The whole lifecycle, once per table row: create, read back through every reader,
/// update, read back again, delete, and confirm the delete is observable.
#[tokio::test]
async fn an_export_round_trips_through_persisted_state() {
    for case in cases() {
        let client = Client::new();
        let label = case.label.clone();

        // CREATE — the ARN is server-minted and satisfies the modelled Arn pattern.
        let created = client.ok("CreateExport", &case.create_request()).await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreateExport returned no ExportArn: {created}"))
            .to_string();
        assert_modelled_arn(&label, &arn, &format!("export/{}", case.name));

        // GET — every submitted member comes back, plus the server-assigned ExportArn,
        // with the submitted DataQuery reported exactly as it was stored.
        let fetched = client
            .ok("GetExport", &json!({ "ExportArn": arn }))
            .await;
        let expected_export = case.stored_export(&arn);
        assert_eq!(
            fetched["Export"], expected_export,
            "case {label:?}: GetExport must return what CreateExport stored, got {}",
            fetched["Export"]
        );
        let status = fetched["ExportStatus"].clone();
        assert_eq!(
            status["StatusCode"],
            json!("HEALTHY"),
            "case {label:?}: expected a HEALTHY export, got {status}"
        );
        let created_at = timestamp(&label, &status, "CreatedAt");
        let first_updated_at = timestamp(&label, &status, "LastUpdatedAt");
        assert!(
            as_pair(first_updated_at) >= as_pair(created_at),
            "case {label:?}: expected LastUpdatedAt >= CreatedAt, got {status}"
        );
        // The service reports the instant it recorded to the second, carrying no
        // sub-second part.
        for member in ["CreatedAt", "LastUpdatedAt"] {
            assert_eq!(
                timestamp(&label, &status, member).subsec_nanos(),
                0,
                "case {label:?}: expected a whole-second ExportStatus.{member}, got {status}"
            );
        }

        // A read must not change what a later reader sees.
        let fetched_again = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            fetched, fetched_again,
            "case {label:?}: GetExport is a reporting operation and must answer identically"
        );

        // LIST — the export is present exactly once, as an ExportReference.
        let listed = client.ok("ListExports", &json!({})).await;
        let exports = listed["Exports"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: ListExports returned no Exports: {listed}"));
        assert_eq!(
            exports.len(),
            1,
            "case {label:?}: expected exactly the one created export, got {listed}"
        );
        assert_eq!(
            exports[0],
            json!({
                "ExportArn": arn,
                "ExportName": case.name,
                "ExportStatus": status,
            }),
            "case {label:?}: unexpected ExportReference"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {label:?}: a last page must omit NextToken, got {listed}"
        );

        // TAGS — exactly the tags CreateExport was given, keyed uniquely.
        let tagged = client
            .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
            .await;
        let mut expected_tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
        expected_tags.sort_by_key(|tag| tag["Key"].as_str().unwrap_or_default().to_string());
        assert_eq!(
            tagged["ResourceTags"],
            Value::Array(expected_tags),
            "case {label:?}: ListTagsForResource must report the tags CreateExport persisted"
        );

        // UPDATE — every settable parameter is overwritten.
        let updated_case = Case {
            description: Some(format!("updated {}", case.label)),
            query_statement: "SELECT line_item_resource_id FROM COST_AND_USAGE_REPORT".to_string(),
            queried_tables: vec!["COST_AND_USAGE_REPORT"],
            table_configurations: Some(json!({
                "COST_AND_USAGE_REPORT": { "TIME_GRANULARITY": "MONTHLY" }
            })),
            bucket_owner: Some("210987654321"),
            output: (
                S3_OUTPUT_TYPE_VALUES[S3_OUTPUT_TYPE_VALUES.len() - 1],
                FORMAT_OPTION_VALUES[FORMAT_OPTION_VALUES.len() - 1],
                COMPRESSION_OPTION_VALUES[COMPRESSION_OPTION_VALUES.len() - 1],
                OVERWRITE_OPTION_VALUES[OVERWRITE_OPTION_VALUES.len() - 1],
            ),
            ..case.clone()
        };
        let update_body = updated_case.update_export();
        let update_response = client
            .ok(
                "UpdateExport",
                &json!({ "ExportArn": arn, "Export": update_body.clone() }),
            )
            .await;
        assert_eq!(
            update_response["ExportArn"],
            json!(arn),
            "case {label:?}: UpdateExport must return the ARN it updated"
        );

        let after_update = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        let expected_updated =
            stored_definition(&update_body, &arn, updated_case.queried_tables());
        assert_eq!(
            after_update["Export"], expected_updated,
            "case {label:?}: GetExport must observe the update, got {}",
            after_update["Export"]
        );
        let updated_status = after_update["ExportStatus"].clone();
        assert_eq!(
            as_pair(timestamp(&label, &updated_status, "CreatedAt")),
            as_pair(created_at),
            "case {label:?}: an update must not move CreatedAt"
        );
        // At or after, not strictly after: instants are recorded to the second, and these
        // rows do not wait for a second boundary. The STRICT advance is pinned by
        // `an_update_moves_last_updated_at_and_leaves_created_at`, which does wait for
        // one — do not relax that test to match this row.
        assert!(
            as_pair(timestamp(&label, &updated_status, "LastUpdatedAt")) >= as_pair(created_at),
            "case {label:?}: expected LastUpdatedAt at or after CreatedAt, got {updated_status}"
        );
        // The tags are untouched by an export update.
        let tagged_after_update = client
            .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            tagged_after_update["ResourceTags"], tagged["ResourceTags"],
            "case {label:?}: UpdateExport must not change the export's tags"
        );

        // DELETE — and the deletion is observable by every reader.
        let deleted = client.ok("DeleteExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            deleted["ExportArn"],
            json!(arn),
            "case {label:?}: DeleteExport must return the ARN it removed"
        );
        let (status_code, body) = client.call("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            (status_code, error_code(&body).as_str()),
            (NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {label:?}: GetExport after a delete must be the not-found refusal, got {body}"
        );
        let listed_after_delete = client.ok("ListExports", &json!({})).await;
        assert_eq!(
            listed_after_delete["Exports"],
            json!([]),
            "case {label:?}: a deleted export must not be listed"
        );

        // The name is reusable once the export holding it is gone.
        let recreated = client.ok("CreateExport", &case.create_request()).await;
        assert_eq!(
            recreated["ExportArn"],
            json!(arn),
            "case {label:?}: recreating a deleted export must mint the same name-derived ARN"
        );
    }
}

/// `UpdateExport` moves `LastUpdatedAt` to the instant of the update and leaves
/// `CreatedAt` where the create put it.
///
/// Its own test rather than a step of the table above: the service records instants at
/// whole-second precision, so an advance is only observable across a second boundary and
/// this is the one case that has to wait for one. A blocking sleep rather than
/// `tokio::time::sleep`: the workspace's tokio carries only the `macros`/`rt` features,
/// so there is no time driver to await on.
#[tokio::test]
async fn an_update_moves_last_updated_at_and_leaves_created_at() {
    let client = Client::new();
    let case = cases().remove(0);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let arn = created["ExportArn"].as_str().expect("an ExportArn").to_string();

    let before = client.ok("GetExport", &json!({ "ExportArn": arn })).await["ExportStatus"].clone();
    let created_at = timestamp("before the update", &before, "CreatedAt");

    std::thread::sleep(Duration::from_millis(1_100));
    client
        .ok(
            "UpdateExport",
            &json!({ "ExportArn": arn, "Export": case.update_export() }),
        )
        .await;

    let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await["ExportStatus"].clone();
    assert_eq!(
        as_pair(timestamp("after the update", &after, "CreatedAt")),
        as_pair(created_at),
        "an update must not move CreatedAt, got {after} after {before}"
    );
    assert!(
        as_pair(timestamp("after the update", &after, "LastUpdatedAt")) > as_pair(created_at),
        "an update a second later must advance LastUpdatedAt, got {after} after {before}"
    );
    // A read does not move it either: the next reader sees the same instant.
    let again = client.ok("GetExport", &json!({ "ExportArn": arn })).await["ExportStatus"].clone();
    assert_eq!(
        again, after,
        "reading an export must not move its timestamps"
    );
}

/// `ListExports` pages over the persisted exports: every page size walks each export
/// exactly once, in one stable order, and the final page carries no token.
#[tokio::test]
async fn list_exports_pages_over_every_persisted_export() {
    const TOTAL: usize = 7;
    let client = Client::new();
    let template = cases().remove(0);

    let mut created: Vec<String> = Vec::new();
    for index in 0..TOTAL {
        let case = Case {
            name: format!("paged_{index:02}"),
            ..template.clone()
        };
        let response = client.ok("CreateExport", &case.create_request()).await;
        created.push(
            response["ExportArn"]
                .as_str()
                .expect("CreateExport returns an ExportArn")
                .to_string(),
        );
    }
    created.sort();

    // MaxResults is modelled `range 1..300`; walk every page size up to one past the
    // number of exports, plus the unpaged call.
    for max_results in [None, Some(1), Some(2), Some(3), Some(TOTAL), Some(TOTAL + 1)] {
        let case = format!("MaxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= TOTAL + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            let mut request = Map::new();
            if let Some(max) = max_results {
                request.insert("MaxResults".to_string(), json!(max));
            }
            if let Some(token) = &token {
                request.insert("NextToken".to_string(), json!(token));
            }
            let response = client.ok("ListExports", &Value::Object(request)).await;
            let page = response["Exports"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no Exports in {response}"))
                .clone();
            if let Some(max) = max_results {
                assert!(
                    page.len() <= max,
                    "case {case:?}: a page carried {} items, more than MaxResults",
                    page.len()
                );
            }
            for reference in &page {
                seen.push(
                    reference["ExportArn"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: no ExportArn in {reference}"))
                        .to_string(),
                );
            }
            match response.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case {case:?}: paging must walk every export exactly once, in ARN order"
        );
    }

    // A token this service never issued is a refusal, not a silent empty page. Each row
    // is a different way of not being an issued token: not hex, hex but too short to
    // carry the integrity tag, hex of a plausible ARN with no tag, and an issued token
    // with its tag corrupted.
    let issued = client
        .ok("ListExports", &json!({ "MaxResults": 1 }))
        .await["NextToken"]
        .as_str()
        .expect("a first page must carry a NextToken")
        .to_string();
    let mut corrupted = issued.clone();
    corrupted.replace_range(0..1, if issued.starts_with('a') { "b" } else { "a" });
    for forged in [
        "not-a-token".to_string(),
        hex::encode("x"),
        hex::encode(&created[0]),
        corrupted,
        issued[..issued.len() - 2].to_string(),
    ] {
        let (status, body) = client
            .call("ListExports", &json!({ "NextToken": forged }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {forged:?}: an unissued NextToken must be refused, got {body}"
        );
    }

    // And the token the service DID issue is honoured.
    let resumed = client
        .ok("ListExports", &json!({ "NextToken": issued }))
        .await;
    assert_eq!(
        resumed["Exports"].as_array().expect("Exports").len(),
        TOTAL - 1,
        "an issued token must resume after the page it ended, got {resumed}"
    );
}

/// A page token is bound to the LISTING that issued it as well as to its position.
///
/// The two paginated listings page over different key spaces (`ListExports` over export
/// ARNs, `ListTagsForResource` over one export's tag keys), so a token handed to the other
/// listing names a position in a sequence it was never cut from: it must be refused, not
/// resumed at an unrelated offset. Asserted in both directions, and paired with the
/// round trip that each listing honours its OWN token, so the refusal is about the
/// binding and not about tokens being rejected wholesale.
#[tokio::test]
async fn a_page_token_is_not_accepted_by_the_other_listing() {
    let client = Client::new();

    // Two exports, so ListExports has a second page; the first carries several tags, so
    // ListTagsForResource does too.
    let tagged = cases()
        .into_iter()
        .find(|case| case.tags.len() > 1)
        .expect("a table row with more than one tag");
    let second = Case {
        name: "second_export".to_string(),
        ..tagged.clone()
    };
    let owner_arn = client.ok("CreateExport", &tagged.create_request()).await["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    client.ok("CreateExport", &second.create_request()).await;

    let exports_token = client
        .ok("ListExports", &json!({ "MaxResults": 1 }))
        .await["NextToken"]
        .as_str()
        .expect("a first page over two exports must carry a NextToken")
        .to_string();
    let tags_token = client
        .ok(
            "ListTagsForResource",
            &json!({ "ResourceArn": owner_arn, "MaxResults": 1 }),
        )
        .await["NextToken"]
        .as_str()
        .expect("a first page over several tags must carry a NextToken")
        .to_string();

    for (listing, foreign) in [
        ("ListExports", tags_token.clone()),
        ("ListTagsForResource", exports_token.clone()),
    ] {
        let mut request = Map::new();
        request.insert("NextToken".to_string(), json!(foreign));
        if listing == "ListTagsForResource" {
            request.insert("ResourceArn".to_string(), json!(owner_arn));
        }
        let (status, body) = client.call(listing, &Value::Object(request)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {listing:?}: a token issued by the other listing must be refused, got {body}"
        );
    }

    // Each listing still honours the token it issued itself.
    let exports_page = client
        .ok("ListExports", &json!({ "NextToken": exports_token }))
        .await;
    assert_eq!(
        exports_page["Exports"].as_array().expect("Exports").len(),
        1,
        "the ListExports token must resume after its own page, got {exports_page}"
    );
    let tags_page = client
        .ok(
            "ListTagsForResource",
            &json!({ "ResourceArn": owner_arn, "NextToken": tags_token }),
        )
        .await;
    assert_eq!(
        tags_page["ResourceTags"]
            .as_array()
            .expect("ResourceTags")
            .len(),
        tagged.tags.len() - 1,
        "the ListTagsForResource token must resume after its own page, got {tags_page}"
    );
}

/// An export is identified by its ARN, and the ARN carries the region of the call that
/// created it — so one name is one export PER REGION, not per account.
///
/// Asserted over several regions differing in length and in hyphen count, because the
/// rule is about the ARN's region segment rather than about any one region.
#[tokio::test]
async fn one_name_is_a_distinct_export_in_each_region() {
    let client = Client::new();
    let case = Case {
        name: "shared_name".to_string(),
        ..cases().remove(0)
    };

    // Regions differing in length and in how many hyphens they carry; all are legal
    // region segments of the modelled Arn pattern (`[-a-z0-9]*`).
    let regions = ["us-east-1", "eu-west-2", "ap-southeast-4", "us-gov-east-1"];
    let mut arns: Vec<String> = Vec::new();
    for region in regions {
        let created = client
            .ok_in_region(region, "CreateExport", &case.create_request())
            .await;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: CreateExport returns an ExportArn"))
            .to_string();
        assert_modelled_arn(region, &arn, &format!("export/{}", case.name));
        assert_eq!(
            arn.split(':').nth(3),
            Some(region),
            "case {region:?}: the ARN must carry the call's region, got {arn:?}"
        );
        assert!(
            !arns.contains(&arn),
            "case {region:?}: the same name in a new region must mint a new ARN, got {arn:?}"
        );
        arns.push(arn);
    }

    // Each one is its own export, readable under its own ARN, and all of them are listed.
    for (region, arn) in regions.iter().zip(&arns) {
        let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            fetched["Export"]["ExportArn"],
            json!(arn),
            "case {region:?}: GetExport must report the ARN it was asked for, got {fetched}"
        );
        assert_eq!(
            fetched["Export"]["Name"],
            json!(case.name),
            "case {region:?}: GetExport must report the created name, got {fetched}"
        );
    }
    let listed = client.ok("ListExports", &json!({})).await;
    assert_eq!(
        listed["Exports"].as_array().expect("Exports").len(),
        regions.len(),
        "every per-region export must be listed, got {listed}"
    );

    // Within ONE region the name is taken, and deleting that one frees it again without
    // touching the others.
    let (status, body) = client
        .call_in_region(regions[0], "CreateExport", &case.create_request())
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {:?}: the name is already taken in this region, got {body}",
        regions[0]
    );
    client
        .ok("DeleteExport", &json!({ "ExportArn": arns[0] }))
        .await;
    let recreated = client
        .ok_in_region(regions[0], "CreateExport", &case.create_request())
        .await;
    assert_eq!(
        recreated["ExportArn"],
        json!(arns[0]),
        "after the delete the name is free again in that region, got {recreated}"
    );
    for (region, arn) in regions.iter().zip(&arns).skip(1) {
        let (status, _) = client.call("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {region:?}: a delete in another region must not remove this export"
        );
    }
}

/// The refusals the lifecycle owes a client, over the inputs that trigger them.
#[tokio::test]
async fn unknown_arns_and_duplicate_names_are_refused() {
    let client = Client::new();
    let case = cases().remove(0);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let arn = created["ExportArn"].as_str().expect("an ExportArn").to_string();

    // A name already in use cannot be created again.
    let (status, body) = client.call("CreateExport", &case.create_request()).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a duplicate Export.Name must be refused, got {body}"
    );

    // A tag key given twice names two values for one tag.
    let mut duplicate_tags = case.clone();
    duplicate_tags.name = "duplicate_tag_keys".to_string();
    duplicate_tags.tags = vec![
        ("env".to_string(), "dev".to_string()),
        ("env".to_string(), "prod".to_string()),
    ];
    let (status, body) = client
        .call("CreateExport", &duplicate_tags.create_request())
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a repeated tag key must be refused, got {body}"
    );

    // Every ARN-addressed operation answers the not-found refusal for an ARN that names
    // nothing. The absent ARNs still satisfy the modelled Arn pattern, so the refusal
    // comes from the handler and not from boundary validation.
    let absent = [
        engine::emulator_arn(SIGNING_NAME, REGION, "export/never_created"),
        engine::emulator_arn(SIGNING_NAME, "eu-west-1", &format!("export/{}", case.name)),
        engine::emulator_arn(SIGNING_NAME, REGION, "export/a"),
    ];
    for arn in &absent {
        for (operation, request) in [
            ("GetExport", json!({ "ExportArn": arn })),
            ("DeleteExport", json!({ "ExportArn": arn })),
            (
                "UpdateExport",
                json!({ "ExportArn": arn, "Export": case.export() }),
            ),
            ("ListTagsForResource", json!({ "ResourceArn": arn })),
            (
                "TagResource",
                json!({ "ResourceArn": arn, "ResourceTags": [{ "Key": "k", "Value": "v" }] }),
            ),
            (
                "UntagResource",
                json!({ "ResourceArn": arn, "ResourceTagKeys": ["k"] }),
            ),
        ] {
            let (status, body) = client.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (NOT_FOUND_STATUS, "ResourceNotFoundException"),
                "case {operation:?} on {arn:?}: expected the not-found refusal, got {body}"
            );
            assert_eq!(
                body["ResourceId"],
                json!(arn),
                "case {operation:?} on {arn:?}: the refusal must name the ARN it could not find"
            );
        }
    }

    // An export's name is part of its ARN, so UpdateExport cannot rename it.
    let renamed = Case {
        name: "renamed_export".to_string(),
        ..case.clone()
    };
    let (status, body) = client
        .call(
            "UpdateExport",
            &json!({ "ExportArn": arn, "Export": renamed.export() }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "renaming an export through UpdateExport must be refused, got {body}"
    );

    // Deleting twice: the second call finds nothing to delete.
    client.ok("DeleteExport", &json!({ "ExportArn": arn })).await;
    let (status, body) = client.call("DeleteExport", &json!({ "ExportArn": arn })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "a second DeleteExport must be the not-found refusal, got {body}"
    );
}

/// Several exports coexist: each reader answers about the one its ARN names, and a write
/// to one is invisible in the others.
#[tokio::test]
async fn exports_are_isolated_from_one_another() {
    let client = Client::new();
    let table = cases();
    let mut arns: Vec<(String, Case)> = Vec::new();
    for case in table.iter().take(4) {
        let response = client.ok("CreateExport", &case.create_request()).await;
        arns.push((
            response["ExportArn"]
                .as_str()
                .expect("an ExportArn")
                .to_string(),
            case.clone(),
        ));
    }

    for (arn, case) in &arns {
        let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
        assert_eq!(
            fetched["Export"]["Name"],
            json!(case.name),
            "case {:?}: GetExport answered about another export: {fetched}",
            case.label
        );
    }

    // Delete one; the others are untouched.
    let (removed_arn, removed_case) = arns.remove(1);
    client
        .ok("DeleteExport", &json!({ "ExportArn": removed_arn }))
        .await;
    let listed = client.ok("ListExports", &json!({})).await;
    let names: Vec<String> = listed["Exports"]
        .as_array()
        .expect("Exports")
        .iter()
        .map(|reference| reference["ExportName"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !names.contains(&removed_case.name),
        "case {:?}: a deleted export must be gone, got {names:?}",
        removed_case.label
    );
    for (_, case) in &arns {
        assert!(
            names.contains(&case.name),
            "case {:?}: deleting another export must not remove it, got {names:?}",
            case.label
        );
    }
}

/// The store is bounded: once it holds the maximum number of exports, a further
/// `CreateExport` is refused with the modelled `ServiceQuotaExceededException` (HTTP 402)
/// and the store does not grow, while a name freed by a delete can be created again.
#[tokio::test]
async fn creating_past_the_export_limit_is_refused_with_the_quota_error() {
    let client = Client::new();
    let template = cases().remove(0);
    let at_limit = |index: usize| Case {
        name: format!("bounded_{index:04}"),
        ..template.clone()
    };

    for index in 0..MAX_EXPORTS {
        client
            .ok("CreateExport", &at_limit(index).create_request())
            .await;
    }

    let (status, body) = client
        .call("CreateExport", &at_limit(MAX_EXPORTS).create_request())
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (402, "ServiceQuotaExceededException"),
        "case {MAX_EXPORTS}: creating past the limit must report the quota error, got {body}"
    );
    assert_eq!(
        body["QuotaCode"],
        json!("ExportsPerAccount"),
        "the quota error must name the quota it enforces, got {body}"
    );

    // A full store does not turn a bad request into a quota failure: a name already in
    // use is still owed the 400.
    let (status, body) = client
        .call("CreateExport", &at_limit(0).create_request())
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a duplicate name must be refused as invalid even when the store is full, got {body}"
    );

    // The refusal did not store anything: the count is still the limit.
    let listed = client
        .ok("ListExports", &json!({ "MaxResults": 300 }))
        .await;
    let mut total = listed["Exports"].as_array().expect("Exports").len();
    let mut token = listed.get("NextToken").and_then(Value::as_str).map(str::to_string);
    while let Some(next) = token {
        let page = client
            .ok(
                "ListExports",
                &json!({ "MaxResults": 300, "NextToken": next }),
            )
            .await;
        total += page["Exports"].as_array().expect("Exports").len();
        token = page.get("NextToken").and_then(Value::as_str).map(str::to_string);
    }
    assert_eq!(
        total, MAX_EXPORTS,
        "a refused CreateExport must not have stored anything"
    );

    // Freeing a slot lets the next create through again.
    let freed = at_limit(0);
    let freed_arn = engine::emulator_arn(SIGNING_NAME, REGION, &format!("export/{}", freed.name));
    client
        .ok("DeleteExport", &json!({ "ExportArn": freed_arn }))
        .await;
    client
        .ok("CreateExport", &at_limit(MAX_EXPORTS).create_request())
        .await;
}
