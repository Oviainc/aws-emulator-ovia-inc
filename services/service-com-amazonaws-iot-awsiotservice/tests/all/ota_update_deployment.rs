// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `CreateOTAUpdate` and `GetOTAUpdate`: an OTA update is
//! a RECORD plus the AWS IoT job it deploys through, and targeting a device with it is
//! what creates that device's job execution.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 `POST /otaUpdates/{otaUpdateId}` and `GET /otaUpdates/{otaUpdateId}`
//! signed for the `iot` signing name — then reads the update back through
//! `GetOTAUpdate` and the shared job-execution store the IoT Jobs data plane serves
//! (`service_common::iot_job_executions`), which is where the executions a job creates
//! live.
//!
//! The rules under test:
//!
//! * a `CreateOTAUpdate` registers the update under the `otaUpdateId` the request
//!   names, mints its `otaUpdateArn` scoped to the signed region, deploys it through
//!   the job the OTA Update Manager derives from that id, answers the non-terminal
//!   creation status, and hands every member back unchanged from `GetOTAUpdate`;
//! * every DISTINCT thing the update targets holds exactly one `QUEUED` execution of
//!   that job afterwards;
//! * a request it refuses (a target that names no device, a target ARN of another
//!   resource type, a thing group, a file naming no location, an id already in use, or
//!   a fan-out past a ceiling) writes nothing at all;
//! * the creation the clock owes completes on its own, at the instant it was due.
//!
//! Each table spans the class the rule generalises over — how many targets and files
//! one update carries and in what form, which optional members the request carries,
//! and the region it is signed for — rather than one recorded request.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    CREATED_OTA_UPDATE_STATUS, MAX_JOB_TARGETS, MAX_OTA_UPDATE_FILES, MAX_OTA_UPDATE_ID_LEN,
    NEW_OTA_UPDATE_STATUS, OTA_UPDATE_CREATION_SPAN, OTA_UPDATE_JOB_ID_PREFIX,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{JobExecutionStore, MAX_THING_NAME_LEN, STATUS_QUEUED};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";
/// An IAM role ARN of the shape `CreateOTAUpdate$roleArn` is modelled with (len
/// 20..2048); the operation neither assumes nor resolves it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ota-update-role";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// The `otaUpdateInfo` a `GetOTAUpdate` reply carries.
    fn info(&self) -> &Value {
        self.body
            .get("otaUpdateInfo")
            .unwrap_or_else(|| panic!("a GetOTAUpdate reply carries otaUpdateInfo: {:?}", self.body))
    }

    /// One member of the reported `otaUpdateInfo`.
    fn info_member(&self, name: &str) -> Option<&Value> {
        self.info().get(name)
    }
}

/// One stored job execution, reduced to what a reader of the store sees.
#[derive(Debug, PartialEq, Eq)]
struct StoredExecution {
    job_id: String,
    status: String,
    version_number: i64,
    execution_number: i64,
    document: Option<String>,
    in_progress_timeout_minutes: Option<i64>,
    started: bool,
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can read the SHARED job-execution store the control
/// plane writes into.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
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
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/things/{thing_name}"),
            Some(&json!({})),
            REGION,
        )
        .await
    }

    async fn create_thing_group(&self, group_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/thing-groups/{group_name}"),
            Some(&json!({})),
            REGION,
        )
        .await
    }

    async fn create_ota_update(&self, ota_update_id: &str, request: &Value) -> Reply {
        self.create_ota_update_in(ota_update_id, request, REGION)
            .await
    }

    async fn create_ota_update_in(
        &self,
        ota_update_id: &str,
        request: &Value,
        region: &str,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/otaUpdates/{ota_update_id}"),
            Some(request),
            region,
        )
        .await
    }

    async fn get_ota_update(&self, ota_update_id: &str) -> Reply {
        self.get_ota_update_in(ota_update_id, REGION).await
    }

    async fn get_ota_update_in(&self, ota_update_id: &str, region: &str) -> Reply {
        self.call("GET", &format!("/otaUpdates/{ota_update_id}"), None, region)
            .await
    }

    /// What the SHARED store holds for `thing_name`, in enqueue order.
    fn executions_for(&self, thing_name: &str) -> Vec<StoredExecution> {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .executions_for(thing_name)
            .iter()
            .map(|record| StoredExecution {
                job_id: record.job_id.clone(),
                status: record.status.clone(),
                version_number: record.version_number,
                execution_number: record.execution_number,
                document: record.job_document.clone(),
                in_progress_timeout_minutes: record.in_progress_timeout_minutes,
                started: record.started_at.is_some(),
            })
            .collect()
    }
}

/// The thing ARN a target entry names a device by.
fn thing_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// The thing group ARN a target entry names a group by.
fn thing_group_arn(group_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/{group_name}")
}

/// A file located in S3, the form `OTAUpdateFile$fileLocation.s3Location` names.
fn s3_file(file_name: &str, file_version: &str) -> Value {
    json!({
        "fileName": file_name,
        "fileType": 0,
        "fileVersion": file_version,
        "fileLocation": {
            "s3Location": {"bucket": "firmware-images", "key": file_name, "version": "1"}
        }
    })
}

/// A file located in an IoT stream, the other form `fileLocation` names.
fn stream_file(file_name: &str, stream_id: &str, file_id: i64) -> Value {
    json!({
        "fileName": file_name,
        "fileLocation": {"stream": {"streamId": stream_id, "fileId": file_id}}
    })
}

/// One row of the create table.
struct CreateCase {
    case: &'static str,
    ota_update_id: &'static str,
    /// The things the update targets, each as a `thing/…` ARN; a name repeated here is
    /// ONE device targeted twice.
    targets: &'static [&'static str],
    /// The `files` member, always at least one (modelled `len 1..`).
    files: Vec<Value>,
    /// The `protocols` member, when the row carries one.
    protocols: Option<Vec<&'static str>>,
    /// `targetSelection`, when the row carries one.
    target_selection: Option<&'static str>,
    /// The `description` member, when the row carries one.
    description: Option<&'static str>,
    /// `awsJobTimeoutConfig.inProgressTimeoutInMinutes`, when the row carries one.
    in_progress_timeout_minutes: Option<i64>,
    /// `additionalParameters`, when the row carries any.
    additional_parameters: Option<Value>,
    /// `awsJobExecutionsRolloutConfig`, when the row carries one.
    rollout_config: Option<Value>,
    /// The region the row is signed for; its ARNs must be scoped to it.
    region: &'static str,
}

impl CreateCase {
    /// The request body this row sends.
    fn request(&self) -> Value {
        let mut request = json!({
            "targets": self.targets.iter().map(|name| thing_arn(name)).collect::<Vec<String>>(),
            "files": self.files.clone(),
            "roleArn": ROLE_ARN,
        });
        if let Some(protocols) = &self.protocols {
            request["protocols"] = json!(protocols);
        }
        if let Some(selection) = self.target_selection {
            request["targetSelection"] = json!(selection);
        }
        if let Some(description) = self.description {
            request["description"] = json!(description);
        }
        if let Some(minutes) = self.in_progress_timeout_minutes {
            request["awsJobTimeoutConfig"] = json!({"inProgressTimeoutInMinutes": minutes});
        }
        if let Some(parameters) = &self.additional_parameters {
            request["additionalParameters"] = parameters.clone();
        }
        if let Some(rollout) = &self.rollout_config {
            request["awsJobExecutionsRolloutConfig"] = rollout.clone();
        }
        request
    }
}

/// RULE: a `CreateOTAUpdate` records the update under the id the request names, deploys
/// it through the AWS IoT job derived from that id, and every distinct thing it targets
/// holds exactly one `QUEUED` execution of that job afterwards — while `GetOTAUpdate`
/// reports the stored update member for member.
#[tokio::test]
async fn an_ota_update_is_recorded_and_deployed_to_every_targeted_thing() {
    let emulator = Emulator::new();

    // The row at the `files` ceiling: the bound is a count, so it is exercised AT the
    // published maximum, with one past it covered by the refusal table below.
    let many_files: Vec<Value> = (0..MAX_OTA_UPDATE_FILES)
        .map(|index| s3_file(&format!("image-{index}.bin"), "1.0.0"))
        .collect();

    let cases: Vec<CreateCase> = vec![
        CreateCase {
            case: "one target, one S3 file, no optional member",
            ota_update_id: "ota-minimal",
            targets: &["sensor-a"],
            files: vec![s3_file("firmware.bin", "1.0.0")],
            protocols: None,
            target_selection: None,
            description: None,
            in_progress_timeout_minutes: None,
            additional_parameters: None,
            rollout_config: None,
            region: REGION,
        },
        CreateCase {
            case: "two targets, two files, every optional member",
            ota_update_id: "ota-full",
            targets: &["sensor-b", "sensor-c"],
            files: vec![
                s3_file("firmware.bin", "2.0.0"),
                stream_file("bootloader.bin", "stream-one", 1),
            ],
            protocols: Some(vec!["HTTP", "MQTT"]),
            target_selection: Some("CONTINUOUS"),
            description: Some("install firmware 2.0.0"),
            in_progress_timeout_minutes: Some(30),
            additional_parameters: Some(json!({"channel": "beta", "wave": "1"})),
            rollout_config: Some(json!({"maximumPerMinute": 10})),
            region: REGION,
        },
        CreateCase {
            case: "one device targeted twice is one execution",
            ota_update_id: "ota-duplicate-target",
            targets: &["sensor-d", "sensor-d"],
            files: vec![stream_file("firmware.bin", "stream-two", 0)],
            protocols: Some(vec!["MQTT"]),
            target_selection: Some("SNAPSHOT"),
            description: None,
            in_progress_timeout_minutes: Some(1),
            additional_parameters: None,
            rollout_config: Some(json!({
                "exponentialRate": {
                    "baseRatePerMinute": 1,
                    "incrementFactor": 2.0,
                    "rateIncreaseCriteria": {"numberOfNotifiedThings": 1}
                }
            })),
            region: REGION,
        },
        CreateCase {
            case: "a file carrying code-signing material",
            ota_update_id: "ota_signed_1",
            targets: &["sensor-e"],
            files: vec![json!({
                "fileName": "firmware.bin",
                "fileType": 255,
                "fileLocation": {
                    "s3Location": {"bucket": "firmware-images", "key": "signed/firmware.bin"}
                },
                "codeSigning": {"awsSignerJobId": "signer-job-1"},
                "attributes": {"board": "rev-c"}
            })],
            protocols: Some(vec!["HTTP"]),
            target_selection: None,
            description: Some("signed image"),
            in_progress_timeout_minutes: None,
            additional_parameters: Some(json!({"only": ""})),
            rollout_config: None,
            region: REGION,
        },
        CreateCase {
            case: "three targets, signed for another region",
            ota_update_id: "ota-another-region",
            targets: &["sensor-f", "sensor-g", "sensor-h"],
            files: vec![s3_file("firmware.bin", "3.0.0")],
            protocols: Some(vec!["MQTT", "HTTP"]),
            target_selection: Some("SNAPSHOT"),
            description: None,
            in_progress_timeout_minutes: Some(120),
            additional_parameters: None,
            rollout_config: None,
            region: "eu-west-1",
        },
        CreateCase {
            case: "files at the ceiling one update may carry",
            ota_update_id: "ota-many-files",
            targets: &["sensor-i"],
            files: many_files,
            protocols: None,
            target_selection: None,
            description: None,
            in_progress_timeout_minutes: None,
            additional_parameters: None,
            rollout_config: None,
            region: REGION,
        },
    ];

    for case in &cases {
        for thing_name in case.targets {
            let created = emulator.create_thing(thing_name).await;
            assert_eq!(
                created.status, 200,
                "case {:?}: the target device must be registrable, got {} {:?}",
                case.case, created.status, created.body
            );
        }

        let request = case.request();
        let reply = emulator
            .create_ota_update_in(case.ota_update_id, &request, case.region)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );

        let expected_job_id = format!("{OTA_UPDATE_JOB_ID_PREFIX}{}", case.ota_update_id);
        assert_eq!(
            reply.text("otaUpdateId").as_deref(),
            Some(case.ota_update_id),
            "case {:?}: expected the otaUpdateId it was created under, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("otaUpdateArn"),
            Some(format!(
                "arn:aws:iot:{}:{ACCOUNT_ID}:otaupdate/{}",
                case.region, case.ota_update_id
            )),
            "case {:?}: expected the OTA update ARN scoped to the signed region, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("awsIotJobId").as_deref(),
            Some(expected_job_id.as_str()),
            "case {:?}: expected the job id derived from the update id, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("awsIotJobArn"),
            Some(format!(
                "arn:aws:iot:{}:{ACCOUNT_ID}:job/{expected_job_id}",
                case.region
            )),
            "case {:?}: expected the deployed job's ARN scoped to the signed region, got {:?}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.text("otaUpdateStatus").as_deref(),
            Some(NEW_OTA_UPDATE_STATUS),
            "case {:?}: expected the creation status an update is answered in, got {:?}",
            case.case,
            reply.body
        );

        // ROUND TRIP: every member the create supplied comes back from the reader,
        // together with the identity the create minted.
        let read = emulator.get_ota_update_in(case.ota_update_id, case.region).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the created update to be readable, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            read.info_member("otaUpdateId"),
            Some(&json!(case.ota_update_id)),
            "case {:?}: expected the stored id, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("targets"),
            Some(&request["targets"]),
            "case {:?}: expected the targets it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("otaUpdateFiles"),
            Some(&request["files"]),
            "case {:?}: expected the files it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("protocols"),
            request.get("protocols"),
            "case {:?}: expected the protocols it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("targetSelection"),
            request.get("targetSelection"),
            "case {:?}: expected the targetSelection it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("description"),
            request.get("description"),
            "case {:?}: expected the description it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("additionalParameters"),
            request.get("additionalParameters"),
            "case {:?}: expected the additionalParameters it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("awsJobExecutionsRolloutConfig"),
            request.get("awsJobExecutionsRolloutConfig"),
            "case {:?}: expected the rollout config it was created with, got {:?}",
            case.case,
            read.body
        );
        assert_eq!(
            read.info_member("awsIotJobId"),
            Some(&json!(expected_job_id)),
            "case {:?}: expected the deployed job's id, got {:?}",
            case.case,
            read.body
        );
        assert!(
            read.info_member("creationDate").is_some_and(Value::is_number),
            "case {:?}: expected a creationDate timestamp, got {:?}",
            case.case,
            read.body
        );
        assert!(
            read.info_member("errorInfo").is_none(),
            "case {:?}: a creation that did not fail reports no errorInfo, got {:?}",
            case.case,
            read.body
        );

        // The update's targets ARE its job's: one QUEUED execution per DISTINCT thing,
        // carrying the per-device timeout the request named and no job document.
        let mut distinct: Vec<&str> = case.targets.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        for thing_name in distinct {
            assert_eq!(
                emulator.executions_for(thing_name),
                vec![StoredExecution {
                    job_id: expected_job_id.clone(),
                    status: STATUS_QUEUED.to_string(),
                    version_number: 1,
                    execution_number: 1,
                    document: None,
                    in_progress_timeout_minutes: case.in_progress_timeout_minutes,
                    started: false,
                }],
                "case {:?}: expected one queued execution of {expected_job_id} on \
                 {thing_name}, got {:?}",
                case.case,
                emulator.executions_for(thing_name)
            );
        }
    }
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    ota_update_id: String,
    /// The `targets` member, verbatim.
    targets: Value,
    /// The `files` member, verbatim.
    files: Value,
    status: u16,
    code: &'static str,
    /// A fragment the refusal's message must carry, so the caller learns which
    /// constraint it violated.
    message_carries: String,
}

/// RULE: a `CreateOTAUpdate` this operation refuses writes NOTHING — no update, no job
/// and no execution for the targets that did resolve.
#[tokio::test]
async fn a_refused_create_records_no_update_and_no_execution() {
    let emulator = Emulator::new();

    let known_group = "fleet-known";
    assert_eq!(
        emulator.create_thing_group(known_group).await.status,
        200,
        "the thing group a case targets must be registrable"
    );

    let one_file = json!([s3_file("firmware.bin", "1.0.0")]);
    let too_long_name = "x".repeat(MAX_THING_NAME_LEN + 1);
    let over_target_limit: Vec<String> = (0..=MAX_JOB_TARGETS)
        .map(|index| thing_arn(&format!("sensor-over-{index}")))
        .collect();
    let over_file_limit: Vec<Value> = (0..=MAX_OTA_UPDATE_FILES)
        .map(|index| s3_file(&format!("image-{index}.bin"), "1.0.0"))
        .collect();

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a target ARN of another resource type",
            ota_update_id: "ota-bad-type".to_string(),
            targets: json!([format!(
                "arn:aws:iot:{REGION}:{ACCOUNT_ID}:billinggroup/fleet"
            )]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "billinggroup".to_string(),
        },
        RefusalCase {
            case: "a target that is not an ARN at all",
            ota_update_id: "ota-not-an-arn".to_string(),
            targets: json!(["sensor-a"]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "is not an IoT thing or thing group ARN".to_string(),
        },
        RefusalCase {
            case: "a thing target whose name no request could address",
            ota_update_id: "ota-unaddressable".to_string(),
            targets: json!([thing_arn(&too_long_name)]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: format!("1..{MAX_THING_NAME_LEN}"),
        },
        RefusalCase {
            case: "a thing group the registry does not hold",
            ota_update_id: "ota-unknown-group".to_string(),
            targets: json!([thing_group_arn("fleet-unknown")]),
            files: one_file.clone(),
            status: 404,
            code: "ResourceNotFoundException",
            message_carries: "Thing group fleet-unknown does not exist".to_string(),
        },
        RefusalCase {
            case: "a thing group the registry does hold, whose members are unknown",
            ota_update_id: "ota-known-group".to_string(),
            targets: json!([thing_group_arn(known_group)]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "thing group target cannot be expanded".to_string(),
        },
        RefusalCase {
            case: "a file naming no location at all",
            ota_update_id: "ota-no-location".to_string(),
            targets: json!([thing_arn("sensor-a")]),
            files: json!([{"fileName": "firmware.bin"}]),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "files[0].fileLocation".to_string(),
        },
        RefusalCase {
            case: "a later file whose location names neither a stream nor an s3Location",
            ota_update_id: "ota-empty-location".to_string(),
            targets: json!([thing_arn("sensor-a")]),
            files: json!([s3_file("firmware.bin", "1.0.0"), {"fileLocation": {}}]),
            status: 400,
            code: "InvalidRequestException",
            message_carries: "files[1].fileLocation".to_string(),
        },
        RefusalCase {
            case: "one target past the ceiling one update may name",
            ota_update_id: "ota-over-targets".to_string(),
            targets: json!(over_target_limit),
            files: one_file.clone(),
            status: 410,
            code: "LimitExceededException",
            message_carries: format!("at most {MAX_JOB_TARGETS} targets"),
        },
        RefusalCase {
            case: "one file past the ceiling one update may carry",
            ota_update_id: "ota-over-files".to_string(),
            targets: json!([thing_arn("sensor-a")]),
            files: json!(over_file_limit),
            status: 410,
            code: "LimitExceededException",
            message_carries: format!("at most {MAX_OTA_UPDATE_FILES} files"),
        },
        RefusalCase {
            case: "an id one character past what the deployed job's id may be",
            ota_update_id: "o".repeat(MAX_OTA_UPDATE_ID_LEN + 1),
            targets: json!([thing_arn("sensor-a")]),
            files: one_file.clone(),
            status: 400,
            code: "InvalidRequestException",
            message_carries: format!("at most {MAX_OTA_UPDATE_ID_LEN} characters"),
        },
    ];

    for case in &cases {
        let request = json!({
            "targets": case.targets.clone(),
            "files": case.files.clone(),
            "roleArn": ROLE_ARN,
        });
        let reply = emulator.create_ota_update(&case.ota_update_id, &request).await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected {}, got {} {:?}",
            case.case, case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(case.code),
            "case {:?}: expected {}, got {:?} {:?}",
            case.case,
            case.code,
            reply.error_code,
            reply.body
        );
        assert!(
            reply.message().contains(&case.message_carries),
            "case {:?}: expected a message carrying {:?}, got {:?}",
            case.case,
            case.message_carries,
            reply.body
        );

        // Nothing was written: the update does not exist and no device was targeted.
        let read = emulator.get_ota_update(&case.ota_update_id).await;
        assert_eq!(
            read.status, 404,
            "case {:?}: the refused create must record no update, got {} {:?}",
            case.case, read.status, read.body
        );
        assert_eq!(
            emulator.executions_for("sensor-a").len(),
            0,
            "case {:?}: the refused create must queue no execution, got {:?}",
            case.case,
            emulator.executions_for("sensor-a")
        );
    }
}

/// RULE: the `otaUpdateId` identifies the update, so a second create under a live id is
/// a conflict and leaves the recorded update exactly as it was.
#[tokio::test]
async fn a_second_create_under_a_live_id_conflicts_and_changes_nothing() {
    let emulator = Emulator::new();
    let ota_update_id = "ota-conflict";

    let first = json!({
        "targets": [thing_arn("sensor-first")],
        "files": [s3_file("firmware.bin", "1.0.0")],
        "roleArn": ROLE_ARN,
    });
    assert_eq!(
        emulator.create_ota_update(ota_update_id, &first).await.status,
        200,
        "the first create of {ota_update_id} must be accepted"
    );

    let second = json!({
        "targets": [thing_arn("sensor-second")],
        "files": [s3_file("other.bin", "9.9.9")],
        "roleArn": ROLE_ARN,
    });
    let reply = emulator.create_ota_update(ota_update_id, &second).await;
    assert_eq!(
        reply.status, 409,
        "expected 409 for a repeat create, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("ResourceAlreadyExistsException"),
        "expected ResourceAlreadyExistsException, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.body.get("resourceId"),
        Some(&json!(ota_update_id)),
        "expected the conflict to name the resource that exists, got {:?}",
        reply.body
    );

    let read = emulator.get_ota_update(ota_update_id).await;
    assert_eq!(
        read.info_member("targets"),
        Some(&first["targets"]),
        "expected the FIRST update's targets to survive the conflict, got {:?}",
        read.body
    );
    assert_eq!(
        emulator.executions_for("sensor-second").len(),
        0,
        "expected the refused create to target nothing, got {:?}",
        emulator.executions_for("sensor-second")
    );
    let unknown = emulator.get_ota_update("ota-never-created").await;
    assert_eq!(
        unknown.status, 404,
        "expected 404 for an id nothing created, got {} {:?}",
        unknown.status, unknown.body
    );
}

/// RULE: an update answered in the non-terminal creation status is owed its completion
/// by the clock — every reader after the due instant reports `CREATE_COMPLETE`, stamped
/// with the instant of the transition rather than the instant of the read.
#[tokio::test]
async fn the_creation_the_clock_owes_completes_on_its_own() {
    let emulator = Emulator::new();
    let ota_update_id = "ota-transition";

    let request = json!({
        "targets": [thing_arn("sensor-transition")],
        "files": [s3_file("firmware.bin", "1.0.0")],
        "roleArn": ROLE_ARN,
    });
    let started = Instant::now();
    let created = emulator.create_ota_update(ota_update_id, &request).await;
    assert_eq!(
        created.status, 200,
        "expected 200, got {} {:?}",
        created.status, created.body
    );

    // Before the span is up the update is still being created, so a caller acting
    // straight after the create sees the transient status.
    let early = emulator.get_ota_update(ota_update_id).await;
    if started.elapsed() < OTA_UPDATE_CREATION_SPAN {
        assert_eq!(
            early.info_member("otaUpdateStatus"),
            Some(&json!(NEW_OTA_UPDATE_STATUS)),
            "expected the transient creation status inside the span, got {:?}",
            early.body
        );
    }

    tokio::time::sleep(OTA_UPDATE_CREATION_SPAN + Duration::from_millis(250)).await;

    let settled = emulator.get_ota_update(ota_update_id).await;
    assert_eq!(
        settled.info_member("otaUpdateStatus"),
        Some(&json!(CREATED_OTA_UPDATE_STATUS)),
        "expected the completed creation status past the span, got {:?}",
        settled.body
    );
    let last_modified = settled.info_member("lastModifiedDate").cloned();
    let creation_date = settled.info_member("creationDate").cloned();
    assert!(
        last_modified
            .as_ref()
            .zip(creation_date.as_ref())
            .is_some_and(|(modified, created)| {
                modified.as_f64().unwrap_or_default() >= created.as_f64().unwrap_or_default()
            }),
        "expected the transition to move lastModifiedDate on from creationDate, got {:?}",
        settled.body
    );

    // The transition happened once: a second read reports the same instant, not the
    // instant of the read.
    let again = emulator.get_ota_update(ota_update_id).await;
    assert_eq!(
        again.info_member("lastModifiedDate").cloned(),
        last_modified,
        "expected a stable lastModifiedDate across reads, got {:?}",
        again.body
    );
    assert_eq!(
        again.info_member("otaUpdateStatus"),
        Some(&json!(CREATED_OTA_UPDATE_STATUS)),
        "expected the completed status to stay, got {:?}",
        again.body
    );
}
