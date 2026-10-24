// SPDX-License-Identifier: CC0-1.0

//! The application ASSIGNMENT CONFIGURATION slice end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over a single
//! example:
//!
//! 1. an application's assignment configuration is ONE stored flag per application:
//!    `PutApplicationAssignmentConfiguration` REPLACES it and
//!    `GetApplicationAssignmentConfiguration` reports exactly what the last put stored,
//!    for every sequence of values the model's `AssignmentRequired` boolean admits;
//! 2. an application nothing has configured carries the MODEL's default for that member
//!    (`smithy_query(query="operation",operation="GetApplicationAssignmentConfiguration")`
//!    → `AssignmentRequired` `"default": true`), whatever else the create sent — asserted
//!    over a table of creates varying `Status`, `PortalOptions` and `Name`;
//! 3. the put is `@idempotent` and the get is `@readonly`: repeating either answers
//!    identically and changes nothing a later caller sees;
//! 4. the flag belongs to ONE application: configuring one leaves every other
//!    application's configuration alone;
//! 5. the configuration and the application's ASSIGNMENTS are independent — per
//!    `PutApplicationAssignmentConfiguration`'s own documentation, "If an assignment is
//!    created using CreateApplicationAssignment, the user retains access if
//!    AssignmentsRequired is set to true" — so a put neither grants nor revokes an
//!    assignment;
//! 6. an `ApplicationArn` this account holds no application for is refused
//!    `ResourceNotFoundException` by both operations — never a 200 — and the refused put
//!    stores nothing. The status is the one this service answers every refusal of a
//!    request with (`scaffolded::modeled_error_status`), not the shape's `@httpError`;
//! 7. `AssignmentRequired` is `@required`, so a put that omits it is refused
//!    `ValidationException` (HTTP 400) by the modelled constraints before the handler
//!    runs: the member's `@default` is what an UNCONFIGURED application carries, not a
//!    licence to omit it on the wire.
//! 8. the configuration lives INSIDE the application, and the application inside its
//!    instance: while a `DeleteInstance` is still pending the application is still there
//!    and reports its configuration, and once the clock makes the removal due both
//!    operations answer `ResourceNotFoundException` and nothing of the configuration is
//!    left in the store.
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

/// The value `AssignmentRequired` takes when nothing has configured an application: the
/// model's own `@default` on that member, not a value this test chose.
const MODELLED_DEFAULT: bool = true;

/// The HTTP status this service answers a refusal of the REQUEST with, whatever 4xx the
/// refused shape's `@httpError` declares.
///
/// SOURCE: `scaffolded::modeled_error_status`, which records that IAM Identity Center's
/// awsJson1_1 wire responses carry 400 for these refusals; the `__type` stays the
/// model's.
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
/// could plausibly be confused with the assignment configuration — the `Status`
/// (`ENABLED | DISABLED`), whether it was sent at all, and the shape of `PortalOptions`
/// (absent, `Visibility` only, a `SignInOptions`) — plus the length and character class
/// of `Name` (`len 1..100`, `^[\S\s]*$`, so spaces and punctuation are admitted). Every
/// constraint is from `smithy_query(query="operation",operation="CreateApplication")`.
///
/// They do NOT differ along the PROVIDER: `CreateApplication` creates an OAuth 2.0
/// customer managed application, so `custom` is the only published provider it accepts
/// (`state::BuiltInApplicationProvider::accepts_customer_managed_application`).
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

/// `PutApplicationAssignmentConfiguration`. `required` is `None` for a request that sends
/// no `AssignmentRequired` at all, which the member's own `@required` refuses before the
/// handler sees it.
async fn put_configuration(
    router: &MultiServiceRouter,
    application_arn: &str,
    required: Option<bool>,
) -> (u16, Value) {
    let mut body = json!({"ApplicationArn": application_arn});
    if let Some(required) = required {
        body["AssignmentRequired"] = json!(required);
    }
    call(router, "PutApplicationAssignmentConfiguration", &body).await
}

/// `GetApplicationAssignmentConfiguration`.
async fn get_configuration(
    router: &MultiServiceRouter,
    application_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "GetApplicationAssignmentConfiguration",
        &json!({"ApplicationArn": application_arn}),
    )
    .await
}

/// A get that must succeed, as the `AssignmentRequired` it reported.
async fn configuration_of(
    router: &MultiServiceRouter,
    case: &str,
    application_arn: &str,
) -> bool {
    let (status, body) = get_configuration(router, application_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetApplicationAssignmentConfiguration must answer 200, got \
         {status} {body}"
    );
    body["AssignmentRequired"].as_bool().unwrap_or_else(|| {
        panic!("case {case:?}: the get reported no AssignmentRequired, got {body}")
    })
}

/// Rule 2 (with rule 3): every created application starts on the MODELLED default, and
/// reading it is a report.
#[tokio::test]
async fn an_unconfigured_application_reports_the_modelled_default() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    for application in applications() {
        let case = application.label;
        let arn = minted_application(&router, &instance_arn, &application).await;

        let (status, body) = get_configuration(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a get of a fresh application must answer 200, got {status} \
             {body}"
        );
        assert_eq!(
            body["AssignmentRequired"].as_bool(),
            Some(MODELLED_DEFAULT),
            "case {case:?}: an unconfigured application must report the modelled \
             default {MODELLED_DEFAULT}, got {body}"
        );

        // Rule 3: the get reported rather than mutated, so a second one answers
        // identically.
        let (status, again) = get_configuration(&router, &arn).await;
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

/// Rule 1 and rule 3: each sequence of puts leaves the application on the value the LAST
/// put stored, and a repeated put of that value changes nothing.
#[tokio::test]
async fn the_configuration_is_whatever_the_last_put_stored() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // The rows differ along what rule 1 generalises over: the value put and how many puts
    // the application received.
    let sequences: Vec<(&str, Vec<bool>, bool)> = vec![
        ("a single put of false", vec![false], false),
        ("a single put of true", vec![true], true),
        ("false then true", vec![false, true], true),
        ("true then false", vec![true, false], false),
        (
            "false three times over — @idempotent",
            vec![false, false, false],
            false,
        ),
        ("true then true", vec![true, true], true),
        (
            "alternating five times, ending on false",
            vec![true, false, true, false, false],
            false,
        ),
    ];

    for (index, (case, puts, expected)) in sequences.iter().enumerate() {
        let application =
            Application::new("a configured application", &format!("configured-{index}"));
        let arn = minted_application(&router, &instance_arn, &application).await;

        for (step, required) in puts.iter().enumerate() {
            let (status, body) = put_configuration(&router, &arn, Some(*required)).await;
            assert_eq!(
                status, 200,
                "case {case:?} put {step}: the put must answer 200, got {status} {body}"
            );
            assert!(
                body.get("AssignmentRequired").is_none(),
                "case {case:?} put {step}: the put's modelled output carries no members, \
                 got {body}"
            );
        }

        let actual = configuration_of(&router, case, &arn).await;
        assert_eq!(
            actual, *expected,
            "case {case:?}: the get must report the value the last put stored, expected \
             {expected}, got {actual}"
        );

        // Rule 3: one more put of the same value is `@idempotent` — the stored value does
        // not move.
        let last = puts.last().copied();
        let (status, body) = put_configuration(&router, &arn, last).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated put must answer 200, got {status} {body}"
        );
        let again = configuration_of(&router, case, &arn).await;
        assert_eq!(
            again, *expected,
            "case {case:?}: a repeated put must leave the same value, expected \
             {expected}, got {again}"
        );
    }
}

/// Rule 4: the flag is per application, so configuring one application moves no other
/// application's configuration.
#[tokio::test]
async fn configuring_one_application_leaves_every_other_alone() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;

    // One ARN per row of the application table, each given a DIFFERENT value, so a flag
    // stored on the wrong record shows up as another row's value.
    let rows = applications();
    let mut arns: Vec<String> = Vec::new();
    for application in &rows {
        arns.push(minted_application(&router, &instance_arn, application).await);
    }
    // Every other row gets `false`; the rest keep the modelled default.
    let wanted: Vec<bool> = (0..rows.len())
        .map(|index| if index % 2 == 0 { false } else { MODELLED_DEFAULT })
        .collect();

    for (index, arn) in arns.iter().enumerate() {
        if !wanted[index] {
            let (status, body) = put_configuration(&router, arn, Some(false)).await;
            assert_eq!(
                status, 200,
                "case {:?}: the put must answer 200, got {status} {body}",
                rows[index].label
            );
        }
    }

    for (index, arn) in arns.iter().enumerate() {
        let case = rows[index].label;
        let actual = configuration_of(&router, case, arn).await;
        assert_eq!(
            actual, wanted[index],
            "case {case:?}: each application keeps its own configuration, expected {}, \
             got {actual}",
            wanted[index]
        );
    }
}

/// Rule 5: the configuration says how access is GAINED, it is not the assignment itself —
/// a put neither grants nor revokes one.
#[tokio::test]
async fn a_put_neither_grants_nor_revokes_an_assignment() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let arn = minted_application(
        &router,
        &instance_arn,
        &Application::new("an application with one assigned principal", "assigned-app"),
    )
    .await;

    // A `PrincipalId` of the model's form (`UUId`-like identity-store id) and both
    // `PrincipalType` values the model declares (`USER | GROUP`).
    let principals = [
        ("USER", "90677c9a-10d1-7034-b5a2-0e6a43ac1d2f"),
        ("GROUP", "a0e7fc4a-b031-70a1-5d9c-4b1f0a2e8c33"),
    ];
    for (principal_type, principal_id) in principals {
        let (status, body) = call(
            &router,
            "CreateApplicationAssignment",
            &json!({
                "ApplicationArn": arn,
                "PrincipalType": principal_type,
                "PrincipalId": principal_id,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal_type:?}: CreateApplicationAssignment must answer 200, got \
             {status} {body}"
        );
    }

    // Walk the flag through both values; the assignments must survive each put.
    for required in [false, true, false] {
        let (status, body) = put_configuration(&router, &arn, Some(required)).await;
        assert_eq!(
            status, 200,
            "case {required}: the put must answer 200, got {status} {body}"
        );
        let actual = configuration_of(&router, &format!("put {required}"), &arn).await;
        assert_eq!(
            actual, required,
            "case {required}: the get must report the value just put, got {actual}"
        );

        for (principal_type, principal_id) in principals {
            let (status, body) = call(
                &router,
                "DescribeApplicationAssignment",
                &json!({
                    "ApplicationArn": arn,
                    "PrincipalType": principal_type,
                    "PrincipalId": principal_id,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {required} / {principal_type:?}: the assignment must survive the \
                 put, got {status} {body}"
            );
            assert_eq!(
                body["PrincipalId"].as_str(),
                Some(principal_id),
                "case {required} / {principal_type:?}: the describe must report the \
                 assigned principal, got {body}"
            );
        }
    }
}

/// Rule 6: an `ApplicationArn` nothing holds is refused by both operations, and the
/// refused put stores nothing.
#[tokio::test]
async fn an_unminted_application_arn_is_refused_by_both_operations() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    // An application that DOES exist, so the refusals below are about the ARN and not
    // about an empty store. It is configured AWAY from the modelled default first, so a
    // refused put that wrote the flag onto the wrong record would show up as a changed
    // value rather than as the value it started on.
    let live = minted_application(
        &router,
        &instance_arn,
        &Application::new("a live application beside the refusals", "live-app"),
    )
    .await;
    let (status, body) = put_configuration(&router, &live, Some(false)).await;
    assert_eq!(
        status, 200,
        "case {live:?}: seeding the live application to false must answer 200, got \
         {status} {body}"
    );

    for arn in [UNKNOWN_APPLICATION_ARN, UNKNOWN_APPLICATION_ARN_GOV] {
        for required in [Some(true), Some(false)] {
            let (status, body) = put_configuration(&router, arn, required).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {arn:?} / {required:?}: the put of an unminted ARN must answer \
                 {REFUSAL_STATUS}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {arn:?} / {required:?}: expected ResourceNotFoundException, got \
                 {body}"
            );
        }

        let (status, body) = get_configuration(&router, arn).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {arn:?}: the get of an unminted ARN must answer {REFUSAL_STATUS}, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {arn:?}: expected ResourceNotFoundException, got {body}"
        );
    }

    // The refused puts stored nothing: the live application still carries the value it
    // was seeded with, and the unminted ARNs still resolve to nothing.
    let actual = configuration_of(&router, "a live application beside the refusals", &live).await;
    assert!(
        !actual,
        "case {live:?}: a refused put must leave every stored configuration alone, \
         expected false, got {actual}"
    );
}

/// `AssignmentRequired` is `@required`, so a put that omits it is refused by the modelled
/// constraints BEFORE the handler runs — a 400 `ValidationException`, whatever the ARN
/// resolves to, and nothing is stored. The member's `@default` is what an UNCONFIGURED
/// application carries (rule 2), not a licence to omit it on the wire.
#[tokio::test]
async fn a_put_that_omits_assignment_required_is_refused() {
    let router = router();
    let instance_arn = seeded_instance_arn(&router).await;
    let arn = minted_application(
        &router,
        &instance_arn,
        &Application::new("an application configured to false", "omitting-app"),
    )
    .await;

    let (status, body) = put_configuration(&router, &arn, Some(false)).await;
    assert_eq!(
        status, 200,
        "case \"seed false\": the put must answer 200, got {status} {body}"
    );

    for case in [arn.as_str(), UNKNOWN_APPLICATION_ARN] {
        let (status, body) = put_configuration(&router, case, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a put with no AssignmentRequired must answer 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    let actual = configuration_of(&router, "an application configured to false", &arn).await;
    assert!(
        !actual,
        "case {arn:?}: the refused put must store nothing, expected false, got {actual}"
    );
}

/// Rule 8: the configuration is a field of the application record, and an application
/// belongs to the instance it was created under, so it lasts exactly as long as that
/// instance does — whatever the configuration had been set to.
///
/// The deletion a `DeleteInstance` owes falls due a span LATER, never at once, so the
/// rows assert BOTH sides of that instant: before it the application still reports its
/// configuration, after it both operations refuse the ARN.
#[tokio::test]
async fn a_configuration_lasts_exactly_as_long_as_its_instance() {
    // The rows differ along what the configuration was when the instance went away: never
    // configured (so the modelled default), configured away from it, and configured back
    // onto it by an explicit put.
    let rows: Vec<(&str, Option<bool>, bool)> = vec![
        ("never configured", None, MODELLED_DEFAULT),
        ("configured to false", Some(false), false),
        ("configured to true explicitly", Some(true), true),
    ];

    for (case, configured, expected) in rows {
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
        if let Some(required) = configured {
            let (status, body) = put_configuration(&router, &arn, Some(required)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the put must answer 200, got {status} {body}"
            );
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
        let actual = configuration_of(&router, case, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: while the instance deletion is pending the get must still \
             report {expected}, got {actual}"
        );

        // Move the stored due instant into the past rather than sleeping for it; the
        // service applies the removal itself on the next request.
        hold_deletion(&engine, SystemTime::now() - Duration::from_secs(60));

        let (status, body) = get_configuration(&router, &arn).await;
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

        for required in [Some(true), Some(false)] {
            let (status, body) = put_configuration(&router, &arn, required).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?} / {required:?}: the put of an application whose instance is \
                 gone must answer {REFUSAL_STATUS}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?} / {required:?}: expected ResourceNotFoundException from the \
                 put, got {body}"
            );
        }

        // Nothing of the configuration is left behind for a later resource to inherit.
        let remaining = with_store(&engine, |store| store.applications.len());
        assert_eq!(
            remaining, 0,
            "case {case:?}: the applications of a gone instance must be gone too, \
             expected 0, got {remaining}"
        );
    }
}
