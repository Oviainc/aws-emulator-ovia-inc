// SPDX-License-Identifier: CC0-1.0

//! What a matching job's RUN is: the records it reads, the match groups it resolves
//! them into, the output table it writes and the metrics it reports.
//!
//! Driven end to end with the emulator's own AWS Glue and Amazon S3 mounted beside
//! Entity Resolution on ONE shared engine — what `core/router` assembles — so every case
//! resolves its input table in the service that owns the catalog and reads and writes
//! its objects in the service that owns objects. Every Entity Resolution call travels
//! the real wire (`restJson1`, the modelled method and URI) through
//! `MultiServiceRouter::dispatch`, so what is asserted is what an SDK client would see.
//!
//! RULE ONE — a rule-based matching job runs: it reads the records of every input
//! source's Glue table out of that table's Amazon S3 location, gives the records that
//! agree on all of a rule's `matchingKeys` ONE `matchId` and every other group a
//! different one, writes the matched records as the output table under the job's
//! `outputS3Path` with the `matchId` and the `ruleName` that joined each record, and
//! reports `SUCCEEDED` with `JobMetrics` counting the rows it read, the records it
//! processed, the rows it could not and the groups it resolved. Connectedness is
//! transitive and a record no rule joins to another stands alone.
//!
//! RULE TWO — there is no success for work that produced nothing: a run that cannot
//! read its input table, cannot write its output or resolves in a way this emulator does
//! not run reports the terminal `FAILED` with the reason as its modelled
//! `ErrorDetails$errorMessage`, no metrics, and no object under the output prefix.
//!
//! The tables span what the rules generalise over: how many matching keys a rule
//! compares and how many columns one key spans, how many rules a workflow declares and
//! how a chain of them connects records, whether the input source asks for
//! normalization, how many objects the table's location holds and whether a row can be
//! read as a record of that table at all, how the table's header is declared (named
//! columns, a skipped header, or no column list at all), and whether an output attribute
//! is hashed. The recorded case — a four-row table matched on one email key — is one row
//! among them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call is signed for and every ARN below is scoped to.
const REGION: &str = "us-east-1";

/// The account id the emulator mints its ARNs under, which is also the `CatalogId` the
/// Glue Data Catalog of this account is held under.
const ACCOUNT_ID: &str = "000000000000";

/// The bucket every case's input objects are staged in.
const INPUT_BUCKET: &str = "er-input";

/// The bucket every workflow writes its output table to.
const OUTPUT_BUCKET: &str = "er-output";

/// The key prefix `outputSourceConfig[0].outputS3Path` names.
const OUTPUT_PREFIX: &str = "output";

/// The Glue database every input table is created in.
const DATABASE: &str = "crm";

/// How long a job may take to settle before the test gives up. The emulator's own
/// per-status span is well under a second, so this is a generous ceiling that keeps the
/// suite deterministic on a loaded machine rather than a second assertion about timing.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(15);

/// How often a job is polled while waiting for it to settle.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A SigV4 credential scope naming `service`: it is what routes a request to that
/// service. The key id and signature are not credentials — this harness turns request
/// authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The Glue table ARN an `InputSource$inputSourceARN` carries for `table`
/// (`^arn:…:glue:<region>:<account>:table/<database>/<table>$`,
/// `smithy_query(query="shape",shape="InputSource")`).
fn glue_table_arn(table: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/{DATABASE}/{table}")
}

/// The IAM role a workflow is created with.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/entity-resolution")
}

/// One Entity Resolution response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    /// A top-level string member, or `""` when the body carries none.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// Entity Resolution, Glue and Amazon S3 on ONE shared engine, the way the router
/// mounts every service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_entityresolution_awsveniceservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One Entity Resolution call, as a client makes it.
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!(
                "https://entityresolution.{REGION}.amazonaws.com{path}"
            ))
            .header("authorization", authorization("entityresolution"))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// One raw path-style request to the emulator's own Amazon S3 front door: the bucket
    /// and the key live in the URI and the body is the object's own bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", authorization("s3"))
            .body(::bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            response.into_body().as_ref().to_vec(),
        )
    }

    /// One Glue call (`awsJson1_1`, `X-Amz-Target: AWSGlue.<Operation>`), as a client
    /// makes it.
    async fn glue(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AWSGlue.{operation}"))
            .header("authorization", authorization("glue"))
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `PUT /<bucket>` — `CreateBucket`.
    async fn given_bucket(&self, bucket: &str) {
        let (status, body) = self.s3("PUT", &format!("/{bucket}"), &[]).await;
        assert_eq!(
            status,
            200,
            "the bucket {bucket} must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// `PUT /<bucket>/<key>` — `PutObject`, storing `body` as the object's own bytes.
    async fn given_object(&self, bucket: &str, key: &str, body: &str) {
        let (status, answered) = self
            .s3("PUT", &format!("/{bucket}/{key}"), body.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "the object {key} must be stored, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// `GET /<bucket>/<key>` — `GetObject` — as the text it holds, or `None` when the
    /// store does not hold it.
    async fn object_text(&self, bucket: &str, key: &str) -> Option<String> {
        let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), &[]).await;
        (status == 200).then(|| String::from_utf8_lossy(&body).to_string())
    }

    /// The keys Amazon S3 holds under `prefix`, read out of the `<Key>` elements
    /// `ListObjectsV2` answered with.
    async fn keys_under(&self, bucket: &str, prefix: &str) -> Vec<String> {
        let (status, body) = self
            .s3("GET", &format!("/{bucket}?list-type=2&prefix={prefix}"), &[])
            .await;
        let xml = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "the prefix {prefix:?} must be listable, got {status} {xml}"
        );
        xml.split("<Key>")
            .skip(1)
            .filter_map(|tail| tail.split_once("</Key>"))
            .map(|(key, _)| key.to_string())
            .collect()
    }

    /// `AWSGlue.CreateDatabase` for the one database every input table lives in.
    async fn given_database(&self) {
        let (status, body) = self
            .glue("CreateDatabase", &json!({"DatabaseInput": {"Name": DATABASE}}))
            .await;
        assert_eq!(
            status, 200,
            "the Glue database must be created, got {status} {body}"
        );
    }

    /// `AWSGlue.CreateTable` for one input table: where its objects live, which columns
    /// it declares (none, for a table whose header names them) and whether its SerDe
    /// declares a header to skip.
    async fn given_table(
        &self,
        name: &str,
        location: &str,
        columns: &[&str],
        skip_header: Option<&str>,
    ) {
        let mut storage = json!({ "Location": location });
        if !columns.is_empty() {
            storage["Columns"] = Value::Array(
                columns
                    .iter()
                    .map(|column| json!({ "Name": column, "Type": "string" }))
                    .collect(),
            );
        }
        if let Some(lines) = skip_header {
            storage["SerdeInfo"] = json!({
                "SerializationLibrary": "org.apache.hadoop.hive.serde2.OpenCSVSerde",
                "Parameters": { "skip.header.line.count": lines },
            });
        }
        let (status, body) = self
            .glue(
                "CreateTable",
                &json!({
                    "DatabaseName": DATABASE,
                    "TableInput": {
                        "Name": name,
                        "TableType": "EXTERNAL_TABLE",
                        "StorageDescriptor": storage,
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "the Glue table {name} must be created, got {status} {body}"
        );
    }

    /// `POST /schemas` — `CreateSchemaMapping`.
    async fn given_schema_mapping(&self, name: &str, fields: &Value) {
        let reply = self
            .call(
                "POST",
                "/schemas",
                Some(&json!({ "schemaName": name, "mappedInputFields": fields })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "the schema mapping {name} must be created, got {reply:?}"
        );
    }

    /// `POST /matchingworkflows` — `CreateMatchingWorkflow`.
    async fn create_workflow(&self, request: &Value) -> Reply {
        self.call("POST", "/matchingworkflows", Some(request)).await
    }

    /// `POST /matchingworkflows/{workflowName}/jobs` — `StartMatchingJob`. The operation
    /// models no body members at all, so the request carries no payload.
    async fn start_job(&self, workflow_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/matchingworkflows/{workflow_name}/jobs"),
            None,
        )
        .await
    }

    /// `GET /matchingworkflows/{workflowName}/jobs/{jobId}` — `GetMatchingJob`.
    async fn get_job(&self, workflow_name: &str, job_id: &str) -> Reply {
        self.call(
            "GET",
            &format!("/matchingworkflows/{workflow_name}/jobs/{job_id}"),
            None,
        )
        .await
    }

    /// `POST /matchingworkflows/{workflowName}/matches` — `GetMatchId`, the reader of
    /// the records a rule-based workflow has processed.
    async fn get_match_id(&self, workflow_name: &str, record: Value) -> Reply {
        self.call(
            "POST",
            &format!("/matchingworkflows/{workflow_name}/matches"),
            Some(&json!({ "record": record, "applyNormalization": false })),
        )
        .await
    }

    /// Poll `GetMatchingJob` until the job reports a terminal `JobStatus`, and answer
    /// that reply.
    async fn settle(&self, case: &str, workflow_name: &str, job_id: &str) -> Reply {
        let started = Instant::now();
        loop {
            let reply = self.get_job(workflow_name, job_id).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected HTTP 200 from GetMatchingJob, got {reply:?}"
            );
            let status = reply.text("status");
            if status == "SUCCEEDED" || status == "FAILED" {
                return reply;
            }
            assert!(
                started.elapsed() < SETTLE_TIMEOUT,
                "case {case:?}: the job never settled; last status {status:?} in {:?}",
                reply.body
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

/// A rule-based `resolutionTechniques` carrying `rules`.
fn rule_based(rules: Value) -> Value {
    json!({
        "resolutionType": "RULE_MATCHING",
        "ruleBasedProperties": {
            "rules": rules,
            "attributeMatchingModel": "ONE_TO_ONE",
        },
    })
}

/// A `CreateMatchingWorkflow` request over one input table and one output path.
fn workflow_request(
    name: &str,
    schema_name: &str,
    table: &str,
    normalize: Option<bool>,
    output: Value,
    output_s3_path: &str,
    techniques: Value,
) -> Value {
    let mut source = json!({
        "inputSourceARN": glue_table_arn(table),
        "schemaName": schema_name,
    });
    if let Some(flag) = normalize {
        source["applyNormalization"] = json!(flag);
    }
    json!({
        "workflowName": name,
        "inputSourceConfig": [source],
        "outputSourceConfig": [{ "outputS3Path": output_s3_path, "output": output }],
        "resolutionTechniques": techniques,
        "roleArn": role_arn(),
    })
}

/// What one input record must be reported as: the group it belongs to (a label local to
/// the case — records sharing a label must share one `matchId`, and two labels must
/// never share one) and the `MatchRule` the output table carries for it.
#[derive(Debug, Clone, Copy)]
struct Expected {
    /// The record's `id` column, which is how a case addresses one record.
    id: &'static str,
    group: &'static str,
    rule: &'static str,
}

/// One row of the matching table.
struct Case {
    case: &'static str,
    /// The objects staged under this case's table location: `(object name, body)`.
    objects: Vec<(&'static str, String)>,
    /// The columns the Glue table declares; empty for a table that declares none, whose
    /// header then names them.
    columns: Vec<&'static str>,
    /// The `skip.header.line.count` the table's SerDe declares, when it declares one.
    skip_header: Option<&'static str>,
    /// `CreateSchemaMapping$mappedInputFields`.
    fields: Value,
    /// `ruleBasedProperties.rules`.
    rules: Value,
    /// `InputSource$applyNormalization`.
    normalize: Option<bool>,
    /// `OutputSource$output`.
    output: Value,
    /// Every record the run must report, by its `id` column.
    expected: Vec<Expected>,
    /// `metrics.inputRecords`: every row of the table, readable or not.
    input_records: i64,
    /// `metrics.recordsNotProcessed`.
    not_processed: i64,
}

/// The schema mapping most cases use: `id` as the unique id, `email` and `phone` each
/// their own matching key.
fn id_email_phone_fields() -> Value {
    json!([
        { "fieldName": "id", "type": "UNIQUE_ID" },
        { "fieldName": "email", "type": "EMAIL_ADDRESS", "matchKey": "email" },
        { "fieldName": "phone", "type": "PHONE_NUMBER", "matchKey": "phone" },
    ])
}

/// The output attributes most cases use: the record's own `id` and `email`, unhashed.
fn id_email_output() -> Value {
    json!([{ "name": "id" }, { "name": "email" }])
}

/// The rows of the matching table.
fn matching_cases() -> Vec<Case> {
    vec![
        Case {
            case: "one rule on one matching key: the records that share it are one group",
            objects: vec![(
                "people.csv",
                "id,email,phone\n\
                 r1,ada@example.com,555-0100\n\
                 r2,ada@example.com,555-0101\n\
                 r3,bob@example.com,555-0102\n\
                 r4,cy@example.com,555-0103\n"
                    .to_string(),
            )],
            columns: vec!["id", "email", "phone"],
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }]),
            normalize: None,
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "emailRule" },
                Expected { id: "r2", group: "a", rule: "emailRule" },
                Expected { id: "r3", group: "b", rule: "" },
                Expected { id: "r4", group: "c", rule: "" },
            ],
            input_records: 4,
            not_processed: 0,
        },
        Case {
            case: "a rule on two matching keys matches only the records that agree on both",
            objects: vec![(
                "people.csv",
                "id,email,phone\n\
                 r1,ada@example.com,555-0100\n\
                 r2,ada@example.com,555-0100\n\
                 r3,ada@example.com,555-0999\n"
                    .to_string(),
            )],
            columns: vec!["id", "email", "phone"],
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "bothRule", "matchingKeys": ["email", "phone"] }]),
            normalize: None,
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "bothRule" },
                Expected { id: "r2", group: "a", rule: "bothRule" },
                Expected { id: "r3", group: "b", rule: "" },
            ],
            input_records: 3,
            not_processed: 0,
        },
        Case {
            case: "a matching key spanning two columns compares them as one value",
            objects: vec![(
                "people.csv",
                "id,first,last,email\n\
                 r1,Ada,Lovelace,a@example.com\n\
                 r2,Ada,Lovelace,b@example.com\n\
                 r3,Ada,Byron,c@example.com\n"
                    .to_string(),
            )],
            columns: vec!["id", "first", "last", "email"],
            skip_header: None,
            fields: json!([
                { "fieldName": "id", "type": "UNIQUE_ID" },
                { "fieldName": "first", "type": "NAME_FIRST", "groupName": "NAME",
                  "matchKey": "fullname" },
                { "fieldName": "last", "type": "NAME_LAST", "groupName": "NAME",
                  "matchKey": "fullname" },
                { "fieldName": "email", "type": "EMAIL_ADDRESS", "matchKey": "email" },
            ]),
            rules: json!([{ "ruleName": "nameRule", "matchingKeys": ["fullname"] }]),
            normalize: None,
            output: json!([{ "name": "id" }, { "name": "first" }, { "name": "last" }]),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "nameRule" },
                Expected { id: "r2", group: "a", rule: "nameRule" },
                Expected { id: "r3", group: "b", rule: "" },
            ],
            input_records: 3,
            not_processed: 0,
        },
        Case {
            case: "two rules: a chain of matches is ONE group, each record under the rule \
                   that joined it",
            objects: vec![(
                "people.csv",
                "id,email,phone\n\
                 r1,ada@example.com,555-0100\n\
                 r2,ada@example.com,555-0200\n\
                 r3,zoe@example.com,555-0200\n\
                 r4,quinn@example.com,555-0900\n"
                    .to_string(),
            )],
            columns: vec!["id", "email", "phone"],
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([
                { "ruleName": "emailRule", "matchingKeys": ["email"] },
                { "ruleName": "phoneRule", "matchingKeys": ["phone"] },
            ]),
            normalize: None,
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "emailRule" },
                Expected { id: "r2", group: "a", rule: "emailRule" },
                Expected { id: "r3", group: "a", rule: "phoneRule" },
                Expected { id: "r4", group: "b", rule: "" },
            ],
            input_records: 4,
            not_processed: 0,
        },
        Case {
            case: "an input source that asks for normalization matches across case and \
                   spacing",
            objects: vec![(
                "people.csv",
                "id,email,phone\n\
                 r1,Ada@Example.com,555-0100\n\
                 r2,\"  ada@example.com \",555-0200\n\
                 r3,bob@example.com,555-0300\n"
                    .to_string(),
            )],
            columns: vec!["id", "email", "phone"],
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }]),
            normalize: Some(true),
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "emailRule" },
                Expected { id: "r2", group: "a", rule: "emailRule" },
                Expected { id: "r3", group: "b", rule: "" },
            ],
            input_records: 3,
            not_processed: 0,
        },
        Case {
            case: "the same records WITHOUT normalization are three groups",
            objects: vec![(
                "people.csv",
                "id,email,phone\n\
                 r1,Ada@Example.com,555-0100\n\
                 r2,\"  ada@example.com \",555-0200\n\
                 r3,bob@example.com,555-0300\n"
                    .to_string(),
            )],
            columns: vec!["id", "email", "phone"],
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }]),
            normalize: Some(false),
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "" },
                Expected { id: "r2", group: "b", rule: "" },
                Expected { id: "r3", group: "c", rule: "" },
            ],
            input_records: 3,
            not_processed: 0,
        },
        Case {
            case: "several objects under the table's location, and a row that is not a \
                   record of that table",
            objects: vec![
                (
                    "part-1.csv",
                    "id,email,phone\n\
                     r1,ada@example.com,555-0100\n\
                     r2,ada@example.com,555-0200\n"
                        .to_string(),
                ),
                (
                    "part-2.csv",
                    "id,email,phone\n\
                     r3,bob@example.com,555-0300\n\
                     r4,only-two-fields\n"
                        .to_string(),
                ),
            ],
            columns: vec!["id", "email", "phone"],
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }]),
            normalize: None,
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "emailRule" },
                Expected { id: "r2", group: "a", rule: "emailRule" },
                Expected { id: "r3", group: "b", rule: "" },
            ],
            input_records: 4,
            not_processed: 1,
        },
        Case {
            case: "a table whose SerDe declares its header lines rather than naming its \
                   columns in them",
            objects: vec![(
                "people.csv",
                "customer id,customer email,customer phone\n\
                 r1,ada@example.com,555-0100\n\
                 r2,ada@example.com,555-0200\n"
                    .to_string(),
            )],
            columns: vec!["id", "email", "phone"],
            skip_header: Some("1"),
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }]),
            normalize: None,
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "emailRule" },
                Expected { id: "r2", group: "a", rule: "emailRule" },
            ],
            input_records: 2,
            not_processed: 0,
        },
        Case {
            case: "a table that declares no columns reads its header as their names",
            objects: vec![(
                "people.csv",
                "id,email,phone\n\
                 r1,ada@example.com,555-0100\n\
                 r2,bob@example.com,555-0100\n"
                    .to_string(),
            )],
            columns: Vec::new(),
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "phoneRule", "matchingKeys": ["phone"] }]),
            normalize: None,
            output: id_email_output(),
            expected: vec![
                Expected { id: "r1", group: "a", rule: "phoneRule" },
                Expected { id: "r2", group: "a", rule: "phoneRule" },
            ],
            input_records: 2,
            not_processed: 0,
        },
        Case {
            case: "an input table holding no object at all is a run that processed nothing",
            objects: Vec::new(),
            columns: vec!["id", "email", "phone"],
            skip_header: None,
            fields: id_email_phone_fields(),
            rules: json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }]),
            normalize: None,
            output: id_email_output(),
            expected: Vec::new(),
            input_records: 0,
            not_processed: 0,
        },
    ]
}

/// The output table one run wrote, as `id -> (column -> value)` plus the header, read
/// back out of Amazon S3 through the `csv` crate.
struct OutputTable {
    header: Vec<String>,
    rows: Vec<BTreeMap<String, String>>,
}

impl OutputTable {
    fn parse(case: &str, document: &str) -> Self {
        let mut reader = ::csv::Reader::from_reader(document.as_bytes());
        let header: Vec<String> = reader
            .headers()
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the output table has no header: {error}")
            })
            .iter()
            .map(ToString::to_string)
            .collect();
        let rows = reader
            .records()
            .map(|row| {
                let row = row.unwrap_or_else(|error| {
                    panic!("case {case:?}: the output table has an unreadable record: {error}")
                });
                header
                    .iter()
                    .cloned()
                    .zip(row.iter().map(ToString::to_string))
                    .collect()
            })
            .collect();
        Self { header, rows }
    }

    /// The row whose `id` column is `id`.
    fn row(&self, id: &str) -> Option<&BTreeMap<String, String>> {
        self.rows
            .iter()
            .find(|row| row.get("id").map(String::as_str) == Some(id))
    }
}

/// A `metrics` member of a settled job, or `None` when the reply carries no metrics.
fn metric(reply: &Reply, member: &str) -> Option<i64> {
    reply
        .body
        .get("metrics")
        .and_then(|metrics| metrics.get(member))
        .and_then(Value::as_i64)
}

#[tokio::test]
async fn a_rule_based_run_resolves_records_into_match_groups_and_writes_them() {
    let emulator = Emulator::new();
    emulator.given_bucket(INPUT_BUCKET).await;
    emulator.given_bucket(OUTPUT_BUCKET).await;
    emulator.given_database().await;

    for (index, case) in matching_cases().into_iter().enumerate() {
        let label = case.case;
        let table = format!("people{index}");
        let schema = format!("schema{index}");
        let workflow = format!("workflow{index}");
        let prefix = format!("in/{table}/");
        for (name, body) in &case.objects {
            emulator
                .given_object(INPUT_BUCKET, &format!("{prefix}{name}"), body)
                .await;
        }
        emulator
            .given_table(
                &table,
                &format!("s3://{INPUT_BUCKET}/{prefix}"),
                &case.columns,
                case.skip_header,
            )
            .await;
        emulator.given_schema_mapping(&schema, &case.fields).await;
        let created = emulator
            .create_workflow(&workflow_request(
                &workflow,
                &schema,
                &table,
                case.normalize,
                case.output.clone(),
                &format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
                rule_based(case.rules.clone()),
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the workflow must be created, got {created:?}"
        );

        let started = emulator.start_job(&workflow).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: StartMatchingJob must answer 200, got {started:?}"
        );
        let job_id = started.text("jobId");
        let settled = emulator.settle(label, &workflow, &job_id).await;
        assert_eq!(
            settled.text("status"),
            "SUCCEEDED",
            "case {label:?}: a run that read its input table and wrote its output must \
             SUCCEED, got {:?}",
            settled.body
        );

        // The metrics are what the run counted, not what the case configured: every row
        // of the input table, the records of them it processed, the rows it could not
        // read as records, and the groups it resolved.
        let processed = case.input_records - case.not_processed;
        let groups: std::collections::BTreeSet<&str> =
            case.expected.iter().map(|record| record.group).collect();
        for (member, expected) in [
            ("inputRecords", case.input_records),
            ("totalRecordsProcessed", processed),
            ("recordsNotProcessed", case.not_processed),
            ("matchIDs", groups.len() as i64),
        ] {
            assert_eq!(
                metric(&settled, member),
                Some(expected),
                "case {label:?}: metrics.{member} must be {expected}, got {:?}",
                settled.body
            );
        }

        // The output table is ONE object under the job's own prefix beneath the
        // workflow's `outputS3Path`, so two runs of one workflow never overwrite each
        // other.
        let keys = emulator
            .keys_under(OUTPUT_BUCKET, &format!("{OUTPUT_PREFIX}/{workflow}/"))
            .await;
        assert_eq!(
            keys.len(),
            1,
            "case {label:?}: the run must write exactly one output object under the \
             workflow's output path, got {keys:?}"
        );
        assert_eq!(
            keys[0],
            format!("{OUTPUT_PREFIX}/{workflow}/{job_id}/part-00000.csv"),
            "case {label:?}: the output object must live under this job's own prefix, got \
             {:?}",
            keys[0]
        );
        let document = emulator
            .object_text(OUTPUT_BUCKET, &keys[0])
            .await
            .unwrap_or_else(|| panic!("case {label:?}: the output object must be readable"));
        let output = OutputTable::parse(label, &document);

        // The header is the workflow's own output attributes, then the three columns the
        // run itself determines.
        let mut expected_header: Vec<String> = case
            .output
            .as_array()
            .expect("the case's output attributes are a list")
            .iter()
            .map(|attribute| attribute["name"].as_str().unwrap_or_default().to_string())
            .collect();
        expected_header.extend(
            ["InputSourceARN", "MatchID", "MatchRule"]
                .iter()
                .map(ToString::to_string),
        );
        assert_eq!(
            output.header, expected_header,
            "case {label:?}: the output table's columns must be the workflow's output \
             attributes and the match columns, got {:?}",
            output.header
        );
        assert_eq!(
            output.rows.len(),
            case.expected.len(),
            "case {label:?}: the output table must hold one record per processed record, \
             got {:?}",
            output.rows
        );

        // Records of one group share a `matchId`; two groups never do.
        let mut id_of_group: BTreeMap<&str, String> = BTreeMap::new();
        for record in &case.expected {
            let row = output.row(record.id).unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the output table must hold the record {:?}, got {:?}",
                    record.id, output.rows
                )
            });
            let match_id = row.get("MatchID").cloned().unwrap_or_default();
            assert!(
                !match_id.is_empty(),
                "case {label:?}: record {:?} must carry a matchId, got {row:?}",
                record.id
            );
            assert_eq!(
                row.get("MatchRule").map(String::as_str),
                Some(record.rule),
                "case {label:?}: record {:?} must report the rule that joined it, got \
                 {row:?}",
                record.id
            );
            assert_eq!(
                row.get("InputSourceARN").map(String::as_str),
                Some(glue_table_arn(&table).as_str()),
                "case {label:?}: record {:?} must report the input source it was read \
                 from, got {row:?}",
                record.id
            );
            match id_of_group.get(record.group) {
                None => {
                    assert!(
                        !id_of_group
                            .values()
                            .any(|held| *held == match_id),
                        "case {label:?}: group {:?} must not share the matchId {match_id:?} \
                         with another group",
                        record.group
                    );
                    id_of_group.insert(record.group, match_id);
                }
                Some(held) => assert_eq!(
                    *held, match_id,
                    "case {label:?}: record {:?} must share its group's matchId, got \
                     {match_id:?}",
                    record.id
                ),
            }
        }
    }
}

/// One row of the unperformable-run table: how the workflow or its input is configured,
/// and what the reason the job reports must name.
struct FailureCase {
    case: &'static str,
    /// The table's `StorageDescriptor$Location`, or `None` for a workflow whose input
    /// table the catalog does not hold at all.
    location: Option<&'static str>,
    /// `outputSourceConfig[0].outputS3Path`.
    output_s3_path: String,
    /// `resolutionTechniques`.
    techniques: Value,
    /// A fragment the reported `errorDetails.errorMessage` must carry, so the reason
    /// names what could not be done rather than being any sentence at all.
    reason_carries: String,
}

fn failure_cases() -> Vec<FailureCase> {
    vec![
        FailureCase {
            case: "an input table the catalog does not hold",
            location: None,
            output_s3_path: format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
            techniques: rule_based(json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }])),
            reason_carries: "AWS Glue Data Catalog".to_string(),
        },
        FailureCase {
            case: "an input table whose location names a bucket Amazon S3 does not hold",
            location: Some("s3://er-missing-bucket/in/"),
            output_s3_path: format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
            techniques: rule_based(json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }])),
            reason_carries: "could not be listed".to_string(),
        },
        FailureCase {
            case: "an input table whose location is not an Amazon S3 location",
            location: Some("hdfs://cluster/in/"),
            output_s3_path: format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
            techniques: rule_based(json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }])),
            reason_carries: "Amazon S3 location".to_string(),
        },
        FailureCase {
            case: "a workflow whose output source names no S3 path",
            location: Some("s3://er-input/in/"),
            output_s3_path: String::new(),
            techniques: rule_based(json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }])),
            reason_carries: "nowhere to write".to_string(),
        },
        FailureCase {
            case: "a workflow that resolves with a machine-learning model",
            location: Some("s3://er-input/in/"),
            output_s3_path: format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
            techniques: json!({ "resolutionType": "ML_MATCHING" }),
            reason_carries: "ML_MATCHING".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_run_that_produced_nothing_fails_with_the_reason_and_writes_no_output() {
    let emulator = Emulator::new();
    emulator.given_bucket(INPUT_BUCKET).await;
    emulator.given_bucket(OUTPUT_BUCKET).await;
    emulator.given_database().await;

    for (index, case) in failure_cases().into_iter().enumerate() {
        let label = case.case;
        let table = format!("absent{index}");
        let schema = format!("schema{index}");
        let workflow = format!("failing{index}");
        if let Some(location) = case.location {
            emulator
                .given_table(&table, location, &["id", "email"], None)
                .await;
        }
        emulator
            .given_schema_mapping(&schema, &id_email_phone_fields())
            .await;
        let created = emulator
            .create_workflow(&workflow_request(
                &workflow,
                &schema,
                &table,
                None,
                id_email_output(),
                &case.output_s3_path,
                case.techniques.clone(),
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the workflow must be created, got {created:?}"
        );

        let started = emulator.start_job(&workflow).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: StartMatchingJob must answer 200, got {started:?}"
        );
        let job_id = started.text("jobId");
        let settled = emulator.settle(label, &workflow, &job_id).await;
        assert_eq!(
            settled.text("status"),
            "FAILED",
            "case {label:?}: a run that produced nothing must report FAILED, got {:?}",
            settled.body
        );
        let reason = settled
            .body
            .get("errorDetails")
            .and_then(|details| details.get("errorMessage"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !reason.is_empty() && reason.chars().count() <= 2048,
            "case {label:?}: the reason must satisfy the modelled length 1..2048, got \
             {reason:?}"
        );
        assert!(
            reason.contains(&case.reason_carries),
            "case {label:?}: the reason must name what could not be done ({:?}), got \
             {reason:?}",
            case.reason_carries
        );
        assert!(
            settled.body.get("metrics").is_none(),
            "case {label:?}: a run that counted nothing must report no metrics, got {:?}",
            settled.body
        );
        let keys = emulator
            .keys_under(OUTPUT_BUCKET, &format!("{OUTPUT_PREFIX}/{workflow}/"))
            .await;
        assert!(
            keys.is_empty(),
            "case {label:?}: a failed run must write no output object, got {keys:?}"
        );
    }
}

#[tokio::test]
async fn a_record_a_run_processed_is_readable_through_get_match_id() {
    let emulator = Emulator::new();
    emulator.given_bucket(INPUT_BUCKET).await;
    emulator.given_bucket(OUTPUT_BUCKET).await;
    emulator.given_database().await;
    emulator
        .given_object(
            INPUT_BUCKET,
            "indexed/people.csv",
            "id,email,phone\n\
             r1,ada@example.com,555-0100\n\
             r2,ada@example.com,555-0200\n\
             r3,bob@example.com,555-0300\n",
        )
        .await;
    emulator
        .given_table(
            "indexed",
            &format!("s3://{INPUT_BUCKET}/indexed/"),
            &["id", "email", "phone"],
            None,
        )
        .await;
    emulator
        .given_schema_mapping("indexedschema", &id_email_phone_fields())
        .await;
    let created = emulator
        .create_workflow(&workflow_request(
            "indexedflow",
            "indexedschema",
            "indexed",
            None,
            id_email_output(),
            &format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
            rule_based(json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }])),
        ))
        .await;
    assert_eq!(
        created.status, 200,
        "the workflow must be created, got {created:?}"
    );

    // Before the run, no record of this workflow has been processed, so there is no
    // match id to read: that is what makes the read below evidence of the RUN.
    let before = emulator
        .get_match_id(
            "indexedflow",
            json!({ "id": "r1", "email": "ada@example.com", "phone": "555-0100" }),
        )
        .await;
    assert_eq!(
        before.status, 200,
        "GetMatchId must answer 200, got {before:?}"
    );
    assert!(
        before.body.get("matchId").is_none(),
        "a record no run has processed must have no matchId, got {:?}",
        before.body
    );

    let started = emulator.start_job("indexedflow").await;
    let job_id = started.text("jobId");
    let settled = emulator
        .settle("get match id", "indexedflow", &job_id)
        .await;
    assert_eq!(
        settled.text("status"),
        "SUCCEEDED",
        "the run must succeed, got {:?}",
        settled.body
    );

    // The same group the OUTPUT TABLE reports, read back through the API: the two
    // records that share an email answer one matchId under the rule that joined them,
    // and the third answers its own.
    let keys = emulator
        .keys_under(OUTPUT_BUCKET, &format!("{OUTPUT_PREFIX}/"))
        .await;
    let document = emulator
        .object_text(OUTPUT_BUCKET, &keys[0])
        .await
        .expect("the output object must be readable");
    let output = OutputTable::parse("get match id", &document);

    let mut read: Vec<(String, String)> = Vec::new();
    for (id, email, phone, rule) in [
        ("r1", "ada@example.com", "555-0100", "emailRule"),
        ("r2", "ada@example.com", "555-0200", "emailRule"),
        ("r3", "bob@example.com", "555-0300", ""),
    ] {
        let reply = emulator
            .get_match_id(
                "indexedflow",
                json!({ "id": id, "email": email, "phone": phone }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {id:?}: GetMatchId must answer 200, got {reply:?}"
        );
        let match_id = reply.text("matchId");
        let expected = output
            .row(id)
            .and_then(|row| row.get("MatchID").cloned())
            .unwrap_or_default();
        assert_eq!(
            match_id, expected,
            "case {id:?}: GetMatchId must report the matchId the output table carries"
        );
        assert_eq!(
            reply.text("matchRule"),
            rule,
            "case {id:?}: GetMatchId must report the rule that joined the record, got \
             {reply:?}"
        );
        read.push((id.to_string(), match_id));
    }
    assert_eq!(
        read[0].1, read[1].1,
        "the two records that share a matching key must share one matchId, got {read:?}"
    );
    assert_ne!(
        read[0].1, read[2].1,
        "a record no rule joined must have a matchId of its own, got {read:?}"
    );
}

#[tokio::test]
async fn rows_that_are_not_records_of_their_table_are_counted_not_processed() {
    // A row this emulator cannot read as a record of its table -- here an unterminated
    // quoted field, which swallows the rest of the object into one oversized record --
    // is COUNTED as not processed rather than matched on guessed values, and the run
    // reports the counts that say so: the output table then carries only the records it
    // could read, and no matchId is invented for a row nothing could resolve.
    let emulator = Emulator::new();
    emulator.given_bucket(INPUT_BUCKET).await;
    emulator.given_bucket(OUTPUT_BUCKET).await;
    emulator.given_database().await;
    emulator
        .given_object(
            INPUT_BUCKET,
            "torn/people.csv",
            "id,email,phone\n\
             r1,\"ada@example.com,555-0100\n\
             r2,bob@example.com,555-0200\n",
        )
        .await;
    emulator
        .given_table(
            "torn",
            &format!("s3://{INPUT_BUCKET}/torn/"),
            &["id", "email", "phone"],
            None,
        )
        .await;
    emulator
        .given_schema_mapping("tornschema", &id_email_phone_fields())
        .await;
    let created = emulator
        .create_workflow(&workflow_request(
            "tornflow",
            "tornschema",
            "torn",
            None,
            id_email_output(),
            &format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
            rule_based(json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }])),
        ))
        .await;
    assert_eq!(
        created.status, 200,
        "the workflow must be created, got {created:?}"
    );
    let started = emulator.start_job("tornflow").await;
    let job_id = started.text("jobId");
    let settled = emulator.settle("torn object", "tornflow", &job_id).await;
    assert_eq!(
        settled.text("status"),
        "SUCCEEDED",
        "the run read its input table and wrote its output, so it must SUCCEED, got {:?}",
        settled.body
    );
    for (member, expected) in [
        ("inputRecords", 1),
        ("totalRecordsProcessed", 0),
        ("recordsNotProcessed", 1),
        ("matchIDs", 0),
    ] {
        assert_eq!(
            metric(&settled, member),
            Some(expected),
            "metrics.{member} must be {expected}, got {:?}",
            settled.body
        );
    }
    let keys = emulator
        .keys_under(OUTPUT_BUCKET, &format!("{OUTPUT_PREFIX}/tornflow/"))
        .await;
    let document = emulator
        .object_text(OUTPUT_BUCKET, &keys[0])
        .await
        .expect("the output object must be readable");
    let output = OutputTable::parse("torn object", &document);
    assert!(
        output.rows.is_empty(),
        "no record could be read, so the output table must carry none, got {:?}",
        output.rows
    );
}

#[tokio::test]
async fn an_output_attribute_marked_hashed_carries_the_digest_of_the_value() {
    let emulator = Emulator::new();
    emulator.given_bucket(INPUT_BUCKET).await;
    emulator.given_bucket(OUTPUT_BUCKET).await;
    emulator.given_database().await;
    emulator
        .given_object(
            INPUT_BUCKET,
            "hashed/people.csv",
            "id,email,phone\n\
             r1,ada@example.com,555-0100\n\
             r2,ada@example.com,555-0200\n",
        )
        .await;
    emulator
        .given_table(
            "hashed",
            &format!("s3://{INPUT_BUCKET}/hashed/"),
            &["id", "email", "phone"],
            None,
        )
        .await;
    emulator
        .given_schema_mapping("hashedschema", &id_email_phone_fields())
        .await;
    let created = emulator
        .create_workflow(&workflow_request(
            "hashedflow",
            "hashedschema",
            "hashed",
            None,
            json!([{ "name": "id" }, { "name": "email", "hashed": true }]),
            &format!("s3://{OUTPUT_BUCKET}/{OUTPUT_PREFIX}"),
            rule_based(json!([{ "ruleName": "emailRule", "matchingKeys": ["email"] }])),
        ))
        .await;
    assert_eq!(
        created.status, 200,
        "the workflow must be created, got {created:?}"
    );
    let started = emulator.start_job("hashedflow").await;
    let job_id = started.text("jobId");
    let settled = emulator.settle("hashed output", "hashedflow", &job_id).await;
    assert_eq!(
        settled.text("status"),
        "SUCCEEDED",
        "the run must succeed, got {:?}",
        settled.body
    );

    let keys = emulator
        .keys_under(OUTPUT_BUCKET, &format!("{OUTPUT_PREFIX}/"))
        .await;
    let document = emulator
        .object_text(OUTPUT_BUCKET, &keys[0])
        .await
        .expect("the output object must be readable");
    let output = OutputTable::parse("hashed output", &document);
    // The digest is computed here with the same maintained crate the service uses, so
    // this asserts the VALUE's digest rather than re-stating a literal.
    let expected = ::hex::encode(<::sha2::Sha256 as ::sha2::Digest>::digest(
        "ada@example.com".as_bytes(),
    ));
    for id in ["r1", "r2"] {
        let row = output
            .row(id)
            .unwrap_or_else(|| panic!("the output table must hold {id}, got {:?}", output.rows));
        assert_eq!(
            row.get("email").map(String::as_str),
            Some(expected.as_str()),
            "a hashed output column must carry the digest of the value, got {row:?}"
        );
        assert_eq!(
            row.get("id").map(String::as_str),
            Some(id),
            "an unhashed output column must carry the value itself, got {row:?}"
        );
    }
}
