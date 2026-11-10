// SPDX-License-Identifier: CC0-1.0

//! The account-level Premium Support entitlement `CreateEnvironment` requires, and where
//! its clause sits among the request's other validation clauses, over awsJson1_0.
//!
//! RULE: an Amazon EVS environment runs licensed VCF software under an Amazon Web Services
//! Premium Support subscription, so a `CreateEnvironment` request from an account that
//! holds none is refused — however well formed the rest of it is — with HTTP 400
//! `ValidationException` whose FIRST collected clause is the fixed entitlement clause
//! [`ENTITLEMENT_CLAUSE`], ahead of every member-level clause the same request collects;
//! the clauses are reported both as one `fieldList` entry each, in collection order, and
//! joined into `message` after `Request validation failure: ` by ` | `. Nothing is created
//! by a refused request. An account whose plan DOES qualify is judged on its request
//! alone, and the entitlement clause appears nowhere in that judgement.
//!
//! The subscription is not this service's own fact: it is the account's Support plan,
//! which the Amazon Web Services Support emulator owns (its `support_plan`, granted out of
//! band with `SUPPORT_PLAN`) and which `crate::state::premium_support` reads from that
//! store. So the rows below vary the plan in that store — none at all, plans that do not
//! qualify, and several spellings of ones that do — and never a member of the EVS request.
//!
//! The rows span what the rule generalises over: which plan the account holds, how many
//! other clauses the request collects (none, one, two), which checks those are, and the
//! operation (the gate is `CreateEnvironment`'s; every other operation is served to an
//! unsubscribed account).

use serde_json::{json, Value};

use ::service_com_amazonaws_support_awssupport_20130415::state as support;

use crate::environment_lifecycle::{call, emulator, error_type, Emulator};

/// The clause Amazon EVS states for an account that holds no Premium Support
/// subscription — fixed text in full, naming no member and interpolating nothing.
const ENTITLEMENT_CLAUSE: &str = "You must be subscribed to AWS Premium Support";

/// The clause it states for a request that has not accepted the terms — fixed text in
/// full as well, which `tests/all/create_environment_validation.rs` also reads.
const TERMS_CLAUSE: &str = "Terms and Conditions must be accepted before creating an environment";

/// What the top-level `message` of an aggregated refusal opens with, and what joins the
/// clauses inside it.
const FAILURE_PREFIX: &str = "Request validation failure: ";
/// See [`FAILURE_PREFIX`].
const CLAUSE_JOINER: &str = " | ";

/// A Support plan that counts as a Premium Support subscription, in the spelling the
/// Support emulator's own `QUALIFYING_SUPPORT_PLANS` carries.
pub(crate) const QUALIFYING_SUPPORT_PLAN: &str = "enterprise";

/// Record `plan` as the Support plan the emulated account holds — `None` for an account
/// that holds none, which is what a new Amazon Web Services account has and what the
/// Support store starts with.
///
/// Written into the store that OWNS the fact rather than set through an environment
/// variable, so one test's account class cannot leak into another's.
pub(crate) fn grant_support_plan(engine: &engine::InMemoryEngine, plan: Option<&str>) {
    support::service_state(engine)
        .expect("the in-memory engine carries the Support store")
        .lock()
        .support_plan = plan.map(str::to_string);
}

/// Grant the emulated account a qualifying Premium Support plan: what every suite whose
/// `CreateEnvironment` is expected to be SERVED needs, because an account holding none
/// creates no environment.
pub(crate) fn grant_premium_support(engine: &engine::InMemoryEngine) {
    grant_support_plan(engine, Some(QUALIFYING_SUPPORT_PLAN));
}

/// An emulator whose account holds no Premium Support subscription — the account class
/// the entitlement clause is stated for.
fn unsubscribed_emulator() -> Emulator {
    let emulator = emulator();
    grant_support_plan(&emulator.engine, None);
    emulator
}

/// The `fieldList` messages of a refusal, in the order it reported them.
fn reported_clauses(body: &Value) -> Vec<String> {
    body["fieldList"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["message"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that `body` is the aggregated refusal reporting exactly `clauses`, in order:
/// one `fieldList` entry each, and a `message` joining them.
fn assert_refused_with(case: &str, status: u16, body: &Value, clauses: &[String]) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_type(body),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {body}"
    );
    let reported = reported_clauses(body);
    assert_eq!(
        reported, clauses,
        "case {case:?}: expected the clauses {clauses:?}, got {reported:?}"
    );
    let expected = format!("{FAILURE_PREFIX}{}", clauses.join(CLAUSE_JOINER));
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

/// How many environments the account holds.
async fn environment_count(emulator: &Emulator) -> usize {
    let (status, body) = call(emulator, "ListEnvironments", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListEnvironments: expected HTTP 200, got {status} {body}"
    );
    body["environmentSummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn an_unsubscribed_account_is_refused_with_the_entitlement_clause_first() {
    let emulator = unsubscribed_emulator();

    /// The clause a request naming a VPC the account does not hold collects, and the
    /// one a request naming a subnet that VPC does not hold collects: both name the id
    /// the request gave, which is why each row builds its clauses from that request.
    fn vpc_clause(vpc_id: &str) -> String {
        format!(
            "The VPC id {vpc_id} is not valid or does not exist. Please verify the VPC id \
             exists and is valid."
        )
    }

    /// See [`vpc_clause`].
    fn subnet_clause(subnet_id: &str, vpc_id: &str) -> String {
        format!(
            "The service access subnet id {subnet_id} is not valid or does not exist in VPC \
             {vpc_id}. Please verify the subnet id exists in the VPC and is valid."
        )
    }

    /// The id no VPC and no subnet of the account carries, which the rows below name.
    const ABSENT_VPC: &str = "vpc-01234567";
    /// See [`ABSENT_VPC`].
    const ABSENT_SUBNET: &str = "subnet-89abcdef";

    // One row per number and kind of OTHER clause the same request collects: none, the
    // terms clause, each of the two network clauses, and two at once. The entitlement
    // clause leads every one of them.
    type Row = (
        &'static str,
        fn(&mut Value),
        fn(&Value, &str) -> Vec<String>,
    );
    let rows: [Row; 5] = [
        ("a request every other rule accepts", |_| {}, |_, _| Vec::new()),
        (
            "a request that has not accepted the terms",
            |request| request["termsAccepted"] = json!(false),
            |_, _| vec![TERMS_CLAUSE.to_string()],
        ),
        (
            "a request naming a VPC the account does not hold",
            |request| request["vpcId"] = json!(ABSENT_VPC),
            |_, _| vec![vpc_clause(ABSENT_VPC)],
        ),
        (
            "a request naming a service access subnet the VPC does not hold",
            |request| request["serviceAccessSubnetId"] = json!(ABSENT_SUBNET),
            |_, vpc_id| vec![subnet_clause(ABSENT_SUBNET, vpc_id)],
        ),
        (
            "a request that breaks two other rules at once",
            |request| {
                request["termsAccepted"] = json!(false);
                request["vpcId"] = json!(ABSENT_VPC);
            },
            |_, _| vec![TERMS_CLAUSE.to_string(), vpc_clause(ABSENT_VPC)],
        ),
    ];

    for (index, (case, break_request, other_clauses)) in rows.into_iter().enumerate() {
        let mut request = emulator.create_request(Some(&format!("entitlement-{index}")));
        // The VPC the request is deployed into is minted per row, so the subnet clause's
        // VPC id is read off the request rather than written above.
        let vpc_id = request["vpcId"]
            .as_str()
            .expect("the create request names a VPC")
            .to_string();
        break_request(&mut request);
        let mut expected = vec![ENTITLEMENT_CLAUSE.to_string()];
        expected.extend(other_clauses(&request, &vpc_id));

        let (status, body) = call(&emulator, "CreateEnvironment", &request).await;
        assert_refused_with(case, status, &body, &expected);
        assert_eq!(
            environment_count(&emulator).await,
            0,
            "case {case:?}: a refused request must create no environment"
        );
    }
}

#[tokio::test]
async fn a_plan_decides_whether_the_entitlement_clause_is_collected_at_all() {
    // One row per plan the account may hold: none, plans that do not qualify as a Premium
    // Support subscription, and several spellings of ones that do — the Support emulator's
    // own qualification rule, which this service reads rather than restates.
    for (index, plan) in [
        None,
        Some("basic"),
        Some("developer"),
        Some("enterprise"),
        Some("Business"),
        Some("AWS Enterprise On-Ramp"),
        Some("Business Support+"),
    ]
    .into_iter()
    .enumerate()
    {
        let emulator = emulator();
        grant_support_plan(&emulator.engine, plan);
        let subscribed = plan.is_some_and(support::plan_qualifies);
        let case = format!("the plan {plan:?}");

        let request = emulator.create_request(Some(&format!("plan-{index}")));
        let (status, body) = call(&emulator, "CreateEnvironment", &request).await;
        if subscribed {
            assert_eq!(
                status, 200,
                "case {case:?}: a subscribed account creates an environment, got {status} \
                 ({}) {body}",
                error_type(&body)
            );
            assert!(
                body["environment"]["environmentId"].is_string(),
                "case {case:?}: expected a minted environmentId, got {body}"
            );
        } else {
            assert_refused_with(&case, status, &body, &[ENTITLEMENT_CLAUSE.to_string()]);
        }

        // The other side of the rule: a subscribed account's request is judged on its own
        // clauses, with no entitlement clause among them.
        let mut refused = emulator.create_request(Some(&format!("plan-{index}-terms")));
        refused["termsAccepted"] = json!(false);
        let (status, body) = call(&emulator, "CreateEnvironment", &refused).await;
        let expected = if subscribed {
            vec![TERMS_CLAUSE.to_string()]
        } else {
            vec![ENTITLEMENT_CLAUSE.to_string(), TERMS_CLAUSE.to_string()]
        };
        let case = format!("{case} with unaccepted terms");
        assert_refused_with(&case, status, &body, &expected);
    }
}

#[tokio::test]
async fn every_other_operation_is_served_to_an_unsubscribed_account() {
    let emulator = unsubscribed_emulator();

    // The entitlement is `CreateEnvironment`'s precondition, not the service's: the reads
    // an unsubscribed account makes are answered, and no entitlement clause reaches them.
    for (case, operation, body) in [
        ("the version catalogue", "GetVersions", json!({})),
        ("the environment list", "ListEnvironments", json!({})),
        ("the account settings", "GetAccountSettings", json!({})),
    ] {
        let (status, answer) = call(&emulator, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} ({}) {answer}",
            error_type(&answer)
        );
        assert!(
            !answer.to_string().contains(ENTITLEMENT_CLAUSE),
            "case {case:?}: no entitlement clause may reach this answer, got {answer}"
        );
    }
}
