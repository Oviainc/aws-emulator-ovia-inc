// SPDX-License-Identifier: CC0-1.0

//! The replication-set-scoped data plane, end to end over the wire.
//!
//! One rule drives the first three suites, asserted over a TABLE of all ten operations
//! rather than one call each:
//!
//! > An SSM Contacts operation whose subject is scoped to the account's Incident Manager
//! > replication set — contact-channel activation, rotations, engagements, contact resource
//! > policies and resource tags — is refused BEFORE its target resource is resolved, and
//! > without writing anything, with the operation's own modelled `ValidationException`
//! > (HTTP 400) whenever the account has no ACTIVE replication set.
//!
//! The contact registry's own writes (`CreateContact`, `CreateContactChannel`,
//! `UpdateContact`, `DeleteContact`, `DeleteContactChannel`) are gated by the SAME account
//! precondition — `tests/all/replication_set_precondition.rs` is the suite about them — so
//! this suite's `Emulator` is onboarded before it creates the resources the ten gated
//! operations name. The contact registry's READS (`ListContacts`, `GetContact`) are the
//! positive control: they answer whatever the account's state is, which is what makes a
//! 400 from the ten gated operations a statement about the gate rather than about the
//! service being down.
//!
//! The remaining suites assert what each of those operations DOES once the gate is open,
//! each over a table spanning what its rule generalises over: the activation code that was
//! delivered to a channel, the recurrence shape and team of a rotation, the page size of a
//! listing, the policy document attached to a contact, and the by-key merge of a tag write.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    service_state, AccountReplicationSet, EngagementRecord,
};

use crate::contact_lifecycle::{error_code, parse, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{onboard, set_replication_set};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A daily recurrence, the smallest `RecurrenceSettings` that describes a schedule.
///
/// `NumberOfOnCalls` and `RecurrenceMultiplier` are the two members the model makes
/// required (ranges 1.. and 1..100); `DailySettings` is one of the three settings lists.
const DAILY_RECURRENCE: &str = r#"{
    "NumberOfOnCalls": 1,
    "RecurrenceMultiplier": 1,
    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}]
}"#;

/// A well-formed resource policy document.
const A_POLICY: &str = r#"{"Version": "2012-10-17", "Statement": []}"#;

/// A fixed wall-clock instant, as `StartEngagement` stamps one.
fn at_epoch_secs(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// The resources the gated operations name when the target EXISTS, and the never-created
/// ARNs they name when it does not.
struct Targets {
    contact: String,
    channel: String,
    rotation: String,
    engagement: String,
}

impl Targets {
    /// ARNs of resources this emulator really holds.
    async fn live(emulator: &Emulator) -> Self {
        onboard(emulator);
        let contact = emulator
            .create_contact(
                json!({
                    "Alias": "oncall-primary",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                    "Tags": [{"Key": "team", "Value": "sre"}],
                }),
                "the gated suite's live contact",
            )
            .await;
        let channel = create_channel(
            emulator,
            json!({
                "ContactId": contact,
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
            "the gated suite's live channel",
        )
        .await;
        let rotation = create_rotation(
            emulator,
            json!({
                "Name": "live-rotation",
                "ContactIds": [contact],
                "TimeZoneId": "UTC",
                "Recurrence": parse(DAILY_RECURRENCE),
            }),
            "the gated suite's live rotation",
        )
        .await;
        let engagement = engagement_arn("oncall-primary", "11111111-2222-3333-4444-555555555555");
        insert_engagement(
            emulator,
            EngagementRecord {
                arn: engagement.clone(),
                contact_arn: contact.clone(),
                sender: "cli".to_string(),
                subject: "disk pressure".to_string(),
                content: "node-7 is at 95%".to_string(),
                public_subject: None,
                public_content: None,
                incident_id: None,
                start_time: at_epoch_secs(1_767_225_600),
                stop_time: None,
                stop_reason: None,
            },
        );

        Self {
            contact,
            channel,
            rotation,
            engagement,
        }
    }

    /// Well-formed ARNs naming resources nothing ever created.
    fn absent() -> Self {
        Self {
            contact: format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
            channel: format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/never-created/00000000-0000-0000-0000-000000000000"
            ),
            rotation: format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/00000000-0000-0000-0000-000000000000"
            ),
            engagement: engagement_arn(
                "never-created",
                "00000000-0000-0000-0000-000000000000",
            ),
        }
    }
}

/// A well-formed `SsmContactsArn` for an engagement.
fn engagement_arn(alias: &str, id: &str) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:engagement/{alias}/{id}")
}

/// Put `record` in the store, the way `StartEngagement` does, so this suite's stored
/// engagement is fixed and independent of the clock: `DescribeEngagement` must report what
/// the store holds, not what the request said.
fn insert_engagement(emulator: &Emulator, record: EngagementRecord) {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    state.lock().engagements.insert(record.arn.clone(), record);
}

/// The activation code the store has delivered to `channel_arn`, if any.
///
/// AWS delivers it out of band and never returns it, so the store IS the channel's mailbox:
/// this is the test standing in for the contact reading their email.
fn delivered_code(emulator: &Emulator, channel_arn: &str) -> Option<String> {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    state
        .contact_channels
        .get(channel_arn)
        .and_then(|channel| channel.activation_code.clone())
}

/// `CreateContactChannel`, asserting it answered 200.
async fn create_channel(emulator: &Emulator, request: Value, case: &str) -> String {
    let (status, body) = emulator.call("CreateContactChannel", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateContactChannel to answer 200, got {status} {body:?}"
    );
    body["ContactChannelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ContactChannelArn, got {body:?}"))
        .to_string()
}

/// `CreateRotation`, asserting it answered 200.
async fn create_rotation(emulator: &Emulator, request: Value, case: &str) -> String {
    let (status, body) = emulator.call("CreateRotation", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation to answer 200, got {status} {body:?}"
    );
    body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string()
}

/// The `ActivationStatus` `GetContactChannel` reports for `channel_arn`.
async fn activation_status(emulator: &Emulator, channel_arn: &str, case: &str) -> String {
    let (status, body) = emulator
        .call("GetContactChannel", json!({"ContactChannelId": channel_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContactChannel to answer 200, got {status} {body:?}"
    );
    body["ActivationStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an ActivationStatus, got {body:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// The ten gated operations, as one table
// ---------------------------------------------------------------------------

/// One gated operation: how to build its request, and what an ONBOARDED account gets when
/// that request names a resource nothing created.
struct GatedOperation {
    operation: &'static str,
    body: fn(&Targets) -> Value,
    /// 404 for an operation that resolves a resource; 200 for the listing, which resolves
    /// none — an empty page is the right answer to "list what is not there".
    absent_status: u16,
}

const GATED_OPERATIONS: &[GatedOperation] = &[
    GatedOperation {
        operation: "ActivateContactChannel",
        body: |targets| json!({"ContactChannelId": targets.channel, "ActivationCode": "000000"}),
        absent_status: 404,
    },
    GatedOperation {
        operation: "SendActivationCode",
        body: |targets| json!({"ContactChannelId": targets.channel}),
        absent_status: 404,
    },
    GatedOperation {
        operation: "CreateRotation",
        body: |targets| {
            json!({
                "Name": "gated-rotation",
                "ContactIds": [targets.contact],
                "TimeZoneId": "UTC",
                "Recurrence": parse(DAILY_RECURRENCE),
            })
        },
        absent_status: 404,
    },
    GatedOperation {
        operation: "ListRotations",
        body: |_| json!({"RotationNamePrefix": "gated"}),
        absent_status: 200,
    },
    GatedOperation {
        operation: "DescribeEngagement",
        body: |targets| json!({"EngagementId": targets.engagement}),
        absent_status: 404,
    },
    GatedOperation {
        operation: "GetContactPolicy",
        body: |targets| json!({"ContactArn": targets.contact}),
        absent_status: 404,
    },
    GatedOperation {
        operation: "PutContactPolicy",
        body: |targets| json!({"ContactArn": targets.contact, "Policy": A_POLICY}),
        absent_status: 404,
    },
    GatedOperation {
        operation: "ListTagsForResource",
        body: |targets| json!({"ResourceARN": targets.contact}),
        absent_status: 404,
    },
    GatedOperation {
        operation: "TagResource",
        body: |targets| {
            json!({
                "ResourceARN": targets.rotation,
                "Tags": [{"Key": "gate", "Value": "refused"}],
            })
        },
        absent_status: 404,
    },
    GatedOperation {
        operation: "UntagResource",
        body: |targets| json!({"ResourceARN": targets.contact, "TagKeys": ["team"]}),
        absent_status: 404,
    },
];

/// Everything a refused data-plane call must leave alone, read straight from the store.
#[derive(Debug, PartialEq)]
struct Snapshot {
    contact_tags: Vec<(Option<String>, Option<String>)>,
    contact_policy: Option<String>,
    channel_status: String,
    channel_code: Option<String>,
    rotation_arns: Vec<String>,
    rotation_tags: Vec<(Option<String>, Option<String>)>,
}

fn snapshot(emulator: &Emulator, targets: &Targets) -> Snapshot {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    let contact = state
        .contacts
        .get(&targets.contact)
        .expect("the live contact is in the store");
    let channel = state
        .contact_channels
        .get(&targets.channel)
        .expect("the live channel is in the store");
    let rotation = state
        .rotations
        .get(&targets.rotation)
        .expect("the live rotation is in the store");
    Snapshot {
        contact_tags: contact
            .tags
            .iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect(),
        contact_policy: contact.policy.clone(),
        channel_status: channel.activation_status.clone(),
        channel_code: channel.activation_code.clone(),
        rotation_arns: state.rotations.keys().cloned().collect(),
        rotation_tags: rotation
            .tags
            .iter()
            .map(|tag| (tag.key.clone(), tag.value.clone()))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Rule: the account gate, for every one of the ten operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_gated_operation_is_refused_without_an_active_replication_set() {
    // The two ways an account has no ACTIVE replication set: none at all, and one that has
    // not finished replicating.
    let no_active_set: &[(&str, Option<AccountReplicationSet>)] = &[
        ("no replication set at all", None),
        (
            "a replication set that is not active yet",
            Some(AccountReplicationSet {
                arn: format!("arn:aws:ssm-incidents::{ACCOUNT}:replication-set/emulator-set"),
                active: false,
            }),
        ),
    ];

    for (account_case, replication_set) in no_active_set {
        let emulator = Emulator::new();
        // The fixtures are created while the account IS onboarded, so the only thing that
        // differs when the table runs is the precondition.
        let live = Targets::live(&emulator).await;
        let absent = Targets::absent();
        let before = snapshot(&emulator, &live);

        for (target_case, targets) in [("a stored resource", &live), ("an absent resource", &absent)]
        {
            for gated in GATED_OPERATIONS {
                set_replication_set(&emulator, replication_set.clone());
                let case = format!("{} on {target_case}, {account_case}", gated.operation);

                let (status, body) = emulator.call(gated.operation, (gated.body)(targets)).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account precondition to answer 400, \
                     got {status} {body:?}"
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case:?}: expected a ValidationException — NOT a \
                     ResourceNotFoundException and NOT a NotImplemented, even for an absent \
                     resource — got {body:?}"
                );

                // And the refusal wrote nothing: no tag, no policy, no rotation, no
                // activation.
                onboard(&emulator);
                let after = snapshot(&emulator, &live);
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused call must write nothing, got {after:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_onboarded_account_naming_an_absent_resource_gets_the_modelled_answer() {
    let emulator = Emulator::new();
    // One live set of resources, so "absent" cannot be confused with "the store is empty".
    let _live = Targets::live(&emulator).await;
    let absent = Targets::absent();

    for gated in GATED_OPERATIONS {
        let case = gated.operation;
        let (status, body) = emulator.call(gated.operation, (gated.body)(&absent)).await;
        assert_eq!(
            status, gated.absent_status,
            "case {case:?}: expected HTTP {}, got {status} {body:?}",
            gated.absent_status
        );
        if gated.absent_status == 404 {
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn no_gated_operation_answers_not_implemented() {
    // The 501 channel is the defect this suite exists for: whichever way the account is
    // configured, no gated operation may answer "not written yet".
    let emulator = Emulator::new();
    let live = Targets::live(&emulator).await;

    for onboarded in [true, false] {
        for gated in GATED_OPERATIONS {
            if onboarded {
                onboard(&emulator);
            } else {
                set_replication_set(&emulator, None);
            }
            let case = format!("{} with onboarded={onboarded}", gated.operation);
            let (status, body) = emulator.call(gated.operation, (gated.body)(&live)).await;
            assert_ne!(
                status, 501,
                "case {case:?}: expected a modelled answer, got {status} {body:?}"
            );
            assert!(
                !error_code(&body).contains("NotImplemented"),
                "case {case:?}: expected a modelled answer, got {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ActivateContactChannel / SendActivationCode
// ---------------------------------------------------------------------------

/// A channel to activate: the model's whole `ChannelType` value set with the address form
/// each one documents, and `DeferActivation` absent, false and true.
struct ChannelRow {
    case: &'static str,
    name: &'static str,
    channel_type: &'static str,
    address: &'static str,
    defer_activation: Option<bool>,
}

const CHANNEL_ROWS: &[ChannelRow] = &[
    ChannelRow {
        case: "an email channel with DeferActivation absent",
        name: "email",
        channel_type: "EMAIL",
        address: "ops@example.com",
        defer_activation: None,
    },
    ChannelRow {
        case: "an SMS channel with DeferActivation false",
        name: "sms",
        channel_type: "SMS",
        address: "+15550100",
        defer_activation: Some(false),
    },
    ChannelRow {
        case: "a voice channel with activation deferred",
        name: "voice",
        channel_type: "VOICE",
        address: "+15550101",
        defer_activation: Some(true),
    },
];

/// Create the contact every channel row hangs off, onboarded.
async fn onboarded_contact(emulator: &Emulator, alias: &str) -> String {
    onboard(emulator);
    emulator
        .create_contact(
            json!({
                "Alias": alias,
                "Type": "PERSONAL",
                "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
            }),
            alias,
        )
        .await
}

#[tokio::test]
async fn a_channel_activates_only_with_the_code_that_was_delivered_to_it() {
    for row in CHANNEL_ROWS {
        let emulator = Emulator::new();
        let contact = onboarded_contact(&emulator, "oncall-primary").await;
        let mut request = json!({
            "ContactId": contact,
            "Name": row.name,
            "Type": row.channel_type,
            "DeliveryAddress": {"SimpleAddress": row.address},
        });
        if let Some(defer) = row.defer_activation {
            request["DeferActivation"] = json!(defer);
        }
        let channel = create_channel(&emulator, request, row.case).await;

        // A channel created with activation deferred has had no code delivered; one created
        // without has.
        let deferred = row.defer_activation.unwrap_or(false);
        assert_eq!(
            delivered_code(&emulator, &channel).is_some(),
            !deferred,
            "case {:?}: a code is delivered at creation exactly when activation is not \
             deferred",
            row.case
        );
        assert_eq!(
            activation_status(&emulator, &channel, row.case).await,
            "NOT_ACTIVATED",
            "case {:?}: a created channel starts unactivated",
            row.case
        );

        // Codes that were NOT delivered to this channel: each is refused with a 400
        // ValidationException and leaves the channel unactivated.
        let wrong_codes: &[&str] = &["000000", "999999", "1234567890"];
        for wrong in wrong_codes {
            if delivered_code(&emulator, &channel).as_deref() == Some(*wrong) {
                continue;
            }
            let case = format!("{}, activated with the wrong code {wrong}", row.case);
            let (status, body) = emulator
                .call(
                    "ActivateContactChannel",
                    json!({"ContactChannelId": channel, "ActivationCode": wrong}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            assert_eq!(
                activation_status(&emulator, &channel, &case).await,
                "NOT_ACTIVATED",
                "case {case:?}: a refused activation must not activate the channel"
            );
        }

        // `SendActivationCode` delivers a code — to a deferred channel, which had none, and
        // again to one that already had one, superseding it.
        let superseded = delivered_code(&emulator, &channel);
        let (status, body) = emulator
            .call("SendActivationCode", json!({"ContactChannelId": channel}))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected SendActivationCode to answer 200, got {status} {body:?}",
            row.case
        );
        assert!(
            body == json!({}) || body.is_null(),
            "case {:?}: SendActivationCode models an empty output, got {body:?}",
            row.case
        );
        let code = delivered_code(&emulator, &channel)
            .unwrap_or_else(|| panic!("case {:?}: a code was delivered", row.case));
        assert!(
            code.len() >= 6 && code.len() <= 10 && code.chars().all(|c| c.is_ascii_digit()),
            "case {:?}: a delivered code must satisfy the modelled ActivationCode \
             (len 6..10, ^[0-9]*$), got {code:?}",
            row.case
        );
        assert_eq!(
            activation_status(&emulator, &channel, row.case).await,
            "NOT_ACTIVATED",
            "case {:?}: sending a code must not activate the channel",
            row.case
        );

        // The superseded code no longer activates.
        if let Some(superseded) = superseded.filter(|old| *old != code) {
            let case = format!("{}, activated with the superseded code", row.case);
            let (status, body) = emulator
                .call(
                    "ActivateContactChannel",
                    json!({"ContactChannelId": channel, "ActivationCode": superseded}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: only the latest code activates, got {status} {body:?}"
            );
        }

        // The delivered code does — twice, because activation is idempotent.
        for attempt in 1..=2 {
            let case = format!("{}, activated with the delivered code", row.case);
            let (status, body) = emulator
                .call(
                    "ActivateContactChannel",
                    json!({"ContactChannelId": channel, "ActivationCode": code}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: expected HTTP 200, got {status} {body:?}"
            );
            assert_eq!(
                activation_status(&emulator, &channel, &case).await,
                "ACTIVATED",
                "case {case:?} attempt {attempt}: the channel must read back ACTIVATED"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CreateRotation / ListRotations
// ---------------------------------------------------------------------------

/// A rotation to create. The rows differ along what the create/list round trip generalises
/// over: which of `RecurrenceSettings`' three settings lists the recurrence carries (and a
/// `ShiftCoverages` map beside one), the size of the team, whether `StartTime` and `Tags`
/// are named, and the time zone.
struct RotationRow {
    case: &'static str,
    name: &'static str,
    /// How many of the fixture's contacts are on the team, in listing order.
    team: usize,
    time_zone_id: &'static str,
    recurrence: &'static str,
    start_time_secs: Option<i64>,
    tags: Option<&'static str>,
}

const ROTATION_ROWS: &[RotationRow] = &[
    RotationRow {
        case: "a daily rotation with one contact and no start time",
        name: "rota-daily",
        team: 1,
        time_zone_id: "UTC",
        recurrence: DAILY_RECURRENCE,
        start_time_secs: None,
        tags: None,
    },
    RotationRow {
        case: "a weekly rotation with two contacts, a start time and tags",
        name: "rota-weekly",
        team: 2,
        time_zone_id: "America/Los_Angeles",
        recurrence: r#"{
            "NumberOfOnCalls": 1,
            "RecurrenceMultiplier": 2,
            "WeeklySettings": [
                {"DayOfWeek": "MON", "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 30}},
                {"DayOfWeek": "FRI", "HandOffTime": {"HourOfDay": 17, "MinuteOfHour": 0}}
            ]
        }"#,
        start_time_secs: Some(1_767_225_600),
        tags: Some(r#"[{"Key": "team", "Value": "sre"}, {"Key": "tier", "Value": "1"}]"#),
    },
    RotationRow {
        case: "a monthly rotation at the recurrence multiplier's published maximum",
        name: "rota-monthly",
        team: 3,
        time_zone_id: "Asia/Seoul",
        recurrence: r#"{
            "NumberOfOnCalls": 2,
            "RecurrenceMultiplier": 100,
            "MonthlySettings": [
                {"DayOfMonth": 1, "HandOffTime": {"HourOfDay": 0, "MinuteOfHour": 0}},
                {"DayOfMonth": 28, "HandOffTime": {"HourOfDay": 23, "MinuteOfHour": 59}}
            ]
        }"#,
        start_time_secs: Some(0),
        tags: None,
    },
    RotationRow {
        case: "a weekly rotation whose coverage names the days it covers",
        name: "rota-covered",
        team: 1,
        time_zone_id: "UTC",
        recurrence: r#"{
            "NumberOfOnCalls": 1,
            "RecurrenceMultiplier": 1,
            "WeeklySettings": [
                {"DayOfWeek": "WED", "HandOffTime": {"HourOfDay": 8, "MinuteOfHour": 0}}
            ],
            "ShiftCoverages": {
                "WED": [{
                    "Start": {"HourOfDay": 8, "MinuteOfHour": 0},
                    "End": {"HourOfDay": 18, "MinuteOfHour": 0}
                }]
            }
        }"#,
        start_time_secs: None,
        tags: None,
    },
];

/// Three contacts, so a row can put one, two or three of them on its team.
async fn rotation_team(emulator: &Emulator) -> Vec<String> {
    onboard(emulator);
    let mut team = Vec::new();
    for alias in ["rota-member-1", "rota-member-2", "rota-member-3"] {
        team.push(
            emulator
                .create_contact(
                    json!({
                        "Alias": alias,
                        "Type": "PERSONAL",
                        "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                    }),
                    alias,
                )
                .await,
        );
    }
    team
}

/// The `CreateRotation` body a row creates its rotation with.
fn create_rotation_request(row: &RotationRow, team: &[String]) -> Value {
    let mut request = json!({
        "Name": row.name,
        "ContactIds": team[..row.team].to_vec(),
        "TimeZoneId": row.time_zone_id,
        "Recurrence": parse(row.recurrence),
    });
    if let Some(secs) = row.start_time_secs {
        request["StartTime"] = json!(secs);
    }
    if let Some(tags) = row.tags {
        request["Tags"] = parse(tags);
    }
    request
}

/// Every rotation `ListRotations` reports for `request`, paging to exhaustion, asserting no
/// page is larger than the `MaxResults` the request names.
async fn all_rotations(emulator: &Emulator, request: Value, case: &str) -> Vec<Value> {
    let mut request = request;
    let limit = request["MaxResults"].as_i64();
    let mut rotations: Vec<Value> = Vec::new();
    loop {
        let (status, body) = emulator.call("ListRotations", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRotations to answer 200, got {status} {body:?}"
        );
        let page = body["Rotations"].as_array().cloned().unwrap_or_default();
        if let Some(limit) = limit.filter(|limit| *limit > 0) {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: a page held {} entries for MaxResults={limit}",
                page.len()
            );
        }
        rotations.extend(page);
        match body["NextToken"].as_str() {
            Some(token) => {
                assert!(
                    rotations.len() <= 64,
                    "case {case:?}: ListRotations paging did not terminate, collected {} \
                     entries",
                    rotations.len()
                );
                request["NextToken"] = json!(token);
            }
            None => return rotations,
        }
    }
}

#[tokio::test]
async fn a_created_rotation_reads_back_exactly_as_it_was_created() {
    let emulator = Emulator::new();
    let team = rotation_team(&emulator).await;

    let mut created: Vec<(&RotationRow, String)> = Vec::new();
    for row in ROTATION_ROWS {
        let request = create_rotation_request(row, &team);
        let arn = create_rotation(&emulator, request.clone(), row.case).await;
        assert!(
            arn.starts_with(&format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/")),
            "case {:?}: expected a rotation ARN in the request's region, got {arn:?}",
            row.case
        );
        created.push((row, arn));
    }

    // Each rotation reads back through `ListRotations` with exactly what its create named.
    for (row, arn) in &created {
        let case = row.case;
        let reported = all_rotations(&emulator, json!({"RotationNamePrefix": row.name}), case).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the prefix to select exactly this rotation, \
             got {reported:?}"
        );
        let rotation = &reported[0];
        assert_eq!(
            rotation["RotationArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the created ARN, got {rotation:?}"
        );
        assert_eq!(
            rotation["Name"].as_str(),
            Some(row.name),
            "case {case:?}: expected the created Name, got {rotation:?}"
        );
        assert_eq!(
            rotation["ContactIds"],
            json!(team[..row.team].to_vec()),
            "case {case:?}: expected the team in the order the create listed it, \
             got {rotation:?}"
        );
        assert_eq!(
            rotation["TimeZoneId"].as_str(),
            Some(row.time_zone_id),
            "case {case:?}: expected the created TimeZoneId, got {rotation:?}"
        );
        assert_eq!(
            rotation["Recurrence"],
            parse(row.recurrence),
            "case {case:?}: expected the created Recurrence, got {rotation:?}"
        );
        assert_eq!(
            rotation["StartTime"].as_f64(),
            row.start_time_secs.map(|secs| secs as f64),
            "case {case:?}: expected the created StartTime, got {rotation:?}"
        );
        // The tags the create carried are reached through `ListTagsForResource`, not
        // through the rotation summary.
        let (status, body) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["Tags"],
            row.tags.map(parse).unwrap_or_else(|| json!([])),
            "case {case:?}: expected the tags the create carried, got {body:?}"
        );
    }

    // And the unfiltered listing reports every one of them, in ARN order, whatever the page
    // size — including the sizes that split the result.
    let mut expected: Vec<String> = created.iter().map(|(_, arn)| arn.clone()).collect();
    expected.sort();
    for max_results in [None, Some(1), Some(2), Some(3), Some(1024)] {
        let case = format!("the whole listing at MaxResults={max_results:?}");
        let mut request = json!({});
        if let Some(limit) = max_results {
            request["MaxResults"] = json!(limit);
        }
        let reported: Vec<String> = all_rotations(&emulator, request, &case)
            .await
            .iter()
            .map(|rotation| rotation["RotationArn"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: paging must report every rotation exactly once, in ARN order"
        );
    }
}

#[tokio::test]
async fn two_rotations_may_share_a_name_and_a_token_replays_the_first_create() {
    let emulator = Emulator::new();
    let team = rotation_team(&emulator).await;
    let request = create_rotation_request(&ROTATION_ROWS[0], &team);

    // The model declares no conflict error for `CreateRotation`, so the same name creates a
    // second rotation with its own ARN.
    let first = create_rotation(&emulator, request.clone(), "the first create").await;
    let second = create_rotation(&emulator, request.clone(), "the second create").await;
    assert_ne!(
        first, second,
        "two creates of the same name must mint two ARNs, got {first:?} twice"
    );

    // With an `IdempotencyToken`, a retry is the SAME call.
    let mut tokened = request.clone();
    tokened["IdempotencyToken"] = json!("rota-token-1");
    let third = create_rotation(&emulator, tokened.clone(), "the tokened create").await;
    let replayed = create_rotation(&emulator, tokened, "the retried create").await;
    assert_eq!(
        third, replayed,
        "a retry carrying the same token must answer with the ARN already minted"
    );

    let reported = all_rotations(&emulator, json!({}), "after three creates").await;
    assert_eq!(
        reported.len(),
        3,
        "expected exactly three rotations, got {reported:?}"
    );
}

#[tokio::test]
async fn a_rotation_request_outside_the_rule_is_refused() {
    let emulator = Emulator::new();
    let team = rotation_team(&emulator).await;
    let absent = Targets::absent();

    // 400: the recurrence names no schedule, or an enum member is outside the model's value
    // set (`DAY_OF_WEEK_VALUES` in src/ops/types/shared_types.rs).
    let refused_400: &[(&str, Value)] = &[
        (
            "a recurrence with none of the three settings lists",
            json!({
                "Name": "rota-empty",
                "ContactIds": [team[0]],
                "TimeZoneId": "UTC",
                "Recurrence": {"NumberOfOnCalls": 1, "RecurrenceMultiplier": 1},
            }),
        ),
        (
            "a recurrence whose settings lists are all empty",
            json!({
                "Name": "rota-empty-lists",
                "ContactIds": [team[0]],
                "TimeZoneId": "UTC",
                "Recurrence": {
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                    "DailySettings": [],
                    "WeeklySettings": [],
                    "MonthlySettings": [],
                },
            }),
        ),
        (
            "a weekly setting naming a day outside DayOfWeek",
            json!({
                "Name": "rota-badday",
                "ContactIds": [team[0]],
                "TimeZoneId": "UTC",
                "Recurrence": {
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                    "WeeklySettings": [
                        {"DayOfWeek": "MONDAY", "HandOffTime": {"HourOfDay": 9, "MinuteOfHour": 0}}
                    ],
                },
            }),
        ),
        (
            "a shift coverage keyed by a day outside DayOfWeek",
            json!({
                "Name": "rota-badcoverage",
                "ContactIds": [team[0]],
                "TimeZoneId": "UTC",
                "Recurrence": {
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                    "ShiftCoverages": {
                        "Monday": [{
                            "Start": {"HourOfDay": 8, "MinuteOfHour": 0},
                            "End": {"HourOfDay": 18, "MinuteOfHour": 0}
                        }]
                    },
                },
            }),
        ),
        // A member outside its modelled constraint is refused at the boundary, before the
        // handler runs: `RecurrenceMultiplier` range 1..100, `NumberOfOnCalls` range 1..,
        // `Name` pattern ^[a-zA-Z0-9_\-\s\.]*$, `ContactIds` length 1..30.
        (
            "a recurrence multiplier past its published maximum",
            json!({
                "Name": "rota-multiplier",
                "ContactIds": [team[0]],
                "TimeZoneId": "UTC",
                "Recurrence": {
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 101,
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                },
            }),
        ),
        (
            "a number of on-calls below its published minimum",
            json!({
                "Name": "rota-oncalls",
                "ContactIds": [team[0]],
                "TimeZoneId": "UTC",
                "Recurrence": {
                    "NumberOfOnCalls": 0,
                    "RecurrenceMultiplier": 1,
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                },
            }),
        ),
        (
            "a name outside its modelled pattern",
            json!({
                "Name": "rota/slash",
                "ContactIds": [team[0]],
                "TimeZoneId": "UTC",
                "Recurrence": parse(DAILY_RECURRENCE),
            }),
        ),
        (
            "an empty contact list",
            json!({
                "Name": "rota-noteam",
                "ContactIds": [],
                "TimeZoneId": "UTC",
                "Recurrence": parse(DAILY_RECURRENCE),
            }),
        ),
    ];

    for (case, request) in refused_400 {
        let (status, body) = emulator.call("CreateRotation", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    // 404: a team member that names no stored contact, wherever it sits in the list.
    let refused_404: &[(&str, Value)] = &[
        (
            "the only team member is absent",
            json!([absent.contact.clone()]),
        ),
        (
            "the first team member is absent",
            json!([absent.contact.clone(), team[0].clone()]),
        ),
        (
            "the last team member is absent",
            json!([team[0].clone(), absent.contact.clone()]),
        ),
    ];
    for (case, contact_ids) in refused_404 {
        let (status, body) = emulator
            .call(
                "CreateRotation",
                json!({
                    "Name": "rota-absent",
                    "ContactIds": contact_ids,
                    "TimeZoneId": "UTC",
                    "Recurrence": parse(DAILY_RECURRENCE),
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
    }

    // And nothing above created a rotation.
    let reported = all_rotations(&emulator, json!({}), "after the refused creates").await;
    assert!(
        reported.is_empty(),
        "a refused create must write nothing, got {reported:?}"
    );

    // A `NextToken` this listing did not issue is refused rather than silently restarting
    // the scan.
    for token in ["not-a-token", "deadbeef", "00"] {
        let (status, body) = emulator
            .call("ListRotations", json!({"NextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a token this list did not issue, \
             got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {token:?}: expected a ValidationException, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DescribeEngagement
// ---------------------------------------------------------------------------

/// An engagement in the store, and what `DescribeEngagement` must report for it. The rows
/// differ along which optional members the record carries: none, all, and a stopped
/// engagement.
struct EngagementRow {
    case: &'static str,
    id: &'static str,
    public_subject: Option<&'static str>,
    public_content: Option<&'static str>,
    incident_id: Option<&'static str>,
    start_secs: u64,
    stop_secs: Option<u64>,
}

const ENGAGEMENT_ROWS: &[EngagementRow] = &[
    EngagementRow {
        case: "an engagement carrying only its required members",
        id: "11111111-1111-1111-1111-111111111111",
        public_subject: None,
        public_content: None,
        incident_id: None,
        start_secs: 1_767_225_600,
        stop_secs: None,
    },
    EngagementRow {
        case: "an engagement carrying every optional member",
        id: "22222222-2222-2222-2222-222222222222",
        public_subject: Some("disk pressure"),
        public_content: Some("a node is low on disk"),
        incident_id: Some("arn:aws:ssm-incidents::000000000000:incident-record/x/1"),
        start_secs: 1_767_225_600,
        stop_secs: None,
    },
    EngagementRow {
        case: "a stopped engagement",
        id: "33333333-3333-3333-3333-333333333333",
        public_subject: None,
        public_content: None,
        incident_id: None,
        start_secs: 1_767_225_600,
        stop_secs: Some(1_767_229_200),
    },
];

#[tokio::test]
async fn describe_engagement_reports_what_the_store_holds() {
    let emulator = Emulator::new();
    let contact = onboarded_contact(&emulator, "oncall-primary").await;

    for row in ENGAGEMENT_ROWS {
        let arn = engagement_arn("oncall-primary", row.id);
        insert_engagement(
            &emulator,
            EngagementRecord {
                arn: arn.clone(),
                contact_arn: contact.clone(),
                sender: "cli".to_string(),
                subject: format!("subject for {}", row.id),
                content: format!("content for {}", row.id),
                public_subject: row.public_subject.map(str::to_string),
                public_content: row.public_content.map(str::to_string),
                incident_id: row.incident_id.map(str::to_string),
                start_time: at_epoch_secs(row.start_secs),
                stop_time: row.stop_secs.map(at_epoch_secs),
                stop_reason: None,
            },
        );

        let case = row.case;
        let (status, body) = emulator
            .call("DescribeEngagement", json!({"EngagementId": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            body["EngagementArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the stored ARN, got {body:?}"
        );
        assert_eq!(
            body["ContactArn"].as_str(),
            Some(contact.as_str()),
            "case {case:?}: expected the engaged contact, got {body:?}"
        );
        assert_eq!(
            body["Subject"].as_str(),
            Some(format!("subject for {}", row.id).as_str()),
            "case {case:?}: expected the stored Subject, got {body:?}"
        );
        assert_eq!(
            body["Content"].as_str(),
            Some(format!("content for {}", row.id).as_str()),
            "case {case:?}: expected the stored Content, got {body:?}"
        );
        assert_eq!(
            body["PublicSubject"].as_str(),
            row.public_subject,
            "case {case:?}: expected the stored PublicSubject, got {body:?}"
        );
        assert_eq!(
            body["PublicContent"].as_str(),
            row.public_content,
            "case {case:?}: expected the stored PublicContent, got {body:?}"
        );
        assert_eq!(
            body["IncidentId"].as_str(),
            row.incident_id,
            "case {case:?}: expected the stored IncidentId, got {body:?}"
        );
        assert_eq!(
            body["StartTime"].as_f64(),
            Some(row.start_secs as f64),
            "case {case:?}: expected the stored StartTime, got {body:?}"
        );
        assert_eq!(
            body["StopTime"].as_f64(),
            row.stop_secs.map(|secs| secs as f64),
            "case {case:?}: expected the stored StopTime, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// PutContactPolicy / GetContactPolicy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_contacts_policy_is_the_last_one_put() {
    let emulator = Emulator::new();
    let contact = onboarded_contact(&emulator, "oncall-primary").await;

    // A contact that has had no put carries no policy.
    let (status, body) = emulator
        .call("GetContactPolicy", json!({"ContactArn": contact}))
        .await;
    assert_eq!(
        status, 200,
        "expected GetContactPolicy to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ContactArn"].as_str(),
        Some(contact.as_str()),
        "expected the contact's own ARN, got {body:?}"
    );
    assert!(
        body.get("Policy").is_none_or(Value::is_null),
        "a contact with no policy must report none, got {body:?}"
    );

    // The rows differ along the shape of the document: an empty object, a statement list, a
    // nested condition, and one at the modelled Policy's minimum length past `{}`.
    let documents: &[(&str, &str)] = &[
        ("an empty policy object", "{}"),
        (
            "a policy with one statement",
            r#"{"Version": "2012-10-17", "Statement": [
                {"Sid": "ShareContact", "Effect": "Allow",
                 "Principal": {"AWS": "000000000000"},
                 "Action": "ssm-contacts:GetContact",
                 "Resource": "*"}
            ]}"#,
        ),
        (
            "a policy whose statement carries a nested condition",
            r#"{"Version": "2012-10-17", "Statement": [
                {"Effect": "Deny", "Action": ["ssm-contacts:*"], "Resource": "*",
                 "Condition": {"StringNotEquals": {"aws:PrincipalOrgID": ["o-1234567890"]}}}
            ]}"#,
        ),
    ];

    for (case, document) in documents {
        let (status, body) = emulator
            .call(
                "PutContactPolicy",
                json!({"ContactArn": contact, "Policy": document}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutContactPolicy to answer 200, got {status} {body:?}"
        );
        assert!(
            body == json!({}) || body.is_null(),
            "case {case:?}: PutContactPolicy models an empty output, got {body:?}"
        );

        let (status, body) = emulator
            .call("GetContactPolicy", json!({"ContactArn": contact}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetContactPolicy to answer 200, got {status} {body:?}"
        );
        // The policy reads back verbatim, and a put REPLACES rather than accumulates: the
        // reported document is the last one put, not a concatenation.
        assert_eq!(
            body["Policy"].as_str(),
            Some(*document),
            "case {case:?}: expected the document the last put carried, got {body:?}"
        );
    }

    // A document that is not a JSON object is refused, and leaves the stored policy alone.
    let stored = documents.last().expect("the table is not empty").1;
    let refused: &[(&str, &str)] = &[
        ("a document that is not JSON at all", "not json"),
        ("a JSON array", "[1, 2, 3]"),
        ("a JSON string", "\"policy\""),
        ("a JSON number", "17"),
        ("a truncated object", "{\"Version\":"),
    ];
    for (case, document) in refused {
        let (status, body) = emulator
            .call(
                "PutContactPolicy",
                json!({"ContactArn": contact, "Policy": document}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );

        let (_, body) = emulator
            .call("GetContactPolicy", json!({"ContactArn": contact}))
            .await;
        assert_eq!(
            body["Policy"].as_str(),
            Some(stored),
            "case {case:?}: a refused put must write nothing, got {body:?}"
        );
    }

    // Only contacts carry a resource policy: a rotation's ARN is not a contact's.
    let team = rotation_team(&emulator).await;
    let rotation = create_rotation(
        &emulator,
        create_rotation_request(&ROTATION_ROWS[0], &team),
        "a rotation to read a policy off",
    )
    .await;
    for operation in ["GetContactPolicy", "PutContactPolicy"] {
        let (status, body) = emulator
            .call(
                operation,
                json!({"ContactArn": rotation, "Policy": A_POLICY}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {operation:?}: expected HTTP 404 for a non-contact ARN, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {operation:?}: expected a ResourceNotFoundException, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// TagResource / UntagResource / ListTagsForResource
// ---------------------------------------------------------------------------

/// A tag write to apply. The rows differ along what the merge rule generalises over: the
/// number of tags in the batch (none, one, several), whether a key is already on the
/// resource, and the value's form — the modelled `TagKey` (len 1..128, pattern
/// `^[\\\/a-zA-Z0-9_+=\-]*$`) and `TagValue` (len 1..256, pattern
/// `^[\p{L}\p{Z}\p{N}_.:\/=+\-@]*$`) at their published maxima and with a non-ASCII value.
struct TagWrite {
    case: &'static str,
    tags: &'static str,
}

const TAG_WRITES: &[TagWrite] = &[
    TagWrite {
        case: "an empty batch",
        tags: "[]",
    },
    TagWrite {
        case: "one new key",
        tags: r#"[{"Key": "team", "Value": "sre"}]"#,
    },
    TagWrite {
        case: "the same key with another value",
        tags: r#"[{"Key": "team", "Value": "platform"}]"#,
    },
    TagWrite {
        case: "two new keys beside it",
        tags: r#"[{"Key": "tier", "Value": "1"}, {"Key": "env", "Value": "prod"}]"#,
    },
    TagWrite {
        case: "a batch that both replaces and adds",
        tags: r#"[{"Key": "env", "Value": "staging"}, {"Key": "owner", "Value": "ops@example.com"}]"#,
    },
    TagWrite {
        case: "a non-ASCII value the modelled TagValue pattern admits",
        tags: r#"[{"Key": "equipe", "Value": "Équipe Nuit 2"}]"#,
    },
];

/// Apply `write` to `expected` the way the rule says: a key already present takes the new
/// value, a key not present is appended.
fn merge(expected: &mut Vec<(String, String)>, write: &Value) {
    for tag in write.as_array().cloned().unwrap_or_default() {
        let key = tag["Key"].as_str().unwrap_or_default().to_string();
        let value = tag["Value"].as_str().unwrap_or_default().to_string();
        match expected.iter_mut().find(|(stored, _)| *stored == key) {
            Some(entry) => entry.1 = value,
            None => expected.push((key, value)),
        }
    }
}

/// The tags `ListTagsForResource` reports for `arn`, as key/value pairs in report order.
async fn reported_tags(emulator: &Emulator, arn: &str, case: &str) -> Vec<(String, String)> {
    let (status, body) = emulator
        .call("ListTagsForResource", json!({"ResourceARN": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {status} {body:?}"
    );
    body["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_tag_write_merges_by_key_on_every_taggable_resource() {
    let emulator = Emulator::new();
    let team = rotation_team(&emulator).await;
    let rotation = create_rotation(
        &emulator,
        create_rotation_request(&ROTATION_ROWS[0], &team),
        "a rotation to tag",
    )
    .await;
    let tagged_contact = emulator
        .create_contact(
            json!({
                "Alias": "tagged-contact",
                "Type": "ESCALATION",
                "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
                "Tags": [{"Key": "created", "Value": "withtags"}],
            }),
            "a contact created with tags",
        )
        .await;

    /// A taggable resource to drive the merge rule over, and the tags its create left on
    /// it.
    struct TaggableResource {
        case: &'static str,
        arn: String,
        initial: Vec<(String, String)>,
    }

    // Both taggable kinds, one of which was created WITH a tag: the merge rule is the same.
    let resources = vec![
        TaggableResource {
            case: "a contact created with no tags",
            arn: team[0].clone(),
            initial: Vec::new(),
        },
        TaggableResource {
            case: "a contact created with a tag",
            arn: tagged_contact.clone(),
            initial: vec![("created".to_string(), "withtags".to_string())],
        },
        TaggableResource {
            case: "a rotation",
            arn: rotation.clone(),
            initial: Vec::new(),
        },
    ];

    for TaggableResource {
        case: resource_case,
        arn,
        initial,
    } in &resources
    {
        let mut expected = initial.clone();
        assert_eq!(
            reported_tags(&emulator, arn, resource_case).await,
            expected,
            "case {resource_case:?}: expected the tags the create carried"
        );

        for write in TAG_WRITES {
            let case = format!("{resource_case}, tagged by {}", write.case);
            let tags = parse(write.tags);
            // Applying the same write twice must leave the same tags: the result is
            // determined by the request, not by how many times it was sent.
            for attempt in 1..=2 {
                let (status, body) = emulator
                    .call(
                        "TagResource",
                        json!({"ResourceARN": arn, "Tags": tags.clone()}),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?} attempt {attempt}: expected HTTP 200, got {status} {body:?}"
                );
                assert!(
                    body == json!({}) || body.is_null(),
                    "case {case:?} attempt {attempt}: TagResource models an empty output, \
                     got {body:?}"
                );
            }
            merge(&mut expected, &tags);
            assert_eq!(
                reported_tags(&emulator, arn, &case).await,
                expected,
                "case {case:?}: expected the merge by key"
            );
        }

        // Untagging is by key and is idempotent: a key the resource does not carry removes
        // nothing.
        let removals: &[(&str, &str)] = &[
            ("no keys at all", "[]"),
            ("one key it carries", r#"["team"]"#),
            ("the same key again", r#"["team"]"#),
            ("a key it never carried", r#"["absent-key"]"#),
            ("two keys at once", r#"["tier", "env"]"#),
        ];
        for (removal_case, keys) in removals {
            let case = format!("{resource_case}, untagged by {removal_case}");
            let keys: Vec<String> = parse(keys)
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|key| key.as_str().unwrap_or_default().to_string())
                .collect();
            let (status, body) = emulator
                .call(
                    "UntagResource",
                    json!({"ResourceARN": arn, "TagKeys": keys.clone()}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body:?}"
            );
            expected.retain(|(key, _)| !keys.contains(key));
            assert_eq!(
                reported_tags(&emulator, arn, &case).await,
                expected,
                "case {case:?}: expected only the named keys to be removed"
            );
        }
    }
}

#[tokio::test]
async fn a_tag_write_outside_the_rule_is_refused() {
    let emulator = Emulator::new();
    let team = rotation_team(&emulator).await;
    let arn = &team[0];
    let absent = Targets::absent();

    // A tag with no `Key` names nothing to read back or remove.
    for (case, tags) in [
        ("a tag carrying only a Value", json!([{"Value": "sre"}])),
        (
            "one keyless tag beside a well-formed one",
            json!([{"Key": "team", "Value": "sre"}, {"Value": "orphan"}]),
        ),
    ] {
        let (status, body) = emulator
            .call("TagResource", json!({"ResourceARN": arn, "Tags": tags}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        assert!(
            reported_tags(&emulator, arn, case).await.is_empty(),
            "case {case:?}: a refused write must write nothing — including the well-formed \
             tags in the same batch"
        );
    }

    // The per-resource ceiling is the modelled `TagsList` length (0..50): 50 tags are
    // accepted, the 51st distinct key is not, and replacing an existing key at the ceiling
    // still is.
    let fifty: Vec<Value> = (0..50)
        .map(|index| json!({"Key": format!("key-{index}"), "Value": "v"}))
        .collect();
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": arn, "Tags": fifty.clone()}),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected 50 tags — the modelled TagsList maximum — to be accepted, \
         got {status} {body:?}"
    );
    assert_eq!(
        reported_tags(&emulator, arn, "fifty tags").await.len(),
        50,
        "expected the resource to carry 50 tags"
    );

    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": arn, "Tags": [{"Key": "key-50", "Value": "v"}]}),
        )
        .await;
    assert_eq!(
        status, 402,
        "expected the 51st distinct key to exceed the quota, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ServiceQuotaExceededException"),
        "expected a ServiceQuotaExceededException, got {body:?}"
    );
    assert_eq!(
        reported_tags(&emulator, arn, "after the refused 51st tag")
            .await
            .len(),
        50,
        "a refused write must leave the resource's tags exactly as they were"
    );

    let (status, body) = emulator
        .call(
            "TagResource",
            json!({"ResourceARN": arn, "Tags": [{"Key": "key-0", "Value": "replaced"}]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "replacing a key at the ceiling adds no tag, so it is accepted, got {status} {body:?}"
    );

    // A batch naming the same NEW key twice adds one tag, not two: at 49 stored tags it
    // lands exactly on the ceiling rather than past it.
    let (status, body) = emulator
        .call("UntagResource", json!({"ResourceARN": arn, "TagKeys": ["key-0"]}))
        .await;
    assert_eq!(
        status, 200,
        "expected UntagResource to answer 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call(
            "TagResource",
            json!({
                "ResourceARN": arn,
                "Tags": [
                    {"Key": "key-50", "Value": "once"},
                    {"Key": "key-50", "Value": "twice"},
                ],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a repeated key in one batch adds one tag, so 49 + it is on the ceiling, \
         got {status} {body:?}"
    );
    let tags = reported_tags(&emulator, arn, "after the repeated-key batch").await;
    assert_eq!(
        tags.len(),
        50,
        "expected the repeated key to have been added once, got {tags:?}"
    );
    assert!(
        tags.contains(&("key-50".to_string(), "twice".to_string())),
        "expected the last value in the batch to win, got {tags:?}"
    );

    // Every ARN that names no taggable resource answers the same way, on all three
    // operations.
    let untaggable: &[(&str, String)] = &[
        ("an ARN naming a contact nothing created", absent.contact),
        ("an ARN naming a rotation nothing created", absent.rotation),
        ("a contact-channel ARN", absent.channel),
        ("an engagement ARN", absent.engagement),
    ];
    for (case, resource) in untaggable {
        for (operation, request) in [
            (
                "TagResource",
                json!({"ResourceARN": resource, "Tags": [{"Key": "team", "Value": "sre"}]}),
            ),
            (
                "UntagResource",
                json!({"ResourceARN": resource, "TagKeys": ["team"]}),
            ),
            ("ListTagsForResource", json!({"ResourceARN": resource})),
        ] {
            let case = format!("{operation} on {case}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
            );
        }
    }
}
