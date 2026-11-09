// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateImportTask`: starting an import from a CloudTrail Lake event
//! data store into a managed CloudWatch Logs log group.
//!
//! CloudWatch Logs and CloudTrail are mounted on ONE shared engine — the same assembly
//! `core/router` performs — so the source an import names is a REAL event data store
//! CloudTrail itself holds, created and deleted through CloudTrail's own API, and a create
//! that is accepted or refused here is decided by state no part of this suite handed to
//! CloudWatch Logs.
//!
//! The rule under test: `CreateImportTask` starts an import only from a data source the
//! service that OWNS it holds in a state it can be read from — a source no event data store
//! answers to is `ResourceNotFoundException`, one queued for deletion is
//! `InvalidOperationException` — and an accepted create persists the import `IN_PROGRESS`
//! with the managed destination log group it reports, so every value it hands back
//! (`importId`, `importDestinationArn`, `creationTime`) is read back through
//! `DescribeImportTasks` and `DescribeLogGroups`. The account runs at most the documented
//! three imports at a time, and the slot a finished one held is released by the CLOCK.
//!
//! Every case drives both services the way a client does — `POST /` with the service's own
//! `X-Amz-Target` through the router that mounts them — so what is asserted is what a client
//! reads off the wire.
//!
//! The table spans what the rule generalises over rather than one request: each form an
//! `importFilter` takes (absent, one bound, both bounds, both equal), several sources, a role
//! ARN of either form, and — for the refusals — a source ARN that names no store, one that
//! names a resource of another service, one that is not an ARN at all, and a store that is on
//! its way out.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, IMPORT_COMPLETION_DELAY_MS, IMPORT_STATUS_IN_PROGRESS, MAX_ACTIVE_IMPORT_TASKS,
};

/// The Region every request here is signed for, which is the Region the ARNs carry.
const REGION: &str = "eu-west-2";

/// The operation under test.
const OP: &str = "CreateImportTask";

/// The role an import runs under ("The ARN of the IAM role that grants CloudWatch Logs
/// permission to import from the CloudTrail Lake Event Data Store").
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/CloudWatchLogsImport";

/// The sentence CloudWatch Logs states the event-time window constraint in: "The
/// startEventTime must be less than or equal to endEventTime" (`CreateImportTask`'s own
/// documentation). Spelled out here rather than read from the crate, so the wording is pinned
/// by the test rather than re-asserted from the implementation's own constant.
const EVENT_TIME_RANGE: &str = "The startEventTime must be less than or equal to endEventTime.";

/// The sentence CloudWatch Logs states the source-state constraint in: "The specified source
/// must be in an ACTIVE state". Spelled out for the same reason as [`EVENT_TIME_RANGE`].
const SOURCE_NOT_ACTIVE: &str = "The specified source must be in an ACTIVE state.";

/// Room for a test's own scheduling on top of the span the import clock owes, so a sleep that
/// has to land past that instant does even on a loaded host.
const SCHEDULING_SLACK_MS: i64 = 1_000;

/// CloudWatch Logs and CloudTrail on one shared engine, reached over the wire.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One awsJson1_1 call to the service signing as `service` under the target `prefix`.
    async fn call(
        &self,
        service: &str,
        prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch Logs call: its status and body.
    async fn logs(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call("logs", "Logs_20140328", operation, body).await
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One CloudWatch Logs call that must be refused as a client error: its wire error code
    /// and the message it reports.
    async fn logs_refusal(&self, case: &str, operation: &str, body: &Value) -> (String, String) {
        let (status, parsed) = self.logs(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {operation} {body} must be refused with HTTP 400, got {status} {parsed}"
        );
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: an error body carries __type, got {parsed}"));
        let message = parsed
            .get("message")
            .or_else(|| parsed.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a refusal carries a message, got {parsed}"));
        (
            code.rsplit(['#', '/'])
                .next()
                .unwrap_or(code)
                .to_string(),
            message.to_string(),
        )
    }

    /// An event data store CloudTrail itself holds, answering its ARN — the source an import
    /// names. `termination_protection` decides whether it can later be deleted.
    async fn event_data_store(
        &self,
        case: &str,
        name: &str,
        termination_protection: bool,
    ) -> String {
        let (status, parsed) = self
            .call(
                "cloudtrail",
                "CloudTrail_20131101",
                "CreateEventDataStore",
                &json!({"Name": name, "TerminationProtectionEnabled": termination_protection}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEventDataStore {name} must succeed, got {status} {parsed}"
        );
        parsed["EventDataStoreArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: a created event data store carries an ARN, got {parsed}")
            })
            .to_string()
    }

    /// Queue the event data store under `arn` for deletion, which is what takes it out of the
    /// state an import may read from.
    async fn delete_event_data_store(&self, case: &str, arn: &str) {
        let (status, parsed) = self
            .call(
                "cloudtrail",
                "CloudTrail_20131101",
                "DeleteEventDataStore",
                &json!({"EventDataStore": arn}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEventDataStore {arn} must succeed, got {status} {parsed}"
        );
    }

    /// The import tasks CloudWatch Logs reports, in the order it reports them.
    async fn imports(&self, case: &str, request: &Value) -> Vec<Value> {
        let page = self.logs_ok(case, "DescribeImportTasks", request).await;
        page.get("imports")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: DescribeImportTasks reports imports, got {page}")
            })
            .clone()
    }

    /// The one import task reported under `import_id`.
    async fn reported_import(&self, case: &str, import_id: &str) -> Value {
        let reported = self.imports(case, &json!({"importId": import_id})).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: one import id reports one task, got {reported:?}"
        );
        reported.into_iter().next().expect("one task")
    }

    /// The log group CloudWatch Logs reports under `name`, or `None` when it holds none.
    async fn log_group(&self, case: &str, name: &str) -> Option<Value> {
        let page = self
            .logs_ok(
                case,
                "DescribeLogGroups",
                &json!({"logGroupNamePrefix": name}),
            )
            .await;
        page.get("logGroups")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: DescribeLogGroups reports logGroups, got {page}")
            })
            .iter()
            .find(|group| group.get("logGroupName").and_then(Value::as_str) == Some(name))
            .cloned()
    }
}

/// The id suffix of an event data store ARN, which is what the managed destination log group
/// is named after.
fn store_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// A string member of a reported structure.
fn text(case: &str, value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {value}"))
        .to_string()
}

/// One accepted-create case: the `importFilter` it sends, and the role it names.
struct Accepted {
    case: &'static str,
    /// The `importFilter` member, or `None` for a create that sends none.
    filter: Option<Value>,
    role_arn: &'static str,
}

/// The accepted creates, spanning each form an `ImportFilter` takes — both members are
/// optional, so a create may bound the event time at neither, one or both ends — and both
/// forms of role ARN a caller writes (a role at the account root and one under a path).
fn accepted_cases() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "no event-time filter",
            filter: None,
            role_arn: ROLE_ARN,
        },
        Accepted {
            case: "a start bound only",
            filter: Some(json!({"startEventTime": 1_600_000_000_000i64})),
            role_arn: ROLE_ARN,
        },
        Accepted {
            case: "an end bound only",
            filter: Some(json!({"endEventTime": 1_700_000_000_000i64})),
            role_arn: "arn:aws:iam::123456789012:role/service-role/CloudWatchLogsImport",
        },
        Accepted {
            case: "both bounds",
            filter: Some(json!({
                "startEventTime": 1_600_000_000_000i64,
                "endEventTime": 1_700_000_000_000i64,
            })),
            role_arn: ROLE_ARN,
        },
        Accepted {
            case: "both bounds at the same instant",
            filter: Some(json!({
                "startEventTime": 1_650_000_000_000i64,
                "endEventTime": 1_650_000_000_000i64,
            })),
            role_arn: ROLE_ARN,
        },
    ]
}

/// An accepted create persists the import and the managed log group it delivers into, and
/// every value it hands back is read back over the wire.
#[tokio::test]
async fn created_imports_are_persisted_and_read_back() {
    for accepted in accepted_cases() {
        let case = accepted.case;
        // One emulator per row, so the account's active-import slots (their own rule, below)
        // do not couple the rows of this table.
        let emulator = Emulator::new();
        let source = emulator
            .event_data_store(case, "lake-store-to-import-from", true)
            .await;

        let mut request = json!({
            "importSourceArn": source,
            "importRoleArn": accepted.role_arn,
        });
        if let Some(filter) = accepted.filter.clone() {
            request["importFilter"] = filter;
        }
        let created = emulator.logs_ok(case, OP, &request).await;

        let import_id = text(case, &created, "importId");
        // The modelled `ImportId` is 1..256 of `^[\-a-zA-Z0-9]+$`.
        assert!(
            (1..=256).contains(&import_id.len())
                && import_id
                    .chars()
                    .all(|character| character == '-' || character.is_ascii_alphanumeric()),
            "case {case:?}: expected a modelled importId, got {import_id:?}"
        );

        // "The ARN of the CloudWatch Logs log group created as the destination for the
        // imported events", named after the event data store the events come from.
        let destination_name = format!("/aws/cloudtrail/{}", store_id(&source));
        let expected_destination = log_group_arn(REGION, &destination_name, false);
        assert_eq!(
            text(case, &created, "importDestinationArn"),
            expected_destination,
            "case {case:?}: expected the managed destination ARN, got {created}"
        );

        // The destination log group is a REAL log group of this service: the create made it,
        // and every log-group read reports it.
        let group = emulator
            .log_group(case, &destination_name)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: the managed destination log group {destination_name} exists")
            });
        assert_eq!(
            text(case, &group, "logGroupArn"),
            expected_destination,
            "case {case:?}: expected the destination group's ARN, got {group}"
        );
        assert_eq!(
            text(case, &group, "arn"),
            log_group_arn(REGION, &destination_name, true),
            "case {case:?}: expected the destination group's wildcard ARN, got {group}"
        );

        let creation_time = created
            .get("creationTime")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: a create reports a creationTime, got {created}"));

        // The task the create persisted, read back through the reporting operation.
        let reported = emulator.reported_import(case, &import_id).await;
        for (member, expected) in [
            ("importId", import_id.clone()),
            ("importSourceArn", source.clone()),
            ("importDestinationArn", expected_destination.clone()),
            ("importStatus", IMPORT_STATUS_IN_PROGRESS.to_string()),
        ] {
            assert_eq!(
                text(case, &reported, member),
                expected,
                "case {case:?}: expected {member} {expected:?}, got {reported}"
            );
        }
        assert_eq!(
            reported.get("creationTime").and_then(Value::as_i64),
            Some(creation_time),
            "case {case:?}: expected the reported creationTime {creation_time}, got {reported}"
        );
        assert_eq!(
            reported.get("lastUpdatedTime").and_then(Value::as_i64),
            Some(creation_time),
            "case {case:?}: a task nothing has changed was last updated when it was created, \
             got {reported}"
        );
        // Nothing has been imported yet, which is what the statistics say.
        assert_eq!(
            reported
                .get("importStatistics")
                .and_then(|statistics| statistics.get("bytesImported"))
                .and_then(Value::as_i64),
            Some(0),
            "case {case:?}: expected no bytes imported yet, got {reported}"
        );
        // The window the create named, reported exactly as it was sent — and absent for a
        // create that named none.
        assert_eq!(
            reported.get("importFilter").cloned(),
            accepted.filter,
            "case {case:?}: expected the filter the create named, got {reported}"
        );
    }
}

/// Two imports from the SAME source deliver into the one managed log group that source's
/// imports are destined for, and each is its own task under its own id.
#[tokio::test]
async fn two_imports_from_one_source_share_the_managed_destination() {
    let case = "two imports from one source";
    let emulator = Emulator::new();
    let source = emulator.event_data_store(case, "lake-store-twice", true).await;
    let request = json!({"importSourceArn": source, "importRoleArn": ROLE_ARN});

    let first = emulator.logs_ok(case, OP, &request).await;
    let second = emulator.logs_ok(case, OP, &request).await;

    let (first_id, second_id) = (text(case, &first, "importId"), text(case, &second, "importId"));
    assert_ne!(
        first_id, second_id,
        "case {case:?}: each create mints its own import id, got {first_id:?} twice"
    );
    assert_eq!(
        text(case, &first, "importDestinationArn"),
        text(case, &second, "importDestinationArn"),
        "case {case:?}: both imports deliver into one managed group, got {first} and {second}"
    );

    let reported = emulator.imports(case, &json!({})).await;
    let mut ids: Vec<String> = reported
        .iter()
        .map(|import| text(case, import, "importId"))
        .collect();
    ids.sort();
    let mut expected = vec![first_id, second_id];
    expected.sort();
    assert_eq!(
        ids, expected,
        "case {case:?}: both tasks are reported, got {reported:?}"
    );

    // One group, not two: the second import delivered into the group the first one made.
    let destination_name = format!("/aws/cloudtrail/{}", store_id(&source));
    let page = emulator
        .logs_ok(case, "DescribeLogGroups", &json!({}))
        .await;
    let matching = page
        .get("logGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: DescribeLogGroups reports logGroups, got {page}"))
        .iter()
        .filter(|group| group.get("logGroupName").and_then(Value::as_str) == Some(&*destination_name))
        .count();
    assert_eq!(
        matching, 1,
        "case {case:?}: expected one managed destination group, got {matching}"
    );
}

/// One refused-create case: the request it sends, and the error code and message it must be
/// refused with.
struct Refused {
    case: &'static str,
    request: Value,
    code: &'static str,
    message: String,
}

/// The refusal a source no event data store answers to carries, in the sentence form this
/// service states a missing resource in.
fn source_not_found() -> String {
    "The specified import source does not exist.".to_string()
}

/// The refusal a member that is not an ARN at all carries, which is the validation-error
/// template this service refuses an unparsable ARN with.
fn unparsable(member: &str, value: &str) -> String {
    format!("1 validation error detected: Unable to parse {member} {value}")
}

/// A create whose source names no event data store the owning service holds, or whose
/// parameters are not what the operation takes, is refused — and persists nothing.
#[tokio::test]
async fn a_create_the_service_cannot_start_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    // One real store, so a case about another member is refused for that member rather than
    // for its source.
    let source = emulator
        .event_data_store("the real source", "lake-store-present", true)
        .await;

    let cases = vec![
        Refused {
            case: "a well-formed event data store ARN no store answers to",
            request: json!({
                "importSourceArn":
                    "arn:aws:cloudtrail:eu-west-2:123456789012:eventdatastore/\
                     11111111-1111-1111-1111-111111111111",
                "importRoleArn": ROLE_ARN,
            }),
            code: "ResourceNotFoundException",
            message: source_not_found(),
        },
        Refused {
            case: "a source ARN naming a resource of another service",
            request: json!({
                "importSourceArn": "arn:aws:s3:::import-source-bucket",
                "importRoleArn": ROLE_ARN,
            }),
            code: "ResourceNotFoundException",
            message: source_not_found(),
        },
        Refused {
            case: "a source ARN naming a log group of this service",
            request: json!({
                "importSourceArn":
                    "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/cloudtrail/not-a-source",
                "importRoleArn": ROLE_ARN,
            }),
            code: "ResourceNotFoundException",
            message: source_not_found(),
        },
        Refused {
            case: "a source that is not an ARN at all",
            request: json!({
                "importSourceArn": "lake-store-present",
                "importRoleArn": ROLE_ARN,
            }),
            code: "InvalidParameterException",
            message: unparsable("importSourceArn", "lake-store-present"),
        },
        Refused {
            case: "a role that is not an ARN at all",
            request: json!({
                "importSourceArn": source.clone(),
                "importRoleArn": "CloudWatchLogsImport",
            }),
            code: "InvalidParameterException",
            message: unparsable("importRoleArn", "CloudWatchLogsImport"),
        },
        Refused {
            case: "an event-time window whose end precedes its start",
            request: json!({
                "importSourceArn": source.clone(),
                "importRoleArn": ROLE_ARN,
                "importFilter": {
                    "startEventTime": 1_700_000_000_000i64,
                    "endEventTime": 1_600_000_000_000i64,
                },
            }),
            code: "InvalidParameterException",
            message: EVENT_TIME_RANGE.to_string(),
        },
    ];

    for refused in cases {
        let case = refused.case;
        let (code, message) = emulator.logs_refusal(case, OP, &refused.request).await;
        assert_eq!(
            code, refused.code,
            "case {case:?}: expected {}, got {code:?}: {message:?}",
            refused.code
        );
        assert_eq!(
            message, refused.message,
            "case {case:?}: expected {:?}, got {message:?}",
            refused.message
        );
    }

    // Not one of them minted a task or a managed destination group.
    let reported = emulator.imports("after the refusals", &json!({})).await;
    assert!(
        reported.is_empty(),
        "case \"after the refusals\": a refused create persists no import task, got {reported:?}"
    );
    let groups = emulator
        .logs_ok("after the refusals", "DescribeLogGroups", &json!({}))
        .await;
    assert_eq!(
        groups.get("logGroups").and_then(Value::as_array),
        Some(&vec![]),
        "case \"after the refusals\": a refused create creates no log group, got {groups}"
    );
}

/// A source the owning service holds, but has queued for deletion, is not in a state an
/// import may read from.
#[tokio::test]
async fn a_source_on_its_way_out_is_not_in_an_active_state() {
    let case = "a source queued for deletion";
    let emulator = Emulator::new();
    // Deletable: "TerminationProtectionEnabled must be set to False on the event data store".
    let source = emulator
        .event_data_store(case, "lake-store-on-its-way-out", false)
        .await;

    // While the store is there, the import starts.
    let accepted = emulator
        .logs_ok(
            "a source that is there",
            OP,
            &json!({"importSourceArn": source, "importRoleArn": ROLE_ARN}),
        )
        .await;
    let started = text("a source that is there", &accepted, "importId");

    emulator.delete_event_data_store(case, &source).await;

    let (code, message) = emulator
        .logs_refusal(
            case,
            OP,
            &json!({"importSourceArn": source, "importRoleArn": ROLE_ARN}),
        )
        .await;
    assert_eq!(
        code, "InvalidOperationException",
        "case {case:?}: expected InvalidOperationException, got {code:?}: {message:?}"
    );
    assert_eq!(
        message, SOURCE_NOT_ACTIVE,
        "case {case:?}: expected {SOURCE_NOT_ACTIVE:?}, got {message:?}"
    );

    // Only the import started while the source was there is held.
    let reported = emulator.imports(case, &json!({})).await;
    let ids: Vec<String> = reported
        .iter()
        .map(|import| text(case, import, "importId"))
        .collect();
    assert_eq!(
        ids,
        vec![started],
        "case {case:?}: the refused create persists no second task, got {reported:?}"
    );
}

/// The account runs at most the documented number of imports at once, and the slot a finished
/// one held is released by the CLOCK rather than by a call.
#[tokio::test]
async fn the_account_runs_at_most_the_documented_imports_at_once() {
    let case = "the account's active-import slots";
    let emulator = Emulator::new();

    // One source per import, so what refuses the create past the limit is the ACCOUNT's slots
    // and not anything the sources share.
    let mut sources: Vec<String> = Vec::new();
    for index in 0..=MAX_ACTIVE_IMPORT_TASKS {
        sources.push(
            emulator
                .event_data_store(case, &format!("lake-store-{index}"), true)
                .await,
        );
    }
    let create = |source: &str| json!({"importSourceArn": source, "importRoleArn": ROLE_ARN});

    for (index, source) in sources.iter().take(MAX_ACTIVE_IMPORT_TASKS).enumerate() {
        let accepted = emulator
            .logs_ok(&format!("{case}: import {index}"), OP, &create(source))
            .await;
        assert_eq!(
            accepted
                .get("importId")
                .and_then(Value::as_str)
                .map(str::is_empty),
            Some(false),
            "case {case:?}: import {index} must be accepted with an id, got {accepted}"
        );
    }

    let past_the_limit = sources
        .last()
        .expect("one source beyond the limit")
        .to_string();
    let (code, message) = emulator
        .logs_refusal(case, OP, &create(&past_the_limit))
        .await;
    assert_eq!(
        code, "ThrottlingException",
        "case {case:?}: expected ThrottlingException past the limit, got {code:?}: {message:?}"
    );
    let expected = format!(
        "the maximum of {MAX_ACTIVE_IMPORT_TASKS} active import(s) per account has been reached"
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    let reported = emulator.imports(case, &json!({})).await;
    assert_eq!(
        reported.len(),
        MAX_ACTIVE_IMPORT_TASKS,
        "case {case:?}: the refused create persists nothing, got {reported:?}"
    );
    // The other guard on an accepted create — that a destination group it would have to CREATE
    // is refused past the account's log-group quota — is not driven here: that quota is a
    // million groups, which no wire suite can stand up.

    // The imports the clock has since completed hold no slot, so the create it refused a
    // moment ago is accepted — without anything having been cancelled or deleted.
    tokio::time::sleep(std::time::Duration::from_millis(
        u64::try_from(IMPORT_COMPLETION_DELAY_MS + SCHEDULING_SLACK_MS)
            .expect("a span of the clock is not negative"),
    ))
    .await;
    let accepted = emulator
        .logs_ok(
            &format!("{case}: once the clock has completed the running imports"),
            OP,
            &create(&past_the_limit),
        )
        .await;
    assert_eq!(
        emulator
            .reported_import(case, &text(case, &accepted, "importId"))
            .await
            .get("importStatus")
            .and_then(Value::as_str),
        Some(IMPORT_STATUS_IN_PROGRESS),
        "case {case:?}: the import that took the freed slot is running"
    );
}
