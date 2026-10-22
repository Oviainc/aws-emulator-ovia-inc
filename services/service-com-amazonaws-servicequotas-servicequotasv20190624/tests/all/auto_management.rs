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
//! * an exclusion list is stored as given — each service's codes in the order the
//!   caller named them;
//! * updating overwrites only the members the update call names, and an exclusion list
//!   it does name replaces the stored one outright;
//! * stopping moves the opt-in to `OptInStatus = DISABLED` and keeps the preferences,
//!   after which the calls that need Automatic Management RUNNING are refused with the
//!   modelled `NoSuchResourceException`;
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

/// The `OptInType` member `name`, checked against the model's own value list so a row
/// cannot name a type the model does not declare.
fn opt_in_type(name: &str) -> &'static str {
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

/// The scalar members of a reported configuration, as `(OptInLevel, OptInType,
/// OptInStatus, NotificationArn)` — each `None` when the member is absent.
fn scalars(body: &Value) -> (Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
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

/// Read the configuration back, which must succeed: the read reports
/// `OptInStatus: DISABLED` rather than failing even when nothing was ever started.
async fn configuration(client: &Client, case: &str) -> Value {
    let (status, body) = client.call(GET_CONFIG, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case}: expected {GET_CONFIG} to answer HTTP 200, got {status} with {body}"
    );
    body
}

/// Dispatch a call that must be refused, and return its status, error code and the
/// `Message` member that says what was wrong. `quota_workflow`'s own helper reports
/// only the status and the code.
async fn refused(client: &Client, operation: &str, body: &Value) -> (u16, String, String) {
    let (status, parsed) = client.call(operation, body).await;
    let member = |name: &str| {
        parsed
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    };
    (status, member("__type"), member("Message"))
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
/// The rows differ along everything the rule generalises over: the opt-in type, a
/// notification ARN present or absent, and an exclusion list absent, naming one
/// service or several, carrying one quota code or several, carrying the same code
/// twice, carrying a code the catalog does not know, and naming a service with no
/// quota codes at all.
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
            case: "notify and adjust, with a notification ARN",
            opt_in_type: "NotifyAndAdjust",
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
            case: "a quota code the catalog does not know",
            opt_in_type: "NotifyOnly",
            notification: None,
            exclusions: &[("lambda", &["L-00000000"])],
        },
        StartCase {
            case: "a service excluded with no quota codes of its own",
            opt_in_type: "NotifyAndAdjust",
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
            opt_in_type: Some("NotifyAndAdjust"),
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
            opt_in_type: Some("NotifyAndAdjust"),
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
    }
}

/// Stopping moves the opt-in to `DISABLED` and keeps the preferences on record; after
/// that the calls that need Automatic Management RUNNING are refused, and starting
/// again records a whole new configuration.
///
/// The rows differ in the configuration that was running when the stop arrived — how
/// many of its optional members were set, and how large its exclusion list was —
/// because the rule is the same for all of them.
#[tokio::test]
async fn stopping_disables_the_opt_in_and_keeps_the_preferences() {
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
            opt_in_type: "NotifyAndAdjust",
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

        // Only the status moved: the preferences the account chose are still reported.
        let read = configuration(&client, case).await;
        assert_eq!(
            scalars(&read),
            (
                Some(ACCOUNT_LEVEL),
                Some(opt_in_type(started_type)),
                Some(OPT_IN_STATUS_DISABLED),
                arn.as_deref(),
            ),
            "case {case}: stopping must only move OptInStatus to DISABLED, got {read}"
        );
        assert_eq!(
            exclusion_codes(&read),
            expected_codes(exclusions),
            "case {case}: stopping must keep the exclusion list on record, got {read}"
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
                    "OptInType": opt_in_type("NotifyAndAdjust"),
                }),
            )
            .await;
        let restarted = configuration(&client, case).await;
        assert_eq!(
            scalars(&restarted),
            (
                Some(ACCOUNT_LEVEL),
                Some(opt_in_type("NotifyAndAdjust")),
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
/// of it is recorded — in this service's own modelled `IllegalArgumentException`, whose
/// `Message` names the member the caller got wrong (see `src/service.rs` and the
/// `error_envelope` suite) — so the account is left with no configuration at all.
///
/// The rows differ in WHICH constraint the request breaks: an `OptInLevel` or
/// `OptInType` outside its value set, a required member left out, and a
/// `NotificationArn` that does not match the modeled pattern.
#[tokio::test]
async fn a_configuration_that_breaks_a_modeled_constraint_records_nothing() {
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an opt-in level outside the value set",
            json!({"OptInLevel": "ORGANIZATION", "OptInType": "NotifyOnly"}),
            "OptInLevel",
        ),
        (
            "an opt-in level in the wrong case",
            json!({"OptInLevel": "Account", "OptInType": "NotifyOnly"}),
            "OptInLevel",
        ),
        (
            "an opt-in type outside the value set",
            json!({"OptInLevel": ACCOUNT_LEVEL, "OptInType": "NotifySometimes"}),
            "OptInType",
        ),
        (
            "an absent required opt-in type",
            json!({"OptInLevel": ACCOUNT_LEVEL}),
            "OptInType",
        ),
        (
            "a notification ARN that is not an ARN",
            json!({
                "OptInLevel": ACCOUNT_LEVEL,
                "OptInType": "NotifyOnly",
                "NotificationArn": "quota-alerts",
            }),
            "NotificationArn",
        ),
        (
            "a notification ARN whose account id is not twelve digits",
            json!({
                "OptInLevel": ACCOUNT_LEVEL,
                "OptInType": "NotifyOnly",
                "NotificationArn": "arn:aws:notifications::123:configuration/quota-alerts",
            }),
            "NotificationArn",
        ),
    ];

    for (case, request, member) in rows {
        let engine = emulator();
        let client = client(&engine, "us-east-1");

        let (status, code, message) = refused(&client, "StartAutoManagement", &request).await;
        assert_eq!(
            status, CLIENT_FAULT,
            "case {case}: expected {request} to be refused with HTTP {CLIENT_FAULT}, got {status}"
        );
        assert_eq!(
            code, "IllegalArgumentException",
            "case {case}: expected {request} to be refused as IllegalArgumentException, got {code}"
        );
        assert!(
            message.starts_with(member),
            "case {case}: the refusal must name the member that was wrong ({member}), \
             got {message:?}"
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
