// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRemainingFreeTrialDays`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. A feature's free trial runs 30 days from the instant it began for an account, so
//!    the days reported for it are the whole days of that window still to come — the
//!    whole 30 while the trial has not begun, and none once the window has run out.
//!    Asserted over a table of elapsed spans, and over every reported feature.
//! 2. The trial is per ACCOUNT and per FEATURE: a feature enabled later starts its own
//!    window then, a member account's windows are its own, and switching a feature off
//!    and on again does not buy a second trial.
//! 3. The report covers the modelled catalogue: every `FreeTrialFeatureResult` member
//!    once, in canonical order, and the deprecated `dataSources` view agrees with the
//!    `features` entry for the feature that governs each of its nodes.
//! 4. An account that is neither the detector's own nor an associated member of it is
//!    not an error: the call answers HTTP 200 and names the account in
//!    `unprocessedAccounts` with the reason it was not processed — once per DISTINCT id.
//! 5. A request the service cannot understand — a value that cannot be an account id, a
//!    value that cannot be a detector id, or a well-formed `DetectorId` this
//!    account/Region does not own — is refused with HTTP 400 under the modelled
//!    `BadRequestException` shape.
//! 6. Reading the trial is a REPORT: it consumes no day, moves no switch and two reads
//!    of one account agree.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    now_millis, service_state, DAY_MILLIS,
};

use crate::detector_lifecycle::{
    call, enabled_detector, error_code, error_shape, new_router, router_over, Router,
    ABSENT_DETECTOR_ID,
};
use crate::member_association::{
    administrator, create_members, disassociate_members, invite_members, unprocessed_ids,
    unprocessed_result, DEFAULT_REGION, MALFORMED_ACCOUNT_IDS, MEMBERS, NON_MEMBER_ACCOUNT_IDS,
    NOT_AN_ASSOCIATED_MEMBER,
};

/// The error class a handler rejection carries in the body's `__type`.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, as `x-amzn-errortype`
/// carries it.
const ENVELOPE: &str = "BadRequestException";

/// How long GuardDuty's free trial of one feature runs, in days: an account enabling
/// GuardDuty, or one of its protection plans, gets a 30-day free trial of it. Stated
/// here independently of the service's own constant, so a change to one is caught by
/// the other.
const FREE_TRIAL_DAYS: i64 = 30;

/// Every feature a report names, in the order it reports them: the
/// `FreeTrialFeatureResult` members in the order the model declares them
/// (`smithy_query(shape="FreeTrialFeatureResult")`), which is what
/// `FreeTrialFeatureConfigurationResult.Name` targets.
const REPORTED_FEATURES: [&str; 12] = [
    "FLOW_LOGS",
    "CLOUD_TRAIL",
    "DNS_LOGS",
    "S3_DATA_EVENTS",
    "EKS_AUDIT_LOGS",
    "EBS_MALWARE_PROTECTION",
    "RDS_LOGIN_EVENTS",
    "LAMBDA_NETWORK_LOGS",
    "EKS_RUNTIME_MONITORING",
    "EC2_RUNTIME_MONITORING",
    "FARGATE_RUNTIME_MONITORING",
    "AI_PROTECTION",
];

/// The same catalogue as a SET, spelled in alphabetical order: what a report must
/// cover, stated independently of the order the service reports it in, so a report
/// that left a member out or named one twice fails even if the order above were to
/// drift (`smithy_query(shape="FreeTrialFeatureResult")`).
const MODELLED_FEATURES: [&str; 12] = [
    "AI_PROTECTION",
    "CLOUD_TRAIL",
    "DNS_LOGS",
    "EBS_MALWARE_PROTECTION",
    "EC2_RUNTIME_MONITORING",
    "EKS_AUDIT_LOGS",
    "EKS_RUNTIME_MONITORING",
    "FARGATE_RUNTIME_MONITORING",
    "FLOW_LOGS",
    "LAMBDA_NETWORK_LOGS",
    "RDS_LOGIN_EVENTS",
    "S3_DATA_EVENTS",
];

/// The reported features whose trial has NOT begun for an account that enabled
/// GuardDuty without naming any features.
///
/// `RUNTIME_MONITORING` is the one opt-in feature such a create leaves DISABLED, and
/// its switch is the one that covers the EC2 and Fargate runtime reports — so those two
/// reports have their whole trial ahead of them while every other one's has started.
const NOT_STARTED_AT_CREATE: [&str; 2] = ["EC2_RUNTIME_MONITORING", "FARGATE_RUNTIME_MONITORING"];

/// The `dataSources` nodes a report fills, paired with the reported feature whose
/// trial governs each. The node paths are the `jsonName`s of `DataSourcesFreeTrial`
/// and its nested shapes (`smithy_query(shape="DataSourcesFreeTrial")`).
const GOVERNED_NODES: [(&[&str], &str); 6] = [
    (&["cloudTrail"], "CLOUD_TRAIL"),
    (&["dnsLogs"], "DNS_LOGS"),
    (&["flowLogs"], "FLOW_LOGS"),
    (&["s3Logs"], "S3_DATA_EVENTS"),
    (&["kubernetes", "auditLogs"], "EKS_AUDIT_LOGS"),
    (
        &["malwareProtection", "scanEc2InstanceWithFindings"],
        "EBS_MALWARE_PROTECTION",
    ),
];

/// `GetRemainingFreeTrialDays` over the wire
/// (`POST /detector/{DetectorId}/freeTrial/daysRemaining`).
async fn remaining_free_trial_days(
    router: &Router,
    detector_id: &str,
    account_ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/freeTrial/daysRemaining"),
        Some(json!({"accountIds": account_ids})),
    )
    .await
}

/// The same call, asserting it succeeded.
async fn free_trial_of(router: &Router, detector_id: &str, account_ids: &[&str]) -> Value {
    let (status, body) = remaining_free_trial_days(router, detector_id, account_ids).await;
    assert_eq!(
        status, 200,
        "get-remaining-free-trial-days {account_ids:?}: expected 200, got {status} {body:?}"
    );
    body
}

/// The `accounts` entry for `account_id`, if the response carries one.
fn account_of(body: &Value, account_id: &str) -> Option<Value> {
    body["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|account| account["accountId"].as_str() == Some(account_id))
        .cloned()
}

/// The `freeTrialDaysRemaining` the account's `features` list reports for `feature`.
fn days_for(account: &Value, feature: &str) -> i64 {
    account["features"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["name"].as_str() == Some(feature))
        .and_then(|entry| entry["freeTrialDaysRemaining"].as_i64())
        .unwrap_or_else(|| panic!("expected {feature} to report its remaining days: {account:?}"))
}

/// The feature names the account's `features` list reports, in order.
fn reported_names(account: &Value) -> Vec<String> {
    account["features"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| entry["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `freeTrialDaysRemaining` at one `dataSources` node path.
fn node_days(account: &Value, path: &[&str]) -> i64 {
    let mut node = &account["dataSources"];
    for segment in path {
        node = &node[*segment];
    }
    node["freeTrialDaysRemaining"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected {path:?} to report its remaining days: {account:?}"))
}

/// Put every free trial that has BEGUN for one account `days` days into its window,
/// through this service's own state module — the store the handlers read.
///
/// The seam a span of days needs: a trial runs for 30 days, which no test can wait
/// out, and the instant it began is the single stored fact a report counts from. A
/// trial that has not begun is left alone, so this moves the window without starting
/// one.
fn age_trials(
    engine: &engine::InMemoryEngine,
    region: &str,
    detector_id: &str,
    account_id: Option<&str>,
    days: i64,
) {
    let state = service_state(engine).expect("this service's state");
    let mut state = state.lock();
    let detector = state
        .detector_in_mut(region, detector_id)
        .expect("the detector the create minted");
    let features = match account_id {
        None => &mut detector.features,
        Some(account_id) => &mut detector
            .members
            .get_mut(account_id)
            .expect("the member the create recorded")
            .features,
    };
    // Dated off the wall clock the report reads, so the span the report measures is
    // exactly `days` whole days whatever instant the create stamped.
    let began = now_millis() - days * DAY_MILLIS;
    for feature in features.iter_mut() {
        if feature.free_trial_started_at_millis.is_some() {
            feature.free_trial_started_at_millis = Some(began);
        }
    }
}

/// `UpdateDetector` with a feature list, asserting it succeeded.
async fn configure(router: &Router, detector_id: &str, features: &[(&str, &str)]) {
    let entries: Vec<Value> = features
        .iter()
        .map(|(name, status)| json!({"name": name, "status": status}))
        .collect();
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}"),
        Some(json!({"features": entries})),
    )
    .await;
    assert_eq!(
        status, 200,
        "update-detector {features:?}: expected 200, got {status} {body:?}"
    );
}

/// An enabled detector over an engine the test keeps a handle on, so it can age the
/// trials the create started.
async fn detector_over_engine() -> (Router, Arc<engine::InMemoryEngine>, String) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector(&router).await;
    (router, engine, detector_id)
}

/// The report covers the modelled catalogue, and a brand-new account's whole trial is
/// still ahead of it: every reported feature answers the full window, and the
/// deprecated `dataSources` view agrees with the `features` entry for the feature that
/// governs each node.
#[tokio::test]
async fn a_new_accounts_whole_free_trial_is_still_ahead_of_it() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let administrator = administrator();

    let body = free_trial_of(&router, &detector_id, &[&administrator]).await;
    assert!(
        unprocessed_ids(&body).is_empty(),
        "the detector's own account must be processed, got {body:?}"
    );
    let account = account_of(&body, &administrator)
        .unwrap_or_else(|| panic!("expected the detector's own account, got {body:?}"));

    assert_eq!(
        reported_names(&account),
        REPORTED_FEATURES.to_vec(),
        "every FreeTrialFeatureResult member must be reported once, in canonical order: got \
         {account:?}"
    );
    // The same claim about the SET, checked against the catalogue spelled in another
    // order: every member covered, none twice.
    let mut covered = reported_names(&account);
    covered.sort();
    let distinct = {
        let mut names = covered.clone();
        names.dedup();
        names
    };
    assert_eq!(
        distinct, covered,
        "no feature may be reported twice: got {covered:?}"
    );
    assert_eq!(
        covered,
        MODELLED_FEATURES.to_vec(),
        "the report must cover the whole FreeTrialFeatureResult catalogue: got {covered:?}"
    );
    for feature in REPORTED_FEATURES {
        assert_eq!(
            days_for(&account, feature),
            FREE_TRIAL_DAYS,
            "case {feature}: a trial begun now (or not begun) must report the whole window, got \
             {account:?}"
        );
    }
    for (path, feature) in GOVERNED_NODES {
        assert_eq!(
            node_days(&account, path),
            days_for(&account, feature),
            "case {path:?}: the deprecated view and the {feature} entry are one trial, got \
             {account:?}"
        );
    }
}

/// The days reported are the whole days left of the 30-day window, counted from the
/// instant the trial began.
///
/// The rows differ along the span that has elapsed: none of it, one day, the middle,
/// the last day, exactly the published length, one day past it, and far past it. A
/// feature whose trial has not begun reports the whole window in every row, which is
/// what makes the count a function of the START instant and not of the clock alone.
#[tokio::test]
async fn days_remaining_is_the_whole_days_left_of_the_free_trial_window() {
    // (days elapsed since the trial began, days the report must answer)
    const ELAPSED: [(i64, i64); 7] = [
        (0, 30),
        (1, 29),
        (15, 15),
        (29, 1),
        (30, 0),
        (31, 0),
        (400, 0),
    ];

    for (elapsed, expected) in ELAPSED {
        let (router, engine, detector_id) = detector_over_engine().await;
        let administrator = administrator();
        age_trials(&engine, DEFAULT_REGION, &detector_id, None, elapsed);

        let body = free_trial_of(&router, &detector_id, &[&administrator]).await;
        let account = account_of(&body, &administrator)
            .unwrap_or_else(|| panic!("case {elapsed} days: expected an account, got {body:?}"));
        for feature in REPORTED_FEATURES {
            let begun = !NOT_STARTED_AT_CREATE.contains(&feature);
            let want = if begun { expected } else { FREE_TRIAL_DAYS };
            assert_eq!(
                days_for(&account, feature),
                want,
                "case {feature} after {elapsed} days: expected {want} days left, got {account:?}"
            );
        }
    }
}

/// The trial is per FEATURE: one enabled later starts its own window then, while the
/// windows already running keep the days they have left.
#[tokio::test]
async fn a_feature_enabled_later_starts_its_own_free_trial() {
    const AGED_DAYS: i64 = 20;
    // Runtime Monitoring is the ONE feature a bare create leaves off ("with an
    // exception to RUNTIME_MONITORING, all the optional features are enabled by
    // default"), so it is the switch whose window a later call starts; it governs the
    // EC2 and Fargate runtime reports, and every other report's window is already
    // running.
    const SWITCH: &str = "RUNTIME_MONITORING";
    const REPORTS: [&str; 2] = ["EC2_RUNTIME_MONITORING", "FARGATE_RUNTIME_MONITORING"];

    let (router, engine, detector_id) = detector_over_engine().await;
    let administrator = administrator();
    age_trials(&engine, DEFAULT_REGION, &detector_id, None, AGED_DAYS);
    // Runtime Monitoring already covers Amazon EKS, so the switch a bare create left
    // ENABLED for EKS Runtime Monitoring goes off in the same call that enables it: the
    // two cannot both end up enabled.
    configure(
        &router,
        &detector_id,
        &[(SWITCH, "ENABLED"), ("EKS_RUNTIME_MONITORING", "DISABLED")],
    )
    .await;

    let body = free_trial_of(&router, &detector_id, &[&administrator]).await;
    let account = account_of(&body, &administrator)
        .unwrap_or_else(|| panic!("case {SWITCH}: expected an account, got {body:?}"));
    for report in REPORTS {
        assert_eq!(
            days_for(&account, report),
            FREE_TRIAL_DAYS,
            "case {report}: a trial that began just now must report the whole window, got \
             {account:?}"
        );
    }
    for feature in REPORTED_FEATURES {
        if REPORTS.contains(&feature) || NOT_STARTED_AT_CREATE.contains(&feature) {
            continue;
        }
        assert_eq!(
            days_for(&account, feature),
            FREE_TRIAL_DAYS - AGED_DAYS,
            "case {feature}: enabling {SWITCH} must not move a window already running, got \
             {account:?}"
        );
    }
}

/// The trial is a one-off per account per feature: switching a feature off and on
/// again does not buy a second one, however many times the switch moves.
#[tokio::test]
async fn switching_a_feature_off_and_on_again_does_not_restart_its_free_trial() {
    const AGED_DAYS: i64 = 10;
    // Rows differ along the switch cycled and along how often it is cycled.
    const CYCLES: [(&str, usize); 3] = [
        ("S3_DATA_EVENTS", 1),
        ("EKS_AUDIT_LOGS", 2),
        ("EBS_MALWARE_PROTECTION", 3),
    ];

    for (switch, cycles) in CYCLES {
        let (router, engine, detector_id) = detector_over_engine().await;
        let administrator = administrator();
        age_trials(&engine, DEFAULT_REGION, &detector_id, None, AGED_DAYS);
        for _ in 0..cycles {
            configure(&router, &detector_id, &[(switch, "DISABLED")]).await;
            configure(&router, &detector_id, &[(switch, "ENABLED")]).await;
        }

        let body = free_trial_of(&router, &detector_id, &[&administrator]).await;
        let account = account_of(&body, &administrator)
            .unwrap_or_else(|| panic!("case {switch}: expected an account, got {body:?}"));
        assert_eq!(
            days_for(&account, switch),
            FREE_TRIAL_DAYS - AGED_DAYS,
            "case {switch} cycled {cycles} times: the window must keep the instant it began at, \
             got {account:?}"
        );
    }
}

/// The trial is per ACCOUNT: a member account's windows are its own, so ageing one
/// account's windows moves nothing another account reports.
#[tokio::test]
async fn each_accounts_free_trial_is_counted_from_its_own_start() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(Arc::clone(&engine));
    let detector_id = enabled_detector(&router).await;
    let administrator = administrator();
    let (status, body) = create_members(&router, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    // An invitation is what associates a recorded account with its administrator,
    // and only an associated member's trial is the administrator's to read.
    let member_ids: Vec<&str> = MEMBERS.iter().map(|(id, _)| *id).collect();
    let (status, body) = invite_members(&router, &detector_id, &member_ids).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );

    // Each account's windows are aged by a different span, the administrator's
    // included.
    let aged: Vec<(String, i64)> = std::iter::once((administrator.clone(), 25_i64))
        .chain(
            MEMBERS
                .iter()
                .enumerate()
                .map(|(index, (account_id, _))| ((*account_id).to_string(), index as i64 * 7)),
        )
        .collect();
    for (account_id, days) in &aged {
        let owner = (*account_id != administrator).then_some(account_id.as_str());
        age_trials(&engine, DEFAULT_REGION, &detector_id, owner, *days);
    }

    let ids: Vec<&str> = aged.iter().map(|(id, _)| id.as_str()).collect();
    let body = free_trial_of(&router, &detector_id, &ids).await;
    assert!(
        unprocessed_ids(&body).is_empty(),
        "the detector's own account and its associated members must all be processed, got {body:?}"
    );
    for (account_id, days) in &aged {
        let account = account_of(&body, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected an account, got {body:?}"));
        assert_eq!(
            days_for(&account, "S3_DATA_EVENTS"),
            FREE_TRIAL_DAYS - days,
            "case {account_id} aged {days} days: each account's window is its own, got {account:?}"
        );
    }
}

/// An account that is neither the detector's own nor an associated member of it is
/// reported, not refused: HTTP 200, the account named in `unprocessedAccounts` with
/// the reason, and no entry in `accounts`. A repeated id is reported once.
#[tokio::test]
async fn an_account_the_detector_does_not_administer_is_reported_unprocessed() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let (status, body) = create_members(&router, &detector_id, &MEMBERS).await;
    assert_eq!(
        status, 200,
        "create-members: expected 200, got {status} {body:?}"
    );
    // A member a disassociation moved is no longer associated with this
    // administrator, so it is not processed either — and nor is one the
    // administrator has only recorded, which the invitation below associates.
    let (disassociated, _) = MEMBERS[0];
    let (recorded_only, _) = MEMBERS[1];
    let (status, body) = invite_members(&router, &detector_id, &[disassociated]).await;
    assert_eq!(
        status, 200,
        "invite-members: expected 200, got {status} {body:?}"
    );
    let (status, body) = disassociate_members(&router, &detector_id, &[disassociated]).await;
    assert_eq!(
        status, 200,
        "disassociate-members: expected 200, got {status} {body:?}"
    );
    assert!(
        unprocessed_ids(&body).is_empty(),
        "disassociate-members: expected an invited member processed, got {body:?}"
    );

    // The administrator's own account is the detector's own and IS processed, so the
    // shared non-member table's row for it is not a case here.
    let administrator = administrator();
    let mut cases: Vec<(&str, &str)> = NON_MEMBER_ACCOUNT_IDS
        .into_iter()
        .filter(|(account_id, _)| *account_id != administrator)
        .collect();
    cases.push((disassociated, "a member a disassociation moved"));
    cases.push((recorded_only, "a member recorded and not invited yet"));
    for (account_id, why) in cases {
        // The id is listed twice: `AccountIds` names a SET, so it is reported once.
        let body = free_trial_of(&router, &detector_id, &[account_id, account_id]).await;
        assert_eq!(
            unprocessed_ids(&body),
            vec![account_id.to_string()],
            "case {account_id} ({why}): expected one unprocessed entry, got {body:?}"
        );
        assert_eq!(
            unprocessed_result(&body, account_id).as_deref(),
            Some(NOT_AN_ASSOCIATED_MEMBER),
            "case {account_id} ({why}): expected the reason it was not processed, got {body:?}"
        );
        assert!(
            account_of(&body, account_id).is_none(),
            "case {account_id} ({why}): an unprocessed account reports no trial, got {body:?}"
        );
    }
}

/// A request the service cannot understand is refused with HTTP 400 under the modelled
/// `BadRequestException` shape, and reports nothing: a value that cannot be an account
/// id, a value that cannot be a detector id, and a well-formed `DetectorId` this
/// account/Region does not own.
#[tokio::test]
async fn a_request_the_service_cannot_understand_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let administrator = administrator();

    for (account_id, why) in MALFORMED_ACCOUNT_IDS {
        let (status, body) = remaining_free_trial_days(&router, &detector_id, &[account_id]).await;
        assert_eq!(
            status, 400,
            "case {account_id:?} ({why}): expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            CLASS,
            "case {account_id:?} ({why}): expected the {CLASS} class, got {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            ENVELOPE,
            "case {account_id:?} ({why}): expected the {ENVELOPE} shape, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                "The request failed because parameter accountIds has accountId values that are formatted incorrectly."
            ),
            "case {account_id:?} ({why}): expected the format refusal naming the list, got {body:?}"
        );
    }

    // (the detector id sent, why it fails, the message it is refused with)
    let detector_cases: [(&str, &str, &str); 4] = [
        (
            ABSENT_DETECTOR_ID,
            "well formed, never minted",
            "The request is rejected because the input detectorId is not owned by the current account.",
        ),
        (
            "not-a-detector",
            "not a detector id at all",
            "The request is rejected because the parameter detectorId has an invalid value.",
        ),
        (
            "DEADBEEFDEADBEEFDEADBEEFDEADBEEF",
            "well formed upper-cased, so an OWNERSHIP failure",
            "The request is rejected because the input detectorId is not owned by the current account.",
        ),
        (
            "0123456789abcdef0123456789abcdeg",
            "32 characters, one of them not hexadecimal",
            "The request is rejected because the parameter detectorId has an invalid value.",
        ),
    ];
    for (sent, why, message) in detector_cases {
        let (status, body) = remaining_free_trial_days(&router, sent, &[&administrator]).await;
        assert_eq!(
            status, 400,
            "case {sent} ({why}): expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            CLASS,
            "case {sent} ({why}): expected the {CLASS} class, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(message),
            "case {sent} ({why}): expected the refusal naming detectorId, got {body:?}"
        );
    }
}

/// Reading the trial is a REPORT: it consumes no day and moves no switch, so repeated
/// reads agree with each other and the detector reads back unchanged.
#[tokio::test]
async fn reading_the_remaining_days_changes_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let administrator = administrator();

    let (before_status, before) = call(&router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        before_status, 200,
        "get-detector: expected 200, got {before_status} {before:?}"
    );

    let first = free_trial_of(&router, &detector_id, &[&administrator]).await;
    for round in 1..=3 {
        let again = free_trial_of(&router, &detector_id, &[&administrator]).await;
        assert_eq!(
            again, first,
            "read {round}: a report must not change what a later reader sees"
        );
    }

    let (after_status, after) = call(&router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        after_status, 200,
        "get-detector: expected 200, got {after_status} {after:?}"
    );
    assert_eq!(
        after, before,
        "reporting the free trial must leave the detector's configuration exactly as it was"
    );
}
