// SPDX-License-Identifier: CC0-1.0

//! The record a per-quota read answers with, and the page a listing of this service
//! serves.
//!
//! Two rules are pinned here, over a TABLE rather than over one recorded quota:
//!
//! * a `QuotaCode` IDENTIFIES one quota, so `GetServiceQuota`,
//!   `GetAWSDefaultServiceQuota` and the two listings all answer for the pair asked
//!   for with THAT quota's own published definition — its name, description, unit,
//!   value, adjustability, scope, rate window and usage-metric dimensions — and a
//!   code the service does not publish is refused instead of answered with another
//!   quota's record;
//! * `MaxResults` bounds what a CALLER may ask for and not what a listing serves when
//!   none is named, which is that LISTING'S own unasked page — the whole published set
//!   for the default-quota listing, a bounded page for the applied one — with a
//!   `NextToken` after it exactly while items remain, while an asked page size cuts the
//!   same listing into `NextToken`-chained pages that walk to it once.
//!
//! The rows below span the shapes a `ServiceQuota` comes in — adjustable and fixed,
//! counted and measured, rate-shaped and plain, regional and global, with AWS's own
//! `QuotaCode` and with the catalog's derived one — and they cross four services, so
//! a record bound to the wrong row fails here whichever row it is bound to. The
//! harness (`Client`, `walk_pages`) is the one `quota_workflow.rs` owns.

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::APPLIED_QUOTA_PAGE_SIZE;

use crate::quota_workflow::{client, emulator, walk_pages, Client};

/// One published quota, as the catalog (`src/state.rs::seed_quota_catalog`, seeded
/// from AWS's published Service Quotas data) must report it for the pair that names
/// it.
#[derive(Debug)]
struct Row {
    /// The `ServiceCode` the quota belongs to.
    service_code: &'static str,
    /// AWS's own `QuotaCode`, or `None` for a row whose code the catalog derives —
    /// such a row is located by its name and then read back by the code the listing
    /// reports for it, so both kinds of code are exercised.
    quota_code: Option<&'static str>,
    /// `QuotaName`.
    quota_name: &'static str,
    /// `Description`, or `None` for a row that publishes none.
    description: Option<&'static str>,
    /// `Unit`.
    unit: &'static str,
    /// The AWS default `Value`.
    value: f64,
    /// `Adjustable`.
    adjustable: bool,
    /// `GlobalQuota`.
    global_quota: bool,
    /// `Period` as `(PeriodValue, PeriodUnit)`, or `None`.
    period: Option<(i64, &'static str)>,
    /// The `Resource` dimension of `UsageMetric.MetricDimensions` — the value a
    /// client uses verbatim as a CloudWatch dimension — or `None` for a quota that
    /// publishes no usage metric.
    metric_resource: Option<&'static str>,
}

/// The table: one row per quota, drawn from four services and every `ServiceQuota`
/// shape. Source for each row: the catalog in `src/state.rs`.
const ROWS: &[Row] = &[
    // A rate quota AWS publishes as not adjustable, with its own one-second window.
    Row {
        service_code: "lambda",
        quota_code: Some("L-B2AA0F47"),
        quota_name: "Rate of control plane API requests (excludes invocation, GetFunction, and \
                     GetPolicy requests)",
        description: Some(
            "The maximum number of API requests per second (excluding invocation, GetFunction, \
             and GetPolicy requests).",
        ),
        unit: "None",
        value: 15.0,
        adjustable: false,
        global_quota: false,
        period: Some((1, "SECOND")),
        metric_resource: None,
    },
    // A `Count` quota of the same service: the row a code bound positionally would
    // answer with a rate quota's definition for. A concurrency count publishes no
    // rate window.
    Row {
        service_code: "lambda",
        quota_code: Some("L-72E0D058"),
        quota_name: "Number of concurrent MicroVM image builds",
        description: Some("Number of concurrent MicroVM image builds per account per region"),
        unit: "Count",
        value: 10.0,
        adjustable: true,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    Row {
        service_code: "lambda",
        quota_code: Some("L-F55076EC"),
        quota_name: "Rate of capacity provider write API requests",
        description: Some(
            "The maximum combined rate (requests per second) for all capacity provider write \
             APIs.",
        ),
        unit: "None",
        value: 1.0,
        adjustable: false,
        global_quota: false,
        period: Some((1, "SECOND")),
        metric_resource: None,
    },
    // An adjustable rate quota that publishes NO window: a row whose optional
    // members differ from its neighbours'.
    Row {
        service_code: "lambda",
        quota_code: Some("L-B82A30EA"),
        quota_name: "Rate of SendDurableExecutionCallbackSuccess API requests",
        description: Some(
            "The maximum number of SendDurableExecutionCallbackSuccess API requests per second.",
        ),
        unit: "None",
        value: 6000.0,
        adjustable: true,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    // The per-second rate quota of another API of the same service: the same
    // description form around its OWN operation's name, and published as not
    // adjustable over a one-second window.
    Row {
        service_code: "lambda",
        quota_code: Some("L-4273958C"),
        quota_name: "Rate of GetPolicy API requests",
        description: Some("The maximum number of GetPolicy API requests per second."),
        unit: "None",
        value: 15.0,
        adjustable: false,
        global_quota: false,
        period: Some((1, "SECOND")),
        metric_resource: None,
    },
    // The BURST allowance beside such a rate, which is a quota of its own.
    Row {
        service_code: "lambda",
        quota_code: Some("L-25EEC0A4"),
        quota_name: "Burst rate of ResumeMicrovm API requests",
        description: Some(
            "The maximum number of ResumeMicrovm API requests that can be sent in one burst.",
        ),
        unit: "None",
        value: 5.0,
        adjustable: true,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    // A DURATION quota of the same service that is not a rate: no window, not
    // adjustable, and its own published sentence.
    Row {
        service_code: "lambda",
        quota_code: Some("L-B430C318"),
        quota_name: "Max Execution Duration of a MicroVM (in Hours)",
        description: Some("The maximum execution duration of a MicroVM (in Hours)."),
        unit: "None",
        value: 8.0,
        adjustable: false,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    // A MEASURED quota: CloudWatch publishes its usage, so it carries both a usage
    // metric and the window that metric is published over.
    Row {
        service_code: "lambda",
        quota_code: Some("L-B99A9384"),
        quota_name: "Concurrent executions",
        description: Some(
            "The maximum number of function invocations that can run concurrently in this \
             Region.",
        ),
        unit: "None",
        value: 1000.0,
        adjustable: true,
        global_quota: false,
        period: Some((5, "MINUTE")),
        metric_resource: Some("ConcurrentExecutions"),
    },
    // A sized quota: a unit that is neither `None` nor `Count`.
    Row {
        service_code: "lambda",
        quota_code: Some("L-2ACBD22F"),
        quota_name: "Function and layer storage",
        description: Some(
            "The maximum total storage for the .zip deployment packages and layers of this \
             account in this Region.",
        ),
        unit: "Gigabytes",
        value: 75.0,
        adjustable: true,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    // A row the catalog has no AWS `QuotaCode` for: it resolves by the derived code
    // the listing reports, and its definition is its own all the same.
    Row {
        service_code: "lambda",
        quota_code: None,
        quota_name: "Function timeout",
        description: Some(
            "The maximum amount of time that a function can run before Lambda stops it.",
        ),
        unit: "Seconds",
        value: 900.0,
        adjustable: false,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    // The measured quota of another service, whose usage-metric `Resource`
    // dimension is NOT its resource type: a client queries CloudWatch with this
    // exact value.
    Row {
        service_code: "vpc",
        quota_code: Some("L-F678F1CE"),
        quota_name: "VPCs per Region",
        description: Some(
            "The maximum number of VPCs per Region. This quota is directly tied to the maximum \
             number of internet gateways per Region.",
        ),
        unit: "None",
        value: 5.0,
        adjustable: true,
        global_quota: false,
        period: Some((5, "MINUTE")),
        metric_resource: Some("VPCsPerRegion"),
    },
    // A fixed quota that publishes no description at all: an absent member stays
    // absent rather than borrowing a neighbour's text.
    Row {
        service_code: "apigateway",
        quota_code: Some("L-8C2F9A1D"),
        quota_name: "Throttle burst rate",
        description: None,
        unit: "None",
        value: 5000.0,
        adjustable: false,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    Row {
        service_code: "dynamodb",
        quota_code: Some("L-F98FE922"),
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
        metric_resource: Some("TableCount"),
    },
    Row {
        service_code: "ec2",
        quota_code: Some("L-0263D0A3"),
        quota_name: "EC2-VPC Elastic IPs",
        description: None,
        unit: "None",
        value: 5.0,
        adjustable: true,
        global_quota: false,
        period: None,
        metric_resource: None,
    },
    // A GLOBAL quota: counted account-wide rather than per region.
    Row {
        service_code: "iam",
        quota_code: None,
        quota_name: "Roles per account",
        description: None,
        unit: "None",
        value: 1000.0,
        adjustable: true,
        global_quota: true,
        period: None,
        metric_resource: None,
    },
];

/// The services the page-size rule is pinned over: `iam` publishes a handful of
/// quotas, `lambda` and `vpc` a few dozen, `ec2` hundreds.
const PAGED_SERVICES: &[&str] = &["iam", "lambda", "vpc", "ec2"];

/// The `Resource` dimension of a record's usage metric, when it publishes one.
fn metric_resource(record: &Value) -> Option<&str> {
    record["UsageMetric"]["MetricDimensions"]["Resource"].as_str()
}

/// The `(PeriodValue, PeriodUnit)` a record publishes, when it publishes a window.
fn period(record: &Value) -> Option<(i64, &str)> {
    let period = record["Period"].as_object()?;
    Some((
        period.get("PeriodValue").and_then(Value::as_i64)?,
        period.get("PeriodUnit").and_then(Value::as_str)?,
    ))
}

/// The `QuotaCode` the listing reports for `quota_name`, failing when the service
/// publishes no quota of that name.
async fn code_of(client: &Client, service_code: &str, quota_name: &str) -> String {
    let listed = walk_pages(
        client,
        "ListServiceQuotas",
        &json!({"ServiceCode": service_code}),
        "Quotas",
        None,
    )
    .await;
    let matching: Vec<&Value> = listed
        .iter()
        .filter(|quota| quota["QuotaName"].as_str() == Some(quota_name))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {service_code}/{quota_name}: one quota of a service carries one name, got {} \
         ({matching:?})",
        matching.len()
    );
    matching[0]["QuotaCode"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {service_code}/{quota_name}: a listed quota carries a QuotaCode")
        })
        .to_string()
}

/// Assert `record` is the row `expected` describes, whichever read produced it.
fn assert_is_row(case: &str, record: &Value, expected: &Row, quota_code: &str) {
    assert_eq!(
        record["QuotaCode"].as_str(),
        Some(quota_code),
        "case {case}: expected the record of {quota_code}, got {record}"
    );
    assert_eq!(
        record["ServiceCode"].as_str(),
        Some(expected.service_code),
        "case {case}: expected ServiceCode {}, got {record}",
        expected.service_code
    );
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
        metric_resource(record),
        expected.metric_resource,
        "case {case}: expected the usage metric's Resource dimension {:?}, got {:?}",
        expected.metric_resource,
        record["UsageMetric"]
    );
}

/// RULE: a `QuotaCode` resolves to the quota it names — every read path answers for
/// the pair asked for with that quota's own published definition.
#[tokio::test]
async fn a_quota_code_resolves_to_its_own_definition_on_every_read_path() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    for expected in ROWS {
        let quota_code = match expected.quota_code {
            Some(code) => code.to_string(),
            None => code_of(&client, expected.service_code, expected.quota_name).await,
        };
        let case = format!("{}/{quota_code}", expected.service_code);
        let pair = json!({"ServiceCode": expected.service_code, "QuotaCode": quota_code});

        // The applied read and the AWS-default read: nothing has been applied here,
        // so both report the published row.
        for operation in ["GetServiceQuota", "GetAWSDefaultServiceQuota"] {
            let record = client.ok(operation, pair.clone()).await["Quota"].clone();
            assert_is_row(&format!("{case} {operation}"), &record, expected, &quota_code);
        }

        // The listing filtered to the one code, and the whole walked listing: the
        // same record, found BY its code rather than by its position.
        let filtered = client.ok("ListServiceQuotas", pair.clone()).await["Quotas"][0].clone();
        assert_is_row(
            &format!("{case} ListServiceQuotas(QuotaCode)"),
            &filtered,
            expected,
            &quota_code,
        );

        for (operation, what) in [
            ("ListServiceQuotas", "applied"),
            ("ListAWSDefaultServiceQuotas", "default"),
        ] {
            let listed = walk_pages(
                &client,
                operation,
                &json!({"ServiceCode": expected.service_code}),
                "Quotas",
                None,
            )
            .await;
            let found: Vec<&Value> = listed
                .iter()
                .filter(|quota| quota["QuotaCode"].as_str() == Some(quota_code.as_str()))
                .collect();
            assert_eq!(
                found.len(),
                1,
                "case {case} {operation}: the {what} listing must carry {quota_code} exactly \
                 once, got {} times",
                found.len()
            );
            assert_is_row(
                &format!("{case} {operation}"),
                found[0],
                expected,
                &quota_code,
            );

            // No OTHER code of the service answers with this row's definition: a
            // name belongs to the one code that publishes it.
            let namesakes: Vec<&Value> = listed
                .iter()
                .filter(|quota| {
                    quota["QuotaName"].as_str() == Some(expected.quota_name)
                        && quota["QuotaCode"].as_str() != Some(quota_code.as_str())
                })
                .collect();
            assert!(
                namesakes.is_empty(),
                "case {case} {operation}: {:?} must belong to {quota_code} alone, also found on \
                 {namesakes:?}",
                expected.quota_name
            );
        }
    }
}

/// RULE: a `(ServiceCode, QuotaCode)` pair the catalog does not publish is REFUSED
/// with the modelled `NoSuchResourceException` — never answered with the record of
/// some other quota.
#[tokio::test]
async fn a_pair_the_catalog_does_not_publish_is_refused_rather_than_substituted() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: a code another service publishes, a code of this service's own shape
    // that nothing publishes, a published code in the wrong case, and a code whose
    // service publishes quotas under a different code entirely.
    let absent = [
        ("lambda", "L-F678F1CE", "vpc's VPCs-per-Region code under lambda"),
        ("vpc", "L-B2AA0F47", "lambda's control-plane rate code under vpc"),
        ("lambda", "L-00000000", "a well-formed code nothing publishes"),
        ("lambda", "l-b2aa0f47", "a published code in the wrong case"),
        ("dynamodb", "L-0263D0A3", "an EC2 code under dynamodb"),
        ("iam", "L-F98FE922", "a DynamoDB code under iam"),
    ];

    for (service_code, quota_code, why) in absent {
        let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
        for operation in [
            "GetServiceQuota",
            "GetAWSDefaultServiceQuota",
            "ListServiceQuotas",
        ] {
            let (status, body) = client.call(operation, &pair).await;
            assert_ne!(
                status, 200,
                "case {why} ({operation} {service_code}/{quota_code}): an absent pair must not \
                 be answered, got {body}"
            );
            let code = body["__type"].as_str().unwrap_or("<no __type>");
            assert!(
                code.ends_with("NoSuchResourceException"),
                "case {why} ({operation} {service_code}/{quota_code}): expected \
                 NoSuchResourceException, got {status} {body}"
            );
            assert!(
                body["Quota"].is_null() && body["Quotas"].is_null(),
                "case {why} ({operation} {service_code}/{quota_code}): a refusal carries no \
                 quota record, got {body}"
            );
        }
    }
}

/// RULE: `MaxResults` bounds what a CALLER may ask for, not what a listing of this
/// service serves when the caller names none — which is that LISTING'S own unasked page:
/// the whole published set for the default-quota listing, and
/// `APPLIED_QUOTA_PAGE_SIZE` records for the applied one. Either way a `NextToken`
/// follows a page exactly while items remain, and it resumes exactly where the page
/// stopped.
#[tokio::test]
async fn an_unasked_quota_listing_serves_its_own_page_and_an_asked_page_resumes() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: the quota listings of this service, over item sets that are shorter than
    // the smallest asked page but one (iam's few quotas), a few dozen long (lambda,
    // vpc) and hundreds long (ec2's quotas).
    let mut listings: Vec<(&str, Value, &str, usize)> = Vec::new();
    for service_code in PAGED_SERVICES {
        listings.push((
            "ListServiceQuotas",
            json!({"ServiceCode": service_code}),
            "Quotas",
            APPLIED_QUOTA_PAGE_SIZE,
        ));
        listings.push((
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": service_code}),
            "Quotas",
            usize::MAX,
        ));
    }

    let mut shorter_than_the_largest_page = 0_usize;
    let mut longer_than_the_largest_page = 0_usize;

    for (operation, base, items, unasked_limit) in listings {
        let case = format!("{operation} {base}");
        // The published listing, walked one page at a time with the tokens the
        // service itself issues for a caller-named page size.
        let whole = walk_pages(&client, operation, &base, items, Some(100)).await;
        assert!(
            !whole.is_empty(),
            "case {case}: expected a non-empty listing, got {whole:?}"
        );
        if whole.len() > 100 {
            longer_than_the_largest_page += 1;
        } else {
            shorter_than_the_largest_page += 1;
        }

        let first = client.ok(operation, base.clone()).await;
        let page = first[items].as_array().cloned().unwrap_or_default();
        let served = unasked_limit.min(whole.len());
        assert_eq!(
            page.len(),
            served,
            "case {case}: an unasked listing carries the first {served} of its {} items, got {}",
            whole.len(),
            page.len()
        );
        assert_eq!(
            page,
            whole[..served],
            "case {case}: the unasked page must be the first items of the published listing"
        );
        assert_eq!(
            !first["NextToken"].as_str().unwrap_or_default().is_empty(),
            served < whole.len(),
            "case {case}: a NextToken follows the unasked page exactly while items remain \
             ({served} of {} served), got {first}",
            whole.len()
        );

        // A page a CALLER cut short is followed by a token that resumes exactly where
        // it stopped: no item repeated, none skipped.
        for size in [1_usize, 3] {
            if whole.len() <= size {
                continue;
            }
            let mut asked = base.clone();
            asked["MaxResults"] = json!(size);
            let short = client.ok(operation, asked.clone()).await;
            let token = short["NextToken"].as_str().unwrap_or_default().to_string();
            assert!(
                !token.is_empty(),
                "case {case} MaxResults={size}: {} items remain after this page, so a NextToken \
                 must follow it, got {short}",
                whole.len() - size
            );
            let mut resume = base.clone();
            resume["NextToken"] = json!(token);
            resume["MaxResults"] = json!(size);
            let resumed = client.ok(operation, resume).await;
            let second = resumed[items].as_array().cloned().unwrap_or_default();
            let stop = (size * 2).min(whole.len());
            assert_eq!(
                second,
                whole[size..stop],
                "case {case} MaxResults={size}: the token must resume at item {size}, got {resumed}"
            );
        }

        // Asked page sizes across the modelled 1..100 range yield the same listing.
        // The one-item walk is skipped for a listing of hundreds of items, which the
        // other sizes already cover.
        for size in [1_i64, 3, 11, 100] {
            if size == 1 && whole.len() > 100 {
                continue;
            }
            let paged = walk_pages(&client, operation, &base, items, Some(size)).await;
            assert_eq!(
                paged, whole,
                "case {case} MaxResults={size}: paging must yield the published listing"
            );
        }
    }

    assert!(
        shorter_than_the_largest_page >= 1 && longer_than_the_largest_page >= 1,
        "case catalog: the rule must be exercised on a listing shorter than the largest page a \
         caller may ask for and on one longer than it, got {shorter_than_the_largest_page} and \
         {longer_than_the_largest_page}"
    );
}
