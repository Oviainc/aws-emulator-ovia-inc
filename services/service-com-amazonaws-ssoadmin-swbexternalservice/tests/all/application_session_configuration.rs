// SPDX-License-Identifier: CC0-1.0

//! The application SESSION CONFIGURATION slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over a single
//! example:
//!
//! 1. an application's session configuration is ONE stored status per application:
//!    `PutApplicationSessionConfiguration` REPLACES it and
//!    `GetApplicationSessionConfiguration` reports exactly what the last put stored, for
//!    every sequence of values the model's `UserBackgroundSessionApplicationStatus` enum
//!    admits;
//! 2. an application nothing has configured reports the resting status — background
//!    sessions nothing has enabled are not enabled — whatever else the create sent,
//!    asserted over a table of creates varying `Status`, `PortalOptions` and `Name`;
//! 3. the put is `@idempotent` and the get is `@readonly`: repeating either answers
//!    identically and changes nothing a later caller sees;
//! 4. the status belongs to ONE application: configuring one leaves every other
//!    application's session configuration alone;
//! 5. `UserBackgroundSessionApplicationStatus` is OPTIONAL on the put's input, so a put
//!    that names no status asks for no change and leaves the stored status where it is;
//! 6. a status outside the modelled enum is refused `ValidationException` (HTTP 400) by
//!    the boundary before the handler runs, and stores nothing;
//! 7. an `ApplicationArn` this account holds no application for is refused
//!    `ResourceNotFoundException` by both operations — never a 200 — and the refused put
//!    stores nothing. The status is the one this service answers every refusal of a
//!    request with (`scaffolded::modeled_error_status`), not the shape's `@httpError`;
//! 8. the session configuration and the ASSIGNMENT configuration are two independent
//!    members of one application record, so a put of either leaves the other alone;
//! 9. the configuration lives INSIDE the application, and the application inside its
//!    instance: while a `DeleteInstance` is still pending the application is still there
//!    and reports its session configuration, and once the clock makes the removal due
//!    both operations answer `ResourceNotFoundException` and nothing of the configuration
//!    is left in the store.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, hold_deletion, router, router_over, seeded_instance_arn, with_store,
};

/// The one published provider an application can be CREATED under: `CreateApplication`
/// makes an OAuth 2.0 CUSTOMER MANAGED application, which is what this provider backs.
///
/// SOURCE: `state::BuiltInApplicationProvider::accepts_customer_managed_application` over
/// `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue `ListApplicationProviders`
/// publishes over the wire.
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// A pattern-valid `ApplicationArn` no `CreateApplication` ever minted, used to pin the
/// not-found rule.
///
/// SOURCE: `smithy_query(query="shape",shape="ApplicationArn")` —
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$`.
const UNKNOWN_APPLICATION_ARN: &str =
    "arn:aws:sso::123456789012:application/ssoins-0000000000000000/apl-0000000000000000";

/// Another unminted ARN, in a non-`aws` partition the same pattern admits
/// (`arn:aws(-[a-z]{1,5}){0,3}`), so the refusal is not keyed to one partition.
const UNKNOWN_APPLICATION_ARN_GOV: &str =
    "arn:aws-us-gov:sso::210987654321:application/ins-abcdefghij012345/apl-ZYXWVUTSRQ987654";

/// Every value `UserBackgroundSessionApplicationStatus` admits on the wire.
///
/// SOURCE: `smithy_query(query="shape",shape="UserBackgroundSessionApplicationStatus")` —
/// `["ENABLED", "DISABLED"]`.
const STATUS_VALUES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The status an application nothing has configured reports: the resting state of the
/// feature, since no put has enabled a background session on it.
///
/// SOURCE: `state::USER_BACKGROUND_SESSION_STATUS_DEFAULT`.
const RESTING_STATUS: &str = "DISABLED";

/// The HTTP status this service answers a refusal of the REQUEST with, whatever 4xx the
/// refused shape's `@httpError` declares.
///
/// SOURCE: `scaffolded::modeled_error_status`.
const REFUSAL_STATUS: u16 = 400;

/// One application to create, as a row of the table: the members of `CreateApplication`
/// the configuration rules must hold across.
#[derive(Debug, Clone)]
struct Application {
    label: &'static str,
    name: String,
    status: Option<&'static str>,
    portal_options: Option<Value>,
}

impl Application {
    fn new(label: &'static str, name: &str) -> Self {
        Self {
            label,
            name: name.to_string(),
            status: None,
            portal_options: None,
        }
    }

    fn with_status(mut self, status: &'static str) -> Self {
        self.status = Some(status);
        self
    }

    fn with_portal_options(mut self, portal_options: Value) -> Self {
        self.portal_options = Some(portal_options);
        self
    }

    fn request(&self, instance_arn: &str) -> Value {
        let mut body = json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": self.name,
        });
        if let Some(status) = self.status {
            body["Status"] = json!(status);
        }
        if let Some(portal_options) = &self.portal_options {
            body["PortalOptions"] = portal_options.clone();
        }
        body
    }
}

/// The applications rule 2 is asserted over. They differ along the create members that
/// could plausibly be confused with the session configuration — the application's own
/// `Status` (`ENABLED | DISABLED`, which shares its value set with
/// `UserBackgroundSessionApplicationStatus` and is a DIFFERENT member), whether it was
/// sent at all, and the shape of `PortalOptions` — plus the length and character class of
/// `Name` (`len 1..100`, `^[\S\s]*$`). Every constraint is from
/// `smithy_query(query="operation",operation="CreateApplication")`.
fn applications() -> Vec<Application> {
    vec![
        Application::new("the shortest Name, nothing else sent", "a"),
        Application::new("a DISABLED application", "disabled-app").with_status("DISABLED"),
        Application::new("an ENABLED application with the portal hidden", "hidden-app")
            .with_status("ENABLED")
            .with_portal_options(json!({"Visibility": "DISABLED"})),
        Application::new(
            "a Name at the 100-character limit, spanning spaces and punctuation",
            &format!("App 9 (prod) - {}", "y".repeat(85)),
        )
        .with_portal_options(json!({"SignInOptions": {"Origin": "IDENTITY_CENTER"}})),
        Application::new(
            "an APPLICATION sign-in origin with its required ApplicationUrl",
            "application-origin",
        )
        .with_portal_options(json!({
            "SignInOptions": {
                "Origin": "APPLICATION",
                "ApplicationUrl": "https://app.example.com/sso/start?tenant=one",
            },
            "Visibility": "ENABLED",
        })),
    ]
}

/// `CreateApplication` for one row, returning the ARN it minted.
async fn minted_application(
    router: &MultiServiceRouter,
    instance_arn: &str,
    application: &Application,
) -> String {
    let (status, body) = call(
        router,
        "CreateApplication",
        &application.request(instance_arn),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateApplication must answer 200, got {status} {body}",
        application.label
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication minted no ApplicationArn: {body}"))
        .to_string()
}

/// `PutApplicationSessionConfiguration`. `status` is `None` for a request that sends no
/// `UserBackgroundSessionApplicationStatus` at all, which the model admits (rule 5).
async fn put_session(
    router: &MultiServiceRouter,
    application_arn: &str,
    status: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"ApplicationArn": application_arn});
    if let Some(status) = status {
        body["UserBackgroundSessionApplicationStatus"] = json!(status);
    }
    call(router, "PutApplicationSessionConfiguration", &body).await
}

/// `GetApplicationSessionConfiguration`.
async fn get_session(router: &MultiServiceRouter, application_arn: &str) -> (u16, Value) {
    call(
        router,
        "GetApplicationSessionConfiguration",
        &json!({"ApplicationArn": application_arn}),
    )
    .await
}

/// A get that must succeed, as the `UserBackgroundSessionApplicationStatus` it reported.
async fn session_status_of(
    router: &MultiServiceRouter,
    case: &str,
    application_arn: &str,
) -> String {
    let (status, body) = get_session(router, application_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetApplicationSessionConfiguration must answer 200, got {status} \
         {body}"
    );
    body["UserBackgroundSessionApplicationStatus"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the get reported no UserBackgroundSessionApplicationStatus, \
                 got {body}"
            )
        })
        .to_string()
}

/// A put that must succeed.
async fn put_ok(router: &MultiServiceRouter, case: &str, application_arn: &str, status: Option<&str>) {
    let (code, body) = put_session(router, application_arn, status).await;
    assert_eq!(
        code, 200,
        "case {case:?}: a put of {status:?} must answer 200, got {code} {body}"
    );
    assert!(
        body.get("UserBackgroundSessionApplicationStatus").is_none(),
        "case {case:?}: the put's modelled output carries no members, got {body}"
    );
}

/// Rule 2 (with rule 3): every created application starts on the resting status, and
/// reading it is a report.
#[tokio::test]
async fn an_unconfigured_application_reports_the_resting_status() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for application in applications() {
        let case = application.label;
        let arn = minted_application(&router, &instance_arn, &application).await;

        let (status, body) = get_session(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a get of a fresh application must answer 200, got {status} \
             {body}"
        );
        assert_eq!(
            body["UserBackgroundSessionApplicationStatus"].as_str(),
            Some(RESTING_STATUS),
            "case {case:?}: an unconfigured application must report {RESTING_STATUS}, got \
             {body}"
        );

        // Rule 3: the get reported rather than mutated, so a second one answers
        // identically.
        let (status, again) = get_session(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated get must answer 200, got {status} {again}"
        );
        assert_eq!(
            again, body,
            "case {case:?}: a get must change nothing a later caller sees, got {again}"
        );
    }
}

/// Rule 1 and rule 3: each sequence of puts leaves the application on the status the LAST
/// put stored, and a repeated put of that status changes nothing.
#[tokio::test]
async fn the_configuration_is_whatever_the_last_put_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // The rows differ along what rule 1 generalises over: which value of the enum is put,
    // and how many puts the application received.
    let sequences: Vec<(&str, Vec<&str>, &str)> = vec![
        ("a single put of ENABLED", vec!["ENABLED"], "ENABLED"),
        ("a single put of DISABLED", vec!["DISABLED"], "DISABLED"),
        (
            "ENABLED then DISABLED",
            vec!["ENABLED", "DISABLED"],
            "DISABLED",
        ),
        (
            "DISABLED then ENABLED",
            vec!["DISABLED", "ENABLED"],
            "ENABLED",
        ),
        (
            "ENABLED three times over — @idempotent",
            vec!["ENABLED", "ENABLED", "ENABLED"],
            "ENABLED",
        ),
        (
            "alternating five times, ending on ENABLED",
            vec!["DISABLED", "ENABLED", "DISABLED", "ENABLED", "ENABLED"],
            "ENABLED",
        ),
    ];

    for (index, (case, puts, expected)) in sequences.iter().enumerate() {
        let application =
            Application::new("a configured application", &format!("sessions-{index}"));
        let arn = minted_application(&router, &instance_arn, &application).await;

        for (step, status) in puts.iter().enumerate() {
            put_ok(&router, &format!("{case} put {step}"), &arn, Some(status)).await;
        }

        let actual = session_status_of(&router, case, &arn).await;
        assert_eq!(
            actual, *expected,
            "case {case:?}: the get must report the status the last put stored, expected \
             {expected:?}, got {actual:?}"
        );

        // Rule 3: one more put of the same status is `@idempotent` — the stored status
        // does not move.
        put_ok(&router, case, &arn, puts.last().copied()).await;
        let again = session_status_of(&router, case, &arn).await;
        assert_eq!(
            again, *expected,
            "case {case:?}: a repeated put must leave the same status, expected \
             {expected:?}, got {again:?}"
        );
    }
}

/// Rule 5: the status is OPTIONAL on the put's input, so a put that names none asks for no
/// change — whatever the application was left on, including never having been configured.
#[tokio::test]
async fn a_put_that_names_no_status_changes_nothing() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // The rows differ along the status the application was on when the member-less put
    // arrived: never configured, and each value of the enum.
    let mut rows: Vec<(String, Option<&str>, &str)> =
        vec![("never configured".to_string(), None, RESTING_STATUS)];
    for value in STATUS_VALUES {
        rows.push((format!("configured to {value}"), Some(value), value));
    }

    for (index, (case, configured, expected)) in rows.iter().enumerate() {
        let arn = minted_application(
            &router,
            &instance_arn,
            &Application::new("an application given a member-less put", &format!("bare-{index}")),
        )
        .await;
        if let Some(status) = configured {
            put_ok(&router, case, &arn, Some(status)).await;
        }

        // Twice over: a member-less put is a no-op however often it arrives.
        for round in 0..2 {
            put_ok(&router, &format!("{case} round {round}"), &arn, None).await;
            let actual = session_status_of(&router, case, &arn).await;
            assert_eq!(
                actual, *expected,
                "case {case:?} round {round}: a put with no status must leave the stored \
                 status alone, expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// Rule 4: the status is per application, so configuring one application moves no other
/// application's session configuration.
#[tokio::test]
async fn configuring_one_application_leaves_every_other_alone() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // One ARN per row of the application table. Every other row is put to `ENABLED`; the
    // rest are never configured, so a status stored on the wrong record shows up as
    // another row's value.
    let rows = applications();
    let mut arns: Vec<String> = Vec::new();
    for application in &rows {
        arns.push(minted_application(&router, &instance_arn, application).await);
    }
    let wanted: Vec<&str> = (0..rows.len())
        .map(|index| if index % 2 == 0 { "ENABLED" } else { RESTING_STATUS })
        .collect();

    for (index, arn) in arns.iter().enumerate() {
        if wanted[index] == "ENABLED" {
            put_ok(&router, rows[index].label, arn, Some("ENABLED")).await;
        }
    }

    for (index, arn) in arns.iter().enumerate() {
        let case = rows[index].label;
        let actual = session_status_of(&router, case, arn).await;
        assert_eq!(
            actual, wanted[index],
            "case {case:?}: each application keeps its own session configuration, expected \
             {:?}, got {actual:?}",
            wanted[index]
        );
    }
}

/// Rule 8: the session configuration and the ASSIGNMENT configuration are two independent
/// members of one application record — a put of one never moves the other.
#[tokio::test]
async fn the_session_and_assignment_configurations_are_independent() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let arn = minted_application(
        &router,
        &instance_arn,
        &Application::new("an application with both configurations", "both-configs"),
    )
    .await;

    // Walk the session status over every value of the enum while the assignment flag is
    // held at `false`, the opposite of its own modelled default — so a handler that wrote
    // one member while reading the other shows up on either side.
    let (status, body) = call(
        &router,
        "PutApplicationAssignmentConfiguration",
        &json!({"ApplicationArn": arn, "AssignmentRequired": false}),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutApplicationAssignmentConfiguration must answer 200, got {status} {body}"
    );

    for session_status in STATUS_VALUES {
        put_ok(&router, session_status, &arn, Some(session_status)).await;

        let actual = session_status_of(&router, session_status, &arn).await;
        assert_eq!(
            actual, session_status,
            "case {session_status:?}: the get must report the status just put, got \
             {actual:?}"
        );

        let (status, body) = call(
            &router,
            "GetApplicationAssignmentConfiguration",
            &json!({"ApplicationArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {session_status:?}: GetApplicationAssignmentConfiguration must answer \
             200, got {status} {body}"
        );
        assert_eq!(
            body["AssignmentRequired"].as_bool(),
            Some(false),
            "case {session_status:?}: a session put must leave the assignment \
             configuration alone, got {body}"
        );
    }

    // …and back the other way: moving the assignment flag leaves the session status where
    // the last session put left it.
    let (status, body) = call(
        &router,
        "PutApplicationAssignmentConfiguration",
        &json!({"ApplicationArn": arn, "AssignmentRequired": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "PutApplicationAssignmentConfiguration must answer 200, got {status} {body}"
    );
    let actual = session_status_of(&router, "after an assignment put", &arn).await;
    let expected = STATUS_VALUES[STATUS_VALUES.len() - 1];
    assert_eq!(
        actual, expected,
        "an assignment put must leave the session status alone, expected {expected:?}, got \
         {actual:?}"
    );
}

/// Rule 6: a status outside the modelled enum is refused by the boundary, on the put, and
/// stores nothing.
#[tokio::test]
async fn a_status_outside_the_modelled_enum_is_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let arn = minted_application(
        &router,
        &instance_arn,
        &Application::new("an application configured to ENABLED", "strict-app"),
    )
    .await;
    put_ok(&router, "seed ENABLED", &arn, Some("ENABLED")).await;

    // Values the model's enum does not carry: another case, a near-miss suffix, a value of
    // another enum of this service, the empty string and a value of the right shape but
    // the wrong vocabulary.
    let unknown = [
        ("a lowercase value", "enabled"),
        ("a mixed-case value", "Enabled"),
        ("a near-miss with a suffix", "ENABLED_V2"),
        ("a value of another enum", "CREATE_IN_PROGRESS"),
        ("the empty string", ""),
        ("a plausible third state", "PENDING"),
    ];
    for (case, value) in unknown {
        let (status, body) = put_session(&router, &arn, Some(value)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a put of an unmodelled status must answer 400, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    let actual = session_status_of(&router, "an application configured to ENABLED", &arn).await;
    assert_eq!(
        actual, "ENABLED",
        "a refused put must store nothing, expected \"ENABLED\", got {actual:?}"
    );
}

/// Rule 7: an `ApplicationArn` nothing holds is refused by both operations, and the
/// refused put stores nothing.
#[tokio::test]
async fn an_unminted_application_arn_is_refused_by_both_operations() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    // An application that DOES exist, so the refusals below are about the ARN and not
    // about an empty store. It is configured first, so a refused put that wrote onto the
    // wrong record would show up as a changed status.
    let live = minted_application(
        &router,
        &instance_arn,
        &Application::new("a live application beside the refusals", "live-app"),
    )
    .await;
    put_ok(&router, "seed ENABLED", &live, Some("ENABLED")).await;

    for arn in [UNKNOWN_APPLICATION_ARN, UNKNOWN_APPLICATION_ARN_GOV] {
        // Including the member-less put: an ARN nothing holds is refused whether or not
        // the request asks for a change.
        for status in [Some("ENABLED"), Some("DISABLED"), None] {
            let (code, body) = put_session(&router, arn, status).await;
            assert_eq!(
                code, REFUSAL_STATUS,
                "case {arn:?} / {status:?}: the put of an unminted ARN must answer \
                 {REFUSAL_STATUS}, got {code} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {arn:?} / {status:?}: expected ResourceNotFoundException, got {body}"
            );
        }

        let (code, body) = get_session(&router, arn).await;
        assert_eq!(
            code, REFUSAL_STATUS,
            "case {arn:?}: the get of an unminted ARN must answer {REFUSAL_STATUS}, got \
             {code} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {arn:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    let actual = session_status_of(&router, "a live application beside the refusals", &live).await;
    assert_eq!(
        actual, "ENABLED",
        "a refused put must leave every stored configuration alone, expected \"ENABLED\", \
         got {actual:?}"
    );
}

/// Rule 9: the configuration is a member of the application record, and an application
/// belongs to the instance it was created under, so it lasts exactly as long as that
/// instance does — whatever the configuration had been set to.
///
/// The deletion a `DeleteInstance` owes falls due a span LATER, never at once, so the rows
/// assert BOTH sides of that instant.
#[tokio::test]
async fn a_configuration_lasts_exactly_as_long_as_its_instance() {
    // The rows differ along what the session configuration was when the instance went
    // away: never configured, and each value of the enum put explicitly.
    let mut rows: Vec<(String, Option<&str>, &str)> =
        vec![("never configured".to_string(), None, RESTING_STATUS)];
    for value in STATUS_VALUES {
        rows.push((format!("configured to {value}"), Some(value), value));
    }

    for (case, configured, expected) in rows {
        let case = case.as_str();
        // One engine per row: the account's enablement is settled once, so a deleted
        // instance is never replaced inside the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = router_over(engine.clone());
        let instance_arn = seeded_instance_arn(&router).await;
        let arn = minted_application(
            &router,
            &instance_arn,
            &Application::new("an application of the deleted instance", "doomed-app"),
        )
        .await;
        if let Some(status) = configured {
            put_ok(&router, case, &arn, Some(status)).await;
        }

        let (status, body) = call(
            &router,
            "DeleteInstance",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteInstance must answer 200, got {status} {body}"
        );

        // The removal is still PENDING: the application is there and reports the
        // configuration it was left with.
        let actual = session_status_of(&router, case, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: while the instance deletion is pending the get must still \
             report {expected:?}, got {actual:?}"
        );

        // Move the stored due instant into the past rather than sleeping for it; the
        // service applies the removal itself on the next request.
        hold_deletion(&engine, SystemTime::now() - Duration::from_secs(60));

        let (status, body) = get_session(&router, &arn).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: the get of an application whose instance is gone must answer \
             {REFUSAL_STATUS}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException from the get, got {body}"
        );

        for wanted in STATUS_VALUES {
            let (status, body) = put_session(&router, &arn, Some(wanted)).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?} / {wanted:?}: the put of an application whose instance is \
                 gone must answer {REFUSAL_STATUS}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} / {wanted:?}: expected ResourceNotFoundException from the \
                 put, got {body}"
            );
        }

        // Nothing of the configuration is left behind for a later resource to inherit.
        let remaining = with_store(&engine, |store| store.applications.len());
        assert_eq!(
            remaining, 0,
            "case {case:?}: the applications of a gone instance must be gone too, expected \
             0, got {remaining}"
        );
    }
}
