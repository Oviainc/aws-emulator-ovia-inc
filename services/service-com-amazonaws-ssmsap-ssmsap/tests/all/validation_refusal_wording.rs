// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusals it raises itself, driven over the wire.
//!
//! The rule these cases pin: a refusal this service raises after the modelled constraints
//! are satisfied carries the service's own wording for that error class — the fixed text
//! exactly, and as variable parts only values the request carried or the store holds —
//! together with the status and the wire error code of the class. Nothing of the
//! environment the call reached (the account the request was signed for, the Region's
//! endpoint) is worded into a message whose condition is not about it, so the same
//! request is refused in the same words in every account and Region.
//!
//! Four classes, one per refusal this suite spans:
//!
//! * The resource-permission calls are DATABASE-scoped: `ResourceArn` has to name a SAP
//!   HANA database this account and Region holds, and an ARN that does not — whatever it
//!   does name — is the `ValidationException` (HTTP 400) they declare, stating that the
//!   database ARN is not valid, with `PutResourcePermission` tagging the input member its
//!   refusal is about.
//! * `UpdateApplicationSettings` takes an `ApplicationId` plus at least one update
//!   member, and a request naming none of them is refused for naming no update option —
//!   before the application is resolved, so an unregistered id is refused the same way.
//! * `GetDatabase` identifies a database by its own ARN or by the application, component
//!   and database ids, so a request naming neither alternative is refused naming both.
//! * `ListOperations` requires an `ApplicationId`, and one that names no registered
//!   application is refused with this operation's own wording for that condition.
//!
//! The tables span what those rules generalise over: for the permission calls, each of
//! the three operations, the read's narrowing, and the ways an ARN can name no database
//! (the ARN of a registered application, of one of its components, of a database its
//! registration never named, of a deregistered one, one of another Region, one of another
//! service, both branches of the modelled `Arn` pattern) beside the databases that DO
//! resolve as a positive control; for the three message classes, the members the request
//! carries and whether the application it names is registered.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state` directly, and the expected
//! texts are written out here rather than read off the crate, so what they pin is the
//! wording and not the handler's own construction of it.

use serde_json::{json, Value};

use crate::application_lifecycle::{settle, Emulator, Reply};
use crate::resource_permissions::{
    assert_invalid_database_arn, get_in_region, put_in_region, RESOURCE_ARN_MEMBER,
};
use crate::resource_tags::{application_arn, database_arn, register, registration};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::PERMISSION_ACTION_TYPE_VALUES;

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region this suite signs for, which is the Region every resource it registers lives
/// in.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so an ARN can name a resource of a Region the call is not
/// made at.
const OTHER_REGION: &str = "eu-west-1";

/// The registration whose resources the permission rows point at: a HANA application with
/// two SAP HANA databases.
const PRESENT: &str = "wording-present";

/// The registration that is deregistered, so its databases stop existing.
const GONE: &str = "wording-gone";

/// The databases [`PRESENT`]'s credentials name.
const DATABASES: &[&str] = &["HDB", "SYSTEMDB"];

/// The `ComponentId` of the SAP HANA component the registrations describe: a component is
/// identified by the System ID and the `ComponentType` it was declared with, and
/// `crate::application_lifecycle::valid_registration` carries the System ID `HDB`.
const HANA_COMPONENT: &str = "HDB-HANA";

/// The source resource a grant names: a source is the counterparty a restore is permitted
/// for, so it need not be a resource of this service at all.
const SOURCE: &str = "arn:aws:s3:::sap-backint-bucket/restore";

/// `DeleteResourcePermission` for every grant held on `resource_arn`, at `region`'s
/// endpoint.
async fn delete_in_region(emulator: &Emulator, region: &str, resource_arn: &str) -> Reply {
    emulator
        .send_in_region(
            region,
            "/delete-resource-permission",
            json!({ "ResourceArn": resource_arn }),
        )
        .await
}

/// A refusal this service raised itself: the `ValidationException` (HTTP 400) every
/// operation of this service declares, with the exact message the class is worded with.
fn assert_refused(reply: &Reply, expected: &str, case: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
    );
    let stated = message(reply);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected {expected:?}, got {stated:?}"
    );
}

/// Assert `stated` carries nothing of the environment the call reached: the account the
/// request was signed for and the Region's name belong in a message only where the
/// condition is about them — which the refusals built from an ARN are, since the ARN the
/// request carried embeds both.
fn assert_free_of_environment(stated: &str, case: &str) {
    assert!(
        !stated.contains(ACCOUNT),
        "case {case:?}: expected no account id in the refusal, got {stated:?}"
    );
    assert!(
        !stated.contains(REGION),
        "case {case:?}: expected no Region in the refusal, got {stated:?}"
    );
}

/// An emulator holding [`PRESENT`] and a deregistered [`GONE`], both settled.
async fn emulator_with_registrations() -> Emulator {
    let emulator = Emulator::new();
    register(
        &emulator,
        PRESENT,
        registration(PRESENT, &[], Some(DATABASES)),
    )
    .await;
    register(&emulator, GONE, registration(GONE, &[], Some(&["HDB"]))).await;
    let reply = emulator
        .send("/deregister-application", json!({ "ApplicationId": GONE }))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    // Both the registrations and the deregistration are asynchronous: waiting the clock
    // out leaves every resource the rows name in its terminal state.
    settle().await;
    emulator
}

/// The ARNs that name no SAP HANA database of this account and Region: (label, the Region
/// the call is made at, the ARN it names).
fn unresolvable_arns() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "the own ARN of a registered application",
            REGION,
            application_arn(REGION, PRESENT),
        ),
        (
            "a component of a registered application",
            REGION,
            format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:component/{PRESENT}/{HANA_COMPONENT}"),
        ),
        (
            "a database the registration never named",
            REGION,
            database_arn(REGION, PRESENT, "NEVER"),
        ),
        (
            "a database of an application never registered",
            REGION,
            database_arn(REGION, "wording-never-registered", "HDB"),
        ),
        (
            "a database of an application that was deregistered",
            REGION,
            database_arn(REGION, GONE, "HDB"),
        ),
        (
            "a database of this Region named at another endpoint",
            OTHER_REGION,
            database_arn(REGION, PRESENT, DATABASES[0]),
        ),
        (
            "an ARN carrying another Region",
            REGION,
            database_arn(OTHER_REGION, PRESENT, DATABASES[0]),
        ),
        (
            "another service's resource in the colon-segment ARN form",
            REGION,
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:sap-prd"),
        ),
        (
            "another service's resource in the path ARN form",
            REGION,
            "arn:aws:s3:::a-bucket/an-object".to_string(),
        ),
    ]
}

/// RULE: the resource-permission calls are database-scoped, so an ARN that names no SAP
/// HANA database of this account and Region is refused with the modelled
/// `ValidationException` (HTTP 400) stating that the database ARN is not valid — the grant
/// tagging the input member it is about, the read and the revocation stating the condition
/// alone — while the databases that DO resolve are served.
#[tokio::test]
async fn an_arn_that_names_no_database_is_refused_as_an_invalid_database_arn() {
    let emulator = emulator_with_registrations().await;
    let action = PERMISSION_ACTION_TYPE_VALUES[0];

    for (label, region, arn) in unresolvable_arns() {
        let grant = put_in_region(&emulator, region, &arn, action, SOURCE).await;
        assert_invalid_database_arn(&grant, Some(RESOURCE_ARN_MEMBER), &arn, label);

        for (narrowing, requested) in [("unnarrowed", None), ("narrowed", Some(action))] {
            let case = format!("{label} / {narrowing}");
            let read = get_in_region(&emulator, region, &arn, requested).await;
            assert_invalid_database_arn(&read, None, &arn, &case);
        }

        let revoked = delete_in_region(&emulator, region, &arn).await;
        assert_invalid_database_arn(&revoked, None, &arn, label);
    }

    // The positive control: every database the registration DID name is served by all
    // three calls, so what the rows above pin is the ARN and not the calls.
    for database_id in DATABASES {
        let arn = database_arn(REGION, PRESENT, database_id);
        let case = format!("the registered database {database_id}");
        let grant = put_in_region(&emulator, REGION, &arn, action, SOURCE).await;
        assert_eq!(
            grant.status, 200,
            "case {case:?}: expected 200 from PutResourcePermission, got {grant:?}"
        );
        let read = get_in_region(&emulator, REGION, &arn, Some(action)).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetResourcePermission, got {read:?}"
        );
        let revoked = delete_in_region(&emulator, REGION, &arn).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: expected 200 from DeleteResourcePermission, got {revoked:?}"
        );
    }
}

/// RULE: `UpdateApplicationSettings` takes an `ApplicationId` plus at least one update
/// member, so a request that names no update option is refused for exactly that — in
/// fixed text naming no member and nothing of the request — before the application it
/// names is resolved.
#[tokio::test]
async fn an_update_naming_no_option_is_refused_for_naming_no_update_option() {
    let emulator = emulator_with_registrations().await;
    let expected = "No update option was specified.";

    // The rows: the members a request can carry beside the `@required` `ApplicationId`
    // without naming any update — including the EMPTY credential lists, which the model
    // admits (`ApplicationCredentialList` length 0..20) and which ask for no change.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a registered application and nothing else",
            json!({ "ApplicationId": PRESENT }),
        ),
        (
            "an application that was never registered",
            json!({ "ApplicationId": "wording-never-registered" }),
        ),
        (
            "an application that was deregistered",
            json!({ "ApplicationId": GONE }),
        ),
        (
            "empty credential lists",
            json!({
                "ApplicationId": PRESENT,
                "CredentialsToAddOrUpdate": [],
                "CredentialsToRemove": [],
            }),
        ),
    ];

    for (label, body) in rows {
        let reply = emulator.send("/update-application-settings", body).await;
        assert_refused(&reply, expected, label);
        assert_free_of_environment(&message(&reply), label);
    }

    // The positive control: an update that DOES name an option is served, so the refusal
    // is about the request naming none.
    let reply = emulator
        .send(
            "/update-application-settings",
            json!({
                "ApplicationId": PRESENT,
                "DatabaseArn": database_arn(REGION, PRESENT, DATABASES[0]),
            }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"an update naming one option\": expected 200, got {reply:?}"
    );
}

/// RULE: `GetDatabase` identifies the database to read by its own ARN or by the
/// application, component and database ids, so a request that names neither alternative is
/// refused in fixed text naming BOTH of them.
#[tokio::test]
async fn a_read_naming_no_database_is_refused_naming_both_alternatives() {
    let emulator = emulator_with_registrations().await;
    let expected = "Either database ARN or a combination of applicationId, componentId and \
                    databaseId must be provided";

    // The rows: the members a request can carry without naming a database to read — none
    // at all, and each of the two that only NARROW where a reference is resolved.
    let rows: Vec<(&str, Value)> = vec![
        ("no member at all", json!({})),
        (
            "a registered application alone",
            json!({ "ApplicationId": PRESENT }),
        ),
        (
            "an unregistered application alone",
            json!({ "ApplicationId": "wording-never-registered" }),
        ),
        (
            "a component alone",
            json!({ "ComponentId": HANA_COMPONENT }),
        ),
        (
            "an application and a component",
            json!({ "ApplicationId": PRESENT, "ComponentId": HANA_COMPONENT }),
        ),
    ];

    for (label, body) in rows {
        let reply = emulator.send("/get-database", body).await;
        assert_refused(&reply, expected, label);
        assert_free_of_environment(&message(&reply), label);
        assert!(
            reply.member("Database").is_none(),
            "case {label:?}: a refusal reports no database, got {reply:?}"
        );
    }

    // The positive control: a request that DOES name a database by its own ARN is served.
    let reply = emulator
        .send(
            "/get-database",
            json!({ "DatabaseArn": database_arn(REGION, PRESENT, DATABASES[0]) }),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case \"a database named by its ARN\": expected 200, got {reply:?}"
    );
}

/// RULE: `ListOperations` reports the records of ONE registered application, so an
/// `ApplicationId` that names no application of this account and Region is refused for
/// not existing — the id quoted inside fixed text, with nothing of the environment the
/// call reached.
#[tokio::test]
async fn a_listing_of_an_unregistered_application_is_refused_for_not_existing() {
    let emulator = emulator_with_registrations().await;

    // The rows: references no registration of this Region answers, spanning the ways an
    // id can fail to resolve and the forms the modelled `ApplicationId` pattern
    // (`^[\w\d\.-]+$`, length 1..60) admits.
    let rows: Vec<(&str, String)> = vec![
        (
            "an application never registered",
            "wording-never-registered".to_string(),
        ),
        ("an application that was deregistered", GONE.to_string()),
        ("a dotted id", "wording.never.registered".to_string()),
        ("an underscored id", "wording_never_registered".to_string()),
        ("a single-character id", "x".to_string()),
        ("the longest id the model admits", "w".repeat(60)),
    ];

    for (label, reference) in rows {
        let reply = emulator
            .send("/list-operations", json!({ "ApplicationId": reference }))
            .await;
        let expected = format!("The application '{reference}' does not exist");
        assert_refused(&reply, &expected, label);
        assert_free_of_environment(&message(&reply), label);
        assert!(
            reply.member("Operations").is_none(),
            "case {label:?}: a refusal reports no operations, got {reply:?}"
        );
    }

    // The same id at another regional endpoint is refused in the same words, while the
    // Region that holds the registration serves the listing: the wording is the
    // condition's, and the condition is per Region.
    let elsewhere = emulator
        .send_in_region(
            OTHER_REGION,
            "/list-operations",
            json!({ "ApplicationId": PRESENT }),
        )
        .await;
    let expected = format!("The application '{PRESENT}' does not exist");
    assert_refused(&elsewhere, &expected, "a registration of another Region");

    let served = emulator
        .send("/list-operations", json!({ "ApplicationId": PRESENT }))
        .await;
    assert_eq!(
        served.status, 200,
        "case \"the registered application\": expected 200, got {served:?}"
    );
}
