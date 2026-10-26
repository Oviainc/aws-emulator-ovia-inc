// SPDX-License-Identifier: CC0-1.0

//! The BILLING PERIOD a read of a percentage custom line item's resource associations
//! is scoped to, end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` -> `BatchAssociateResourcesToCustomLineItem` ->
//! `BatchDisassociateResourcesFromCustomLineItem` ->
//! `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: `ListResourcesAssociatedToCustomLineItem` reports the associations of the
//! custom line item for ONE billing period — "the billing period for which the resource
//! associations will be listed", and the CURRENT billing period when the request names
//! none, the default this service documents for its period-scoped reads ("If you don't
//! provide a billing period, the current billing period is used") — and an association
//! is in effect only in the billing periods of its own half-open range, which begins no
//! earlier than the period the associating call was made in, because the association
//! did not exist before then.
//!
//! The rows differ along where the association's own range falls relative to the
//! current billing period (none at all, open-ended, bounded, wholly past, wholly
//! future), what ended it (a bounded disassociation taking effect in the current period
//! or in a later one, none at all), how far the period a read asks for is from the
//! association's start, and how a billing period is spelled (`2026-7` / `2026-07`) —
//! never along a renaming.
//!
//! Every billing period here is therefore named as an offset from the CURRENT billing
//! period ([`period`]): the clock decides which period a call lands in, so that is what
//! an association made now and a read that names no period are both scoped to.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// An AWS-managed pricing plan every billing group here computes with: the
/// `PricingPlanFullArn` pattern enumerates it, so no plan has to be created first
/// (`smithy_query(query="shape",shape="PricingPlanFullArn")`).
const MANAGED_PRICING_PLAN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// A live emulator holding only this service, with the emulated account designated the
/// organization's payer account — Billing Conductor's control plane is the payer
/// account's alone, so that is the world in which these associations are reachable.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send a JSON body to one of this service's routes and return the status and the
    /// parsed body. The method matters: the batch association routes are `PUT`
    /// (`smithy_query(query="operation",operation=
    /// "BatchAssociateResourcesToCustomLineItem")`).
    async fn send(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/billingconductor/aws4_request, \
                 SignedHeaders=host, Signature=00",
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a billing group and return its ARN.
    async fn billing_group(&self, name: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-billing-group",
                &json!({
                    "Name": name,
                    "AccountGrouping": { "LinkedAccountIds": [] },
                    "ComputationPreference": { "PricingPlanArn": MANAGED_PRICING_PLAN },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case billing group {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("billing group {name:?}"))
    }

    /// A percentage custom line item: the only kind resources can be associated to.
    async fn percentage_item(&self, group_arn: &str, name: &str) -> String {
        let (status, body) = self
            .send(
                "POST",
                "/create-custom-line-item",
                &json!({
                    "Name": name,
                    "Description": "a custom line item",
                    "BillingGroupArn": group_arn,
                    "ChargeDetails": {
                        "Type": "FEE",
                        "Percentage": { "PercentageValue": 10.0 },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case custom line item {name:?}: expected HTTP 200, got {status} with {body}"
        );
        arn_of(&body, &format!("custom line item {name:?}"))
    }

    /// `BatchAssociateResourcesToCustomLineItem`, asserted to succeed for every
    /// resource: the state these reads are about.
    async fn associate(&self, case: &str, target: &str, resource: &str, range: Option<Value>) {
        let mut body = json!({ "TargetArn": target, "ResourceArns": [resource] });
        if let Some(range) = range {
            body["BillingPeriodRange"] = range;
        }
        let (status, answered) = self
            .send("PUT", "/batch-associate-resources-to-custom-line-item", &body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the association to answer HTTP 200, got {status} with \
             {answered}"
        );
        assert_eq!(
            answered.get("FailedAssociatedResources"),
            Some(&json!([])),
            "case {case:?}: expected no failed resource, got {answered}"
        );
    }

    /// `BatchDisassociateResourcesFromCustomLineItem`, asserted to succeed for every
    /// resource.
    async fn disassociate(&self, case: &str, target: &str, resource: &str, range: Option<Value>) {
        let mut body = json!({ "TargetArn": target, "ResourceArns": [resource] });
        if let Some(range) = range {
            body["BillingPeriodRange"] = range;
        }
        let (status, answered) = self
            .send(
                "PUT",
                "/batch-disassociate-resources-from-custom-line-item",
                &body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the disassociation to answer HTTP 200, got {status} with \
             {answered}"
        );
        assert_eq!(
            answered.get("FailedDisassociatedResources"),
            Some(&json!([])),
            "case {case:?}: expected no failed resource, got {answered}"
        );
    }

    /// The `(Arn, EndBillingPeriod)` pairs a read reports, in the order it reports them.
    /// `extra` carries whatever the request names beside the item's `Arn` — nothing at
    /// all for the default read.
    async fn associated(&self, case: &str, target: &str, extra: Value) -> Vec<(String, Option<String>)> {
        let mut body = json!({ "Arn": target });
        for (member, value) in extra.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        let (status, answered) = self
            .send(
                "POST",
                "/list-resources-associated-to-custom-line-item",
                &body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer HTTP 200, got {status} with {answered}"
        );
        answered
            .get("AssociatedResources")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|element| {
                (
                    element
                        .get("Arn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    element
                        .get("EndBillingPeriod")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                )
            })
            .collect()
    }

    /// What a read of `billing_period` reports for one resource: `None` when the read
    /// does not report it at all, `Some(end)` with the `EndBillingPeriod` it carries
    /// there when it does.
    async fn in_period(
        &self,
        case: &str,
        target: &str,
        resource: &str,
        billing_period: &str,
    ) -> Option<Option<String>> {
        self.associated(case, target, json!({ "BillingPeriod": billing_period }))
            .await
            .into_iter()
            .find(|(arn, _)| arn == resource)
            .map(|(_, end)| end)
    }

    /// The same, for the read that names NO billing period.
    async fn by_default(&self, case: &str, target: &str, resource: &str) -> Option<Option<String>> {
        self.associated(case, target, json!({}))
            .await
            .into_iter()
            .find(|(arn, _)| arn == resource)
            .map(|(_, end)| end)
    }
}

/// The `Arn` a create answered with.
fn arn_of(body: &Value, label: &str) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label}: expected an Arn member, got {body}"))
        .to_string()
}

/// The `BillingPeriod` `months` after the CURRENT billing period (negative for
/// earlier), in the zero-padded spelling the modelled `BillingPeriod` pattern
/// `^\d{4}-(0?[1-9]|1[012])$` allows.
fn period(months: i64) -> String {
    let current =
        service_com_amazonaws_billingconductor_awsbillingconductor::state::current_billing_period();
    let (year, month) = current
        .split_once('-')
        .expect("the current billing period is spelled YYYY-MM");
    let total = year.parse::<i64>().expect("a four-digit year") * 12
        + month.parse::<i64>().expect("a month")
        - 1
        + months;
    format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1)
}

/// The same billing period with its month spelled without a leading zero — the other
/// spelling the modelled `BillingPeriod` pattern allows. A month from October on has
/// only the one spelling, so for those this is the value it was given.
fn one_digit_month(billing_period: &str) -> String {
    let (year, month) = billing_period
        .split_once('-')
        .expect("a billing period is spelled YYYY-MM");
    format!("{year}-{}", month.trim_start_matches('0'))
}

/// One row of the default-read table: the range the associating call named, and what the
/// read that names NO billing period reports for the association afterwards.
struct DefaultReadRow {
    case: &'static str,
    /// The `BillingPeriodRange` of the associating call, if any.
    associated: Option<Value>,
    /// What the default read reports: `None` when it does not report the association at
    /// all, `Some(end)` with the `EndBillingPeriod` it carries when it does.
    by_default: Option<Option<String>>,
    /// A billing period INSIDE the association's own range, and what a read naming it
    /// reports — so a row the default read leaves out is shown to be period scoping
    /// rather than a lost record.
    inside: (String, Option<Option<String>>),
}

/// A read that names no `BillingPeriod` is scoped to the CURRENT billing period: it
/// reports exactly the associations in effect now, each with the `EndBillingPeriod` it
/// carries in that period, and it answers the same as a read that names the current
/// period itself.
#[tokio::test]
async fn the_default_read_is_scoped_to_the_current_billing_period() {
    let rows: Vec<DefaultReadRow> = vec![
        DefaultReadRow {
            case: "an association with no range at all",
            associated: None,
            by_default: Some(None),
            inside: (period(0), Some(None)),
        },
        DefaultReadRow {
            case: "an association open-ended from the current period",
            associated: Some(json!({ "InclusiveStartBillingPeriod": period(0) })),
            by_default: Some(None),
            inside: (period(6), Some(None)),
        },
        DefaultReadRow {
            case: "an association bounded to a later period",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": period(0),
                "ExclusiveEndBillingPeriod": period(2),
            })),
            by_default: Some(Some(period(2))),
            inside: (period(1), Some(Some(period(2)))),
        },
        DefaultReadRow {
            case: "an association that ended before the current period",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": period(-3),
                "ExclusiveEndBillingPeriod": period(0),
            })),
            by_default: None,
            inside: (period(-2), Some(Some(period(0)))),
        },
        DefaultReadRow {
            case: "an association wholly in past periods",
            associated: Some(json!({
                "InclusiveStartBillingPeriod": period(-6),
                "ExclusiveEndBillingPeriod": period(-3),
            })),
            by_default: None,
            inside: (period(-5), Some(Some(period(-3)))),
        },
        DefaultReadRow {
            case: "an association that only starts in a later period",
            associated: Some(json!({ "InclusiveStartBillingPeriod": period(1) })),
            by_default: None,
            inside: (period(1), Some(None)),
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;
        let target = emulator.percentage_item(&home, "target").await;
        let resource = home.clone();
        emulator
            .associate(row.case, &target, &resource, row.associated.clone())
            .await;

        let reported = emulator.by_default(row.case, &target, &resource).await;
        assert_eq!(
            reported, row.by_default,
            "case {:?}: expected the default read to report {:?}, got {reported:?}",
            row.case, row.by_default
        );

        // The default read is the read of the CURRENT billing period, in either
        // spelling of that month.
        for current in [period(0), one_digit_month(&period(0))] {
            let named = emulator
                .in_period(row.case, &target, &resource, &current)
                .await;
            assert_eq!(
                named, row.by_default,
                "case {:?}: expected the read of {current} to answer as the default read \
                 ({:?}), got {named:?}",
                row.case, row.by_default
            );
        }

        let (inside_period, expected_inside) = row.inside;
        let inside = emulator
            .in_period(row.case, &target, &resource, &inside_period)
            .await;
        assert_eq!(
            inside, expected_inside,
            "case {:?}: expected the read of {inside_period} to report {expected_inside:?}, \
             got {inside:?}",
            row.case
        );
    }
}

/// One row of the disassociation table: the billing period a bounded
/// `BatchDisassociateResourcesFromCustomLineItem` takes effect from, and what the
/// default read reports for the association afterwards.
struct EndedRow {
    case: &'static str,
    /// `InclusiveStartBillingPeriod` of the disassociating call, `None` for a call that
    /// names no range at all and so ends the association in every period.
    disassociated_from: Option<String>,
    /// What the default read reports afterwards.
    by_default: Option<Option<String>>,
}

/// A resource whose association a bounded disassociation has ENDED as of the current
/// billing period is no longer in the default listing: the record keeps the periods
/// before the disassociation, so a read of one of those still reports it, and a
/// disassociation that only takes effect later leaves the association in effect now,
/// carrying the period it will end at.
#[tokio::test]
async fn an_association_ended_in_the_current_period_leaves_the_default_listing() {
    let rows: Vec<EndedRow> = vec![
        EndedRow {
            case: "a disassociation taking effect in the current period",
            disassociated_from: Some(period(0)),
            by_default: None,
        },
        EndedRow {
            case: "a disassociation taking effect in the next period",
            disassociated_from: Some(period(1)),
            by_default: Some(Some(period(1))),
        },
        EndedRow {
            case: "a disassociation taking effect two periods on",
            disassociated_from: Some(period(2)),
            by_default: Some(Some(period(2))),
        },
        EndedRow {
            case: "a disassociation with no range at all",
            disassociated_from: None,
            by_default: None,
        },
    ];

    for row in rows {
        let emulator = Emulator::new();
        let home = emulator.billing_group("home").await;
        let target = emulator.percentage_item(&home, "target").await;
        let resource = home.clone();
        // The association the issue's scenario makes: no `BillingPeriodRange` at all, so
        // it is in effect from the current billing period on.
        emulator.associate(row.case, &target, &resource, None).await;
        assert_eq!(
            emulator.by_default(row.case, &target, &resource).await,
            Some(None),
            "case {:?}: expected the association to be in the default listing before the \
             disassociation",
            row.case
        );

        emulator
            .disassociate(
                row.case,
                &target,
                &resource,
                row.disassociated_from
                    .as_ref()
                    .map(|from| json!({ "InclusiveStartBillingPeriod": from })),
            )
            .await;

        let reported = emulator.by_default(row.case, &target, &resource).await;
        assert_eq!(
            reported, row.by_default,
            "case {:?}: expected the default read to report {:?} after the disassociation, got \
             {reported:?}",
            row.case, row.by_default
        );
    }
}

/// One row of the probe table: a billing period a read asks for, and whether each of the
/// two associations is in effect in it.
struct ProbeRow {
    case: &'static str,
    billing_period: String,
    /// Is the association made with NO range in effect in that period?
    open: bool,
    /// Is the association the call bounded to `[period(-4), period(-1))` in effect in
    /// it?
    backdated: bool,
}

/// No billing period BEFORE an association is made reports it: an association with no
/// range of its own is in effect from the period the associating call landed in, while
/// one whose call named an earlier inclusive start is in effect from the period the
/// caller named.
#[tokio::test]
async fn a_period_before_the_association_does_not_report_it() {
    let rows: Vec<ProbeRow> = vec![
        ProbeRow {
            case: "a period long before this account held any custom line item",
            // A fixed month far outside both ranges, whatever the clock says.
            billing_period: "2020-01".to_string(),
            open: false,
            backdated: false,
        },
        ProbeRow {
            case: "a year before the current period",
            billing_period: period(-12),
            open: false,
            backdated: false,
        },
        ProbeRow {
            case: "the period before the backdated range starts",
            billing_period: period(-5),
            open: false,
            backdated: false,
        },
        ProbeRow {
            case: "the backdated range's inclusive start",
            billing_period: period(-4),
            open: false,
            backdated: true,
        },
        ProbeRow {
            case: "the period before the backdated range's exclusive end",
            billing_period: period(-2),
            open: false,
            backdated: true,
        },
        ProbeRow {
            case: "the backdated range's exclusive end",
            billing_period: period(-1),
            open: false,
            backdated: false,
        },
        ProbeRow {
            case: "the current period",
            billing_period: period(0),
            open: true,
            backdated: false,
        },
        ProbeRow {
            case: "the current period, one-digit month",
            billing_period: one_digit_month(&period(0)),
            open: true,
            backdated: false,
        },
        ProbeRow {
            case: "the next period",
            billing_period: period(1),
            open: true,
            backdated: false,
        },
        ProbeRow {
            case: "a year after the current period",
            billing_period: period(12),
            open: true,
            backdated: false,
        },
    ];

    let emulator = Emulator::new();
    let home = emulator.billing_group("home").await;
    let target = emulator.percentage_item(&home, "target").await;
    // The association with no range names the billing group the target applies to; the
    // backdated one names another custom line item of that same group.
    let open = home.clone();
    let backdated = emulator.percentage_item(&home, "backdated").await;
    emulator.associate("the open association", &target, &open, None).await;
    emulator
        .associate(
            "the backdated association",
            &target,
            &backdated,
            Some(json!({
                "InclusiveStartBillingPeriod": period(-4),
                "ExclusiveEndBillingPeriod": period(-1),
            })),
        )
        .await;

    for row in rows {
        for (resource, label, expected) in [
            (&open, "the association with no range", row.open),
            (&backdated, "the backdated association", row.backdated),
        ] {
            let reported = emulator
                .in_period(row.case, &target, resource, &row.billing_period)
                .await;
            assert_eq!(
                reported.is_some(),
                expected,
                "case {:?} ({label}, {}): expected it to be {}, got {reported:?}",
                row.case,
                row.billing_period,
                if expected { "in effect" } else { "left out" }
            );
        }
    }
}
