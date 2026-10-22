// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the pricing-rule slice — `CreatePricingRule`,
//! `ListPricingRules` and `DeletePricingRule` — driven over the service's real
//! `restJson1` wire through [`MultiServiceRouter::dispatch`], the same path the
//! production router serves.
//!
//! RULE: a pricing rule is a stored resource identified by its `PricingRuleArn` in
//! either spelling the pattern allows (the full ARN or the bare
//! `[a-zA-Z0-9]{10}` id). `DeletePricingRule` removes exactly the rule that ARN
//! names, so afterwards a read no longer reports it, its name is free again and its
//! `ClientToken` no longer replays into it; a second delete of the same ARN, like any
//! ARN naming no stored rule, is refused with the modelled `ValidationException`
//! (`PRICINGRULES_NOT_EXIST`), and a rule a pricing plan still names is refused with
//! the modelled `ConflictException`
//! (`PRICING_RULE_ATTACHED_TO_PRICING_PLAN_DELETE_CONFLICT`) and stays in state.
//!
//! Every test asserts that rule over a TABLE whose rows differ along what the rule
//! generalises over — the rule's `Scope`/`Type` and the members those make
//! meaningful, which spelling of the ARN the call uses, how many pricing plans name
//! the rule, and the page size a read asks for — rather than along one recorded call.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The scalar `PricingRuleListElement` members a read echoes straight back out of
/// state, exactly as `CreatePricingRule` accepted them
/// (`smithy_query(query="shape",shape="PricingRuleListElement")`).
const ECHOED_MEMBERS: &[&str] = &[
    "Name",
    "Description",
    "Scope",
    "Type",
    "ModifierPercentage",
    "Service",
    "BillingEntity",
    "UsageType",
    "Operation",
];

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// POST a JSON body to one of this service's routes and return the status plus
    /// the parsed response body.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        self.post_with_token(path, body, None).await
    }

    /// As [`Self::post`], with the `ClientToken` in the header the model binds it to
    /// (`X-Amzn-Client-Token`).
    async fn post_with_token(
        &self,
        path: &str,
        body: &Value,
        client_token: Option<&str>,
    ) -> (u16, Value) {
        let mut builder = http::Request::builder()
            .method("POST")
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
            );
        if let Some(token) = client_token {
            builder = builder.header("x-amzn-client-token", token);
        }
        let request = builder
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
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Does `value` match the full form of the modelled `PricingRuleArn` pattern,
/// `arn:aws:billingconductor::[0-9]{12}:pricingrule/[a-zA-Z0-9]{10}`?
fn is_pricing_rule_arn(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:aws:billingconductor::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    let Some(id) = resource.strip_prefix("pricingrule/") else {
        return false;
    };
    account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && id.len() == 10
        && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The bare resource id of a full ARN — the other spelling the `PricingRuleArn`
/// pattern accepts for the same resource.
fn bare_id(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or(arn)
}

/// `CreatePricingRule`, asserting it succeeded, and the ARN it minted.
async fn create_rule(em: &Emulator, case: &str, input: &Value) -> String {
    let (status, body) = em.post("/create-pricing-rule", input).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePricingRule expected 200, got {status} with {body:?}"
    );
    let arn = body["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreatePricingRule returned no Arn: {body:?}"))
        .to_string();
    assert!(
        is_pricing_rule_arn(&arn),
        "case {case:?}: expected a PricingRuleArn, got {arn:?}"
    );
    arn
}

/// `ListPricingRules`, asserting it succeeded, and the page it reported.
async fn list_rules(em: &Emulator, case: &str, input: &Value) -> (Vec<Value>, Option<String>) {
    let (status, body) = em.post("/list-pricing-rules", input).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPricingRules expected 200, got {status} with {body:?}"
    );
    let rules = body["PricingRules"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListPricingRules reported no PricingRules list"))
        .clone();
    let next = body["NextToken"].as_str().map(str::to_string);
    (rules, next)
}

/// Every stored rule's ARN, read back through `ListPricingRules`.
async fn listed_arns(em: &Emulator, case: &str) -> Vec<String> {
    let (rules, next) = list_rules(em, case, &json!({})).await;
    assert_eq!(
        next, None,
        "case {case:?}: a default page over this suite's rules needs no NextToken, got {next:?}"
    );
    rules
        .iter()
        .map(|rule| rule["Arn"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The one element `ListPricingRules` reports for `arn`, or `None` when the rule is
/// not in state.
async fn read_rule(em: &Emulator, case: &str, arn: &str) -> Option<Value> {
    let (rules, _) = list_rules(em, case, &json!({"Filters": {"Arns": [arn]}})).await;
    assert!(
        rules.len() <= 1,
        "case {case:?}: an ARN filter names one rule, got {rules:?}"
    );
    rules.into_iter().next()
}

/// `CreatePricingPlan` naming `rule_arns`, asserting it succeeded.
async fn create_plan(em: &Emulator, case: &str, name: &str, rule_arns: &[&str]) -> String {
    let (status, body) = em
        .post(
            "/create-pricing-plan",
            &json!({"Name": name, "PricingRuleArns": rule_arns}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePricingPlan expected 200, got {status} with {body:?}"
    );
    body["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreatePricingPlan returned no Arn: {body:?}"))
        .to_string()
}

/// `DeletePricingRule` for `arn`.
async fn delete_rule(em: &Emulator, arn: &str) -> (u16, Value) {
    em.post("/delete-pricing-rule", &json!({"Arn": arn})).await
}

/// Assert the read reports exactly the members the create sent, and nothing the
/// create left out.
fn assert_echoes_the_create(case: &str, created: &Value, element: &Value) {
    for member in ECHOED_MEMBERS {
        assert_eq!(
            element.get(*member),
            created.get(*member),
            "case {case:?}: expected the read to report {member} as created \
             ({:?}), got {:?}",
            created.get(*member),
            element.get(*member)
        );
    }
    match created.get("Tiering") {
        None => assert!(
            element.get("Tiering").is_none(),
            "case {case:?}: a rule created without a Tiering reports none, got {element:?}"
        ),
        Some(sent) => {
            let read = element
                .get("Tiering")
                .unwrap_or_else(|| panic!("case {case:?}: the read reported no Tiering: {element:?}"));
            assert_eq!(
                read.get("FreeTier")
                    .and_then(|free| free.get("Activated"))
                    .and_then(Value::as_bool),
                sent.get("FreeTier")
                    .and_then(|free| free.get("Activated"))
                    .and_then(Value::as_bool),
                "case {case:?}: expected Tiering.FreeTier.Activated as created, got {read:?}"
            );
            let sent_tiers = sent.get("CustomTiers").and_then(Value::as_array);
            let read_tiers = read.get("CustomTiers").and_then(Value::as_array);
            assert_eq!(
                sent_tiers.map(Vec::len),
                read_tiers.map(Vec::len),
                "case {case:?}: expected as many CustomTiers as created, got {read:?}"
            );
            for (index, sent_tier) in sent_tiers.into_iter().flatten().enumerate() {
                let read_tier = &read_tiers.expect("checked above")[index];
                for member in ["BeginRangeInclusive", "EndRangeExclusive", "RateValue"] {
                    assert_eq!(
                        read_tier.get(member).and_then(Value::as_f64),
                        sent_tier.get(member).and_then(Value::as_f64),
                        "case {case:?}: expected CustomTiers[{index}].{member} as created, got \
                         {read_tier:?}"
                    );
                }
            }
        }
    }
}

/// One row of the create/read/delete table: a rule the model's `Scope` / `Type`
/// combinations make meaningful, and the ARN spelling its delete uses.
struct Row {
    label: &'static str,
    input: Value,
    delete_by_bare_id: bool,
}

/// The table: one row per `PricingRuleScope` value (`GLOBAL`, `SERVICE`, `SKU`,
/// `BILLING_ENTITY`) crossed with the `PricingRuleType` values (`MARKUP`,
/// `DISCOUNT`, `TIERING`) and the members each of those makes meaningful
/// (`smithy_query(query="shape",shape="PricingRuleScope,PricingRuleType")`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "GLOBAL MARKUP, no optional members",
            input: json!({
                "Name": "GlobalMarkup",
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 12.5,
            }),
            delete_by_bare_id: false,
        },
        Row {
            label: "SERVICE DISCOUNT with a description",
            input: json!({
                "Name": "Service_Discount-1",
                "Description": "ten percent off compute",
                "Scope": "SERVICE",
                "Type": "DISCOUNT",
                "ModifierPercentage": 10.0,
                "Service": "AmazonEC2",
            }),
            delete_by_bare_id: true,
        },
        Row {
            label: "SKU DISCOUNT, the usage type and operation a SKU scope names",
            input: json!({
                "Name": "Sku.Discount@2",
                "Scope": "SKU",
                "Type": "DISCOUNT",
                "ModifierPercentage": 1.25,
                "Service": "AmazonEC2",
                "UsageType": "USW2-BoxUsage:m2.2xlarge",
                "Operation": "RunInstances:0202",
            }),
            delete_by_bare_id: false,
        },
        Row {
            label: "BILLING_ENTITY MARKUP at the bottom of the modelled range",
            input: json!({
                "Name": "Marketplace+Markup",
                "Scope": "BILLING_ENTITY",
                "Type": "MARKUP",
                "ModifierPercentage": 0.0,
                "BillingEntity": "AWS Marketplace",
            }),
            delete_by_bare_id: true,
        },
        Row {
            label: "GLOBAL TIERING with only a free tier",
            input: json!({
                "Name": "FreeTierOnly",
                "Scope": "GLOBAL",
                "Type": "TIERING",
                "Tiering": {"FreeTier": {"Activated": true}},
            }),
            delete_by_bare_id: false,
        },
        Row {
            label: "GLOBAL TIERING with custom tiers, one of them open-ended",
            input: json!({
                "Name": "CustomTiers",
                "Scope": "GLOBAL",
                "Type": "TIERING",
                "Tiering": {
                    "FreeTier": {"Activated": true},
                    "CustomTiers": [
                        {"BeginRangeInclusive": 0.0, "EndRangeExclusive": 100.0, "RateValue": 0.5},
                        {"BeginRangeInclusive": 100.0, "RateValue": 0.25},
                    ],
                },
            }),
            delete_by_bare_id: true,
        },
    ]
}

/// RULE: a created pricing rule is reported by the read exactly as it was created,
/// and `DeletePricingRule` removes exactly the rule its `Arn` names — in either
/// spelling — leaving every other stored rule where it was.
#[tokio::test]
async fn delete_removes_exactly_the_rule_its_arn_names() {
    let em = Emulator::new();
    let table = rows();

    // Create every row, then read each one back: a create is only real if a later
    // read reports it.
    let mut arns: Vec<String> = Vec::new();
    for row in &table {
        let arn = create_rule(&em, row.label, &row.input).await;
        let element = read_rule(&em, row.label, &arn)
            .await
            .unwrap_or_else(|| panic!("case {:?}: the created rule is not readable", row.label));
        assert_echoes_the_create(row.label, &row.input, &element);
        assert_eq!(
            element["AssociatedPricingPlanCount"].as_i64().unwrap_or(0),
            0,
            "case {:?}: a rule no pricing plan names is associated with none, got {element:?}",
            row.label
        );
        let created_at = element["CreationTime"].as_i64().unwrap_or_default();
        assert!(
            created_at > 0,
            "case {:?}: expected a CreationTime in epoch millis, got {element:?}",
            row.label
        );
        assert_eq!(
            element["LastModifiedTime"].as_i64().unwrap_or_default(),
            created_at,
            "case {:?}: a rule nothing modified was last modified when it was created, got \
             {element:?}",
            row.label
        );
        arns.push(arn);
    }
    let mut remaining = arns.clone();
    assert_eq!(
        listed_arns(&em, "every created rule").await,
        remaining,
        "expected every created rule in state"
    );

    // Delete them one at a time, naming the rule by its full ARN on some rows and by
    // the bare id the same pattern allows on the others. Each answer names the full
    // ARN, and each delete removes that rule and only that rule.
    for (row, arn) in table.iter().zip(arns.iter()) {
        let named = if row.delete_by_bare_id {
            bare_id(arn)
        } else {
            arn.as_str()
        };
        let (status, body) = delete_rule(&em, named).await;
        assert_eq!(
            status, 200,
            "case {:?}: DeletePricingRule({named:?}) expected 200, got {status} with {body:?}",
            row.label
        );
        assert_eq!(
            body["Arn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: expected the deleted rule's full ARN back, got {body:?}",
            row.label
        );
        remaining.retain(|stored| stored != arn);
        assert_eq!(
            listed_arns(&em, row.label).await,
            remaining,
            "case {:?}: expected only the rules that were not deleted to remain",
            row.label
        );
        assert!(
            read_rule(&em, row.label, arn).await.is_none(),
            "case {:?}: the deleted rule is still readable",
            row.label
        );
    }
}

/// RULE: a pricing rule a pricing plan still names cannot be deleted — the modelled
/// `ConflictException` (HTTP 409) with the reason the model enumerates for it — and
/// the refused call leaves the rule in state. A rule no plan names deletes.
#[tokio::test]
async fn a_rule_a_pricing_plan_names_cannot_be_deleted() {
    let em = Emulator::new();

    // The rows differ along how the plan spells the rule's ARN and how many plans
    // name it; the last row is the control — the same delete with no plan at all.
    let cases: Vec<(&str, usize, bool)> = vec![
        ("one plan naming the rule by its full ARN", 1, false),
        ("one plan naming the rule by its bare id", 1, true),
        ("two plans naming the rule", 2, false),
        ("no plan naming the rule", 0, false),
    ];
    for (index, (case, plans, by_bare_id)) in cases.iter().enumerate() {
        let rule = create_rule(
            &em,
            case,
            &json!({
                "Name": format!("ConflictRule{index}"),
                "Scope": "GLOBAL",
                "Type": "MARKUP",
                "ModifierPercentage": 5.0,
            }),
        )
        .await;
        let named_in_plan = if *by_bare_id {
            bare_id(&rule).to_string()
        } else {
            rule.clone()
        };
        for plan in 0..*plans {
            create_plan(
                &em,
                case,
                &format!("ConflictPlan{index}_{plan}"),
                &[named_in_plan.as_str()],
            )
            .await;
        }

        // The association count a read reports moves with the plans, whichever
        // spelling they used.
        let element = read_rule(&em, case, &rule)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the created rule is not readable"));
        assert_eq!(
            element["AssociatedPricingPlanCount"].as_i64().unwrap_or(0),
            i64::try_from(*plans).expect("a small count"),
            "case {case:?}: expected AssociatedPricingPlanCount {plans}, got {element:?}"
        );

        let (status, body) = delete_rule(&em, &rule).await;
        if *plans == 0 {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the delete to succeed, got {status} with {body:?}"
            );
            assert!(
                read_rule(&em, case, &rule).await.is_none(),
                "case {case:?}: the deleted rule is still readable"
            );
            continue;
        }
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("PRICING_RULE_ATTACHED_TO_PRICING_PLAN_DELETE_CONFLICT"),
            "case {case:?}: expected the reason the model enumerates for this conflict, got \
             {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(rule.as_str()),
            "case {case:?}: expected the conflict to name the rule in use, got {body:?}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("pricingrule"),
            "case {case:?}: expected the conflict to name the resource type, got {body:?}"
        );
        assert!(
            read_rule(&em, case, &rule).await.is_some(),
            "case {case:?}: a refused delete must leave the rule in state"
        );
    }
}

/// RULE: a `DeletePricingRule` whose `Arn` names no stored rule — never created, in
/// another account or another partition, or already deleted — is refused with the
/// modelled `ValidationException` (HTTP 400), the `PRICINGRULES_NOT_EXIST` reason the
/// model enumerates, and a `Message` built from the request.
#[tokio::test]
async fn deleting_a_rule_that_is_not_stored_is_refused() {
    let em = Emulator::new();
    let doomed = create_rule(
        &em,
        "a rule to delete twice",
        &json!({
            "Name": "DeletedTwice",
            "Scope": "GLOBAL",
            "Type": "DISCOUNT",
            "ModifierPercentage": 3.0,
        }),
    )
    .await;
    let (status, body) = delete_rule(&em, &doomed).await;
    assert_eq!(
        status, 200,
        "setup DeletePricingRule: expected 200, got {status} with {body:?}"
    );

    // The rows differ along WHY no rule is stored under the ARN: never minted (in
    // both spellings the pattern allows), minted in another account or another
    // partition, or deleted a moment ago (again in both spellings).
    let cases: Vec<(&str, String)> = vec![
        ("a bare id that was never created", "ZZZZZZZZZZ".to_string()),
        (
            "a full ARN that was never created",
            "arn:aws:billingconductor::000000000000:pricingrule/AAAAAAAAAA".to_string(),
        ),
        (
            "a rule in another account",
            "arn:aws:billingconductor::123456789012:pricingrule/BBBBBBBBBB".to_string(),
        ),
        (
            "a rule in the China partition",
            "arn:aws-cn:billingconductor::000000000000:pricingrule/CCCCCCCCCC".to_string(),
        ),
        ("the full ARN of a deleted rule", doomed.clone()),
        ("the bare id of a deleted rule", bare_id(&doomed).to_string()),
    ];
    for (case, arn) in &cases {
        let (status, body) = delete_rule(&em, arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("PRICINGRULES_NOT_EXIST"),
            "case {case:?}: expected the PRICINGRULES_NOT_EXIST reason, got {body:?}"
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("Arn"),
            "case {case:?}: expected the refusal to name the member at fault, got {body:?}"
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(bare_id(arn)),
            "case {case:?}: expected the Message to name the rule asked for, got {message:?}"
        );
    }
}

/// RULE: deleting a rule releases everything that only existed because of it — its
/// name (unique per rule) and the `ClientToken` entry that replayed into it — so the
/// same create can be made again and mints a NEW rule rather than resurrecting the
/// deleted one.
#[tokio::test]
async fn a_deleted_rule_frees_its_name_and_its_client_token() {
    let em = Emulator::new();
    // The rows differ along whether the repeated create carries the token of the
    // first one: a replayed token must not point at a rule that no longer exists.
    for (case, token) in [
        ("the same ClientToken as the deleted rule", Some("token-1")),
        ("no ClientToken at all", None),
    ] {
        let input = json!({
            "Name": "ReusableName",
            "Scope": "SERVICE",
            "Type": "MARKUP",
            "ModifierPercentage": 4.5,
            "Service": "AmazonS3",
        });
        let (status, body) = em
            .post_with_token("/create-pricing-rule", &input, token)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreatePricingRule expected 200, got {status} with {body:?}"
        );
        let first = body["Arn"].as_str().unwrap_or_default().to_string();

        // While the rule exists, the name is taken and the token replays into it.
        let (status, body) = em
            .post_with_token("/create-pricing-rule", &input, token)
            .await;
        if token.is_some() {
            assert_eq!(
                status, 200,
                "case {case:?}: a replayed ClientToken completes successfully, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                body["Arn"].as_str(),
                Some(first.as_str()),
                "case {case:?}: a replayed ClientToken answers with the first ARN, got {body:?}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {case:?}: a second rule with a taken name is a conflict, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {body:?}"
            );
        }
        assert_eq!(
            listed_arns(&em, case).await,
            vec![first.clone()],
            "case {case:?}: expected exactly one stored rule"
        );

        let (status, body) = delete_rule(&em, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeletePricingRule expected 200, got {status} with {body:?}"
        );

        // The name is free again, and the token no longer names the deleted rule.
        let (status, body) = em
            .post_with_token("/create-pricing-rule", &input, token)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the name of a deleted rule is reusable, got {status} with {body:?}"
        );
        let second = body["Arn"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            second, first,
            "case {case:?}: the re-created rule is a NEW rule, not the deleted one"
        );
        assert_eq!(
            listed_arns(&em, case).await,
            vec![second.clone()],
            "case {case:?}: expected only the re-created rule in state"
        );

        // Leave the store empty for the next row.
        let (status, body) = delete_rule(&em, &second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: teardown DeletePricingRule expected 200, got {status} with {body:?}"
        );
    }
}

/// RULE: the read reports every stored rule across its pages — a page holds at most
/// the `MaxResults` asked for, and the `NextToken` resumes where the page stopped.
#[tokio::test]
async fn the_read_pages_over_every_stored_rule() {
    let em = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        created.push(
            create_rule(
                &em,
                "a rule to page over",
                &json!({
                    "Name": format!("PagedRule{index}"),
                    "Scope": "GLOBAL",
                    "Type": "MARKUP",
                    "ModifierPercentage": 1.0,
                }),
            )
            .await,
        );
    }

    // The rows differ along the page size: the modelled minimum, sizes that do and do
    // not divide the stored count, exactly the count, and the modelled maximum
    // (`MaxPricingRuleResults`, range 1..100).
    for size in [1, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = json!({"MaxResults": size});
            if let Some(next) = &token {
                input["NextToken"] = json!(next);
            }
            let (rules, next) = list_rules(&em, "paging", &input).await;
            assert!(
                rules.len() <= usize::try_from(size).expect("a small page size"),
                "case MaxResults={size}: a page holds at most the size asked for, got {rules:?}"
            );
            seen.extend(
                rules
                    .iter()
                    .map(|rule| rule["Arn"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case MaxResults={size}: paging did not terminate after {pages} pages"
            );
            token = next;
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "case MaxResults={size}: expected every stored rule, once, in ARN order"
        );
    }
}
