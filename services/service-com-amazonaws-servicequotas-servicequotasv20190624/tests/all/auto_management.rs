// SPDX-License-Identifier: CC0-1.0

//! Starting, updating and stopping Service Quotas Automatic Management, end to end
//! over the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine — so what these tests exercise is the
//! HTTP behaviour and the persisted state behind it, not a handler called directly.
//! The harness (`emulator`, `client`, `Client`, `ACCOUNT`) is `quota_workflow.rs`'s,
//! so every suite in this crate speaks to the emulator the same way.
//!
//! This suite owns the WRITE side; `auto_management_configuration.rs` owns how a
//! stored configuration is reported (in particular the `QuotaName` each excluded
//! `QuotaCode` is paired with), so the assertions here read the configuration back
//! through `GetAutoManagementConfiguration` and compare what was STORED.
//!
//! The rules it asserts, each over a TABLE of inputs rather than one case:
//!
//! * starting records the WHOLE configuration the call carries and sets
//!   `OptInStatus = ENABLED`;
//! * an exclusion list that RESOLVES against the quota catalog is stored as given —
//!   each service's codes in the order the caller named them — and every entry is
//!   reported with both its `QuotaCode` and its `QuotaName`; the refusal of one that
//!   does not resolve is `auto_management_exclusions.rs`'s rule;
//! * updating overwrites only the members the update call names, and an exclusion list
//!   it does name replaces the stored one outright;
//! * an `OptInType` is stored as given whichever member of the enum it names, by BOTH
//!   writers of the member, and switching the mode leaves `OptInStatus` as it was;
//! * stopping removes the whole configuration, so the read afterwards answers exactly
//!   what an account that never opted in is answered, and the calls that need Automatic
//!   Management RUNNING are refused with the modelled `NoSuchResourceException`;
//! * the configuration is per ACCOUNT, not per region: a start or an update made at
//!   one regional endpoint is what every endpoint reports;
//! * a request that breaks a modelled constraint records nothing.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::ops::types::{
    OPT_IN_LEVEL_VALUES, OPT_IN_TYPE_VALUES,
};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    OPT_IN_STATUS_DISABLED, OPT_IN_STATUS_ENABLED,
};

use crate::quota_workflow::{client, emulator, Client, ACCOUNT};

/// The only `OptInLevel` member the model declares, taken from the model's own value
/// list rather than spelled again here.
const ACCOUNT_LEVEL: &str = OPT_IN_LEVEL_VALUES[0];

/// The operation that reads a configuration back.
const GET_CONFIG: &str = "GetAutoManagementConfiguration";

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see `scaffolded::wire_http_status` and the
/// `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// A User Notifications ARN of this emulator's account, which satisfies the modeled
/// `AmazonResourceName` pattern `^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$`. It is the
/// CALLER's own ARN, so the test builds it rather than the emulator minting it.
fn notification_arn(name: &str) -> String {
    format!("arn:aws:notifications::{ACCOUNT}:configuration/{name}")
}

/// The `OptInType` member `name`, checked against the model's own value list, so a row
/// cannot ask for a type the model does not declare.
///
/// `pub(crate)` so `auto_management_opt_in_type.rs` — which owns the refusal of a value
/// OUTSIDE that list — shares this one reader of the member instead of growing a copy.
pub(crate) fn opt_in_type(name: &str) -> &'static str {
    OPT_IN_TYPE_VALUES
        .iter()
        .copied()
        .find(|member| *member == name)
        .unwrap_or_else(|| {
            panic!(
                "case {name}: not an OptInType member; the model declares \
                 {OPT_IN_TYPE_VALUES:?}"
            )
        })
}

/// A declared `OptInType` member OTHER than `name`, so an update of the member has
/// something to change; `name` itself if the enum ever declares only one member.
fn another_opt_in_type(name: &str) -> &'static str {
    OPT_IN_TYPE_VALUES
        .iter()
        .copied()
        .find(|member| *member != name)
        .unwrap_or_else(|| opt_in_type(name))
}

/// The scalar members of a reported configuration, as `(OptInLevel, OptInType,
/// OptInStatus, NotificationArn)` — each `None` when the member is absent.
///
/// `pub(crate)` so the suites that assert what a write left on record read it the same
/// way (see `auto_management_opt_in_type.rs`).
pub(crate) fn scalars(body: &Value) -> (Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
    let member = |name: &str| body.get(name).and_then(Value::as_str);
    (
        member("OptInLevel"),
        member("OptInType"),
        member("OptInStatus"),
        member("NotificationArn"),
    )
}

/// The excluded `QuotaCode`s of a reported configuration, per service and in reported
/// order: the projection of the exclusion list this suite asserts on, since pairing a
/// code with its catalog `QuotaName` is `auto_management_configuration.rs`'s rule.
///
/// `None` when the read reported no `ExclusionList` at all.
fn exclusion_codes(body: &Value) -> Option<BTreeMap<String, Vec<String>>> {
    let reported = body.get("ExclusionList")?.as_object()?;
    Some(
        reported
            .iter()
            .map(|(service_code, entries)| {
                let codes = entries
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|entry| {
                        entry
                            .get("QuotaCode")
                            .and_then(Value::as_str)
                            .unwrap_or("<no QuotaCode>")
                            .to_string()
                    })
                    .collect();
                (service_code.clone(), codes)
            })
            .collect(),
    )
}

/// The exclusion list a row expects, in the same shape [`exclusion_codes`] reports.
fn expected_codes(exclusions: &[(&str, &[&str])]) -> Option<BTreeMap<String, Vec<String>>> {
    if exclusions.is_empty() {
        return None;
    }
    Some(
        exclusions
            .iter()
            .map(|(service_code, codes)| {
                (
                    (*service_code).to_string(),
                    codes.iter().map(|code| (*code).to_string()).collect(),
                )
            })
            .collect(),
    )
}

/// Assert that every entry of a reported exclusion list carries BOTH members a
/// `QuotaInfo` pairs: the excluded `QuotaCode` and the `QuotaName` the catalog holds
/// for it. Every stored exclusion names a quota the catalog holds — a write refuses one
/// that does not — so a half-populated entry is a defect.
fn assert_every_entry_is_named(case: &str, body: &Value) {
    let Some(reported) = body.get("ExclusionList").and_then(Value::as_object) else {
        return;
    };
    for (service_code, entries) in reported {
        for entry in entries.as_array().map(Vec::as_slice).unwrap_or_default() {
            let member = |name: &str| entry.get(name).and_then(Value::as_str);
            assert!(
                member("QuotaCode").is_some() && member("QuotaName").is_some(),
                "case {case}: every excluded quota of {service_code} must be reported with \
                 both its QuotaCode and its QuotaName, got {entry:?}"
            );
        }
    }
}

/// Read the configuration back, which must succeed: the read reports
/// `OptInStatus: DISABLED` rather than failing even when nothing was ever started.
pub(crate) async fn configuration(client: &Client, case: &str) -> Value {
    let (status, body) = client.call(GET_CONFIG, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case}: expected {GET_CONFIG} to answer HTTP 200, got {status} with {body}"
    );
    body
}

/// Dispatch a call that must be refused, and return its status, error code and the
/// message member that says what was wrong. `quota_workflow`'s own helper reports
/// only the status and the code.
///
/// The modelled errors carry their own `Message` member, while the synthetic
/// `ValidationException` the request-validation front end answers with carries the
/// protocol's generic lowercase `message`; a client reads whichever its SDK maps, so
/// this reads both.
pub(crate) async fn refused(
    client: &Client,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, parsed) = client.call(operation, body).await;
    let member = |name: &str| {
        parsed
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    };
    let message = parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string();
    (status, member("__type"), message)
}

/// One row of the start table.
struct StartCase {
    case: &'static str,
    opt_in_type: &'static str,
    notification: Option<&'static str>,
    exclusions: &'static [(&'static str, &'static [&'static str])],
}

/// Starting records the WHOLE configuration the call carries, and the read reports it
/// as `ENABLED`.
///
/// The rows differ along everything the rule generalises over: a notification ARN
/// present or absent, and an exclusion list absent, naming one service or several,
/// carrying one quota code or several, carrying the same code twice, and naming a
/// service with no quota codes at all. The opt-in type is spanned by
/// [`every_modeled_opt_in_type_is_stored_by_both_writers`] over every member the enum
/// declares.
#[tokio::test]
async fn starting_records_the_whole_configuration_and_the_read_reports_it() {
    let rows = [
        StartCase {
            case: "notify only, no ARN, no exclusions",
            opt_in_type: "NotifyOnly",
            notification: None,
            exclusions: &[],
        },
        StartCase {
            case: "notify only, with a notification ARN",
            opt_in_type: "NotifyOnly",
            notification: Some("quota-alerts"),
            exclusions: &[],
        },
        StartCase {
            case: "one service, one quota code the catalog knows",
            opt_in_type: "NotifyOnly",
            notification: None,
            exclusions: &[("lambda", &["L-B99A9384"])],
        },
        StartCase {
            case: "two services, several codes, one of them named twice",
            opt_in_type: "NotifyOnly",
            notification: Some("several-services"),
            exclusions: &[
                ("ec2", &["L-1216C47A", "L-0263D0A3", "L-1216C47A"]),
                ("dynamodb", &["L-F98FE922"]),
            ],
        },
        StartCase {
            case: "a service excluded with no quota codes of its own",
            opt_in_type: "NotifyOnly",
            notification: None,
            exclusions: &[("s3", &[])],
        },
    ];

    for StartCase {
        case,
        opt_in_type: requested_type,
        notification,
        exclusions,
    } in rows
    {
        // A fresh emulator per row: the configuration is one record per account, so
        // one row's start would otherwise be the next row's starting point.
        let engine = emulator();
        let client = client(&engine, "us-east-1");
        let arn = notification.map(notification_arn);

        let mut request = json!({
            "OptInLevel": ACCOUNT_LEVEL,
            "OptInType": opt_in_type(requested_type),
        });
        if let Some(arn) = &arn {
            request["NotificationArn"] = json!(arn);
        }
        if !exclusions.is_empty() {
            request["ExclusionList"] = exclusion_request(exclusions);
        }

        let started = client.ok("StartAutoManagement", request.clone()).await;
        assert_eq!(
            started,
            json!({}),
            "case {case}: StartAutoManagement models no output members, got {started}"
        );

        let read = configuration(&client, case).await;
        assert_eq!(
            scalars(&read),
            (
                Some(ACCOUNT_LEVEL),
                Some(opt_in_type(requested_type)),
                Some(OPT_IN_STATUS_ENABLED),
                arn.as_deref()
            ),
            "case {case}: starting with {request} must read back as stored, got {read}"
        );
        assert_eq!(
            exclusion_codes(&read),
            expected_codes(exclusions),
            "case {case}: the exclusion list must be reported as named, got {read}"
        );
        assert_every_entry_is_named(case, &read);
    }
}

/// The `ExclusionList` member of a request, from a row's exclusions.
fn exclusion_request(exclusions: &[(&str, &[&str])]) -> Value {
    let mut map = serde_json::Map::new();
    for (service_code, codes) in exclusions {
        map.insert((*service_code).to_string(), json!(codes));
    }
    Value::Object(map)
}

/// One row of the update table.
struct UpdateCase {
    case: &'static str,
    opt_in_type: Option<&'static str>,
    notification: Option<&'static str>,
    exclusions: Option<&'static [(&'static str, &'static [&'static str])]>,
}

/// Updating overwrites ONLY the members the update call names, and an exclusion list
/// it does name replaces the stored one outright.
///
/// The rows differ in WHICH members the update names — none, each one alone, and all
/// of them — and in the form of the exclusion list it names (another service, an empty
/// one).
#[tokio::test]
async fn updating_overwrites_only_the_members_it_names() {
    /// The exclusion list every row starts from.
    const BASELINE_EXCLUSIONS: &[(&str, &[&str])] = &[("lambda", &["L-B99A9384"])];

    let rows = [
        UpdateCase {
            case: "an update that names nothing changes nothing",
            opt_in_type: None,
            notification: None,
            exclusions: None,
        },
        UpdateCase {
            case: "only the opt-in type",
            opt_in_type: Some("NotifyOnly"),
            notification: None,
            exclusions: None,
        },
        UpdateCase {
            case: "only the notification ARN",
            opt_in_type: None,
            notification: Some("second-channel"),
            exclusions: None,
        },
        UpdateCase {
            case: "an exclusion list replaces the stored one outright",
            opt_in_type: None,
            notification: None,
            exclusions: Some(&[("ec2", &["L-0263D0A3", "L-1216C47A"])]),
        },
        UpdateCase {
            case: "an empty exclusion list clears the stored one",
            opt_in_type: None,
            notification: None,
            exclusions: Some(&[]),
        },
        UpdateCase {
            case: "every member at once",
            opt_in_type: Some("NotifyOnly"),
            notification: Some("second-channel"),
            exclusions: Some(&[("dynamodb", &["L-F98FE922"]), ("vpc", &["L-045E6E9C"])]),
        },
    ];

    for UpdateCase {
        case,
        opt_in_type: new_type,
        notification,
        exclusions,
    } in rows
    {
        let engine = emulator();
        let client = client(&engine, "us-east-1");
        let baseline_arn = notification_arn("quota-alerts");

        client
            .ok(
                "StartAutoManagement",
                json!({
                    "OptInLevel": ACCOUNT_LEVEL,
                    "OptInType": opt_in_type("NotifyOnly"),
                    "NotificationArn": baseline_arn,
                    "ExclusionList": exclusion_request(BASELINE_EXCLUSIONS),
                }),
            )
            .await;

        let new_arn = notification.map(notification_arn);
        let mut request = json!({});
        if let Some(new_type) = new_type {
            request["OptInType"] = json!(opt_in_type(new_type));
        }
        if let Some(arn) = &new_arn {
            request["NotificationArn"] = json!(arn);
        }
        if let Some(exclusions) = exclusions {
            request["ExclusionList"] = exclusion_request(exclusions);
        }

        let updated = client
            .ok("UpdateAutoManagement", request.clone())
            .await;
        assert_eq!(
            updated,
            json!({}),
            "case {case}: UpdateAutoManagement models no output members, got {updated}"
        );

        // A member the update did not name keeps the baseline's value; the level and
        // the status are not the update's to change at all.
        let read = configuration(&client, case).await;
        assert_eq!(
            scalars(&read),
            (
                Some(ACCOUNT_LEVEL),
                Some(opt_in_type(new_type.unwrap_or("NotifyOnly"))),
                Some(OPT_IN_STATUS_ENABLED),
                Some(new_arn.as_deref().unwrap_or(baseline_arn.as_str())),
            ),
            "case {case}: updating with {request} must change only what it names, got {read}"
        );
        assert_eq!(
            exclusion_codes(&read),
            expected_codes(exclusions.unwrap_or(BASELINE_EXCLUSIONS)),
            "case {case}: updating with {request} must leave the exclusion list it does \
             not name alone, got {read}"
        );
        assert_every_entry_is_named(case, &read);
    }
}

/// Stopping removes the configurations the opt-in carried: the read afterwards reports
/// `OptInStatus: DISABLED` and nothing else, whatever the stopped configuration held.
/// After that the calls that need Automatic Management RUNNING are refused, and
/// starting again records a whole new configuration.
///
/// The rows differ in the configuration that was running when the stop arrived — how
/// many of its optional members were set, and how large its exclusion list was —
/// because the rule is the same for all of them.
#[tokio::test]
async fn stopping_removes_the_whole_configuration() {
    let rows = [
        StartCase {
            case: "a bare configuration",
            opt_in_type: "NotifyOnly",
            notification: None,
            exclusions: &[],
        },
        StartCase {
            case: "a configuration with only a notification ARN",
            opt_in_type: "NotifyOnly",
            notification: Some("quota-alerts"),
            exclusions: &[],
        },
        StartCase {
            case: "a configuration with only an exclusion list",
            opt_in_type: "NotifyOnly",
            notification: None,
            exclusions: &[("lambda", &["L-B99A9384"]), ("ec2", &["L-0263D0A3"])],
        },
        StartCase {
            case: "a configuration with every member set",
            opt_in_type: "NotifyOnly",
            notification: Some("quota-alerts"),
            exclusions: &[("vpc", &["L-045E6E9C"])],
        },
    ];

    for StartCase {
        case,
        opt_in_type: started_type,
        notification,
        exclusions,
    } in rows
    {
        let engine = emulator();
        let client = client(&engine, "us-east-1");
        let arn = notification.map(notification_arn);

        let mut request = json!({
            "OptInLevel": ACCOUNT_LEVEL,
            "OptInType": opt_in_type(started_type),
        });
        if let Some(arn) = &arn {
            request["NotificationArn"] = json!(arn);
        }
        if !exclusions.is_empty() {
            request["ExclusionList"] = exclusion_request(exclusions);
        }
        client.ok("StartAutoManagement", request).await;

        let stopped = client.ok("StopAutoManagement", json!({})).await;
        assert_eq!(
            stopped,
            json!({}),
            "case {case}: StopAutoManagement models no output members, got {stopped}"
        );

        // The whole configuration went with the stop: the only thing left to report is
        // that Automatic Management is not running.
        let read = configuration(&client, case).await;
        assert_eq!(
            scalars(&read),
            (None, None, Some(OPT_IN_STATUS_DISABLED), None),
            "case {case}: stopping must remove the configuration it stopped, leaving \
             OptInStatus DISABLED with no other member, got {read}"
        );
        assert_eq!(
            exclusion_codes(&read),
            None,
            "case {case}: stopping must remove the exclusion list with the rest of the \
             configuration, got {read}"
        );

        // Nothing is running, so the calls that need a running configuration are
        // refused — both of them, with the same modelled error.
        for operation in ["StopAutoManagement", "UpdateAutoManagement"] {
            let (status, code, _) = refused(&client, operation, &json!({})).await;
            assert_eq!(
                status, CLIENT_FAULT,
                "case {case}: expected {operation} after the stop to answer HTTP \
                 {CLIENT_FAULT}, got {status}"
            );
            assert_eq!(
                code, "NoSuchResourceException",
                "case {case}: expected {operation} after the stop to answer \
                 NoSuchResourceException, got {code}"
            );
        }

        // Starting again is a whole new configuration, ENABLED, with nothing left over
        // from the one that was stopped.
        client
            .ok(
                "StartAutoManagement",
                json!({
                    "OptInLevel": ACCOUNT_LEVEL,
                    "OptInType": opt_in_type("NotifyOnly"),
                }),
            )
            .await;
        let restarted = configuration(&client, case).await;
        assert_eq!(
            scalars(&restarted),
            (
                Some(ACCOUNT_LEVEL),
                Some(opt_in_type("NotifyOnly")),
                Some(OPT_IN_STATUS_ENABLED),
                None,
            ),
            "case {case}: starting again records a whole new configuration, got {restarted}"
        );
        assert_eq!(
            exclusion_codes(&restarted),
            None,
            "case {case}: starting again leaves no exclusion list behind, got {restarted}"
        );
    }
}

/// The configuration belongs to the ACCOUNT, not to a region: `OptInLevel` has the
/// single member `ACCOUNT`, so a start made at one regional endpoint is what every
/// other endpoint reports, and so is an update made at a third.
///
/// The rows are region triples drawn from `state::KNOWN_REGIONS`, spanning the
/// commercial, GovCloud and China partitions.
#[tokio::test]
async fn a_start_or_update_is_one_record_for_the_whole_account() {
    let rows: [(&str, [&str; 3]); 4] = [
        ("start in the default region", ["us-east-1", "eu-west-1", "ap-northeast-1"]),
        ("start away from the default region", ["eu-central-1", "us-east-1", "sa-east-1"]),
        ("across partitions", ["us-gov-west-1", "cn-north-1", "us-west-2"]),
        ("the far regions", ["ap-southeast-7", "mx-central-1", "il-central-1"]),
    ];

    for (case, [start_region, other, third]) in rows {
        let engine = emulator();
        let starter = client(&engine, start_region);

        starter
            .ok(
                "StartAutoManagement",
                json!({
                    "OptInLevel": ACCOUNT_LEVEL,
                    "OptInType": opt_in_type("NotifyOnly"),
                    "NotificationArn": notification_arn("quota-alerts"),
                    "ExclusionList": exclusion_request(&[("lambda", &["L-B99A9384"])]),
                }),
            )
            .await;

        let expected = configuration(&starter, case).await;
        for region in [other, third] {
            let elsewhere = client(&engine, region);
            let read = configuration(&elsewhere, case).await;
            assert_eq!(
                read, expected,
                "case {case}: the configuration started at {start_region} must read the \
                 same at {region}, expected {expected}, got {read}"
            );
        }

        // And an update made at one endpoint is what every endpoint then reports.
        let updated_arn = notification_arn("second-channel");
        let updater = client(&engine, third);
        updater
            .ok(
                "UpdateAutoManagement",
                json!({"NotificationArn": updated_arn}),
            )
            .await;
        for region in [start_region, other, third] {
            let reader = client(&engine, region);
            let read = configuration(&reader, case).await;
            assert_eq!(
                read.get("NotificationArn").and_then(Value::as_str),
                Some(updated_arn.as_str()),
                "case {case}: the update made at {third} must be what {region} reports, \
                 got {read}"
            );
        }
    }
}

/// A request that breaks a modelled constraint is refused at the boundary, before any
/// of it is recorded, by the validator that owns the violation (see `src/service.rs` and
/// the `error_envelope` suite): a value outside an enumerated type's value set is a
/// malformed request, refused by the request-validation front end as
/// `ValidationException` inside AWS's count envelope and reporting the value set the
/// member admits, while every other constraint keeps this service's own modelled
/// `IllegalArgumentException` naming the member the caller got wrong. Either way the
/// account is left with no configuration at all.
///
/// The rows differ in WHICH constraint the request breaks: an `OptInLevel` or
/// `OptInType` outside its value set, a required member left out, and a
/// `NotificationArn` that does not match the modeled pattern.
#[tokio::test]
async fn a_configuration_that_breaks_a_modeled_constraint_records_nothing() {
    // Each row: why it is invalid, the request, the error code the refusal carries, and
    // the opening of the clause its message states.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "an opt-in level outside the value set",
            json!({"OptInLevel": "ORGANIZATION", "OptInType": "NotifyOnly"}),
            "ValidationException",
            "1 validation error detected: Value at 'optInLevel' failed to satisfy \
             constraint: Member must satisfy enum value set: ["
                .to_string(),
        ),
        (
            "an opt-in level in the wrong case",
            json!({"OptInLevel": "Account", "OptInType": "NotifyOnly"}),
            "ValidationException",
            "1 validation error detected: Value at 'optInLevel' failed to satisfy \
             constraint: Member must satisfy enum value set: ["
                .to_string(),
        ),
        (
            "an opt-in type outside the value set",
            json!({"OptInLevel": ACCOUNT_LEVEL, "OptInType": "NotifySometimes"}),
            "ValidationException",
            "1 validation error detected: Value at 'optInType' failed to satisfy \
             constraint: Member must satisfy enum value set: ["
                .to_string(),
        ),
        (
            "an absent required opt-in type",
            json!({"OptInLevel": ACCOUNT_LEVEL}),
            "IllegalArgumentException",
            "OptInType failed to satisfy constraint: Member must not be null".to_string(),
        ),
        (
            "a notification ARN that is not an ARN",
            json!({
                "OptInLevel": ACCOUNT_LEVEL,
                "OptInType": "NotifyOnly",
                "NotificationArn": "quota-alerts",
            }),
            "IllegalArgumentException",
            "NotificationArn failed to satisfy constraint: ".to_string(),
        ),
        (
            "a notification ARN whose account id is not twelve digits",
            json!({
                "OptInLevel": ACCOUNT_LEVEL,
                "OptInType": "NotifyOnly",
                "NotificationArn": "arn:aws:notifications::123:configuration/quota-alerts",
            }),
            "IllegalArgumentException",
            "NotificationArn failed to satisfy constraint: ".to_string(),
        ),
    ];

    for (case, request, expected_code, clause) in rows {
        let engine = emulator();
        let client = client(&engine, "us-east-1");

        let (status, code, message) = refused(&client, "StartAutoManagement", &request).await;
        assert_eq!(
            status, CLIENT_FAULT,
            "case {case}: expected {request} to be refused with HTTP {CLIENT_FAULT}, got {status}"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected {request} to be refused as {expected_code}, got {code}"
        );
        assert!(
            message.starts_with(&clause),
            "case {case}: the refusal must state {clause:?}, got {message:?}"
        );

        // Nothing was recorded, so the read still reports an account that never opted
        // in: DISABLED, with no preferences.
        let read = configuration(&client, case).await;
        assert_eq!(
            scalars(&read),
            (None, None, Some(OPT_IN_STATUS_DISABLED), None),
            "case {case}: a refused start must record nothing, got {read}"
        );
        assert_eq!(
            exclusion_codes(&read),
            None,
            "case {case}: a refused start must record no exclusion list, got {read}"
        );
    }
}

/// Updating or stopping before anything has been started is refused with the modelled
/// `NoSuchResourceException`: there is no running configuration to change.
#[tokio::test]
async fn changing_a_configuration_that_was_never_started_is_refused() {
    // Rows: the operation, and a body that is well formed for it.
    let rows: Vec<(&str, Value)> = vec![
        ("StopAutoManagement", json!({})),
        ("UpdateAutoManagement", json!({})),
        (
            "UpdateAutoManagement",
            json!({"OptInType": opt_in_type("NotifyOnly")}),
        ),
        (
            "UpdateAutoManagement",
            json!({"NotificationArn": notification_arn("quota-alerts")}),
        ),
        (
            "UpdateAutoManagement",
            json!({"ExclusionList": exclusion_request(&[("lambda", &["L-B99A9384"])])}),
        ),
    ];

    for (operation, request) in rows {
        let engine = emulator();
        let client = client(&engine, "us-east-1");
        let case = format!("{operation} {request}");

        let (status, code, _) = refused(&client, operation, &request).await;
        assert_eq!(
            status, CLIENT_FAULT,
            "case {case}: expected HTTP {CLIENT_FAULT}, got {status}"
        );
        assert_eq!(
            code, "NoSuchResourceException",
            "case {case}: expected NoSuchResourceException, got {code}"
        );

        // And the refusal recorded nothing: the read still reports DISABLED with no
        // preferences.
        let read = configuration(&client, &case).await;
        assert_eq!(
            scalars(&read),
            (None, None, Some(OPT_IN_STATUS_DISABLED), None),
            "case {case}: a refused change must record nothing, got {read}"
        );
    }
}

/// An exclusion list a row carries, as the tables below spell one.
type Exclusions = &'static [(&'static str, &'static [&'static str])];

/// The optional members a row carries beside the opt-in type, so the rule is asserted
/// for a request that names the member alone and for one that names it beside
/// everything else the operation takes.
///
/// `(case, NotificationArn, ExclusionList)`.
const OPTIONAL_MEMBERS: [(&str, Option<&str>, Option<Exclusions>); 4] = [
    ("the opt-in type alone", None, None),
    ("beside a notification ARN", Some("quota-alerts"), None),
    (
        "beside an exclusion list",
        None,
        Some(&[("lambda", &["L-B99A9384"])]),
    ),
    (
        "beside every optional member",
        Some("quota-alerts"),
        Some(&[("ec2", &["L-1216C47A"]), ("dynamodb", &["L-F98FE922"])]),
    ),
];

/// The exclusion list the running configuration of the update rows carries.
const RUNNING_EXCLUSIONS: &[(&str, &[&str])] = &[("vpc", &["L-045E6E9C"])];

/// A request body naming `requested` as its `OptInType`, plus the optional members of a
/// row. `OptInLevel` is included only for the writer that requires it.
fn opt_in_request(
    requested: &str,
    with_level: bool,
    notification: Option<&str>,
    exclusions: Option<Exclusions>,
) -> Value {
    let mut request = json!({ "OptInType": requested });
    if with_level {
        request["OptInLevel"] = json!(ACCOUNT_LEVEL);
    }
    if let Some(name) = notification {
        request["NotificationArn"] = json!(notification_arn(name));
    }
    if let Some(exclusions) = exclusions {
        request["ExclusionList"] = exclusion_request(exclusions);
    }
    request
}

/// EVERY `OptInType` member the enum declares is stored as given by both writers of the
/// member: the member carries no constraint narrowing it to a subset of the enum, so a
/// value the enum declares is never invalid input, and nothing beyond the boundary
/// validator's enum check stands between the caller and the stored mode.
///
/// The rows span every member of the model's own value list (`ops::types::
/// OPT_IN_TYPE_VALUES`, generated from `OptInType`), each one submitted alone and beside
/// every other member the operation takes, through the full writer
/// (`StartAutoManagement`) and the partial one (`UpdateAutoManagement`) over a running
/// configuration that was started on a DIFFERENT member, so each row genuinely moves the
/// stored mode. A member the model gains is covered without this table being edited.
#[tokio::test]
async fn every_modeled_opt_in_type_is_stored_by_both_writers() {
    for requested in OPT_IN_TYPE_VALUES {
        let requested = opt_in_type(requested);
        for (shape, notification, exclusions) in OPTIONAL_MEMBERS {
            // The FULL writer records the whole call, whichever member it names.
            let case = format!("StartAutoManagement {requested} {shape}");
            let start_engine = emulator();
            let start_client = client(&start_engine, "us-east-1");
            let request = opt_in_request(requested, true, notification, exclusions);
            let started = start_client
                .ok("StartAutoManagement", request.clone())
                .await;
            assert_eq!(
                started,
                json!({}),
                "case {case}: StartAutoManagement models no output members, got {started}"
            );

            let read = configuration(&start_client, &case).await;
            assert_eq!(
                scalars(&read),
                (
                    Some(ACCOUNT_LEVEL),
                    Some(requested),
                    Some(OPT_IN_STATUS_ENABLED),
                    notification.map(notification_arn).as_deref(),
                ),
                "case {case}: starting with {request} must read back as stored, got {read}"
            );
            assert_eq!(
                exclusion_codes(&read),
                expected_codes(exclusions.unwrap_or(&[])),
                "case {case}: the exclusion list must be reported as named, got {read}"
            );
            assert_every_entry_is_named(&case, &read);

            // The PARTIAL writer moves a running configuration from one member to
            // another, and leaves the members the call does not name alone.
            let case = format!("UpdateAutoManagement {requested} {shape}");
            let running_engine = emulator();
            let running_client = client(&running_engine, "us-east-1");
            let running_arn = notification_arn("running-channel");
            let running_type = another_opt_in_type(requested);
            running_client
                .ok(
                    "StartAutoManagement",
                    json!({
                        "OptInLevel": ACCOUNT_LEVEL,
                        "OptInType": running_type,
                        "NotificationArn": running_arn,
                        "ExclusionList": exclusion_request(RUNNING_EXCLUSIONS),
                    }),
                )
                .await;

            let update = opt_in_request(requested, false, notification, exclusions);
            let updated = running_client
                .ok("UpdateAutoManagement", update.clone())
                .await;
            assert_eq!(
                updated,
                json!({}),
                "case {case}: UpdateAutoManagement models no output members, got {updated}"
            );

            let read = configuration(&running_client, &case).await;
            let expected_arn = notification
                .map(notification_arn)
                .unwrap_or_else(|| running_arn.clone());
            assert_eq!(
                scalars(&read),
                (
                    Some(ACCOUNT_LEVEL),
                    Some(requested),
                    Some(OPT_IN_STATUS_ENABLED),
                    Some(expected_arn.as_str()),
                ),
                "case {case}: updating with {update} must store the mode it names and \
                 leave OptInStatus ENABLED, got {read}"
            );
            assert_eq!(
                exclusion_codes(&read),
                expected_codes(exclusions.unwrap_or(RUNNING_EXCLUSIONS)),
                "case {case}: updating with {update} must leave the exclusion list it \
                 does not name alone, got {read}"
            );
        }
    }
}

/// The Automatic Management lifecycle a client drives when it changes its mode: start on
/// one member, switch to another with the partial writer — which must leave `OptInStatus`
/// at `ENABLED`, since the opt-in type is not the status — and stop, which removes the
/// configuration the switched mode was part of.
///
/// The rows are every ORDERED pair of members the model declares, so each switch is
/// driven in both directions as well as from a member to itself.
#[tokio::test]
async fn switching_the_opt_in_type_keeps_the_status_and_a_stop_removes_the_configuration() {
    for started in OPT_IN_TYPE_VALUES {
        for switched in OPT_IN_TYPE_VALUES {
            let started = opt_in_type(started);
            let switched = opt_in_type(switched);
            let case = format!("{started} -> {switched}");
            let engine = emulator();
            let client = client(&engine, "us-east-1");

            client
                .ok(
                    "StartAutoManagement",
                    json!({"OptInLevel": ACCOUNT_LEVEL, "OptInType": started}),
                )
                .await;
            let read = configuration(&client, &case).await;
            assert_eq!(
                scalars(&read),
                (
                    Some(ACCOUNT_LEVEL),
                    Some(started),
                    Some(OPT_IN_STATUS_ENABLED),
                    None,
                ),
                "case {case}: the start must read back ENABLED on the mode it named, \
                 got {read}"
            );

            let updated = client
                .ok("UpdateAutoManagement", json!({"OptInType": switched}))
                .await;
            assert_eq!(
                updated,
                json!({}),
                "case {case}: UpdateAutoManagement models no output members, got {updated}"
            );
            let read = configuration(&client, &case).await;
            assert_eq!(
                scalars(&read),
                (
                    Some(ACCOUNT_LEVEL),
                    Some(switched),
                    Some(OPT_IN_STATUS_ENABLED),
                    None,
                ),
                "case {case}: switching the mode must not change OptInStatus, got {read}"
            );

            client.ok("StopAutoManagement", json!({})).await;
            let read = configuration(&client, &case).await;
            assert_eq!(
                scalars(&read),
                (None, None, Some(OPT_IN_STATUS_DISABLED), None),
                "case {case}: stopping must remove the configuration the switched mode \
                 was part of, got {read}"
            );
        }
    }
}

