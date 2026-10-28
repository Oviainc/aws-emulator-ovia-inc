// SPDX-License-Identifier: CC0-1.0

//! WHAT the account-level Incident Manager precondition says when it refuses, end to end
//! over the wire.
//!
//! The rule under test:
//!
//! > While the account holds no ACTIVE Incident Manager replication set, every
//! > replication-set-scoped operation of this service is refused with its own modelled
//! > `ValidationException` (HTTP 400), decided from the account alone before any member of
//! > the request is resolved — so neither the operation called, nor whether the ARN it named
//! > resolves to a stored resource, nor the partition or Region that ARN is written in
//! > changes the answer. WHICH of the two wordings the refusal carries follows the resource
//! > family the request addressed: a rotation and an engagement are reachable only THROUGH
//! > the replication set, so with no set the ARN naming one resolves to nothing and the
//! > refusal reports that ARN verbatim ("Invalid value provided - Invalid resource Arn
//! > <arn>"); a call addressing a contact, a contact channel or a page, or naming no
//! > resource at all, is refused about the ACCOUNT instead. The family comes from what the
//! > member names: `RotationId` and `EngagementId` name those two families, while the
//! > tagging trio's `ResourceARN` names "a resource of this service" — a contact or a
//! > rotation — so there the family is the one the VALUE spells.
//!
//! The sibling suites assert the refusal's CLASS (`tests/all/cold_account_gate.rs`,
//! `tests/all/data_plane_precondition.rs`, `tests/all/arn_read_precondition.rs`) and that no
//! admitted request shape escapes it (`tests/all/account_gate_request_shapes.rs`); this one
//! is about its TEXT, which a client branches on when the error code alone cannot tell it
//! whether to onboard the account or to fix the ARN.
//!
//! The table spans the gated surface — every operation that consults the precondition — and
//! three subject flavours: well-formed ARNs nothing created, the same ARNs written in the
//! other partitions and with the Region field the modelled pattern leaves empty, and ARNs of
//! resources this store really holds.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{service_state, AccountReplicationSet};

use crate::account_gate_arn_wording::{ACCOUNT_NOT_FOUND, INVALID_RESOURCE_ARN_PREFIX};
use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

/// Epoch seconds, as the wire carries a modelled `DateTime`.
const WINDOW_START: i64 = 1_735_689_600;
const WINDOW_END: i64 = 1_735_693_200;

/// A `Uuid` of the form the model constrains `RotationOverrideId` to (`length` 36..39,
/// `pattern` `^([a-fA-Z0-9]{8,11}-…-[a-fA-F0-9]{12}){1}$`, from
/// `smithy_query(query="operation",operation="DeleteRotationOverride")`).
const AN_OVERRIDE_ID: &str = "00000000-0000-0000-0000-000000000000";

/// A well-formed resource policy document: a resource-based policy states the principal
/// each of its statements grants an action to, so one with no statement at all is not one.
const A_POLICY: &str = r#"{"Version": "2012-10-17", "Statement": [
    {"Effect": "Allow", "Principal": {"AWS": "000000000000"},
     "Action": "ssm-contacts:GetContact"}
]}"#;

// ---------------------------------------------------------------------------
// The subjects the gated requests name
// ---------------------------------------------------------------------------

/// One ARN per resource family the gated operations address, plus the override id that is
/// unique only within its rotation.
struct Subjects {
    case: &'static str,
    contact: String,
    channel: String,
    rotation: String,
    rotation_override_id: String,
    engagement: String,
    page: String,
}

impl Subjects {
    /// Well-formed ARNs, in the `aws` partition and this emulator's Region, of resources
    /// nothing ever created.
    fn absent() -> Self {
        Self {
            case: "ARNs nothing created, in the aws partition",
            contact: format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
            channel: format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/never-created/{AN_OVERRIDE_ID}"
            ),
            rotation: format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/{AN_OVERRIDE_ID}"),
            rotation_override_id: AN_OVERRIDE_ID.to_string(),
            engagement: format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:engagement/never-created/{AN_OVERRIDE_ID}"
            ),
            page: format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:page/never-created/{AN_OVERRIDE_ID}"
            ),
        }
    }

    /// The same absent resources named in the OTHER two partitions the modelled
    /// `SsmContactsArn` pattern admits (`aws-cn`, `aws-us-gov`) and, for one of them, with
    /// the empty Region field that pattern also admits — forms a client may legitimately
    /// send, and none of which the account-level answer may depend on.
    ///
    /// Source: `smithy_query(query="shape",shape="SsmContactsArn")`, whose `pattern` is
    /// `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=/,.@]*:[0-9]+:([\w+=/,.@:-]+)*$`.
    fn absent_in_other_partitions() -> Self {
        Self {
            case: "ARNs nothing created, in the aws-cn and aws-us-gov partitions and with an \
                   empty Region",
            contact: format!("arn:aws-cn:ssm-contacts:cn-north-1:{ACCOUNT}:contact/never-created"),
            channel: format!(
                "arn:aws-us-gov:ssm-contacts:us-gov-west-1:{ACCOUNT}:contact-channel/never-created/{AN_OVERRIDE_ID}"
            ),
            rotation: format!("arn:aws:ssm-contacts::{ACCOUNT}:rotation/{AN_OVERRIDE_ID}"),
            rotation_override_id: "11111111-2222-3333-4444-555555555555".to_string(),
            engagement: format!(
                "arn:aws-cn:ssm-contacts:cn-north-1:{ACCOUNT}:engagement/never-created/{AN_OVERRIDE_ID}"
            ),
            page: format!(
                "arn:aws-us-gov:ssm-contacts:us-gov-west-1:{ACCOUNT}:page/never-created/{AN_OVERRIDE_ID}"
            ),
        }
    }
}

/// Onboard the account, create one resource of every family through the wire, and answer
/// their ARNs — the flavour that proves the refusal does not depend on the subject being
/// absent.
///
/// The page comes from an engagement of a contact whose plan engages its channel, and is
/// read from the store because AWS never puts a page's accept code on the wire and this
/// suite only needs the `PageArn` the start minted.
async fn stored_subjects(emulator: &Emulator) -> Subjects {
    onboard(emulator);

    let contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
                "Tags": [{"Key": "team", "Value": "sre"}],
            }),
            "the suite's stored contact",
        )
        .await;
    let channel = emulator
        .create_channel(
            json!({
                "ContactId": contact,
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
            "the suite's stored channel",
        )
        .await;
    let (status, body) = emulator
        .call(
            "UpdateContact",
            json!({
                "ContactId": contact,
                "Plan": {
                    "Stages": [{
                        "DurationInMinutes": 5,
                        "Targets": [{"ChannelTargetInfo": {"ContactChannelId": channel}}],
                    }],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the suite's stored contact must take a plan engaging its channel, got {status} \
         {body:?}"
    );

    let rotation = emulator
        .create_rotation(&contact, "stored-rotation", "the suite's stored rotation")
        .await;
    let (status, body) = emulator
        .call(
            "CreateRotationOverride",
            json!({
                "RotationId": rotation,
                "NewContactIds": [contact],
                "StartTime": WINDOW_START,
                "EndTime": WINDOW_END,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the suite's stored rotation override must be created, got {status} {body:?}"
    );
    let rotation_override_id = body["RotationOverrideId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a RotationOverrideId, got {body:?}"))
        .to_string();

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

    let page = {
        let state =
            service_state(&*emulator.engine).expect("the engine holds this service's state");
        let state = state.lock();
        let mut pages: Vec<_> = state
            .pages
            .values()
            .filter(|page| page.engagement_arn == engagement)
            .collect();
        pages.sort_by_key(|page| page.sequence);
        pages
            .first()
            .unwrap_or_else(|| panic!("expected the start to have paged {channel}"))
            .arn
            .clone()
    };

    Subjects {
        case: "ARNs of resources this store holds",
        contact,
        channel,
        rotation,
        rotation_override_id,
        engagement,
        page,
    }
}

// ---------------------------------------------------------------------------
// The gated surface, as one table
// ---------------------------------------------------------------------------

/// Which of the two closed-gate wordings a request is refused with — the expectation this
/// suite states per row, from the resource family the request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wording {
    /// Reports the ACCOUNT, naming nothing of the request.
    Account,
    /// Reports the ARN the request named, verbatim.
    SubjectArn,
}

/// One gated request: the operation, the body, the resource ARN it names (when it names
/// one), and which wording its refusal must carry.
struct Row {
    operation: &'static str,
    request: Value,
    subject: Option<String>,
    wording: Wording,
}

/// Every operation of this service that consults the account-level precondition, driven
/// against `subjects`.
///
/// `wording` is stated per row from the family the row's subject belongs to: a rotation or
/// an engagement is addressed through the replication set, so its refusal names the ARN; a
/// contact, a contact channel, a page, and a request that resolves no resource at all
/// (`CreateContact`, `CreateRotation`, `ListRotations`) are refused about the account.
fn gated_requests(subjects: &Subjects) -> Vec<Row> {
    let contact = &subjects.contact;
    let channel = &subjects.channel;
    let rotation = &subjects.rotation;
    let engagement = &subjects.engagement;
    let page = &subjects.page;
    let override_id = &subjects.rotation_override_id;
    let recurrence = json!({
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
    });

    vec![
        // The contact registry.
        Row {
            operation: "CreateContact",
            request: json!({
                "Alias": "refused-by-the-gate",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
            }),
            subject: None,
            wording: Wording::Account,
        },
        Row {
            operation: "GetContact",
            request: json!({"ContactId": contact}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "UpdateContact",
            request: json!({"ContactId": contact, "DisplayName": "renamed"}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "DeleteContact",
            request: json!({"ContactId": contact}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        // Contact channels.
        Row {
            operation: "CreateContactChannel",
            request: json!({
                "ContactId": contact,
                "Name": "refused",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "refused@example.com"},
            }),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "GetContactChannel",
            request: json!({"ContactChannelId": channel}),
            subject: Some(channel.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "ListContactChannels",
            request: json!({"ContactId": contact}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "UpdateContactChannel",
            request: json!({"ContactChannelId": channel, "Name": "renamed"}),
            subject: Some(channel.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "DeleteContactChannel",
            request: json!({"ContactChannelId": channel}),
            subject: Some(channel.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "ActivateContactChannel",
            request: json!({"ContactChannelId": channel, "ActivationCode": "000000"}),
            subject: Some(channel.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "DeactivateContactChannel",
            request: json!({"ContactChannelId": channel}),
            subject: Some(channel.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "SendActivationCode",
            request: json!({"ContactChannelId": channel}),
            subject: Some(channel.clone()),
            wording: Wording::Account,
        },
        // On-call rotations and their overrides.
        Row {
            operation: "CreateRotation",
            request: json!({
                "Name": "refused-rotation",
                "ContactIds": [contact],
                "TimeZoneId": "UTC",
                "Recurrence": recurrence,
            }),
            subject: None,
            wording: Wording::Account,
        },
        Row {
            operation: "ListRotations",
            request: json!({"RotationNamePrefix": "stored"}),
            subject: None,
            wording: Wording::Account,
        },
        Row {
            operation: "GetRotation",
            request: json!({"RotationId": rotation}),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "UpdateRotation",
            request: json!({"RotationId": rotation, "Recurrence": recurrence}),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "DeleteRotation",
            request: json!({"RotationId": rotation}),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "CreateRotationOverride",
            request: json!({
                "RotationId": rotation,
                "NewContactIds": [contact],
                "StartTime": WINDOW_START,
                "EndTime": WINDOW_END,
            }),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "GetRotationOverride",
            request: json!({"RotationId": rotation, "RotationOverrideId": override_id}),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "ListRotationOverrides",
            request: json!({
                "RotationId": rotation,
                "StartTime": WINDOW_START,
                "EndTime": WINDOW_END,
            }),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "DeleteRotationOverride",
            request: json!({"RotationId": rotation, "RotationOverrideId": override_id}),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        // Engagements and the pages they send.
        Row {
            operation: "StartEngagement",
            request: json!({
                "ContactId": contact,
                "Sender": "cli",
                "Subject": "refused",
                "Content": "refused",
            }),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "StopEngagement",
            request: json!({"EngagementId": engagement}),
            subject: Some(engagement.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "DescribeEngagement",
            request: json!({"EngagementId": engagement}),
            subject: Some(engagement.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "ListPagesByEngagement",
            request: json!({"EngagementId": engagement}),
            subject: Some(engagement.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "ListPagesByContact",
            request: json!({"ContactId": contact}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "DescribePage",
            request: json!({"PageId": page}),
            subject: Some(page.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "AcceptPage",
            request: json!({"PageId": page, "AcceptType": "READ", "AcceptCode": "000000"}),
            subject: Some(page.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "ListPageReceipts",
            request: json!({"PageId": page}),
            subject: Some(page.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "ListPageResolutions",
            request: json!({"PageId": page}),
            subject: Some(page.clone()),
            wording: Wording::Account,
        },
        // Resource policies and tags.
        Row {
            operation: "GetContactPolicy",
            request: json!({"ContactArn": contact}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "PutContactPolicy",
            request: json!({"ContactArn": contact, "Policy": A_POLICY}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        Row {
            operation: "ListTagsForResource",
            request: json!({"ResourceARN": contact}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
        // The tagging trio takes any taggable ARN, so the family it names decides the
        // wording: a rotation here, a contact on its two siblings.
        Row {
            operation: "TagResource",
            request: json!({
                "ResourceARN": rotation,
                "Tags": [{"Key": "gate", "Value": "closed"}],
            }),
            subject: Some(rotation.clone()),
            wording: Wording::SubjectArn,
        },
        Row {
            operation: "UntagResource",
            request: json!({"ResourceARN": contact, "TagKeys": ["team"]}),
            subject: Some(contact.clone()),
            wording: Wording::Account,
        },
    ]
}

/// Every state of the account's replication-set record that is not ACTIVE.
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

/// The refusal the closed gate answers with: HTTP 400, the error code
/// `ValidationException`, a `Reason` inside the modelled value set, a present but empty
/// `Fields`, and exactly the `expected` wording — the account-level one (read from the
/// crate's own constant rather than copied), or the one that names the request's own ARN
/// after the fixed text AWS words it with. The other wording must NOT appear, which is what
/// keeps "onboard this account" and "that ARN names nothing" distinguishable.
fn assert_gate_wording(
    case: &str,
    status: u16,
    body: &Value,
    subject: Option<&str>,
    expected: Wording,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the account precondition to answer HTTP 400, got {status} \
         with {body:?}"
    );
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {:?} in {body:?}",
        error_code(body)
    );
    let message = body["Message"].as_str().unwrap_or_default();
    match expected {
        Wording::Account => assert_eq!(
            message, ACCOUNT_NOT_FOUND,
            "case {case:?}: expected the account-level refusal {ACCOUNT_NOT_FOUND:?}, \
             got {message:?} in {body:?}"
        ),
        Wording::SubjectArn => {
            let subject = subject.unwrap_or_else(|| {
                panic!("case {case:?}: an ARN-worded row must name the ARN it expects reported")
            });
            assert_eq!(
                message,
                format!("{INVALID_RESOURCE_ARN_PREFIX}{subject}"),
                "case {case:?}: expected the refusal to report the supplied ARN verbatim, got \
                 {message:?} in {body:?}"
            );
            assert_ne!(
                message, ACCOUNT_NOT_FOUND,
                "case {case:?}: a replication-set-scoped ARN may not be refused about the \
                 account, got {message:?} in {body:?}"
            );
        }
    }
    let reason = body["Reason"].as_str().unwrap_or_default();
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
        "case {case:?}: expected a Reason inside the modelled ValidationExceptionReason set \
         {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?} in {body:?}"
    );
    assert_eq!(
        body["Fields"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: a precondition refusal blames no member, so Fields must be present \
         and empty, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: one wording per resource family, over the whole gated surface
// ---------------------------------------------------------------------------

/// Every gated operation, over ARNs nothing created and the same ARNs in the other
/// partitions, is refused with the wording its subject's family decides — and the partition
/// and Region it is written in change nothing.
#[tokio::test]
async fn every_gated_operation_words_the_refusal_from_the_family_it_addressed() {
    for subjects in [Subjects::absent(), Subjects::absent_in_other_partitions()] {
        for (account_case, replication_set) in closed_gate_states() {
            let emulator = Emulator::fresh();
            set_replication_set(&emulator, replication_set.clone());

            for row in gated_requests(&subjects) {
                let case = format!(
                    "{} on {}, {account_case}",
                    row.operation, subjects.case
                );
                let (status, body) = emulator.call(row.operation, row.request.clone()).await;
                assert_gate_wording(&case, status, &body, row.subject.as_deref(), row.wording);
            }
        }
    }
}

/// The same table against ARNs the store really HOLDS: the gate is decided before the ARN is
/// resolved, so a stored subject is refused in exactly the same words as an absent one — and
/// the refused calls leave the stored resources as they were.
#[tokio::test]
async fn a_stored_subject_is_refused_in_the_same_words_as_an_absent_one() {
    for (account_case, replication_set) in closed_gate_states() {
        let emulator = Emulator::fresh();
        let subjects = stored_subjects(&emulator).await;

        set_replication_set(&emulator, replication_set.clone());
        for row in gated_requests(&subjects) {
            let case = format!("{} on {}, {account_case}", row.operation, subjects.case);
            let (status, body) = emulator.call(row.operation, row.request.clone()).await;
            assert_gate_wording(&case, status, &body, row.subject.as_deref(), row.wording);
        }

        // Onboarded again, the subjects the refused calls named are all still there: a
        // refusal that is decided before the ARN is resolved cannot have deleted, renamed or
        // acknowledged anything.
        onboard(&emulator);
        for (operation, request) in [
            ("GetContact", json!({"ContactId": subjects.contact})),
            (
                "GetContactChannel",
                json!({"ContactChannelId": subjects.channel}),
            ),
            ("GetRotation", json!({"RotationId": subjects.rotation})),
            (
                "DescribeEngagement",
                json!({"EngagementId": subjects.engagement}),
            ),
            ("DescribePage", json!({"PageId": subjects.page})),
        ] {
            let case = format!("{operation} after the refusals, {account_case}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the stored subject to still be reported, got {status} \
                 with {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: a value that is no ARN at all leaves the account wording
// ---------------------------------------------------------------------------

/// The complement: a value that is not an ARN of this service names no resource of any
/// family, so the closed gate reports the ACCOUNT — and once the account is onboarded the
/// same bytes are reported as that value being an invalid resource ARN. Both wordings stay
/// reachable, which is what lets a client tell "onboard this account" from "fix this ARN".
#[tokio::test]
async fn a_value_that_is_no_arn_is_refused_about_the_account_until_the_account_is_onboarded() {
    // `ResourceARN` is modelled as an `AmazonResourceName` (length only, no pattern), so a
    // value that is not an ARN at all reaches the handler and the service itself reports it.
    let not_an_arn = "my-contact";
    let requests = [
        ("ListTagsForResource", json!({"ResourceARN": not_an_arn})),
        (
            "TagResource",
            json!({"ResourceARN": not_an_arn, "Tags": [{"Key": "k", "Value": "v"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceARN": not_an_arn, "TagKeys": ["k"]}),
        ),
    ];

    for (operation, request) in &requests {
        // Gate closed: the answer is the account's, because the value names no
        // replication-set-scoped resource.
        let emulator = Emulator::fresh();
        let case = format!("{operation} on a value that is no ARN, account not onboarded");
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_gate_wording(&case, status, &body, Some(not_an_arn), Wording::Account);

        // Gate open: the same bytes are reported as the value being an invalid resource ARN.
        onboard(&emulator);
        let case = format!("{operation} on a value that is no ARN, account onboarded");
        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!("{INVALID_RESOURCE_ARN_PREFIX}{not_an_arn}"),
            "case {case:?}: expected the ARN-resolution refusal naming the supplied value, got \
             {message:?} in {body:?}"
        );
        assert_ne!(
            message, ACCOUNT_NOT_FOUND,
            "case {case:?}: an onboarded account may not be refused about the account, got \
             {message:?} in {body:?}"
        );
    }
}
