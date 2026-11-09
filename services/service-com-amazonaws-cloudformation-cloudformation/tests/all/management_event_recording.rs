// SPDX-License-Identifier: CC0-1.0

//! The AUDIT leg of this front door: every CloudFormation call it serves is a management
//! event.
//!
//! RULE: a service that serves a control-plane API call records that call in the account's
//! management-event log — the operation's own modelled name as `eventName`, this service's
//! `eventSource`, the Region and the caller it was served for, the members the request
//! carried as `requestParameters`, what a MUTATING call answered as `responseElements` (a
//! reporting call and a failed call have none, and `readOnly` is what tells a read from a
//! write), the FAULT SHAPE's name as `errorCode` with the message the caller was answered
//! with, and as `requestID` the id the CALLER was answered under — so a client holding a
//! response can find the record of that very call. A call this emulator has not implemented
//! answers 501 and is recorded as nothing: it was never served.
//!
//! The tables below span what the rule generalises over rather than the one call a recording
//! made: the kinds of call (create, read, list, write, delete), both outcomes (served and
//! refused), three fault shapes over three operations — the service's own `ValidationError`
//! refusal, whose fault shape is NOT the code the awsQuery wire carries, among them — and
//! both halves of the correlation (a success, whose id lives in `ResponseMetadata`, and a
//! refusal, whose id lives in the error envelope).
//!
//! Every call goes over the wire through the harness this crate's suites share (a
//! form-encoded awsQuery POST in, the XML envelope out); the records are read back out of
//! the SHARED log in the engine the services were built from — the one store CloudTrail's
//! `LookupEvents` reads. That CloudTrail half, over the real router, is
//! `core/router/tests/all/cloudformation_management_events.rs`.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_cloudformation_cloudformation::service::management_events::EVENT_SOURCE;
use service_common::management_events::{ManagementEvent, ManagementEventLog};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, member, router_with_engine, wait_for_the_transition, xml_text, REGION};
use crate::stack_lifecycle::iam;

/// The access key id the shared request builder signs with, which is what the recorded
/// caller of every call below is.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The template every stack here is created from: one resource of CloudFormation's OWN
/// type, so no other service is called and nothing but this service's own store is written.
const TEMPLATE: &str =
    r#"{"Resources":{"TheHandle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// A second template, for the update that has to change something about the stack.
const UPDATED_TEMPLATE: &str =
    r#"{"Resources":{"TheOtherHandle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// The form fields of one call, owned so a table row can hold them.
type Form = Vec<(String, String)>;

/// The emulator one case drives: the router the calls go through, and the engine whose
/// shared management-event log they are recorded in.
struct Audited {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Audited {
    /// The services this crate's harness mounts over ONE shared engine, as the production
    /// router mounts them.
    fn new() -> Self {
        let (router, engine) = router_with_engine();
        Self { router, engine }
    }

    /// One awsQuery call to CloudFormation.
    async fn call(&self, action: &str, members: &[(String, String)]) -> (u16, String) {
        call(&self.router, action, members).await
    }

    /// The records of THIS service's calls, oldest first, as the CloudTrail event document
    /// a lookup reports.
    ///
    /// Filtered by `eventSource`, which is how the log is read: the engine is shared, so
    /// the calls the suite makes to IAM to write a role are recorded there too, under IAM's
    /// own source.
    fn records(&self) -> Vec<Value> {
        let log = self.engine.state::<ManagementEventLog>();
        let held = log.lock();
        held.iter()
            .map(ManagementEvent::to_cloudtrail_event_json)
            .filter(|event| event.get("eventSource") == Some(&Value::String(EVENT_SOURCE.into())))
            .collect()
    }

    /// The records of the calls to `event_name`, oldest first.
    fn records_of(&self, event_name: &str) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|event| event.get("eventName") == Some(&Value::String(event_name.into())))
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
}

/// A string member of a record, or `None` when the record does not carry it.
fn text_at(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// A trust policy with one statement granting `principal` the right to assume the role.
fn trust(principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{principal}\"}},\"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// Create a role carrying `document` as its trust policy through IAM's own wire, answered
/// with the ARN IAM persisted for it.
async fn create_role(audited: &Audited, name: &str, document: &str) -> String {
    let (status, created) = iam(
        &audited.router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole({name:?}) must persist the role, got {status}: {created}"
    );
    xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: CreateRole({name:?}) reported no Arn: {created}"))
}

/// The members of a `CreateStack` for `name`, with one tag.
fn create_params(name: &str) -> Form {
    vec![
        member("StackName", name),
        member("TemplateBody", TEMPLATE),
        member("Tags.member.1.Key", "owner"),
        member("Tags.member.1.Value", "audit"),
    ]
}

/// RULE: every call this front door SERVES is recorded, whatever it answered — and one it
/// has not implemented is recorded as nothing.
///
/// The rows differ along the kind of call (create, read, list, write, delete), along the
/// outcome (served, refused), along which fault shape refused it, and along whether the
/// operation is implemented at all, which is what the rule generalises over.
#[tokio::test]
async fn every_served_call_is_recorded_and_an_unimplemented_one_is_not() {
    let audited = Audited::new();

    // (case, operation, fields, the HTTP status the caller sees, the recorded errorCode)
    let rows: Vec<(&str, &str, Form, u16, Option<&str>)> = vec![
        (
            "a create",
            "CreateStack",
            create_params("audit-served"),
            200,
            None,
        ),
        (
            "a read of one stack",
            "DescribeStacks",
            vec![member("StackName", "audit-served")],
            200,
            None,
        ),
        ("a read of every stack", "DescribeStacks", vec![], 200, None),
        ("a list", "ListStacks", vec![], 200, None),
        (
            // The service's own refusal: HTTP 400 under the wire code `ValidationError`,
            // whose fault shape is the synthetic `ValidationException`.
            "a read of a stack the Region holds none of",
            "DescribeStacks",
            vec![member("StackName", "audit-no-such-stack")],
            400,
            Some("ValidationException"),
        ),
        (
            // A MODELLED error, whose shape name and wire code agree.
            "a create of a name already taken",
            "CreateStack",
            create_params("audit-served"),
            400,
            Some("AlreadyExistsException"),
        ),
        (
            "a write",
            "SetStackPolicy",
            vec![
                member("StackName", "audit-served"),
                member(
                    "StackPolicyBody",
                    r#"{"Statement":[{"Effect":"Allow","Action":"Update:*","Principal":"*","Resource":"*"}]}"#,
                ),
            ],
            200,
            None,
        ),
        (
            "a template question that touches no stack",
            "ValidateTemplate",
            vec![member("TemplateBody", TEMPLATE)],
            200,
            None,
        ),
        (
            "a delete",
            "DeleteStack",
            vec![member("StackName", "audit-served")],
            200,
            None,
        ),
        (
            // Still a stub in this emulator: it answers 501, so nothing was served and
            // nothing is recorded. This row named `ActivateOrganizationsAccess` until that
            // one was implemented (`organizations_access.rs`), so it names a stub that IS
            // still one (`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/`).
            "an unimplemented operation",
            "ListStackRefactors",
            vec![],
            501,
            None,
        ),
    ];

    for (case, action, members, expected_status, expected_error) in &rows {
        let before = audited.records().len();
        let (status, body) = audited.call(action, members).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
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
            text_at(event, "/eventName").as_deref(),
            Some(*action),
            "case {case:?}: the record names the operation, got {event}"
        );
        assert_eq!(
            text_at(event, "/eventSource").as_deref(),
            Some(EVENT_SOURCE),
            "case {case:?}: the record names this service as its source, got {event}"
        );
        assert_eq!(
            text_at(event, "/awsRegion").as_deref(),
            Some(REGION),
            "case {case:?}: the record is in the Region the call was served in, got {event}"
        );
        assert_eq!(
            text_at(event, "/userIdentity/accessKeyId").as_deref(),
            Some(ACCESS_KEY_ID),
            "case {case:?}: the record names the credential the call was signed with, got \
             {event}"
        );
        assert!(
            text_at(event, "/userIdentity/arn").is_some_and(|arn| arn.contains(":iam::")),
            "case {case:?}: the record names the IAM entity the caller is, got {event}"
        );
        assert_eq!(
            text_at(event, "/errorCode").as_deref(),
            *expected_error,
            "case {case:?}: expected errorCode {expected_error:?}, got {event}"
        );
        assert_eq!(
            event.get("eventType").and_then(Value::as_str),
            Some("AwsApiCall"),
            "case {case:?}: a recorded API call is an AwsApiCall, got {event}"
        );
        if expected_error.is_some() {
            assert!(
                text_at(event, "/errorMessage").is_some_and(|message| !message.is_empty()),
                "case {case:?}: the record carries the message the caller was answered with, \
                 got {event}"
            );
            assert_eq!(
                event.get("responseElements"),
                Some(&Value::Null),
                "case {case:?}: a failed call answered nothing, got {event}"
            );
        }
    }
}

/// RULE: `readOnly` says whether the call only REPORTED state, a reporting call records no
/// `responseElements` at all while a mutating one records what it answered, and
/// `requestParameters` reproduces the members the request carried.
///
/// The rows span the kinds of call this service serves — a create, a read, a list, an
/// update and a delete — so the rule is tested over the class and not over one operation.
#[tokio::test]
async fn a_read_and_a_write_are_told_apart_by_what_the_record_carries() {
    let audited = Audited::new();
    let (status, created) = audited.call("CreateStack", &create_params("audit-kinds")).await;
    assert_eq!(
        status, 200,
        "setup: the create must be served, got {status} with {created}"
    );
    let stack_id = xml_text(&created, "StackId").expect("the create answers the stack id");
    // The update below is served only once the create has settled ("… is in
    // CREATE_IN_PROGRESS state and can not be updated"), so the suite waits past the
    // instant the create's terminal status is due.
    wait_for_the_transition().await;

    for (case, action, members) in [
        (
            "the read of one stack",
            "DescribeStacks",
            vec![member("StackName", "audit-kinds")],
        ),
        ("the list", "ListStacks", vec![]),
        (
            "the update",
            "UpdateStack",
            vec![
                member("StackName", "audit-kinds"),
                member("TemplateBody", UPDATED_TEMPLATE),
            ],
        ),
        (
            "the delete",
            "DeleteStack",
            vec![member("StackName", "audit-kinds")],
        ),
    ] {
        // The delete is served only once the update has settled ("cannot be deleted while in
        // status UPDATE_IN_PROGRESS"), by the same rule the create above is waited out under:
        // CloudFormation runs one operation at a time against a stack.
        if action == "DeleteStack" {
            wait_for_the_transition().await;
        }
        let (status, body) = audited.call(action, &members).await;
        assert_eq!(
            status, 200,
            "setup {case:?}: expected {action} to be served, got {status} with {body}"
        );
    }

    // (case, the operation, readOnly, whether the record carries responseElements)
    for (case, event_name, read_only, answers) in [
        ("the create", "CreateStack", false, true),
        ("the read of one stack", "DescribeStacks", true, false),
        ("the list", "ListStacks", true, false),
        ("the update", "UpdateStack", false, true),
        ("the delete", "DeleteStack", false, true),
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
    }

    // The create's record carries what the call ANSWERED — the id the stack is addressable
    // by afterwards — and the members the request carried, down to the tags.
    let create = audited.record_of("the create", "CreateStack");
    assert_eq!(
        text_at(&create, "/responseElements/stackId").as_deref(),
        Some(stack_id.as_str()),
        "the record carries the stack id the create answered, got {create}"
    );
    for (pointer, expected) in [
        ("/requestParameters/stackName", "audit-kinds"),
        ("/requestParameters/templateBody", TEMPLATE),
        ("/requestParameters/tags/0/key", "owner"),
        ("/requestParameters/tags/0/value", "audit"),
    ] {
        assert_eq!(
            text_at(&create, pointer).as_deref(),
            Some(expected),
            "case {pointer:?}: expected {expected:?} in the record, got {create}"
        );
    }

    // A read records the selection it was asked for, named as an event document names it.
    let read = audited.record_of("the read of one stack", "DescribeStacks");
    assert_eq!(
        text_at(&read, "/requestParameters/stackName").as_deref(),
        Some("audit-kinds"),
        "the record carries the member the request named, got {read}"
    );
}

/// RULE: a record's `requestID` is the id the CALLER was answered under — the
/// `ResponseMetadata/RequestId` of a success and the `<RequestId>` of an error envelope —
/// so a client holding a response finds the record of that very call, and two calls to one
/// operation are told apart by the call each record answered.
///
/// The rows are the issue's own sequence generalised: three refusals of a `RoleARN` this
/// account's IAM records show CloudFormation cannot assume, differing in the role's trust,
/// and two acceptances, one of them with a repaired role — each joined to its record by the
/// id the caller was answered under alone.
#[tokio::test]
async fn a_record_names_the_request_the_caller_was_answered_for() {
    let audited = Audited::new();

    // Every row names a role IAM holds. The refusals differ along WHY the trust withholds
    // the grant — another service principal, an explicit deny, an action that is not
    // assuming the role — and the acceptances along whether the role is the control's or a
    // repaired one.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a role trusted by another service only",
            trust("lambda.amazonaws.com"),
            false,
        ),
        (
            "a role whose trust denies cloudformation",
            trust("cloudformation.amazonaws.com").replace("Allow", "Deny"),
            false,
        ),
        (
            "a role trusted for another action",
            trust("cloudformation.amazonaws.com").replace("sts:AssumeRole", "s3:GetObject"),
            false,
        ),
        (
            "the control: a role cloudformation may assume",
            trust("cloudformation.amazonaws.com"),
            true,
        ),
        (
            "the repaired role",
            trust("cloudformation.amazonaws.com"),
            true,
        ),
    ];

    let mut answered: Vec<String> = Vec::new();
    for (index, (case, document, accepted)) in rows.iter().enumerate() {
        let role_arn = create_role(&audited, &format!("audit-role-{index}"), document).await;
        let stack_name = format!("audit-join-{index}");
        let mut members = create_params(&stack_name);
        members.push(member("RoleARN", &role_arn));
        let (status, body) = audited.call("CreateStack", &members).await;

        let event = audited
            .records()
            .pop()
            .unwrap_or_else(|| panic!("case {case:?}: the call must be recorded"));
        // The id the CALLER was answered under: `ResponseMetadata/RequestId` on the
        // success, the error envelope's own `<RequestId>` on the refusal.
        let caller_id = xml_text(&body, "RequestId").unwrap_or_else(|| {
            panic!("case {case:?}: the response must name the request it answers, got {body}")
        });
        assert_eq!(
            text_at(&event, "/requestID").as_deref(),
            Some(caller_id.as_str()),
            "case {case:?}: the record must name the request the caller was answered for \
             ({caller_id}), got {event}"
        );
        answered.push(caller_id);

        assert_eq!(
            text_at(&event, "/eventName").as_deref(),
            Some("CreateStack"),
            "case {case:?}: the record names the operation, got {event}"
        );
        // The role the call was made WITH is in the record either way, which is the join a
        // reader of event history makes to learn which role a call used.
        assert_eq!(
            text_at(&event, "/requestParameters/roleARN").as_deref(),
            Some(role_arn.as_str()),
            "case {case:?}: the record carries the RoleARN the request sent, got {event}"
        );

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a role cloudformation may assume must be accepted, got \
                 {status} with {body}"
            );
            let stack_id = xml_text(&body, "StackId").unwrap_or_else(|| {
                panic!("case {case:?}: the create must answer a stack id, got {body}")
            });
            assert_eq!(
                event.get("errorCode"),
                None,
                "case {case:?}: a served call carries no error code, got {event}"
            );
            assert_eq!(
                text_at(&event, "/responseElements/stackId").as_deref(),
                Some(stack_id.as_str()),
                "case {case:?}: the record carries the stack id the call answered, got {event}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a role cloudformation cannot assume must be refused, got \
                 {status} with {body}"
            );
            // What the CALLER saw: the awsQuery wire code of the refusal.
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected the wire code ValidationError, got {body}"
            );
            // What the RECORD carries: the fault shape's name, and the caller's own
            // message, built from the ARN the request named.
            assert_eq!(
                text_at(&event, "/errorCode").as_deref(),
                Some("ValidationException"),
                "case {case:?}: the record names the fault shape, got {event}"
            );
            assert_eq!(
                text_at(&event, "/errorMessage"),
                Some(format!("Role {role_arn} is invalid or cannot be assumed")),
                "case {case:?}: the record carries the message the caller saw, got {event}"
            );
            assert_eq!(
                event.get("responseElements"),
                Some(&Value::Null),
                "case {case:?}: a refused create minted nothing, got {event}"
            );
        }
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
