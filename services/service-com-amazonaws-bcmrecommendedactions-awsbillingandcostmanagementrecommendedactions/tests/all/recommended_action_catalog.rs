// SPDX-License-Identifier: CC0-1.0

//! What the inventory the emulator starts with reports, over the real awsJson1_0
//! wire: `POST /` with
//! `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions`.
//!
//! The service models no create, update or delete operation — AWS computes the
//! recommendations itself — so the inventory the store starts with IS the data
//! contract a client reads, and two rules hold over it:
//!
//! * every record reports the `(feature, severity)` pair the service publishes for
//!   its action type: the pairing is a property of the service's action-type
//!   catalog, not of account data, so it is the same for every record of a type;
//! * a recommendation is reported exactly when the account is in the condition that
//!   produces it, so the inventory carries the baseline recommendations of an
//!   account in good standing with nothing configured — `CREATE_BUDGET` among them —
//!   and none that presuppose a condition the account is not in;
//! * `EQUALS` and `NOT_EQUALS` are complements over whatever is held: for every
//!   modelled filter value the two sides are disjoint and together exactly the
//!   unfiltered listing, and the `EQUALS` side is non-empty for every value the
//!   inventory carries.
//!
//! All three are asserted over the whole modelled value space, not over one value:
//! the `CATALOG` table spans all twenty action types, `BASELINE`/`RULED_OUT` together
//! name every one of them with the condition that decides it, and the partition cases
//! sweep every member of `ActionType`, `Feature` and `Severity`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions as bcm;

use bcm::state::{ActionType, Feature, Severity};

/// The awsJson1_0 target this service answers.
const TARGET: &str = "AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions";

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name (`bcm-recommended-actions`, per the crate's OPERATIONS.md) — the
/// routing signal the router reads. The signature is not checked: request
/// authentication is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261029/us-east-1/bcm-recommended-actions/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The published action-type catalog, written out INDEPENDENTLY of the production
/// table so a wrong pair in either one is visible: `(type, feature, severity)` for
/// every member of `ActionType` (members from
/// `smithy_query(query="shape", shape="ActionType")`; the feature and severity each
/// type is published with come from the AWS Billing and Cost Management
/// recommended-actions catalog, which fixes one feature and one severity tier per
/// type).
const CATALOG: &[(ActionType, Feature, Severity)] = &[
    (
        ActionType::AddAlternateBillingContact,
        Feature::Account,
        Severity::Info,
    ),
    (
        ActionType::CreateAnomalyMonitor,
        Feature::CostAnomalyDetection,
        Severity::Info,
    ),
    (ActionType::CreateBudget, Feature::Budgets, Severity::Info),
    (
        ActionType::EnableCostOptimizationHub,
        Feature::CostOptimizationHub,
        Severity::Info,
    ),
    (
        ActionType::MigrateToGranularPermissions,
        Feature::Iam,
        Severity::Warning,
    ),
    (ActionType::PaymentsDue, Feature::Payments, Severity::Info),
    (
        ActionType::PaymentsPastDue,
        Feature::Payments,
        Severity::Critical,
    ),
    (
        ActionType::ReviewAnomalies,
        Feature::CostAnomalyDetection,
        Severity::Warning,
    ),
    (
        ActionType::ReviewBudgetAlerts,
        Feature::Budgets,
        Severity::Warning,
    ),
    (
        ActionType::ReviewBudgetsExceeded,
        Feature::Budgets,
        Severity::Warning,
    ),
    (
        ActionType::ReviewExpiringRi,
        Feature::Reservations,
        Severity::Warning,
    ),
    (
        ActionType::ReviewExpiringSp,
        Feature::SavingsPlans,
        Severity::Warning,
    ),
    (
        ActionType::ReviewFreetierUsageAlerts,
        Feature::FreeTier,
        Severity::Warning,
    ),
    (
        ActionType::ReviewFreetierCreditsRemaining,
        Feature::FreeTier,
        Severity::Info,
    ),
    (
        ActionType::ReviewFreetierDaysRemaining,
        Feature::FreeTier,
        Severity::Info,
    ),
    (
        ActionType::ReviewSavingsOpportunityRecommendations,
        Feature::CostOptimizationHub,
        Severity::Info,
    ),
    (
        ActionType::UpdateExpiredPaymentMethod,
        Feature::Payments,
        Severity::Critical,
    ),
    (
        ActionType::UpdateInvalidPaymentMethod,
        Feature::Payments,
        Severity::Critical,
    ),
    (
        ActionType::UpdateTaxExemptionCertificate,
        Feature::TaxSettings,
        Severity::Warning,
    ),
    (
        ActionType::UpdateTaxRegistrationNumber,
        Feature::TaxSettings,
        Severity::Warning,
    ),
];

/// The conditions an account in good standing with nothing configured IS in, so the
/// recommendation each one produces must be reported. The second column is the
/// condition, which is what makes the row a statement rather than a copy of the
/// inventory.
const BASELINE: &[(ActionType, &str)] = &[
    (
        ActionType::AddAlternateBillingContact,
        "no alternate billing contact is on the account",
    ),
    (
        ActionType::CreateAnomalyMonitor,
        "no cost anomaly monitor is configured",
    ),
    (ActionType::CreateBudget, "no budget exists"),
    (
        ActionType::EnableCostOptimizationHub,
        "Cost Optimization Hub is not enabled",
    ),
    (
        ActionType::MigrateToGranularPermissions,
        "the billing policies still name the coarse billing permissions",
    ),
    (
        ActionType::ReviewExpiringRi,
        "Reserved Instances the account holds expire soon",
    ),
    (
        ActionType::ReviewExpiringSp,
        "Savings Plans the account holds expire soon",
    ),
    (
        ActionType::ReviewSavingsOpportunityRecommendations,
        "the account's usage carries savings opportunities",
    ),
];

/// The conditions that account is NOT in, so no record of these types may be
/// reported: reporting one would tell a client the account is in a state it is not.
const RULED_OUT: &[(ActionType, &str)] = &[
    (ActionType::PaymentsDue, "no invoice is due"),
    (ActionType::PaymentsPastDue, "no invoice is past due"),
    (
        ActionType::UpdateExpiredPaymentMethod,
        "the payment method on file has not expired",
    ),
    (
        ActionType::UpdateInvalidPaymentMethod,
        "the payment method on file is valid",
    ),
    (
        ActionType::ReviewAnomalies,
        "no anomaly monitor exists, so no anomaly has been detected",
    ),
    (
        ActionType::ReviewBudgetAlerts,
        "no budget exists, so no budget alert has been triggered",
    ),
    (
        ActionType::ReviewBudgetsExceeded,
        "no budget exists, so none can be exceeded",
    ),
    (
        ActionType::ReviewFreetierUsageAlerts,
        "the account is not on Free Tier",
    ),
    (
        ActionType::ReviewFreetierCreditsRemaining,
        "the account is not on Free Tier",
    ),
    (
        ActionType::ReviewFreetierDaysRemaining,
        "the account is not on Free Tier",
    ),
    (
        ActionType::UpdateTaxExemptionCertificate,
        "no tax exemption certificate on file is expiring",
    ),
    (
        ActionType::UpdateTaxRegistrationNumber,
        "the tax registration number on file is current",
    ),
];

/// One emulator: a shared engine, and a router with this service mounted on it,
/// built the way `core/router` builds it. Its store holds the inventory the service
/// computed for this account; nothing here seeds.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(bcm::EmulatedService::new(engine));
        Self { router }
    }

    /// `ListRecommendedActions` with `body` as the payload: the status and payload.
    async fn list(&self, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "bcm-recommended-actions.us-east-1.amazonaws.com")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", TARGET)
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "response body is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        });
        (status, payload)
    }

    /// Every record of a selection, following `nextToken` to the last page.
    async fn list_all(&self, case: &str, selection: &Value) -> Vec<Value> {
        let mut collected = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..64 {
            let mut body = selection.clone();
            if let Some(token) = token.take() {
                body.as_object_mut()
                    .unwrap_or_else(|| panic!("case {case:?}: the selection is not a JSON object"))
                    .insert("nextToken".to_string(), json!(token));
            }
            let (status, payload) = self.list(&body).await;
            assert_eq!(
                status, 200,
                "case {case:?} (page {page}): expected HTTP 200, got {status} with {payload}"
            );
            let records = payload
                .get("recommendedActions")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?} (page {page}): expected a recommendedActions array, got {payload}")
                });
            collected.extend(records.iter().cloned());
            match payload.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => return collected,
            }
        }
        panic!("case {case:?}: the listing never terminated, got {} records", collected.len())
    }
}

/// A record's member as a string, failing the case when it is missing.
fn member(case: &str, record: &Value, name: &str) -> String {
    record
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: record carries no {name}: {record}"))
        .to_string()
}

/// The `id`s of a set of records.
fn ids(case: &str, records: &[Value]) -> BTreeSet<String> {
    records
        .iter()
        .map(|record| member(case, record, "id"))
        .collect()
}

/// A single-filter selection.
fn filtered(key: &str, match_option: &str, value: &str) -> Value {
    json!({"filter": {"actions": [
        {"key": key, "matchOption": match_option, "values": [value]}
    ]}})
}

#[tokio::test]
async fn every_reported_record_carries_the_feature_and_severity_published_for_its_type() {
    let emulator = Emulator::new();
    let reported = emulator.list_all("whole inventory", &json!({})).await;
    assert!(
        !reported.is_empty(),
        "case \"whole inventory\": expected the computed recommendations, got none"
    );

    // Every reported record is a case: its type decides the pair it must report, and
    // the catalog is where that pair comes from. A type the catalog does not name
    // cannot be reported at all — the model closes the enum.
    for record in &reported {
        let case = member("whole inventory", record, "type");
        let (_, feature, severity) = CATALOG
            .iter()
            .find(|(action_type, _, _)| action_type.as_str() == case)
            .unwrap_or_else(|| {
                panic!("case {case:?}: reported a type the published catalog does not name: {record}")
            });
        let actual = (
            member(&case, record, "feature"),
            member(&case, record, "severity"),
        );
        assert_eq!(
            actual,
            (feature.as_str().to_string(), severity.as_str().to_string()),
            "case {case:?}: expected the published pair ({}, {}), got {actual:?}",
            feature.as_str(),
            severity.as_str()
        );
    }
}

#[tokio::test]
async fn the_catalog_covers_every_modelled_action_type_exactly_once() {
    // The table above is the test's own copy of the catalog; it is only a statement
    // about the service if it spans the modelled value space.
    let tabled: BTreeSet<&str> = CATALOG
        .iter()
        .map(|(action_type, _, _)| action_type.as_str())
        .collect();
    let modelled: BTreeSet<&str> = ActionType::ALL
        .iter()
        .map(|action_type| action_type.as_str())
        .collect();
    assert_eq!(
        tabled, modelled,
        "case \"catalog coverage\": the catalog table must name every modelled ActionType once, \
         got {tabled:?} against {modelled:?}"
    );
    assert_eq!(
        CATALOG.len(),
        tabled.len(),
        "case \"catalog coverage\": the catalog table lists an action type twice, got {} rows for \
         {} types",
        CATALOG.len(),
        tabled.len()
    );
}

#[tokio::test]
async fn equals_and_not_equals_partition_the_inventory_for_every_modelled_filter_value() {
    let emulator = Emulator::new();
    let reported = emulator.list_all("unfiltered", &json!({})).await;
    let unfiltered = ids("unfiltered", &reported);
    assert!(
        !unfiltered.is_empty(),
        "case \"unfiltered\": expected the computed recommendations, got none"
    );

    // One case per member of every enum a filter key names: TYPE over ActionType,
    // FEATURE over Feature, SEVERITY over Severity. The two sides are complements
    // over whatever the inventory holds — disjoint, and together the whole unfiltered
    // set — and the EQUALS side is non-empty exactly for the values the inventory
    // carries, which is read out of the unfiltered listing rather than assumed.
    let mut cases: Vec<(&str, String, String)> = Vec::new();
    for action_type in ActionType::ALL {
        cases.push(("TYPE", "type".to_string(), action_type.as_str().to_string()));
    }
    for feature in Feature::ALL {
        cases.push(("FEATURE", "feature".to_string(), feature.as_str().to_string()));
    }
    for severity in Severity::ALL {
        cases.push((
            "SEVERITY",
            "severity".to_string(),
            severity.as_str().to_string(),
        ));
    }

    for (key, record_member, value) in cases {
        let case = format!("{key} {value}");
        let carried: BTreeSet<String> = reported
            .iter()
            .filter(|record| member(&case, record, &record_member) == value)
            .map(|record| member(&case, record, "id"))
            .collect();
        let kept = ids(
            &case,
            &emulator
                .list_all(&case, &filtered(key, "EQUALS", &value))
                .await,
        );
        let excluded = ids(
            &case,
            &emulator
                .list_all(&case, &filtered(key, "NOT_EQUALS", &value))
                .await,
        );

        assert_eq!(
            kept, carried,
            "case {case:?}: EQUALS must keep exactly the records carrying this value, got \
             {kept:?}"
        );
        assert!(
            kept.is_disjoint(&excluded),
            "case {case:?}: a record cannot be both kept and excluded, got {kept:?} and \
             {excluded:?}"
        );
        let union: BTreeSet<String> = kept.union(&excluded).cloned().collect();
        assert_eq!(
            union, unfiltered,
            "case {case:?}: EQUALS and NOT_EQUALS must reconstruct the unfiltered listing, got \
             {union:?}"
        );
    }
}

#[tokio::test]
async fn the_inventory_carries_the_baseline_recommendations_and_nothing_a_condition_rules_out() {
    let emulator = Emulator::new();
    let reported = emulator.list_all("whole inventory", &json!({})).await;
    let types: BTreeSet<String> = reported
        .iter()
        .map(|record| member("whole inventory", record, "type"))
        .collect();

    // A recommendation follows from a condition the service detects on the account.
    // These are the conditions an account in good standing with nothing configured is
    // in, so each of these types must be reported.
    for (action_type, condition) in BASELINE {
        let case = format!("{} ({condition})", action_type.as_str());
        assert!(
            types.contains(action_type.as_str()),
            "case {case:?}: the account is in this condition, so the inventory must carry a \
             record of this type, got {types:?}"
        );
    }

    // And these are the conditions it is NOT in, so no record of these types may be
    // reported — a client reading them would conclude the account is in a state it is
    // not.
    for (action_type, condition) in RULED_OUT {
        let case = format!("{} ({condition})", action_type.as_str());
        assert!(
            !types.contains(action_type.as_str()),
            "case {case:?}: the account is not in this condition, so the inventory must carry no \
             record of this type, got {types:?}"
        );
    }

    // Between them the two tables account for every modelled type: a type in neither
    // would leave the inventory's content unasserted.
    let tabled: BTreeSet<&str> = BASELINE
        .iter()
        .chain(RULED_OUT.iter())
        .map(|(action_type, _)| action_type.as_str())
        .collect();
    let modelled: BTreeSet<&str> = ActionType::ALL
        .iter()
        .map(|action_type| action_type.as_str())
        .collect();
    assert_eq!(
        tabled, modelled,
        "case \"condition coverage\": every modelled ActionType must be in one of the two \
         tables, got {tabled:?} against {modelled:?}"
    );
}

#[tokio::test]
async fn an_equals_filter_over_a_carried_type_reports_that_types_published_pair() {
    let emulator = Emulator::new();

    // Reading one recommendation class the way a client does — filter to the type,
    // read the pair back. Every baseline row is a case, including the types the
    // inventory did not carry before (a budget to create, an anomaly monitor to
    // create, an alternate billing contact to add, billing permissions to migrate).
    for (action_type, _) in BASELINE {
        let (_, feature, severity) = CATALOG
            .iter()
            .find(|(tabled, _, _)| tabled.as_str() == action_type.as_str())
            .unwrap_or_else(|| panic!("the catalog must name {}", action_type.as_str()));
        let case = format!("TYPE EQUALS {}", action_type.as_str());
        let records = emulator
            .list_all(
                &case,
                &filtered("TYPE", "EQUALS", action_type.as_str()),
            )
            .await;
        assert!(
            !records.is_empty(),
            "case {case:?}: expected at least one record of this type, got none"
        );
        for record in &records {
            assert_eq!(
                member(&case, record, "type"),
                action_type.as_str(),
                "case {case:?}: EQUALS kept a record of another type: {record}"
            );
            assert_eq!(
                (
                    member(&case, record, "feature"),
                    member(&case, record, "severity")
                ),
                (feature.as_str().to_string(), severity.as_str().to_string()),
                "case {case:?}: expected the published pair ({}, {}), got {record}",
                feature.as_str(),
                severity.as_str()
            );
            assert!(
                record
                    .get("nextSteps")
                    .and_then(Value::as_array)
                    .is_some_and(|steps| !steps.is_empty()),
                "case {case:?}: expected at least one next step, got {record}"
            );
        }
    }
}
