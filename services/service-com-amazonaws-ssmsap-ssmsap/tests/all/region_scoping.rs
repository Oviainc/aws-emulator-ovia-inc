// SPDX-License-Identifier: CC0-1.0

//! Region scoping: Systems Manager for SAP holds a registration in the account AND the
//! Region it was made in, so a call reaches only the applications of the regional
//! endpoint it was signed for.
//!
//! The rule pinned here: every application-scoped operation resolves its resource in the
//! Region its request's SigV4 credential scope names, so a resource registered at one
//! regional endpoint is never listed, read, controlled or deregistered through another,
//! and an `ApplicationId` in use in one Region is free in every other.
//!
//! The tables span what the rule generalises over: the Region a resource is registered
//! in, the Region a later call is signed for (every other one, including a Region nothing
//! was ever registered in), the operation making that call, the identifier form it names
//! the resource by (`ApplicationId` / `ApplicationArn` / `OperationId`), and the number of
//! applications a Region holds. The issue's `us-east-1` / `eu-west-1` pair is two rows of
//! the Region table.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns, driven here through its `send_in_region`),
//! so the restJson1 routing, the boundary constraint walk, the handler and the response
//! serialization all run as they do in the emulator binary. Nothing reaches into
//! `crate::state`: a registration is only believed when a later call reports it back.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    no_operation_message, FILTER_VALUE_MAX_LENGTH, TRANSITION_SPAN,
};

use crate::application_lifecycle::{valid_registration, Emulator, Reply};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The account every emulated ARN and account-scoped refusal carries, from the core's
/// single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The modelled error code the application-CONTROL operations declare for an application
/// that is not registered (`smithy_query(query="operation",
/// operation="StartApplication,StopApplication")`: `ResourceNotFoundException`,
/// `@httpError 404`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The regional endpoints this suite signs for. AWS Region codes, one per partition-free
/// commercial geography, so the rows differ in every segment of the code (geography,
/// direction, ordinal); `us-east-1` is also the Region an UNSIGNED request falls back to
/// (`service_com_amazonaws_ssmsap_ssmsap::state::DEFAULT_REGION`), so it is one row
/// rather than the only one.
const REGIONS: &[&str] = &["us-east-1", "eu-west-1", "ap-southeast-2", "sa-east-1"];

/// A Region this suite never registers anything in: a listing there must be empty rather
/// than report another Region's applications.
const UNUSED_REGION: &str = "me-central-1";

/// An `ApplicationId` this suite registers in EVERY Region, which is what makes the
/// id-reuse row a real one: an id is unique per account and Region, not globally.
const SHARED_ID: &str = "HDB-SHARED";

/// Wait out a status change the clock owes. The emulator never sleeps; this is the test
/// waiting for the clock, which is the only thing that can make the change due.
async fn settle() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(300)).await;
}

/// The ARN `RegisterApplication` must mint for `id` registered in `region`.
fn application_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ssm-sap:{region}:{ACCOUNT}:application/{id}")
}

/// The id this suite registers only in `region`, so a listing or a read naming it proves
/// which Region's store answered.
fn local_id(region: &str) -> String {
    format!("HDB-{}", region.to_uppercase().replace('-', "."))
}

/// What an operation that models no `ResourceNotFoundException` reports when this account
/// and Region hold no registration answering `reference`.
fn validation_refusal_message(reference: &str) -> String {
    format!("No registered application found for account: {ACCOUNT}, and application: {reference}")
}

/// What an application-CONTROL call reports for an application that is not registered in
/// the account and Region it was signed for.
fn control_refusal_message(reference: &str) -> String {
    format!("Application {reference} doesn't exist.")
}

/// Register a minimal HANA application in `region`, asserting the registration itself
/// succeeded, and return the reply so a row can read its ARN and `OperationId`.
async fn register(emulator: &Emulator, region: &str, id: &str) -> Reply {
    let reply = emulator
        .send_in_region(region, "/register-application", valid_registration(id, "HANA"))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {region:?}/{id:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    reply
}

/// `(Id, Arn)` of every `ApplicationSummary` a listing reported, in the order reported.
fn listed(reply: &Reply) -> Vec<(String, String)> {
    reply
        .member("Applications")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let read = |name: &str| {
                        item.get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (read("Id"), read("Arn"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A member of the reply's `Application`, as a string.
fn application_member(reply: &Reply, name: &str) -> String {
    reply
        .member("Application")
        .and_then(|application| application.get(name))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn the_same_application_id_registers_independently_in_every_region() {
    let emulator = Emulator::new();

    // One `ApplicationId`, registered at every regional endpoint: an id is unique within
    // an account AND a Region, so none of these conflicts with the ones before it.
    for region in REGIONS {
        let reply = register(&emulator, region, SHARED_ID).await;
        let minted = application_member(&reply, "Arn");
        assert_eq!(
            minted,
            application_arn(region, SHARED_ID),
            "case {region:?}: expected the ARN of the region the registration was signed \
             for, got {minted:?}"
        );
    }

    // And each Region reports its OWN registration back, ARN included.
    for region in REGIONS {
        let reply = emulator
            .send_in_region(region, "/get-application", json!({"ApplicationId": SHARED_ID}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200 from GetApplication, got {reply:?}"
        );
        let read = application_member(&reply, "Arn");
        assert_eq!(
            read,
            application_arn(region, SHARED_ID),
            "case {region:?}: expected this region's own ARN, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_applications_of_the_region_it_is_signed_for() {
    let emulator = Emulator::new();

    // Each Region gets one id of its own plus the shared one, so a listing that crossed
    // Regions would be caught both by an id it must not hold and by a count.
    for region in REGIONS {
        register(&emulator, region, &local_id(region)).await;
        register(&emulator, region, SHARED_ID).await;
    }

    for region in REGIONS {
        let reply = emulator
            .send_in_region(region, "/list-applications", json!({}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200 from ListApplications, got {reply:?}"
        );

        let mut expected = vec![
            (SHARED_ID.to_string(), application_arn(region, SHARED_ID)),
            (
                local_id(region),
                application_arn(region, &local_id(region)),
            ),
        ];
        expected.sort();
        let mut reported = listed(&reply);
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {region:?}: expected only this region's applications {expected:?}, got \
             {reported:?}"
        );
    }

    // A Region nothing was registered in holds nothing — it does not inherit another
    // Region's applications, and the listing is an empty list rather than a refusal.
    let reply = emulator
        .send_in_region(UNUSED_REGION, "/list-applications", json!({}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {UNUSED_REGION:?}: expected 200 from ListApplications, got {reply:?}"
    );
    assert_eq!(
        listed(&reply),
        Vec::new(),
        "case {UNUSED_REGION:?}: expected an empty listing, got {reply:?}"
    );

    // Filters narrow within the Region; they never widen past it. A filter naming
    // another Region's application — by the id only that Region holds, or by its ARN —
    // matches nothing here.
    for region in REGIONS {
        for other in REGIONS.iter().filter(|other| *other != region) {
            let mut values = vec![("Id", local_id(other))];
            let arn = application_arn(other, SHARED_ID);
            // `FilterValue` is `@length(min: 1, max: 64)`, so an ARN longer than that is
            // a value the API itself refuses: the row is only a filter row where the
            // model admits the value.
            if arn.len() <= FILTER_VALUE_MAX_LENGTH {
                values.push(("Arn", arn));
            }
            for (name, value) in values {
                let reply = emulator
                    .send_in_region(
                        region,
                        "/list-applications",
                        json!({"Filters": [{
                            "Name": name,
                            "Operator": "Equals",
                            "Value": value,
                        }]}),
                    )
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {region:?} filtering {name} on {other:?}: expected 200, got \
                     {reply:?}"
                );
                assert_eq!(
                    listed(&reply),
                    Vec::new(),
                    "case {region:?} filtering {name} on {other:?}: another region's \
                     application matches nothing here, got {reply:?}"
                );
            }
        }
    }
}

/// One operation reaching for an application by a reference, and what it must answer when
/// the Region it is signed for holds no such registration.
struct ForeignCall {
    /// What the case is called in an assertion.
    label: &'static str,
    /// The operation's URI path.
    path: &'static str,
    /// Which Region of [`REGIONS`] this row registers its application in, so the rows
    /// between them put every Region in the home position.
    home: usize,
    /// The modelled status the refusal carries.
    status: u16,
    /// The modelled error code the refusal carries.
    code: &'static str,
    /// Whether the reference the request names is the application's ARN rather than its
    /// id — the ARN embeds the Region, so it cannot resolve in another one either.
    by_arn: bool,
}

/// The request body naming `reference` for `path`.
fn request_for(path: &str, reference: &str, by_arn: bool) -> Value {
    if path == "/get-application" && by_arn {
        json!({"ApplicationArn": reference})
    } else {
        json!({"ApplicationId": reference})
    }
}

#[tokio::test]
async fn a_call_signed_for_another_region_resolves_nothing_and_is_refused() {
    // One row per operation that names an application, each with a different Region in
    // the home position; `GetApplication` appears twice, once per identifier form.
    let cases: &[ForeignCall] = &[
        ForeignCall {
            label: "GetApplication by ApplicationId",
            path: "/get-application",
            home: 0,
            status: 400,
            code: VALIDATION_EXCEPTION,
            by_arn: false,
        },
        ForeignCall {
            label: "GetApplication by ApplicationArn",
            path: "/get-application",
            home: 1,
            status: 400,
            code: VALIDATION_EXCEPTION,
            by_arn: true,
        },
        ForeignCall {
            label: "StartApplication",
            path: "/start-application",
            home: 2,
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            by_arn: false,
        },
        ForeignCall {
            label: "StopApplication",
            path: "/stop-application",
            home: 3,
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            by_arn: false,
        },
        ForeignCall {
            label: "DeregisterApplication",
            path: "/deregister-application",
            home: 0,
            status: 400,
            code: VALIDATION_EXCEPTION,
            by_arn: false,
        },
        ForeignCall {
            // A listing scoped to an application: it models
            // `ResourceNotFoundException`, so an application of another Region is a 404
            // here rather than another Region's components.
            label: "ListComponents",
            path: "/list-components",
            home: 1,
            status: 404,
            code: RESOURCE_NOT_FOUND_EXCEPTION,
            by_arn: false,
        },
    ];

    for case in cases {
        let home = REGIONS[case.home];
        let id = local_id(home);
        let emulator = Emulator::new();
        register(&emulator, home, &id).await;
        // So a control call at the home region is not refused for a change still owed.
        settle().await;

        let reference = if case.by_arn {
            application_arn(home, &id)
        } else {
            id.clone()
        };
        let body = request_for(case.path, &reference, case.by_arn);

        // Every OTHER regional endpoint, and one nothing was ever registered at, resolves
        // nothing for the reference.
        let foreign: Vec<&str> = REGIONS
            .iter()
            .copied()
            .filter(|region| *region != home)
            .chain(std::iter::once(UNUSED_REGION))
            .collect();
        for region in foreign {
            let label = format!("{} at {region:?} (registered in {home:?})", case.label);
            let reply = emulator
                .send_in_region(region, case.path, body.clone())
                .await;
            assert_eq!(
                reply.status, case.status,
                "case {label}: expected HTTP {}, got {reply:?}",
                case.status
            );
            assert_eq!(
                reply.code(),
                case.code,
                "case {label}: expected {}, got {reply:?}",
                case.code
            );
            let expected = if case.code == RESOURCE_NOT_FOUND_EXCEPTION {
                control_refusal_message(&reference)
            } else {
                validation_refusal_message(&reference)
            };
            let stated = message(&reply);
            assert_eq!(
                stated, expected,
                "case {label}: expected {expected:?}, got {stated:?}"
            );
        }

        // The refusal is about the Region, not about the request: the SAME call at the
        // home endpoint succeeds.
        let reply = emulator.send_in_region(home, case.path, body).await;
        assert_eq!(
            reply.status, 200,
            "case {} at its own region {home:?}: expected 200, got {reply:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_foreign_region_neither_mutates_nor_hides_an_application() {
    let emulator = Emulator::new();
    let home = REGIONS[0];
    let elsewhere = REGIONS[1];
    let id = local_id(home);
    register(&emulator, home, &id).await;
    settle().await;

    // Every call another Region could make against the application.
    for path in [
        "/start-application",
        "/stop-application",
        "/deregister-application",
    ] {
        let reply = emulator
            .send_in_region(elsewhere, path, json!({"ApplicationId": id}))
            .await;
        assert!(
            reply.status >= 400,
            "case {path:?} at {elsewhere:?}: expected a refusal, got {reply:?}"
        );
    }

    // None of them touched the application: it is still registered at home, still
    // ACTIVATED, and still the only one there.
    let reply = emulator
        .send_in_region(home, "/get-application", json!({"ApplicationId": id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {home:?}: expected 200 from GetApplication, got {reply:?}"
    );
    let status = application_member(&reply, "Status");
    assert_eq!(
        status, "ACTIVATED",
        "case {home:?}: a refused foreign-region call must not move the status, got \
         {status:?}"
    );

    // Deregistering AT HOME removes it there and nowhere else: the shared id registered
    // in another Region is untouched.
    register(&emulator, elsewhere, SHARED_ID).await;
    let reply = emulator
        .send_in_region(home, "/deregister-application", json!({"ApplicationId": id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {home:?}: expected 200 from DeregisterApplication, got {reply:?}"
    );
    // Deregistration is asynchronous: the application is listed as DELETING at home until
    // the removal the clock owes it falls due.
    settle().await;
    let reply = emulator
        .send_in_region(home, "/list-applications", json!({}))
        .await;
    assert_eq!(
        listed(&reply),
        Vec::new(),
        "case {home:?}: the deregistered application is gone, got {reply:?}"
    );
    let reply = emulator
        .send_in_region(elsewhere, "/list-applications", json!({}))
        .await;
    assert_eq!(
        listed(&reply),
        vec![(
            SHARED_ID.to_string(),
            application_arn(elsewhere, SHARED_ID)
        )],
        "case {elsewhere:?}: another region's deregistration changes nothing here, got \
         {reply:?}"
    );

    // And the id is free again in its own Region, so a Region whose last application went
    // away still serves registrations.
    let reply = register(&emulator, home, &id).await;
    let minted = application_member(&reply, "Arn");
    assert_eq!(
        minted,
        application_arn(home, &id),
        "case {home:?}: expected the home region's ARN after re-registering, got {minted:?}"
    );
}

#[tokio::test]
async fn an_operation_record_is_readable_only_in_the_region_that_filed_it() {
    let emulator = Emulator::new();

    // One operation record per Region, from that Region's own registration.
    let mut records: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let reply = register(&emulator, region, SHARED_ID).await;
        let operation_id = reply
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !operation_id.is_empty(),
            "case {region:?}: RegisterApplication reports an OperationId, got {reply:?}"
        );
        records.push((region, operation_id));
    }

    for (region, operation_id) in &records {
        let body = json!({"OperationId": operation_id});

        let reply = emulator
            .send_in_region(region, "/get-operation", body.clone())
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200 from GetOperation at the region that filed the \
             record, got {reply:?}"
        );

        for other in REGIONS
            .iter()
            .copied()
            .filter(|other| other != region)
            .chain(std::iter::once(UNUSED_REGION))
        {
            let reply = emulator
                .send_in_region(other, "/get-operation", body.clone())
                .await;
            assert_eq!(
                reply.status, 400,
                "case {operation_id:?} (filed in {region:?}) read at {other:?}: expected \
                 400, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                VALIDATION_EXCEPTION,
                "case {operation_id:?} read at {other:?}: expected \
                 {VALIDATION_EXCEPTION}, got {reply:?}"
            );
            let stated = message(&reply);
            let expected = no_operation_message(operation_id);
            assert_eq!(
                stated, expected,
                "case {operation_id:?} read at {other:?}: expected {expected:?}, got \
                 {stated:?}"
            );
        }
    }
}
