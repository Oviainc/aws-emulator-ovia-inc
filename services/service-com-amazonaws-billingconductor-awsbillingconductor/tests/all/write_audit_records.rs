// SPDX-License-Identifier: CC0-1.0

//! The ATTRIBUTION record this service leaves for a call that writes, over the whole
//! write surface rather than one operation.
//!
//! RULE under test: a call this service SERVES that mutates stored state leaves exactly
//! ONE structured record naming the operation, the access key id the caller presented,
//! the ARN the call addressed and the outcome of the write — while a call that only
//! REPORTS state leaves none. So every stored value a caller changes is attributable to
//! the caller that changed it, whichever operation changed it.
//!
//! The tables span what the rule generalises over rather than one recorded call: the
//! operation (EVERY one the model declares — the row set is checked against the model, so
//! an operation the model gains lands in neither table and fails here), the kind of write
//! (minting a resource, updating one, deleting one, associating and disassociating
//! accounts / pricing rules / percentage resources, and editing tags), the resource KIND
//! the call addresses (all four the modelled tagging `Arn` enumerates), the ARN SPELLING
//! it is addressed by (the full ARN, the `billinggroup` separator the pattern makes
//! optional, and the bare custom-line-item id — all resolved to one identity in the
//! record), and whether the write changed anything at all.
//!
//! Every assertion goes over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`], the same path the production router serves, and the
//! records are read back from a `tracing` subscriber this binary installs — nothing is
//! written to disk.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

/// The access key id each test below presents. The capture is process-wide, so one access
/// key id per test is what keeps a sibling test's records — and the other suites', which
/// all sign with `AKIDEXAMPLE` — out of this one's assertions.
const WRITE_AKID: &str = "AKIDAUDITWRITES";
const TAGGING_AKID: &str = "AKIDAUDITTAGGING";
const READ_AKID: &str = "AKIDAUDITREADS";

/// The prefix those three share, which is how the capture tells a record it must keep
/// from the stream of every other suite in this binary.
const AUDIT_AKID_PREFIX: &str = "AKIDAUDIT";

/// The status the model binds to the tagging operations
/// (`smithy_query(query="operation",operation="TagResource")`: `http.code` 204 for all
/// three).
const TAGGING_STATUS: u16 = 204;

/// One of the pricing plans Amazon Web Services owns, in the full-ARN form
/// `ComputationPreference.PricingPlanArn` requires — a plan a billing group may name
/// before this suite has created one of its own.
const MANAGED_PLAN_ARN: &str = "arn:aws:billingconductor::aws:pricingplan/BasicPricingPlan";

/// The operations that only REPORT state, so the rule owes them no record: every `List*`
/// and `Get*` the model declares. The set is checked against the model by
/// [`every_state_changing_operation_leaves_one_audit_record`], which drives all the
/// others, and every member of it is driven by
/// [`a_reporting_call_leaves_no_audit_record`].
const REPORTING_OPERATIONS: &[&str] = &[
    "GetBillingGroupCostReport",
    "GetBillingTransferPreference",
    "ListAccountAssociations",
    "ListBillingGroupCostReports",
    "ListBillingGroups",
    "ListCustomLineItemVersions",
    "ListCustomLineItems",
    "ListPricingPlans",
    "ListPricingPlansAssociatedWithPricingRule",
    "ListPricingRules",
    "ListPricingRulesAssociatedToPricingPlan",
    "ListResourcesAssociatedToCustomLineItem",
    "ListTagsForResource",
];

/// A live emulator holding only this service, signing with one access key id.
struct Emulator {
    router: MultiServiceRouter,
    /// The engine the router was built over, so this suite can record the billing
    /// transfer `UpdateBillingTransferPreference` addresses.
    engine: Arc<engine::InMemoryEngine>,
    /// The access key id every request this emulator sends presents, and so the one the
    /// records it provokes must name.
    akid: &'static str,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account: Billing Conductor authorizes its control plane for
    /// the payer account alone (see the `crate::service` module docs), so this is the
    /// world in which the writes under test are reachable at all. The denial every other
    /// caller gets is asserted in `tests/all/payer_account_gate.rs`.
    fn new(akid: &'static str) -> Self {
        // The records this emulator's calls leave reach the capture only while it is this
        // binary's installed subscriber, which the first call to it installs — so it is
        // installed HERE, before any call is served (see [`capture`]).
        capture();
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        engine
            .state::<BillingConductorState>()
            .lock()
            .payer_account_id = Some(::engine::emulator_account_id().to_string());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            engine,
            akid,
        }
    }

    /// Record a billing transfer the organization HAS — an AWS Organizations resource
    /// this API never creates — through the shared seam
    /// `crate::billing_transfer_identity::designate_transfer`.
    fn designate_transfer(&self, arn: &str) -> String {
        crate::billing_transfer_identity::designate_transfer(&self.engine, arn)
    }

    /// Send one request over the wire and return the status plus the parsed body
    /// (`Value::Null` for an empty body).
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://billingconductor.us-east-1.amazonaws.com{path}"
            ))
            .header("host", "billingconductor.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={}/20260101/us-east-1/\
                     billingconductor/aws4_request, SignedHeaders=host, Signature=00",
                    self.akid
                ),
            )
            .body(payload)
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

    /// The same call, asserting the status the model binds to the operation's success.
    async fn served(
        &self,
        case: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
        status: u16,
    ) -> Value {
        let (answered, served) = self.call(method, path, body).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status} from {method} {path}, got {answered} \
             with {served}"
        );
        served
    }

    /// Create a resource for a later row to address, and answer the ARN it minted. The
    /// create is itself a write, so it leaves its own record, which this takes out of the
    /// capture — asserting it is there, so the capture below is never silently deaf.
    async fn create(&self, case: &str, path: &str, body: &Value) -> String {
        let served = self.served(case, "POST", path, Some(body), 200).await;
        let records = capture().drain(self.akid);
        assert_eq!(
            records.len(),
            1,
            "setup {case:?}: expected the create to leave one audit record, got {records:?}"
        );
        arn_of(case, &served)
    }
}

/// One state-changing call, and the record it owes.
struct Write<'a> {
    /// The modelled operation name the record must carry.
    operation: &'static str,
    method: &'static str,
    uri: &'a str,
    /// The request body, `Value::Null` for an operation whose members all travel in the
    /// path and the query (`UntagResource`).
    body: Value,
    /// The status the model binds to the operation's success.
    status: u16,
    /// The resource identity the record must name, through one of its `*_arn` fields;
    /// `None` for a create, whose identity is the `Arn` its own response reports.
    arn: Option<String>,
    /// The outcome the write must report.
    outcome: &'static str,
}

/// Drive one state-changing call and assert THE RULE for it: it is served, and it leaves
/// exactly one record naming the operation, the caller's access key id, the ARN it
/// addressed and the outcome of the write. The operation name is remembered in `driven`,
/// which the caller checks against the model; the ARN the record named is answered back.
async fn assert_one_record(em: &Emulator, driven: &mut BTreeSet<String>, write: &Write<'_>) -> String {
    let case = format!("{} ({} {})", write.operation, write.method, write.uri);
    let body = if write.body.is_null() {
        None
    } else {
        Some(&write.body)
    };
    let served = em
        .served(&case, write.method, write.uri, body, write.status)
        .await;
    let addressed = match &write.arn {
        Some(arn) => arn.clone(),
        None => arn_of(&case, &served),
    };

    let records = capture().drain(em.akid);
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: expected exactly one audit record for a served write, got {records:?}"
    );
    let record = &records[0];
    assert_eq!(
        record.field("operation"),
        Some(write.operation),
        "case {case:?}: expected the record to name the operation, got {record:?}"
    );
    assert_eq!(
        record.field("akid"),
        Some(em.akid),
        "case {case:?}: expected the record to name the presented access key id {:?}, \
         got {record:?}",
        em.akid
    );
    assert_eq!(
        record.field("outcome"),
        Some(write.outcome),
        "case {case:?}: expected the outcome {:?}, got {record:?}",
        write.outcome
    );
    assert!(
        record.addresses(&addressed),
        "case {case:?}: expected an *_arn field naming {addressed:?}, got {record:?}"
    );
    driven.insert(write.operation.to_string());
    addressed
}

/// THE RULE over EVERY state-changing operation of this service: each one leaves exactly
/// one audit record for the call it served, naming the operation, the caller's access key
/// id, the ARN it addressed and the outcome of the write.
///
/// The rows differ along the operation, the kind of write (mint, update, delete,
/// associate, disassociate, tag, untag), the resource kind addressed and — for the
/// creates — the shape of what is created (a flat and a percentage custom line item, the
/// latter associating a resource as it is minted).
#[tokio::test]
async fn every_state_changing_operation_leaves_one_audit_record() {
    let em = Emulator::new(WRITE_AKID);
    let mut driven = BTreeSet::new();

    // The capture is installed by the emulator above, so a record emitted from here on
    // reaches it: a self-test first, which is what makes a missing record further down
    // this suite's subject rather than its harness.
    tracing::info!(
        operation = "HarnessProbe",
        akid = WRITE_AKID,
        outcome = "probed",
        "the capture harness must see a record emitted on this thread"
    );
    let probe = capture().drain(WRITE_AKID);
    assert_eq!(
        probe.len(),
        1,
        "the capture harness must see one record of its own, got {probe:?}"
    );

    // A pricing plan and a pricing rule, each minted by the operation under test.
    let plan = assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "CreatePricingPlan",
            method: "POST",
            uri: "/create-pricing-plan",
            body: json!({ "Name": "AuditPlan" }),
            status: 200,
            arn: None,
            outcome: "created",
        },
    )
    .await;
    let rule = assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "CreatePricingRule",
            method: "POST",
            uri: "/create-pricing-rule",
            body: json!({
                "Name": "AuditRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 10.0,
            }),
            status: 200,
            arn: None,
            outcome: "created",
        },
    )
    .await;

    // The two association surfaces of a pricing plan.
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "AssociatePricingRules",
            method: "PUT",
            uri: "/associate-pricing-rules",
            body: json!({ "Arn": plan, "PricingRuleArns": [rule] }),
            status: 200,
            arn: Some(plan.clone()),
            outcome: "associated",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "DisassociatePricingRules",
            method: "PUT",
            uri: "/disassociate-pricing-rules",
            body: json!({ "Arn": plan, "PricingRuleArns": [rule] }),
            status: 200,
            arn: Some(plan.clone()),
            outcome: "disassociated",
        },
    )
    .await;

    // A billing group on that plan, and the two account-membership surfaces.
    let group = assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "CreateBillingGroup",
            method: "POST",
            uri: "/create-billing-group",
            body: json!({
                "Name": "AuditGroup",
                "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": "111122223333",
            }),
            status: 200,
            arn: None,
            outcome: "created",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "AssociateAccounts",
            method: "POST",
            uri: "/associate-accounts",
            body: json!({ "Arn": group, "AccountIds": ["222233334444"] }),
            status: 200,
            arn: Some(group.clone()),
            outcome: "associated",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "DisassociateAccounts",
            method: "POST",
            uri: "/disassociate-accounts",
            body: json!({ "Arn": group, "AccountIds": ["222233334444"] }),
            status: 200,
            arn: Some(group.clone()),
            outcome: "disassociated",
        },
    )
    .await;

    // Two custom line items: a flat one, and a percentage one that associates a resource
    // as it is minted.
    let flat = assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "CreateCustomLineItem",
            method: "POST",
            uri: "/create-custom-line-item",
            body: json!({
                "Name": "AuditFlatItem",
                "Description": "the flat item this suite audits",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 10.0 } },
            }),
            status: 200,
            arn: None,
            outcome: "created",
        },
    )
    .await;
    let percentage = assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "CreateCustomLineItem",
            method: "POST",
            uri: "/create-custom-line-item",
            body: json!({
                "Name": "AuditPercentageItem",
                "Description": "the percentage item this suite audits",
                "BillingGroupArn": group,
                "ChargeDetails": {
                    "Type": "CREDIT",
                    "Percentage": { "PercentageValue": 12.5, "AssociatedValues": [flat] },
                },
                "Tags": { "kind": "customlineitem" },
            }),
            status: 200,
            arn: None,
            outcome: "created",
        },
    )
    .await;

    // The two resource-association surfaces of a percentage item. The billing group the
    // item applies to is in scope of its charge, and is not associated to it yet.
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "BatchAssociateResourcesToCustomLineItem",
            method: "PUT",
            uri: "/batch-associate-resources-to-custom-line-item",
            body: json!({ "TargetArn": percentage, "ResourceArns": [group] }),
            status: 200,
            arn: Some(percentage.clone()),
            outcome: "associated",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "BatchDisassociateResourcesFromCustomLineItem",
            method: "PUT",
            uri: "/batch-disassociate-resources-from-custom-line-item",
            body: json!({ "TargetArn": percentage, "ResourceArns": [group] }),
            status: 200,
            arn: Some(percentage.clone()),
            outcome: "disassociated",
        },
    )
    .await;

    // The four updates of a stored resource — including the preference of a billing
    // transfer, the one resource this API never creates.
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "UpdatePricingRule",
            method: "PUT",
            uri: "/update-pricing-rule",
            body: json!({ "Arn": rule, "Description": "the audited rule" }),
            status: 200,
            arn: Some(rule.clone()),
            outcome: "updated",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "UpdatePricingPlan",
            method: "PUT",
            uri: "/update-pricing-plan",
            body: json!({ "Arn": plan, "Description": "the audited plan" }),
            status: 200,
            arn: Some(plan.clone()),
            outcome: "updated",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "UpdateBillingGroup",
            method: "POST",
            uri: "/update-billing-group",
            body: json!({ "Arn": group, "Description": "the audited group" }),
            status: 200,
            arn: Some(group.clone()),
            outcome: "updated",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "UpdateCustomLineItem",
            method: "POST",
            uri: "/update-custom-line-item",
            body: json!({ "Arn": flat, "Description": "the audited flat item" }),
            status: 200,
            arn: Some(flat.clone()),
            outcome: "updated",
        },
    )
    .await;
    let transfer = em.designate_transfer(&format!(
        "arn:aws:organizations::{}:transfer/o-auditorgabc/billing/outbound/rt-auditwrite1",
        ::engine::emulator_account_id()
    ));
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "UpdateBillingTransferPreference",
            method: "PUT",
            uri: "/update-billing-transfer-preference",
            body: json!({
                "ResponsibilityTransferArn": transfer,
                "AutoBillingTransferBillingGroupCreation": { "Enabled": false },
            }),
            status: 200,
            arn: Some(transfer.clone()),
            outcome: "updated",
        },
    )
    .await;

    // The two writes that edit a resource's stored tags, here on the pricing plan.
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "TagResource",
            method: "POST",
            uri: &path_with_label(&plan),
            body: json!({ "Tags": { "team": "finance" } }),
            status: TAGGING_STATUS,
            arn: Some(plan.clone()),
            outcome: "tagged",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "UntagResource",
            method: "DELETE",
            uri: &path_with_tag_keys(&plan, &["team"]),
            body: Value::Null,
            status: TAGGING_STATUS,
            arn: Some(plan.clone()),
            outcome: "untagged",
        },
    )
    .await;

    // The four deletes, each taking one of the resources above out of existence.
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "DeleteCustomLineItem",
            method: "POST",
            uri: "/delete-custom-line-item",
            body: json!({ "Arn": percentage }),
            status: 200,
            arn: Some(percentage.clone()),
            outcome: "deleted",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "DeleteBillingGroup",
            method: "POST",
            uri: "/delete-billing-group",
            body: json!({ "Arn": group }),
            status: 200,
            arn: Some(group.clone()),
            outcome: "deleted",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "DeletePricingRule",
            method: "POST",
            uri: "/delete-pricing-rule",
            body: json!({ "Arn": rule }),
            status: 200,
            arn: Some(rule.clone()),
            outcome: "deleted",
        },
    )
    .await;
    assert_one_record(
        &em,
        &mut driven,
        &Write {
            operation: "DeletePricingPlan",
            method: "POST",
            uri: "/delete-pricing-plan",
            body: json!({ "Arn": plan }),
            status: 200,
            arn: Some(plan.clone()),
            outcome: "deleted",
        },
    )
    .await;

    // THE ROW SET IS CHECKED AGAINST THE MODEL: every operation the model declares is
    // either a write this walk drove or one of the reporting operations
    // [`REPORTING_OPERATIONS`] names, so an operation the model gains is in neither and
    // fails here rather than silently writing with nothing attributing it to a caller.
    let modelled: BTreeSet<String> = crate::payer_account_gate::modelled_operations()
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    let reporting: BTreeSet<String> = REPORTING_OPERATIONS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert!(
        driven.is_disjoint(&reporting),
        "expected no operation to be both a write and a reporting operation, got {:?}",
        driven.intersection(&reporting).collect::<Vec<_>>()
    );
    let covered: BTreeSet<String> = driven.union(&reporting).cloned().collect();
    assert_eq!(
        covered, modelled,
        "expected every modelled operation to be either a write this suite drives or a \
         reporting operation it asserts leaves no record"
    );
}

/// THE RULE on the tagging surface, the one pair of writes that addresses EVERY taggable
/// kind and does it through several ARN spellings: the record names the resource in the
/// one spelling this service resolves it to, so one resource is one identity in the
/// stream however the request addressed it — and a merge or a removal that writes nothing
/// reports that outcome instead.
///
/// The rows differ along the resource kind (all four the modelled tagging `Arn`
/// enumerates), the ARN spelling (the full ARN, the `billinggroup` separator the pattern
/// makes optional, and the bare custom-line-item id) and whether the write changes
/// anything.
#[tokio::test]
async fn the_tagging_writes_record_the_resource_they_addressed() {
    let em = Emulator::new(TAGGING_AKID);
    // The billing group is created FIRST so its minted id is all digits, the form the
    // modelled tagging `Arn` spells a billing group in (`billinggroup/?[0-9]{12}`). It
    // names a pricing plan Amazon Web Services owns, so nothing has to exist before it.
    let group = em
        .create(
            "a billing group to tag",
            "/create-billing-group",
            &json!({
                "Name": "TagAuditGroup",
                "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
                "ComputationPreference": { "PricingPlanArn": MANAGED_PLAN_ARN },
                "PrimaryAccountId": "111122223333",
            }),
        )
        .await;
    let plan = em
        .create(
            "a pricing plan to tag",
            "/create-pricing-plan",
            &json!({ "Name": "TagAuditPlan" }),
        )
        .await;
    let rule = em
        .create(
            "a pricing rule to tag",
            "/create-pricing-rule",
            &json!({
                "Name": "TagAuditRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 1.5,
            }),
        )
        .await;
    let item = em
        .create(
            "a custom line item to tag",
            "/create-custom-line-item",
            &json!({
                "Name": "TagAuditItem",
                "Description": "the item this suite tags",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 1.0 } },
            }),
        )
        .await;

    // The bare 10-character id the `CustomLineItemArn` pattern admits beside the full
    // ARN, and the billing group spelled without the separator the pattern makes
    // optional: one resource, another spelling.
    let item_id = bare_id(&item);
    let group_unseparated = group.replace("billinggroup/", "billinggroup");

    // (label, the ARN as the request spells it, the identity the record must name)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a pricing plan by its full ARN", &plan, &plan),
        ("a pricing rule by its full ARN", &rule, &rule),
        ("a billing group by its full ARN", &group, &group),
        (
            "a billing group without the optional separator",
            &group_unseparated,
            &group,
        ),
        ("a custom line item by its full ARN", &item, &item),
        ("a custom line item by its bare id", &item_id, &item),
    ];

    for &(label, addressed, identity) in &rows {
        let mut driven = BTreeSet::new();
        // A key the resource does not carry yet: the merge writes, so the outcome is the
        // written one.
        assert_one_record(
            &em,
            &mut driven,
            &Write {
                operation: "TagResource",
                method: "POST",
                uri: &path_with_label(addressed),
                body: json!({ "Tags": { "audited": label } }),
                status: TAGGING_STATUS,
                arn: Some(identity.to_string()),
                outcome: "tagged",
            },
        )
        .await;
        // The SAME pair again: a merge of what is already stored leaves the stored map
        // exactly as it was, and says so.
        assert_one_record(
            &em,
            &mut driven,
            &Write {
                operation: "TagResource",
                method: "POST",
                uri: &path_with_label(addressed),
                body: json!({ "Tags": { "audited": label } }),
                status: TAGGING_STATUS,
                arn: Some(identity.to_string()),
                outcome: "unchanged",
            },
        )
        .await;
        // The key the resource carries, removed…
        assert_one_record(
            &em,
            &mut driven,
            &Write {
                operation: "UntagResource",
                method: "DELETE",
                uri: &path_with_tag_keys(addressed, &["audited"]),
                body: Value::Null,
                status: TAGGING_STATUS,
                arn: Some(identity.to_string()),
                outcome: "untagged",
            },
        )
        .await;
        // …and a key the resource never carried: nothing to remove, nothing written.
        assert_one_record(
            &em,
            &mut driven,
            &Write {
                operation: "UntagResource",
                method: "DELETE",
                uri: &path_with_tag_keys(addressed, &["never-set"]),
                body: Value::Null,
                status: TAGGING_STATUS,
                arn: Some(identity.to_string()),
                outcome: "unchanged",
            },
        )
        .await;
        assert_eq!(
            driven,
            ["TagResource".to_string(), "UntagResource".to_string()]
                .into_iter()
                .collect::<BTreeSet<String>>(),
            "case {label:?}: expected the two tagging writes to be the operations driven"
        );
    }
}

/// THE OTHER HALF OF THE RULE: a call that only REPORTS state leaves no record at all —
/// there is no write to attribute to a caller, and a read that recorded one would bury
/// the writes in the stream.
///
/// The rows are every reporting operation the model declares, driven against a world the
/// setup created, so each one is SERVED rather than refused before it could have recorded
/// anything.
#[tokio::test]
async fn a_reporting_call_leaves_no_audit_record() {
    let em = Emulator::new(READ_AKID);
    let plan = em
        .create(
            "a pricing plan to read",
            "/create-pricing-plan",
            &json!({ "Name": "ReadAuditPlan" }),
        )
        .await;
    let rule = em
        .create(
            "a pricing rule to read",
            "/create-pricing-rule",
            &json!({
                "Name": "ReadAuditRule",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 2.5,
            }),
        )
        .await;
    let group = em
        .create(
            "a billing group to read",
            "/create-billing-group",
            &json!({
                "Name": "ReadAuditGroup",
                "AccountGrouping": { "LinkedAccountIds": ["111122223333"] },
                "ComputationPreference": { "PricingPlanArn": plan },
                "PrimaryAccountId": "111122223333",
            }),
        )
        .await;
    let item = em
        .create(
            "a custom line item to read",
            "/create-custom-line-item",
            &json!({
                "Name": "ReadAuditItem",
                "Description": "the item this suite reads",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Flat": { "ChargeValue": 5.0 } },
            }),
        )
        .await;
    let transfer = em.designate_transfer(&format!(
        "arn:aws:organizations::{}:transfer/o-auditorgdef/billing/outbound/rt-auditread01",
        ::engine::emulator_account_id()
    ));

    // (operation, method, uri, body, the status the model binds to its success)
    let reads: Vec<(&str, &str, String, Value, u16)> = vec![
        (
            "ListPricingPlans",
            "POST",
            "/list-pricing-plans".to_string(),
            json!({}),
            200,
        ),
        (
            "ListPricingRules",
            "POST",
            "/list-pricing-rules".to_string(),
            json!({}),
            200,
        ),
        (
            "ListBillingGroups",
            "POST",
            "/list-billing-groups".to_string(),
            json!({ "MaxResults": 5 }),
            200,
        ),
        (
            "ListCustomLineItems",
            "POST",
            "/list-custom-line-items".to_string(),
            json!({ "MaxResults": 5 }),
            200,
        ),
        (
            "ListCustomLineItemVersions",
            "POST",
            "/list-custom-line-item-versions".to_string(),
            json!({ "Arn": item }),
            200,
        ),
        (
            "ListResourcesAssociatedToCustomLineItem",
            "POST",
            "/list-resources-associated-to-custom-line-item".to_string(),
            json!({ "Arn": item }),
            200,
        ),
        (
            "ListAccountAssociations",
            "POST",
            "/list-account-associations".to_string(),
            json!({}),
            200,
        ),
        (
            "ListBillingGroupCostReports",
            "POST",
            "/list-billing-group-cost-reports".to_string(),
            json!({}),
            200,
        ),
        (
            "ListPricingPlansAssociatedWithPricingRule",
            "POST",
            "/list-pricing-plans-associated-with-pricing-rule".to_string(),
            json!({ "PricingRuleArn": rule }),
            200,
        ),
        (
            "ListPricingRulesAssociatedToPricingPlan",
            "POST",
            "/list-pricing-rules-associated-to-pricing-plan".to_string(),
            json!({ "PricingPlanArn": plan }),
            200,
        ),
        (
            "GetBillingGroupCostReport",
            "POST",
            "/get-billing-group-cost-report".to_string(),
            json!({ "Arn": group }),
            200,
        ),
        (
            "GetBillingTransferPreference",
            "POST",
            "/get-billing-transfer-preference".to_string(),
            json!({ "ResponsibilityTransferArn": transfer }),
            200,
        ),
        (
            "ListTagsForResource",
            "GET",
            path_with_label(&item),
            Value::Null,
            TAGGING_STATUS,
        ),
    ];

    let mut read_operations = BTreeSet::new();
    for (operation, method, uri, body, status) in &reads {
        let case = format!("{operation} ({method} {uri})");
        let request = if body.is_null() { None } else { Some(body) };
        em.served(&case, method, uri, request, *status).await;
        let records = capture().drain(READ_AKID);
        assert!(
            records.is_empty(),
            "case {case:?}: expected a reporting call to leave no audit record, got \
             {records:?}"
        );
        read_operations.insert((*operation).to_string());
    }

    // The rows are EVERY reporting operation the model declares — the same set
    // [`every_state_changing_operation_leaves_one_audit_record`] checks its write rows
    // against, so neither table can quietly stop covering the surface.
    let reporting: BTreeSet<String> = REPORTING_OPERATIONS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(
        read_operations, reporting,
        "expected one row per reporting operation"
    );
}

/// The `Arn` a create reported, which is the identity its own record must name.
fn arn_of(case: &str, served: &Value) -> String {
    served["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Arn in {served}"))
        .to_string()
}

/// The bare resource id of a full ARN — the last segment, which the
/// `CustomLineItemArn` pattern admits on its own.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/')
        .next()
        .expect("an ARN ends in its resource id")
        .to_string()
}

/// `ResourceArn` is bound to a NON-greedy `@httpLabel` (`/tags/{ResourceArn}`), so an ARN
/// travels as ONE path segment with its `/` and `:` percent-encoded — exactly what an SDK
/// client sends.
fn path_with_label(resource_arn: &str) -> String {
    format!("/tags/{}", encode_label(resource_arn))
}

/// The same path with `TagKeys`, which is modelled `@httpQuery("tagKeys")`, so every key
/// is its own query pair.
fn path_with_tag_keys(resource_arn: &str, tag_keys: &[&str]) -> String {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode_label(key)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", path_with_label(resource_arn))
}

/// Percent-encode the two characters of an ARN that one path segment may not carry.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '/' => "%2F".to_string(),
            ':' => "%3A".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One captured record: its fields, rendered as strings.
#[derive(Clone, Debug)]
struct CapturedRecord {
    fields: BTreeMap<String, String>,
}

impl CapturedRecord {
    /// One field's value, `None` when the record carries no such field.
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// Whether the record names `arn` through one of its `*_arn` fields — the ARN the
    /// call addressed, whichever resource kind the operation names it by.
    fn addresses(&self, arn: &str) -> bool {
        self.fields
            .iter()
            .any(|(name, value)| name.ends_with("_arn") && value == arn)
    }
}

/// Collects a record's fields into strings, whatever type each was recorded as.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A `tracing` subscriber that keeps the records THIS suite's callers provoked, so a test
/// can read them back.
///
/// Installed ONCE as this test binary's global subscriber (see [`capture`]): a
/// thread-scoped subscriber would not do, because `tracing` caches a callsite's interest
/// globally and a sibling test running with no subscriber would cache a handler's
/// callsite as "nobody is listening". Only the records whose access key id this suite
/// presents are kept, so the stream of every other suite in this binary is dropped rather
/// than accumulated. Nothing is written to disk.
#[derive(Clone, Default)]
struct Capture {
    records: Arc<Mutex<Vec<CapturedRecord>>>,
}

impl Capture {
    /// Take the records `akid` provoked, oldest first, leaving another access key id's
    /// records where they are — the capture is process-wide, so two tests draining their
    /// own records in parallel must not drop each other's.
    fn drain(&self, akid: &str) -> Vec<CapturedRecord> {
        let mut records = self.records.lock().expect("the capture buffer");
        let drained: Vec<CapturedRecord> = records
            .iter()
            .filter(|record| record.field("akid") == Some(akid))
            .cloned()
            .collect();
        records.retain(|record| record.field("akid") != Some(akid));
        drained
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // This suite asserts on events, not spans; one id for every span is enough.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut fields = BTreeMap::new();
        event.record(&mut FieldCollector(&mut fields));
        let record = CapturedRecord { fields };
        if record
            .field("akid")
            .is_some_and(|akid| akid.starts_with(AUDIT_AKID_PREFIX))
        {
            self.records
                .lock()
                .expect("the capture buffer")
                .push(record);
        }
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// This binary's one capture, installed as the global `tracing` subscriber on first use.
fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = Capture::default();
        // Installing the global subscriber also rebuilds `tracing`'s callsite interest
        // cache, so a callsite another test already evaluated starts being delivered.
        let _ = tracing::subscriber::set_global_default(capture.clone());
        capture
    })
}
