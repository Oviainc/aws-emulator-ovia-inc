// SPDX-License-Identifier: CC0-1.0

//! That the refusal for a preference the requested resource type does not support is worded
//! the SAME whatever else the request carries.
//!
//! `tests/all/preference_resource_type_support.rs` asserts WHICH resource types each
//! preference may be set for and the sentence each refusal carries, over the cross product of
//! preference and `ResourceType`. This suite takes the other dimension of the same class: one
//! preference on one resource type it does not support, with the rest of the request varied —
//! the scope level, the preference value, and a second preference set alongside — and asserts
//! the one sentence every time, with no part of it taken from the request.
//!
//! The expected sentence is not restated here: it is read from the one table that states it
//! ([`crate::preference_resource_type_support::members`]), so these rows cannot drift from the
//! suite that owns it.
//!
//! Every call goes through the shared wire harness (`tests/all/harness.rs`), i.e. the same
//! `MultiServiceRouter::dispatch` the emulator's front door uses, so what is asserted is what
//! a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    ORGANIZATION_SCOPE_VALUE, SCOPE_ACCOUNT_ID, SCOPE_ORGANIZATION, SCOPE_RESOURCE_ARN,
};

use crate::harness::{call, emulator, error_code, instance_arn, opt_in, ACCOUNT};
use crate::preference_resource_type_support::members;

/// The generic wording this service must never answer: a refusal built from a member name and
/// the value the request carried. Asserted absent on every row below, so a refusal that falls
/// back to a parameter template fails whatever its remaining text says.
const GENERIC_PARAMETER_PREFIX: &str = "The value supplied for parameter";

/// The preference these rows set, and a `ResourceType` it is not settable for: the preference
/// whose resource types this service restricts without the member's own documentation naming
/// any, so it is the row most likely to be worded per request.
const MEMBER: &str = "enhancedInfrastructureMetrics";

/// A `ResourceType` the model's enum admits that `MEMBER` does not apply to — no
/// effective-preferences shape of that resource type carries the preference.
const UNSUPPORTED_RESOURCE_TYPE: &str = "EcsService";

/// One refused preference, with the scope level, the preference value and the other members
/// of the request varied: every row is refused `400 InvalidParameterValueException` with the
/// SAME sentence, which carries nothing the request supplied.
#[tokio::test]
async fn the_refusal_is_worded_the_same_whatever_else_the_request_carries() {
    let router = emulator();
    opt_in(&router).await;

    let (_, _, supported, expected) = members()
        .into_iter()
        .find(|(member, ..)| *member == MEMBER)
        .unwrap_or_else(|| panic!("{MEMBER} is a preference whose resource types are restricted"));
    assert!(
        !supported.contains(&UNSUPPORTED_RESOURCE_TYPE),
        "case \"the table\": {UNSUPPORTED_RESOURCE_TYPE} must be a resource type {MEMBER} is \
         not settable for, got {supported:?}"
    );

    // Every scope level the model's `ScopeName` enum admits, each with the value that scope
    // requires, plus a request naming no scope at all.
    let scopes: Vec<(&str, Option<Value>)> = vec![
        ("no scope", None),
        (
            SCOPE_ACCOUNT_ID,
            Some(json!({"name": SCOPE_ACCOUNT_ID, "value": ACCOUNT})),
        ),
        (
            SCOPE_ORGANIZATION,
            Some(json!({"name": SCOPE_ORGANIZATION, "value": ORGANIZATION_SCOPE_VALUE})),
        ),
        (
            SCOPE_RESOURCE_ARN,
            Some(json!({
                "name": SCOPE_RESOURCE_ARN,
                "value": instance_arn("i-0123456789abcdef0"),
            })),
        ),
    ];
    // Both values of the preference's modelled enum (Active | Inactive), and whether a
    // preference the resource type DOES support rides along in the same request.
    let values = ["Active", "Inactive"];
    let companions: [(&str, Option<Value>); 2] = [
        ("alone", None),
        ("with inferredWorkloadTypes", Some(json!("Active"))),
    ];

    for (scope_case, scope) in &scopes {
        for value in values {
            for (companion_case, companion) in &companions {
                let case = format!("{MEMBER}={value} at {scope_case}, {companion_case}");
                let mut request =
                    json!({"resourceType": UNSUPPORTED_RESOURCE_TYPE, MEMBER: value});
                if let Some(scope) = scope {
                    request["scope"] = scope.clone();
                }
                if let Some(companion) = companion {
                    request["inferredWorkloadTypes"] = companion.clone();
                }

                let (status, body) =
                    call(&router, "PutRecommendationPreferences", request.clone()).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected {request} to be refused with 400, got {status}: \
                     {body}"
                );
                assert!(
                    error_code(&body).contains("InvalidParameterValueException"),
                    "case {case:?}: expected InvalidParameterValueException, got {body}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected the service's sentence {expected:?}, got \
                     {message:?}"
                );
                assert!(
                    !message.contains(GENERIC_PARAMETER_PREFIX),
                    "case {case:?}: the refusal must not be built from a parameter template, \
                     got {message:?}"
                );
                // Nothing the request supplied is in the sentence: not the resource type, not
                // the member the preference arrived on, not its value, not the scope.
                for echoed in [UNSUPPORTED_RESOURCE_TYPE, MEMBER, value, scope_case] {
                    assert!(
                        !message.contains(echoed),
                        "case {case:?}: the refusal must not echo the request's {echoed:?}, \
                         got {message:?}"
                    );
                }

                // The whole request is refused, so the preference the resource type DOES
                // support is not stored either.
                let (read_status, read) = call(
                    &router,
                    "GetRecommendationPreferences",
                    json!({"resourceType": UNSUPPORTED_RESOURCE_TYPE}),
                )
                .await;
                assert_eq!(
                    read_status, 200,
                    "case {case:?}: reading the preferences back: got {read_status}: {read}"
                );
                assert_eq!(
                    read["recommendationPreferencesDetails"],
                    json!([]),
                    "case {case:?}: a refused request must store nothing, got {read}"
                );
            }
        }
    }
}
