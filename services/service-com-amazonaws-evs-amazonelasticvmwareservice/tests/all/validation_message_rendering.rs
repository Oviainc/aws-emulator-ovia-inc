// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request its modelled constraints refuse, over awsJson1_0.
//!
//! The status and the error code of these refusals were already right; what is under
//! test is the only part of them that says WHICH member failed and WHY — the message.
//! One rule, stated for every operation of the service by the renderer in
//! `src/state.rs` and reached through `Service::override_refusal`:
//!
//! * the envelope is `1 validation error detected: <clause>` for a single violation and
//!   `<n> validation error(s) detected: <clause>,<clause>` for several, the clauses
//!   ordered by the member path they are reported against and joined by a bare comma;
//! * a clause is `Value at '<member path>' failed to satisfy constraint: <requirement>`
//!   and never quotes the rejected value; only an ABSENT member keeps `Value null at`,
//!   because absence is what that clause states;
//! * a `@pattern` is printed as the expression a value has to satisfy, without the `^`
//!   and `$` anchors the model wrote to require a whole-string match;
//! * a violation of a collection ELEMENT is reported against the collection member that
//!   holds it, with the element's requirement bundled as
//!   `Member must satisfy constraint: [<requirement>]`;
//! * a value outside an enumerated set on a member that is NOT a collection element is
//!   stated by naming that member instead — `Member <member> must satisfy enum value set:
//!   [<values>]`, under the `<n> validation error(s) detected: ` envelope — and the value
//!   set is printed in the order this service states it for that type, which is its own
//!   fixed text rather than the model's declaration order.
//!
//! The rows span what the rule generalises over: the constraint KIND (pattern, length,
//! range, enum value set, absence), the member and the operation carrying it, the NESTING
//! (a top-level member, a member of a list element, a list element itself), the number of
//! simultaneous violations, and the rejected value — including values that SPELL the
//! clause marker, a whole clause, or regex anchors, which is the input a renderer that
//! parsed the framework's value-carrying clause would copy back into the message. Every `requirement` below is the
//! member's own modelled constraint as `smithy_query` reports it for this service's model:
//!
//!   AccountSetting.name / AccountSetting.value -> @length(1..128) @pattern ^[a-zA-Z0-9]+$
//!   CreateEnvironmentRequest.vpcId -> @length(12..21) @pattern ^vpc-[a-f0-9]{8}([a-f0-9]{9})?$
//!   CreateEnvironmentRequest.serviceAccessSubnetId ->
//!       @length(15..24) @pattern ^subnet-[a-f0-9]{8}([a-f0-9]{9})?$
//!   CreateEnvironmentRequest.environmentName -> @length(1..100) @pattern ^[a-zA-Z0-9_-]+$
//!   CreateEnvironmentRequest.vcfVersion -> enum VcfVersion [VCF-5.2.1, VCF-5.2.2, SELF_DEPLOYED]
//!   ListEnvironmentsRequest.maxResults -> MaxResults @range(min: 1, max: 100)
//!   ListEnvironmentsRequest.state -> EnvironmentStateList of enum EnvironmentState
//!       [CREATING, CREATED, DELETING, DELETED, CREATE_FAILED]

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/evs/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=0";

/// The five `EnvironmentState` values the model declares, in the order this service
/// STATES them inside a rendered enum value set — which is its own fixed text for that
/// type and not the order the Smithy model declares them in (`smithy_query(query="shape",
/// shape="EnvironmentState")` declares CREATING, CREATED, DELETING, DELETED,
/// CREATE_FAILED).
const ENVIRONMENT_STATES: [&str; 5] = [
    "CREATE_FAILED",
    "DELETING",
    "CREATED",
    "DELETED",
    "CREATING",
];

/// The three `VcfVersion` values the model declares
/// (`smithy_query(query="shape", shape="VcfVersion")`).
const VCF_VERSIONS: [&str; 3] = ["VCF-5.2.1", "VCF-5.2.2", "SELF_DEPLOYED"];

/// The VPC and the subnet inside it that [`create_request`] names. `CreateEnvironment`
/// checks the network a request names against the account's own Amazon EC2 store, so
/// [`emulator`] registers these there — what these tests exercise is the wording of a
/// MODELLED-constraint refusal, not that precondition.
const VPC_ID: &str = "vpc-0123456789abcdef0";
/// See [`VPC_ID`].
const SUBNET_ID: &str = "subnet-0123456789abcdef0";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise wording, not signing: the request below is unsigned.
    router.set_request_authentication(false);
    crate::environment_lifecycle::register_network(&engine, VPC_ID, SUBNET_ID);
    // An environment is created only for an account holding a Premium Support
    // subscription, which these rows need for the requests their constraints accept
    // (`tests/all/premium_support_entitlement.rs` owns that rule).
    crate::premium_support_entitlement::grant_premium_support(&engine);
    router.register_service(
        service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(engine),
    );
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (the awsJson body's `__type`), or `""` on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `message` the error body carries, or `""` when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// Call one operation the way a client does: `POST /` with the `X-Amz-Target` naming it.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("https://evs.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AmazonElasticVMwareService.{operation}"),
        )
        .header("authorization", SIGNED)
        .body(::bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    Answer {
        status,
        body: payload,
    }
}

/// Ten non-overlapping VLAN subnet CIDR blocks, one per `InitialVlans` member.
fn vlans() -> Value {
    let cidr = |index: u8| json!({ "cidr": format!("10.0.{}.0/28", index * 2) });
    json!({
        "vmkManagement": cidr(0),
        "vmManagement": cidr(1),
        "vMotion": cidr(2),
        "vSan": cidr(3),
        "vTep": cidr(4),
        "edgeVTep": cidr(5),
        "nsxUplink": cidr(6),
        "hcx": cidr(7),
        "expansionVlan1": cidr(8),
        "expansionVlan2": cidr(9),
    })
}

/// A `CreateEnvironment` request every modelled constraint accepts, which each row below
/// then breaks in exactly the members it is a case of.
fn create_request() -> Value {
    json!({
        "vpcId": VPC_ID,
        "serviceAccessSubnetId": SUBNET_ID,
        "vcfVersion": "VCF-5.2.1",
        "termsAccepted": true,
        "initialVlans": vlans(),
    })
}

/// `create_request()` with `overrides` merged in; a `Value::Null` override REMOVES the
/// member, which is how the absent-member row is spelled.
fn create_request_with(overrides: Value) -> Value {
    let mut body = create_request();
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        if value.is_null() {
            body.as_object_mut()
                .expect("the request is an object")
                .remove(member);
        } else {
            body[member] = value.clone();
        }
    }
    body
}

/// A `PutAccountSettings` request carrying `settings`.
fn put_settings(settings: Value) -> Value {
    json!({ "settings": settings })
}

/// One refused request and the whole message this service states for it.
struct Row {
    /// What this row is a case of.
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The complete `message` of the 400, clause by clause.
    expected: String,
}

/// One request refused for a value outside an enumerated set: where the clause reports
/// it, whether the requirement is bundled inside the holding member's, and the values
/// the rendered set must name — in the order this service states them.
struct EnumRow {
    /// What this row is a case of.
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The member path the clause names.
    member: &'static str,
    /// Whether the offending value is a collection ELEMENT, so the element's
    /// requirement is bundled inside the collection member's — which is also what
    /// decides which of the two clause forms the refusal takes.
    wrapped: bool,
    /// The values the enumerated type declares, in the order the refusal states them.
    values: &'static [&'static str],
}

/// Assert the shared half of the rule: HTTP 400, `ValidationException`, and a message
/// that never quotes the value it refused.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a value outside its modelled constraint must be refused with \
         HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert!(
        !answer.message().contains("Value '"),
        "case {case:?}: a clause must not quote the rejected value, got {:?}",
        answer.message()
    );
    assert!(
        !answer.message().contains("oops") && !answer.message().contains("anchored"),
        "case {case:?}: no part of the rejected value may reach the message, got {:?}",
        answer.message()
    );
    assert!(
        !answer.message().contains('^') && !answer.message().contains('$'),
        "case {case:?}: a printed @pattern carries no anchors, got {:?}",
        answer.message()
    );
}

#[tokio::test]
async fn a_constraint_violation_is_stated_as_this_services_validation_envelope() {
    let router = emulator();

    let rows = vec![
        Row {
            case: "a pattern violation on a member of the first list element",
            operation: "PutAccountSettings",
            body: put_settings(json!([
                { "name": "vcfPortedCoreCount", "value": "not-a-number" },
            ])),
            expected: "1 validation error detected: Value at 'settings.1.member.value' \
                       failed to satisfy constraint: Member must satisfy regular \
                       expression pattern: [a-zA-Z0-9]+"
                .to_string(),
        },
        Row {
            case: "the same violation on the second list element names its index",
            operation: "PutAccountSettings",
            body: put_settings(json!([
                { "name": "vcfPortedCoreCount", "value": "100" },
                { "name": "vcfPortedCoreCount", "value": "not a number" },
            ])),
            expected: "1 validation error detected: Value at 'settings.2.member.value' \
                       failed to satisfy constraint: Member must satisfy regular \
                       expression pattern: [a-zA-Z0-9]+"
                .to_string(),
        },
        Row {
            case: "a pattern violation on the other member of the same element",
            operation: "PutAccountSettings",
            body: put_settings(json!([
                { "name": "vcf.ported.core.count", "value": "100" },
            ])),
            expected: "1 validation error detected: Value at 'settings.1.member.name' \
                       failed to satisfy constraint: Member must satisfy regular \
                       expression pattern: [a-zA-Z0-9]+"
                .to_string(),
        },
        Row {
            case: "a length violation one past the ceiling, inside a list element",
            operation: "PutAccountSettings",
            body: put_settings(json!([
                { "name": "vcfPortedCoreCount", "value": "a".repeat(129) },
            ])),
            expected: "1 validation error detected: Value at 'settings.1.member.value' \
                       failed to satisfy constraint: Member must have length less than \
                       or equal to 128"
                .to_string(),
        },
        Row {
            case: "a pattern violation on a top-level member",
            operation: "CreateEnvironment",
            body: create_request_with(json!({ "environmentName": "bad name!" })),
            expected: "1 validation error detected: Value at 'environmentName' failed to \
                       satisfy constraint: Member must satisfy regular expression \
                       pattern: [a-zA-Z0-9_-]+"
                .to_string(),
        },
        Row {
            case: "a rejected value that spells the clause marker itself",
            operation: "CreateEnvironment",
            body: create_request_with(
                json!({ "environmentName": "x failed to satisfy constraint: oops" }),
            ),
            expected: "1 validation error detected: Value at 'environmentName' failed to \
                       satisfy constraint: Member must satisfy regular expression \
                       pattern: [a-zA-Z0-9_-]+"
                .to_string(),
        },
        Row {
            case: "a rejected value that spells a whole clause, quoting and all",
            operation: "CreateEnvironment",
            body: create_request_with(json!({
                "environmentName":
                    "Value 'y' at 'z' failed to satisfy constraint: Member must not be null",
            })),
            expected: "1 validation error detected: Value at 'environmentName' failed to \
                       satisfy constraint: Member must satisfy regular expression \
                       pattern: [a-zA-Z0-9_-]+"
                .to_string(),
        },
        Row {
            case: "a rejected value carrying regex anchors of its own",
            operation: "CreateEnvironment",
            body: create_request_with(json!({ "environmentName": "^anchored$" })),
            expected: "1 validation error detected: Value at 'environmentName' failed to \
                       satisfy constraint: Member must satisfy regular expression \
                       pattern: [a-zA-Z0-9_-]+"
                .to_string(),
        },
        Row {
            case: "the same smuggling from inside a list element",
            operation: "PutAccountSettings",
            body: put_settings(json!([
                {
                    "name": "vcfPortedCoreCount",
                    "value": "' at 'settings.9.member.value' failed to satisfy constraint: oops",
                },
            ])),
            expected: "1 validation error detected: Value at 'settings.1.member.value' \
                       failed to satisfy constraint: Member must satisfy regular \
                       expression pattern: [a-zA-Z0-9]+"
                .to_string(),
        },
        Row {
            case: "two simultaneous violations, ordered by member path",
            operation: "CreateEnvironment",
            body: create_request_with(json!({
                "vpcId": "vpc-zzzzzzzz",
                "serviceAccessSubnetId": "subnet-zzzzzzzz",
            })),
            expected: "2 validation error(s) detected: Value at 'serviceAccessSubnetId' \
                       failed to satisfy constraint: Member must satisfy regular \
                       expression pattern: subnet-[a-f0-9]{8}([a-f0-9]{9})?,Value at \
                       'vpcId' failed to satisfy constraint: Member must satisfy regular \
                       expression pattern: vpc-[a-f0-9]{8}([a-f0-9]{9})?"
                .to_string(),
        },
        Row {
            case: "three simultaneous violations, still ordered by member path",
            operation: "CreateEnvironment",
            body: create_request_with(json!({
                "vpcId": "vpc-zzzzzzzz",
                "serviceAccessSubnetId": "subnet-zzzzzzzz",
                "environmentName": "bad name!",
            })),
            expected: "3 validation error(s) detected: Value at 'environmentName' failed \
                       to satisfy constraint: Member must satisfy regular expression \
                       pattern: [a-zA-Z0-9_-]+,Value at 'serviceAccessSubnetId' failed \
                       to satisfy constraint: Member must satisfy regular expression \
                       pattern: subnet-[a-f0-9]{8}([a-f0-9]{9})?,Value at 'vpcId' failed \
                       to satisfy constraint: Member must satisfy regular expression \
                       pattern: vpc-[a-f0-9]{8}([a-f0-9]{9})?"
                .to_string(),
        },
        Row {
            case: "a length violation one past the ceiling on a top-level member",
            operation: "CreateEnvironment",
            body: create_request_with(json!({ "environmentName": "n".repeat(101) })),
            expected: "1 validation error detected: Value at 'environmentName' failed to \
                       satisfy constraint: Member must have length less than or equal to \
                       100"
                .to_string(),
        },
        Row {
            case: "a range violation one past the ceiling",
            operation: "ListEnvironments",
            body: json!({ "maxResults": 101 }),
            expected: "1 validation error detected: Value at 'maxResults' failed to \
                       satisfy constraint: Member must have value less than or equal to \
                       100"
                .to_string(),
        },
        Row {
            case: "a range violation far past the ceiling",
            operation: "ListEnvironments",
            body: json!({ "maxResults": 100_000 }),
            expected: "1 validation error detected: Value at 'maxResults' failed to \
                       satisfy constraint: Member must have value less than or equal to \
                       100"
                .to_string(),
        },
        Row {
            case: "a range violation one below the floor",
            operation: "ListEnvironments",
            body: json!({ "maxResults": 0 }),
            expected: "1 validation error detected: Value at 'maxResults' failed to \
                       satisfy constraint: Member must have value greater than or equal \
                       to 1"
                .to_string(),
        },
        Row {
            case: "an absent required member keeps the null subject",
            operation: "CreateEnvironment",
            body: create_request_with(json!({ "vpcId": null })),
            expected: "1 validation error detected: Value null at 'vpcId' failed to \
                       satisfy constraint: Member must not be null"
                .to_string(),
        },
    ];

    for row in rows {
        let case = row.case;
        let answer = call(&router, row.operation, &row.body).await;
        assert_refused(case, &answer);
        assert_eq!(
            answer.message(),
            row.expected,
            "case {case:?}: expected {:?}, got {:?}",
            row.expected,
            answer.message()
        );
    }
}

#[tokio::test]
async fn an_enum_violation_is_reported_against_the_member_that_holds_it() {
    let router = emulator();

    // A value outside an enumerated set is the one requirement whose rendering carries a
    // set whose ORDER is not part of the rule, so each row states the path and the
    // nesting it expects and the values the set must name.
    let rows = [
        EnumRow {
            case: "an element of a list of enums is reported against the list member",
            operation: "ListEnvironments",
            body: json!({ "state": ["NOT_A_REAL_STATE"] }),
            member: "state",
            wrapped: true,
            values: &ENVIRONMENT_STATES,
        },
        EnumRow {
            case: "a lowercase spelling of a real value is still outside the set",
            operation: "ListEnvironments",
            body: json!({ "state": ["creating"] }),
            member: "state",
            wrapped: true,
            values: &ENVIRONMENT_STATES,
        },
        EnumRow {
            case: "a second element of the list is reported against the same member",
            operation: "ListEnvironments",
            body: json!({ "state": ["CREATED", "ALMOST_CREATED"] }),
            member: "state",
            wrapped: true,
            values: &ENVIRONMENT_STATES,
        },
        EnumRow {
            case: "an enum member that is NOT a collection element keeps its own path",
            operation: "CreateEnvironment",
            body: create_request_with(json!({ "vcfVersion": "VCF-9.9.9" })),
            member: "vcfVersion",
            wrapped: false,
            values: &VCF_VERSIONS,
        },
    ];

    for row in rows {
        let EnumRow {
            case,
            operation,
            body,
            member,
            wrapped,
            values,
        } = row;
        let answer = call(&router, operation, &body).await;
        assert_refused(case, &answer);
        // A collection ELEMENT is reported through the member that holds it, with its
        // requirement bundled inside that member's; a member's OWN value outside an
        // enumerated set is stated by naming the member instead, under the envelope that
        // counts its failures as `error(s)`.
        let expected = if wrapped {
            format!(
                "1 validation error detected: Value at '{member}' failed to satisfy \
                 constraint: Member must satisfy constraint: [Member must satisfy enum \
                 value set: [{}]]",
                values.join(", ")
            )
        } else {
            format!(
                "1 validation error(s) detected: Member {member} must satisfy enum value \
                 set: [{}]",
                values.join(", ")
            )
        };
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn a_value_inside_every_constraint_is_not_refused() {
    let router = emulator();

    // The other side of the rule: the envelope is stated for a violation and for nothing
    // else. Each row sits exactly ON a published bound, or is a legal value of an
    // enumerated set, so none of them may be worded as a refusal.
    let rows = [
        ("maxResults at its floor", "ListEnvironments", json!({ "maxResults": 1 })),
        ("maxResults at its ceiling", "ListEnvironments", json!({ "maxResults": 100 })),
        (
            "every declared state value",
            "ListEnvironments",
            json!({ "state": ENVIRONMENT_STATES }),
        ),
        (
            // A setting's value is also checked against the DOMAIN
            // `AccountSetting$name` documents for the named setting (a numeric string,
            // at most 1,000,000 cores for `vcfPortedCoreCount`) — see
            // `tests/all/account_settings.rs`, which owns that rule. So the value that
            // may not be worded as a modelled-constraint refusal is one inside BOTH: the
            // documented maximum, which no 128-character value could be.
            "a setting value at the maximum its documentation gives it",
            "PutAccountSettings",
            put_settings(json!([
                { "name": "vcfPortedCoreCount", "value": "1000000" },
            ])),
        ),
        (
            "an environment name at its length ceiling",
            "CreateEnvironment",
            create_request_with(json!({ "environmentName": "n".repeat(100) })),
        ),
    ];

    for (case, operation, body) in rows {
        let answer = call(&router, operation, &body).await;
        assert_ne!(
            answer.status, 400,
            "case {case:?}: a value inside every modelled constraint must not be \
             refused, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.message().contains("validation error"),
            "case {case:?}: nothing was refused, so no validation envelope may be \
             stated, got {:?}",
            answer.message()
        );
    }
}
