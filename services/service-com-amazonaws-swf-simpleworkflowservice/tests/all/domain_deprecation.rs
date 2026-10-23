// SPDX-License-Identifier: CC0-1.0

//! `DeprecateDomain`, end to end over the service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`), through the
//! same `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: `DeprecateDomain` moves the ONE stored domain
//! record it names from `REGISTERED` to `DEPRECATED` and, with it, every activity and
//! workflow type registered in that domain (the operation's doc: "Deprecating a domain
//! also deprecates all activity and workflow types registered in the domain"), leaving
//! executions started before the call running ("Executions that were started before the
//! domain was deprecated continues to run"); a name that no stored domain carries is
//! refused with HTTP 400 `UnknownResourceFault` and a domain that is already deprecated
//! with HTTP 400 `DomainDeprecatedFault` ("Returned when the specified domain has been
//! deprecated"), each naming the requested domain in its message, and neither of which
//! changes what a later caller sees.
//!
//! The tables below vary what that rule generalises over: which kinds of type the
//! domain holds and how many (none, one, several names, several versions of one name,
//! workflow types, activity types, both), the shape of the domain name, the retention
//! period it was registered with, which domain of several is addressed, whether the
//! domain has a running execution, and the way a name fails to address a stored domain.

use serde_json::{json, Map, Value};
use service_com_amazonaws_swf_simpleworkflowservice::ops::types::REGISTRATION_STATUS_VALUES;

use crate::workflow_execution_lifecycle::{
    error_code, full_defaults, merge, register_domain_body, string_at, Emulator,
};

/// One type identity: `(name, version)` — the pair every SWF type record is keyed by.
type TypeId = (&'static str, &'static str);

/// The two kinds of type a domain holds, and the operations and members each one is
/// addressed through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Workflow,
    Activity,
}

impl Kind {
    fn register_operation(self) -> &'static str {
        match self {
            Self::Workflow => "RegisterWorkflowType",
            Self::Activity => "RegisterActivityType",
        }
    }

    fn describe_operation(self) -> &'static str {
        match self {
            Self::Workflow => "DescribeWorkflowType",
            Self::Activity => "DescribeActivityType",
        }
    }

    /// The input member that carries the type identity.
    fn member(self) -> &'static str {
        match self {
            Self::Workflow => "workflowType",
            Self::Activity => "activityType",
        }
    }
}

/// One type registered in a domain.
#[derive(Debug, Clone, Copy)]
struct RegisteredType {
    kind: Kind,
    id: TypeId,
}

fn workflow(id: TypeId) -> RegisteredType {
    RegisteredType {
        kind: Kind::Workflow,
        id,
    }
}

fn activity(id: TypeId) -> RegisteredType {
    RegisteredType {
        kind: Kind::Activity,
        id,
    }
}

/// One domain that gets deprecated. Rows differ in the registered retention period,
/// the shape of the name, and which types the domain holds.
struct DomainCase {
    domain: &'static str,
    retention: &'static str,
    types: Vec<RegisteredType>,
}

/// The status a domain or type reports, checked against the modelled
/// `RegistrationStatus` value set (`REGISTRATION_STATUS_VALUES`) so no case can pass on
/// a status the model does not define.
fn modelled_status(case: &str, body: &Value, pointer: &str) -> String {
    let status = string_at(body, pointer);
    assert!(
        REGISTRATION_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: {pointer} must be one of {REGISTRATION_STATUS_VALUES:?}, \
         got {status:?} in {body:?}"
    );
    status
}

/// An epoch-seconds timestamp member, or `None` when the member is absent.
fn timestamp_at(body: &Value, pointer: &str) -> Option<f64> {
    body.pointer(pointer).and_then(Value::as_f64)
}

async fn describe_domain(emulator: &Emulator, case: &str, domain: &str) -> Value {
    emulator
        .ok(case, "DescribeDomain", &json!({ "name": domain }))
        .await
}

/// Read one type back through its own `Describe*`, whichever kind it is.
async fn describe_type(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    ty: RegisteredType,
) -> Value {
    let mut body = Map::new();
    body.insert("domain".to_string(), json!(domain));
    body.insert(
        ty.kind.member().to_string(),
        json!({ "name": ty.id.0, "version": ty.id.1 }),
    );
    emulator
        .ok(case, ty.kind.describe_operation(), &Value::Object(body))
        .await
}

/// Register `case.domain` with its types and assert everything starts `REGISTERED`
/// with no `deprecationDate` — the before half of each case.
async fn seed_domain(emulator: &Emulator, case: &DomainCase) {
    emulator
        .ok(
            case.domain,
            "RegisterDomain",
            &register_domain_body(case.domain, case.retention),
        )
        .await;
    for ty in &case.types {
        let mut body = json!({ "domain": case.domain, "name": ty.id.0, "version": ty.id.1 });
        if ty.kind == Kind::Workflow {
            // Full registration-time defaults, so a start in this domain needs no
            // overrides at all.
            merge(&mut body, full_defaults("default-tasks", "TERMINATE"));
        }
        emulator
            .ok(case.domain, ty.kind.register_operation(), &body)
            .await;
    }

    let described = describe_domain(emulator, case.domain, case.domain).await;
    assert_eq!(
        modelled_status(case.domain, &described, "/domainInfo/status"),
        "REGISTERED",
        "case {:?}: a freshly registered domain is REGISTERED, got {described:?}",
        case.domain
    );
    for ty in &case.types {
        let described = describe_type(emulator, case.domain, case.domain, *ty).await;
        assert_eq!(
            modelled_status(case.domain, &described, "/typeInfo/status"),
            "REGISTERED",
            "case {:?}: type {ty:?} starts REGISTERED, got {described:?}",
            case.domain
        );
        assert_eq!(
            timestamp_at(&described, "/typeInfo/deprecationDate"),
            None,
            "case {:?}: type {ty:?} carries no deprecationDate before deprecation, \
             got {described:?}",
            case.domain
        );
    }
}

/// Every domain the deprecation table covers.
fn domain_cases() -> Vec<DomainCase> {
    vec![
        // No types at all: the domain's own status is the whole effect.
        DomainCase {
            domain: "empty-domain",
            retention: "1",
            types: Vec::new(),
        },
        DomainCase {
            domain: "one-type-domain",
            retention: "7",
            types: vec![workflow(("orders", "1.0"))],
        },
        // Activity types only: the cascade is over BOTH registries, not just the
        // workflow one.
        DomainCase {
            domain: "activities-only-domain",
            retention: "3",
            types: vec![activity(("pack", "1.0")), activity(("pack", "2.0"))],
        },
        // Several distinct names AND several versions of one name, of both kinds, so
        // the cascade is over every stored type rather than one per name or per kind.
        DomainCase {
            domain: "many-types.domain_1",
            retention: "90",
            types: vec![
                workflow(("orders", "1.0")),
                workflow(("orders", "2.0")),
                workflow(("shipping", "1.0")),
                workflow(("白板-workflow", "1")),
                activity(("charge", "1.0")),
                activity(("charge", "1.1")),
                activity(("notify", "7")),
            ],
        },
        // `workflowExecutionRetentionPeriodInDays` accepts the literal "NONE"; the
        // retention value has nothing to do with deprecation, which is the point.
        DomainCase {
            domain: "no-retention-domain",
            retention: "NONE",
            types: vec![workflow(("audit", "0.1")), activity(("audit", "0.1"))],
        },
    ]
}

#[tokio::test]
async fn deprecating_a_domain_moves_its_record_and_cascades_to_its_types() {
    let emulator = Emulator::new();
    let cases = domain_cases();
    for case in &cases {
        seed_domain(&emulator, case).await;
    }

    for (index, case) in cases.iter().enumerate() {
        let before = describe_domain(&emulator, case.domain, case.domain).await;
        let deprecated = emulator
            .ok(
                case.domain,
                "DeprecateDomain",
                &json!({ "name": case.domain }),
            )
            .await;
        assert_eq!(
            deprecated,
            json!({}),
            "case {:?}: DeprecateDomain's output shape is Unit, got {deprecated:?}",
            case.domain
        );

        let after = describe_domain(&emulator, case.domain, case.domain).await;
        assert_eq!(
            modelled_status(case.domain, &after, "/domainInfo/status"),
            "DEPRECATED",
            "case {:?}: the stored domain must report DEPRECATED, got {after:?}",
            case.domain
        );
        // Nothing else about the record moved: the ARN and the retention period are
        // what registration stored.
        assert_eq!(
            after.pointer("/domainInfo/arn"),
            before.pointer("/domainInfo/arn"),
            "case {:?}: deprecation must not change the domain ARN, got {after:?}",
            case.domain
        );
        assert_eq!(
            string_at(
                &after,
                "/configuration/workflowExecutionRetentionPeriodInDays"
            ),
            case.retention,
            "case {:?}: deprecation must not change the retention period, got {after:?}",
            case.domain
        );

        // Every type registered in this domain is deprecated with it, and carries a
        // deprecationDate at or after its own creationDate.
        for ty in &case.types {
            let described = describe_type(&emulator, case.domain, case.domain, *ty).await;
            assert_eq!(
                modelled_status(case.domain, &described, "/typeInfo/status"),
                "DEPRECATED",
                "case {:?}: type {ty:?} must be deprecated with its domain, got {described:?}",
                case.domain
            );
            let creation = timestamp_at(&described, "/typeInfo/creationDate").unwrap_or_else(|| {
                panic!("case {:?}: no creationDate in {described:?}", case.domain)
            });
            let deprecation =
                timestamp_at(&described, "/typeInfo/deprecationDate").unwrap_or_else(|| {
                    panic!(
                        "case {:?}: type {ty:?} must report a deprecationDate, got {described:?}",
                        case.domain
                    )
                });
            assert!(
                deprecation >= creation,
                "case {:?}: type {ty:?} deprecationDate {deprecation} must be at or after \
                 creationDate {creation}, got {described:?}",
                case.domain
            );
        }

        // One call addresses ONE record: the domains deprecated earlier stay
        // deprecated, and the ones not yet addressed are still REGISTERED with their
        // types untouched.
        for (other_index, other) in cases.iter().enumerate() {
            let expected = if other_index <= index {
                "DEPRECATED"
            } else {
                "REGISTERED"
            };
            let described = describe_domain(&emulator, other.domain, other.domain).await;
            assert_eq!(
                modelled_status(other.domain, &described, "/domainInfo/status"),
                expected,
                "case {:?}: after deprecating {:?}, this domain must be {expected}, \
                 got {described:?}",
                other.domain,
                case.domain
            );
            for ty in &other.types {
                let described = describe_type(&emulator, other.domain, other.domain, *ty).await;
                assert_eq!(
                    modelled_status(other.domain, &described, "/typeInfo/status"),
                    expected,
                    "case {:?}: after deprecating {:?}, type {ty:?} must be {expected}, \
                     got {described:?}",
                    other.domain,
                    case.domain
                );
            }
        }
    }
}

#[tokio::test]
async fn deprecating_an_already_deprecated_domain_is_refused() {
    let emulator = Emulator::new();
    let cases = domain_cases();
    for case in &cases {
        seed_domain(&emulator, case).await;
        emulator
            .ok(
                case.domain,
                "DeprecateDomain",
                &json!({ "name": case.domain }),
            )
            .await;
    }

    for case in &cases {
        // The state the first deprecation left, as the readers report it.
        let after_first = describe_domain(&emulator, case.domain, case.domain).await;
        let types_after_first: Vec<Value> = {
            let mut described = Vec::new();
            for ty in &case.types {
                described.push(describe_type(&emulator, case.domain, case.domain, *ty).await);
            }
            described
        };

        // Repeated attempts are each refused the same way; none of them is a silent
        // re-deprecation that re-dates the types. The fault's message is the name of
        // the domain it refers to, so it is request-derived and therefore a
        // requirement, over every name shape the table covers.
        for round in 1..=3 {
            let round_case = format!("{} refusal #{round}", case.domain);
            let (status, body) = emulator
                .call("DeprecateDomain", &json!({ "name": case.domain }))
                .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "DomainDeprecatedFault"),
                "case {round_case:?}: expected 400 DomainDeprecatedFault, got {status} {code} \
                 with {body:?}"
            );
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {round_case:?}: expected a message, got {body:?}")
                });
            assert_eq!(
                message, case.domain,
                "case {round_case:?}: the fault message is the deprecated domain's name, \
                 got {message:?}"
            );
        }

        let after_refusals = describe_domain(&emulator, case.domain, case.domain).await;
        assert_eq!(
            after_refusals, after_first,
            "case {:?}: a refused deprecation must not change the stored domain, \
             got {after_refusals:?}",
            case.domain
        );
        for (ty, expected) in case.types.iter().zip(&types_after_first) {
            let described = describe_type(&emulator, case.domain, case.domain, *ty).await;
            assert_eq!(
                &described, expected,
                "case {:?}: a refused deprecation must not re-date type {ty:?}, \
                 got {described:?}",
                case.domain
            );
        }
    }
}

/// One way a `name` can fail to address a stored domain. The message is request-derived
/// — the fault names the domain the request asked for — so it is a requirement here.
struct UnknownCase {
    case: &'static str,
    name: String,
}

#[tokio::test]
async fn deprecating_a_domain_that_is_not_stored_is_an_unknown_resource() {
    let emulator = Emulator::new();
    let stored = DomainCase {
        domain: "stored-domain",
        retention: "7",
        types: vec![workflow(("orders", "1.0")), activity(("charge", "1.0"))],
    };
    seed_domain(&emulator, &stored).await;

    let cases = vec![
        UnknownCase {
            case: "never registered",
            name: "no-such-domain".to_string(),
        },
        // Domain names are matched exactly: a different case, or extra characters, is a
        // different name and so no stored domain.
        UnknownCase {
            case: "differs in case",
            name: stored.domain.to_uppercase(),
        },
        UnknownCase {
            case: "a prefix of a stored name",
            name: "stored".to_string(),
        },
        UnknownCase {
            case: "a stored name with a suffix",
            name: format!("{}-2", stored.domain),
        },
        UnknownCase {
            case: "surrounded by whitespace",
            name: format!(" {} ", stored.domain),
        },
        // A type registered inside the stored domain is not itself a domain.
        UnknownCase {
            case: "a workflow type name",
            name: "orders".to_string(),
        },
        UnknownCase {
            case: "an activity type name",
            name: "charge".to_string(),
        },
        // The domain's ARN is not its name: this operation takes `DomainName`.
        UnknownCase {
            case: "the stored domain's ARN",
            name: string_at(
                &describe_domain(&emulator, "arn", stored.domain).await,
                "/domainInfo/arn",
            ),
        },
    ];

    for case in &cases {
        let (status, body) = emulator
            .call("DeprecateDomain", &json!({ "name": case.name }))
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "UnknownResourceFault"),
            "case {:?}: expected 400 UnknownResourceFault for {:?}, got {status} {code} \
             with {body:?}",
            case.case,
            case.name
        );
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {:?}: expected a message, got {body:?}", case.case));
        assert_eq!(
            message,
            format!("Unknown domain: {}", case.name),
            "case {:?}: the fault message names the requested domain, got {message:?}",
            case.case
        );
    }

    // None of those refusals touched the store, so the stored domain is still
    // deprecable exactly once.
    let described = describe_domain(&emulator, "after refusals", stored.domain).await;
    assert_eq!(
        modelled_status("after refusals", &described, "/domainInfo/status"),
        "REGISTERED",
        "case \"after refusals\": a refused deprecation must leave the domain REGISTERED, \
         got {described:?}"
    );
    emulator
        .ok(
            "after refusals",
            "DeprecateDomain",
            &json!({ "name": stored.domain }),
        )
        .await;
    let described = describe_domain(&emulator, "after refusals", stored.domain).await;
    assert_eq!(
        modelled_status("after refusals", &described, "/domainInfo/status"),
        "DEPRECATED",
        "case \"after refusals\": the domain deprecates after the refusals, got {described:?}"
    );
}

#[tokio::test]
async fn executions_started_before_the_deprecation_keep_running() {
    let emulator = Emulator::new();
    let case = DomainCase {
        domain: "running-domain",
        retention: "7",
        types: vec![workflow(("orders", "1.0"))],
    };
    seed_domain(&emulator, &case).await;

    // Two runs of different workflowIds, so the assertion is not about one record.
    let mut runs: Vec<(&str, String)> = Vec::new();
    for workflow_id in ["wf-1", "wf-2"] {
        let started = emulator
            .ok(
                case.domain,
                "StartWorkflowExecution",
                &json!({
                    "domain": case.domain,
                    "workflowId": workflow_id,
                    "workflowType": { "name": "orders", "version": "1.0" },
                }),
            )
            .await;
        runs.push((workflow_id, string_at(&started, "/runId")));
    }

    let histories_before: Vec<usize> = {
        let mut lengths = Vec::new();
        for (workflow_id, run_id) in &runs {
            lengths.push(history_length(&emulator, case.domain, workflow_id, run_id).await);
        }
        lengths
    };

    emulator
        .ok(
            case.domain,
            "DeprecateDomain",
            &json!({ "name": case.domain }),
        )
        .await;

    for ((workflow_id, run_id), before) in runs.iter().zip(&histories_before) {
        let described = emulator
            .ok(
                case.domain,
                "DescribeWorkflowExecution",
                &json!({
                    "domain": case.domain,
                    "execution": { "workflowId": workflow_id, "runId": run_id },
                }),
            )
            .await;
        assert_eq!(
            string_at(&described, "/executionInfo/executionStatus"),
            "OPEN",
            "case {workflow_id:?}: an execution started before the deprecation keeps \
             running, got {described:?}"
        );
        let after = history_length(&emulator, case.domain, workflow_id, run_id).await;
        assert_eq!(
            after, *before,
            "case {workflow_id:?}: deprecation must not change the run's history, \
             expected {before} events, got {after}"
        );
    }
}

/// How many history events one run has, read through `GetWorkflowExecutionHistory`.
async fn history_length(
    emulator: &Emulator,
    domain: &str,
    workflow_id: &str,
    run_id: &str,
) -> usize {
    let body = json!({
        "domain": domain,
        "execution": { "workflowId": workflow_id, "runId": run_id },
    });
    let answer = emulator
        .ok(workflow_id, "GetWorkflowExecutionHistory", &body)
        .await;
    answer
        .pointer("/events")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {workflow_id:?}: no events in {answer:?}"))
        .len()
}
