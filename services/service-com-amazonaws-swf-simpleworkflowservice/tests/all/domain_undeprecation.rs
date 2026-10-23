// SPDX-License-Identifier: CC0-1.0

//! `UndeprecateDomain`, end to end over the service's own wire protocol (`awsJson1_0`,
//! `POST /` with `X-Amz-Target: SimpleWorkflowService.<OperationName>`), through the
//! same `MultiServiceRouter` the emulator mounts in production. The harness is the one
//! `workflow_execution_lifecycle` already builds.
//!
//! The rule every case here asserts: `UndeprecateDomain` moves the ONE stored domain
//! record it names from `DEPRECATED` back to `REGISTERED` and changes nothing else — the
//! types registered in that domain keep their own `RegistrationStatus`, and the ARN, the
//! retention period, the tags and the executions are left as they were; a name that no
//! stored domain carries is refused with HTTP 400 `UnknownResourceFault` naming the
//! requested domain, and a domain that is currently registered with HTTP 400
//! `DomainAlreadyExistsFault` ("if you undeprecate a domain that is currently
//! registered"), neither of which changes what a later caller sees.
//!
//! The tables below vary what that rule generalises over: the shape of the domain name,
//! the retention period it was registered with, how many tags it carries, which kinds of
//! type it holds and how many (none, one, several names, several versions of one name,
//! workflow types, activity types, both), which domain of several is addressed, how many
//! deprecate/undeprecate cycles the record has been through, whether the domain has a
//! running execution, and the way a name fails to address a stored domain.

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

/// One domain that gets deprecated and then undeprecated. Rows differ in the registered
/// retention period, the shape of the name, how many tags the domain carries, and which
/// types it holds.
struct DomainCase {
    domain: &'static str,
    retention: &'static str,
    tags: usize,
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

async fn describe_domain(emulator: &Emulator, case: &str, domain: &str) -> Value {
    emulator
        .ok(case, "DescribeDomain", &json!({ "name": domain }))
        .await
}

/// Read one type back through its own `Describe*`, whichever kind it is.
async fn describe_type(emulator: &Emulator, case: &str, domain: &str, ty: RegisteredType) -> Value {
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

/// The tags a domain carries, read back through `ListTagsForResource` by the domain's
/// own stored ARN.
async fn list_tags(emulator: &Emulator, case: &str, arn: &str) -> Value {
    emulator
        .ok(case, "ListTagsForResource", &json!({ "resourceArn": arn }))
        .await
}

/// Register `case.domain` with its tags and its types, and assert the domain starts
/// `REGISTERED` — the before half of every case.
async fn seed_domain(emulator: &Emulator, case: &DomainCase) {
    let mut body = register_domain_body(case.domain, case.retention);
    if case.tags > 0 {
        let tags: Vec<Value> = (0..case.tags)
            .map(|index| json!({ "key": format!("k{index}"), "value": format!("v{index}") }))
            .collect();
        merge(&mut body, json!({ "tags": tags }));
    }
    emulator.ok(case.domain, "RegisterDomain", &body).await;

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
}

async fn deprecate(emulator: &Emulator, case: &str, domain: &str) {
    emulator
        .ok(case, "DeprecateDomain", &json!({ "name": domain }))
        .await;
}

/// Every domain the undeprecation table covers.
fn domain_cases() -> Vec<DomainCase> {
    vec![
        // No types and no tags at all: the domain's own status is the whole effect.
        DomainCase {
            domain: "empty-domain",
            retention: "1",
            tags: 0,
            types: Vec::new(),
        },
        DomainCase {
            domain: "one-type-domain",
            retention: "7",
            tags: 1,
            types: vec![workflow(("orders", "1.0"))],
        },
        // Activity types only: the rule is over BOTH registries, not just the workflow
        // one.
        DomainCase {
            domain: "activities-only-domain",
            retention: "3",
            tags: 0,
            types: vec![activity(("pack", "1.0")), activity(("pack", "2.0"))],
        },
        // Several distinct names AND several versions of one name, of both kinds, so the
        // rule is checked over every stored type rather than one per name or per kind.
        DomainCase {
            domain: "many-types.domain_1",
            retention: "90",
            tags: 3,
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
        // retention value has nothing to do with (un)deprecation, which is the point.
        DomainCase {
            domain: "no-retention-domain",
            retention: "NONE",
            tags: 2,
            types: vec![workflow(("audit", "0.1")), activity(("audit", "0.1"))],
        },
    ]
}

#[tokio::test]
async fn undeprecating_a_domain_restores_its_own_record_only() {
    let emulator = Emulator::new();
    let cases = domain_cases();
    for case in &cases {
        seed_domain(&emulator, case).await;
        deprecate(&emulator, case.domain, case.domain).await;
    }

    for (index, case) in cases.iter().enumerate() {
        let before = describe_domain(&emulator, case.domain, case.domain).await;
        assert_eq!(
            modelled_status(case.domain, &before, "/domainInfo/status"),
            "DEPRECATED",
            "case {:?}: the domain is DEPRECATED before the call, got {before:?}",
            case.domain
        );
        let arn = string_at(&before, "/domainInfo/arn");
        let tags_before = list_tags(&emulator, case.domain, &arn).await;

        let undeprecated = emulator
            .ok(
                case.domain,
                "UndeprecateDomain",
                &json!({ "name": case.domain }),
            )
            .await;
        assert_eq!(
            undeprecated,
            json!({}),
            "case {:?}: UndeprecateDomain's output shape is Unit, got {undeprecated:?}",
            case.domain
        );

        let after = describe_domain(&emulator, case.domain, case.domain).await;
        assert_eq!(
            modelled_status(case.domain, &after, "/domainInfo/status"),
            "REGISTERED",
            "case {:?}: the stored domain must report REGISTERED again, got {after:?}",
            case.domain
        );
        // Nothing else about the record moved: the ARN, the retention period and the
        // tags are what registration stored.
        assert_eq!(
            after.pointer("/domainInfo/arn"),
            before.pointer("/domainInfo/arn"),
            "case {:?}: undeprecation must not change the domain ARN, got {after:?}",
            case.domain
        );
        assert_eq!(
            string_at(
                &after,
                "/configuration/workflowExecutionRetentionPeriodInDays"
            ),
            case.retention,
            "case {:?}: undeprecation must not change the retention period, got {after:?}",
            case.domain
        );
        let tags_after = list_tags(&emulator, case.domain, &arn).await;
        assert_eq!(
            tags_after, tags_before,
            "case {:?}: undeprecation must not change the domain's tags, got {tags_after:?}",
            case.domain
        );

        // Every type registered in this domain keeps its own status throughout: a
        // domain's deprecation was never propagated onto them, so its undeprecation is
        // not either.
        for ty in &case.types {
            let described = describe_type(&emulator, case.domain, case.domain, *ty).await;
            assert_eq!(
                modelled_status(case.domain, &described, "/typeInfo/status"),
                "REGISTERED",
                "case {:?}: type {ty:?} keeps its own status when its domain is \
                 undeprecated, got {described:?}",
                case.domain
            );
        }

        // One call addresses ONE record: the domains undeprecated earlier are
        // REGISTERED, and the ones not yet addressed are still DEPRECATED.
        for (other_index, other) in cases.iter().enumerate() {
            let expected = if other_index <= index {
                "REGISTERED"
            } else {
                "DEPRECATED"
            };
            let described = describe_domain(&emulator, other.domain, other.domain).await;
            assert_eq!(
                modelled_status(other.domain, &described, "/domainInfo/status"),
                expected,
                "case {:?}: after undeprecating {:?}, this domain must be {expected}, \
                 got {described:?}",
                other.domain,
                case.domain
            );
        }
    }
}

#[tokio::test]
async fn deprecation_and_undeprecation_round_trip_over_repeated_cycles() {
    let emulator = Emulator::new();
    let cases = domain_cases();
    for case in &cases {
        seed_domain(&emulator, case).await;
    }

    // The invariant: the stored status is decided by the LAST call that moved it, so an
    // arbitrary number of deprecate/undeprecate cycles leaves the record REGISTERED and
    // each half of a cycle is accepted exactly once per state. Three cycles, so no row
    // passes on a single transition.
    for case in &cases {
        for cycle in 1..=3 {
            let cycle_case = format!("{} cycle #{cycle}", case.domain);

            deprecate(&emulator, &cycle_case, case.domain).await;
            let described = describe_domain(&emulator, &cycle_case, case.domain).await;
            assert_eq!(
                modelled_status(&cycle_case, &described, "/domainInfo/status"),
                "DEPRECATED",
                "case {cycle_case:?}: the domain is DEPRECATED after DeprecateDomain, \
                 got {described:?}"
            );

            emulator
                .ok(
                    &cycle_case,
                    "UndeprecateDomain",
                    &json!({ "name": case.domain }),
                )
                .await;
            let described = describe_domain(&emulator, &cycle_case, case.domain).await;
            assert_eq!(
                modelled_status(&cycle_case, &described, "/domainInfo/status"),
                "REGISTERED",
                "case {cycle_case:?}: the domain is REGISTERED after UndeprecateDomain, \
                 got {described:?}"
            );
        }

        // A record back on REGISTERED is deprecable again — the status really moved in
        // the store rather than the undeprecation being a no-op answer.
        deprecate(&emulator, case.domain, case.domain).await;
        emulator
            .refused(
                case.domain,
                "DeprecateDomain",
                &json!({ "name": case.domain }),
                "DomainDeprecatedFault",
            )
            .await;
        emulator
            .ok(
                case.domain,
                "UndeprecateDomain",
                &json!({ "name": case.domain }),
            )
            .await;
    }
}

#[tokio::test]
async fn undeprecating_a_registered_domain_is_refused() {
    let emulator = Emulator::new();
    let cases = domain_cases();
    for case in &cases {
        seed_domain(&emulator, case).await;
    }

    for case in &cases {
        // The state registration left, as the readers report it.
        let before = describe_domain(&emulator, case.domain, case.domain).await;
        let types_before: Vec<Value> = {
            let mut described = Vec::new();
            for ty in &case.types {
                described.push(describe_type(&emulator, case.domain, case.domain, *ty).await);
            }
            described
        };

        // Repeated attempts are each refused the same way; none of them is a silent
        // re-registration. The fault names the requested domain, so a message is checked
        // to carry that name over every name shape the table covers.
        for round in 1..=3 {
            let round_case = format!("{} refusal #{round}", case.domain);
            let (status, body) = emulator
                .call("UndeprecateDomain", &json!({ "name": case.domain }))
                .await;
            let code = error_code(&body);
            assert_eq!(
                (status, code.as_str()),
                (400, "DomainAlreadyExistsFault"),
                "case {round_case:?}: expected 400 DomainAlreadyExistsFault, got {status} \
                 {code} with {body:?}"
            );
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {round_case:?}: expected a message, got {body:?}")
                });
            assert!(
                message.contains(case.domain),
                "case {round_case:?}: the fault message names the requested domain, \
                 got {message:?}"
            );
        }

        let after = describe_domain(&emulator, case.domain, case.domain).await;
        assert_eq!(
            after, before,
            "case {:?}: a refused undeprecation must not change the stored domain, \
             got {after:?}",
            case.domain
        );
        for (ty, expected) in case.types.iter().zip(&types_before) {
            let described = describe_type(&emulator, case.domain, case.domain, *ty).await;
            assert_eq!(
                &described, expected,
                "case {:?}: a refused undeprecation must not change type {ty:?}, \
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
async fn undeprecating_a_domain_that_is_not_stored_is_an_unknown_resource() {
    let emulator = Emulator::new();
    let stored = DomainCase {
        domain: "stored-domain",
        retention: "7",
        tags: 1,
        types: vec![workflow(("orders", "1.0")), activity(("charge", "1.0"))],
    };
    seed_domain(&emulator, &stored).await;
    deprecate(&emulator, stored.domain, stored.domain).await;

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
            .call("UndeprecateDomain", &json!({ "name": case.name }))
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

    // None of those refusals touched the store, so the stored domain is still deprecated
    // and undeprecates exactly once.
    let described = describe_domain(&emulator, "after refusals", stored.domain).await;
    assert_eq!(
        modelled_status("after refusals", &described, "/domainInfo/status"),
        "DEPRECATED",
        "case \"after refusals\": a refused undeprecation must leave the domain \
         DEPRECATED, got {described:?}"
    );
    emulator
        .ok(
            "after refusals",
            "UndeprecateDomain",
            &json!({ "name": stored.domain }),
        )
        .await;
    let described = describe_domain(&emulator, "after refusals", stored.domain).await;
    assert_eq!(
        modelled_status("after refusals", &described, "/domainInfo/status"),
        "REGISTERED",
        "case \"after refusals\": the domain undeprecates after the refusals, \
         got {described:?}"
    );
}

#[tokio::test]
async fn an_undeprecated_domain_takes_new_types_and_executions() {
    let emulator = Emulator::new();
    let case = DomainCase {
        domain: "revived-domain",
        retention: "7",
        tags: 0,
        types: vec![workflow(("orders", "1.0"))],
    };
    seed_domain(&emulator, &case).await;

    // A run started before the deprecation, so the undeprecation is checked not to
    // disturb what was already there.
    let started = emulator
        .ok(
            case.domain,
            "StartWorkflowExecution",
            &json!({
                "domain": case.domain,
                "workflowId": "wf-before",
                "workflowType": { "name": "orders", "version": "1.0" },
            }),
        )
        .await;
    let run_before = string_at(&started, "/runId");

    deprecate(&emulator, case.domain, case.domain).await;
    emulator
        .ok(
            case.domain,
            "UndeprecateDomain",
            &json!({ "name": case.domain }),
        )
        .await;

    // "After a domain has been undeprecated it can be used to create new workflow
    // executions or register new types."
    let mut body = json!({ "domain": case.domain, "name": "returns", "version": "1.0" });
    merge(&mut body, full_defaults("default-tasks", "TERMINATE"));
    emulator
        .ok(case.domain, "RegisterWorkflowType", &body)
        .await;
    let described = describe_type(&emulator, case.domain, case.domain, workflow(("returns", "1.0")))
        .await;
    assert_eq!(
        modelled_status(case.domain, &described, "/typeInfo/status"),
        "REGISTERED",
        "case {:?}: a type registered after undeprecation reads back REGISTERED, \
         got {described:?}",
        case.domain
    );

    let started = emulator
        .ok(
            case.domain,
            "StartWorkflowExecution",
            &json!({
                "domain": case.domain,
                "workflowId": "wf-after",
                "workflowType": { "name": "returns", "version": "1.0" },
            }),
        )
        .await;
    let run_after = string_at(&started, "/runId");
    assert_ne!(
        run_after, run_before,
        "case {:?}: each start mints its own runId, got {run_after:?} twice",
        case.domain
    );

    // Both runs — the one started before the deprecation and the one started after the
    // undeprecation — are OPEN.
    for (workflow_id, run_id) in [("wf-before", &run_before), ("wf-after", &run_after)] {
        let described = emulator
            .ok(
                workflow_id,
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
            "case {workflow_id:?}: the run must still be OPEN, got {described:?}"
        );
    }
}
