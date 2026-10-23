// SPDX-License-Identifier: CC0-1.0

//! `StartEngagement`, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1 request
//! (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the contact suite
//! builds, so what is asserted is what a signing AWS client would see.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one call:
//!
//! 1. a start of a stored contact or escalation plan PERSISTS a new engagement under a
//!    freshly minted `EngagementArn` named after the engaged contact, stamped with the
//!    instant it started and not stopped, and `DescribeEngagement`/`ListEngagements` report
//!    exactly the message the request carried from then on. The rows span the model's whole
//!    `ContactType` value set, which optional members (`PublicSubject`, `PublicContent`,
//!    `IncidentId`) the request names, the published length bounds of `Sender`, `Subject`
//!    and `Content`, a multi-line `Content`, and two engagements of the SAME contact;
//! 2. the account-level Incident Manager precondition is evaluated BEFORE the `ContactId`
//!    is resolved: with no ACTIVE replication set the call is refused with the modelled
//!    `ValidationException` (HTTP 400) and persists nothing, whether or not the contact it
//!    named exists. The rows span the three states the precondition can be in and both a
//!    live and a never-created contact;
//! 3. an onboarded account naming a contact the store does not hold gets the modelled
//!    `ResourceNotFoundException` (HTTP 404) and nothing is persisted. The rows span the
//!    `SsmContactsArn` forms the model's own pattern admits (all three partitions and a
//!    Region-less ARN);
//! 4. `IdempotencyToken` makes a retry the SAME call: a repeat carrying the token of a start
//!    that already engaged that contact answers that engagement's ARN and starts nothing
//!    new, while a token naming another contact, a fresh token and the empty token (the
//!    member's published minimum length, which names no token at all) each start their own;
//! 5. a member outside its modelled constraint is refused with HTTP 400 before anything is
//!    persisted. The rows sit one unit past each published bound of the members that carry
//!    one, plus an ill-formed `ContactId`.
//!
//! The onboarded path is reached through the contact suite's own account fixture
//! (`contact_updates::onboard`/`set_replication_set`), which writes the replication set the
//! handlers read: that is the only way to reach it, because Incident Manager's
//! `CreateReplicationSet` is deprecated and refuses every call.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::CONTACT_TYPE_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{service_state, AccountReplicationSet};

use crate::contact_lifecycle::{error_code, parse, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A contact to create before the engagements are started.
///
/// `contact_type` spans the model's whole `ContactType` value set (rendered into
/// `src/ops/types/shared_types.rs` as `CONTACT_TYPE_VALUES`), and `plan` spans both shapes
/// the modelled `Plan` admits — a `Stages` list and a `RotationIds` list.
struct ContactFixture {
    alias: &'static str,
    contact_type: &'static str,
    plan: &'static str,
}

const CONTACT_FIXTURES: &[ContactFixture] = &[
    ContactFixture {
        alias: "oncall-primary",
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": []}]}"#,
    },
    ContactFixture {
        alias: "escalation-tier-1",
        contact_type: "ESCALATION",
        plan: r#"{"Stages": [
            {"DurationInMinutes": 10, "Targets": []},
            {"DurationInMinutes": 30, "Targets": [{"ContactTargetInfo": {"IsEssential": true}}]}
        ]}"#,
    },
    ContactFixture {
        alias: "weekday_schedule-1",
        contact_type: "ONCALL_SCHEDULE",
        // `RotationIds` is an `SsmContactsArn` list; the value only has to satisfy that
        // shape's pattern, which `CreateContact` stores as given.
        plan: r#"{"RotationIds": [
            "arn:aws:ssm-contacts:us-east-1:000000000000:rotation/00000000-0000-4000-8000-000000000001"
        ]}"#,
    },
];

/// Create every fixture contact and answer its `ContactArn`, keyed by alias.
async fn create_fixture_contacts(emulator: &Emulator) -> Vec<(&'static str, String)> {
    // The value set the fixtures claim to span is the model's own, so a value added to the
    // model shows up here rather than silently going untested.
    for value in CONTACT_TYPE_VALUES {
        assert!(
            CONTACT_FIXTURES
                .iter()
                .any(|fixture| fixture.contact_type == *value),
            "case {value:?}: the fixtures must cover every modelled ContactType, \
             CONTACT_TYPE_VALUES is {CONTACT_TYPE_VALUES:?}"
        );
    }

    let mut created = Vec::new();
    for fixture in CONTACT_FIXTURES {
        let arn = emulator
            .create_contact(
                json!({
                    "Alias": fixture.alias,
                    "Type": fixture.contact_type,
                    "Plan": parse(fixture.plan),
                }),
                fixture.alias,
            )
            .await;
        created.push((fixture.alias, arn));
    }
    created
}

/// The `ContactArn` of the fixture contact `alias` names.
fn contact_of(contacts: &[(&'static str, String)], alias: &str) -> String {
    contacts
        .iter()
        .find(|(fixture, _)| *fixture == alias)
        .map(|(_, arn)| arn.clone())
        .unwrap_or_else(|| panic!("the fixtures created no contact under the alias {alias:?}"))
}

/// How many engagements the store holds right now, read straight from it.
fn engagement_count(emulator: &Emulator) -> usize {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    state.engagements.len()
}

/// Whole seconds since the epoch, as the wire reports a modelled timestamp.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

/// A replication set that exists but has not finished replicating: present, NOT active.
fn inactive_replication_set() -> AccountReplicationSet {
    AccountReplicationSet {
        active: false,
        ..active_replication_set()
    }
}

/// `Ok(())` when `arn` is the engagement ARN form Incident Manager mints for `alias`:
/// `arn:aws:ssm-contacts:<region>:<account>:engagement/<contact alias>/<uuid>`.
fn check_engagement_arn(arn: &str, alias: &str) -> Result<(), String> {
    let prefix = format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:engagement/{alias}/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return Err(format!("expected an ARN starting {prefix:?}, got {arn:?}"));
    };
    // The opaque id AWS puts in a generated resource name is UUID shaped: 8-4-4-4-12
    // lowercase hex characters.
    let groups: Vec<&str> = id.split('-').collect();
    if groups.iter().map(|group| group.len()).collect::<Vec<_>>() != vec![8, 4, 4, 4, 12] {
        return Err(format!("expected a UUID-shaped id, got {id:?}"));
    }
    if !id
        .chars()
        .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(format!("expected a lowercase hex id, got {id:?}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rule 1: a start persists an engagement that reads back
// ---------------------------------------------------------------------------

/// One engagement to start.
///
/// The rows differ along the members the modelled input admits — `PublicSubject` /
/// `PublicContent` present and absent, an `IncidentId` present and absent, the published
/// length bounds of `Sender` (0..255), `Subject` (1..2048) and `Content` (1..8192), and a
/// multi-line `Content`, which the modelled pattern `^[.\s\S]*$` admits — and along the
/// contact engaged, so nothing here is keyed to one recorded request's shape.
struct StartRow {
    case: &'static str,
    alias: &'static str,
    sender: String,
    subject: String,
    content: String,
    public_subject: Option<String>,
    public_content: Option<String>,
    incident_id: Option<String>,
}

fn start_catalog() -> Vec<StartRow> {
    vec![
        StartRow {
            case: "a VOICE/EMAIL engagement of a personal contact, no public message, no \
                   incident",
            alias: "oncall-primary",
            sender: "cli".to_string(),
            subject: "Database latency".to_string(),
            content: "p99 latency is above the SLO.".to_string(),
            public_subject: None,
            public_content: None,
            incident_id: None,
        },
        StartRow {
            case: "a second engagement of the SAME contact, with an SMS pair and an incident",
            alias: "oncall-primary",
            sender: "incident-manager".to_string(),
            subject: "Disk pressure".to_string(),
            content: "node-7 is at 95% of its volume.".to_string(),
            public_subject: Some("Disk".to_string()),
            public_content: Some("node-7 95%".to_string()),
            incident_id: Some("incident/run-7".to_string()),
        },
        StartRow {
            case: "an engagement of an escalation plan with a multi-line Content",
            alias: "escalation-tier-1",
            sender: "ops\\automation".to_string(),
            subject: "Escalating".to_string(),
            content: "line one\nline two\n\tindented".to_string(),
            public_subject: None,
            public_content: Some("escalating".to_string()),
            incident_id: Some("incident 42".to_string()),
        },
        StartRow {
            case: "an engagement of an on-call schedule at every published minimum length: \
                   Sender 0, Subject 1, Content 1",
            alias: "weekday_schedule-1",
            sender: String::new(),
            subject: "s".to_string(),
            content: "c".to_string(),
            public_subject: Some("p".to_string()),
            public_content: Some("q".to_string()),
            incident_id: Some(String::new()),
        },
        StartRow {
            case: "an engagement at every published maximum length: Sender 255, Subject 2048, \
                   Content 8192, IncidentId 1024",
            alias: "escalation-tier-1",
            sender: "s".repeat(255),
            subject: "u".repeat(2048),
            content: "c".repeat(8192),
            public_subject: Some("p".repeat(2048)),
            public_content: Some("q".repeat(8192)),
            incident_id: Some("i".repeat(1024)),
        },
    ]
}

impl StartRow {
    /// The `StartEngagement` request body for this row.
    fn request(&self, contact_arn: &str) -> Value {
        let mut request = json!({
            "ContactId": contact_arn,
            "Sender": self.sender,
            "Subject": self.subject,
            "Content": self.content,
        });
        if let Some(public_subject) = &self.public_subject {
            request["PublicSubject"] = json!(public_subject);
        }
        if let Some(public_content) = &self.public_content {
            request["PublicContent"] = json!(public_content);
        }
        if let Some(incident_id) = &self.incident_id {
            request["IncidentId"] = json!(incident_id);
        }
        request
    }
}

#[tokio::test]
async fn a_started_engagement_is_persisted_and_reads_back_through_describe() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let contacts = create_fixture_contacts(&emulator).await;

    let mut started: Vec<String> = Vec::new();
    for (index, row) in start_catalog().iter().enumerate() {
        let case = row.case;
        let contact_arn = contact_of(&contacts, row.alias);

        let before = now_secs();
        let (status, body) = emulator
            .call("StartEngagement", row.request(&contact_arn))
            .await;
        let after = now_secs();
        assert_eq!(
            status, 200,
            "case {case:?}: expected StartEngagement to answer 200, got {status} {body:?}"
        );
        let arn = body["EngagementArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
            .to_string();
        if let Err(problem) = check_engagement_arn(&arn, row.alias) {
            panic!("case {case:?}: {problem}");
        }
        assert!(
            !started.contains(&arn),
            "case {case:?}: every start mints its own EngagementArn, but {arn:?} was \
             already minted ({started:?})"
        );
        started.push(arn.clone());
        assert_eq!(
            engagement_count(&emulator),
            index + 1,
            "case {case:?}: expected the store to hold one engagement per start, got {}",
            engagement_count(&emulator)
        );

        // The round trip: the engagement is read back through the implemented reader.
        let (status, described) = emulator
            .call("DescribeEngagement", json!({"EngagementId": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeEngagement to answer 200, got {status} {described:?}"
        );
        assert_eq!(
            described["EngagementArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the minted EngagementArn, got {described:?}"
        );
        assert_eq!(
            described["ContactArn"].as_str(),
            Some(contact_arn.as_str()),
            "case {case:?}: expected the engaged ContactArn, got {described:?}"
        );
        assert_eq!(
            described["Sender"].as_str(),
            Some(row.sender.as_str()),
            "case {case:?}: expected the Sender the start carried, got {:?}",
            described["Sender"]
        );
        assert_eq!(
            described["Subject"].as_str(),
            Some(row.subject.as_str()),
            "case {case:?}: expected the Subject the start carried, got {:?}",
            described["Subject"]
        );
        assert_eq!(
            described["Content"].as_str(),
            Some(row.content.as_str()),
            "case {case:?}: expected the Content the start carried, got {:?}",
            described["Content"]
        );
        assert_eq!(
            described["PublicSubject"].as_str().map(str::to_string),
            row.public_subject,
            "case {case:?}: expected the PublicSubject the start carried, got {:?}",
            described["PublicSubject"]
        );
        assert_eq!(
            described["PublicContent"].as_str().map(str::to_string),
            row.public_content,
            "case {case:?}: expected the PublicContent the start carried, got {:?}",
            described["PublicContent"]
        );
        assert_eq!(
            described["IncidentId"].as_str().map(str::to_string),
            row.incident_id,
            "case {case:?}: expected the IncidentId the start carried, got {:?}",
            described["IncidentId"]
        );
        let start_time = described["StartTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a StartTime, got {:?}", described["StartTime"])
        });
        assert!(
            start_time >= before - 1.0 && start_time <= after + 1.0,
            "case {case:?}: expected a StartTime inside [{before}, {after}], got {start_time}"
        );
        assert!(
            described["StopTime"].as_f64().is_none(),
            "case {case:?}: a started engagement is not stopped, got StopTime {:?}",
            described["StopTime"]
        );
    }

    // Every engagement the suite started is listed, exactly once.
    let (status, body) = emulator.call("ListEngagements", json!({})).await;
    assert_eq!(
        status, 200,
        "expected ListEngagements to answer 200, got {status} {body:?}"
    );
    let mut listed: Vec<String> = body["Engagements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|engagement| engagement["EngagementArn"].as_str().map(str::to_string))
        .collect();
    listed.sort();
    let mut expected = started.clone();
    expected.sort();
    assert_eq!(
        listed, expected,
        "expected ListEngagements to report every started engagement, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the account gate, before the contact is resolved
// ---------------------------------------------------------------------------

/// A state the account precondition can be in, and whether it opens the data plane.
struct AccountCase {
    case: &'static str,
    replication_set: Option<AccountReplicationSet>,
    serves: bool,
}

fn account_cases() -> Vec<AccountCase> {
    vec![
        AccountCase {
            case: "an account that was never onboarded",
            replication_set: None,
            serves: false,
        },
        AccountCase {
            case: "an account whose replication set is not ACTIVE yet",
            replication_set: Some(inactive_replication_set()),
            serves: false,
        },
        AccountCase {
            case: "an onboarded account",
            replication_set: Some(active_replication_set()),
            serves: true,
        },
    ]
}

#[tokio::test]
async fn a_start_is_refused_before_the_contact_is_resolved_until_the_account_is_onboarded() {
    let emulator = Emulator::new();
    // The contacts are created while the account is onboarded: the contact registry is
    // exempt from the gate, and this suite is about `StartEngagement`, not about the create.
    onboard(&emulator);
    let contacts = create_fixture_contacts(&emulator).await;

    let absent_contact = format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created");
    let targets: [(&str, String, bool); 2] = [
        (
            "a contact the store holds",
            contact_of(&contacts, "oncall-primary"),
            true,
        ),
        ("a contact nothing ever created", absent_contact, false),
    ];

    for account in account_cases() {
        for (target_case, contact_arn, exists) in &targets {
            set_replication_set(&emulator, account.replication_set.clone());
            let before = engagement_count(&emulator);
            let case = format!("{} naming {target_case}", account.case);

            let (status, body) = emulator
                .call(
                    "StartEngagement",
                    json!({
                        "ContactId": contact_arn,
                        "Sender": "cli",
                        "Subject": "gate",
                        "Content": "is the account onboarded?",
                    }),
                )
                .await;

            if !account.serves {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account gate to answer 400, got {status} {body:?}"
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case:?}: expected a ValidationException, got {body:?}"
                );
                assert_eq!(
                    engagement_count(&emulator),
                    before,
                    "case {case:?}: a refused start must persist nothing"
                );
                continue;
            }

            // The positive control: the SAME request is served once the account is
            // onboarded, so the 400s above are a statement about the gate.
            let expected = if *exists { 200 } else { 404 };
            assert_eq!(
                status, expected,
                "case {case:?}: expected HTTP {expected} once onboarded, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a contact the store does not hold
// ---------------------------------------------------------------------------

/// The `SsmContactsArn` forms the model's own pattern admits:
/// `^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:[-\w+=\/,.@]*:[0-9]+:([\w+=\/,.@:-]+)*$` — all
/// three partitions, and a Region-less ARN, which the `*` in the Region segment admits.
fn absent_contact_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "the aws partition",
            format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"),
        ),
        (
            "the aws-cn partition",
            format!("arn:aws-cn:ssm-contacts:cn-north-1:{ACCOUNT}:contact/never-created"),
        ),
        (
            "the aws-us-gov partition",
            format!("arn:aws-us-gov:ssm-contacts:us-gov-west-1:{ACCOUNT}:contact/never-created"),
        ),
        (
            "a Region-less ARN",
            format!("arn:aws:ssm-contacts::{ACCOUNT}:contact/never-created"),
        ),
    ]
}

#[tokio::test]
async fn a_start_naming_a_contact_the_store_does_not_hold_answers_404() {
    let emulator = Emulator::new();
    onboard(&emulator);
    create_fixture_contacts(&emulator).await;

    for (case, contact_arn) in absent_contact_arns() {
        let (status, body) = emulator
            .call(
                "StartEngagement",
                json!({
                    "ContactId": contact_arn,
                    "Sender": "cli",
                    "Subject": "missing",
                    "Content": "there is no such contact",
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
        assert_eq!(
            engagement_count(&emulator),
            0,
            "case {case:?}: a refused start must persist nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the idempotency token
// ---------------------------------------------------------------------------

/// A token to repeat a start with.
///
/// The values span the characters `IdempotencyToken`'s pattern
/// `^[\\\/a-zA-Z0-9_+=\-]*$` admits and its published minimum length, 0 — which names no
/// token at all, so a repeat carrying it is a second request rather than a retry.
struct TokenRow {
    case: &'static str,
    token: &'static str,
    /// Whether a repeat carrying this token answers the first start's ARN.
    replays: bool,
}

const TOKEN_ROWS: &[TokenRow] = &[
    TokenRow {
        case: "a plain token",
        token: "retry-token-1",
        replays: true,
    },
    TokenRow {
        case: "a token spanning every punctuation character the pattern admits",
        token: "a_b+c=d-e/f\\g",
        replays: true,
    },
    TokenRow {
        case: "the published minimum length, 0, which names no token",
        token: "",
        replays: false,
    },
];

#[tokio::test]
async fn a_repeated_start_with_the_same_idempotency_token_starts_nothing_new() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let contacts = create_fixture_contacts(&emulator).await;
    let primary = contact_of(&contacts, "oncall-primary");
    let escalation = contact_of(&contacts, "escalation-tier-1");

    for row in TOKEN_ROWS {
        let case = row.case;
        let request = |contact: &str, token: &str| {
            json!({
                "ContactId": contact,
                "Sender": "cli",
                "Subject": "retry",
                "Content": "the same call, twice",
                "IdempotencyToken": token,
            })
        };

        let before = engagement_count(&emulator);
        let (status, body) = emulator
            .call("StartEngagement", request(&primary, row.token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first start to answer 200, got {status} {body:?}"
        );
        let first = body["EngagementArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
            .to_string();

        // The retry: the identical request, token included.
        let (status, body) = emulator
            .call("StartEngagement", request(&primary, row.token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry to answer 200, got {status} {body:?}"
        );
        let repeated = body["EngagementArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
            .to_string();

        if row.replays {
            assert_eq!(
                repeated, first,
                "case {case:?}: expected the retry to answer the first engagement's ARN"
            );
            assert_eq!(
                engagement_count(&emulator),
                before + 1,
                "case {case:?}: expected the retry to start no second engagement"
            );
        } else {
            assert_ne!(
                repeated, first,
                "case {case:?}: with no token every call is its own engagement"
            );
            assert_eq!(
                engagement_count(&emulator),
                before + 2,
                "case {case:?}: expected two engagements, one per call"
            );
        }

        // The same token naming ANOTHER contact is a different request, so it engages that
        // contact rather than replaying.
        let held = engagement_count(&emulator);
        let (status, body) = emulator
            .call("StartEngagement", request(&escalation, row.token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a start of another contact to answer 200, \
             got {status} {body:?}"
        );
        let other = body["EngagementArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
            .to_string();
        assert_ne!(
            other, first,
            "case {case:?}: a token reused for another contact must not replay"
        );
        if let Err(problem) = check_engagement_arn(&other, "escalation-tier-1") {
            panic!("case {case:?}: {problem}");
        }
        assert_eq!(
            engagement_count(&emulator),
            held + 1,
            "case {case:?}: expected the other contact's engagement to be stored"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the modelled constraints
// ---------------------------------------------------------------------------

/// A request with exactly one member outside its modelled constraint.
///
/// Each row sits one unit past a published bound of `StartEngagement`'s input (`Sender`
/// 0..255, `Subject` 1..2048, `Content` 1..8192, `IncidentId` 0..1024, `PublicSubject`
/// 1..2048, `PublicContent` 1..8192, `IdempotencyToken` 0..2048), or breaks `ContactId`'s
/// `SsmContactsArn` pattern.
struct RefusedRow {
    case: &'static str,
    member: &'static str,
    value: Value,
}

fn refused_rows() -> Vec<RefusedRow> {
    vec![
        RefusedRow {
            case: "a Sender one character past its published maximum, 255",
            member: "Sender",
            value: json!("s".repeat(256)),
        },
        RefusedRow {
            case: "a Subject one character past its published maximum, 2048",
            member: "Subject",
            value: json!("u".repeat(2049)),
        },
        RefusedRow {
            case: "a Subject below its published minimum, 1",
            member: "Subject",
            value: json!(""),
        },
        RefusedRow {
            case: "a Content one character past its published maximum, 8192",
            member: "Content",
            value: json!("c".repeat(8193)),
        },
        RefusedRow {
            case: "a Content below its published minimum, 1",
            member: "Content",
            value: json!(""),
        },
        RefusedRow {
            case: "a PublicSubject below its published minimum, 1",
            member: "PublicSubject",
            value: json!(""),
        },
        RefusedRow {
            case: "a PublicContent one character past its published maximum, 8192",
            member: "PublicContent",
            value: json!("q".repeat(8193)),
        },
        RefusedRow {
            case: "an IncidentId one character past its published maximum, 1024",
            member: "IncidentId",
            value: json!("i".repeat(1025)),
        },
        RefusedRow {
            case: "an IdempotencyToken one character past its published maximum, 2048",
            member: "IdempotencyToken",
            value: json!("t".repeat(2049)),
        },
        RefusedRow {
            case: "a ContactId that is not an SsmContactsArn",
            member: "ContactId",
            value: json!("oncall-primary"),
        },
    ]
}

#[tokio::test]
async fn members_outside_their_modelled_constraint_are_refused() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let contacts = create_fixture_contacts(&emulator).await;
    let primary = contact_of(&contacts, "oncall-primary");

    for row in refused_rows() {
        let case = row.case;
        let mut request = json!({
            "ContactId": primary,
            "Sender": "cli",
            "Subject": "constraints",
            "Content": "one member is out of bounds",
        });
        request[row.member] = row.value.clone();

        let (status, body) = emulator.call("StartEngagement", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        assert_eq!(
            engagement_count(&emulator),
            0,
            "case {case:?}: a refused request must persist nothing"
        );
    }
}
