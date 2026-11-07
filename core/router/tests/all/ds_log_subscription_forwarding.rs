// SPDX-License-Identifier: CC0-1.0

//! The DOMAIN CONTROLLER SECURITY LOG a subscribed directory forwards, across the three
//! services that have to agree for a client to see it.
//!
//! `ds:CreateLogSubscription` "creates a subscription to forward real-time Directory
//! Service domain controller security logs to the specified Amazon CloudWatch log group",
//! and the acts those controllers audit are the writes to the directory's own objects —
//! which the Directory Service DATA plane (`ds-data`) performs, in a separate crate, while
//! the subscription lives on the CONTROL plane's directory record and the records have to
//! land in CloudWatch Logs' own store. Only this configuration — all three mounted on ONE
//! engine, which is what `core/router` ships — makes the forwarding observable at all.
//!
//! The rule these rows pin: while a directory carries a log subscription, each write to its
//! users, groups and memberships produces a security record in the subscribed log group
//! naming the account acted on; a write made before the subscription existed, or after
//! `DeleteLogSubscription`, produces none. The rows differ along the WRITE (a user created
//! and deleted, a group created, a membership added and removed — each a different audited
//! act) and along WHEN it happens relative to the subscription.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The three signing names, which is how the router picks the service that serves each
/// request (`smithy_query(query="service_info")`).
const DS: &str = "ds";
const DS_DATA: &str = "ds-data";
const LOGS: &str = "logs";

/// The `X-Amz-Target` prefix each `awsJson1_1` service names an operation under: its
/// service shape's name. (`ds-data` is `restJson1` and carries none.)
const DS_TARGET_PREFIX: &str = "DirectoryService_20150416";
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// Past the span a directory's create-owed transitions take to fall due, so the directory
/// is in service with its domain controllers up — the state a security log is forwarded
/// from.
const PAST_ONE_SPAN: Duration = Duration::from_millis(900);

/// The log group the account creates for its directory's security logs.
const LOG_GROUP: &str = "/aws/directoryservice/sim2real";

/// The three services over ONE shared engine, as `router/src/lib.rs` mounts them.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These rows exercise handler behaviour, not request signing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn dispatch(
        &self,
        signing_name: &str,
        uri: &str,
        target: Option<String>,
        body: Value,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!("https://localhost:9297{uri}"))
            .header("host", "localhost:9297")
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(target) = target {
            request = request.header("x-amz-target", target);
        }
        let response = self
            .router
            .dispatch(
                request
                    .body(Bytes::from(body.to_string()))
                    .expect("a well-formed request"),
            )
            .await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Directory Service control-plane call that must succeed.
    async fn ds(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self
            .dispatch(DS, "/", Some(format!("{DS_TARGET_PREFIX}.{operation}")), body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ds {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One Directory Service DATA-plane call that must succeed.
    async fn ds_data(&self, route: &str, directory_id: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self
            .dispatch(
                DS_DATA,
                &format!("{route}?DirectoryId={directory_id}"),
                None,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ds-data {route} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self
            .dispatch(
                LOGS,
                "/",
                Some(format!("{LOGS_TARGET_PREFIX}.{operation}")),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected logs {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Every record the log group holds, as `logs:FilterLogEvents` reports them, parsed
    /// from each event's message.
    async fn forwarded(&self, case: &str) -> Vec<Value> {
        let answer = self
            .logs("FilterLogEvents", json!({ "logGroupName": LOG_GROUP }), case)
            .await;
        answer["events"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected events, got {answer}"))
            .iter()
            .map(|event| {
                let message = event["message"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {event}"));
                serde_json::from_str(message).unwrap_or_else(|_| {
                    panic!("case {case:?}: expected one JSON object per record, got {message}")
                })
            })
            .collect()
    }
}

/// One audited data-plane write: the route, the body, and what the record it produces must
/// say.
struct WriteCase {
    case: &'static str,
    route: &'static str,
    body: Value,
    /// The Windows Security event id the act is logged under.
    event_id: u32,
    /// The account the record must name as the object acted on.
    target: &'static str,
    /// The member a membership change names, for the acts that are about a membership.
    member: Option<&'static str>,
}

/// A data-plane write is forwarded to the subscribed log group while the subscription
/// exists, and is forwarded neither before it is created nor after it is deleted.
#[tokio::test]
async fn directory_writes_reach_the_subscribed_log_group_while_the_subscription_exists() {
    let emulator = Emulator::new();

    // The account's own log group, created through CloudWatch Logs the way a client does.
    emulator
        .logs("CreateLogGroup", json!({ "logGroupName": LOG_GROUP }), "setup")
        .await;

    let created = emulator
        .ds(
            "CreateMicrosoftAD",
            json!({
                "Name": "corp.example.com",
                "Password": "Sim2Real#Pass99",
                "Edition": "Standard",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            "setup",
        )
        .await;
    let directory_id = created["DirectoryId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateMicrosoftAD must report a DirectoryId, got {created}"))
        .to_string();
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // A write performed while NO subscription exists is never forwarded.
    emulator
        .ds_data(
            "/Users/CreateUser",
            &directory_id,
            json!({"SAMAccountName": "beforeSub", "GivenName": "Before"}),
            "before the subscription",
        )
        .await;
    let before = emulator.forwarded("before the subscription").await;
    assert!(
        before.is_empty(),
        "a write performed before CreateLogSubscription must not be forwarded, got {before:?}"
    );

    emulator
        .ds(
            "CreateLogSubscription",
            json!({ "DirectoryId": &directory_id, "LogGroupName": LOG_GROUP }),
            "subscribe",
        )
        .await;

    // Each audited write of the data plane, in the order a client performs them: a user
    // created, a group created, that user added to the group and taken out of it again,
    // and the user deleted.
    let writes = vec![
        WriteCase {
            case: "a user created",
            route: "/Users/CreateUser",
            body: json!({"SAMAccountName": "s2rLogSub1", "GivenName": "Sim", "Surname": "Real"}),
            event_id: 4720,
            target: "s2rLogSub1",
            member: None,
        },
        WriteCase {
            case: "a group created",
            route: "/Groups/CreateGroup",
            body: json!({"SAMAccountName": "s2rAuditors"}),
            event_id: 4727,
            target: "s2rAuditors",
            member: None,
        },
        WriteCase {
            case: "a member added to a group",
            route: "/GroupMemberships/AddGroupMember",
            body: json!({"GroupName": "s2rAuditors", "MemberName": "s2rLogSub1"}),
            event_id: 4728,
            target: "s2rAuditors",
            member: Some("s2rLogSub1"),
        },
        WriteCase {
            case: "a member removed from a group",
            route: "/GroupMemberships/RemoveGroupMember",
            body: json!({"GroupName": "s2rAuditors", "MemberName": "s2rLogSub1"}),
            event_id: 4729,
            target: "s2rAuditors",
            member: Some("s2rLogSub1"),
        },
        WriteCase {
            case: "a user deleted",
            route: "/Users/DeleteUser",
            body: json!({"SAMAccountName": "s2rLogSub1"}),
            event_id: 4726,
            target: "s2rLogSub1",
            member: None,
        },
    ];

    let mut expected = 0usize;
    for write in &writes {
        let case = write.case;
        emulator
            .ds_data(write.route, &directory_id, write.body.clone(), case)
            .await;
        expected += 1;

        let records = emulator.forwarded(case).await;
        assert_eq!(
            records.len(),
            expected,
            "case {case:?}: expected the write to add exactly one record, got {records:?}"
        );
        let record = records
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: expected a record"));
        assert_eq!(
            record["EventID"],
            json!(write.event_id),
            "case {case:?}: expected the record's EventID to be {}, got {record}",
            write.event_id
        );
        assert_eq!(
            record["DirectoryId"],
            json!(&directory_id),
            "case {case:?}: expected the record to name its directory, got {record}"
        );
        assert_eq!(
            record["TargetAccountName"],
            json!(write.target),
            "case {case:?}: expected the record to name the account acted on, got {record}"
        );
        assert_eq!(
            record.get("MemberName").and_then(Value::as_str),
            write.member,
            "case {case:?}: expected MemberName {:?}, got {record}",
            write.member
        );
        // The record came from one of the directory's own domain controllers, which is
        // where a domain controller security log comes from.
        let controller = record["DomainControllerId"].as_str().unwrap_or_default();
        assert!(
            controller.starts_with("dc-"),
            "case {case:?}: expected the record to name the domain controller that logged it, \
             got {record}"
        );
    }

    // Deleting the subscription STOPS the forwarding: the write still succeeds and the
    // group gains nothing.
    emulator
        .ds(
            "DeleteLogSubscription",
            json!({ "DirectoryId": &directory_id }),
            "unsubscribe",
        )
        .await;
    let before_unsubscribed = emulator.forwarded("after the delete").await;
    emulator
        .ds_data(
            "/Users/CreateUser",
            &directory_id,
            json!({"SAMAccountName": "afterSub", "GivenName": "After"}),
            "after the delete",
        )
        .await;
    let after = emulator.forwarded("after the delete").await;
    assert_eq!(
        after, before_unsubscribed,
        "a write performed after DeleteLogSubscription must not be forwarded, got {after:?}"
    );

    // Re-subscribing starts it again — and forwards nothing of what happened while no
    // subscription existed.
    emulator
        .ds(
            "CreateLogSubscription",
            json!({ "DirectoryId": &directory_id, "LogGroupName": LOG_GROUP }),
            "resubscribe",
        )
        .await;
    emulator
        .ds_data(
            "/Users/CreateUser",
            &directory_id,
            json!({"SAMAccountName": "afterResub", "GivenName": "Again"}),
            "after resubscribing",
        )
        .await;
    let resumed = emulator.forwarded("after resubscribing").await;
    assert_eq!(
        resumed.len(),
        after.len() + 1,
        "expected forwarding to resume with exactly the new write, got {resumed:?}"
    );
    let last = resumed
        .last()
        .expect("the resumed listing carries the new record");
    assert_eq!(
        last["TargetAccountName"],
        json!("afterResub"),
        "expected the resumed record to name the account just created, got {last}"
    );
    assert!(
        !resumed
            .iter()
            .any(|record| record["TargetAccountName"] == json!("afterSub")),
        "expected nothing written while unsubscribed to be forwarded later, got {resumed:?}"
    );
}
