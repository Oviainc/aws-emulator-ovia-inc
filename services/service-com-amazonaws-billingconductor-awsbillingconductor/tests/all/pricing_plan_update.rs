// SPDX-License-Identifier: CC0-1.0

//! `UpdatePricingPlan` end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`] — the same path the production router serves.
//!
//! RULE under test: `UpdatePricingPlan` resolves the pricing plan its `Arn` names (the
//! modelled `PricingPlanArn` pattern spells that either as the full ARN or as the bare
//! id, and both name ONE record), replaces exactly the members the request carries
//! (`Name`, `Description`), keeps every member it omits at the value state already
//! holds, moves `LastModifiedTime` to now, and answers with the plan's whole state
//! afterwards — including `Size`, "the pricing rules count that's currently associated
//! with this pricing plan", derived from the plan's associations rather than stored. A
//! `Name` another plan holds is refused 409 `ConflictException`
//! (`RESOURCE_NAME_CONFLICT`: "The name must be unique to each pricing plan"), an `Arn`
//! no plan of this account answers to is 404 `ResourceNotFoundException`, and one of the
//! plans Amazon Web Services owns — the `BasicPricingPlan` / `Passthrough` ids the
//! `PricingPlanArn` pattern admits beside a customer-minted one, whose ARN carries the
//! literal `aws` account segment — is refused with this operation's modelled 400
//! `ValidationException` (`MISMATCHED_PRICINGPLAN_ARN`), because it is not a plan this
//! account may modify. A refusal writes nothing.
//!
//! Every table's rows differ along what the rule generalises over: which optional
//! members the request names (none, one, both), the member class they fall in (a name
//! under a pattern, a free-text description, including the empty string a length of 0
//! admits), how the `Arn` is spelled, how many pricing rules the plan associates (0, 1,
//! two spellings of one rule, 3, the modelled create maximum of 30), and which class the
//! addressed plan falls in (this account's, nobody's, Amazon Web Services').
//!
//! Each row's result is read back out of state through a SECOND call rather than taken
//! from the response of the call under test, so an assertion passes only if the change
//! was persisted: the plan's associations through the read-only
//! `ListPricingRulesAssociatedToPricingPlan`, its tags through the read-only
//! `ListTagsForResource`, its name through the uniqueness index `CreatePricingPlan`
//! consults, and its `Name` / `Description` / `Size` through a further `Arn`-only
//! `UpdatePricingPlan` (which names no member, so it reports stored state).
//! `ListPricingPlans` — the one reader that would report all of it at once — is still a
//! scaffold (`src/ops/list_pricing_plans.rs` carries the `SCAFFOLDED, NOT IMPLEMENTED`
//! marker), so it is not used here.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_billingconductor_awsbillingconductor::{
    ComAmazonawsBillingconductorAwsbillingconductorState as BillingConductorState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

// An ARN in a non-greedy `@httpLabel` travels as one path segment, with its `/` and `:`
// percent-encoded — exactly what an SDK client sends. That encoding rule has ONE home in
// this test target, beside the tagging routes it is the subject of.
use crate::tagging_surface::encode_label;

/// `PUT /update-pricing-plan` — the operation's modelled HTTP binding
/// (`smithy_query(query="operation",operation="UpdatePricingPlan")`).
const UPDATE_PATH: &str = "/update-pricing-plan";

/// `POST /create-pricing-plan`, which gives this suite something to update.
const CREATE_PATH: &str = "/create-pricing-plan";

/// `POST /list-pricing-rules-associated-to-pricing-plan` — the read-only reader of the
/// associations `Size` counts.
const LIST_RULES_PATH: &str = "/list-pricing-rules-associated-to-pricing-plan";

/// `ListTagsForResource` answers on its modelled `@http(code: 204)`, with `Tags` in the
/// body (`smithy_query(query="operation",operation="ListTagsForResource")`).
const LIST_TAGS_STATUS: u16 = 204;

/// The pricing plan ids Amazon Web Services owns rather than the caller, enumerated by
/// the modelled `PricingPlanArn` pattern beside the customer-minted `[a-zA-Z0-9]{10}`
/// form (`smithy_query(query="shape",shape="PricingPlanArn")`).
const AWS_MANAGED_PLAN_IDS: &[&str] = &["BasicPricingPlan", "Passthrough"];

/// The modelled maximum length of `CreatePricingPlanInput.PricingRuleArns`
/// (`PricingRuleArnsInput`, len 0..30) — the largest association set one create can
/// record, and so the largest `Size` this suite can set up.
const MAX_ASSOCIATIONS_PER_CREATE: usize = 30;

/// A live emulator holding only this service.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A router over ONE shared engine, with the emulated account designated the
    /// organization's PAYER account.
    ///
    /// Billing Conductor is a payer-account-only API (see the `crate::service` module
    /// docs): its control plane is authorized for the payer account alone, and the store
    /// records no payer account by default — so this harness designates one, which is
    /// the world in which the plan lifecycle these tests are about is reachable at all.
    /// The denial every other caller gets is asserted in `tests/all/payer_account_gate.rs`,
    /// over the default store.
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
    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let builder = http::Request::builder()
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
            );
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        });
        let response = self
            .router
            .dispatch(builder.body(payload).expect("build the request"))
            .await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `CreatePricingPlan`: the created plan's full ARN.
    async fn create_plan(&self, body: &Value) -> String {
        let (status, parsed) = self.call("POST", CREATE_PATH, Some(body)).await;
        assert_eq!(
            status, 200,
            "setup CreatePricingPlan {body}: expected 200, got {status} with {parsed}"
        );
        parsed
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// `CreatePricingPlan` without asserting it succeeded — the probe that reads the
    /// name uniqueness index.
    async fn try_create_plan(&self, body: &Value) -> (u16, Value) {
        self.call("POST", CREATE_PATH, Some(body)).await
    }

    /// `UpdatePricingPlan` against `arn`, carrying the members of `patch` beside it.
    async fn update_plan(&self, arn: &str, patch: &Value) -> (u16, Value) {
        let mut body = json!({ "Arn": arn });
        if let Some(members) = patch.as_object() {
            for (name, value) in members {
                body[name] = value.clone();
            }
        }
        self.call("PUT", UPDATE_PATH, Some(&body)).await
    }

    /// The plan's persisted `Name` / `Description` / `Size`, read through an
    /// `Arn`-only `UpdatePricingPlan`: it names no member, so it replaces nothing and
    /// reports what state holds — a verification the call under test cannot satisfy by
    /// echoing its own request.
    async fn read_back(&self, arn: &str) -> Value {
        let (status, parsed) = self.update_plan(arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "read-back UpdatePricingPlan {arn}: expected 200, got {status} with {parsed}"
        );
        parsed
    }

    /// The FULL rule ARNs `ListPricingRulesAssociatedToPricingPlan` reports for a plan —
    /// an independent, read-only view of the associations `Size` counts.
    async fn associated_rules(&self, arn: &str) -> Vec<String> {
        let (status, parsed) = self
            .call(
                "POST",
                LIST_RULES_PATH,
                Some(&json!({ "PricingPlanArn": arn })),
            )
            .await;
        assert_eq!(
            status, 200,
            "ListPricingRulesAssociatedToPricingPlan {arn}: expected 200, got {status} with \
             {parsed}"
        );
        parsed
            .get("PricingRuleArns")
            .and_then(Value::as_array)
            .map(|arns| {
                arns.iter()
                    .map(|arn| arn.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The plan's tags, read through `ListTagsForResource` (`GET /tags/{ResourceArn}`,
    /// the ARN travelling as ONE percent-encoded path segment).
    async fn tags(&self, arn: &str) -> Value {
        let path = format!("/tags/{}", encode_label(arn));
        let (status, parsed) = self.call("GET", &path, None).await;
        assert_eq!(
            status, LIST_TAGS_STATUS,
            "ListTagsForResource {arn}: expected {LIST_TAGS_STATUS}, got {status} with {parsed}"
        );
        parsed.get("Tags").cloned().unwrap_or(Value::Null)
    }
}

/// The wire error code of a failure response (`__type`), without any namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The emulator's account id — the account every ARN this service mints carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The full `PricingPlanArn` of a plan id held by this account.
fn plan_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::{}:pricingplan/{id}", account())
}

/// The full `PricingRuleArn` of a rule id held by this account.
fn rule_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::{}:pricingrule/{id}", account())
}

/// The full ARN of an AWS-managed pricing plan: the `PricingPlanArn` pattern spells the
/// account segment of those as the literal `aws`.
fn managed_plan_arn(id: &str) -> String {
    format!("arn:aws:billingconductor::aws:pricingplan/{id}")
}

/// The bare-id form of a full ARN — the other spelling the modelled pattern admits for
/// the same resource.
fn bare_id(arn: &str) -> String {
    arn.rsplit('/').next().unwrap_or_default().to_string()
}

/// A well-formed 10-character rule id (`[a-zA-Z0-9]{10}`) derived from `index`.
fn rule_id(index: usize) -> String {
    format!("RULE{index:06}")
}

/// A member of a response body, as a string (absent reads as `None`).
fn text(body: &Value, member: &str) -> Option<String> {
    body.get(member).and_then(Value::as_str).map(str::to_string)
}

/// A member of a response body, as an integer (absent reads as `None`).
fn integer(body: &Value, member: &str) -> Option<i64> {
    body.get(member).and_then(Value::as_i64)
}

/// Wall-clock millis since the epoch — the unit the modelled `Instant` long carries, so
/// a test can bracket the `LastModifiedTime` a call must report.
fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("the host clock is after the epoch")
            .as_millis(),
    )
    .expect("the epoch millis fit in an i64")
}

/// The request that creates the plan the member tables start from: both members
/// `UpdatePricingPlan` can change carry a known starting value, and the plan also holds
/// state no update may touch (two rule associations and a tag).
fn base_plan(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "the plan as created",
        "PricingRuleArns": [rule_id(1), rule_arn(&rule_id(2))],
        "Tags": { "team": "billing" },
    })
}

/// How one row spells the plan ARN its request addresses: both spellings the modelled
/// `PricingPlanArn` pattern admits name the same record.
type ArnSpelling = fn(&str) -> String;

/// One row of the member-replacement table.
struct UpdateCase {
    label: &'static str,
    /// The members the `UpdatePricingPlan` request names beside `Arn`.
    patch: Value,
    /// `Name` and `Description` as the plan must report them after the call.
    name: &'static str,
    description: Option<&'static str>,
}

#[tokio::test]
async fn update_replaces_the_members_it_names_and_keeps_the_rest() {
    let cases = [
        UpdateCase {
            label: "no member named: the plan is untouched",
            patch: json!({}),
            name: "BasePlan",
            description: Some("the plan as created"),
        },
        UpdateCase {
            label: "description only",
            patch: json!({ "Description": "a second description" }),
            name: "BasePlan",
            description: Some("a second description"),
        },
        UpdateCase {
            label: "an empty description is a value, not an omission",
            patch: json!({ "Description": "" }),
            name: "BasePlan",
            description: Some(""),
        },
        UpdateCase {
            label: "a description at the modelled maximum length",
            patch: json!({ "Description": "d".repeat(1024) }),
            name: "BasePlan",
            // Compared against the same 1024-character value below.
            description: None,
        },
        UpdateCase {
            label: "name only",
            patch: json!({ "Name": "RenamedPlan" }),
            name: "RenamedPlan",
            description: Some("the plan as created"),
        },
        UpdateCase {
            label: "name set to the name it already holds",
            patch: json!({ "Name": "BasePlan" }),
            name: "BasePlan",
            description: Some("the plan as created"),
        },
        UpdateCase {
            label: "a name using every character class its pattern admits",
            patch: json!({ "Name": "Plan_9+=.-@x" }),
            name: "Plan_9+=.-@x",
            description: Some("the plan as created"),
        },
        UpdateCase {
            label: "both members at once",
            patch: json!({ "Name": "BothAtOnce", "Description": "changed with the name" }),
            name: "BothAtOnce",
            description: Some("changed with the name"),
        },
    ];


    let long_description = "d".repeat(1024);
    for (index, case) in cases.iter().enumerate() {
        let em = Emulator::new();
        let arn = em.create_plan(&base_plan("BasePlan")).await;
        let expected_description = case
            .description
            .map_or(long_description.as_str(), |value| value);

        let (status, body) = em.update_plan(&arn, &case.patch).await;
        assert_eq!(
            status, 200,
            "case {:?} (row {index}): expected 200, got {status} with {body}",
            case.label
        );

        // The response, then the SAME plan read back out of state.
        for (label, reported) in [("the response", &body), ("state", &em.read_back(&arn).await)] {
            assert_eq!(
                text(reported, "Arn").as_deref(),
                Some(arn.as_str()),
                "case {:?} in {label}: expected Arn {arn:?}, got {:?}",
                case.label,
                text(reported, "Arn")
            );
            assert_eq!(
                text(reported, "Name").as_deref(),
                Some(case.name),
                "case {:?} in {label}: expected Name {:?}, got {:?}",
                case.label,
                case.name,
                text(reported, "Name")
            );
            assert_eq!(
                text(reported, "Description").as_deref(),
                Some(expected_description),
                "case {:?} in {label}: expected a Description of {} characters, got {:?}",
                case.label,
                expected_description.len(),
                text(reported, "Description").map(|value| value.len())
            );
            // `Size` counts the two rules the plan was created with; no update changes
            // the associations.
            assert_eq!(
                integer(reported, "Size"),
                Some(2),
                "case {:?} in {label}: expected Size 2, got {:?}",
                case.label,
                integer(reported, "Size")
            );
            assert!(
                integer(reported, "LastModifiedTime").is_some_and(|value| value > 0),
                "case {:?} in {label}: expected a LastModifiedTime in epoch millis, got {:?}",
                case.label,
                integer(reported, "LastModifiedTime")
            );
        }

        // Everything the operation does NOT own is as the create left it: the plan's
        // associations and its tags, each read through its own read-only operation.
        let mut expected_rules = vec![rule_arn(&rule_id(1)), rule_arn(&rule_id(2))];
        expected_rules.sort();
        assert_eq!(
            em.associated_rules(&arn).await,
            expected_rules,
            "case {:?}: expected the plan's rule associations to be untouched",
            case.label
        );
        assert_eq!(
            em.tags(&arn).await,
            json!({ "team": "billing" }),
            "case {:?}: expected the plan's tags to be untouched",
            case.label
        );
    }
}

#[tokio::test]
async fn size_reports_the_number_of_associated_pricing_rules() {
    // (case, the spellings the plan was created with, the count `Size` owes back). Two
    // spellings of ONE rule are ONE association, so the count is of RULES, not of
    // recorded strings.
    let many: Vec<String> = (0..MAX_ASSOCIATIONS_PER_CREATE).map(rule_id).collect();
    let rows: Vec<(&str, Vec<String>, i64)> = vec![
        ("no association at all", vec![], 0),
        ("one bare rule id", vec![rule_id(7)], 1),
        ("one full rule ARN", vec![rule_arn(&rule_id(8))], 1),
        (
            "both spellings of one rule",
            vec![rule_id(9), rule_arn(&rule_id(9))],
            1,
        ),
        (
            "three distinct rules",
            vec![rule_id(3), rule_id(4), rule_arn(&rule_id(5))],
            3,
        ),
        (
            "the modelled create maximum",
            many.clone(),
            i64::try_from(MAX_ASSOCIATIONS_PER_CREATE).expect("30 fits in an i64"),
        ),
    ];

    for (case, created_with, expected) in &rows {
        let em = Emulator::new();
        let arn = em
            .create_plan(&json!({ "Name": "SizedPlan", "PricingRuleArns": created_with }))
            .await;

        let (status, body) = em
            .update_plan(&arn, &json!({ "Description": "sized" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            integer(&body, "Size"),
            Some(*expected),
            "case {case:?}: expected Size {expected}, got {:?}",
            integer(&body, "Size")
        );

        // The same count, derived independently: the associations the read-only
        // `ListPricingRulesAssociatedToPricingPlan` reports for the plan.
        let listed = i64::try_from(em.associated_rules(&arn).await.len()).expect("a small count");
        assert_eq!(
            integer(&body, "Size"),
            Some(listed),
            "case {case:?}: expected Size to equal the {listed} associations the plan reports, \
             got {:?}",
            integer(&body, "Size")
        );
    }
}

#[tokio::test]
async fn either_spelling_of_the_arn_addresses_the_same_plan() {
    // (case, how the request spells the plan ARN).
    let spellings: [(&str, ArnSpelling); 2] = [
        ("the full ARN the create minted", |arn| arn.to_string()),
        ("the bare plan id", bare_id),
    ];

    for (case, spell) in &spellings {
        let em = Emulator::new();
        let arn = em.create_plan(&base_plan("BasePlan")).await;
        let addressed = spell(&arn);

        let (status, body) = em
            .update_plan(&addressed, &json!({ "Description": "addressed either way" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} ({addressed}): expected 200, got {status} with {body}"
        );
        // Whatever the request spelled, the resource is one record and the response
        // reports its full ARN.
        assert_eq!(
            text(&body, "Arn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the full ARN {arn:?} back, got {:?}",
            text(&body, "Arn")
        );
        let stored = em.read_back(&arn).await;
        assert_eq!(
            text(&stored, "Description").as_deref(),
            Some("addressed either way"),
            "case {case:?}: expected the update to reach the plan the full ARN names, got {:?}",
            text(&stored, "Description")
        );
    }
}

#[tokio::test]
async fn last_modified_time_moves_to_now_on_every_update() {
    let em = Emulator::new();
    let arn = em.create_plan(&base_plan("BasePlan")).await;

    // (case, the patch the update carries) — a rename, a description change and an
    // update that names nothing all move the timestamp, because the call is the event.
    let rounds: Vec<(&str, Value)> = vec![
        ("a description change", json!({ "Description": "first" })),
        ("a rename", json!({ "Name": "TimedPlan" })),
        ("an update naming no member", json!({})),
    ];

    let mut previous = 0_i64;
    for (case, patch) in &rounds {
        let before = now_millis();
        let (status, body) = em.update_plan(&arn, patch).await;
        let after = now_millis();
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let reported = integer(&body, "LastModifiedTime").unwrap_or_default();
        assert!(
            (before..=after).contains(&reported),
            "case {case:?}: expected a LastModifiedTime inside the call's own window \
             [{before}, {after}], got {reported}"
        );
        assert!(
            reported >= previous,
            "case {case:?}: expected LastModifiedTime not to go backwards from {previous}, got \
             {reported}"
        );
        previous = reported;
    }
}

#[tokio::test]
async fn a_rename_moves_the_name_uniqueness_index() {
    let em = Emulator::new();
    let first = em.create_plan(&base_plan("FirstPlan")).await;
    let second = em.create_plan(&json!({ "Name": "SecondPlan" })).await;

    // Renaming the first plan frees the name it held and takes the new one.
    let (status, body) = em
        .update_plan(&first, &json!({ "Name": "MovedPlan" }))
        .await;
    assert_eq!(
        status, 200,
        "case \"rename the first plan\": expected 200, got {status} with {body}"
    );

    // (case, the name a fresh create asks for, the status the index owes it).
    let probes: Vec<(&str, &str, u16)> = vec![
        ("the name the rename freed", "FirstPlan", 200),
        ("the name the rename took", "MovedPlan", 409),
        ("a name another plan still holds", "SecondPlan", 409),
        ("a name nobody holds", "ThirdPlan", 200),
    ];
    for (case, name, expected) in &probes {
        let (status, answered) = em.try_create_plan(&json!({ "Name": name })).await;
        assert_eq!(
            status, *expected,
            "case {case:?} ({name}): expected HTTP {expected} from CreatePricingPlan, got \
             {status} with {answered}"
        );
    }

    // The same index refuses an update that would take a name another plan holds, and
    // the refusal names the plan that holds it.
    let (status, answered) = em
        .update_plan(&second, &json!({ "Name": "MovedPlan" }))
        .await;
    assert_eq!(
        status, 409,
        "case \"update onto a name another plan holds\": expected 409, got {status} with \
         {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ConflictException",
        "case \"update onto a name another plan holds\": expected ConflictException, got \
         {answered}"
    );
    assert_eq!(
        text(&answered, "Reason").as_deref(),
        Some("RESOURCE_NAME_CONFLICT"),
        "case \"update onto a name another plan holds\": expected the RESOURCE_NAME_CONFLICT \
         reason, got {:?}",
        text(&answered, "Reason")
    );
    assert_eq!(
        text(&answered, "ResourceId").as_deref(),
        Some(first.as_str()),
        "case \"update onto a name another plan holds\": expected the conflict to name the \
         holder {first:?}, got {:?}",
        text(&answered, "ResourceId")
    );

    // The refused update changed nothing: the second plan still answers to its own name.
    let stored = em.read_back(&second).await;
    assert_eq!(
        text(&stored, "Name").as_deref(),
        Some("SecondPlan"),
        "case \"a refused rename writes nothing\": expected the plan to keep its Name, got {:?}",
        text(&stored, "Name")
    );
}

#[tokio::test]
async fn a_plan_this_account_cannot_update_is_refused_and_nothing_is_written() {
    /// One refusal row.
    struct Row {
        case: &'static str,
        /// How the request spells the plan it addresses.
        arn: String,
        /// The members it names beside `Arn`.
        patch: Value,
        status: u16,
        code: &'static str,
        /// The `Reason` the modelled shape carries, when it carries one.
        reason: Option<&'static str>,
    }

    let mut rows = vec![
        Row {
            case: "a full plan ARN nothing created",
            arn: plan_arn("NOPLAN0001"),
            patch: json!({ "Description": "no such plan" }),
            status: 404,
            code: "ResourceNotFoundException",
            reason: None,
        },
        Row {
            case: "a bare plan id nothing created",
            arn: "NOPLAN0002".to_string(),
            patch: json!({ "Name": "NoSuchPlan" }),
            status: 404,
            code: "ResourceNotFoundException",
            reason: None,
        },
        Row {
            case: "a plan ARN in another account",
            arn: "arn:aws:billingconductor::000000000000:pricingplan/NOPLAN0003".to_string(),
            patch: json!({ "Description": "another account's plan" }),
            status: 404,
            code: "ResourceNotFoundException",
            reason: None,
        },
    ];
    // Every plan Amazon Web Services owns, in both spellings the pattern admits: not a
    // plan this account may modify, in this operation's modelled 400.
    for id in AWS_MANAGED_PLAN_IDS {
        rows.push(Row {
            case: "an AWS-managed plan, by its bare id",
            arn: (*id).to_string(),
            patch: json!({ "Description": "an AWS-managed plan" }),
            status: 400,
            code: "ValidationException",
            reason: Some("MISMATCHED_PRICINGPLAN_ARN"),
        });
        rows.push(Row {
            case: "an AWS-managed plan, by its full ARN",
            arn: managed_plan_arn(id),
            patch: json!({ "Name": "RenamedManagedPlan" }),
            status: 400,
            code: "ValidationException",
            reason: Some("MISMATCHED_PRICINGPLAN_ARN"),
        });
    }

    let em = Emulator::new();
    // One plan of this account's own, so each row can also show the refusal wrote
    // nothing anywhere.
    let own = em.create_plan(&base_plan("UntouchedPlan")).await;

    for row in &rows {
        let (status, answered) = em.update_plan(&row.arn, &row.patch).await;
        assert_eq!(
            status, row.status,
            "case {:?} ({}): expected HTTP {}, got {status} with {answered}",
            row.case, row.arn, row.status
        );
        assert_eq!(
            error_code(&answered),
            row.code,
            "case {:?} ({}): expected {}, got {answered}",
            row.case,
            row.arn,
            row.code
        );
        assert!(
            text(&answered, "Message").is_some_and(|message| !message.is_empty()),
            "case {:?} ({}): expected the modelled required Message, got {answered}",
            row.case,
            row.arn
        );
        assert_eq!(
            text(&answered, "Reason").as_deref(),
            row.reason,
            "case {:?} ({}): expected Reason {:?}, got {:?}",
            row.case,
            row.arn,
            row.reason,
            text(&answered, "Reason")
        );

        // Nothing was created for the plan the refused request named, and the plan this
        // account does hold is exactly as it was.
        let stored = em.read_back(&own).await;
        assert_eq!(
            text(&stored, "Name").as_deref(),
            Some("UntouchedPlan"),
            "case {:?}: expected the refusal to leave this account's plan alone, got {:?}",
            row.case,
            text(&stored, "Name")
        );
        assert_eq!(
            text(&stored, "Description").as_deref(),
            Some("the plan as created"),
            "case {:?}: expected the refusal to leave this account's plan alone, got {:?}",
            row.case,
            text(&stored, "Description")
        );
        // An AWS-managed plan still reports no association of this account's — the
        // refused update minted no record for it.
        if AWS_MANAGED_PLAN_IDS.contains(&bare_id(&row.arn).as_str()) {
            assert!(
                em.associated_rules(&row.arn).await.is_empty(),
                "case {:?}: expected the AWS-managed plan to hold no association of this \
                 account's",
                row.case
            );
        }
    }
}
