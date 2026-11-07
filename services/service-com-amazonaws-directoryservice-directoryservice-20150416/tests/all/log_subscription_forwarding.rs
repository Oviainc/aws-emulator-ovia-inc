// SPDX-License-Identifier: CC0-1.0

//! What a log subscription FORWARDS, driven end to end over the wire with Directory
//! Service and CloudWatch Logs mounted on ONE engine — the shape `core/router/src/lib.rs`
//! mounts them in — so the records land in the store `logs:FilterLogEvents` and
//! `logs:DescribeLogStreams` answer from.
//!
//! `CreateLogSubscription` "creates a subscription to forward real-time Directory Service
//! domain controller security logs to the specified Amazon CloudWatch log group", so the
//! rule under test is about DELIVERY, not about the stored subscription (that is the
//! sibling `log_subscriptions` suite): while a directory carries a log subscription, every
//! act that changes the directory's own objects produces a domain-controller security
//! record in the subscribed log group, naming the account acted on; an act performed while
//! no subscription exists — before the create, or after `DeleteLogSubscription` — produces
//! none.
//!
//! The rows differ along what the rule generalises over: the ACT (a computer account
//! created, an account's password reset — two different audited changes, logged under
//! their own Windows Security event ids), the FORM of the subscribed `LogGroupName` the
//! model admits, WHEN the act happens relative to the subscription (before, during,
//! after), and HOW MANY directories and groups are in play, so one directory's records
//! cannot pass by being the only ones.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_directoryservice_directoryservice_20150416::state::security_log;
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::microsoft_ad::minimal_microsoft_ad;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of each service's `awsJson1_1` protocol: its service shape's
/// name.
const DS_TARGET_PREFIX: &str = "DirectoryService_20150416";
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// A new password that meets a directory's own policy, for the reset rows.
const GOOD_PASSWORD: &str = "Sim2Real-Fwd-Pw1!";

/// The administrator account a Managed Microsoft AD directory is created with, from
/// `CreateMicrosoftADRequest$Password`'s documentation: "the default administrative user
/// named `Admin`".
const ADMINISTRATOR: &str = "Admin";

/// Both services over ONE shared engine.
struct Forwarding {
    router: MultiServiceRouter,
}

impl Forwarding {
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
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One call of either service, returning the status and the parsed body.
    async fn call(
        &self,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
        };
        (status, parsed)
    }

    /// One Directory Service call that must succeed.
    async fn ds(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.call("ds", DS_TARGET_PREFIX, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One CloudWatch Logs call that must succeed.
    async fn logs(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self
            .call("logs", LOGS_TARGET_PREFIX, operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200 from CloudWatch Logs, got \
             {status} with {parsed}"
        );
        parsed
    }

    /// Create the log group `name` through CloudWatch Logs' own API: the subscription names
    /// a group the ACCOUNT owns, so every row creates it the way a client does.
    async fn create_log_group(&self, name: &str, case: &str) {
        self.logs("CreateLogGroup", json!({ "logGroupName": name }), case)
            .await;
    }

    /// Every event the log group `name` holds, as `logs:FilterLogEvents` reports them.
    async fn forwarded(&self, name: &str, case: &str) -> Vec<Value> {
        let answer = self
            .logs("FilterLogEvents", json!({ "logGroupName": name }), case)
            .await;
        answer["events"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected events, got {answer}"))
            .clone()
    }

    /// Every stream the log group `name` holds, as `logs:DescribeLogStreams` reports them.
    async fn streams(&self, name: &str, case: &str) -> Vec<String> {
        let answer = self
            .logs("DescribeLogStreams", json!({ "logGroupName": name }), case)
            .await;
        answer["logStreams"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected logStreams, got {answer}"))
            .iter()
            .filter_map(|stream| stream["logStreamName"].as_str())
            .map(str::to_string)
            .collect()
    }

    /// A Managed Microsoft AD directory, created and in service with its domain controllers
    /// up — which is what a security log is forwarded from.
    async fn directory(&self, name: &str, case: &str) -> String {
        let created = self
            .ds("CreateMicrosoftAD", minimal_microsoft_ad(name), case)
            .await;
        let directory_id = created["DirectoryId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a DirectoryId, got {created}"))
            .to_string();
        tokio::time::sleep(PAST_ONE_SPAN).await;
        directory_id
    }

    /// The `DomainControllerId`s this directory reports, in the order it reports them.
    async fn domain_controllers(&self, directory_id: &str, case: &str) -> Vec<String> {
        let answer = self
            .ds(
                "DescribeDomainControllers",
                json!({ "DirectoryId": directory_id }),
                case,
            )
            .await;
        answer["DomainControllers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected DomainControllers, got {answer}"))
            .iter()
            .filter(|controller| controller["Status"].as_str() == Some("Active"))
            .filter_map(|controller| controller["DomainControllerId"].as_str())
            .map(str::to_string)
            .collect()
    }

    async fn subscribe(&self, directory_id: &str, log_group_name: &str, case: &str) {
        self.ds(
            "CreateLogSubscription",
            json!({ "DirectoryId": directory_id, "LogGroupName": log_group_name }),
            case,
        )
        .await;
    }

    async fn unsubscribe(&self, directory_id: &str, case: &str) {
        self.ds(
            "DeleteLogSubscription",
            json!({ "DirectoryId": directory_id }),
            case,
        )
        .await;
    }
}

/// One audited act a row performs on a directory: what it asks for, and what the record it
/// produces must say.
#[derive(Clone, Copy)]
enum Act {
    /// `CreateComputer`, which creates a computer account in the directory.
    CreateComputer,
    /// `ResetUserPassword`, which resets the administrator account's password.
    ResetUserPassword,
}

impl Act {
    /// The Windows Security event id and the account name the record must name, as this
    /// service's own catalog states them — read from the catalog rather than re-spelled
    /// here, so a row cannot assert an event id the emitter does not write.
    fn audited(self) -> security_log::AuditedAct {
        match self {
            Self::CreateComputer => security_log::COMPUTER_ACCOUNT_CREATED,
            Self::ResetUserPassword => security_log::USER_PASSWORD_RESET,
        }
    }

    /// Perform the act against `directory_id`, with `attempt` distinguishing one
    /// performance of it from the next, and answer the account name it acted on.
    async fn perform(
        self,
        emulator: &Forwarding,
        directory_id: &str,
        attempt: &str,
        case: &str,
    ) -> String {
        match self {
            Self::CreateComputer => {
                let computer_name = format!("srv-{attempt}");
                emulator
                    .ds(
                        "CreateComputer",
                        json!({
                            "DirectoryId": directory_id,
                            "ComputerName": &computer_name,
                            "Password": "Cmp-1-Time!Pw",
                        }),
                        case,
                    )
                    .await;
                computer_name
            }
            Self::ResetUserPassword => {
                emulator
                    .ds(
                        "ResetUserPassword",
                        json!({
                            "DirectoryId": directory_id,
                            "UserName": ADMINISTRATOR,
                            "NewPassword": GOOD_PASSWORD,
                        }),
                        case,
                    )
                    .await;
                ADMINISTRATOR.to_string()
            }
        }
    }
}

/// One row of the forwarding table.
struct ForwardingCase {
    case: &'static str,
    /// The directory's fully qualified name.
    domain: &'static str,
    /// The `LogGroupName` the subscription names, varied along the form the modelled
    /// pattern `^[-._/#A-Za-z0-9]+$` admits.
    log_group_name: String,
    act: Act,
}

/// `at` in milliseconds since the Unix epoch, the unit every CloudWatch Logs timestamp is
/// expressed in.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The parsed message of one forwarded record.
fn record(event: &Value, case: &str) -> Value {
    let message = event["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {event}"));
    serde_json::from_str(message).unwrap_or_else(|_| {
        panic!("case {case:?}: expected the forwarded record to be one JSON object, got {message}")
    })
}

/// RULE: while a directory carries a log subscription, each audited change to its objects
/// produces a domain-controller security record in the subscribed log group naming the
/// account acted on; a change made before the subscription existed, or after it was
/// deleted, produces none.
#[tokio::test]
async fn an_audited_act_is_forwarded_only_while_the_subscription_exists() {
    let emulator = Forwarding::new();

    let cases = vec![
        ForwardingCase {
            case: "a computer account created, path-shaped log group",
            domain: "computers.example.com",
            log_group_name: "/aws/directoryservice/security".to_string(),
            act: Act::CreateComputer,
        },
        ForwardingCase {
            case: "a password reset, every character class the pattern admits",
            domain: "passwords.example.com",
            log_group_name: "Group-1_2.3/four#five".to_string(),
            act: Act::ResetUserPassword,
        },
        ForwardingCase {
            case: "a computer account created, digits only",
            domain: "digits.example.com",
            log_group_name: "20150416".to_string(),
            act: Act::CreateComputer,
        },
    ];

    for ForwardingCase {
        case,
        domain,
        log_group_name,
        act,
    } in &cases
    {
        let case = *case;
        emulator.create_log_group(log_group_name, case).await;
        let directory_id = emulator.directory(domain, case).await;
        let controllers = emulator.domain_controllers(&directory_id, case).await;
        let serving = controllers.first().unwrap_or_else(|| {
            panic!("case {case:?}: expected the directory to report a domain controller in service")
        });

        // An act performed while NO subscription exists is never forwarded: the group is
        // empty and has no stream at all.
        let before = act.perform(&emulator, &directory_id, "before", case).await;
        let unsubscribed = emulator.forwarded(log_group_name, case).await;
        assert!(
            unsubscribed.is_empty(),
            "case {case:?}: an act on {before:?} performed before the subscription must not be \
             forwarded, got {unsubscribed:?}"
        );

        emulator
            .subscribe(&directory_id, log_group_name, case)
            .await;

        // ...and once the subscription exists, the same act reaches the group.
        let acted_from = epoch_millis(SystemTime::now());
        let target = act.perform(&emulator, &directory_id, "during", case).await;
        let acted_until = epoch_millis(SystemTime::now());

        let delivered = emulator.forwarded(log_group_name, case).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected exactly the subscribed act to be forwarded, got \
             {delivered:?}"
        );
        let event = &delivered[0];
        assert_eq!(
            event["logStreamName"].as_str(),
            Some(serving.as_str()),
            "case {case:?}: expected the record in the stream of the domain controller that \
             logged it ({serving}), got {event}"
        );
        let timestamp = event["timestamp"].as_i64().unwrap_or_default();
        assert!(
            (acted_from..=acted_until).contains(&timestamp),
            "case {case:?}: expected the record stamped at the instant of the act, within \
             [{acted_from}, {acted_until}], got {timestamp}"
        );

        let audited = act.audited();
        let parsed = record(event, case);
        for (member, expected) in [
            ("Channel", json!(security_log::SECURITY_CHANNEL)),
            ("EventID", json!(audited.event_id)),
            ("Description", json!(audited.description)),
            ("DirectoryId", json!(&directory_id)),
            ("DomainControllerId", json!(serving)),
            ("TargetDomainName", json!(domain)),
            ("TargetAccountName", json!(&target)),
        ] {
            assert_eq!(
                parsed[member], expected,
                "case {case:?}: expected the record's {member} to be {expected}, got {parsed}"
            );
        }

        // The stream the records are written to is the controller's own, and no other
        // stream was opened in the group.
        let streams = emulator.streams(log_group_name, case).await;
        assert_eq!(
            streams,
            vec![serving.clone()],
            "case {case:?}: expected one stream, named for the serving domain controller, got \
             {streams:?}"
        );

        // A second act while the subscription still exists is forwarded too, so the
        // delivery is not a one-off.
        let second = act.perform(&emulator, &directory_id, "again", case).await;
        let both = emulator.forwarded(log_group_name, case).await;
        assert_eq!(
            both.len(),
            2,
            "case {case:?}: expected the second act on {second:?} to be forwarded as well, got \
             {both:?}"
        );

        // Deleting the subscription STOPS the forwarding: the act still succeeds and the
        // group gains nothing.
        emulator.unsubscribe(&directory_id, case).await;
        let after = act.perform(&emulator, &directory_id, "after", case).await;
        let unchanged = emulator.forwarded(log_group_name, case).await;
        assert_eq!(
            unchanged, both,
            "case {case:?}: an act on {after:?} performed after DeleteLogSubscription must not \
             be forwarded, got {unchanged:?}"
        );
    }
}

/// RULE: a record reaches the group the directory that produced it subscribed to, and no
/// other — the subscription is per directory.
#[tokio::test]
async fn a_record_reaches_only_the_group_its_own_directory_subscribed_to() {
    let emulator = Forwarding::new();

    // Two subscribed directories with their own groups, and a third that subscribes to
    // nothing, so no row can pass by being the only directory in the account.
    let rows = [
        ("first.example.com", "/aws/ds/first"),
        ("second.example.com", "/aws/ds/second"),
    ];
    let mut subscribed: Vec<(String, &str, String)> = Vec::new();
    for (domain, group) in rows {
        emulator.create_log_group(group, domain).await;
        let directory_id = emulator.directory(domain, domain).await;
        emulator.subscribe(&directory_id, group, domain).await;
        subscribed.push((directory_id, group, domain.to_string()));
    }
    let quiet = emulator.directory("quiet.example.com", "quiet").await;

    // Each subscribed directory logs one act, and the quiet one logs one too.
    let mut expected: Vec<(&str, String, String)> = Vec::new();
    for (directory_id, group, domain) in &subscribed {
        let target = Act::CreateComputer
            .perform(&emulator, directory_id, "own", domain)
            .await;
        expected.push((group, directory_id.clone(), target));
    }
    Act::CreateComputer
        .perform(&emulator, &quiet, "quiet", "quiet")
        .await;

    for (group, directory_id, target) in &expected {
        let case = format!("{group} holds only {directory_id}'s records");
        let delivered = emulator.forwarded(group, &case).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected exactly one record, got {delivered:?}"
        );
        let parsed = record(&delivered[0], &case);
        assert_eq!(
            parsed["DirectoryId"],
            json!(directory_id),
            "case {case:?}: expected the record of this directory alone, got {parsed}"
        );
        assert_eq!(
            parsed["TargetAccountName"],
            json!(target),
            "case {case:?}: expected the record to name the account acted on, got {parsed}"
        );
    }
}

/// RULE: forwarding to a destination CloudWatch Logs does not hold loses the record and
/// nothing else — the directory call that produced it still succeeds, and no log group is
/// conjured for it.
#[tokio::test]
async fn a_subscription_naming_a_group_that_is_not_there_forwards_nothing() {
    let emulator = Forwarding::new();

    let directory_id = emulator.directory("absent.example.com", "absent").await;
    let missing = "/aws/ds/never-created";
    emulator.subscribe(&directory_id, missing, "absent").await;

    // The act is served as it is for any directory...
    let target = Act::CreateComputer
        .perform(&emulator, &directory_id, "absent", "absent")
        .await;
    assert!(
        !target.is_empty(),
        "case \"absent\": expected the audited act to be served and name an account"
    );

    // ...and CloudWatch Logs still holds no group of that name, so nothing was created to
    // deliver into.
    let groups = emulator
        .logs(
            "DescribeLogGroups",
            json!({ "logGroupNamePrefix": missing }),
            "absent",
        )
        .await;
    let reported = groups["logGroups"].as_array().cloned().unwrap_or_default();
    assert!(
        reported.is_empty(),
        "case \"absent\": expected no log group to be created for an unheld destination, got \
         {reported:?}"
    );
}
