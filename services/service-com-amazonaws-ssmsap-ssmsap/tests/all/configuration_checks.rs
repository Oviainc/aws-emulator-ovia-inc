// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the configuration check slice, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary.
//! Nothing reaches into `crate::state` directly: a `StartConfigurationChecks` call is
//! only believed when a later `GetConfigurationCheckOperation` reports the same record
//! back.
//!
//! RULE under test: `StartConfigurationChecks` files ONE configuration check operation
//! per distinct check the request asks for — the whole modelled
//! `ConfigurationCheckType` catalogue when it asks for none — against the application
//! it names, each with its own fresh `OperationId`, the check it was started for and
//! the `INPROGRESS` status, readable afterwards by `GetConfigurationCheckOperation`
//! until the clock brings it to its terminal status; an application this account has
//! not registered is refused, and a check already running against an application is a
//! conflict.
//!
//! The rows span the shapes a request's `ConfigurationCheckIds` can take (absent,
//! empty, each single catalogue value, several, repeated) rather than one recorded
//! call, and the catalogue they are read from is the model's own enum, so a check added
//! to the model is exercised here without this file changing.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::CONFIGURATION_CHECK_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    application_not_found_message, configuration_check_entry,
    MAX_CONFIGURATION_CHECKS_PER_REQUEST, TRANSITION_SPAN,
};

use crate::application_lifecycle::{is_operation_id, valid_registration, Emulator, Reply};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The modelled error code a start against an unregistered application is answered
/// with (`smithy_query(query="operation", operation="StartConfigurationChecks")`:
/// `ResourceNotFoundException` @httpError 404).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The modelled error code a start of a check that is already running is answered with
/// (same query: `ConflictException` @httpError 409).
const CONFLICT: &str = "ConflictException";

/// `OperationStatus$INPROGRESS`, as the model spells it.
const INPROGRESS: &str = "INPROGRESS";

/// `OperationStatus$SUCCESS`, as the model spells it — the terminal status a check whose
/// rules were evaluated ends in.
const SUCCESS: &str = "SUCCESS";

/// Wait out the terminal status the clock owes a freshly started check: comfortably
/// longer than [`TRANSITION_SPAN`]. The emulator never sleeps; this is the test waiting
/// for the clock, which is the only thing that can make the transition due.
///
/// `pub(crate)` so `tests/all/configuration_check_reads.rs`, which pins what a READ of
/// one of these records may do, waits for the clock the same way.
pub(crate) async fn settle() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(300)).await;
}

/// Every check in the model's catalogue, as owned strings.
fn whole_catalogue() -> Vec<String> {
    CONFIGURATION_CHECK_TYPE_VALUES
        .iter()
        .map(|check| (*check).to_string())
        .collect()
}

/// The `ConfigurationCheckOperations` of a `StartConfigurationChecks` reply.
///
/// `pub(crate)` for the same reason as [`settle`]: one reading of a start's reply for
/// every suite of this target.
pub(crate) fn started(reply: &Reply) -> Vec<Value> {
    reply
        .member("ConfigurationCheckOperations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `ConfigurationCheckOperation` a `GetConfigurationCheckOperation` reply reports,
/// `Value::Null` when it reports none.
///
/// `pub(crate)` for the same reason as [`started`]: one reading of a read's reply for
/// every suite of this target.
pub(crate) fn read_record(reply: &Reply) -> Value {
    reply
        .member("ConfigurationCheckOperation")
        .cloned()
        .unwrap_or(Value::Null)
}

/// A member of one `ConfigurationCheckOperation`, treating JSON `null` as absent.
pub(crate) fn field<'a>(operation: &'a Value, name: &str) -> Option<&'a Value> {
    operation.get(name).filter(|value| !value.is_null())
}

pub(crate) fn string_field(operation: &Value, name: &str) -> String {
    field(operation, name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl Emulator {
    /// Register a HANA application and wait out its registration, so a check is started
    /// against an application that is fully `ACTIVATED`.
    pub(crate) async fn with_application(&self, id: &str) {
        let reply = self.send("/register-application", valid_registration(id, "HANA")).await;
        assert_eq!(
            reply.status, 200,
            "case {id:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        settle().await;
    }

    pub(crate) async fn start_checks(&self, body: Value) -> Reply {
        self.send("/start-configuration-checks", body).await
    }

    pub(crate) async fn get_check(&self, id: &str) -> Reply {
        self.send(
            "/get-configuration-check-operation",
            json!({"OperationId": id}),
        )
        .await
    }
}

/// One `StartConfigurationChecks` request: a label, the `ConfigurationCheckIds` it
/// carries (`None` = the member is absent), and the checks it must start, in order.
struct StartCase {
    label: &'static str,
    requested: Option<Vec<String>>,
    expected: Vec<String>,
}

/// The rows: every shape `ConfigurationCheckIds` can take. Every check named is read
/// off the model's own catalogue rather than written out, so a change to the
/// `ConfigurationCheckType` enum keeps the rows testing the rule instead of failing
/// boundary validation.
fn start_cases() -> Vec<StartCase> {
    let catalogue = whole_catalogue();
    let mut cases = vec![
        StartCase {
            label: "no ConfigurationCheckIds member at all",
            requested: None,
            expected: catalogue.clone(),
        },
        StartCase {
            label: "an empty ConfigurationCheckIds list",
            requested: Some(vec![]),
            expected: catalogue.clone(),
        },
    ];
    // Two checks, asked for in the reverse of the catalogue's order, so the response's
    // order is the request's rather than the enum's.
    if let (Some(first), Some(second)) = (catalogue.first(), catalogue.get(1)) {
        cases.push(StartCase {
            label: "two checks, in the order asked for",
            requested: Some(vec![second.clone(), first.clone()]),
            expected: vec![second.clone(), first.clone()],
        });
    }
    // The same check named twice is one check to perform.
    if let Some(last) = catalogue.last() {
        cases.push(StartCase {
            label: "the same check named twice is one check to perform",
            requested: Some(vec![last.clone(), last.clone()]),
            expected: vec![last.clone()],
        });
    }
    for check in &catalogue {
        cases.push(StartCase {
            label: "a single check from the modelled catalogue",
            requested: Some(vec![check.clone()]),
            expected: vec![check.clone()],
        });
    }
    cases
}

#[tokio::test]
async fn a_start_files_one_readable_operation_per_requested_check() {
    for case in start_cases() {
        let StartCase {
            label,
            requested,
            expected,
        } = case;
        let label = format!("{label} ({requested:?})");
        // One emulator per row: the rows are independent requests, not a sequence.
        let emulator = Emulator::new();
        emulator.with_application("HDB-TEST").await;

        let mut body = json!({"ApplicationId": "HDB-TEST"});
        if let Some(checks) = requested.as_ref() {
            body.as_object_mut()
                .expect("the body is a JSON object")
                .insert("ConfigurationCheckIds".to_string(), json!(checks));
        }
        let reply = emulator.start_checks(body).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200, got {}: {:?}",
            reply.status, reply.body
        );

        let operations = started(&reply);
        assert_eq!(
            operations
                .iter()
                .map(|operation| string_field(operation, "ConfigurationCheckId"))
                .collect::<Vec<String>>(),
            expected,
            "case {label:?}: expected one operation per requested check, got {operations:?}"
        );

        let mut ids: Vec<String> = Vec::new();
        for operation in &operations {
            let id = string_field(operation, "Id");
            assert!(
                is_operation_id(&id),
                "case {label:?}: expected a modelled OperationId, got {id:?}"
            );
            ids.push(id);
            assert_eq!(
                string_field(operation, "ApplicationId"),
                "HDB-TEST",
                "case {label:?}: expected the application the request named, got {operation:?}"
            );
            assert_eq!(
                string_field(operation, "Status"),
                INPROGRESS,
                "case {label:?}: a freshly started check is still running, got {operation:?}"
            );
            assert!(
                field(operation, "StartTime").is_some(),
                "case {label:?}: expected a StartTime, got {operation:?}"
            );
            // Nothing has ended and no rule has run yet, so neither member is reported.
            for absent in ["EndTime", "StatusMessage", "RuleStatusCounts"] {
                assert!(
                    field(operation, absent).is_none(),
                    "case {label:?}: expected no {absent} while the check runs, got {operation:?}"
                );
            }
        }
        assert_eq!(
            ids.iter().collect::<BTreeSet<&String>>().len(),
            ids.len(),
            "case {label:?}: every started check must get its own id, got {ids:?}"
        );

        // ROUND TRIP: each id names a record the reader reports identically.
        for (operation, id) in operations.iter().zip(&ids) {
            let fetched = emulator.get_check(id).await;
            assert_eq!(
                fetched.status, 200,
                "case {label:?}: expected 200 from GetConfigurationCheckOperation for {id:?}, \
                 got {}: {:?}",
                fetched.status, fetched.body
            );
            let read_back = read_record(&fetched);
            for member in ["Id", "ApplicationId", "ConfigurationCheckId", "Status"] {
                assert_eq!(
                    string_field(&read_back, member),
                    string_field(operation, member),
                    "case {label:?}: {member} must read back as it was reported, got {read_back:?}"
                );
            }
        }

        // And the terminal status the clock owes falls due: the check ends in SUCCESS,
        // with the tally of the rule results its check's rules produced and nothing to
        // explain.
        settle().await;
        for (operation, id) in operations.iter().zip(&ids) {
            let check = string_field(operation, "ConfigurationCheckId");
            let catalogued = configuration_check_entry(&check)
                .unwrap_or_else(|| panic!("case {label:?}: the service catalogues {check:?}"));
            let fetched = emulator.get_check(id).await;
            let done = read_record(&fetched);
            assert_eq!(
                string_field(&done, "Status"),
                SUCCESS,
                "case {label:?}: expected the terminal status once due, got {done:?}"
            );
            assert!(
                field(&done, "StatusMessage").is_none(),
                "case {label:?}: a check whose rules ran has nothing to explain, got {done:?}"
            );
            let start = field(&done, "StartTime")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            let end = field(&done, "EndTime")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {label:?}: expected an EndTime, got {done:?}"));
            assert!(
                end >= start,
                "case {label:?}: expected EndTime at or after StartTime, got {done:?}"
            );
            // The counts are the tally of the rule results the run produced, so they add
            // up to the number of rules the catalogued check evaluates.
            let counts = field(&done, "RuleStatusCounts").unwrap_or_else(|| {
                panic!("case {label:?}: expected RuleStatusCounts once the rules ran, got {done:?}")
            });
            let total: i64 = ["Passed", "Failed", "Warning", "Info", "Unknown"]
                .iter()
                .map(|bucket| {
                    counts
                        .get(bucket)
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| {
                            panic!(
                                "case {label:?}: expected a {bucket} count, got {counts:?}"
                            )
                        })
                })
                .sum();
            assert_eq!(
                total,
                catalogued.rule_count() as i64,
                "case {label:?}: the counts must tally every rule of {check:?}, got {counts:?}"
            );
            // The record names the check it performed with the catalogue's own prose.
            assert_eq!(
                string_field(&done, "ConfigurationCheckName"),
                catalogued.name,
                "case {label:?}: expected the catalogued name of {check:?}, got {done:?}"
            );
            assert_eq!(
                string_field(&done, "ConfigurationCheckDescription"),
                catalogued.description,
                "case {label:?}: expected the catalogued description of {check:?}, got {done:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_check_still_running_cannot_be_started_again_until_it_ends() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-TEST").await;
    emulator.with_application("other-app").await;

    // The checks are read off the model's own catalogue rather than written out: the one
    // that runs, another to show the conflict is scoped to the (application, check)
    // pair, and a third that a refused request names alongside the running one. A row
    // the catalogue is too small to supply a distinct check for is skipped.
    let catalogue = whole_catalogue();
    let running = catalogue
        .first()
        .cloned()
        .expect("the model's ConfigurationCheckType catalogue holds at least one check");
    let second = catalogue.get(1).cloned();
    let third = catalogue.get(2).cloned();

    let first = emulator
        .start_checks(json!({
            "ApplicationId": "HDB-TEST",
            "ConfigurationCheckIds": [running.clone()],
        }))
        .await;
    assert_eq!(first.status, 200, "expected 200, got {first:?}");
    let running_id = string_field(&started(&first)[0], "Id");

    // The conflict is scoped to the check AND the application: a different check, and
    // the same check on another application, are accepted while the first runs.
    let mut accepted = vec![(
        "the same check on another application",
        json!({"ApplicationId": "other-app", "ConfigurationCheckIds": [running.clone()]}),
    )];
    if let Some(second) = second.as_ref() {
        accepted.push((
            "another check on the same application",
            json!({"ApplicationId": "HDB-TEST", "ConfigurationCheckIds": [second.clone()]}),
        ));
    }
    for (label, body) in accepted {
        let reply = emulator.start_checks(body).await;
        assert_eq!(
            reply.status, 200,
            "case {label:?}: expected 200, got {}: {:?}",
            reply.status, reply.body
        );
    }

    // Re-starting the running check is refused — alone, and as part of a request that
    // also names a check that is not running, which must leave nothing behind.
    let mut refused = vec![("the running check alone", vec![running.clone()])];
    if let Some(third) = third.as_ref() {
        refused.push((
            "a request that also names a free check",
            vec![third.clone(), running.clone()],
        ));
    }
    for (label, checks) in refused {
        let reply = emulator
            .start_checks(json!({
                "ApplicationId": "HDB-TEST",
                "ConfigurationCheckIds": checks,
            }))
            .await;
        assert_eq!(
            reply.status, 409,
            "case {label:?}: expected 409, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            CONFLICT,
            "case {label:?}: expected {CONFLICT}, got {reply:?}"
        );
        assert!(
            message(&reply).contains(&running),
            "case {label:?}: the refusal must name the check that is running, got {:?}",
            message(&reply)
        );
    }

    // The refused request wrote nothing: the free check it also named was never
    // started, so it starts cleanly once the running check has ended.
    settle().await;
    let done = emulator.get_check(&running_id).await;
    assert_eq!(
        string_field(&read_record(&done), "Status"),
        SUCCESS,
        "expected the first check to have ended, got {done:?}"
    );

    let restarted: Vec<String> = match third.as_ref() {
        Some(third) => vec![running.clone(), third.clone()],
        None => vec![running.clone()],
    };
    let again = emulator
        .start_checks(json!({
            "ApplicationId": "HDB-TEST",
            "ConfigurationCheckIds": restarted.clone(),
        }))
        .await;
    assert_eq!(
        again.status, 200,
        "a check that has ended can be started again, got {again:?}"
    );
    let new_ids: Vec<String> = started(&again)
        .iter()
        .map(|operation| string_field(operation, "Id"))
        .collect();
    assert_eq!(
        new_ids.len(),
        restarted.len(),
        "expected one operation per requested check, got {new_ids:?}"
    );
    assert!(
        !new_ids.contains(&running_id),
        "a re-started check is a NEW operation record, got {new_ids:?}"
    );
}

#[tokio::test]
async fn starting_checks_against_an_unregistered_application_is_not_found() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-TEST").await;

    // Rows: an id nothing registered, the registered id in another case (ids are
    // compared exactly), and an id in the recorded probe's form.
    for id in [
        "NO-SUCH",
        "hdb-test",
        "sim2real-probe-ssm-sap-app-8d37e33f",
    ] {
        let reply = emulator
            .start_checks(json!({"ApplicationId": id}))
            .await;
        assert_eq!(
            reply.status, 404,
            "case {id:?}: expected 404, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            RESOURCE_NOT_FOUND,
            "case {id:?}: expected {RESOURCE_NOT_FOUND}, got {reply:?}"
        );
        let expected = application_not_found_message(id);
        let actual = message(&reply);
        assert_eq!(
            actual, expected,
            "case {id:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_check_outside_the_modelled_catalogue_is_refused_and_starts_nothing() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-TEST").await;

    for checks in [
        vec!["SAP_CHECK_99".to_string()],
        vec!["sap_check_01".to_string()],
        vec![
            whole_catalogue()
                .first()
                .cloned()
                .expect("the catalogue holds at least one check"),
            "NOT_A_CHECK".to_string(),
        ],
    ] {
        let reply = emulator
            .start_checks(json!({
                "ApplicationId": "HDB-TEST",
                "ConfigurationCheckIds": checks.clone(),
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {checks:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {checks:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }

    // Nothing was started, so the checks the refused requests named are still free.
    let reply = emulator
        .start_checks(json!({"ApplicationId": "HDB-TEST"}))
        .await;
    assert_eq!(
        reply.status, 200,
        "a refused request must leave every check free to start, got {reply:?}"
    );
    assert_eq!(
        started(&reply).len(),
        CONFIGURATION_CHECK_TYPE_VALUES.len(),
        "expected the whole catalogue to start, got {reply:?}"
    );
}

#[tokio::test]
async fn a_repeated_check_is_one_check_and_a_list_past_the_ceiling_starts_nothing() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-TEST").await;

    let catalogue = whole_catalogue();
    let repeat = catalogue
        .first()
        .cloned()
        .expect("the catalogue holds at least one check");

    // A check named twice in one request is ONE check to perform, so a list of repeats
    // — including one filling the whole ceiling — is accepted and starts one operation
    // per DISTINCT check, not one per entry.
    for entries in [
        catalogue.len() + 1,
        2 * catalogue.len(),
        MAX_CONFIGURATION_CHECKS_PER_REQUEST,
    ] {
        let emulator = Emulator::new();
        emulator.with_application("HDB-TEST").await;
        let checks: Vec<String> = catalogue
            .iter()
            .cloned()
            .cycle()
            .take(entries)
            .collect();
        let reply = emulator
            .start_checks(json!({
                "ApplicationId": "HDB-TEST",
                "ConfigurationCheckIds": checks,
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {entries} entries: a list of repeats is accepted, got {reply:?}"
        );
        assert_eq!(
            started(&reply).len(),
            catalogue.len().min(entries),
            "case {entries} entries: expected one operation per distinct check, got \
             {reply:?}"
        );
    }

    // Past the ceiling the request is refused, and nothing is started: the checks the
    // refused request named are still free afterwards.
    for over in [1usize, 2, 50] {
        let checks: Vec<String> = std::iter::repeat_n(
            repeat.clone(),
            MAX_CONFIGURATION_CHECKS_PER_REQUEST + over,
        )
        .collect();
        let reply = emulator
            .start_checks(json!({
                "ApplicationId": "HDB-TEST",
                "ConfigurationCheckIds": checks,
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {over} past the ceiling: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {over} past the ceiling: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
    }

    let reply = emulator
        .start_checks(json!({
            "ApplicationId": "HDB-TEST",
            "ConfigurationCheckIds": catalogue.clone(),
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "a refused request must leave every check free to start, got {reply:?}"
    );
    assert_eq!(
        started(&reply).len(),
        catalogue.len(),
        "expected one operation per named check, got {reply:?}"
    );
}

#[tokio::test]
async fn an_operation_id_that_names_no_configuration_check_is_refused() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-TEST").await;

    // Well-formed ids (the model's `OperationId` pattern) that name no configuration
    // check operation: all-zero, mixed case, and the braced form the pattern allows.
    for id in [
        "00000000-0000-0000-0000-000000000000",
        "DEADBEEF-dead-BEEF-dead-beefDEADbeef",
        "{11111111-2222-3333-4444-555555555555}",
    ] {
        let reply = emulator.get_check(id).await;
        assert_eq!(
            reply.status, 400,
            "case {id:?}: expected 400, got {}: {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {id:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert!(
            message(&reply).contains(id),
            "case {id:?}: the refusal must name the id it could not resolve, got {:?}",
            message(&reply)
        );
    }

    // An application operation and a configuration check operation are separate
    // resources: the id of one never resolves through the other's reader.
    let application_operation = emulator
        .send(
            "/start-application",
            json!({"ApplicationId": "HDB-TEST"}),
        )
        .await;
    let operation_id = application_operation
        .member("OperationId")
        .and_then(Value::as_str)
        .expect("StartApplication reports an OperationId")
        .to_string();
    let crossed = emulator.get_check(&operation_id).await;
    assert_eq!(
        crossed.status, 400,
        "an application OperationId is not a configuration check operation, got {crossed:?}"
    );
    assert_eq!(
        crossed.code(),
        VALIDATION_EXCEPTION,
        "expected {VALIDATION_EXCEPTION} for an application OperationId, got {crossed:?}"
    );
    assert!(
        message(&crossed).contains(&operation_id),
        "the refusal must name the id it could not resolve, got {:?}",
        message(&crossed)
    );

    let check = emulator
        .start_checks(json!({
            "ApplicationId": "HDB-TEST",
            "ConfigurationCheckIds": [whole_catalogue()
                .first()
                .cloned()
                .expect("the catalogue holds at least one check")],
        }))
        .await;
    let check_id = string_field(&started(&check)[0], "Id");
    let crossed_back = emulator
        .send("/get-operation", json!({"OperationId": check_id.clone()}))
        .await;
    assert_eq!(
        crossed_back.status, 400,
        "a configuration check id is not an application operation, got {crossed_back:?}"
    );
    assert_eq!(
        crossed_back.code(),
        VALIDATION_EXCEPTION,
        "expected {VALIDATION_EXCEPTION} for a configuration check id, got {crossed_back:?}"
    );
    assert!(
        message(&crossed_back).contains(&check_id),
        "the refusal must name the id it could not resolve, got {:?}",
        message(&crossed_back)
    );
}

#[tokio::test]
async fn deregistering_an_application_forgets_its_configuration_checks() {
    let emulator = Emulator::new();
    emulator.with_application("HDB-TEST").await;
    emulator.with_application("survivor").await;

    let mut ids: Vec<(&str, String)> = Vec::new();
    for application in ["HDB-TEST", "survivor"] {
        let reply = emulator
            .start_checks(json!({"ApplicationId": application}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {application:?}: expected 200, got {reply:?}"
        );
        for operation in started(&reply) {
            ids.push((application, string_field(&operation, "Id")));
        }
    }

    let deregistered = emulator
        .send(
            "/deregister-application",
            json!({"ApplicationId": "HDB-TEST"}),
        )
        .await;
    assert_eq!(
        deregistered.status, 200,
        "expected 200 from DeregisterApplication, got {deregistered:?}"
    );

    // While the deregistration is still running the application exists, so its check
    // runs still resolve: they are forgotten when the removal the clock owes falls due.
    for (application, id) in &ids {
        let reply = emulator.get_check(id).await;
        assert_eq!(
            reply.status, 200,
            "case {application:?}/{id:?}: a run still resolves while the application is \
             DELETING, got {}: {:?}",
            reply.status, reply.body
        );
    }
    settle().await;

    for (application, id) in &ids {
        let reply = emulator.get_check(id).await;
        let expected = if *application == "HDB-TEST" { 400 } else { 200 };
        assert_eq!(
            reply.status, expected,
            "case {application:?}/{id:?}: expected {expected}, got {}: {:?}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn a_configuration_check_belongs_to_the_region_it_was_started_in() {
    // The service is regional (see `state::RegionState`), so a check runs against an
    // application of the endpoint's own Region and is read back only there. The rows
    // span a Region the application is NOT registered in and the one it is.
    let emulator = Emulator::new();
    emulator.with_application("HDB-TEST").await;

    let elsewhere = "eu-west-1";
    let refused = emulator
        .send_in_region(
            elsewhere,
            "/start-configuration-checks",
            json!({"ApplicationId": "HDB-TEST"}),
        )
        .await;
    assert_eq!(
        refused.status, 404,
        "case {elsewhere:?}: an application of another Region is not reachable here, got {refused:?}"
    );

    let started_here = emulator
        .start_checks(json!({"ApplicationId": "HDB-TEST"}))
        .await;
    assert_eq!(
        started_here.status, 200,
        "expected 200 at the application's own endpoint, got {started_here:?}"
    );
    for operation in started(&started_here) {
        let id = string_field(&operation, "Id");
        let crossed = emulator
            .send_in_region(
                elsewhere,
                "/get-configuration-check-operation",
                json!({"OperationId": id}),
            )
            .await;
        assert_eq!(
            crossed.status, 400,
            "case {id:?}: a check of another Region does not resolve at {elsewhere:?}, got {crossed:?}"
        );
        let here = emulator.get_check(&id).await;
        assert_eq!(
            here.status, 200,
            "case {id:?}: the check resolves at the Region it was started in, got {here:?}"
        );
    }
}
