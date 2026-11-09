// SPDX-License-Identifier: CC0-1.0

//! What a stop of Service Quotas Automatic Management leaves behind, end to end over
//! the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine, so what is exercised is the HTTP
//! behaviour and the persisted state behind it. The harness (`emulator`, `client`,
//! `Client`, `ACCOUNT`) is `quota_workflow.rs`'s.
//!
//! The rule asserted, over a TABLE of running configurations rather than one case:
//! stopping Automatic Management removes the configurations associated with it, so the
//! read that follows a stop answers — member for member — exactly what an account that
//! never opted in is answered, and a later start has to be given the whole
//! configuration afresh.
//!
//! The rows span what the rule generalises over: the opt-in type (every member the
//! model's enum declares), a notification ARN present or absent, an exclusion list
//! absent, naming one service, naming several with several codes each, and naming a
//! service with no codes of its own, and the regional endpoint the stop is issued at
//! relative to the ones the start and the read use, since the configuration is one
//! record for the whole account.
//!
//! `auto_management.rs` owns the accepting side of the writers,
//! `auto_management_exclusions.rs` the refusal of an exclusion that names no quota, and
//! `auto_management_configuration.rs` how a stored configuration is reported.

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
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// An exclusion list as the rows below spell one: service code -> quota codes.
type Exclusions = &'static [(&'static str, &'static [&'static str])];

/// A User Notifications ARN of this emulator's account, which satisfies the modelled
/// `AmazonResourceName` pattern `^arn:aws(-[\w]+)*:*:.+:[0-9]{12}:.+$`. It is the
/// CALLER's own ARN, so the test builds it rather than the emulator minting it.
fn notification_arn(name: &str) -> String {
    format!("arn:aws:notifications::{ACCOUNT}:configuration/{name}")
}

/// The `ExclusionList` member of a request, from a row's exclusions.
fn exclusion_request(exclusions: Exclusions) -> Value {
    let mut list = serde_json::Map::new();
    for (service_code, quota_codes) in exclusions {
        list.insert((*service_code).to_string(), json!(quota_codes));
    }
    Value::Object(list)
}

/// Read the configuration back, which must succeed whether or not anything is running:
/// an account with nothing on record is reported rather than refused.
async fn configuration(client: &Client, case: &str) -> Value {
    let (status, body) = client.call(GET_CONFIG, &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case}: expected {GET_CONFIG} to answer HTTP 200, got {status} with {body}"
    );
    body
}

/// Assert that `operation` is refused with the modelled `NoSuchResourceException`,
/// which is what every call that needs Automatic Management RUNNING answers once the
/// configuration has been removed.
async fn assert_needs_a_running_configuration(client: &Client, case: &str, operation: &str) {
    let (status, code) = client.refused(operation, json!({})).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_FAULT, "NoSuchResourceException"),
        "case {case}: expected {operation} after the stop to be refused with HTTP \
         {CLIENT_FAULT} NoSuchResourceException, got {status} {code}"
    );
}

/// One row of the stop table: the configuration that is running when the stop arrives,
/// and the endpoints the start, the stop and the read are issued at.
struct StopCase {
    case: &'static str,
    notification: Option<&'static str>,
    exclusions: Exclusions,
    start_region: &'static str,
    stop_region: &'static str,
    read_region: &'static str,
}

/// The rows the rule is asserted over. The regions are members of
/// `state::KNOWN_REGIONS`, spanning the commercial, GovCloud and China partitions,
/// and the excluded quota codes are catalog entries (a write refuses one the catalog
/// does not hold — `auto_management_exclusions.rs`'s rule).
const ROWS: &[StopCase] = &[
    StopCase {
        case: "a bare configuration, one endpoint throughout",
        notification: None,
        exclusions: &[],
        start_region: "us-east-1",
        stop_region: "us-east-1",
        read_region: "us-east-1",
    },
    StopCase {
        case: "a configuration with only a notification ARN",
        notification: Some("quota-alerts"),
        exclusions: &[],
        start_region: "eu-west-1",
        stop_region: "eu-west-1",
        read_region: "eu-west-1",
    },
    StopCase {
        case: "a configuration with only an exclusion list of one service",
        notification: None,
        exclusions: &[("lambda", &["L-B99A9384"])],
        start_region: "us-east-1",
        stop_region: "ap-northeast-1",
        read_region: "us-east-1",
    },
    StopCase {
        case: "a configuration excluding several codes of several services",
        notification: Some("several-services"),
        exclusions: &[
            ("ec2", &["L-1216C47A", "L-0263D0A3"]),
            ("dynamodb", &["L-F98FE922"]),
            ("vpc", &["L-DF5E4CA3"]),
        ],
        start_region: "us-gov-west-1",
        stop_region: "us-west-2",
        read_region: "cn-north-1",
    },
    StopCase {
        case: "a configuration excluding a service with no codes of its own",
        notification: Some("whole-service"),
        exclusions: &[("s3", &[])],
        start_region: "mx-central-1",
        stop_region: "il-central-1",
        read_region: "sa-east-1",
    },
];

/// Stopping Automatic Management removes the configurations associated with it: the
/// read that follows a stop is the read of an account that never opted in, and nothing
/// the stopped opt-in carried comes back when Automatic Management is started again.
#[tokio::test]
async fn a_stop_removes_the_configurations_the_opt_in_carried() {
    // Every `OptInType` the model declares, so the rule is pinned for each mode rather
    // than for the one the recorded case happened to use.
    for &opt_in_type in OPT_IN_TYPE_VALUES {
        for &StopCase {
            case,
            notification,
            exclusions,
            start_region,
            stop_region,
            read_region,
        } in ROWS
        {
            let case = format!("{opt_in_type} {case}");
            let engine = emulator();
            let starter = client(&engine, start_region);
            let stopper = client(&engine, stop_region);
            let reader = client(&engine, read_region);

            // The answer an account that never opted in is given, read from the service
            // itself rather than spelled here: it is what the stop has to return the
            // account to.
            let never_opted_in = configuration(&reader, &case).await;
            assert_eq!(
                never_opted_in,
                json!({ "OptInStatus": OPT_IN_STATUS_DISABLED }),
                "case {case}: an account that never opted in must be reported as \
                 {OPT_IN_STATUS_DISABLED} alone, got {never_opted_in}"
            );

            let arn = notification.map(notification_arn);
            let mut request = json!({
                "OptInLevel": ACCOUNT_LEVEL,
                "OptInType": opt_in_type,
            });
            if let Some(arn) = &arn {
                request["NotificationArn"] = json!(arn);
            }
            if !exclusions.is_empty() {
                request["ExclusionList"] = exclusion_request(exclusions);
            }
            starter.ok("StartAutoManagement", request.clone()).await;

            // The row genuinely stored a configuration, so what the stop removes below
            // is a configuration that was there to remove.
            let running = configuration(&reader, &case).await;
            assert_ne!(
                running, never_opted_in,
                "case {case}: starting with {request} must record a configuration, got \
                 {running}"
            );
            assert_eq!(
                running.get("OptInStatus").and_then(Value::as_str),
                Some(OPT_IN_STATUS_ENABLED),
                "case {case}: the started configuration must be reported ENABLED, got \
                 {running}"
            );

            let stopped = stopper.ok("StopAutoManagement", json!({})).await;
            assert_eq!(
                stopped,
                json!({}),
                "case {case}: StopAutoManagement models no output members, got {stopped}"
            );

            // The whole answer is compared, so no member of the stopped configuration
            // can survive unnoticed.
            let read = configuration(&reader, &case).await;
            assert_eq!(
                read, never_opted_in,
                "case {case}: the stop must remove the configuration {running}, leaving \
                 the answer {never_opted_in} an account that never opted in gets, got \
                 {read}"
            );

            // Nothing is running any more, so the calls that need a running
            // configuration are refused — wherever they are issued.
            for refused_client in [&stopper, &reader] {
                for operation in ["StopAutoManagement", "UpdateAutoManagement"] {
                    assert_needs_a_running_configuration(refused_client, &case, operation).await;
                }
            }

            // A later start is given the whole configuration afresh: none of the
            // preferences the stop removed is merged back in.
            starter
                .ok(
                    "StartAutoManagement",
                    json!({ "OptInLevel": ACCOUNT_LEVEL, "OptInType": opt_in_type }),
                )
                .await;
            let restarted = configuration(&reader, &case).await;
            assert_eq!(
                restarted,
                json!({
                    "OptInLevel": ACCOUNT_LEVEL,
                    "OptInType": opt_in_type,
                    "OptInStatus": OPT_IN_STATUS_ENABLED,
                }),
                "case {case}: starting again must record only what that call carried, \
                 got {restarted}"
            );
        }
    }
}
