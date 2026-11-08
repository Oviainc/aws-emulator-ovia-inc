// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the cost anomaly monitor registry (`CreateAnomalyMonitor`,
//! `GetAnomalyMonitors`).
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`], so nothing here reaches past the HTTP surface a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::state::{
    CUSTOM_MONITOR_DIMENSION_KEY, MAX_ANOMALY_MONITORS, MONITOR_TYPE_CUSTOM,
    MONITOR_TYPE_DIMENSIONAL,
};

use crate::harness::{error_code, Emulator};

/// Every value the modelled `MonitorDimension` enum carries
/// (`smithy_query(query="shape", shape="MonitorDimension")`), with the
/// `MonitorSpecification` each one needs, from `AnomalyMonitor$MonitorSpecification`:
/// `SERVICE` and `LINKED_ACCOUNT` take none, `TAG` takes a tag key and `COST_CATEGORY` a
/// cost category key.
fn managed_monitor_cases() -> Vec<(&'static str, &'static str, Option<Value>)> {
    vec![
        ("a managed monitor over services", "SERVICE", None),
        (
            "a managed monitor over linked accounts",
            "LINKED_ACCOUNT",
            None,
        ),
        (
            "a managed monitor over a tag key",
            "TAG",
            Some(json!({"Tags": {"Key": "CostCenter"}})),
        ),
        (
            "a managed monitor over a cost category key",
            "COST_CATEGORY",
            Some(json!({"CostCategories": {"Key": "Department"}})),
        ),
    ]
}

/// The three `MonitorSpecification` forms a customer managed monitor is documented to
/// take (`AnomalyMonitor$MonitorSpecification`): linked accounts, cost allocation tags and
/// cost categories.
fn custom_monitor_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a custom monitor tracking linked accounts",
            json!({
                "Dimensions": {
                    "Key": CUSTOM_MONITOR_DIMENSION_KEY,
                    "Values": [engine::emulator_account_id()],
                }
            }),
        ),
        (
            "a custom monitor tracking cost allocation tags",
            json!({"Tags": {"Key": "Team", "Values": ["platform", "data"]}}),
        ),
        (
            "a custom monitor tracking cost categories",
            json!({"CostCategories": {"Key": "Department", "Values": ["Engineering"]}}),
        ),
    ]
}

/// A `CreateAnomalyMonitor` body for a managed monitor.
fn managed_body(name: &str, dimension: &str, specification: Option<Value>) -> Value {
    let mut monitor = json!({
        "MonitorName": name,
        "MonitorType": MONITOR_TYPE_DIMENSIONAL,
        "MonitorDimension": dimension,
    });
    if let Some(specification) = specification {
        monitor["MonitorSpecification"] = specification;
    }
    json!({"AnomalyMonitor": monitor})
}

/// A `CreateAnomalyMonitor` body for a customer managed monitor.
fn custom_body(name: &str, specification: Value) -> Value {
    json!({
        "AnomalyMonitor": {
            "MonitorName": name,
            "MonitorType": MONITOR_TYPE_CUSTOM,
            "MonitorSpecification": specification,
        }
    })
}

/// The ARN a successful create answered with.
fn arn_of(created: &Value) -> String {
    created["MonitorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("MonitorArn must be present, got {created}"))
        .to_string()
}

/// Every monitor the account holds, in the order `GetAnomalyMonitors` reports them.
async fn all_monitors(emulator: &Emulator) -> Vec<Value> {
    emulator.ok("GetAnomalyMonitors", &json!({})).await["AnomalyMonitors"]
        .as_array()
        .cloned()
        .unwrap_or_else(Vec::new)
}

/// Assert `value` is the `YearMonthDay` form this service renders a monitor date in:
/// `YYYY-MM-DDThh:mm:ssZ`, which the modelled pattern
/// `^(\d{4}-\d{2}-\d{2})(T\d{2}:\d{2}:\d{2}Z)?$` admits.
fn assert_year_month_day(label: &str, value: &str) {
    assert_eq!(
        value.len(),
        20,
        "case {label:?}: a rendered YearMonthDay with a time of day is 20 characters, got \
         {value:?}"
    );
    let bytes = value.as_bytes();
    for (index, expected) in [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ] {
        assert_eq!(
            bytes[index],
            expected,
            "case {label:?}: expected {:?} at index {index} of {value:?}",
            char::from(expected)
        );
    }
    for index in [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18] {
        assert!(
            bytes[index].is_ascii_digit(),
            "case {label:?}: expected a digit at index {index} of {value:?}"
        );
    }
}

/// Assert a minted monitor ARN is the one AWS shape: `arn:aws:ce::<account>:anomalymonitor/`
/// plus the 8-4-4-4-12 lowercase-hex id those ARNs carry.
fn assert_monitor_arn(label: &str, arn: &str) {
    let expected_prefix = format!(
        "arn:aws:ce::{}:anomalymonitor/",
        engine::emulator_account_id()
    );
    let id = arn.strip_prefix(&expected_prefix).unwrap_or_else(|| {
        panic!("case {label:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}")
    });
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {label:?}: expected an 8-4-4-4-12 id, got {id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())),
        "case {label:?}: expected lowercase hex in the id, got {id:?}"
    );
}

/// RULE: `CreateAnomalyMonitor` mints the monitor's ARN, persists the monitor as the
/// caller described it, and `GetAnomalyMonitors` reports that stored monitor — the
/// caller's `MonitorName`, `MonitorType`, `MonitorDimension` and `MonitorSpecification`
/// unchanged, with the service-owned dates read off its own clock.
///
/// The table spans the class of monitors the model admits: every `MonitorDimension` value
/// a managed monitor can take (two that need no specification, two configured by a key)
/// and every specification form a customer managed monitor is documented to take.
#[tokio::test]
async fn a_created_monitor_is_reported_as_it_was_described() {
    let emulator = Emulator::start();

    let mut cases: Vec<(String, Value)> = Vec::new();
    for (label, dimension, specification) in managed_monitor_cases() {
        cases.push((
            label.to_string(),
            managed_body(&format!("monitor-{dimension}"), dimension, specification),
        ));
    }
    for (index, (label, specification)) in custom_monitor_cases().into_iter().enumerate() {
        cases.push((
            label.to_string(),
            custom_body(&format!("custom-monitor-{index}"), specification),
        ));
    }

    let mut minted: Vec<String> = Vec::new();
    for (label, body) in &cases {
        let created = emulator.ok("CreateAnomalyMonitor", body).await;
        let arn = arn_of(&created);
        assert_monitor_arn(label, &arn);
        assert!(
            !minted.contains(&arn),
            "case {label:?}: every monitor gets its own ARN, got a repeat of {arn:?}"
        );
        minted.push(arn);
    }

    let reported = all_monitors(&emulator).await;
    assert_eq!(
        reported.len(),
        cases.len(),
        "every created monitor must be reported, got {reported:?}"
    );

    for ((label, body), arn) in cases.iter().zip(&minted) {
        let sent = &body["AnomalyMonitor"];
        let monitor = reported
            .iter()
            .find(|monitor| monitor["MonitorArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!("case {label:?}: expected the monitor {arn:?} to be reported")
            });
        for member in ["MonitorName", "MonitorType", "MonitorDimension"] {
            assert_eq!(
                monitor[member], sent[member],
                "case {label:?}: {member} must round-trip, got {monitor}"
            );
        }
        assert_eq!(
            monitor["MonitorSpecification"], sent["MonitorSpecification"],
            "case {label:?}: MonitorSpecification must round-trip, got {monitor}"
        );
        let creation_date = monitor["CreationDate"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: CreationDate must be present"));
        assert_year_month_day(label, creation_date);
        assert_eq!(
            monitor["LastUpdatedDate"].as_str(),
            Some(creation_date),
            "case {label:?}: a monitor nobody updated was last written when it was created, \
             got {monitor}"
        );
        assert!(
            monitor.get("LastEvaluatedDate").is_none(),
            "case {label:?}: nothing has evaluated the monitor, so LastEvaluatedDate must be \
             omitted, got {monitor}"
        );
        // The member's modelled default is 0, which the wire form may carry explicitly or
        // omit; both read as "no dimension value has been evaluated".
        assert_eq!(
            monitor["DimensionalValueCount"].as_i64().unwrap_or(0),
            0,
            "case {label:?}: no cost data has been evaluated, got {monitor}"
        );
    }
}

/// RULE: the members the SERVICE owns are minted by the service, so a caller cannot set
/// them — a create that fills `MonitorArn`, the dates and `DimensionalValueCount` is
/// answered with the service's own values.
#[tokio::test]
async fn the_service_owns_the_arn_the_dates_and_the_value_count() {
    let emulator = Emulator::start();
    let claimed_arn = "arn:aws:ce::999999999999:anomalymonitor/claimed-by-the-caller";
    let created = emulator
        .ok(
            "CreateAnomalyMonitor",
            &json!({
                "AnomalyMonitor": {
                    "MonitorArn": claimed_arn,
                    "MonitorName": "caller-supplied-metadata",
                    "MonitorType": MONITOR_TYPE_DIMENSIONAL,
                    "MonitorDimension": "SERVICE",
                    "CreationDate": "1999-01-01T00:00:00Z",
                    "LastUpdatedDate": "1999-01-02T00:00:00Z",
                    "LastEvaluatedDate": "1999-01-03T00:00:00Z",
                    "DimensionalValueCount": 4321,
                }
            }),
        )
        .await;
    let arn = arn_of(&created);
    assert_ne!(
        arn, claimed_arn,
        "the ARN must be minted by the service, got {arn:?}"
    );
    assert_monitor_arn("caller-supplied metadata", &arn);

    let reported = all_monitors(&emulator).await;
    assert_eq!(
        reported.len(),
        1,
        "exactly the one created monitor must be reported, got {reported:?}"
    );
    let monitor = &reported[0];
    assert_eq!(
        monitor["MonitorArn"].as_str(),
        Some(arn.as_str()),
        "the reported ARN must be the minted one, got {monitor}"
    );
    for member in ["CreationDate", "LastUpdatedDate"] {
        let value = monitor[member]
            .as_str()
            .unwrap_or_else(|| panic!("{member} must be present, got {monitor}"));
        assert!(
            !value.starts_with("1999"),
            "case {member:?}: the date must be the service's own clock reading, got {value:?}"
        );
    }
    assert!(
        monitor.get("LastEvaluatedDate").is_none(),
        "LastEvaluatedDate must stay absent, got {monitor}"
    );
    assert_eq!(
        monitor["DimensionalValueCount"].as_i64().unwrap_or(0),
        0,
        "DimensionalValueCount must be the service's own count, got {monitor}"
    );
}

/// RULE: a monitor whose `MonitorType`, `MonitorDimension` and `MonitorSpecification` do
/// not hold together as those members document is refused 400 `ValidationException`, and
/// nothing is persisted.
///
/// The table spans the class: a managed monitor missing its dimension, a managed monitor
/// carrying a specification for each of the two dimensions that take none, a managed
/// monitor missing the key each of the other two dimensions is configured by (absent
/// specification, wrong selector and an empty key), and a customer managed monitor
/// naming a dimension, carrying no specification, carrying an empty one, or selecting a
/// dimension other than the documented one.
#[tokio::test]
async fn a_monitor_whose_members_contradict_each_other_is_refused() {
    let cases = vec![
        (
            "a managed monitor with no dimension",
            json!({"AnomalyMonitor": {
                "MonitorName": "no-dimension",
                "MonitorType": MONITOR_TYPE_DIMENSIONAL,
            }}),
        ),
        (
            "a managed service monitor carrying a specification",
            managed_body(
                "service-with-spec",
                "SERVICE",
                Some(json!({"Dimensions": {"Key": "SERVICE", "Values": ["AmazonS3"]}})),
            ),
        ),
        (
            "a managed linked-account monitor carrying a specification",
            managed_body(
                "accounts-with-spec",
                "LINKED_ACCOUNT",
                Some(json!({"Dimensions": {
                    "Key": CUSTOM_MONITOR_DIMENSION_KEY,
                    "Values": [engine::emulator_account_id()],
                }})),
            ),
        ),
        (
            "a managed tag monitor with no specification",
            managed_body("tag-without-spec", "TAG", None),
        ),
        (
            "a managed tag monitor whose specification names the wrong selector",
            managed_body(
                "tag-with-cost-categories",
                "TAG",
                Some(json!({"CostCategories": {"Key": "Department"}})),
            ),
        ),
        (
            "a managed tag monitor whose specification carries an empty key",
            managed_body(
                "tag-with-empty-key",
                "TAG",
                Some(json!({"Tags": {"Key": ""}})),
            ),
        ),
        (
            "a managed cost-category monitor with no specification",
            managed_body("cost-category-without-spec", "COST_CATEGORY", None),
        ),
        (
            "a managed cost-category monitor whose specification names the wrong selector",
            managed_body(
                "cost-category-with-tags",
                "COST_CATEGORY",
                Some(json!({"Tags": {"Key": "CostCenter"}})),
            ),
        ),
        (
            "a custom monitor with no specification",
            json!({"AnomalyMonitor": {
                "MonitorName": "custom-without-spec",
                "MonitorType": MONITOR_TYPE_CUSTOM,
            }}),
        ),
        (
            "a custom monitor naming a dimension",
            json!({"AnomalyMonitor": {
                "MonitorName": "custom-with-dimension",
                "MonitorType": MONITOR_TYPE_CUSTOM,
                "MonitorDimension": "SERVICE",
                "MonitorSpecification": {"Dimensions": {
                    "Key": CUSTOM_MONITOR_DIMENSION_KEY,
                    "Values": [engine::emulator_account_id()],
                }},
            }}),
        ),
        (
            "a custom monitor whose specification selects nothing",
            custom_body("custom-with-empty-spec", json!({})),
        ),
        (
            "a custom monitor selecting a dimension it cannot track",
            custom_body(
                "custom-with-service-dimension",
                json!({"Dimensions": {"Key": "SERVICE", "Values": ["AmazonS3"]}}),
            ),
        ),
    ];

    let emulator = Emulator::start();
    for (label, body) in &cases {
        let (status, answered) = emulator.call("CreateAnomalyMonitor", body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected a 400 ValidationException, got {status}: {answered}"
        );
    }
    assert!(
        all_monitors(&emulator).await.is_empty(),
        "a refused create must persist nothing, got {:?}",
        all_monitors(&emulator).await
    );
}

/// RULE: the account holds at most [`MAX_ANOMALY_MONITORS`] monitors, and a create past
/// that is refused with the failure the operation models rather than stored.
#[tokio::test]
async fn a_create_past_the_monitor_limit_is_refused() {
    let emulator = Emulator::start();
    for index in 0..MAX_ANOMALY_MONITORS {
        emulator
            .ok(
                "CreateAnomalyMonitor",
                &managed_body(&format!("at-the-limit-{index}"), "SERVICE", None),
            )
            .await;
    }
    // Twice over: the limit holds for every later call, not only the first one past it.
    for attempt in 0..2 {
        let (status, answered) = emulator
            .call(
                "CreateAnomalyMonitor",
                &managed_body("over-the-limit", "SERVICE", None),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "LimitExceededException"),
            "case {attempt:?}: expected a 400 LimitExceededException, got {status}: {answered}"
        );
    }
    assert_eq!(
        all_monitors(&emulator).await.len(),
        MAX_ANOMALY_MONITORS,
        "a refused create must persist nothing"
    );
}

/// RULE: `GetAnomalyMonitors` reports the monitors the account holds, filtered to
/// `MonitorArnList` when one is given, and pages at `MaxResults` with a `NextPageToken`
/// that resumes exactly where the previous page stopped.
///
/// The table spans the page sizes the result can be cut at: one monitor per page, a size
/// that divides the result, one that does not, exactly the result size and more than it.
#[tokio::test]
async fn monitors_are_filtered_by_arn_and_paged_by_max_results() {
    let emulator = Emulator::start();
    let mut minted: Vec<String> = Vec::new();
    for index in 0..6 {
        let created = emulator
            .ok(
                "CreateAnomalyMonitor",
                &managed_body(&format!("paged-{index}"), "SERVICE", None),
            )
            .await;
        minted.push(arn_of(&created));
    }
    // The registry is keyed on the ARN, so that is the order a page walks.
    let mut expected_order = minted.clone();
    expected_order.sort();

    let reported: Vec<String> = all_monitors(&emulator)
        .await
        .iter()
        .map(|monitor| monitor["MonitorArn"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, expected_order,
        "an unpaged read must report every monitor in ARN order"
    );

    for page_size in [1_usize, 2, 4, 6, 10] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextPageToken"] = json!(token);
            }
            let answered = emulator.ok("GetAnomalyMonitors", &request).await;
            let page = answered["AnomalyMonitors"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {page_size:?}: a page must hold at most MaxResults monitors, got \
                 {} in {answered}",
                page.len()
            );
            for monitor in &page {
                walked.push(
                    monitor["MonitorArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= expected_order.len() + 1,
                "case {page_size:?}: the walk must terminate, got {pages} pages"
            );
            match answered["NextPageToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, expected_order,
            "case {page_size:?}: paging must report every monitor exactly once, in order"
        );
    }

    // The filter: a subset, in any order, reports exactly those monitors.
    let selected = vec![expected_order[4].clone(), expected_order[1].clone()];
    let filtered = emulator
        .ok(
            "GetAnomalyMonitors",
            &json!({"MonitorArnList": selected.clone()}),
        )
        .await;
    let filtered_arns: Vec<String> = filtered["AnomalyMonitors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|monitor| monitor["MonitorArn"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected_filtered = selected.clone();
    expected_filtered.sort();
    assert_eq!(
        filtered_arns, expected_filtered,
        "the filter must report exactly the monitors it names"
    );

    // A filter naming no ARN filters nothing out.
    let unfiltered = emulator
        .ok("GetAnomalyMonitors", &json!({"MonitorArnList": []}))
        .await;
    assert_eq!(
        unfiltered["AnomalyMonitors"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        expected_order.len(),
        "an empty MonitorArnList must filter nothing out, got {unfiltered}"
    );
}

/// RULE: the two members `GetAnomalyMonitors` can be asked wrongly about are each refused
/// with the failure the model declares for it — a cursor this service never issued is 400
/// `InvalidNextTokenException`, and a page size below one is a 400 `ValidationException`.
///
/// `MonitorArnList` is not one of them: it is a filter, so an ARN the account holds no
/// monitor under is selected out rather than refused — asserted in
/// [`crate::unknown_monitor_references`].
#[tokio::test]
async fn a_read_asked_wrongly_is_refused() {
    let emulator = Emulator::start();
    emulator
        .ok(
            "CreateAnomalyMonitor",
            &managed_body("the-only-monitor", "SERVICE", None),
        )
        .await;

    for (label, token) in [
        ("a cursor that is not hex", "not-a-token"),
        ("a cursor with an odd number of hex digits", "abc"),
    ] {
        let (status, answered) = emulator
            .call("GetAnomalyMonitors", &json!({"NextPageToken": token}))
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidNextTokenException"),
            "case {label:?}: expected a 400 InvalidNextTokenException, got {status}: {answered}"
        );
    }

    for page_size in [0, -1] {
        let (status, answered) = emulator
            .call("GetAnomalyMonitors", &json!({"MaxResults": page_size}))
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {page_size:?}: expected a 400 ValidationException, got {status}: {answered}"
        );
    }
}
