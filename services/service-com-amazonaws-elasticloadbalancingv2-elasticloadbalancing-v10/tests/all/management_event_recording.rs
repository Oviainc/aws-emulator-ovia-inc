// SPDX-License-Identifier: CC0-1.0

//! The AUDIT leg of this front door: every call it serves is a management event.
//!
//! RULE: a service that serves a control-plane API call records that call in the account's
//! management-event log — the operation's own name as `eventName`, this service's
//! `eventSource`, the Region and the caller it was served for, the parameters the request
//! carried, the values a MUTATING call answered with as `responseElements` (a reporting
//! call has none, and `readOnly` is what tells the two apart), the FAULT SHAPE's name as
//! `errorCode` for a call that failed, and as `requestID` the id the CALLER was answered
//! under, so a client holding a response can find the record of that very call. A call this
//! emulator has not implemented answers 501 and is recorded as nothing: it was never
//! served.
//!
//! The tables below span what the rule generalises over rather than the one call a
//! recording made: the kinds of call (create, tag, read, delete), both outcomes (served and
//! refused), four fault shapes over five operations, and both halves of the correlation (a
//! success, whose id lives in `ResponseMetadata`, and a refusal, whose id lives in the
//! error envelope).
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the production
//! router uses, as a real awsQuery form POST (the harness this crate's suites share); the
//! records are read back out of the SHARED log in the engine the service was built from —
//! the one store CloudTrail's `LookupEvents` reads.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::service::management_events::EVENT_SOURCE;
use service_common::management_events::{ManagementEvent, ManagementEventLog};
use smithy_server_runtime::MultiServiceRouter;

// The wire helpers this crate's suites share, from a sibling module of this one test
// binary: one router over one engine, one form encoder, one field constructor and one
// element reader, so no two suites drift on how a request is addressed or a response read.
use super::provisioning_loop::{account, action, call, emulator, p, Reply, REGION};

/// The access key id the shared request builder signs with, which is what the recorded
/// caller of every call below is.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The form fields of one awsQuery request, owned so a table row can hold them.
type Form = Vec<(String, String)>;

/// The emulator one case drives: the router the calls go through, and the engine whose
/// shared management-event log they are recorded in.
struct Audited {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Audited {
    /// A router carrying only this service over one shared engine.
    fn new() -> Self {
        let (router, engine) = emulator();
        Self { router, engine }
    }

    /// One awsQuery call to `operation`, carrying `params` beside the `Action` and
    /// `Version` every envelope names.
    async fn call(&self, operation: &str, params: &[(String, String)]) -> Reply {
        let mut form = action(operation);
        form.extend(params.iter().cloned());
        call(&self.router, &form).await
    }

    /// Every record in the shared log, oldest first, as the CloudTrail event document a
    /// lookup reports.
    fn records(&self) -> Vec<Value> {
        self.with_log(ManagementEvent::to_cloudtrail_event_json)
    }

    /// The records of the calls to `event_name`, oldest first.
    fn records_of(&self, event_name: &str) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|event| event.get("eventName") == Some(&Value::String(event_name.to_string())))
            .collect()
    }

    /// The one record of the one call to `event_name`.
    fn record_of(&self, case: &str, event_name: &str) -> Value {
        let found = self.records_of(event_name);
        assert_eq!(
            found.len(),
            1,
            "case {case:?}: expected exactly one {event_name} record, got {found:?}"
        );
        found.into_iter().next().expect("checked just above")
    }

    /// `read` applied to every record in the shared log, oldest first.
    fn with_log<T>(&self, read: impl Fn(&ManagementEvent) -> T) -> Vec<T> {
        let log = self.engine.state::<ManagementEventLog>();
        let held = log.lock();
        held.iter().map(read).collect()
    }
}

/// A string member of a record, or `None` when the record does not carry it.
fn member(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The fields of a `CreateTargetGroup` for `name` on `port`, with one tag.
fn create_params(name: &str, port: &str) -> Form {
    vec![
        p("Name", name),
        p("Protocol", "HTTP"),
        p("Port", port),
        p("VpcId", "vpc-12345678"),
        p("Tags.member.1.Key", "owner"),
        p("Tags.member.1.Value", "audit"),
    ]
}

/// A well-formed target group ARN this service holds no target group for.
fn absent_target_group() -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/no-such-group/0123456789abcdef",
        account()
    )
}

/// A well-formed trust store ARN this service holds no trust store for.
fn absent_trust_store() -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:truststore/no-such-store/0123456789abcdef",
        account()
    )
}

/// Create the target group every case that needs a live one works against, and answer its
/// ARN.
async fn created_target_group(audited: &Audited, name: &str) -> String {
    let reply = audited
        .call("CreateTargetGroup", &create_params(name, "8080"))
        .await;
    assert_eq!(
        reply.status, 200,
        "setup: the create must be served, got {} with {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .expect("setup: the create answers the minted ARN")
}

/// RULE: every call this front door SERVES is recorded, whatever it answered — and one it
/// has not implemented is recorded as nothing.
///
/// The rows differ along the kind of call (create, tag, read, delete), along the outcome
/// (served, refused) and along whether the operation is implemented at all, which is what
/// the rule generalises over.
#[tokio::test]
async fn every_served_call_is_recorded_and_an_unimplemented_one_is_not() {
    let audited = Audited::new();
    let arn = created_target_group(&audited, "audit-served").await;

    // (case, operation, fields, the HTTP status the caller sees, the recorded errorCode)
    let rows: Vec<(&str, &str, Form, u16, Option<&str>)> = vec![
        ("a read of every target group", "DescribeTargetGroups", vec![], 200, None),
        (
            "a read of one target group",
            "DescribeTargetGroups",
            vec![p("TargetGroupArns.member.1", &arn)],
            200,
            None,
        ),
        (
            "a refused read",
            "DescribeTargetGroups",
            vec![p("TargetGroupArns.member.1", &absent_target_group())],
            400,
            Some("TargetGroupNotFoundException"),
        ),
        (
            "a refused create",
            "CreateTargetGroup",
            create_params("audit-served", "9090"),
            400,
            Some("DuplicateTargetGroupNameException"),
        ),
        (
            "a tag write",
            "AddTags",
            vec![
                p("ResourceArns.member.1", &arn),
                p("Tags.member.1.Key", "tier"),
                p("Tags.member.1.Value", "edge"),
            ],
            200,
            None,
        ),
        (
            "a refused tag write",
            "AddTags",
            vec![
                p("ResourceArns.member.1", &absent_target_group()),
                p("Tags.member.1.Key", "tier"),
                p("Tags.member.1.Value", "edge"),
            ],
            400,
            Some("TargetGroupNotFoundException"),
        ),
        (
            "a tag read",
            "DescribeTags",
            vec![p("ResourceArns.member.1", &arn)],
            200,
            None,
        ),
        (
            "a delete",
            "DeleteTargetGroup",
            vec![p("TargetGroupArn", &arn)],
            200,
            None,
        ),
        (
            // Still a stub in this emulator: it answers 501, so nothing was served and
            // nothing is recorded.
            "an unimplemented operation",
            "CreateTrustStore",
            vec![
                p("Name", "audit-stub"),
                p("CaCertificatesBundleS3Bucket", "audit-bucket"),
                p("CaCertificatesBundleS3Key", "bundle.pem"),
            ],
            501,
            None,
        ),
    ];

    for (case, operation, params, expected_status, expected_error) in &rows {
        let before = audited.records().len();
        let reply = audited.call(operation, params).await;
        assert_eq!(
            reply.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {}",
            reply.status, reply.body
        );
        let records = audited.records();
        if *expected_status == 501 {
            assert_eq!(
                records.len(),
                before,
                "case {case:?}: an unimplemented call records nothing, got {records:?}"
            );
            continue;
        }
        assert_eq!(
            records.len(),
            before + 1,
            "case {case:?}: expected exactly one new record, got {records:?}"
        );
        let event = records.last().expect("checked just above");
        assert_eq!(
            member(event, "/eventName").as_deref(),
            Some(*operation),
            "case {case:?}: the record names the operation, got {event}"
        );
        assert_eq!(
            member(event, "/eventSource").as_deref(),
            Some(EVENT_SOURCE),
            "case {case:?}: the record names this service as its source, got {event}"
        );
        assert_eq!(
            member(event, "/awsRegion").as_deref(),
            Some(REGION),
            "case {case:?}: the record is in the Region the call was served in, got {event}"
        );
        assert_eq!(
            member(event, "/userIdentity/accessKeyId").as_deref(),
            Some(ACCESS_KEY_ID),
            "case {case:?}: the record names the credential the call was signed with, got \
             {event}"
        );
        assert_eq!(
            member(event, "/errorCode").as_deref(),
            *expected_error,
            "case {case:?}: expected errorCode {expected_error:?}, got {event}"
        );
        assert_eq!(
            event.get("eventType").and_then(Value::as_str),
            Some("AwsApiCall"),
            "case {case:?}: a recorded API call is an AwsApiCall, got {event}"
        );
    }
}

/// RULE: `readOnly` says whether the call only REPORTED state, a reporting call records no
/// `responseElements` at all while a mutating one records what it answered, and
/// `requestParameters` reproduces the members the request carried.
#[tokio::test]
async fn a_read_and_a_write_are_told_apart_by_what_the_record_carries() {
    let audited = Audited::new();
    let arn = created_target_group(&audited, "audit-kinds").await;
    let reply = audited
        .call(
            "DescribeTargetGroups",
            &[p("TargetGroupArns.member.1", &arn)],
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "setup: the read must be served, got {} with {}",
        reply.status, reply.body
    );

    // (case, the operation, readOnly, whether the record carries responseElements)
    for (case, event_name, read_only, answers) in [
        ("the create", "CreateTargetGroup", false, true),
        ("the read", "DescribeTargetGroups", true, false),
    ] {
        let event = audited.record_of(case, event_name);
        assert_eq!(
            event.get("readOnly"),
            Some(&Value::Bool(read_only)),
            "case {case:?}: expected readOnly {read_only}, got {event}"
        );
        let elements = event.get("responseElements").unwrap_or(&Value::Null);
        assert_eq!(
            !elements.is_null(),
            answers,
            "case {case:?}: expected responseElements present={answers}, got {event}"
        );
        if answers {
            assert_eq!(
                member(&event, "/responseElements/targetGroups/0/targetGroupArn").as_deref(),
                Some(arn.as_str()),
                "case {case:?}: the record carries what the call answered, got {event}"
            );
        }
    }

    // The create's own parameters, as the request carried them: the members a client sent
    // are what a reader of event history reconciles against, down to the tags.
    let create = audited.record_of("the create", "CreateTargetGroup");
    for (pointer, expected) in [
        ("/requestParameters/name", "audit-kinds"),
        ("/requestParameters/protocol", "HTTP"),
        ("/requestParameters/vpcId", "vpc-12345678"),
        ("/requestParameters/tags/0/key", "owner"),
        ("/requestParameters/tags/0/value", "audit"),
    ] {
        assert_eq!(
            member(&create, pointer).as_deref(),
            Some(expected),
            "case {pointer:?}: expected {expected:?} in the record, got {create}"
        );
    }

    // A read records the selection it was asked for, named as an event document names it.
    let read = audited.record_of("the read", "DescribeTargetGroups");
    assert_eq!(
        member(&read, "/requestParameters/targetGroupArns/0").as_deref(),
        Some(arn.as_str()),
        "the record carries the member the request named, got {read}"
    );
}

/// RULE: the `errorCode` of a failed call's record is the FAULT SHAPE's name, which on a
/// query protocol is not the code the caller saw: the wire carries the shape's
/// `awsQueryError` code.
///
/// The rows span four fault shapes over five operations, and both kinds of call (a read and
/// a write), so the rule is tested over the class rather than over the one refusal a
/// recording named.
#[tokio::test]
async fn a_failed_call_records_the_fault_shape_name() {
    let audited = Audited::new();
    // The live target group the name-already-taken row collides with.
    created_target_group(&audited, "audit-faults").await;
    let absent = absent_target_group();
    let absent_store = absent_trust_store();

    // (case, operation, fields, the <Code> the caller sees, the recorded errorCode)
    let rows: Vec<(&str, &str, Form, &str, &str)> = vec![
        (
            "an absent target group ARN",
            "DescribeTargetGroups",
            vec![p("TargetGroupArns.member.1", &absent)],
            "TargetGroupNotFound",
            "TargetGroupNotFoundException",
        ),
        (
            "an absent target group name",
            "DescribeTargetGroups",
            vec![p("Names.member.1", "no-such-group")],
            "TargetGroupNotFound",
            "TargetGroupNotFoundException",
        ),
        (
            "a name already taken",
            "CreateTargetGroup",
            create_params("audit-faults", "9090"),
            "DuplicateTargetGroupName",
            "DuplicateTargetGroupNameException",
        ),
        (
            "health checks disabled on an instance target group",
            "CreateTargetGroup",
            {
                let mut params = create_params("audit-faults-two", "8080");
                params.push(p("HealthCheckEnabled", "false"));
                params
            },
            "InvalidConfigurationRequest",
            "InvalidConfigurationRequestException",
        ),
        (
            "an absent trust store",
            "DescribeTrustStores",
            vec![p("TrustStoreArns.member.1", &absent_store)],
            "TrustStoreNotFound",
            "TrustStoreNotFoundException",
        ),
        (
            "a tag write on an absent target group",
            "AddTags",
            vec![
                p("ResourceArns.member.1", &absent),
                p("Tags.member.1.Key", "tier"),
                p("Tags.member.1.Value", "edge"),
            ],
            "TargetGroupNotFound",
            "TargetGroupNotFoundException",
        ),
        (
            "a tag read on an absent target group",
            "DescribeTags",
            vec![p("ResourceArns.member.1", &absent)],
            "TargetGroupNotFound",
            "TargetGroupNotFoundException",
        ),
    ];

    for (case, operation, params, expected_wire_code, expected_recorded) in &rows {
        let reply = audited.call(operation, params).await;
        assert!(
            (400..500).contains(&reply.status),
            "case {case:?}: expected a client refusal, got {} with {}",
            reply.status,
            reply.body
        );
        let answered = reply.error_code();
        assert_eq!(
            answered, *expected_wire_code,
            "case {case:?}: expected the wire code {expected_wire_code:?}, got {answered:?} in \
             {}",
            reply.body
        );
        let event = audited
            .records_of(operation)
            .pop()
            .unwrap_or_else(|| panic!("case {case:?}: the refused call must be recorded"));
        assert_eq!(
            member(&event, "/errorCode").as_deref(),
            Some(*expected_recorded),
            "case {case:?}: expected errorCode {expected_recorded:?}, got {event}"
        );
        assert!(
            member(&event, "/errorMessage").is_some_and(|message| !message.is_empty()),
            "case {case:?}: the record carries the message the caller was answered with, got \
             {event}"
        );
        assert_eq!(
            event.get("responseElements"),
            Some(&Value::Null),
            "case {case:?}: a failed call answered nothing, got {event}"
        );
    }
}

/// RULE: a record's `requestID` is the id the CALLER was answered under — the
/// `ResponseMetadata/RequestId` of a success and the `<RequestId>` of an error envelope —
/// so a client holding a response finds the record of that very call, and two calls to one
/// operation are told apart by the call each record answered.
///
/// The rows span both places a response states that id and both outcomes, with two calls to
/// ONE operation (a success and a refusal) so the join is what distinguishes their records
/// rather than the operation name.
#[tokio::test]
async fn a_record_names_the_request_the_caller_was_answered_for() {
    let audited = Audited::new();
    let absent = absent_target_group();

    // (case, operation, fields, the status the caller sees)
    let rows: Vec<(&str, &str, Form, u16)> = vec![
        (
            "a successful create",
            "CreateTargetGroup",
            create_params("audit-ids", "8080"),
            200,
        ),
        (
            "a successful read",
            "DescribeTargetGroups",
            vec![p("Names.member.1", "audit-ids")],
            200,
        ),
        (
            "a refused read of the same operation",
            "DescribeTargetGroups",
            vec![p("TargetGroupArns.member.1", &absent)],
            400,
        ),
        (
            "a refused create of the same operation",
            "CreateTargetGroup",
            create_params("audit-ids", "9090"),
            400,
        ),
    ];

    let mut answered: Vec<String> = Vec::new();
    for (case, operation, params, expected_status) in &rows {
        let reply = audited.call(operation, params).await;
        assert_eq!(
            reply.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {}",
            reply.status, reply.body
        );
        let caller_id = reply.text("RequestId").unwrap_or_else(|| {
            panic!(
                "case {case:?}: the response must name the request it answers, got {}",
                reply.body
            )
        });
        let event = audited
            .records()
            .pop()
            .unwrap_or_else(|| panic!("case {case:?}: the call must be recorded"));
        assert_eq!(
            member(&event, "/requestID").as_deref(),
            Some(caller_id.as_str()),
            "case {case:?}: the record must name the request the caller was answered for \
             ({caller_id}), got {event}"
        );
        answered.push(caller_id);
    }

    // One id per call: a record found by a caller's own request id belongs to that call and
    // to no other.
    for (at, id) in answered.iter().enumerate() {
        assert_eq!(
            answered.iter().filter(|other| *other == id).count(),
            1,
            "call {at}: expected a request id of its own, got {answered:?}"
        );
    }
}
