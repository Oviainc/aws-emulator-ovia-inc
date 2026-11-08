// SPDX-License-Identifier: CC0-1.0

//! What the APPLIED catalog of a service publishes: that it carries every quota the
//! service publishes, each under AWS's own `QuotaCode` with the text AWS publishes for
//! THAT quota.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine.
//!
//! The rules pinned, each over a TABLE rather than over one quota:
//!
//! * a service's applied listing publishes the quota of every `(ServiceCode,
//!   QuotaCode)` pair the service publishes — a pair left out of the catalog is a limit
//!   no client enumerating its quotas can discover — with that quota's own published
//!   `QuotaName`, `Description`, `Unit`, `Value`, `Adjustable`, `GlobalQuota` and rate
//!   window, the `ACCOUNT` level the record answers for, and the applied-quota ARN
//!   `arn:aws:servicequotas:<region>:<accountId>:<serviceCode>/<quotaCode>`; the single
//!   read of the pair answers with the same record;
//! * a quota whose published text is the fixed sentence of a CLASS carries that
//!   sentence around its own operation's name rather than a paraphrase of it: the
//!   per-second sentence for a sustained API rate, and the one-burst sentence for the
//!   burst allowance beside it. The text is derived here from each row's `QuotaName`,
//!   so every row of either class in the service's whole listing is a case.
//!
//! `applied_quota_records.rs` pins the record shapes a `ServiceQuota` comes in and
//! `api_rate_quota_rows.rs` the page the rate class is served on; what this suite adds
//! is COVERAGE — that the catalog publishes the pairs the service publishes — asserted
//! across regions. The harness is the one `quota_workflow.rs` owns.

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::LEVEL_ACCOUNT;

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};

/// One quota the service publishes, as its applied listing must report it.
#[derive(Debug)]
struct Published {
    /// The `ServiceCode` the quota belongs to.
    service_code: &'static str,
    /// AWS's own `QuotaCode` for it.
    quota_code: &'static str,
    /// `QuotaName`, as AWS publishes it.
    quota_name: &'static str,
    /// `Description`, or `None` for a quota AWS publishes none for.
    description: Option<&'static str>,
    /// `Unit`.
    unit: &'static str,
    /// The `Value` in force with nothing applied: the AWS default.
    value: f64,
    /// `Adjustable`.
    adjustable: bool,
    /// `GlobalQuota`.
    global_quota: bool,
    /// `Period` as `(PeriodValue, PeriodUnit)`, or `None` for a quota published over no
    /// window.
    period: Option<(i64, &'static str)>,
}

/// The table: the pairs the rule is asserted over.
///
/// SOURCE: AWS's published Service Quotas data for each `ServiceCode` and the recorded
/// Service Quotas listing of it for which `QuotaCode` names which quota — the same
/// source `src/state/quota_rows.rs` cites row by row. The rows differ along everything
/// the rule generalises over: a throughput quota in megabits per second and counted
/// quotas, a sustained API rate and the burst allowance beside one, adjustable and
/// fixed rows, a row published over a one-second window and rows published over none, a
/// measured row and unmeasured ones, a row that publishes no `Description` at all, and
/// three service codes — so a row bound to the wrong quota fails here whichever row it
/// is bound to.
const PUBLISHED: &[Published] = &[
    // A quota of ONE execution environment's network throughput: a unit that is
    // neither `None` nor a count, and adjustable.
    Published {
        service_code: "lambda",
        quota_code: "L-14320ECE",
        quota_name: "Network bandwidth per execution environment",
        description: Some(
            "The maximum sustained network bandwidth, in megabits per second, for a Lambda \
             function's execution environment that is not attached to a VPC.",
        ),
        unit: "Megabits/Second",
        value: 625.0,
        adjustable: true,
        global_quota: false,
        period: None,
    },
    // The sustained rate of one API operation, published as adjustable and over no
    // window of its own.
    Published {
        service_code: "lambda",
        quota_code: "L-BA29C22B",
        quota_name: "Rate of GetDurableExecutionHistory API requests",
        description: Some(
            "The maximum number of GetDurableExecutionHistory API requests per second.",
        ),
        unit: "None",
        value: 15.0,
        adjustable: true,
        global_quota: false,
        period: None,
    },
    // The BURST allowance beside such a rate, which is a quota of its own and carries
    // the class's own sentence.
    Published {
        service_code: "lambda",
        quota_code: "L-25EEC0A4",
        quota_name: "Burst rate of ResumeMicrovm API requests",
        description: Some(
            "The maximum number of ResumeMicrovm API requests that can be sent in one burst.",
        ),
        unit: "None",
        value: 5.0,
        adjustable: true,
        global_quota: false,
        period: None,
    },
    // The sustained rate of another operation of the same service, published as NOT
    // adjustable and over the one-second window AWS publishes such a rate over.
    Published {
        service_code: "lambda",
        quota_code: "L-4273958C",
        quota_name: "Rate of GetPolicy API requests",
        description: Some("The maximum number of GetPolicy API requests per second."),
        unit: "None",
        value: 15.0,
        adjustable: false,
        global_quota: false,
        period: Some((1, "SECOND")),
    },
    // A `Count` quota of the same service: a row a code bound positionally would answer
    // with a rate quota's definition for.
    Published {
        service_code: "lambda",
        quota_code: "L-72E0D058",
        quota_name: "Number of concurrent MicroVM image builds",
        description: Some("Number of concurrent MicroVM image builds per account per region"),
        unit: "Count",
        value: 10.0,
        adjustable: true,
        global_quota: false,
        period: None,
    },
    // A MEASURED quota of another service: CloudWatch publishes its usage, so it is
    // published over the window those counts are published over.
    Published {
        service_code: "dynamodb",
        quota_code: "L-F98FE922",
        quota_name: "Maximum number of tables",
        description: Some(
            "The maximum number of tables that can be created in this account in the current \
             Region.",
        ),
        unit: "None",
        value: 2500.0,
        adjustable: true,
        global_quota: false,
        period: Some((5, "MINUTE")),
    },
    // A fixed quota that publishes NO description: an absent member stays absent rather
    // than borrowing a neighbour's text.
    Published {
        service_code: "vpc",
        quota_code: "L-085A6257",
        quota_name: "IPv6 CIDR blocks per VPC",
        description: None,
        unit: "None",
        value: 1.0,
        adjustable: false,
        global_quota: false,
        period: None,
    },
];

/// The API operation a `QuotaName` names, for each class whose `Description` is a fixed
/// sentence around that name: `Rate of <Api> API requests` (the sustained rate) and
/// `Burst rate of <Api> API requests` (the burst allowance beside it).
///
/// `<Api>` has to name ONE operation — a single word in the casing an operation is named
/// in. A name whose middle is prose ("control plane", "capacity provider write")
/// describes a rate over a GROUP of operations and publishes its own sentence about that
/// group, so it is of neither class.
fn classified_api(quota_name: &str) -> Option<(&str, String)> {
    let (api, sentence) = match quota_name.strip_prefix("Burst rate of ") {
        Some(rest) => (
            rest.strip_suffix(" API requests")?,
            "that can be sent in one burst",
        ),
        None => (
            quota_name
                .strip_prefix("Rate of ")?
                .strip_suffix(" API requests")?,
            "per second",
        ),
    };
    let names_one_operation = !api.is_empty()
        && !api.contains(char::is_whitespace)
        && api.starts_with(char::is_uppercase);
    names_one_operation
        .then(|| (api, format!("The maximum number of {api} API requests {sentence}.")))
}

/// The `(PeriodValue, PeriodUnit)` a record publishes, when it publishes a window.
fn period(record: &Value) -> Option<(i64, &str)> {
    let period = record["Period"].as_object()?;
    Some((
        period.get("PeriodValue").and_then(Value::as_i64)?,
        period.get("PeriodUnit").and_then(Value::as_str)?,
    ))
}

/// The applied listing of `service_code`, walked to its end with the tokens the service
/// itself issues.
async fn applied(caller: &Client, service_code: &str) -> Vec<Value> {
    walk_pages(
        caller,
        "ListServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await
}

/// RULE: the applied catalog of a service publishes the quota of every pair the service
/// publishes, with that quota's own published facts, the `ACCOUNT` level and the
/// applied-quota ARN — and the single read of the pair answers with the same record.
#[tokio::test]
async fn the_applied_catalog_publishes_every_quota_the_service_publishes() {
    let engine = emulator();

    // Two endpoints, because the catalog is published per Region and the ARN of an
    // applied record names the Region it was read at.
    for region in ["us-west-2", "eu-central-1"] {
        let caller = client(&engine, region);

        for expected in PUBLISHED {
            let case = format!("{region} {}/{}", expected.service_code, expected.quota_code);
            let listed = applied(&caller, expected.service_code).await;
            let found: Vec<&Value> = listed
                .iter()
                .filter(|quota| quota["QuotaCode"].as_str() == Some(expected.quota_code))
                .collect();
            assert_eq!(
                found.len(),
                1,
                "case {case}: the applied listing must publish this quota exactly once, got {} \
                 records among {} quotas",
                found.len(),
                listed.len()
            );
            let record = found[0];

            assert_eq!(
                record["QuotaName"].as_str(),
                Some(expected.quota_name),
                "case {case}: expected QuotaName {:?}, got {:?}",
                expected.quota_name,
                record["QuotaName"]
            );
            assert_eq!(
                record["Description"].as_str(),
                expected.description,
                "case {case}: expected Description {:?}, got {:?}",
                expected.description,
                record["Description"]
            );
            assert_eq!(
                record["Unit"].as_str(),
                Some(expected.unit),
                "case {case}: expected Unit {:?}, got {:?}",
                expected.unit,
                record["Unit"]
            );
            assert_eq!(
                record["Value"].as_f64(),
                Some(expected.value),
                "case {case}: expected Value {}, got {:?}",
                expected.value,
                record["Value"]
            );
            assert_eq!(
                record["Adjustable"].as_bool(),
                Some(expected.adjustable),
                "case {case}: expected Adjustable {}, got {:?}",
                expected.adjustable,
                record["Adjustable"]
            );
            assert_eq!(
                record["GlobalQuota"].as_bool(),
                Some(expected.global_quota),
                "case {case}: expected GlobalQuota {}, got {:?}",
                expected.global_quota,
                record["GlobalQuota"]
            );
            assert_eq!(
                period(record),
                expected.period,
                "case {case}: expected Period {:?}, got {:?}",
                expected.period,
                record["Period"]
            );
            assert_eq!(
                record["QuotaAppliedAtLevel"].as_str(),
                Some(LEVEL_ACCOUNT),
                "case {case}: an account-wide record answers for the {LEVEL_ACCOUNT} level, got \
                 {:?}",
                record["QuotaAppliedAtLevel"]
            );
            assert_eq!(
                record["QuotaArn"].as_str(),
                Some(
                    format!(
                        "arn:aws:servicequotas:{region}:{ACCOUNT}:{}/{}",
                        expected.service_code, expected.quota_code
                    )
                    .as_str()
                ),
                "case {case}: an applied quota's ARN names the Region and the account, got {:?}",
                record["QuotaArn"]
            );
            assert_eq!(
                record["ServiceCode"].as_str(),
                Some(expected.service_code),
                "case {case}: a record reports the service it was read for, got {:?}",
                record["ServiceCode"]
            );

            // The single read of the pair: the same record, found BY its code.
            let read = caller
                .ok(
                    "GetServiceQuota",
                    json!({
                        "ServiceCode": expected.service_code,
                        "QuotaCode": expected.quota_code,
                    }),
                )
                .await["Quota"]
                .clone();
            assert_eq!(
                &read, record,
                "case {case}: GetServiceQuota must answer with the listed record, got {read}"
            );
        }
    }
}

/// RULE: a quota of either per-API throttle class publishes that class's fixed sentence
/// around its OWN operation's name — never a paraphrase of it, and never a neighbouring
/// quota's text.
#[tokio::test]
async fn a_per_api_throttle_publishes_its_own_class_sentence() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    let mut sustained = 0_usize;
    let mut bursts = 0_usize;

    // Every service the table covers, so the class rule is asked of whole listings
    // rather than of the rows the table names.
    for service_code in ["lambda", "dynamodb", "vpc"] {
        for quota in applied(&caller, service_code).await {
            let name = quota["QuotaName"].as_str().unwrap_or_default();
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let Some((api, expected)) = classified_api(name) else {
                continue;
            };
            if name.starts_with("Burst rate of ") {
                bursts += 1;
            } else {
                sustained += 1;
            }
            assert_eq!(
                quota["Description"].as_str(),
                Some(expected.as_str()),
                "case {service_code}/{code} ({name:?}): a throttle of {api} publishes \
                 {expected:?}, got {:?}",
                quota["Description"]
            );
        }
    }

    assert!(
        sustained >= 4 && bursts >= 1,
        "case catalog: both classes must be exercised — expected at least 4 sustained rate rows \
         and one burst row, got {sustained} and {bursts}"
    );
}
