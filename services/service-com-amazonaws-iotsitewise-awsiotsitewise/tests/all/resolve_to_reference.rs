// SPDX-License-Identifier: CC0-1.0

//! Cover for the ONE resolved-resource reference every operation that narrows to
//! one resolves through, driven over HTTP through the same `MultiServiceRouter`
//! the emulator boots with.
//!
//! The rule these cases pin: a `(resolveToResourceType, resolveToResourceId)`
//! pair names ONE resolved resource, so a request stating both members narrows to
//! the asset they address, a request stating neither narrows to nothing, a
//! request stating half the pair addresses nothing and is refused as a bad
//! request, and a complete pair addressing no stored asset is refused as an
//! absent resource — identically for every operation that takes the pair.
//!
//! The three operations that take it are `ListActions`, `ListExecutions` and
//! `DescribeComputationModelExecutionSummary`, and each case below is driven
//! through all three, so one shared resolution answers for all of them rather
//! than three copies that can drift.
//!
//! The wire harness (`router_with_engine`, `call`, `error_code`, …) is the one
//! `asset_lifecycle` already defines, the action seeding is `actions`' and the
//! computation-model create is `computation_model_execution_summary`'s, re-used
//! rather than copied.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::ops::types::{ResolveTo, TargetResource};
use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, ExecutionRecord, EXECUTION_STATE_COMPLETED,
};

use crate::actions::{seed_action, Target, ABSENT_ID};
use crate::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_catalog, router_with_engine,
};
use crate::computation_model_execution_summary::create_computation_model;
use crate::hierarchies_etags_and_refusals::error_message;

/// The action that resolves to the fixture's second asset.
const ACTION_RESOLVED: &str = "acacacac-1111-4111-8111-111111111111";

/// The action that resolves to nothing.
const ACTION_UNRESOLVED: &str = "acacacac-2222-4222-8222-222222222222";

/// The execution that resolves to the fixture's second asset.
const EXECUTION_RESOLVED: &str = "ececece1-1111-4111-8111-111111111111";

/// The execution that resolves to nothing.
const EXECUTION_UNRESOLVED: &str = "ececece2-2222-4222-8222-222222222222";

/// The resources the three operations are driven against: two assets, and the
/// computation model the seeded action and execution are recorded against.
struct Fixture {
    router: MultiServiceRouter,
    /// A stored asset nothing in the fixture resolves to.
    asset_a: String,
    /// The stored asset the resolving rows point at.
    asset_b: String,
    /// The target resource both listings are scoped to and the summary is of.
    model: String,
}

/// Build the fixture: two assets and a computation model created through the API,
/// then one resolving and one non-resolving action and execution recorded against
/// the model in the shared store.
///
/// `ExecuteAction` is the only operation that records an action or an execution,
/// and it records one only against a settled model publishing the action it runs,
/// so the rows are recorded directly in the shared store these handlers read —
/// the way the `actions` and `executions` suites record their own tables.
async fn fixture() -> Fixture {
    let (router, engine) = router_with_engine();
    let asset_model = create_model(&router, "Boiler", property_catalog()).await;
    let asset_a = create_asset(&router, "Boiler-A", &asset_model).await;
    let asset_b = create_asset(&router, "Boiler-B", &asset_model).await;
    let model = create_computation_model(&router, "boiler-anomaly").await;

    let anchor = SystemTime::now();
    seed_action(
        &engine,
        ACTION_RESOLVED,
        "Amazon Web Services/ANOMALY_DETECTION_TRAINING",
        Target::ComputationModel,
        &model,
        "{\"case\":\"resolving\"}",
        Some(&asset_b),
        anchor - Duration::from_secs(900),
    );
    seed_action(
        &engine,
        ACTION_UNRESOLVED,
        "Amazon Web Services/ANOMALY_DETECTION_TRAINING",
        Target::ComputationModel,
        &model,
        "{\"case\":\"unresolved\"}",
        None,
        anchor - Duration::from_secs(800),
    );

    {
        let state = service_state(&*engine).expect("the iotsitewise store");
        let mut store = state.lock();
        for (execution_id, resolved) in [
            (EXECUTION_RESOLVED, Some(asset_b.as_str())),
            (EXECUTION_UNRESOLVED, None),
        ] {
            store.executions.insert(
                execution_id.to_string(),
                ExecutionRecord {
                    execution_id: execution_id.to_string(),
                    action_type: Some("Amazon Web Services/ANOMALY_DETECTION_TRAINING".to_string()),
                    target_resource: TargetResource {
                        asset_id: None,
                        computation_model_id: Some(model.clone()),
                    },
                    target_resource_version: "1".to_string(),
                    resolve_to: resolved.map(|asset_id| ResolveTo {
                        asset_id: asset_id.to_string(),
                    }),
                    execution_start_time: anchor - Duration::from_secs(900),
                    // A run already over, so the summary counts it under one
                    // state whatever the clock does while the case runs.
                    settles_at: anchor - Duration::from_secs(600),
                    outcome: EXECUTION_STATE_COMPLETED.to_string(),
                    execution_entity_version: None,
                },
            );
        }
    }

    Fixture {
        router,
        asset_a,
        asset_b,
        model,
    }
}

/// How a request states the resolved-resource reference.
#[derive(Debug, Clone, Copy)]
enum Reference {
    /// Neither member: the request narrows to nothing.
    Unstated,
    /// Both members, addressing the asset the resolving rows point at.
    PairOnResolvedAsset,
    /// Both members, addressing a stored asset no row resolves to.
    PairOnOtherAsset,
    /// Both members, addressing an asset the store does not hold.
    PairOnAbsentAsset,
    /// The type alone.
    TypeOnly,
    /// The id alone, of a stored asset.
    IdOnly,
    /// The id alone, of an asset the store does not hold: a pair stated by
    /// halves addresses nothing, whether or not the half would have resolved.
    AbsentIdOnly,
}

/// What a case expects of each of the three operations.
#[derive(Debug, Clone)]
enum Outcome {
    /// Served: the ids each listing reports, which is also how many executions
    /// the summary counts.
    Served {
        actions: Vec<&'static str>,
        executions: Vec<&'static str>,
    },
    /// Refused, with this status, wire code and message.
    Refused {
        status: u16,
        code: &'static str,
        message: String,
    },
}

/// One row of the case table.
#[derive(Debug, Clone)]
struct Case {
    /// What this row is, named in every assertion.
    case: &'static str,
    reference: Reference,
    outcome: Outcome,
}

/// The cases: one table spanning the forms the reference takes — stated whole,
/// stated not at all, stated by either half alone, and stated whole for a
/// resource the store does not hold — crossed with whether the asset it
/// addresses is one some row resolves to.
fn case_table(absent_asset_message: &str) -> Vec<Case> {
    let stated_by_halves_missing_id =
        "resolveToResourceId is required when resolveToResourceType is specified.".to_string();
    let stated_by_halves_missing_type =
        "resolveToResourceType is required when resolveToResourceId is specified.".to_string();
    vec![
        Case {
            case: "no reference at all",
            reference: Reference::Unstated,
            outcome: Outcome::Served {
                actions: vec![ACTION_RESOLVED, ACTION_UNRESOLVED],
                executions: vec![EXECUTION_RESOLVED, EXECUTION_UNRESOLVED],
            },
        },
        Case {
            case: "the whole pair, on the asset the rows resolve to",
            reference: Reference::PairOnResolvedAsset,
            outcome: Outcome::Served {
                actions: vec![ACTION_RESOLVED],
                executions: vec![EXECUTION_RESOLVED],
            },
        },
        Case {
            case: "the whole pair, on a stored asset no row resolves to",
            reference: Reference::PairOnOtherAsset,
            outcome: Outcome::Served {
                actions: Vec::new(),
                executions: Vec::new(),
            },
        },
        Case {
            case: "the whole pair, on an asset the store does not hold",
            reference: Reference::PairOnAbsentAsset,
            outcome: Outcome::Refused {
                status: 404,
                code: "ResourceNotFoundException",
                message: absent_asset_message.to_string(),
            },
        },
        Case {
            case: "the type alone",
            reference: Reference::TypeOnly,
            outcome: Outcome::Refused {
                status: 400,
                code: "InvalidRequestException",
                message: stated_by_halves_missing_id,
            },
        },
        Case {
            case: "the id alone, of a stored asset",
            reference: Reference::IdOnly,
            outcome: Outcome::Refused {
                status: 400,
                code: "InvalidRequestException",
                message: stated_by_halves_missing_type.clone(),
            },
        },
        Case {
            case: "the id alone, of an asset the store does not hold",
            reference: Reference::AbsentIdOnly,
            outcome: Outcome::Refused {
                status: 400,
                code: "InvalidRequestException",
                message: stated_by_halves_missing_type,
            },
        },
    ]
}

/// The query members a case states the reference with, empty when it states none.
fn reference_query(fixture: &Fixture, reference: Reference) -> String {
    let pair = |asset_id: &str| format!("resolveToResourceType=ASSET&resolveToResourceId={asset_id}");
    match reference {
        Reference::Unstated => String::new(),
        Reference::PairOnResolvedAsset => pair(&fixture.asset_b),
        Reference::PairOnOtherAsset => pair(&fixture.asset_a),
        Reference::PairOnAbsentAsset => pair(ABSENT_ID),
        Reference::TypeOnly => "resolveToResourceType=ASSET".to_string(),
        Reference::IdOnly => format!("resolveToResourceId={}", fixture.asset_b),
        Reference::AbsentIdOnly => format!("resolveToResourceId={ABSENT_ID}"),
    }
}

/// The three operations the reference is resolved for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    ListActions,
    ListExecutions,
    DescribeComputationModelExecutionSummary,
}

impl Operation {
    /// The name every assertion about this operation carries.
    fn label(self) -> &'static str {
        match self {
            Operation::ListActions => "ListActions",
            Operation::ListExecutions => "ListExecutions",
            Operation::DescribeComputationModelExecutionSummary => {
                "DescribeComputationModelExecutionSummary"
            }
        }
    }
}

/// Dispatch `operation` against the fixture's target resource, with the members
/// `reference` states appended.
async fn dispatch(fixture: &Fixture, operation: Operation, reference: Reference) -> (u16, Value) {
    let filter = reference_query(fixture, reference);
    let model = &fixture.model;
    let uri = match operation {
        Operation::ListActions | Operation::ListExecutions => {
            let collection = if operation == Operation::ListActions {
                "actions"
            } else {
                "executions"
            };
            let scope = format!("targetResourceType=COMPUTATION_MODEL&targetResourceId={model}");
            if filter.is_empty() {
                format!("/{collection}?{scope}")
            } else {
                format!("/{collection}?{scope}&{filter}")
            }
        }
        Operation::DescribeComputationModelExecutionSummary => {
            if filter.is_empty() {
                format!("/computation-models/{model}/execution-summary")
            } else {
                format!("/computation-models/{model}/execution-summary?{filter}")
            }
        }
    };
    call(&fixture.router, "GET", &uri, None).await
}

/// The ids a listing body reports under `list`, keyed by `id_member`, sorted so a
/// case names a SET rather than this listing's own order, which its own suite
/// pins.
fn reported_ids(body: &Value, list: &str, id_member: &str) -> Vec<String> {
    let mut ids: Vec<String> = body[list]
        .as_array()
        .unwrap_or_else(|| panic!("expected a {list} list, got {body}"))
        .iter()
        .map(|summary| {
            summary[id_member]
                .as_str()
                .unwrap_or_else(|| panic!("a summary with no {id_member} in {body}"))
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

/// How many executions a summary body counts, across every state it reports.
fn counted_executions(body: &Value) -> u64 {
    body["computationModelExecutionSummary"]
        .as_object()
        .unwrap_or_else(|| panic!("expected a computationModelExecutionSummary map, got {body}"))
        .values()
        .map(|count| {
            count
                .as_str()
                .unwrap_or_else(|| panic!("a non-string summary value in {body}"))
                .parse::<u64>()
                .unwrap_or_else(|_| panic!("a non-decimal summary value in {body}"))
        })
        .sum()
}

/// `expected`, sorted, as the owned strings a reported-id list is compared with.
fn sorted_ids(expected: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = expected.iter().map(|id| id.to_string()).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn every_operation_resolves_the_resolved_resource_reference_the_same_way() {
    let fixture = fixture().await;
    let cases = case_table(&format!("Asset with ID {ABSENT_ID} not found"));

    for row in &cases {
        let case = row.case;
        for operation in [
            Operation::ListActions,
            Operation::ListExecutions,
            Operation::DescribeComputationModelExecutionSummary,
        ] {
            let label = operation.label();
            let (status, body) = dispatch(&fixture, operation, row.reference).await;
            match &row.outcome {
                Outcome::Served {
                    actions,
                    executions,
                } => {
                    assert_eq!(
                        status, 200,
                        "case {case:?} on {label}: expected 200, got {status} {body}"
                    );
                    match operation {
                        Operation::ListActions => assert_eq!(
                            reported_ids(&body, "actionSummaries", "actionId"),
                            sorted_ids(actions),
                            "case {case:?} on {label}: unexpected actions in {body}"
                        ),
                        Operation::ListExecutions => assert_eq!(
                            reported_ids(&body, "executionSummaries", "executionId"),
                            sorted_ids(executions),
                            "case {case:?} on {label}: unexpected executions in {body}"
                        ),
                        Operation::DescribeComputationModelExecutionSummary => {
                            let counted = counted_executions(&body);
                            assert_eq!(
                                counted,
                                executions.len() as u64,
                                "case {case:?} on {label}: expected {} counted executions, got \
                                 {counted} in {body}",
                                executions.len()
                            );
                            // The summary also ECHOES the resource it was
                            // narrowed to, which only a request that named one
                            // has — the same reference, reported back.
                            let narrowed_to = match row.reference {
                                Reference::PairOnResolvedAsset => Some(fixture.asset_b.as_str()),
                                Reference::PairOnOtherAsset => Some(fixture.asset_a.as_str()),
                                _ => None,
                            };
                            assert_eq!(
                                body["resolveTo"]["assetId"].as_str(),
                                narrowed_to,
                                "case {case:?} on {label}: unexpected resolveTo in {body}"
                            );
                        }
                    }
                }
                Outcome::Refused {
                    status: expected_status,
                    code,
                    message,
                } => {
                    assert_eq!(
                        status, *expected_status,
                        "case {case:?} on {label}: expected {expected_status}, got {status} {body}"
                    );
                    let reported_code = error_code(&body);
                    assert!(
                        reported_code.ends_with(code),
                        "case {case:?} on {label}: expected {code}, got {reported_code:?} in {body}"
                    );
                    assert_eq!(
                        error_message(&body),
                        message.as_str(),
                        "case {case:?} on {label}: unexpected refusal message in {body}"
                    );
                }
            }
        }
    }
}
