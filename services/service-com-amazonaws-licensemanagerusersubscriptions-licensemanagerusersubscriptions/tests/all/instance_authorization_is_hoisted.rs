// SPDX-License-Identifier: CC0-1.0

//! The instance-authorization refusal, asserted over EVERY operation that acts on an EC2
//! instance the request names — driven over the real wire path, so each call is
//! deserialized, constraint-checked, handled and serialized exactly as a client's would be.
//!
//! The rule under test: an operation that requires an `InstanceId` and gates on the
//! account's ownership of it decides that BEFORE the request's shape is examined, so a call
//! naming an instance this service will not authorize is refused with the same code, status
//! and sentence whatever else the request gets wrong — including a member the model declares
//! `@required`, a member of the wrong JSON type and a map past its modelled `@length`. The
//! refusal therefore does not depend on which of those operations the caller reached for.
//!
//! The other half of the rule is asserted too: the gate changed WHICH refusal comes first,
//! not whether the modelled constraints are enforced, so the same malformed request on an
//! instance the account's fleet DOES hold is still refused by the boundary validator, and a
//! well-formed one is answered from state.
//!
//! The harness (router + engine + the account's directory fleet the registrations resolve
//! against) is the one in [`super::identity_provider_subscriptions`]; only the rule is new
//! here.

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::seed_account_instances;

// ONE wire harness per service: these come from the sibling suite rather than a second copy
// of the same dispatch plumbing.
use crate::identity_provider_subscriptions::{
    call, harness, provider_union, register, Reply, PRODUCTS,
};

/// A directory of the account's fleet (`Active`, so a registration resolves for it), which
/// the well-formed rows name.
const DIRECTORY: &str = "d-0123456789";

/// The sentence this service fails closed with
/// (`scaffolded::UNAUTHORIZED_EXCEPTION_MESSAGE`), and the status it reports such a refusal
/// with (`scaffolded::SERVICE_REFUSAL_STATUS`). Spelled out here rather than read from the
/// production constants, so a change to them fails a test instead of silently changing what
/// this suite accepts.
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";
const REFUSAL_STATUS: u16 = 400;

/// One EC2 instance of the account's seeded fleet (`state::seed_account_instances`): the
/// only instances a request that acts on an instance is authorized for.
fn held_instance(index: usize) -> String {
    seed_account_instances()[index].0.clone()
}

/// Instance ids the account's fleet does NOT hold. `InstanceId` is a plain `String` in this
/// service's model, so the rows vary in FORM and SIZE rather than against a pattern: EC2's
/// two published id lengths, a near miss on a held instance, its uppercase spelling, and a
/// value that is not an instance id at all.
fn unheld_instances() -> Vec<(&'static str, String)> {
    let held = held_instance(0);
    vec![
        ("the 17-hex-digit id form", "i-0123456789abcdef0".to_string()),
        ("the 8-hex-digit id form", "i-0abc1234".to_string()),
        (
            "a near miss on a held instance",
            format!("{}0", &held[..held.len() - 1]),
        ),
        (
            "an uppercase spelling of a held instance",
            held.to_ascii_uppercase(),
        ),
        (
            "a value that is not an instance id",
            "not-an-instance".to_string(),
        ),
    ]
}

/// One operation that acts on the EC2 instance its request names: it declares `InstanceId`
/// `@required` and its handler gates on the account's ownership of that instance before it
/// looks anything else up.
///
/// The two of them (`smithy_query(query="operation",
/// operation="AssociateUser,ListUserAssociations")`, whose `http.uri` each `path` is).
#[derive(Debug, Clone, Copy)]
struct GatedOperation {
    case: &'static str,
    path: &'static str,
    /// A request of this operation that is well formed apart from the instance it names.
    well_formed: fn(&str) -> Value,
    /// A defect only this operation's input can carry, so the table also covers a modelled
    /// constraint the other one does not declare.
    own_defect: Defect,
}

/// One thing a request gets wrong beside the instance it names, as the edit that makes a
/// well-formed body carry it.
#[derive(Debug, Clone, Copy)]
struct Defect {
    case: &'static str,
    apply: fn(&mut Value),
}

/// A `Tags` map of `count` entries, for the row that goes past the modelled
/// `@length(min: 0, max: 50)` of `AssociateUser`'s `Tags`.
fn tags(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("k{index}"), Value::String("x".to_string()));
    }
    Value::Object(map)
}

const GATED_OPERATIONS: &[GatedOperation] = &[
    GatedOperation {
        case: "AssociateUser",
        path: "/user/AssociateUser",
        well_formed: |instance_id| {
            json!({
                "Username": "someone",
                "InstanceId": instance_id,
                "IdentityProvider": provider_union(DIRECTORY),
            })
        },
        // `Tags` is `@length(min: 0, max: 50)`; one entry past the maximum is a violation
        // the boundary validator refuses.
        own_defect: Defect {
            case: "a Tags map one entry past the modelled maximum",
            apply: |body| body["Tags"] = tags(51),
        },
    },
    GatedOperation {
        case: "ListUserAssociations",
        path: "/user/ListUserAssociations",
        well_formed: |instance_id| {
            json!({
                "InstanceId": instance_id,
                "IdentityProvider": provider_union(DIRECTORY),
            })
        },
        // `MaxResults` targets `BoxInteger`, so a string there is not a value the request
        // can even be deserialized into.
        own_defect: Defect {
            case: "a MaxResults that is not a number",
            apply: |body| body["MaxResults"] = json!("many"),
        },
    },
];

/// What else the request gets wrong, each one something the framework refuses before any
/// handler runs, and each applying to EVERY gated operation: both declare `InstanceId` and
/// `IdentityProvider` `@required`. `InstanceId` itself is left alone in every row — it is
/// the member the gate reads.
const ALSO_WRONG: &[Defect] = &[
    Defect {
        case: "nothing else",
        apply: |_| {},
    },
    MISSING_REQUIRED_MEMBER,
    Defect {
        case: "an IdentityProvider that is not an object",
        apply: |body| body["IdentityProvider"] = json!(DIRECTORY),
    },
    Defect {
        case: "every member but InstanceId omitted",
        apply: |body| {
            let instance_id = body["InstanceId"].clone();
            *body = json!({ "InstanceId": instance_id });
        },
    },
];

/// Omitting a member the model declares `@required` on both gated operations: the defect
/// both halves of the rule are asserted on, so the unauthorized and the authorized answer
/// to ONE request are compared.
const MISSING_REQUIRED_MEMBER: Defect = Defect {
    case: "the required IdentityProvider omitted",
    apply: |body| {
        if let Some(members) = body.as_object_mut() {
            members.remove("IdentityProvider");
        }
    },
};

/// Assert one refusal: the status, the wire error code and the message, naming the case and
/// showing what was actually answered.
fn refused(reply: &Reply, case: &str, status: u16, code: &str, message: &str) {
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected {code}, got {reply:?}"
    );
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(message),
        "case {case:?}: wrong refusal message in {reply:?}"
    );
}

/// RULE: every operation that requires an `InstanceId` and gates on the account's ownership
/// of it is authorized BEFORE its request shape is validated, so a call naming an instance
/// the account's EC2 fleet does not hold is refused with `UnauthorizedException` and the
/// fixed sentence whatever else the request gets wrong — and identically for each of those
/// operations.
#[tokio::test]
async fn an_unauthorized_instance_is_refused_identically_on_every_gated_operation() {
    let h = harness();

    for operation in GATED_OPERATIONS {
        // The shared defects, plus the one only this operation's input can carry.
        let mut defects: Vec<Defect> = ALSO_WRONG.to_vec();
        defects.push(operation.own_defect);

        for (instance_case, instance_id) in unheld_instances() {
            for defect in &defects {
                let mut body = (operation.well_formed)(&instance_id);
                (defect.apply)(&mut body);
                let case = format!(
                    "{}: {instance_case}, {}",
                    operation.case, defect.case
                );
                let reply = call(&h, operation.path, &body).await;
                refused(
                    &reply,
                    &case,
                    REFUSAL_STATUS,
                    "UnauthorizedException",
                    UNAUTHORIZED_MESSAGE,
                );
            }
        }
    }
}

/// RULE, the other half: hoisting the gate changed WHICH refusal comes first, not whether
/// the modelled constraints are enforced — the same request on an instance the account's
/// fleet DOES hold is still refused by the boundary validator, and a well-formed one is
/// answered from state.
#[tokio::test]
async fn an_authorized_request_is_still_constraint_checked() {
    let h = harness();
    // The registration the user-association plane resolves a request through, so a
    // well-formed call is answered rather than refused for an unregistered provider.
    register(
        &h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": PRODUCTS[0],
            "Settings": {
                "Subnets": ["subnet-0123456789abcdef0"],
                "SecurityGroupId": "sg-0123abcd",
            },
        }),
    )
    .await
    .ok("the registration the associations resolve through");

    for (index, operation) in GATED_OPERATIONS.iter().enumerate() {
        // A different instance of the fleet per operation, so the authorized half of the
        // rule is exercised on more than one held id.
        let instance_id = held_instance(index % seed_account_instances().len());

        // Omitting a member the model declares `@required` is a violation the boundary
        // validator names, which is what an authorized request gets.
        let mut body = (operation.well_formed)(&instance_id);
        (MISSING_REQUIRED_MEMBER.apply)(&mut body);
        let case = format!(
            "{}: {} on a held instance",
            operation.case, MISSING_REQUIRED_MEMBER.case
        );
        let reply = call(&h, operation.path, &body).await;
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: an authorized request missing a required member is a modelled \
             validation failure, got {reply:?}"
        );
        assert_eq!(
            reply.status, 400,
            "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
        );

        // And the well-formed request on the same instance is served.
        let body = (operation.well_formed)(&instance_id);
        let reply = call(&h, operation.path, &body).await;
        reply.ok(&format!(
            "{}: a well-formed, authorized request",
            operation.case
        ));
    }
}
