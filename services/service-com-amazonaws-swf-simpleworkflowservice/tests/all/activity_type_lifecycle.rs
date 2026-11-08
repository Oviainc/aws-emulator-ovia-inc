// SPDX-License-Identifier: CC0-1.0

//! The activity-type registration lifecycle — `DeprecateActivityType`,
//! `UndeprecateActivityType` and `DeleteActivityType`, read back through
//! `DescribeActivityType` — end to end over this service's own wire protocol
//! (`awsJson1_0`, `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`),
//! dispatched through the same `MultiServiceRouter` the emulator mounts in production.
//! The harness is the one `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: each of these operations acts on the ONE stored
//! activity-type record its `(domain, name, version)` triple addresses and follows that
//! record's own stored `RegistrationStatus` —
//!
//! * `DeprecateActivityType` moves `REGISTERED` -> `DEPRECATED` and stamps
//!   `deprecationDate`; a record already `DEPRECATED` is HTTP 400 `TypeDeprecatedFault`;
//! * `UndeprecateActivityType` moves `DEPRECATED` -> `REGISTERED` and drops that stamp
//!   (`ActivityTypeInfo$deprecationDate`: "If DEPRECATED, the date and time
//!   DeprecateActivityType was called"); a record already `REGISTERED` is HTTP 400
//!   `TypeAlreadyExistsFault` ("...or if you undeprecate a type that is currently
//!   registered");
//! * `DeleteActivityType` removes a `DEPRECATED` record, freeing the identity to be
//!   registered again; a record still `REGISTERED` is HTTP 400
//!   `TypeNotDeprecatedFault` ("Returned when the resource type has not been
//!   deprecated") with nothing removed;
//! * a triple naming no stored record is HTTP 400 `UnknownResourceFault`;
//! * a refused call changes nothing a later caller sees, and the status of the DOMAIN
//!   the record lives in never stands in for the record's own status.
//!
//! Rows vary what the rule generalises over: the form and length of `name`/`version`
//! (plain, digits, embedded space, non-ASCII, each modelled bound), which version of one
//! name is addressed, which registration-time defaults the record carries, how many
//! times the deprecate/undeprecate cycle is run, and whether the domain holding the
//! record is itself deprecated. No row is privileged.

use serde_json::{json, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::REGISTRATION_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{
    error_code, merge, register_domain_body, string_at, Emulator,
};

/// The modelled maximum length of `ActivityType$name`
/// (`smithy_query(query="shape",shape="com.amazonaws.swf#ActivityType")`: `name` targets
/// `Name`, len 1..256).
const MAX_TYPE_NAME_LEN: usize = 256;

/// The modelled maximum length of `ActivityType$version` (same query: `version` targets
/// `Version`, len 1..64).
const MAX_TYPE_VERSION_LEN: usize = 64;

/// One stored activity type the lifecycle is driven over. Rows differ in the shape and
/// length of the identity and in the registration-time defaults the record carries, so
/// the round trip is checked over a whole record and not just a status member.
struct TypeCase {
    case: &'static str,
    name: String,
    version: String,
    /// Optional `RegisterActivityType` members folded into the registration.
    defaults: Value,
}

fn type_cases() -> Vec<TypeCase> {
    vec![
        TypeCase {
            case: "plain identity, no defaults",
            name: "checkout".to_string(),
            version: "1.0".to_string(),
            defaults: json!({}),
        },
        TypeCase {
            // The same name at another version is another record entirely.
            case: "same name, other version, every default",
            name: "checkout".to_string(),
            version: "2.0".to_string(),
            defaults: json!({
                "description": "the second version",
                "defaultTaskList": { "name": "packing" },
                "defaultTaskPriority": "10",
                "defaultTaskStartToCloseTimeout": "300",
                "defaultTaskHeartbeatTimeout": "NONE",
                "defaultTaskScheduleToStartTimeout": "0",
                "defaultTaskScheduleToCloseTimeout": "600",
            }),
        },
        TypeCase {
            case: "digits only",
            name: "7".to_string(),
            version: "0001".to_string(),
            defaults: json!({ "defaultTaskHeartbeatTimeout": "60" }),
        },
        TypeCase {
            case: "embedded spaces",
            name: "pack items".to_string(),
            version: "v 1".to_string(),
            defaults: json!({}),
        },
        TypeCase {
            case: "non-ASCII identity",
            name: "活動タイプ".to_string(),
            version: "ß-1".to_string(),
            defaults: json!({ "description": "unicode" }),
        },
        TypeCase {
            case: "modelled maximum lengths",
            name: "n".repeat(MAX_TYPE_NAME_LEN),
            version: "v".repeat(MAX_TYPE_VERSION_LEN),
            defaults: json!({}),
        },
    ]
}

/// The `activityType` member of a request addressing one type.
fn type_ref(name: &str, version: &str) -> Value {
    json!({ "name": name, "version": version })
}

/// A request body addressing one type in one domain — the input shape
/// `DeprecateActivityType`, `UndeprecateActivityType`, `DeleteActivityType` and
/// `DescribeActivityType` share.
fn type_request(domain: &str, name: &str, version: &str) -> Value {
    json!({ "domain": domain, "activityType": type_ref(name, version) })
}

/// How Amazon SWF names one activity type in a fault message, built from the requested
/// identity alone.
fn type_reference(name: &str, version: &str) -> String {
    format!("ActivityType=[name={name}, version={version}]")
}

/// A valid identity component that no registration stored: a value differing from
/// `value` while staying inside the modelled length bound `max`, so a row sitting at
/// that bound is still refused by the handler rather than by the boundary validator.
fn absent_variant(value: &str, max: usize) -> String {
    if value.chars().count() >= max {
        let mut chars: Vec<char> = value.chars().collect();
        chars[0] = 'z';
        chars.into_iter().collect()
    } else {
        format!("{value}-absent")
    }
}

async fn register_domain(emulator: &Emulator, case: &str, domain: &str) {
    emulator
        .ok(case, "RegisterDomain", &register_domain_body(domain, "7"))
        .await;
}

async fn register_type(emulator: &Emulator, case: &TypeCase, domain: &str) {
    let mut body = json!({ "domain": domain, "name": case.name, "version": case.version });
    merge(&mut body, case.defaults.clone());
    emulator.ok(case.case, "RegisterActivityType", &body).await;
}

/// Seed one domain holding every row of the table.
async fn seed(emulator: &Emulator, domain: &str, cases: &[TypeCase]) {
    register_domain(emulator, domain, domain).await;
    for case in cases {
        register_type(emulator, case, domain).await;
    }
}

/// The stored record, read back through the implemented reader.
async fn describe_type(emulator: &Emulator, case: &str, domain: &str, ty: &TypeCase) -> Value {
    emulator
        .ok(
            case,
            "DescribeActivityType",
            &type_request(domain, &ty.name, &ty.version),
        )
        .await
}

/// Call one operation addressing `ty`, requiring the modelled empty success.
async fn ok_on_type(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    domain: &str,
    ty: &TypeCase,
) {
    let response = emulator
        .ok(
            case,
            operation,
            &type_request(domain, &ty.name, &ty.version),
        )
        .await;
    assert_eq!(
        response,
        json!({}),
        "case {case:?}: {operation}'s modelled output shape is Unit, got {response:?}"
    );
}

/// Call one operation addressing `ty`, requiring HTTP 400 `expected_code`, and return
/// the fault's message.
async fn refused_on_type(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    domain: &str,
    ty: &TypeCase,
    expected_code: &str,
) -> String {
    let (status, body) = emulator
        .call(operation, &type_request(domain, &ty.name, &ty.version))
        .await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, expected_code),
        "case {case:?}: expected {operation} to answer 400 {expected_code}, \
         got {status} {code} with {body:?}"
    );
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body:?}"))
        .to_string()
}

/// The modelled `RegistrationStatus` the record reports, checked against the model's own
/// value set so no case can pass on a status the model does not define.
fn modelled_status(case: &str, described: &Value) -> String {
    let status = string_at(described, "/typeInfo/status");
    assert!(
        REGISTRATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: status must be one of the modelled RegistrationStatus values \
         {REGISTRATION_STATUS_VALUES:?}, got {status:?}"
    );
    status
}

/// Assert `described` reports a REGISTERED record with no deprecation stamp.
fn assert_registered(case: &str, described: &Value) {
    assert_eq!(
        modelled_status(case, described),
        "REGISTERED",
        "case {case:?}: a type that is not deprecated reports REGISTERED, got {described:?}"
    );
    assert_eq!(
        described.pointer("/typeInfo/deprecationDate"),
        None,
        "case {case:?}: deprecationDate is reported only while the type is DEPRECATED, \
         got {described:?}"
    );
}

/// Assert `described` reports a DEPRECATED record and return its `deprecationDate`,
/// which must be at or after the record's own `creationDate`.
fn assert_deprecated(case: &str, described: &Value) -> f64 {
    assert_eq!(
        modelled_status(case, described),
        "DEPRECATED",
        "case {case:?}: a deprecated type reports DEPRECATED, got {described:?}"
    );
    let creation = described
        .pointer("/typeInfo/creationDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a creationDate in {described:?}"));
    let deprecation = described
        .pointer("/typeInfo/deprecationDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a deprecated record carries its stamp, got {described:?}")
        });
    assert!(
        deprecation >= creation,
        "case {case:?}: deprecationDate {deprecation} must be at or after creationDate \
         {creation}, got {described:?}"
    );
    deprecation
}

/// Assert the identity names no stored record any more: its reader answers the modelled
/// fault, naming the requested type and nothing else.
async fn assert_type_absent(emulator: &Emulator, case: &str, domain: &str, ty: &TypeCase) {
    let message = refused_on_type(
        emulator,
        case,
        "DescribeActivityType",
        domain,
        ty,
        "UnknownResourceFault",
    )
    .await;
    assert_eq!(
        message,
        format!("Unknown type: {}", type_reference(&ty.name, &ty.version)),
        "case {case:?}: the fault names the requested type, got {message:?}"
    );
}

#[tokio::test]
async fn deprecate_and_undeprecate_move_one_record_back_and_forth() {
    let emulator = Emulator::new();
    let domain = "lifecycle";
    let cases = type_cases();
    seed(&emulator, domain, &cases).await;

    for case in &cases {
        let registered = describe_type(&emulator, case.case, domain, case).await;
        assert_registered(case.case, &registered);

        // Two full cycles: the round trip is an invariant, not a one-shot.
        for round in 1..=2 {
            let round_case = format!("{} cycle #{round}", case.case);
            ok_on_type(
                &emulator,
                &round_case,
                "DeprecateActivityType",
                domain,
                case,
            )
            .await;
            let deprecated = describe_type(&emulator, &round_case, domain, case).await;
            let stamp = assert_deprecated(&round_case, &deprecated);

            // A second deprecation of the same record is refused and keeps the first
            // stamp: the move is off REGISTERED, and the fault names the requested type.
            let message = refused_on_type(
                &emulator,
                &round_case,
                "DeprecateActivityType",
                domain,
                case,
                "TypeDeprecatedFault",
            )
            .await;
            assert_eq!(
                message,
                type_reference(&case.name, &case.version),
                "case {round_case:?}: the fault names the requested type, got {message:?}"
            );
            let after_refusal = describe_type(&emulator, &round_case, domain, case).await;
            assert_eq!(
                assert_deprecated(&round_case, &after_refusal),
                stamp,
                "case {round_case:?}: a refused deprecation must not restamp the record, \
                 got {after_refusal:?}"
            );

            ok_on_type(
                &emulator,
                &round_case,
                "UndeprecateActivityType",
                domain,
                case,
            )
            .await;
            let undeprecated = describe_type(&emulator, &round_case, domain, case).await;
            assert_registered(&round_case, &undeprecated);
            // Everything else the record holds — identity, description, registered
            // configuration, creationDate — is exactly what registration stored.
            assert_eq!(
                undeprecated, registered,
                "case {round_case:?}: an undeprecated record reads back as it was \
                 registered, got {undeprecated:?}"
            );
        }

        // Every other row is untouched by this row's cycles.
        for other in &cases {
            if other.case == case.case {
                continue;
            }
            let described = describe_type(&emulator, other.case, domain, other).await;
            assert_registered(other.case, &described);
        }
    }
}

#[tokio::test]
async fn undeprecating_a_registered_type_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let domain = "already-registered";
    let cases = type_cases();
    seed(&emulator, domain, &cases).await;

    for case in &cases {
        let before = describe_type(&emulator, case.case, domain, case).await;
        assert_registered(case.case, &before);

        for round in 1..=3 {
            let round_case = format!("{} refusal #{round}", case.case);
            let message = refused_on_type(
                &emulator,
                &round_case,
                "UndeprecateActivityType",
                domain,
                case,
                "TypeAlreadyExistsFault",
            )
            .await;
            assert_eq!(
                message,
                type_reference(&case.name, &case.version),
                "case {round_case:?}: the fault is built from the requested identity, \
                 got {message:?}"
            );
            let after = describe_type(&emulator, &round_case, domain, case).await;
            assert_eq!(
                after, before,
                "case {round_case:?}: a refused undeprecation must not change what a \
                 later caller sees, got {after:?}"
            );
        }

        // And the same refusal returns once the record is back on REGISTERED.
        ok_on_type(&emulator, case.case, "DeprecateActivityType", domain, case).await;
        ok_on_type(
            &emulator,
            case.case,
            "UndeprecateActivityType",
            domain,
            case,
        )
        .await;
        refused_on_type(
            &emulator,
            case.case,
            "UndeprecateActivityType",
            domain,
            case,
            "TypeAlreadyExistsFault",
        )
        .await;
        let after = describe_type(&emulator, case.case, domain, case).await;
        assert_registered(case.case, &after);
    }
}

#[tokio::test]
async fn a_type_that_is_not_deprecated_cannot_be_deleted() {
    let emulator = Emulator::new();
    let domain = "deletions";
    let cases = type_cases();
    seed(&emulator, domain, &cases).await;

    // The `TypeNotDeprecatedFault` message reports the STATE of the record, so it is one
    // sentence carrying no value out of the request: every row must see the same one.
    let mut messages: Vec<String> = Vec::new();

    for case in &cases {
        let before = describe_type(&emulator, case.case, domain, case).await;
        assert_registered(case.case, &before);

        for round in 1..=2 {
            let round_case = format!("{} refusal #{round}", case.case);
            let message = refused_on_type(
                &emulator,
                &round_case,
                "DeleteActivityType",
                domain,
                case,
                "TypeNotDeprecatedFault",
            )
            .await;
            assert!(
                !message.contains(&case.name)
                    && !message.contains(&case.version)
                    && !message.contains(domain),
                "case {round_case:?}: the fault reports the record's state and names no \
                 value out of the request, got {message:?}"
            );
            messages.push(message);
            let after = describe_type(&emulator, &round_case, domain, case).await;
            assert_eq!(
                after, before,
                "case {round_case:?}: a refused deletion must not change what a later \
                 caller sees, got {after:?}"
            );
        }

        // Once the record IS deprecated the very same request succeeds, the identity
        // stops naming a record, and a second deletion is the unknown-resource fault.
        ok_on_type(&emulator, case.case, "DeprecateActivityType", domain, case).await;
        ok_on_type(&emulator, case.case, "DeleteActivityType", domain, case).await;
        assert_type_absent(&emulator, case.case, domain, case).await;
        let message = refused_on_type(
            &emulator,
            case.case,
            "DeleteActivityType",
            domain,
            case,
            "UnknownResourceFault",
        )
        .await;
        assert_eq!(
            message,
            format!("Unknown type: {}", type_reference(&case.name, &case.version)),
            "case {:?}: a deleted identity names no record, got {message:?}",
            case.case
        );

        // The identity is free again: a fresh registration stores a new REGISTERED
        // record under it.
        register_type(&emulator, case, domain).await;
        let reregistered = describe_type(&emulator, case.case, domain, case).await;
        assert_registered(case.case, &reregistered);
    }

    let first = messages.first().expect("the table is not empty");
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(
            message, first,
            "case #{index}: every refused deletion carries the same state sentence, \
             got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_deprecated_domain_leaves_its_activity_types_registered() {
    // Deprecating a DOMAIN moves only that domain's own status: a type registered in it
    // before the call still reports REGISTERED, so it is still refused a deletion and
    // still refused an undeprecation, and its own deprecation is what moves it.
    let emulator = Emulator::new();
    let domain = "deprecated-domain";
    let cases = type_cases();
    seed(&emulator, domain, &cases).await;

    let before: Vec<Value> = {
        let mut described = Vec::new();
        for case in &cases {
            described.push(describe_type(&emulator, case.case, domain, case).await);
        }
        described
    };

    emulator
        .ok(
            "deprecate domain",
            "DeprecateDomain",
            &json!({ "name": domain }),
        )
        .await;
    let domain_record = emulator
        .ok(
            "deprecate domain",
            "DescribeDomain",
            &json!({ "name": domain }),
        )
        .await;
    assert_eq!(
        string_at(&domain_record, "/domainInfo/status"),
        "DEPRECATED",
        "case \"deprecate domain\": the domain itself reports DEPRECATED, \
         got {domain_record:?}"
    );

    for (case, expected) in cases.iter().zip(&before) {
        let after = describe_type(&emulator, case.case, domain, case).await;
        assert_eq!(
            &after, expected,
            "case {:?}: deprecating the domain must not change the type's record, \
             got {after:?}",
            case.case
        );
        assert_registered(case.case, &after);

        refused_on_type(
            &emulator,
            case.case,
            "DeleteActivityType",
            domain,
            case,
            "TypeNotDeprecatedFault",
        )
        .await;
        refused_on_type(
            &emulator,
            case.case,
            "UndeprecateActivityType",
            domain,
            case,
            "TypeAlreadyExistsFault",
        )
        .await;

        // The type's own deprecation still works in a deprecated domain, and so does
        // the rest of its lifecycle.
        ok_on_type(&emulator, case.case, "DeprecateActivityType", domain, case).await;
        assert_deprecated(
            case.case,
            &describe_type(&emulator, case.case, domain, case).await,
        );
        ok_on_type(
            &emulator,
            case.case,
            "UndeprecateActivityType",
            domain,
            case,
        )
        .await;
        assert_registered(
            case.case,
            &describe_type(&emulator, case.case, domain, case).await,
        );
    }
}

#[tokio::test]
async fn a_triple_naming_no_stored_activity_type_is_an_unknown_resource() {
    let emulator = Emulator::new();
    let home = "home-domain";
    let other = "other-domain";
    let cases = type_cases();
    seed(&emulator, home, &cases).await;
    register_domain(&emulator, other, other).await;

    // Every operation of this lifecycle resolves the triple the same way, so each one
    // must refuse each way of missing identically — with one documented split: a WRITE
    // resolves the domain it writes into before the type inside it, so it reports an
    // ABSENT DOMAIN as the domain, while the READ resolves the type directly and reports
    // the type it was asked for. The flag below is which of the two an operation is.
    let operations = [
        ("DeprecateActivityType", true),
        ("UndeprecateActivityType", true),
        ("DeleteActivityType", true),
        ("DescribeActivityType", false),
    ];

    for case in &cases {
        let absent_name = absent_variant(&case.name, MAX_TYPE_NAME_LEN);
        let absent_version = absent_variant(&case.version, MAX_TYPE_VERSION_LEN);
        let absent_domain = "no-such-domain";
        let missing = vec![
            (
                "domain never registered",
                absent_domain.to_string(),
                case.name.clone(),
                case.version.clone(),
                format!("Unknown type: {}", type_reference(&case.name, &case.version)),
            ),
            (
                // The lookup is domain-scoped: the type exists, just not here.
                "registered in another domain",
                other.to_string(),
                case.name.clone(),
                case.version.clone(),
                format!("Unknown type: {}", type_reference(&case.name, &case.version)),
            ),
            (
                "name no registration stored",
                home.to_string(),
                absent_name.clone(),
                case.version.clone(),
                format!(
                    "Unknown type: {}",
                    type_reference(&absent_name, &case.version)
                ),
            ),
            (
                "version no registration stored",
                home.to_string(),
                case.name.clone(),
                absent_version.clone(),
                format!(
                    "Unknown type: {}",
                    type_reference(&case.name, &absent_version)
                ),
            ),
        ];

        for (why, domain, name, version, expected_message) in missing {
            for (operation, writes) in operations {
                let label = format!("{} / {why} / {operation}", case.case);
                // Only the absent-domain row differs between the two kinds: a write
                // names the domain it could not resolve, a read names the type.
                let expected = if writes && domain == absent_domain {
                    format!("Unknown domain: {absent_domain}")
                } else {
                    expected_message.clone()
                };
                let (status, body) = emulator
                    .call(operation, &type_request(&domain, &name, &version))
                    .await;
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "UnknownResourceFault"),
                    "case {label:?}: expected 400 UnknownResourceFault, got {status} {code} \
                     with {body:?}"
                );
                let message = body
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {label:?}: expected a message, got {body:?}"));
                assert_eq!(
                    message, expected,
                    "case {label:?}: the fault is built from the requested resource, \
                     got {message:?}"
                );
            }
        }

        // None of those refusals touched the stored record.
        let described = describe_type(&emulator, case.case, home, case).await;
        assert_registered(case.case, &described);
    }
}
