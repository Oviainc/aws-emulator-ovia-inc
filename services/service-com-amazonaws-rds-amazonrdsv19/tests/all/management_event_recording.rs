// SPDX-License-Identifier: CC0-1.0

//! The AUDIT leg of this front door: every call it serves is a management event.
//!
//! RULE: a service that serves a control-plane API call records that call in the
//! account's management-event log — the operation's own name as `eventName`, this
//! service's `eventSource`, the Region and caller it was served for, the parameters the
//! request carried with every member the model declares `@sensitive` replaced by
//! CloudTrail's fixed redaction literal, the values a MUTATING call answered with as
//! `responseElements` (a reporting call has none, and `readOnly` is what tells the two
//! apart), and, for a failed call, the FAULT SHAPE's name as `errorCode` with the
//! message the caller was answered with. A call this emulator has not implemented
//! answers 501 and is recorded as nothing: it was never served.
//!
//! The tables below span what the rule generalises over rather than the one call a
//! recording made: the kinds of call (create, modify, tag, read, delete), both outcomes
//! (served and refused), every sensitive member this service's model declares and each
//! operation that carries one, and both relations an error's wire code can have to its
//! shape name (a `awsQueryError` code that differs from the shape name, and one that is
//! the shape name).
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the production
//! router uses, as a real awsQuery form POST; the records are read back out of the SHARED
//! log in the engine the service was built from — the one store CloudTrail's
//! `LookupEvents` reads (the end-to-end leg through that operation is
//! `core/router/tests/all/rds_management_events.rs`).

use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use serde_json::Value;
use service_common::management_events::{ManagementEvent, ManagementEventLog};
use service_com_amazonaws_rds_amazonrdsv19::service::management_events::EVENT_SOURCE;
use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and so the Region every record reports.
const REGION: &str = "us-west-2";

/// The access key id every call below is signed with: a long-term key, which is what
/// makes the recorded caller this emulator's own IAM user.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// The literal CloudTrail stores in place of a sensitive request member's value.
const REDACTED: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

/// Wait out the `creating` -> `available` transition this service owes a new resource,
/// plus slack for a loaded test machine, so a call that needs a settled resource is not
/// refused for its state.
fn wait_for_settle() {
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
}

/// The emulator one case drives: the router the calls go through, and the engine whose
/// shared management-event log they are recorded in.
struct Audited {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Audited {
    /// A router carrying only this service over one shared engine.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Verifying signatures is the emulator's concern, not this service's; these tests
        // sign nothing, so the check is off and the scope header is read for the key id
        // and the Region alone.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// One awsQuery call, as the HTTP status and the XML body it answered.
    async fn call(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2014-10-31");
        for (name, value) in params {
            assert!(
                !value.contains(['&', '=', '+', '%']),
                "test parameter {name} would need form escaping: {value:?}"
            );
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(value);
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://rds.amazonaws.com/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("user-agent", "aws-sdk-rust/audit-suite")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={ACCESS_KEY_ID}/20260101/{REGION}/rds/aws4_request, \
                     SignedHeaders=host, Signature=unverified"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = String::from_utf8(response.body().to_vec()).expect("a UTF-8 body");
        (status, body)
    }

    /// Every record in the shared log, oldest first, as the CloudTrail event document a
    /// lookup reports.
    fn records(&self) -> Vec<Value> {
        self.with_log(|event| event.to_cloudtrail_event_json())
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

/// The text of the first `<name>` element of an XML body, or `None`.
fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The `<Code>` of an awsQuery error body.
fn wire_code(body: &str) -> String {
    element(body, "Code").unwrap_or_default()
}

/// A string member of a record, or `None` when the record does not carry it.
fn member(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// One row of the served-call table: the case, the action, its parameters, the HTTP
/// status the caller sees and the `errorCode` the record must carry.
type ServedRow = (
    &'static str,
    &'static str,
    Params,
    u16,
    Option<&'static str>,
);

/// One row of a table that names a recorded MEMBER and the value it is about: the case,
/// the action, its parameters, the member's recorded name and that value.
type MemberRow = (
    &'static str,
    &'static str,
    Params,
    &'static str,
    &'static str,
);

/// The three required members of a `CreateDBInstance`, plus the master password this
/// suite's instances are created with.
fn create_params(identifier: &str, password: &str) -> Params {
    vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", "mysql".to_string()),
        ("DBInstanceClass", "db.t3.medium".to_string()),
        ("MasterUsername", "admin".to_string()),
        ("MasterUserPassword", password.to_string()),
    ]
}

/// `[(name, value)]` as the call helper takes it.
fn borrowed<'a>(params: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// RULE: every call this front door SERVES is recorded, whatever it answered — and one it
/// has not implemented is recorded as nothing.
///
/// The rows differ along the kind of call (create, modify, tag, read, delete), along the
/// outcome (served, refused) and along whether the operation is implemented at all, which
/// is what the rule generalises over.
#[tokio::test]
async fn every_served_call_is_recorded_and_an_unimplemented_one_is_not() {
    let audited = Audited::new();
    let created = create_params("audit-instance", "sup3r-s3cret");
    let (status, body) = audited.call("CreateDBInstance", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "setup: the create must be served, got {status} with {body}"
    );
    wait_for_settle();

    // (case, action, parameters, the HTTP status the caller sees, the recorded errorCode)
    let rows: Vec<ServedRow> = vec![
        (
            "a modify",
            "ModifyDBInstance",
            vec![
                ("DBInstanceIdentifier", "audit-instance".to_string()),
                ("BackupRetentionPeriod", "7".to_string()),
            ],
            200,
            None,
        ),
        (
            "a tag write",
            "AddTagsToResource",
            vec![
                (
                    "ResourceName",
                    format!("arn:aws:rds:{REGION}:000000000000:db:audit-instance"),
                ),
                ("Tags.Tag.1.Key", "owner".to_string()),
                ("Tags.Tag.1.Value", "audit".to_string()),
            ],
            200,
            None,
        ),
        (
            "a read",
            "DescribeDBInstances",
            vec![("DBInstanceIdentifier", "audit-instance".to_string())],
            200,
            None,
        ),
        (
            "a refused read",
            "DescribeDBInstances",
            vec![("DBInstanceIdentifier", "no-such-instance".to_string())],
            404,
            Some("DBInstanceNotFoundFault"),
        ),
        (
            "a refused create",
            "CreateDBInstance",
            create_params("audit-instance", "another-s3cret"),
            400,
            Some("DBInstanceAlreadyExistsFault"),
        ),
        (
            "a delete",
            "DeleteDBInstance",
            vec![
                ("DBInstanceIdentifier", "audit-instance".to_string()),
                ("SkipFinalSnapshot", "true".to_string()),
            ],
            200,
            None,
        ),
        (
            // Still a stub in this emulator: it answers 501, so nothing was served and
            // nothing is recorded.
            "an unimplemented operation",
            "ModifyDBSnapshot",
            vec![("DBSnapshotIdentifier", "audit-snapshot".to_string())],
            501,
            None,
        ),
    ];

    for (case, action, params, expected_status, expected_error) in &rows {
        let before = audited.records().len();
        let (status, body) = audited.call(action, &borrowed(params)).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        let records = audited.records();
        let recorded = records.len();
        if *expected_status == 501 {
            assert_eq!(
                recorded, before,
                "case {case:?}: an unimplemented call records nothing, got {records:?}"
            );
            continue;
        }
        assert_eq!(
            recorded,
            before + 1,
            "case {case:?}: expected exactly one new record, got {records:?}"
        );
        let event = records.last().expect("checked just above");
        assert_eq!(
            member(event, "/eventName").as_deref(),
            Some(*action),
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
            "case {case:?}: the record names the credential the call was signed with, \
             got {event}"
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

/// RULE: `readOnly` says whether the call only REPORTED state, and a reporting call
/// records no `responseElements` at all while a mutating one records what it answered.
#[tokio::test]
async fn a_read_and_a_write_are_told_apart_by_what_the_record_carries() {
    let audited = Audited::new();
    let created = create_params("audit-kinds", "sup3r-s3cret");
    let (status, body) = audited.call("CreateDBInstance", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "setup: the create must be served, got {status} with {body}"
    );
    let (status, body) = audited
        .call(
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", "audit-kinds")],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the read must be served, got {status} with {body}"
    );

    // (case, the operation, readOnly, whether the record carries responseElements)
    for (case, event_name, read_only, answers) in [
        ("the create", "CreateDBInstance", false, true),
        ("the read", "DescribeDBInstances", true, false),
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
                member(&event, "/responseElements/dBInstance/dBInstanceIdentifier").as_deref(),
                Some("audit-kinds"),
                "case {case:?}: the record carries what the call answered, got {event}"
            );
        }
    }

    // A read records a read: the record of the describe reports the member the request
    // carried, named as an event document names it.
    let read = audited.record_of("the read", "DescribeDBInstances");
    assert_eq!(
        member(&read, "/requestParameters/dBInstanceIdentifier").as_deref(),
        Some("audit-kinds"),
        "the record carries the member the request named, got {read}"
    );
}

/// RULE: a value held at a member the model declares `@sensitive` is NOT stored — the
/// record carries the fixed redaction literal in its place, and the plaintext appears
/// nowhere in event history.
///
/// The rows are every sensitive member this service's model declares
/// (`service::management_events::SENSITIVE_MEMBERS`) on an operation that carries it, so
/// the rule is tested over the class rather than over the one member a recording named.
#[tokio::test]
async fn a_sensitive_member_is_never_stored() {
    let audited = Audited::new();
    // (case, action, parameters, the recorded member, the secret that must not be stored)
    let rows: Vec<MemberRow> = vec![
        (
            "a create's master password",
            "CreateDBInstance",
            create_params("audit-secret-one", "create-pl41nt3xt"),
            "masterUserPassword",
            "create-pl41nt3xt",
        ),
        (
            "a create's TDE credential password",
            "CreateDBInstance",
            {
                let mut params = create_params("audit-secret-two", "another-pl41nt3xt");
                params.push(("TdeCredentialArn", "arn:aws:kms:tde".to_string()));
                params.push(("TdeCredentialPassword", "tde-pl41nt3xt".to_string()));
                params
            },
            "tdeCredentialPassword",
            "tde-pl41nt3xt",
        ),
        (
            "a modify's master password",
            "ModifyDBInstance",
            vec![
                ("DBInstanceIdentifier", "audit-secret-one".to_string()),
                ("MasterUserPassword", "modify-pl41nt3xt".to_string()),
            ],
            "masterUserPassword",
            "modify-pl41nt3xt",
        ),
        (
            "a cluster create's master password",
            "CreateDBCluster",
            vec![
                ("DBClusterIdentifier", "audit-secret-cluster".to_string()),
                ("Engine", "aurora-mysql".to_string()),
                ("MasterUsername", "adminuser".to_string()),
                ("MasterUserPassword", "cluster-pl41nt3xt".to_string()),
            ],
            "masterUserPassword",
            "cluster-pl41nt3xt",
        ),
    ];

    for (case, action, params, recorded_member, secret) in &rows {
        // A modify needs its instance settled: a `creating` instance is not in a state
        // RDS modifies, and this suite is about what the record carries rather than about
        // that refusal.
        if *action == "ModifyDBInstance" {
            wait_for_settle();
        }
        let (status, body) = audited.call(action, &borrowed(params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call must be served, got {status} with {body}"
        );
        let event = audited
            .records_of(action)
            .pop()
            .unwrap_or_else(|| panic!("case {case:?}: the call must be recorded"));
        assert_eq!(
            member(&event, &format!("/requestParameters/{recorded_member}")).as_deref(),
            Some(REDACTED),
            "case {case:?}: requestParameters.{recorded_member} must be the redaction \
             literal, got {event}"
        );
        let whole_log = audited
            .records()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>()
            .join("\n");
        assert!(
            !whole_log.contains(secret),
            "case {case:?}: the plaintext {secret:?} must appear in no record, got \
             {whole_log}"
        );
    }

    // The members that are NOT sensitive are recorded as the call carried them, which is
    // what makes the redaction a withholding of one member rather than of the request.
    let create = audited.records_of("CreateDBInstance").remove(0);
    for (member_name, expected) in [
        ("dBInstanceIdentifier", "audit-secret-one"),
        ("masterUsername", "admin"),
        ("engine", "mysql"),
    ] {
        assert_eq!(
            member(&create, &format!("/requestParameters/{member_name}")).as_deref(),
            Some(expected),
            "case {member_name:?}: expected {expected:?} in the record, got {create}"
        );
    }
}

/// RULE: the `errorCode` of a failed call's record is the FAULT SHAPE's name, which on a
/// query protocol is not always the code the caller saw: the wire carries the shape's
/// `awsQueryError` code.
///
/// The rows span both relations the two names can have — a code that differs from the
/// shape name, and a code that IS the shape name — over four operations.
#[tokio::test]
async fn a_failed_call_records_the_fault_shape_name() {
    let audited = Audited::new();
    let created = create_params("audit-faults", "sup3r-s3cret");
    let (status, body) = audited.call("CreateDBInstance", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "setup: the create must be served, got {status} with {body}"
    );

    // (case, action, parameters, the <Code> the caller sees, the recorded errorCode)
    let rows: Vec<MemberRow> = vec![
        (
            "an unknown DB instance",
            "DescribeDBInstances",
            vec![("DBInstanceIdentifier", "no-such-instance".to_string())],
            "DBInstanceNotFound",
            "DBInstanceNotFoundFault",
        ),
        (
            "an identifier already taken",
            "CreateDBInstance",
            create_params("audit-faults", "sup3r-s3cret"),
            "DBInstanceAlreadyExists",
            "DBInstanceAlreadyExistsFault",
        ),
        (
            "an unknown DB cluster",
            "DescribeDBClusters",
            vec![("DBClusterIdentifier", "no-such-cluster".to_string())],
            "DBClusterNotFoundFault",
            "DBClusterNotFoundFault",
        ),
        (
            "an unknown DB subnet group",
            "DeleteDBSubnetGroup",
            vec![("DBSubnetGroupName", "no-such-subnet-group".to_string())],
            "DBSubnetGroupNotFoundFault",
            "DBSubnetGroupNotFoundFault",
        ),
    ];

    for (case, action, params, expected_wire_code, expected_recorded) in &rows {
        let (status, body) = audited.call(action, &borrowed(params)).await;
        assert!(
            (400..500).contains(&status),
            "case {case:?}: expected a client refusal, got {status} with {body}"
        );
        let answered = wire_code(&body);
        assert_eq!(
            answered, *expected_wire_code,
            "case {case:?}: expected the wire code {expected_wire_code:?}, got {answered:?} \
             in {body}"
        );
        let event = audited
            .records_of(action)
            .pop()
            .unwrap_or_else(|| panic!("case {case:?}: the refused call must be recorded"));
        assert_eq!(
            member(&event, "/errorCode").as_deref(),
            Some(*expected_recorded),
            "case {case:?}: expected errorCode {expected_recorded:?}, got {event}"
        );
        assert!(
            member(&event, "/errorMessage").is_some_and(|message| !message.is_empty()),
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

/// RULE: the resources an event names are the ones the call addressed, as the
/// type-and-ARN pairs a lookup by `ResourceName` or `ResourceType` filters on.
#[tokio::test]
async fn a_record_names_the_resource_the_call_addressed() {
    let audited = Audited::new();
    let created = create_params("audit-resources", "sup3r-s3cret");
    let (status, body) = audited.call("CreateDBInstance", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "setup: the create must be served, got {status} with {body}"
    );
    let (status, body) = audited
        .call(
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "audit-resource-cluster"),
                ("Engine", "aurora-mysql"),
                ("MasterUsername", "adminuser"),
                ("MasterUserPassword", "cluster-s3cret"),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the cluster create must be served, got {status} with {body}"
    );

    let addressed: Vec<(String, Vec<(String, String)>)> = audited.with_log(|event| {
        (
            event.call.event_name.clone(),
            event
                .call
                .resources
                .iter()
                .map(|resource| {
                    (
                        resource.resource_type.clone(),
                        resource.resource_name.clone(),
                    )
                })
                .collect(),
        )
    });

    // (case, the operation, the resource type, the ARN of the resource it addressed)
    for (case, event_name, resource_type, resource_name) in [
        (
            "the instance create",
            "CreateDBInstance",
            "AWS::RDS::DBInstance",
            format!("arn:aws:rds:{REGION}:000000000000:db:audit-resources"),
        ),
        (
            "the cluster create",
            "CreateDBCluster",
            "AWS::RDS::DBCluster",
            format!("arn:aws:rds:{REGION}:000000000000:cluster:audit-resource-cluster"),
        ),
    ] {
        let found = addressed
            .iter()
            .find(|(name, _)| name == event_name)
            .unwrap_or_else(|| panic!("case {case:?}: {event_name} must be recorded"));
        assert!(
            found
                .1
                .contains(&(resource_type.to_string(), resource_name.clone())),
            "case {case:?}: expected the resource ({resource_type}, {resource_name}), got \
             {found:?}"
        );
    }
}
