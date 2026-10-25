// SPDX-License-Identifier: CC0-1.0

//! The AppRegistry application a registration is recorded in, driven over the wire.
//!
//! RULE: registering an SAP application records it in AWS Service Catalog AppRegistry,
//! so the registration assigns that application an AppRegistry application ARN of the
//! Region it was registered in; every projection of the application reports that ARN in
//! `Application$AppRegistryArn` for as long as the application exists, and the ARN is an
//! identifier `GetApplication` resolves that one application by.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: the assigned ARN is only
//! believed when a later `GetApplication` reports it back, and resolution by it is only
//! believed when the application it names comes back.
//!
//! The table spans what the rule generalises over: the application type, the form of the
//! `ApplicationId` the registration carried (hyphenated upper case, lower case, dotted,
//! underscored, one character, the modelled 60-character maximum), and whether the
//! registration carried components, a database ARN or tags — none of which the assigned
//! ARN may depend on. The Region is spanned by its own test, and the issue's `HDB-TEST`
//! HANA registration is ONE row.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The AppRegistry member under test, as the model names it.
const APP_REGISTRY_ARN: &str = "AppRegistryArn";

/// The regional endpoints this suite signs for — AWS Region codes differing in every
/// segment of the code, so the Region row is not one geography. `us-east-1` is also the
/// Region an unsigned request falls back to, so it is one row rather than the only one.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2"];

/// Whether `value` has the form the modelled `AppRegistryArn` admits.
///
/// Source: `smithy_query(query="shape", shape="AppRegistryArn",
/// model_path="services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/com-amazonaws-ssmsap-ssmsap.json")`
/// — `^arn:aws:servicecatalog:[a-z0-9:\/-]+$`, so after the `servicecatalog` prefix only
/// lower-case letters, digits, `:`, `/` and `-` are admitted, and at least one character
/// has to follow.
fn is_app_registry_arn(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:aws:servicecatalog:") else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '/' | '-'))
}

/// The prefix an AppRegistry application ARN of `region` carries: an AppRegistry
/// application is a Service Catalog resource of the Region and account that holds it.
fn app_registry_arn_prefix(region: &str) -> String {
    format!("arn:aws:servicecatalog:{region}:{ACCOUNT}:/applications/")
}

/// The `AppRegistryArn` of the `Application` a reply reports, or `None` when it reports
/// none.
fn reported(reply: &Reply) -> Option<String> {
    reply
        .app(APP_REGISTRY_ARN)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One registration, and what makes it a distinct row.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// Extra members the registration carries beyond the required set, as
    /// `(member, value)` — the registration detail the assigned ARN may not depend on.
    extras: Vec<(&'static str, Value)>,
}

/// The rows: the `ApplicationId` forms the modelled `^[\w\d\.-]+$` admits, both
/// `ApplicationType` values, and the optional members a registration can carry.
fn cases() -> Vec<Case> {
    vec![
        // The registration the issue records.
        Case {
            name: "the recorded HANA registration",
            application_id: "HDB-TEST",
            application_type: "HANA",
            extras: Vec::new(),
        },
        Case {
            name: "a lower-case id with tags",
            application_id: "hdb-tagged",
            application_type: "HANA",
            extras: vec![("Tags", json!({"env": "dev"}))],
        },
        Case {
            name: "a dotted id, ABAP, with components",
            application_id: "abap.app.01",
            application_type: "SAP_ABAP",
            extras: vec![(
                "ComponentsInfo",
                json!([{"ComponentType": "HANA", "Sid": "ABC", "Ec2InstanceId": "i-0123456789abcdef0"}]),
            )],
        },
        Case {
            name: "an underscored id naming a database by ARN",
            application_id: "ABAP_DB_2",
            application_type: "SAP_ABAP",
            extras: vec![(
                "DatabaseArn",
                json!(format!(
                    "arn:aws:ssm-sap:us-east-1:{ACCOUNT}:database/ABAP_DB_2/HDB"
                )),
            )],
        },
        Case {
            name: "the shortest id the model admits",
            application_id: "A",
            application_type: "HANA",
            extras: Vec::new(),
        },
        Case {
            name: "the longest id the model admits (60 characters)",
            // `ApplicationId` is modelled len 1..60.
            application_id: "aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffff",
            application_type: "SAP_ABAP",
            extras: Vec::new(),
        },
    ]
}

/// The body for a row: the smallest accepted registration for its type, plus its extras.
fn registration(case: &Case) -> Value {
    let mut body = valid_registration(case.application_id, case.application_type);
    let object = body
        .as_object_mut()
        .expect("a registration body is an object");
    for (member, value) in &case.extras {
        object.insert((*member).to_string(), value.clone());
    }
    body
}

/// RULE, first half: a registration assigns the application an AppRegistry application
/// ARN of the modelled form, which every read of that application reports unchanged.
#[tokio::test]
async fn a_registration_assigns_an_app_registry_arn_every_read_reports() {
    let mut assigned: BTreeSet<String> = BTreeSet::new();
    let emulator = Emulator::new();

    for case in cases() {
        let name = case.name;
        let registered = emulator.send("/register-application", registration(&case)).await;
        assert_eq!(
            registered.status, 200,
            "case {name:?}: expected 200 from RegisterApplication, got {registered:?}"
        );

        let minted = reported(&registered).unwrap_or_else(|| {
            panic!("case {name:?}: RegisterApplication must report an AppRegistryArn, got {registered:?}")
        });
        assert!(
            is_app_registry_arn(&minted),
            "case {name:?}: expected an ARN of the modelled AppRegistryArn form, got {minted:?}"
        );
        assert!(
            minted.starts_with(&app_registry_arn_prefix("us-east-1")),
            "case {name:?}: expected an AppRegistry application ARN of this account and Region, got {minted:?}"
        );
        let app_registry_application_id = minted
            .strip_prefix(&app_registry_arn_prefix("us-east-1"))
            .unwrap_or_default();
        assert!(
            !app_registry_application_id.is_empty()
                && !app_registry_application_id.contains('/')
                && !app_registry_application_id.contains(case.application_id),
            "case {name:?}: the AppRegistry application id is assigned, not built from the request, got {app_registry_application_id:?}"
        );
        assert!(
            assigned.insert(minted.clone()),
            "case {name:?}: every registration gets its OWN AppRegistry application, got the already-assigned {minted:?}"
        );

        // A read reports the SAME ARN, while the application is still REGISTERING and
        // again once the clock has brought it to ACTIVATED: the ARN belongs to the
        // application, not to a status or to the call that observed it.
        let while_registering = emulator
            .send(
                "/get-application",
                json!({"ApplicationId": case.application_id}),
            )
            .await;
        assert_eq!(
            reported(&while_registering).as_deref(),
            Some(minted.as_str()),
            "case {name:?}: GetApplication must report the assigned ARN, got {while_registering:?}"
        );
    }

    settle().await;

    for case in cases() {
        let name = case.name;
        let read = emulator
            .send(
                "/get-application",
                json!({"ApplicationId": case.application_id}),
            )
            .await;
        let after = reported(&read).unwrap_or_else(|| {
            panic!("case {name:?}: GetApplication must report an AppRegistryArn, got {read:?}")
        });
        assert!(
            assigned.contains(&after),
            "case {name:?}: a settled application reports the ARN it was assigned, got {after:?}"
        );
        assert_eq!(
            read.app_str("Status"),
            "ACTIVATED",
            "case {name:?}: expected the settled status, got {read:?}"
        );
    }
}

/// RULE, second half: the assigned ARN is an identifier of that application alone, so
/// `GetApplication` carrying it as its only member resolves the same application an id
/// or an application ARN resolves.
#[tokio::test]
async fn get_application_resolves_an_application_by_its_assigned_app_registry_arn() {
    let emulator = Emulator::new();

    // Every row is registered, so each lookup has the other applications to tell its own
    // from.
    let mut minted: Vec<(&'static str, String)> = Vec::new();
    for case in cases() {
        let name = case.name;
        let registered = emulator.send("/register-application", registration(&case)).await;
        let arn = reported(&registered).unwrap_or_else(|| {
            panic!("case {name:?}: RegisterApplication must report an AppRegistryArn, got {registered:?}")
        });
        minted.push((case.application_id, arn));
    }
    settle().await;

    for (application_id, arn) in &minted {
        let by_registry = emulator
            .send("/get-application", json!({APP_REGISTRY_ARN: arn}))
            .await;
        assert_eq!(
            by_registry.status, 200,
            "case {application_id:?}: expected 200 resolving by {arn:?}, got {by_registry:?}"
        );
        assert_eq!(
            by_registry.app_str("Id"),
            *application_id,
            "case {application_id:?}: resolving by {arn:?} must answer that application, got {by_registry:?}"
        );
        assert_eq!(
            reported(&by_registry).as_deref(),
            Some(arn.as_str()),
            "case {application_id:?}: the resolved application reports the ARN it was resolved by, got {by_registry:?}"
        );

        // The three identifiers name ONE application, so the lookup by AppRegistry ARN
        // answers exactly what the lookup by id answers.
        let by_id = emulator
            .send("/get-application", json!({"ApplicationId": application_id}))
            .await;
        assert_eq!(
            by_registry.body, by_id.body,
            "case {application_id:?}: the AppRegistryArn lookup must answer the id lookup's application, got {by_registry:?} vs {by_id:?}"
        );

        let by_application_arn = emulator
            .send(
                "/get-application",
                json!({"ApplicationArn": by_id.app_str("Arn")}),
            )
            .await;
        assert_eq!(
            by_registry.body, by_application_arn.body,
            "case {application_id:?}: the AppRegistryArn lookup must answer the ApplicationArn lookup's application, got {by_registry:?} vs {by_application_arn:?}"
        );
    }
}

/// RULE: the AppRegistry application is of the Region the registration was made in, so
/// its ARN carries that Region and resolves the application at THAT regional endpoint
/// alone.
#[tokio::test]
async fn an_assigned_app_registry_arn_is_scoped_to_the_region_it_was_registered_in() {
    let emulator = Emulator::new();
    let application_id = "HDB-REGIONAL";

    let mut by_region: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let registered = emulator
            .send_in_region(
                region,
                "/register-application",
                valid_registration(application_id, "HANA"),
            )
            .await;
        assert_eq!(
            registered.status, 200,
            "case {region:?}: expected 200 from RegisterApplication, got {registered:?}"
        );
        let arn = reported(&registered).unwrap_or_else(|| {
            panic!("case {region:?}: RegisterApplication must report an AppRegistryArn, got {registered:?}")
        });
        assert!(
            arn.starts_with(&app_registry_arn_prefix(region)),
            "case {region:?}: expected an AppRegistry ARN of that Region, got {arn:?}"
        );
        by_region.push((*region, arn));
    }
    settle().await;

    for (region, arn) in &by_region {
        let local = emulator
            .send_in_region(region, "/get-application", json!({APP_REGISTRY_ARN: arn}))
            .await;
        assert_eq!(
            local.status, 200,
            "case {region:?}: the ARN resolves at its own endpoint, got {local:?}"
        );

        for (other, _) in by_region.iter().filter(|(other, _)| other != region) {
            let foreign = emulator
                .send_in_region(other, "/get-application", json!({APP_REGISTRY_ARN: arn}))
                .await;
            assert_eq!(
                foreign.status, 400,
                "case {region:?} read at {other:?}: another Region's AppRegistry ARN resolves nothing, got {foreign:?}"
            );
            assert_eq!(
                foreign.code(),
                VALIDATION_EXCEPTION,
                "case {region:?} read at {other:?}: expected {VALIDATION_EXCEPTION}, got {foreign:?}"
            );
            assert_eq!(
                message(&foreign),
                format!(
                    "No registered application found for account: {ACCOUNT}, and application: {arn}"
                ),
                "case {region:?} read at {other:?}: the refusal names the reference the request carried, got {foreign:?}"
            );
        }
    }
}

/// RULE: the AppRegistry application belongs to the registration, so it lives exactly as
/// long as one — a deregistered application is no longer resolvable by its ARN, and
/// registering the same id again records a NEW AppRegistry application rather than
/// reviving the old one.
#[tokio::test]
async fn an_app_registry_arn_lives_exactly_as_long_as_the_registration_it_was_assigned_to()
{
    let emulator = Emulator::new();
    let application_id = "HDB-TEST";

    let first = emulator
        .send(
            "/register-application",
            valid_registration(application_id, "HANA"),
        )
        .await;
    let first_arn = reported(&first).expect("the first registration reports an AppRegistryArn");
    settle().await;

    let deregistered = emulator
        .send(
            "/deregister-application",
            json!({"ApplicationId": application_id}),
        )
        .await;
    assert_eq!(
        deregistered.status, 200,
        "case {application_id:?}: expected 200 from DeregisterApplication, got {deregistered:?}"
    );
    settle().await;

    let gone = emulator
        .send("/get-application", json!({APP_REGISTRY_ARN: &first_arn}))
        .await;
    assert_eq!(
        gone.status, 400,
        "case {application_id:?}: a deregistered application's AppRegistry ARN resolves nothing, got {gone:?}"
    );
    assert_eq!(
        gone.code(),
        VALIDATION_EXCEPTION,
        "case {application_id:?}: expected {VALIDATION_EXCEPTION}, got {gone:?}"
    );

    let second = emulator
        .send(
            "/register-application",
            valid_registration(application_id, "HANA"),
        )
        .await;
    let second_arn = reported(&second).expect("the second registration reports an AppRegistryArn");
    assert_ne!(
        second_arn, first_arn,
        "case {application_id:?}: a new registration records a new AppRegistry application, got {second_arn:?} twice"
    );
    settle().await;

    let resolved = emulator
        .send("/get-application", json!({APP_REGISTRY_ARN: &second_arn}))
        .await;
    assert_eq!(
        resolved.app_str("Id"),
        application_id,
        "case {application_id:?}: the new ARN resolves the new registration, got {resolved:?}"
    );
    let stale = emulator
        .send("/get-application", json!({APP_REGISTRY_ARN: &first_arn}))
        .await;
    assert_eq!(
        stale.status, 400,
        "case {application_id:?}: the ARN of the ended registration stays unresolvable, got {stale:?}"
    );
}
