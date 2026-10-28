// SPDX-License-Identifier: CC0-1.0

//! WHICH ARN the closed Incident Manager account gate names, end to end over the wire.
//!
//! The rule under test:
//!
//! > A rotation and an engagement are addressed THROUGH the account's Incident Manager
//! > replication set, so while the account holds no ACTIVE set an ARN naming one resolves to
//! > nothing: the call is refused with its modelled `ValidationException` (HTTP 400) whose
//! > message is the fixed text "Invalid value provided - Invalid resource Arn " followed by
//! > the ARN the request carried, verbatim. A gated call that addresses a contact, a contact
//! > channel or a page, or that names no resource at all, is refused about the ACCOUNT
//! > instead ("Invalid value provided - Account not found for the request"). The family is
//! > the one the request's subject member names (`RotationId`, `EngagementId` and the
//! > tagging trio's `ResourceARN`, which names a contact or a rotation, so there the value
//! > decides). Nothing else about the request chooses between the two: not the partition or
//! > Region the ARN is written in, not the form of its resource id, and not whether the ARN
//! > names a resource the store really holds.
//!
//! The table therefore varies what the rule generalises over: the resource FAMILY the ARN
//! names (five of them, each driven through more than one operation), the FORM of its
//! resource id (a hyphenated uuid, a pure-hex id, a caller-chosen alias, an id of more than
//! one segment), the partition and Region field, and the account state that closes the gate.
//! Sibling suites own the refusal's class (`tests/all/cold_account_gate.rs`) and the whole
//! gated surface's text (`tests/all/account_gate_message.rs`); this one is about the ARN the
//! refusal has to carry.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet;

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

/// The two refusal texts, spelled out rather than read from the crate: they are what AWS
/// words these two failures with, so this suite pins the words and not the crate's own
/// rendering of them. The sibling gate suites read the prefix from here, so the three cannot
/// drift apart.
pub(crate) const INVALID_RESOURCE_ARN_PREFIX: &str =
    "Invalid value provided - Invalid resource Arn ";
pub(crate) const ACCOUNT_NOT_FOUND: &str =
    "Invalid value provided - Account not found for the request";

/// Epoch seconds, as the wire carries a modelled `DateTime`.
const WINDOW_START: i64 = 1_735_689_600;
const WINDOW_END: i64 = 1_735_693_200;

/// The recurrence `UpdateRotation` requires: the smallest one the model admits.
fn recurrence() -> Value {
    json!({
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
    })
}

// ---------------------------------------------------------------------------
// The ARN catalog
// ---------------------------------------------------------------------------

/// Which resource family an ARN names, and therefore which wording its refusal carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// Reachable only through the replication set: the refusal names the ARN.
    ReplicationSetScoped,
    /// Resolved without it: the refusal names the account.
    AccountScoped,
}

/// One subject: the resource type segment its ARN carries, the ARN itself, and the family
/// that decides its wording.
struct Subject {
    case: String,
    arn: String,
    family: Family,
}

/// The resource-id forms an `SsmContactsArn` admits, each paired with the case it covers.
///
/// Source: `smithy_query(query="shape",shape="SsmContactsArn")` — `length` 1..2048 and
/// `pattern` `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=/,.@]*:[0-9]+:([\w+=/,.@:-]+)*$`,
/// whose resource field admits a hyphenated uuid, a pure-hex id, a caller-chosen alias and
/// several `/`-joined segments alike, and whose Region field admits the empty string. The
/// three partition alternatives are all exercised below.
fn id_forms() -> Vec<(&'static str, String)> {
    vec![
        (
            "a hyphenated uuid resource id",
            "9b5e4a1c-2d3f-4a5b-8c7d-0e1f2a3b4c5d".to_string(),
        ),
        (
            "a pure-hex resource id",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        ("a one-character resource id", "r".to_string()),
        (
            "a resource id of two further segments",
            "team-sre/0123456789ab".to_string(),
        ),
    ]
}

/// The partition and Region spellings the modelled pattern admits.
fn partitions() -> Vec<(&'static str, String, String)> {
    vec![
        ("the aws partition", "aws".to_string(), REGION.to_string()),
        (
            "the aws-cn partition",
            "aws-cn".to_string(),
            "cn-north-1".to_string(),
        ),
        (
            "the aws-us-gov partition",
            "aws-us-gov".to_string(),
            "us-gov-west-1".to_string(),
        ),
        (
            "an empty Region, which the pattern admits",
            "aws".to_string(),
            String::new(),
        ),
    ]
}

/// Every subject this suite drives: each resource type the gated operations address, over
/// every id form and partition spelling.
fn subjects(resource_type: &str, family: Family) -> Vec<Subject> {
    let mut subjects = Vec::new();
    for (id_case, id) in id_forms() {
        for (partition_case, partition, region) in partitions() {
            subjects.push(Subject {
                case: format!("a {resource_type} ARN with {id_case} in {partition_case}"),
                arn: format!("arn:{partition}:ssm-contacts:{region}:{ACCOUNT}:{resource_type}/{id}"),
                family,
            });
        }
    }
    subjects
}

// ---------------------------------------------------------------------------
// The gated operations, grouped by the resource type they address
// ---------------------------------------------------------------------------

/// One operation addressed by one ARN: its name and the body it needs around that ARN.
struct Addressed {
    operation: &'static str,
    body: fn(&str) -> Value,
}

/// Every gated operation that addresses a ROTATION by ARN — the family the recorded
/// `GetRotation` refusal belongs to, plus its peers the issue did not cite.
fn rotation_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "GetRotation",
            body: |arn| json!({"RotationId": arn}),
        },
        Addressed {
            operation: "UpdateRotation",
            body: |arn| json!({"RotationId": arn, "Recurrence": recurrence()}),
        },
        Addressed {
            operation: "DeleteRotation",
            body: |arn| json!({"RotationId": arn}),
        },
        Addressed {
            operation: "ListRotationOverrides",
            body: |arn| {
                json!({"RotationId": arn, "StartTime": WINDOW_START, "EndTime": WINDOW_END})
            },
        },
    ]
}

/// Every gated operation that addresses an ENGAGEMENT by ARN — the family the recorded
/// `DescribeEngagement` refusal belongs to, plus its peers.
fn engagement_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "DescribeEngagement",
            body: |arn| json!({"EngagementId": arn}),
        },
        Addressed {
            operation: "StopEngagement",
            body: |arn| json!({"EngagementId": arn}),
        },
        Addressed {
            operation: "ListPagesByEngagement",
            body: |arn| json!({"EngagementId": arn}),
        },
    ]
}

/// Gated operations addressing a CONTACT, a CONTACT CHANNEL and a PAGE: the complement, whose
/// refusal reports the account.
fn contact_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "GetContact",
            body: |arn| json!({"ContactId": arn}),
        },
        Addressed {
            operation: "GetContactPolicy",
            body: |arn| json!({"ContactArn": arn}),
        },
        Addressed {
            operation: "ListPagesByContact",
            body: |arn| json!({"ContactId": arn}),
        },
    ]
}

fn channel_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "GetContactChannel",
            body: |arn| json!({"ContactChannelId": arn}),
        },
        Addressed {
            operation: "SendActivationCode",
            body: |arn| json!({"ContactChannelId": arn}),
        },
    ]
}

fn page_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "DescribePage",
            body: |arn| json!({"PageId": arn}),
        },
        Addressed {
            operation: "ListPageReceipts",
            body: |arn| json!({"PageId": arn}),
        },
    ]
}

/// The whole table: every resource type, the operations that address it, and the family that
/// words their refusal.
fn table() -> Vec<(&'static str, Family, Vec<Addressed>)> {
    vec![
        (
            "rotation",
            Family::ReplicationSetScoped,
            rotation_operations(),
        ),
        (
            "engagement",
            Family::ReplicationSetScoped,
            engagement_operations(),
        ),
        ("contact", Family::AccountScoped, contact_operations()),
        (
            "contact-channel",
            Family::AccountScoped,
            channel_operations(),
        ),
        ("page", Family::AccountScoped, page_operations()),
    ]
}

/// Every account state that closes the gate.
fn closed_gate_states() -> Vec<(&'static str, Option<AccountReplicationSet>)> {
    vec![
        ("no replication set at all", None),
        (
            "a replication set that has not finished replicating",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The assertion
// ---------------------------------------------------------------------------

/// The refusal a closed gate owes `subject`: HTTP 400, the error code `ValidationException`,
/// and EXACTLY the message its family decides — the fixed ARN text followed by the request's
/// own ARN, or the account text, never the other one.
fn assert_wording(case: &str, status: u16, body: &Value, arn: &str, family: Family) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the closed gate to answer HTTP 400, got {status} with \
         {body:?}"
    );
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {:?} in {body:?}",
        error_code(body)
    );
    let message = body["Message"].as_str().unwrap_or_default();
    match family {
        Family::ReplicationSetScoped => {
            assert_eq!(
                message,
                format!("{INVALID_RESOURCE_ARN_PREFIX}{arn}"),
                "case {case:?}: expected the refusal to be the fixed text \
                 {INVALID_RESOURCE_ARN_PREFIX:?} followed by the supplied ARN {arn:?}, got \
                 {message:?} in {body:?}"
            );
            assert!(
                message.ends_with(arn),
                "case {case:?}: the supplied ARN must survive into the message verbatim, got \
                 {message:?}"
            );
        }
        Family::AccountScoped => {
            assert_eq!(
                message, ACCOUNT_NOT_FOUND,
                "case {case:?}: expected the account-level refusal {ACCOUNT_NOT_FOUND:?}, got \
                 {message:?} in {body:?}"
            );
            assert!(
                !message.contains(arn),
                "case {case:?}: an account-level refusal names no member of the request, got \
                 {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: the family the ARN names decides the wording
// ---------------------------------------------------------------------------

/// The whole table, over every ARN the catalog spells and both closed-gate states.
#[tokio::test]
async fn the_closed_gate_names_the_arn_of_a_replication_set_scoped_resource() {
    for (account_case, replication_set) in closed_gate_states() {
        let emulator = Emulator::fresh();
        set_replication_set(&emulator, replication_set.clone());

        for (resource_type, family, operations) in table() {
            for subject in subjects(resource_type, family) {
                for addressed in &operations {
                    let case = format!(
                        "{} on {}, {account_case}",
                        addressed.operation, subject.case
                    );
                    let (status, body) = emulator
                        .call(addressed.operation, (addressed.body)(&subject.arn))
                        .await;
                    assert_wording(&case, status, &body, &subject.arn, subject.family);
                }
            }
        }
    }
}

/// The same wording for ARNs the store really HOLDS, and the gate is the only thing that was
/// in the way: once the account is onboarded again the very same reads report the resources.
///
/// This is the round trip for the two operations the recorded checks drive — a rotation and
/// an engagement this emulator minted itself, so the ARNs are the service's own rather than
/// hand-written.
#[tokio::test]
async fn a_stored_rotation_and_engagement_are_named_too_and_read_back_once_onboarded() {
    let emulator = Emulator::fresh();
    onboard(&emulator);

    let contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
            }),
            "the suite's stored contact",
        )
        .await;
    let rotation = emulator
        .create_rotation(&contact, "stored-rotation", "the suite's stored rotation")
        .await;
    let (status, body) = emulator
        .call(
            "StartEngagement",
            json!({
                "ContactId": contact,
                "Sender": "cli",
                "Subject": "disk pressure",
                "Content": "node-7 is at 95%",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the suite's stored engagement must be started, got {status} {body:?}"
    );
    let engagement = body["EngagementArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an EngagementArn, got {body:?}"))
        .to_string();

    for (account_case, replication_set) in closed_gate_states() {
        set_replication_set(&emulator, replication_set.clone());
        for (operation, request, arn) in [
            (
                "GetRotation",
                json!({"RotationId": rotation}),
                rotation.clone(),
            ),
            (
                "DescribeEngagement",
                json!({"EngagementId": engagement}),
                engagement.clone(),
            ),
        ] {
            let case = format!("{operation} on a stored subject, {account_case}");
            let (status, body) = emulator.call(operation, request).await;
            assert_wording(
                &case,
                status,
                &body,
                &arn,
                Family::ReplicationSetScoped,
            );
        }
    }

    // Onboarded again, the same two reads answer about their subjects: the refusals above
    // were the gate, and they changed nothing.
    onboard(&emulator);
    let (status, body) = emulator
        .call("GetRotation", json!({"RotationId": rotation}))
        .await;
    assert_eq!(
        status, 200,
        "case {rotation:?}: expected the stored rotation to be reported once onboarded, got \
         {status} with {body:?}"
    );
    assert_eq!(
        body["RotationArn"].as_str(),
        Some(rotation.as_str()),
        "case {rotation:?}: expected the read to report the rotation it was asked for, got \
         {body:?}"
    );
    let (status, body) = emulator
        .call("DescribeEngagement", json!({"EngagementId": engagement}))
        .await;
    assert_eq!(
        status, 200,
        "case {engagement:?}: expected the stored engagement to be reported once onboarded, \
         got {status} with {body:?}"
    );
    assert_eq!(
        body["EngagementArn"].as_str(),
        Some(engagement.as_str()),
        "case {engagement:?}: expected the read to report the engagement it was asked for, got \
         {body:?}"
    );
}

/// A value that is not an ARN of this service names no replication-set-scoped resource, so
/// the closed gate reports the ACCOUNT — whatever resource type the value spells inside.
///
/// `ResourceARN` is modelled as an `AmazonResourceName` (length only, no pattern), so these
/// values reach the handler at all.
#[tokio::test]
async fn a_value_that_is_no_arn_of_this_service_is_refused_about_the_account() {
    let not_ours = [
        ("a bare name", "my-rotation".to_string()),
        (
            "a rotation ARN of another vendor",
            format!("arn:aws:ssm-incidents:{REGION}:{ACCOUNT}:rotation/0123456789ab"),
        ),
        (
            "a truncated ARN with no resource field",
            "arn:aws:ssm-contacts".to_string(),
        ),
    ];

    for (value_case, value) in not_ours {
        let emulator = Emulator::fresh();
        for (operation, request) in [
            ("ListTagsForResource", json!({"ResourceARN": value})),
            (
                "TagResource",
                json!({"ResourceARN": value, "Tags": [{"Key": "k", "Value": "v"}]}),
            ),
            (
                "UntagResource",
                json!({"ResourceARN": value, "TagKeys": ["k"]}),
            ),
        ] {
            let case = format!("{operation} on {value_case}");
            let (status, body) = emulator.call(operation, request).await;
            assert_wording(&case, status, &body, &value, Family::AccountScoped);
        }
    }
}
