// SPDX-License-Identifier: CC0-1.0

//! `UpdateContact`, end to end over the wire.
//!
//! Three rules are asserted, each over a TABLE rather than one call, driven through the
//! production `MultiServiceRouter` exactly as `contact_lifecycle.rs` drives it (whose
//! `Emulator` harness this suite reuses):
//!
//! 1. The ACCOUNT-level Incident Manager precondition is evaluated BEFORE `ContactId` is
//!    resolved: an account with no ACTIVE replication set is refused with HTTP 400
//!    `ValidationException`, whether or not the contact it named exists, and nothing is
//!    written.
//! 2. For an onboarded account, a `ContactId` the store does not hold answers HTTP 404
//!    `ResourceNotFoundException`.
//! 3. The update is a PATCH: every member the request names replaces the stored one, every
//!    member it omits is left exactly as it was, and `Alias`/`Type` — which the input
//!    cannot carry — never change. Each row's expectation is COMPUTED as "what was created,
//!    overlaid with what the request named", so the assertion states that rule rather than
//!    a transcript.
//!
//! Every plan written here NAMES resources this suite created first — a channel of the
//! contact being updated, another contact, a rotation — because a plan's references are
//! resolved against the store before it is stored; `tests/all/contact_plan_references.rs`
//! is the suite about THAT rule, including what a plan naming something absent answers.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{service_state, AccountReplicationSet};

use crate::contact_lifecycle::{error_code, parse, Emulator, ACCOUNT, REGION};

// ---------------------------------------------------------------------------
// The account precondition, as a test fixture
// ---------------------------------------------------------------------------

/// Give the account the replication set `active` says, or take it away with `None`.
///
/// Incident Manager's own `CreateReplicationSet` is deprecated and refuses every call, so
/// the store is the only place an onboarded account can come from — which is exactly what
/// the handler reads.
///
/// `pub(crate)` because every data-plane suite needs the same fixture — the engagement
/// suite and the gated-operation suite both gate on this one account precondition and read
/// it from the same store — so it is written once here.
pub(crate) fn set_replication_set(
    emulator: &Emulator,
    replication_set: Option<AccountReplicationSet>,
) {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    state.lock().replication_set = replication_set;
}

/// An ACTIVE replication set for the account, as Incident Manager would have minted it.
pub(crate) fn active_replication_set() -> AccountReplicationSet {
    AccountReplicationSet {
        arn: format!("arn:aws:ssm-incidents::{ACCOUNT}:replication-set/emulator-set"),
        active: true,
    }
}

/// Open the contact data plane for this emulator's account.
pub(crate) fn onboard(emulator: &Emulator) {
    set_replication_set(emulator, Some(active_replication_set()));
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// A contact to update: the `Type` and `Plan` shape it was created with.
///
/// The rows span the model's whole `ContactType` value set (`CONTACT_TYPE_VALUES` in
/// `src/ops/types/shared_types.rs`) and both shapes the modelled `Plan` admits — a `Stages`
/// list and a `RotationIds` list — plus one contact created with no `DisplayName` at all,
/// so "omitted member left as it was" is also asserted over an absent starting value.
///
/// A plan's references are resolved when it is written, so a row's `plan` carries the
/// placeholders [`CONTACT_PLACEHOLDER`] and [`ROTATION_PLACEHOLDER`], which
/// [`PlanReferences::fill`] replaces with the resources the fixture created.
struct BaseContact {
    case: &'static str,
    alias: &'static str,
    display_name: Option<&'static str>,
    contact_type: &'static str,
    plan: &'static str,
}

const BASE_CONTACTS: &[BaseContact] = &[
    BaseContact {
        case: "a personal contact with one stage",
        alias: "oncall-primary",
        display_name: Some("On Call Primary"),
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": []}]}"#,
    },
    BaseContact {
        case: "a personal contact created with no DisplayName",
        alias: "pager_backup_2",
        display_name: None,
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [{"DurationInMinutes": 30, "Targets": []}]}"#,
    },
    BaseContact {
        case: "an escalation plan with two stages",
        alias: "escalation-tier-1",
        display_name: Some("Escalation Tier 1"),
        contact_type: "ESCALATION",
        plan: r#"{"Stages": [
            {"DurationInMinutes": 10, "Targets": []},
            {"DurationInMinutes": 20, "Targets": [
                {"ContactTargetInfo": {"ContactId": "<contact>", "IsEssential": true}}
            ]}
        ]}"#,
    },
    BaseContact {
        case: "an on-call schedule whose plan carries rotation ids",
        alias: "weekend-rotation",
        display_name: Some("Weekend Rotation"),
        contact_type: "ONCALL_SCHEDULE",
        plan: r#"{"RotationIds": ["<rotation>"]}"#,
    },
];

/// The placeholder a table row puts where a stored CONTACT's ARN belongs.
pub(crate) const CONTACT_PLACEHOLDER: &str = "<contact>";

/// The placeholder a table row puts where a stored ROTATION's ARN belongs.
pub(crate) const ROTATION_PLACEHOLDER: &str = "<rotation>";

/// The placeholder a table row puts where a stored CONTACT CHANNEL's ARN belongs.
pub(crate) const CHANNEL_PLACEHOLDER: &str = "<channel>";

/// The resources a table row's plan NAMES, created before the plan that names them.
///
/// A plan is a graph of references and every one of them is resolved against the store when
/// the plan is written, so a suite that writes plans has to create what they name: a second
/// contact for a `ContactTargetInfo`, a rotation for a `RotationIds` entry, and — once the
/// contact under test exists — one of its own channels for a `ChannelTargetInfo`.
pub(crate) struct PlanReferences {
    pub(crate) contact: String,
    pub(crate) rotation: String,
    pub(crate) channel: String,
}

impl PlanReferences {
    /// Create the contact and the rotation a plan may name. Called BEFORE the contact under
    /// test, so its own create can already name them.
    pub(crate) async fn create(emulator: &Emulator) -> Self {
        let contact = emulator
            .create_contact(
                json!({
                    "Alias": "plan-target",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
                "the contact a plan target names",
            )
            .await;
        let rotation = emulator
            .create_rotation(&contact, "weekend", "the rotation a plan names")
            .await;
        Self {
            contact,
            rotation,
            // Filled in by `with_channel_of` once the contact under test exists: a channel
            // is created UNDER a stored contact.
            channel: String::new(),
        }
    }

    /// Create a contact channel under `contact_arn` and remember it as the channel a plan
    /// target may name.
    pub(crate) async fn with_channel_of(mut self, emulator: &Emulator, contact_arn: &str) -> Self {
        self.channel = emulator
            .create_channel(
                json!({
                    "ContactId": contact_arn,
                    "Name": "email",
                    "Type": "EMAIL",
                    "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
                }),
                "the channel a plan target names",
            )
            .await;
        self
    }

    /// `text` with every placeholder replaced by the ARN of the resource it stands for,
    /// parsed as JSON.
    pub(crate) fn fill(&self, text: &str) -> Value {
        parse(
            &text
                .replace(CONTACT_PLACEHOLDER, &self.contact)
                .replace(ROTATION_PLACEHOLDER, &self.rotation)
                .replace(CHANNEL_PLACEHOLDER, &self.channel),
        )
    }
}

/// The `CreateContact` body a base row is created with.
fn create_request(row: &BaseContact, references: &PlanReferences) -> Value {
    let mut request = json!({
        "Alias": row.alias,
        "Type": row.contact_type,
        "Plan": references.fill(row.plan),
    });
    if let Some(display_name) = row.display_name {
        request["DisplayName"] = json!(display_name);
    }
    request
}

/// An update to apply: the members the `UpdateContact` request names beside `ContactId`.
///
/// The rows differ along what the PATCH rule generalises over — which optional members the
/// request names (neither, one, both), the value a named one carries (a renamed contact, the
/// empty `DisplayName` the modelled `ContactName` length 0..255 admits, a non-ASCII name the
/// modelled pattern `^[\p{L}\p{Z}\p{N}_.\-]*$` admits) and the shape a named `Plan` carries
/// (no stages, one stage at `StageDurationInMins`' published minimum 0, one at its published
/// maximum 30, a channel target, a contact target, a `RotationIds` list instead of stages).
///
/// A row that names a resource carries the placeholder [`PlanReferences::fill`] replaces, so
/// every plan written here resolves. An update naming NEITHER member is not a row: it writes
/// nothing to patch, and `tests/all/contact_plan_references.rs` asserts what it answers.
struct UpdateRow {
    case: &'static str,
    patch: &'static str,
}

const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "DisplayName only",
        patch: r#"{"DisplayName": "Renamed Primary"}"#,
    },
    UpdateRow {
        case: "DisplayName set to the empty string",
        patch: r#"{"DisplayName": ""}"#,
    },
    UpdateRow {
        case: "DisplayName set to a non-ASCII name",
        patch: r#"{"DisplayName": "Équipe Nuit 2"}"#,
    },
    UpdateRow {
        case: "Plan only, replaced by an empty stage list",
        patch: r#"{"Plan": {"Stages": []}}"#,
    },
    UpdateRow {
        case: "Plan only, one stage at the published minimum duration with a channel target",
        patch: r#"{"Plan": {"Stages": [{"DurationInMinutes": 0, "Targets": [
            {"ChannelTargetInfo": {
                "ContactChannelId": "<channel>",
                "RetryIntervalInMinutes": 2
            }}
        ]}]}}"#,
    },
    UpdateRow {
        case: "Plan only, replaced by rotation ids instead of stages",
        patch: r#"{"Plan": {"RotationIds": ["<rotation>"]}}"#,
    },
    UpdateRow {
        case: "both members named, the plan at the published maximum duration",
        patch: r#"{
            "DisplayName": "Escalation Tier 1.a",
            "Plan": {"Stages": [{"DurationInMinutes": 30, "Targets": [
                {"ContactTargetInfo": {"ContactId": "<contact>", "IsEssential": false}}
            ]}]}
        }"#,
    },
];

/// What `GetContact` must report for `base` after `patch` was applied: the created contact
/// overlaid with the members the request named. This IS the rule under test.
fn expected_after(
    base: &BaseContact,
    patch: &Value,
    references: &PlanReferences,
) -> (Option<String>, Value) {
    let display_name = match patch.get("DisplayName").and_then(Value::as_str) {
        Some(named) => Some(named.to_string()),
        None => base.display_name.map(str::to_string),
    };
    let plan = patch
        .get("Plan")
        .cloned()
        .unwrap_or_else(|| references.fill(base.plan));
    (display_name, plan)
}

/// `GetContact` for `arn`, asserting it answered 200.
async fn get_contact(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("GetContact", json!({"ContactId": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContact to answer 200, got {status} {body:?}"
    );
    body
}

/// A well-formed `SsmContactsArn` naming a contact nothing ever created.
fn absent_contact_arn(alias: &str) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/{alias}")
}

// ---------------------------------------------------------------------------
// Rule 3: named members replace, omitted members are untouched
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_leaves_the_rest() {
    for base in BASE_CONTACTS {
        for update in UPDATE_ROWS {
            let case = format!("{} updated by {}", base.case, update.case);
            let emulator = Emulator::new();
            onboard(&emulator);
            // What the plans name, created BEFORE the plans that name them.
            let references = PlanReferences::create(&emulator).await;
            let arn = emulator
                .create_contact(create_request(base, &references), &case)
                .await;
            let references = references.with_channel_of(&emulator, &arn).await;

            let patch = references.fill(update.patch);
            let mut request = patch.clone();
            request["ContactId"] = json!(arn);
            let (status, body) = emulator.call("UpdateContact", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateContact to answer 200, got {status} {body:?}"
            );
            assert!(
                body == json!({}) || body.is_null(),
                "case {case:?}: UpdateContact models an empty output, got {body:?}"
            );

            let (expected_display_name, expected_plan) = expected_after(base, &patch, &references);
            let stored = get_contact(&emulator, &arn, &case).await;
            assert_eq!(
                stored["DisplayName"].as_str().map(str::to_string),
                expected_display_name,
                "case {case:?}: expected DisplayName {expected_display_name:?}, got {:?}",
                stored["DisplayName"]
            );
            assert_eq!(
                stored["Plan"], expected_plan,
                "case {case:?}: expected Plan {expected_plan:?}, got {:?}",
                stored["Plan"]
            );
            // The input carries no Alias and no Type, so neither can move.
            assert_eq!(
                stored["Alias"].as_str(),
                Some(base.alias),
                "case {case:?}: an update must not change the Alias, got {stored:?}"
            );
            assert_eq!(
                stored["Type"].as_str(),
                Some(base.contact_type),
                "case {case:?}: an update must not change the Type, got {stored:?}"
            );
            assert_eq!(
                stored["ContactArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: an update must not change the ContactArn, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn updates_accumulate_and_the_last_write_wins() {
    let base = &BASE_CONTACTS[0];
    let case = "every update row applied in sequence to one contact";
    let emulator = Emulator::new();
    onboard(&emulator);
    let references = PlanReferences::create(&emulator).await;
    let arn = emulator
        .create_contact(create_request(base, &references), case)
        .await;
    let references = references.with_channel_of(&emulator, &arn).await;

    // The contact as created; each patch overlays the state the previous one left.
    let mut display_name = base.display_name.map(str::to_string);
    let mut plan = references.fill(base.plan);

    for update in UPDATE_ROWS {
        let case = format!("{case}, at {}", update.case);
        let patch = references.fill(update.patch);
        if let Some(named) = patch.get("DisplayName").and_then(Value::as_str) {
            display_name = Some(named.to_string());
        }
        if let Some(named) = patch.get("Plan") {
            plan = named.clone();
        }

        // Applying the same update twice must leave the same contact: the write is
        // determined by the request, not by how many times it was sent.
        for attempt in 1..=2 {
            let mut request = patch.clone();
            request["ContactId"] = json!(arn);
            let (status, body) = emulator.call("UpdateContact", request).await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: expected 200, got {status} {body:?}"
            );
            let stored = get_contact(&emulator, &arn, &case).await;
            assert_eq!(
                stored["DisplayName"].as_str().map(str::to_string),
                display_name,
                "case {case:?} attempt {attempt}: expected DisplayName {display_name:?}, \
                 got {:?}",
                stored["DisplayName"]
            );
            assert_eq!(
                stored["Plan"], plan,
                "case {case:?} attempt {attempt}: expected Plan {plan:?}, got {:?}",
                stored["Plan"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the account precondition, evaluated before the contact is resolved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_is_refused_when_the_account_has_no_active_replication_set() {
    // The two ways an account has no ACTIVE replication set: none at all, and one that has
    // not finished replicating.
    let no_active_set: &[(&str, Option<AccountReplicationSet>)] = &[
        ("no replication set at all", None),
        (
            "a replication set that is not active yet",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
        ),
    ];

    for (account_case, replication_set) in no_active_set {
        for update in UPDATE_ROWS {
            let emulator = Emulator::new();
            // The contact is created while the account IS onboarded, so the only thing the
            // update differs in is the precondition.
            onboard(&emulator);
            let base = &BASE_CONTACTS[0];
            let references = PlanReferences::create(&emulator).await;
            let live = emulator
                .create_contact(create_request(base, &references), account_case)
                .await;
            let references = references.with_channel_of(&emulator, &live).await;
            let absent = absent_contact_arn("never-created-contact");

            for (target_case, target) in [("a stored contact", &live), ("an absent contact", &absent)]
            {
                set_replication_set(&emulator, replication_set.clone());
                let case =
                    format!("{account_case}, {target_case}, updated by {}", update.case);

                let mut request = references.fill(update.patch);
                request["ContactId"] = json!(target);
                let (status, body) = emulator.call("UpdateContact", request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account precondition to answer 400, \
                     got {status} {body:?}"
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case:?}: expected a ValidationException — NOT a \
                     ResourceNotFoundException, even for an absent contact — got {body:?}"
                );

                // Nothing was written: with the account onboarded again, the contact still
                // reads back exactly as it was created.
                onboard(&emulator);
                let stored = get_contact(&emulator, &live, &case).await;
                assert_eq!(
                    stored["DisplayName"].as_str(),
                    base.display_name,
                    "case {case:?}: a refused update must write nothing, got {stored:?}"
                );
                assert_eq!(
                    stored["Plan"],
                    references.fill(base.plan),
                    "case {case:?}: a refused update must write nothing, got {stored:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an onboarded account naming no stored contact
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_onboarded_account_updating_an_unstored_contact_answers_404() {
    let emulator = Emulator::new();
    onboard(&emulator);
    // One live contact, so "absent" cannot be confused with "the store is empty", plus one
    // channel ARN and one deleted contact's ARN: every well-formed ARN that names no STORED
    // CONTACT answers the same way.
    let references = PlanReferences::create(&emulator).await;
    let live = emulator
        .create_contact(create_request(&BASE_CONTACTS[0], &references), "a live contact")
        .await;
    let references = references.with_channel_of(&emulator, &live).await;
    let (status, body) = emulator
        .call(
            "CreateContactChannel",
            json!({
                "ContactId": live,
                "Name": "pager",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected CreateContactChannel to answer 200, got {status} {body:?}"
    );
    let channel_arn = body["ContactChannelArn"]
        .as_str()
        .expect("a created channel carries a ContactChannelArn")
        .to_string();

    let doomed = emulator
        .create_contact(
            create_request(&BASE_CONTACTS[1], &references),
            "a contact to delete",
        )
        .await;
    let (status, body) = emulator
        .call("DeleteContact", json!({"ContactId": doomed}))
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteContact to answer 200, got {status} {body:?}"
    );

    let absent: &[(&str, String)] = &[
        (
            "an ARN naming a contact nothing created",
            absent_contact_arn("never-created-contact"),
        ),
        ("a contact-channel ARN", channel_arn),
        ("a deleted contact's ARN", doomed),
    ];

    for (case, target) in absent {
        for update in UPDATE_ROWS {
            let case = format!("{case}, updated by {}", update.case);
            let mut request = references.fill(update.patch);
            request["ContactId"] = json!(target);
            let (status, body) = emulator.call("UpdateContact", request).await;
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

// ---------------------------------------------------------------------------
// The boundary still refuses what the model refuses
// ---------------------------------------------------------------------------

/// A member outside its modelled constraint is refused with HTTP 400 before the handler
/// runs — asserted on each constraint `UpdateContact`'s input declares.
#[tokio::test]
async fn members_outside_their_modelled_constraint_are_refused() {
    let emulator = Emulator::new();
    onboard(&emulator);
    let references = PlanReferences::create(&emulator).await;
    let arn = emulator
        .create_contact(
            create_request(&BASE_CONTACTS[0], &references),
            "a live contact",
        )
        .await;

    let refused: &[(&str, Value)] = &[
        // `ContactId` is required (SsmContactsArn, len 1..2048, pattern ^arn:...$).
        ("ContactId absent", json!({"DisplayName": "x"})),
        (
            "ContactId outside its modelled pattern",
            json!({"ContactId": "not-an-arn"}),
        ),
        (
            "ContactId shorter than its modelled minimum length",
            json!({"ContactId": ""}),
        ),
        // `DisplayName` is ContactName: len 0..255, pattern ^[\p{L}\p{Z}\p{N}_.\-]*$.
        (
            "DisplayName outside its modelled pattern",
            json!({"ContactId": arn, "DisplayName": "no/slashes!"}),
        ),
        (
            "DisplayName longer than its modelled maximum length",
            json!({"ContactId": arn, "DisplayName": "a".repeat(256)}),
        ),
        // `DurationInMinutes` is StageDurationInMins: range 0..30.
        (
            "a stage duration past the published maximum",
            json!({
                "ContactId": arn,
                "Plan": {"Stages": [{"DurationInMinutes": 31, "Targets": []}]},
            }),
        ),
        (
            "a stage duration below the published minimum",
            json!({
                "ContactId": arn,
                "Plan": {"Stages": [{"DurationInMinutes": -1, "Targets": []}]},
            }),
        ),
    ];

    for (case, request) in refused {
        let (status, body) = emulator.call("UpdateContact", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    // And the live contact is untouched by every refused request.
    let stored = get_contact(&emulator, &arn, "after the refused requests").await;
    assert_eq!(
        stored["DisplayName"].as_str(),
        BASE_CONTACTS[0].display_name,
        "a refused request must write nothing, got {stored:?}"
    );
    assert_eq!(
        stored["Plan"],
        references.fill(BASE_CONTACTS[0].plan),
        "a refused request must write nothing, got {stored:?}"
    );
}
