// SPDX-License-Identifier: CC0-1.0

//! BULK THING REGISTRATION end to end over the wire, with AWS IoT, Amazon S3 and IAM mounted
//! on ONE engine — the shape `core/router/src/lib.rs` mounts them in — so the input file a
//! task names is an object staged with `PutObject` and the role it names is a role IAM
//! actually holds.
//!
//! The RULE asserted here: `StartThingRegistrationTask` reads the newline-delimited JSON
//! object its `inputFileBucket`/`inputFileKey` name and provisions ONE thing per line, each
//! line's parameter values substituted into the `AWS::IoT::Thing` resource of its
//! `templateBody`, into the SAME registry `CreateThing` writes and under the same rules — so
//! `DescribeThing` reads a bulk-provisioned thing back member for member, and
//! `DescribeThingRegistrationTask` reports the task's own record: what the start supplied,
//! the status the clock has brought it to, and how many lines the run provisioned and how
//! many it could not. A `roleArn` IAM answered this service cannot assume, and a
//! `templateBody` that describes no thing, are refused (HTTP 400 `InvalidRequestException`)
//! with no task recorded and nothing provisioned.
//!
//! The tables span what the rule generalises over rather than one recorded file: how many
//! lines the file has and which of them are blank, whether a line supplies a parameter or
//! leaves the template's `Default` to stand, the JSON type of a parameter value, which
//! properties the thing resource states and in which of the two forms the attributes are
//! stated, whether a line names a thing type the account holds, and whether the name is
//! already registered — with the same configuration or a different one.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_iot_awsiotservice::state::thing_registration::{
    MAX_INPUT_FILE_BYTES, REGISTRATION_SPAN, TASK_STATUS_COMPLETED, TASK_STATUS_FAILED,
    TASK_STATUS_IN_PROGRESS,
};
use service_com_amazonaws_iot_awsiotservice::state::SERVICE_PRINCIPAL;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `awsQuery` form encoder and the XML tag reader are this crate's test suite's own, in
/// `topic_rule_kinesis_action`, which puts roles in IAM over the same wire.
use crate::topic_rule_kinesis_action::{form_encode, xml_text};

/// The Region every call below is signed for. Amazon S3 places a bucket created with no
/// `LocationConstraint` in us-east-1, which is also the Region IoT serves a request in, so
/// one Region label serves every call here.
const REGION: &str = "us-east-1";

/// The modelled error a refused start carries (`@httpError 400`).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The modelled error a read of a task nobody started carries (`@httpError 404`).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// What AWS IoT reports for a `roleArn` it cannot assume, spelled out here rather than taken
/// from the handler, so the assertion is independent of the code under test.
const CANNOT_ASSUME_ROLE: &str = "Cannot assume provided role";

/// What a task reports as its `message` when the input file could not be read, spelled out
/// for the same reason.
const UNREADABLE_INPUT_FILE: &str =
    "Unable to read the input file from S3. Check the bucket, the key and the permissions \
     granted by the role.";

/// The bucket every input file below is staged in.
const BUCKET: &str = "iot-bulk-registration";

/// A service principal that is NOT this service, for the role that trusts someone else.
const OTHER_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The margin added to [`REGISTRATION_SPAN`] before a started task's completion is read, so
/// the wait lands past the due instant rather than on it.
const PAST_REGISTRATION: Duration = Duration::from_millis(500);

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// An integer member of the body, or `None` when it is absent or not a number.
    fn number(&self, name: &str) -> Option<i64> {
        self.body.get(name).and_then(Value::as_i64)
    }

    /// The `attributes` member as an ordered map, so two maps compare independently of JSON
    /// member order.
    fn attributes(&self) -> BTreeMap<String, String> {
        self.body
            .get("attributes")
            .and_then(Value::as_object)
            .map(|members: &Map<String, Value>| {
                members
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            value.as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulator: the IoT control plane, Amazon S3 and IAM on ONE shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// One request through the real router, claimed by the service its credential scope
    /// names. Signature verification is off in this harness, so the signature itself is
    /// never checked.
    async fn send(
        &self,
        method: &str,
        path: &str,
        signing_name: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{signing_name}.{REGION}.amazonaws.com{path}"))
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDTESTIOT/20260101/{REGION}/{signing_name}/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", content_type)
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// One IoT control-plane call: restJson1, the body carrying the operation's members.
    async fn iot(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        self.send(method, path, "iot", "application/json", payload)
            .await
    }

    /// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
    /// live in the URI and the body is the object's own bytes.
    async fn s3(&self, method: &str, path: &str, body: Vec<u8>) -> u16 {
        self.send(method, path, "s3", "application/octet-stream", body)
            .await
            .status
    }

    /// One IAM call over the `awsQuery` protocol, answered with its XML body.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://iam.{REGION}.amazonaws.com/"))
            .header("host", format!("iam.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDTESTIOT/20260101/{REGION}/iam/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body.into_bytes()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(&response.into_body()).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: {action} answered {status}: {text}"
        );
        text
    }

    /// `PUT /<bucket>` — `CreateBucket` — so the staged objects have a bucket to live in.
    async fn given_bucket(&self) {
        let status = self.s3("PUT", &format!("/{BUCKET}"), Vec::new()).await;
        assert_eq!(
            status, 200,
            "the input file bucket must be created first, got {status}"
        );
    }

    /// `PUT /<bucket>/<key>` — `PutObject` — staging `contents` as the input file.
    async fn given_object(&self, case: &str, key: &str, contents: &str) {
        let status = self
            .s3(
                "PUT",
                &format!("/{BUCKET}/{key}"),
                contents.as_bytes().to_vec(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the input file must be staged, got {status}"
        );
    }

    /// A role trusting `principal`, answered with the ARN IAM persisted.
    async fn given_role(&self, case: &str, name: &str, principal: &str) -> String {
        let trust = trust_policy(principal);
        let created = self
            .iam(
                case,
                "CreateRole",
                &[("RoleName", name), ("AssumeRolePolicyDocument", &trust)],
            )
            .await;
        xml_text(&created, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: CreateRole reported no Arn: {created}"))
    }

    /// `POST /thing-registration-tasks` — `StartThingRegistrationTask`.
    async fn start_task(&self, key: &str, role_arn: &str, template_body: &str) -> Reply {
        self.iot(
            "POST",
            "/thing-registration-tasks",
            Some(&json!({
                "inputFileBucket": BUCKET,
                "inputFileKey": key,
                "roleArn": role_arn,
                "templateBody": template_body,
            })),
        )
        .await
    }

    /// `GET /thing-registration-tasks/{taskId}` — `DescribeThingRegistrationTask`.
    async fn describe_task(&self, task_id: &str) -> Reply {
        self.iot(
            "GET",
            &format!("/thing-registration-tasks/{task_id}"),
            None,
        )
        .await
    }

    /// `GET /things/{thingName}` — `DescribeThing`, the reader that proves what a task
    /// provisioned.
    async fn describe_thing(&self, thing_name: &str) -> Reply {
        self.iot("GET", &format!("/things/{thing_name}"), None).await
    }

    /// `POST /things/{thingName}` — `CreateThing`, for the names a case has registered
    /// before the task runs.
    async fn create_thing(&self, case: &str, thing_name: &str, request: &Value) {
        let reply = self
            .iot("POST", &format!("/things/{thing_name}"), Some(request))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: {thing_name} must be registered first, got {} {}",
            reply.status, reply.body
        );
    }

    /// `POST /thing-types/{thingTypeName}` — `CreateThingType`.
    async fn create_thing_type(&self, case: &str, thing_type_name: &str) {
        let reply = self
            .iot(
                "POST",
                &format!("/thing-types/{thing_type_name}"),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the thing type must exist first, got {} {}",
            reply.status, reply.body
        );
    }
}

/// A trust policy admitting `principal` to assume the role.
fn trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// A provisioning template declaring `parameters` and one `AWS::IoT::Thing` resource with
/// `properties` — the structure AWS documents for a provisioning template.
fn template(parameters: Value, properties: Value) -> String {
    json!({
        "Parameters": parameters,
        "Resources": {
            "thing": { "Type": "AWS::IoT::Thing", "Properties": properties },
        },
    })
    .to_string()
}

/// The two parameters most rows below provision from: a serial number every line supplies,
/// and a location the template gives a `Default` for.
fn serial_and_location() -> Value {
    json!({
        "SerialNumber": { "Type": "String" },
        "Location": { "Type": "String", "Default": "WA" },
    })
}

/// The thing a row expects in the registry after its task has run.
struct ExpectedThing {
    thing_name: &'static str,
    attributes: Vec<(&'static str, &'static str)>,
    thing_type_name: Option<&'static str>,
}

/// One row of the provisioning table: an input file, the template it is provisioned under,
/// and what the run of it must amount to.
struct Row {
    case: &'static str,
    /// The template body, as a client sends it.
    template_body: String,
    /// The input file's own bytes — newline delimited JSON.
    input_file: String,
    /// "The number of things successfully provisioned."
    success_count: i64,
    /// "The number of things that failed to be provisioned."
    failure_count: i64,
    /// The things the registry must hold afterwards.
    provisioned: Vec<ExpectedThing>,
    /// The names the registry must NOT hold afterwards.
    unprovisioned: Vec<&'static str>,
}

/// A bulk task provisions one thing per line of its input file, and the task reports the run.
#[tokio::test]
async fn a_bulk_task_provisions_one_thing_per_line_of_its_input_file() {
    let emulator = Emulator::new();
    emulator.given_bucket().await;
    let role_arn = emulator
        .given_role("the provisioning table", "bulk-reader", SERVICE_PRINCIPAL)
        .await;

    // A thing type one row provisions against, and the two names another row has registered
    // before its task runs: one with the configuration the template produces, one with a
    // different configuration.
    emulator.create_thing_type("a thing type the account holds", "lightBulb").await;
    emulator
        .create_thing(
            "a name already registered with the same configuration",
            "dup-same",
            &json!({ "attributePayload": { "attributes": { "location": "WA", "model": "a1" } } }),
        )
        .await;
    emulator
        .create_thing(
            "a name already registered with a different configuration",
            "dup-other",
            &json!({ "attributePayload": { "attributes": { "location": "ZZ", "model": "a1" } } }),
        )
        .await;

    // The properties most rows provision: the name from a `Ref`, two attributes (one from a
    // `Ref` the template defaults, one literal for every line).
    let named_properties = json!({
        "ThingName": { "Ref": "SerialNumber" },
        "AttributePayload": {
            "location": { "Ref": "Location" },
            "model": "a1",
        },
    });

    let rows: Vec<Row> = vec![
        Row {
            case: "three lines, each one thing, one of them leaving the defaulted parameter out",
            template_body: template(serial_and_location(), named_properties.clone()),
            input_file: [
                json!({ "SerialNumber": "sn-1", "Location": "OR" }).to_string(),
                json!({ "SerialNumber": "sn-2", "Location": "CA" }).to_string(),
                json!({ "SerialNumber": "sn-3" }).to_string(),
            ]
            .join("\n"),
            success_count: 3,
            failure_count: 0,
            provisioned: vec![
                ExpectedThing {
                    thing_name: "sn-1",
                    attributes: vec![("location", "OR"), ("model", "a1")],
                    thing_type_name: None,
                },
                ExpectedThing {
                    thing_name: "sn-2",
                    attributes: vec![("location", "CA"), ("model", "a1")],
                    thing_type_name: None,
                },
                ExpectedThing {
                    thing_name: "sn-3",
                    attributes: vec![("location", "WA"), ("model", "a1")],
                    thing_type_name: None,
                },
            ],
            unprovisioned: vec![],
        },
        Row {
            case: "a blank line and a trailing newline are no device",
            template_body: template(serial_and_location(), named_properties.clone()),
            input_file: format!(
                "{}\n\n   \n{}\n",
                json!({ "SerialNumber": "blank-1" }),
                json!({ "SerialNumber": "blank-2" })
            ),
            success_count: 2,
            failure_count: 0,
            provisioned: vec![
                ExpectedThing {
                    thing_name: "blank-1",
                    attributes: vec![("location", "WA"), ("model", "a1")],
                    thing_type_name: None,
                },
                ExpectedThing {
                    thing_name: "blank-2",
                    attributes: vec![("location", "WA"), ("model", "a1")],
                    thing_type_name: None,
                },
            ],
            unprovisioned: vec![],
        },
        Row {
            case: "one line without the name's parameter, beside one that has it",
            template_body: template(serial_and_location(), named_properties.clone()),
            input_file: [
                json!({ "SerialNumber": "partial-1" }).to_string(),
                json!({ "Location": "OR" }).to_string(),
            ]
            .join("\n"),
            success_count: 1,
            failure_count: 1,
            provisioned: vec![ExpectedThing {
                thing_name: "partial-1",
                attributes: vec![("location", "WA"), ("model", "a1")],
                thing_type_name: None,
            }],
            unprovisioned: vec![],
        },
        Row {
            case: "a line that is no JSON object of parameter values",
            template_body: template(serial_and_location(), named_properties.clone()),
            input_file: ["not json at all".to_string(), "[1, 2]".to_string()].join("\n"),
            success_count: 0,
            failure_count: 2,
            provisioned: vec![],
            unprovisioned: vec![],
        },
        Row {
            case: "a parameter value of another JSON type, which is the text of the value",
            template_body: template(serial_and_location(), named_properties.clone()),
            input_file: [
                json!({ "SerialNumber": 900001, "Location": 7 }).to_string(),
                json!({ "SerialNumber": "typed-bool", "Location": true }).to_string(),
            ]
            .join("\n"),
            success_count: 2,
            failure_count: 0,
            provisioned: vec![
                ExpectedThing {
                    thing_name: "900001",
                    attributes: vec![("location", "7"), ("model", "a1")],
                    thing_type_name: None,
                },
                ExpectedThing {
                    thing_name: "typed-bool",
                    attributes: vec![("location", "true"), ("model", "a1")],
                    thing_type_name: None,
                },
            ],
            unprovisioned: vec![],
        },
        Row {
            case: "the attributes stated under the AttributePayload attributes member",
            template_body: template(
                serial_and_location(),
                json!({
                    "ThingName": { "Ref": "SerialNumber" },
                    "AttributePayload": {
                        "attributes": { "location": { "Ref": "Location" }, "model": "a1" },
                    },
                }),
            ),
            input_file: json!({ "SerialNumber": "nested-1", "Location": "OR" }).to_string(),
            success_count: 1,
            failure_count: 0,
            provisioned: vec![ExpectedThing {
                thing_name: "nested-1",
                attributes: vec![("location", "OR"), ("model", "a1")],
                thing_type_name: None,
            }],
            unprovisioned: vec![],
        },
        Row {
            case: "the attributes stated as the modelled payload shape, beside its merge member",
            template_body: template(
                serial_and_location(),
                json!({
                    "ThingName": { "Ref": "SerialNumber" },
                    "AttributePayload": {
                        "attributes": { "location": { "Ref": "Location" }, "model": "a1" },
                        "merge": true,
                    },
                }),
            ),
            input_file: json!({ "SerialNumber": "merge-1", "Location": "OR" }).to_string(),
            success_count: 1,
            failure_count: 0,
            provisioned: vec![ExpectedThing {
                thing_name: "merge-1",
                // `merge` is a member of the payload shape, not an attribute of the thing.
                attributes: vec![("location", "OR"), ("model", "a1")],
                thing_type_name: None,
            }],
            unprovisioned: vec![],
        },
        Row {
            case: "a thing resource stating no attributes at all",
            template_body: template(
                serial_and_location(),
                json!({ "ThingName": { "Ref": "SerialNumber" } }),
            ),
            input_file: json!({ "SerialNumber": "bare-1" }).to_string(),
            success_count: 1,
            failure_count: 0,
            provisioned: vec![ExpectedThing {
                thing_name: "bare-1",
                attributes: vec![],
                thing_type_name: None,
            }],
            unprovisioned: vec![],
        },
        Row {
            case: "a thing type the account holds, beside one it does not",
            template_body: template(
                json!({
                    "SerialNumber": { "Type": "String" },
                    "Type": { "Type": "String", "Default": "lightBulb" },
                }),
                json!({
                    "ThingName": { "Ref": "SerialNumber" },
                    "ThingTypeName": { "Ref": "Type" },
                }),
            ),
            input_file: [
                json!({ "SerialNumber": "typed-held" }).to_string(),
                json!({ "SerialNumber": "typed-absent", "Type": "noSuchType" }).to_string(),
            ]
            .join("\n"),
            success_count: 1,
            failure_count: 1,
            provisioned: vec![ExpectedThing {
                thing_name: "typed-held",
                attributes: vec![],
                thing_type_name: Some("lightBulb"),
            }],
            unprovisioned: vec!["typed-absent"],
        },
        Row {
            case: "a name already registered, with the same configuration and with another",
            template_body: template(serial_and_location(), named_properties.clone()),
            input_file: [
                json!({ "SerialNumber": "dup-same" }).to_string(),
                json!({ "SerialNumber": "dup-other" }).to_string(),
            ]
            .join("\n"),
            success_count: 1,
            failure_count: 1,
            provisioned: vec![ExpectedThing {
                thing_name: "dup-same",
                attributes: vec![("location", "WA"), ("model", "a1")],
                thing_type_name: None,
            }],
            unprovisioned: vec![],
        },
        Row {
            case: "a name that is no ThingName the registry can be addressed by",
            template_body: template(serial_and_location(), named_properties.clone()),
            input_file: json!({ "SerialNumber": "no spaces allowed" }).to_string(),
            success_count: 0,
            failure_count: 1,
            provisioned: vec![],
            unprovisioned: vec![],
        },
    ];

    // Every row's file is staged and its task started BEFORE anything is asserted, so the
    // one wait past the registration span serves the whole table.
    let mut started: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let key = format!("devices/{index}.json");
        emulator.given_object(row.case, &key, &row.input_file).await;
        let reply = emulator
            .start_task(&key, &role_arn, &row.template_body)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected the start to be accepted, got {} {}",
            row.case, reply.status, reply.body
        );
        let task_id = reply.text("taskId").unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a taskId, got {}",
                row.case, reply.body
            )
        });
        assert!(
            !task_id.is_empty() && task_id.chars().count() <= 40,
            "case {:?}: expected a TaskId within the modelled length, got {task_id:?}",
            row.case
        );

        // Straight after the start the run is still owed, so the task reports the
        // non-terminal status with no outcome yet.
        let described = emulator.describe_task(&task_id).await;
        assert_eq!(
            described.text("status").as_deref(),
            Some(TASK_STATUS_IN_PROGRESS),
            "case {:?}: expected a fresh task to be {TASK_STATUS_IN_PROGRESS}, got {}",
            row.case,
            described.body
        );
        assert_eq!(
            (
                described.number("successCount"),
                described.number("failureCount"),
                described.number("percentageProgress")
            ),
            (Some(0), Some(0), Some(0)),
            "case {:?}: expected a running task to report no outcome yet, got {}",
            row.case,
            described.body
        );
        assert_eq!(
            described.text("creationDate"),
            described.text("lastModifiedDate"),
            "case {:?}: a task that has not changed since its start was last modified then, \
             got {}",
            row.case,
            described.body
        );
        started.push((index, task_id));
    }

    tokio::time::sleep(REGISTRATION_SPAN + PAST_REGISTRATION).await;

    for (index, task_id) in started {
        let row = &rows[index];
        let case = row.case;
        let key = format!("devices/{index}.json");

        let described = emulator.describe_task(&task_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected the task to read back, got {} {}",
            described.status, described.body
        );
        assert_eq!(
            described.text("status").as_deref(),
            Some(TASK_STATUS_COMPLETED),
            "case {case:?}: expected {TASK_STATUS_COMPLETED} once the run is due, got {}",
            described.body
        );
        assert_eq!(
            (
                described.number("successCount"),
                described.number("failureCount"),
                described.number("percentageProgress")
            ),
            (
                Some(row.success_count),
                Some(row.failure_count),
                Some(100)
            ),
            "case {case:?}: expected {} provisioned and {} failed at 100%, got {}",
            row.success_count,
            row.failure_count,
            described.body
        );
        // The task reports back what the start supplied, member for member.
        assert_eq!(
            (
                described.text("taskId"),
                described.text("inputFileBucket"),
                described.text("inputFileKey"),
                described.text("roleArn"),
                described.text("templateBody"),
            ),
            (
                Some(task_id.clone()),
                Some(BUCKET.to_string()),
                Some(key.clone()),
                Some(role_arn.clone()),
                Some(row.template_body.clone()),
            ),
            "case {case:?}: expected the task to report what the start supplied, got {}",
            described.body
        );
        assert!(
            described.text("message").is_none(),
            "case {case:?}: a task whose run was performed reports no failure message, got {}",
            described.body
        );

        // ROUND TRIP: every provisioned line is a thing the registry's own reader hands
        // back, with the configuration the template described for that line.
        for thing in &row.provisioned {
            let read = emulator.describe_thing(thing.thing_name).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected {} to be registered, got {} {}",
                thing.thing_name, read.status, read.body
            );
            let expected: BTreeMap<String, String> = thing
                .attributes
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect();
            let read_attributes = read.attributes();
            assert_eq!(
                read_attributes, expected,
                "case {case:?}: expected {} to carry {expected:?}, got {read_attributes:?}",
                thing.thing_name
            );
            assert_eq!(
                read.text("thingTypeName").as_deref(),
                thing.thing_type_name,
                "case {case:?}: expected {} to carry thing type {:?}, got {}",
                thing.thing_name,
                thing.thing_type_name,
                read.body
            );
            assert_eq!(
                read.text("thingArn").as_deref(),
                Some(
                    format!(
                        "arn:aws:iot:{REGION}:000000000000:thing/{}",
                        thing.thing_name
                    )
                    .as_str()
                ),
                "case {case:?}: expected the ARN the registry mints, got {}",
                read.body
            );
        }
        for thing_name in &row.unprovisioned {
            let read = emulator.describe_thing(thing_name).await;
            assert_eq!(
                (read.status, read.error_code.as_deref()),
                (404, Some(NOT_FOUND)),
                "case {case:?}: a line that provisioned nothing leaves no thing {thing_name}, \
                 got {} {}",
                read.status,
                read.body
            );
        }
    }
}

/// One refused start: the value it carries and what the refusal must say.
struct Refusal {
    case: &'static str,
    /// The role the start names, or `None` for the role the table holds.
    role_arn: Option<String>,
    /// The template body the start carries, or `None` for a well-formed one.
    template_body: Option<String>,
    /// What the refusal reports, or `None` for a start that is ACCEPTED.
    message: Option<String>,
}

/// A start whose role this service cannot assume, or whose template describes no thing, is
/// refused with nothing recorded and nothing provisioned.
#[tokio::test]
async fn a_start_is_refused_before_any_task_is_recorded() {
    let emulator = Emulator::new();
    emulator.given_bucket().await;
    let held_role = emulator
        .given_role("the refusal table", "bulk-reader", SERVICE_PRINCIPAL)
        .await;
    let other_role = emulator
        .given_role("a role trusting another service", "bulk-other", OTHER_PRINCIPAL)
        .await;
    let well_formed = template(
        serial_and_location(),
        json!({ "ThingName": { "Ref": "SerialNumber" } }),
    );

    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "a role that exists and trusts another service",
            role_arn: Some(other_role),
            template_body: None,
            message: Some(CANNOT_ASSUME_ROLE.to_string()),
        },
        Refusal {
            case: "a well-formed ARN naming a role this account does not hold",
            role_arn: Some("arn:aws:iam::000000000000:role/bulk-absent".to_string()),
            template_body: None,
            message: Some(CANNOT_ASSUME_ROLE.to_string()),
        },
        Refusal {
            case: "a role this account does not hold, under an IAM path",
            role_arn: Some("arn:aws:iam::000000000000:role/service-role/bulk-absent".to_string()),
            template_body: None,
            message: Some(CANNOT_ASSUME_ROLE.to_string()),
        },
        Refusal {
            // Another account's IAM is not this emulator's to ask, so nothing grounds a
            // refusal and the start is accepted.
            case: "another account's role, which this account's IAM answers nothing about",
            role_arn: Some("arn:aws:iam::123456789012:role/bulk-cross-account".to_string()),
            template_body: None,
            message: None,
        },
        Refusal {
            case: "a template body that is no JSON object",
            role_arn: None,
            template_body: Some("{".to_string()),
            message: Some(
                "Invalid template: the provisioning template is not a JSON object".to_string(),
            ),
        },
        Refusal {
            case: "a template body that is a JSON array",
            role_arn: None,
            template_body: Some(json!([]).to_string()),
            message: Some(
                "Invalid template: the provisioning template is not a JSON object".to_string(),
            ),
        },
        Refusal {
            case: "a template declaring no Resources",
            role_arn: None,
            template_body: Some(json!({ "Parameters": {} }).to_string()),
            message: Some(
                "Invalid template: the provisioning template declares no Resources".to_string(),
            ),
        },
        Refusal {
            case: "a template whose Resources describe no thing",
            role_arn: None,
            template_body: Some(
                json!({
                    "Resources": {
                        "certificate": {
                            "Type": "AWS::IoT::Certificate",
                            "Properties": { "CertificateId": { "Ref": "CertificateId" } },
                        },
                    },
                })
                .to_string(),
            ),
            message: Some(
                "Invalid template: the provisioning template declares no AWS::IoT::Thing \
                 resource"
                    .to_string(),
            ),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let thing_name = format!("refused-{index}");
        let key = format!("refused/{index}.json");
        emulator
            .given_object(
                case,
                &key,
                &json!({ "SerialNumber": thing_name }).to_string(),
            )
            .await;
        let reply = emulator
            .start_task(
                &key,
                row.role_arn.as_deref().unwrap_or(&held_role),
                row.template_body.as_deref().unwrap_or(&well_formed),
            )
            .await;

        match &row.message {
            Some(message) => {
                assert_eq!(
                    (reply.status, reply.error_code.as_deref()),
                    (400, Some(INVALID_REQUEST)),
                    "case {case:?}: expected a refused start, got {} {:?}: {}",
                    reply.status,
                    reply.error_code,
                    reply.body
                );
                assert_eq!(
                    reply.text("message").as_deref(),
                    Some(message.as_str()),
                    "case {case:?}: expected {message:?}, got {}",
                    reply.body
                );
                assert!(
                    reply.text("taskId").is_none(),
                    "case {case:?}: a refused start mints no taskId, got {}",
                    reply.body
                );
                // Nothing was recorded, so nothing was provisioned either.
                let read = emulator.describe_thing(&thing_name).await;
                assert_eq!(
                    (read.status, read.error_code.as_deref()),
                    (404, Some(NOT_FOUND)),
                    "case {case:?}: a refused start provisions nothing, got {} {}",
                    read.status,
                    read.body
                );
            }
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: a value IAM answered nothing about grounds no refusal, \
                     got {} {}",
                    reply.status, reply.body
                );
                assert!(
                    reply.text("taskId").is_some(),
                    "case {case:?}: expected a taskId, got {}",
                    reply.body
                );
            }
        }
    }
}

/// A task whose input file the object store does not serve, or whose object is past the
/// ceiling this emulator reads for one task, is recorded and FAILS; and a task id nobody
/// started is a 404.
#[tokio::test]
async fn a_task_whose_input_file_cannot_be_read_fails() {
    let emulator = Emulator::new();
    emulator.given_bucket().await;
    let role_arn = emulator
        .given_role("the unreadable table", "bulk-reader", SERVICE_PRINCIPAL)
        .await;
    let body = template(
        serial_and_location(),
        json!({ "ThingName": { "Ref": "SerialNumber" } }),
    );
    // The sentence a task past the size ceiling reports, spelled out, with the ceiling the
    // crate publishes filled in.
    let oversized = format!(
        "Unable to read the input file: the maximum size of a bulk thing provisioning input \
         file is {MAX_INPUT_FILE_BYTES} bytes."
    );

    // (case, the key the task names, the object staged under it, the thing no line of it
    // provisions, the message the task reports)
    let rows: [(&str, &str, Option<String>, &str, &str); 3] = [
        (
            "a key the bucket holds no object under",
            "absent/1.json",
            None,
            "absent-1",
            UNREADABLE_INPUT_FILE,
        ),
        (
            "a key under a prefix nothing was ever staged under",
            "never/staged/2.json",
            None,
            "absent-2",
            UNREADABLE_INPUT_FILE,
        ),
        (
            "an object one byte past the ceiling one task reads",
            "oversized/3.json",
            Some(format!(
                "{}\n{}",
                json!({ "SerialNumber": "absent-3" }),
                "x".repeat(MAX_INPUT_FILE_BYTES)
            )),
            "absent-3",
            oversized.as_str(),
        ),
    ];

    let mut started: Vec<(&str, &str, &str, String)> = Vec::new();
    for (case, key, staged, thing_name, message) in &rows {
        if let Some(contents) = staged {
            emulator.given_object(case, key, contents).await;
        }
        let reply = emulator.start_task(key, &role_arn, &body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: an unreadable object is a task that fails, not a refused start, \
             got {} {}",
            reply.status, reply.body
        );
        let task_id = reply
            .text("taskId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a taskId, got {}", reply.body));
        started.push((case, thing_name, message, task_id));
    }

    tokio::time::sleep(REGISTRATION_SPAN + PAST_REGISTRATION).await;

    for (case, thing_name, message, task_id) in started {
        let described = emulator.describe_task(&task_id).await;
        assert_eq!(
            described.text("status").as_deref(),
            Some(TASK_STATUS_FAILED),
            "case {case:?}: expected {TASK_STATUS_FAILED}, got {}",
            described.body
        );
        assert_eq!(
            described.text("message").as_deref(),
            Some(message),
            "case {case:?}: expected {message:?}, got {}",
            described.body
        );
        assert_eq!(
            (
                described.number("successCount"),
                described.number("failureCount"),
                described.number("percentageProgress")
            ),
            (Some(0), Some(0), Some(0)),
            "case {case:?}: a run that was never performed provisioned nothing, got {}",
            described.body
        );
        let read = emulator.describe_thing(thing_name).await;
        assert_eq!(
            (read.status, read.error_code.as_deref()),
            (404, Some(NOT_FOUND)),
            "case {case:?}: a failed task provisions nothing, got {} {}",
            read.status,
            read.body
        );
    }

    // A task id the account holds nothing under is a 404, never a hollow 200.
    // (case, the id the read asks for)
    let unknown: [(&str, &str); 2] = [
        ("an id no start ever minted", "00000000-0000-0000-0000-000000000000"),
        ("an id of another form altogether", "task-nobody-started"),
    ];
    for (case, task_id) in unknown {
        let described = emulator.describe_task(task_id).await;
        assert_eq!(
            (described.status, described.error_code.as_deref()),
            (404, Some(NOT_FOUND)),
            "case {case:?}: expected a 404 for an unknown task, got {} {}",
            described.status,
            described.body
        );
        assert_eq!(
            described.text("message").as_deref(),
            Some(format!("Registration task {task_id} does not exist").as_str()),
            "case {case:?}: expected the service's missing-resource wording, got {}",
            described.body
        );
    }
}
