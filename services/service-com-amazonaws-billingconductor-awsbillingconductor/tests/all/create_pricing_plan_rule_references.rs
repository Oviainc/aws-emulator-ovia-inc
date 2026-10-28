// SPDX-License-Identifier: CC0-1.0

//! The `PricingRuleArns` a request names, end to end over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`] — the same path the
//! production router serves.
//!
//! RULE under test: a request whose list of pricing rule ARNs names a rule this account
//! does not hold is refused with the modelled 400 `ValidationException` carrying the
//! reason the model enumerates for it (`PRICINGRULES_NOT_EXIST`), and one that names a
//! single rule more than once — in either spelling the modelled `PricingRuleArn` pattern
//! admits for the same rule — with `DUPLICATE_PRICINGRULE_ARNS`; the WHOLE request is
//! refused, so nothing is minted and no association is recorded. A list whose every
//! entry names a rule that exists is served, and the associations read back through the
//! read-only reader.
//!
//! The rule is about the LIST, not about one operation, so it is asserted on every
//! operation that takes one: `CreatePricingPlan`, `AssociatePricingRules` and
//! `DisassociatePricingRules`.
//!
//! The tables' rows differ along what the rule generalises over: how many entries the
//! list carries (1, 2, 3, the modelled maximum of 30), where the faulty entry sits
//! (first, in the middle, last), how each entry is spelled (bare id, full ARN, mixed),
//! which fault the list carries (an unknown rule, a repeated rule, both at once), and
//! whether the rule ever existed (never created, or created and then deleted).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// `POST /create-pricing-plan` — the operation the rule is reported against
/// (`smithy_query(query="operation",operation="CreatePricingPlan")`).
const CREATE_PLAN_PATH: &str = "/create-pricing-plan";

/// `POST /create-pricing-rule`, which is the only way a pricing rule comes to exist.
const CREATE_RULE_PATH: &str = "/create-pricing-rule";

/// `POST /delete-pricing-rule`, which takes one out of existence again.
const DELETE_RULE_PATH: &str = "/delete-pricing-rule";

/// `PUT /associate-pricing-rules` and `PUT /disassociate-pricing-rules` — the other two
/// operations whose input carries a list of pricing rule ARNs.
const ASSOCIATE_PATH: &str = "/associate-pricing-rules";
const DISASSOCIATE_PATH: &str = "/disassociate-pricing-rules";

/// `POST /list-pricing-rules-associated-to-pricing-plan` — the read-only reader of the
/// associations a create records.
const LIST_RULES_PATH: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// `POST /list-pricing-plans` — the read-only reader that shows a refused create minted
/// no plan.
const LIST_PLANS_PATH: &str = "/list-pricing-plans";

/// The modelled maximum length of `CreatePricingPlanInput.PricingRuleArns`
/// (`PricingRuleArnsInput`, len 0..30).
const MAX_RULES_PER_LIST: usize = 30;

/// The member the refusal must name in `Fields`: the input member that carried the list,
/// as every operation taking one spells it.
const LIST_MEMBER: &str = "PricingRuleArns";

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account — the world in which this payer-account-only
    /// control plane is reachable at all (see `tests/all/payer_account_gate.rs` for the
    /// denial every other caller gets).
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

    /// Send one request over the wire and return the status plus the parsed body.
    async fn call(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
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

    /// `CreatePricingRule`: the full ARN of a rule that now exists.
    async fn create_rule(&self, name: &str) -> String {
        let body = json!({"Name": name, "Scope": "GLOBAL", "Type": "MARKUP",
                          "ModifierPercentage": 10.0});
        let (status, parsed) = self.call("POST", CREATE_RULE_PATH, &body).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingRule {body}: expected 200, got {status} with {parsed}"
        );
        parsed["Arn"]
            .as_str()
            .expect("setup CreatePricingRule: no Arn in the response")
            .to_string()
    }

    /// `count` rules that exist, in the order the service minted them.
    async fn create_rules(&self, prefix: &str, count: usize) -> Vec<String> {
        let mut minted = Vec::with_capacity(count);
        for index in 0..count {
            minted.push(self.create_rule(&format!("{prefix}{index}")).await);
        }
        minted
    }

    /// `DeletePricingRule`, asserting the setup succeeded: the rule stops existing.
    async fn delete_rule(&self, rule: &str) {
        let (status, parsed) = self
            .call("POST", DELETE_RULE_PATH, &json!({"Arn": rule}))
            .await;
        assert_eq!(
            status, 200,
            "setup DeletePricingRule {rule}: expected 200, got {status} with {parsed}"
        );
    }

    /// `CreatePricingPlan` with the entries a row names.
    async fn create_plan(&self, name: &str, rules: &[String]) -> (u16, Value) {
        self.call(
            "POST",
            CREATE_PLAN_PATH,
            &json!({"Name": name, "PricingRuleArns": rules}),
        )
        .await
    }

    /// `CreatePricingPlan`, asserting it was served: the plan's full ARN.
    async fn create_plan_ok(&self, name: &str, rules: &[String]) -> String {
        let (status, parsed) = self.create_plan(name, rules).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {name:?} with {rules:?}: expected 200, got {status} with \
             {parsed}"
        );
        parsed["Arn"]
            .as_str()
            .expect("setup CreatePricingPlan: no Arn in the response")
            .to_string()
    }

    /// The FULL rule ARNs the plan associates, read through the read-only reader.
    async fn associated_rules(&self, plan: &str) -> Vec<String> {
        let (status, parsed) = self
            .call("POST", LIST_RULES_PATH, &json!({"PricingPlanArn": plan}))
            .await;
        assert_eq!(
            status, 200,
            "ListPricingRulesAssociatedToPricingPlan {plan}: expected 200, got {status} with \
             {parsed}"
        );
        parsed["PricingRuleArns"]
            .as_array()
            .map(|arns| {
                arns.iter()
                    .map(|arn| arn.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every pricing plan ARN state holds, in the order the listing reports them.
    async fn stored_plans(&self) -> Vec<String> {
        let (status, parsed) = self.call("POST", LIST_PLANS_PATH, &json!({})).await;
        assert_eq!(
            status, 200,
            "ListPricingPlans: expected 200, got {status} with {parsed}"
        );
        parsed["PricingPlans"]
            .as_array()
            .map(|plans| {
                plans
                    .iter()
                    .map(|plan| plan["Arn"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or("<none>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The full `PricingRuleArn` of a rule id held by this account — the spelling the
/// service normalises every entry to, and so the one a refusal names.
fn rule_arn(id: &str) -> String {
    format!(
        "arn:aws:billingconductor::{}:pricingrule/{id}",
        ::engine::emulator_account_id()
    )
}

/// The bare resource id of a full ARN — the other spelling the modelled
/// `PricingRuleArn` pattern admits for the same rule.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// A well-formed rule id (`[a-zA-Z0-9]{10}`) that names no rule this account holds: the
/// service mints ids from a monotonic counter, so an id of this form is never one of
/// them.
fn unknown_rule_id(index: usize) -> String {
    format!("NOSUCHRUL{index}")
}

/// Assert one refusal: the modelled 400 `ValidationException`, the reason the model
/// enumerates for the fault, the input member at fault in `Fields`, and a message that
/// names the offending rule by the full ARN the service resolved it to.
fn assert_refusal(case: &str, status: u16, body: &Value, reason: &str, offending: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(reason),
        "case {case:?}: expected Reason {reason:?}, got {:?}",
        body["Reason"].as_str()
    );
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some(LIST_MEMBER),
        "case {case:?}: expected the refusal to name the member at fault, got {body}"
    );
    let message = body["Message"].as_str().unwrap_or_default();
    assert!(
        message.contains(offending),
        "case {case:?}: expected the message to name the offending rule {offending}, got \
         {message:?}"
    );
}

/// One row of the refusal table: the entries a `CreatePricingPlan` names and the reason
/// the list earns.
struct RefusalCase {
    label: &'static str,
    /// Builds the `PricingRuleArns` entries from rules that DO exist.
    entries: fn(&[String]) -> Vec<String>,
    /// The `ValidationExceptionReason` the model enumerates for this fault.
    reason: &'static str,
    /// The full ARN the message must name.
    offending: fn(&[String]) -> String,
}

/// RULE: a `CreatePricingPlan` whose `PricingRuleArns` names a rule that does not exist,
/// or names one rule more than once, is refused with the modelled 400
/// `ValidationException` and mints nothing.
#[tokio::test]
async fn a_create_naming_an_unresolvable_rule_list_is_refused_and_mints_nothing() {
    // Rows differ along the size of the list, the position of the faulty entry, the
    // spelling of each entry, and which of the two faults the list carries.
    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            label: "the only entry names no rule, as a bare id",
            entries: |_| vec![unknown_rule_id(1)],
            reason: "PRICINGRULES_NOT_EXIST",
            offending: |_| rule_arn(&unknown_rule_id(1)),
        },
        RefusalCase {
            label: "the only entry names no rule, as a full ARN",
            entries: |_| vec![rule_arn(&unknown_rule_id(2))],
            reason: "PRICINGRULES_NOT_EXIST",
            offending: |_| rule_arn(&unknown_rule_id(2)),
        },
        RefusalCase {
            label: "the FIRST of three entries names no rule",
            entries: |rules| {
                vec![
                    unknown_rule_id(3),
                    rules[0].clone(),
                    bare_id(&rules[1]),
                ]
            },
            reason: "PRICINGRULES_NOT_EXIST",
            offending: |_| rule_arn(&unknown_rule_id(3)),
        },
        RefusalCase {
            label: "the MIDDLE of three entries names no rule",
            entries: |rules| {
                vec![
                    rules[0].clone(),
                    rule_arn(&unknown_rule_id(4)),
                    rules[1].clone(),
                ]
            },
            reason: "PRICINGRULES_NOT_EXIST",
            offending: |_| rule_arn(&unknown_rule_id(4)),
        },
        RefusalCase {
            label: "the LAST entry of a list at the modelled maximum names no rule",
            entries: |rules| {
                let mut entries: Vec<String> = rules[..MAX_RULES_PER_LIST - 1].to_vec();
                entries.push(unknown_rule_id(5));
                entries
            },
            reason: "PRICINGRULES_NOT_EXIST",
            offending: |_| rule_arn(&unknown_rule_id(5)),
        },
        RefusalCase {
            label: "one existing rule named twice, both times as a full ARN",
            entries: |rules| vec![rules[2].clone(), rules[2].clone()],
            reason: "DUPLICATE_PRICINGRULE_ARNS",
            offending: |rules| rules[2].clone(),
        },
        RefusalCase {
            label: "one existing rule named twice, once in each spelling",
            entries: |rules| vec![bare_id(&rules[3]), rules[3].clone()],
            reason: "DUPLICATE_PRICINGRULE_ARNS",
            offending: |rules| rules[3].clone(),
        },
        RefusalCase {
            label: "a repeat at the two ends of a list of three",
            entries: |rules| {
                vec![
                    rules[4].clone(),
                    rules[5].clone(),
                    bare_id(&rules[4]),
                ]
            },
            reason: "DUPLICATE_PRICINGRULE_ARNS",
            offending: |rules| rules[4].clone(),
        },
        RefusalCase {
            label: "a list at the modelled maximum whose last entry repeats its first",
            entries: |rules| {
                let mut entries: Vec<String> = rules[..MAX_RULES_PER_LIST - 1].to_vec();
                entries.push(bare_id(&rules[0]));
                entries
            },
            reason: "DUPLICATE_PRICINGRULE_ARNS",
            offending: |rules| rules[0].clone(),
        },
        RefusalCase {
            label: "a rule that exists nowhere, named twice: the repeat is decided from \
                    the input alone",
            entries: |_| vec![unknown_rule_id(6), rule_arn(&unknown_rule_id(6))],
            reason: "DUPLICATE_PRICINGRULE_ARNS",
            offending: |_| rule_arn(&unknown_rule_id(6)),
        },
    ];

    let em = Emulator::new();
    let rules = em.create_rules("ExistingRule", MAX_RULES_PER_LIST).await;
    // One plan this account does hold, so every row can also show the refusal left
    // state exactly as it was.
    let bystander = em
        .create_plan_ok("BystanderPlan", std::slice::from_ref(&rules[0]))
        .await;
    let before = em.stored_plans().await;

    for case in &cases {
        let entries = (case.entries)(&rules);
        let name = "RefusedPlan";
        let (status, body) = em.create_plan(name, &entries).await;
        assert_refusal(
            case.label,
            status,
            &body,
            case.reason,
            &(case.offending)(&rules),
        );
        assert_eq!(
            body["Arn"].as_str(),
            None,
            "case {:?}: a refused create must mint no ARN, got {body}",
            case.label
        );

        // Nothing was minted: the plan set is as it was, and the name the refused
        // request asked for was never claimed.
        assert_eq!(
            em.stored_plans().await,
            before,
            "case {:?}: expected the refusal to leave the stored plans alone",
            case.label
        );
        assert_eq!(
            em.associated_rules(&bystander).await,
            vec![rules[0].clone()],
            "case {:?}: expected the refusal to leave this account's plan alone",
            case.label
        );
    }

    // The name every refused row asked for is still free — the refusal claimed none of
    // it — so a create naming only rules that exist takes it.
    let accepted = em
        .create_plan_ok("RefusedPlan", std::slice::from_ref(&rules[1]))
        .await;
    assert_eq!(
        em.associated_rules(&accepted).await,
        vec![rules[1].clone()],
        "the create that names only rules that exist must record exactly them"
    );
}

/// RULE (the other side of it): a list whose every entry resolves to a rule that exists
/// is served, whatever its size and whichever spelling each entry uses, and the
/// associations read back through the read-only reader.
#[tokio::test]
async fn a_create_whose_every_entry_names_a_rule_that_exists_is_served() {
    // Rows differ along the number of entries (none at all, one, two, the modelled
    // maximum) and the spelling of each.
    type Entries = fn(&[String]) -> Vec<String>;
    let cases: Vec<(&str, Entries)> = vec![
        ("no entry at all", |_| Vec::new()),
        ("one bare id", |rules| vec![bare_id(&rules[0])]),
        ("one full ARN", |rules| vec![rules[1].clone()]),
        ("two entries, one in each spelling", |rules| {
            vec![bare_id(&rules[2]), rules[3].clone()]
        }),
        ("the modelled maximum of thirty entries", |rules| {
            rules[..MAX_RULES_PER_LIST].to_vec()
        }),
    ];

    let em = Emulator::new();
    let rules = em.create_rules("ServedRule", MAX_RULES_PER_LIST).await;

    for (index, (case, entries)) in cases.iter().enumerate() {
        let named = entries(&rules);
        let plan = em
            .create_plan_ok(&format!("ServedPlan{index}"), &named)
            .await;

        // Whatever spelling the create used, the plan associates the RULES — reported
        // as full ARNs, in ARN order.
        let mut expected: Vec<String> = named
            .iter()
            .map(|entry| {
                if entry.starts_with("arn:") {
                    entry.clone()
                } else {
                    rule_arn(entry)
                }
            })
            .collect();
        expected.sort();
        assert_eq!(
            em.associated_rules(&plan).await,
            expected,
            "case {case:?}: expected the plan to associate exactly the rules the create \
             named"
        );
    }
}

/// RULE: existence is decided against the rules state holds AT THE CALL, not against
/// the shape of the ARN — a rule that was created and then deleted can no longer be
/// named.
#[tokio::test]
async fn a_rule_that_was_deleted_can_no_longer_be_named() {
    let em = Emulator::new();
    let rules = em.create_rules("TransientRule", 2).await;

    // While it exists, a create naming it is served.
    let plan = em
        .create_plan_ok("BeforeDeletePlan", std::slice::from_ref(&rules[0]))
        .await;
    assert_eq!(
        em.associated_rules(&plan).await,
        vec![rules[0].clone()],
        "case \"before the delete\": expected the plan to associate the rule it named"
    );

    // The second rule is associated by nothing, so it can be deleted.
    em.delete_rule(&rules[1]).await;

    // Rows differ along the spelling the create names the deleted rule with.
    for (case, entry) in [
        ("the full ARN of a deleted rule", rules[1].clone()),
        ("the bare id of a deleted rule", bare_id(&rules[1])),
    ] {
        let (status, body) = em
            .create_plan("AfterDeletePlan", std::slice::from_ref(&entry))
            .await;
        assert_refusal(
            case,
            status,
            &body,
            "PRICINGRULES_NOT_EXIST",
            &rules[1].clone(),
        );
    }
}

/// RULE: the list is resolved the same way whichever operation carries it — the two
/// faults earn the same modelled reasons from `CreatePricingPlan`,
/// `AssociatePricingRules` and `DisassociatePricingRules`.
#[tokio::test]
async fn every_operation_taking_a_rule_list_refuses_the_same_two_faults() {
    let em = Emulator::new();
    let rules = em.create_rules("SharedRule", 3).await;
    let plan = em
        .create_plan_ok("SharedPlan", std::slice::from_ref(&rules[0]))
        .await;

    // (case, method, path, the body minus the list, the entries, the reason owed).
    struct SharedCase {
        label: &'static str,
        method: &'static str,
        path: &'static str,
        /// The request body minus the `PricingRuleArns` the row adds.
        base: Value,
        entries: Vec<String>,
        reason: &'static str,
    }
    let cases: Vec<SharedCase> = vec![
        SharedCase {
            label: "CreatePricingPlan naming a rule that does not exist",
            method: "POST",
            path: CREATE_PLAN_PATH,
            base: json!({"Name": "SharedRefused"}),
            entries: vec![unknown_rule_id(7)],
            reason: "PRICINGRULES_NOT_EXIST",
        },
        SharedCase {
            label: "CreatePricingPlan naming one rule twice",
            method: "POST",
            path: CREATE_PLAN_PATH,
            base: json!({"Name": "SharedRefused"}),
            entries: vec![bare_id(&rules[1]), rules[1].clone()],
            reason: "DUPLICATE_PRICINGRULE_ARNS",
        },
        SharedCase {
            label: "AssociatePricingRules naming a rule that does not exist",
            method: "PUT",
            path: ASSOCIATE_PATH,
            base: json!({"Arn": plan.clone()}),
            entries: vec![unknown_rule_id(8)],
            reason: "PRICINGRULES_NOT_EXIST",
        },
        SharedCase {
            label: "AssociatePricingRules naming one rule twice",
            method: "PUT",
            path: ASSOCIATE_PATH,
            base: json!({"Arn": plan.clone()}),
            entries: vec![bare_id(&rules[2]), rules[2].clone()],
            reason: "DUPLICATE_PRICINGRULE_ARNS",
        },
        SharedCase {
            label: "DisassociatePricingRules naming a rule that does not exist",
            method: "PUT",
            path: DISASSOCIATE_PATH,
            base: json!({"Arn": plan.clone()}),
            entries: vec![unknown_rule_id(9)],
            reason: "PRICINGRULES_NOT_EXIST",
        },
        SharedCase {
            label: "DisassociatePricingRules naming one rule twice",
            method: "PUT",
            path: DISASSOCIATE_PATH,
            base: json!({"Arn": plan.clone()}),
            entries: vec![rules[0].clone(), bare_id(&rules[0])],
            reason: "DUPLICATE_PRICINGRULE_ARNS",
        },
    ];

    for case in &cases {
        let mut body = case.base.clone();
        body[LIST_MEMBER] = json!(case.entries);
        let (status, answered) = em.call(case.method, case.path, &body).await;
        let first = case.entries[0].as_str();
        let offending = if first.starts_with("arn:") {
            first.to_string()
        } else {
            rule_arn(first)
        };
        assert_refusal(case.label, status, &answered, case.reason, &offending);

        // Every refusal left the plan's associations exactly as they were.
        assert_eq!(
            em.associated_rules(&plan).await,
            vec![rules[0].clone()],
            "case {:?}: expected the refusal to leave the plan's associations alone",
            case.label
        );
    }
}
