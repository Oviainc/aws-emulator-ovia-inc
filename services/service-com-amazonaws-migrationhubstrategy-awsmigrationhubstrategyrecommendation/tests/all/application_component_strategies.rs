// SPDX-License-Identifier: CC0-1.0

//! `GetApplicationComponentStrategies`: the strategies the service holds for one
//! application component of the account's portfolio, read end to end over HTTP.
//!
//! The rule under test is the one the operation states for ANY application component, not
//! for one request: the answer is computed from the stored component record the
//! `applicationComponentId` names — the strategy option the account prefers (reported
//! first, as the recommendation it selects, `isPreferred` being the flag that choice
//! carried and true when it carried none) and the service's own recommendation when that
//! is a DIFFERENT strategy (carrying the `recommended` status and preferred by nobody) —
//! so a preference naming the strategy and destination the service recommends is ONE
//! strategy carrying both, and a component nothing recommended and nobody chose a
//! preference for reports NO strategy rather than a canned list. An
//! `applicationComponentId` naming no component is refused with 404
//! `ResourceNotFoundException`, and the read changes nothing a later caller sees.
//!
//! The table below therefore spans the class the rule generalises over rather than one
//! component: one row per `TargetDestination` the model declares, cycling independently
//! through `Strategy`, `TransformationToolName` and the three ways a stored choice can
//! carry (or omit) its preferred flag, crossed with the four ways a record can hold a
//! recommendation and a preference (one, the other, both differing, both the same) — plus
//! rows holding only a strategy, only a destination, only a tool, and nothing at all.
//!
//! An application component enters this service only through the account's data being
//! brought in (the import loop, filed separately), so the rows are seeded through the
//! shared engine with the harness `application_component_details` uses — the same
//! `application_components` map an ingestion writes and this operation reads — and every
//! assertion is driven through the router, so the handler, its refusal and the serialized
//! `ApplicationComponentStrategy` list are exercised exactly as a client reaches them. One
//! case goes the whole way round instead: it stores the preference through
//! `UpdateApplicationComponentConfig` over HTTP and reads it back here.

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::StrategyOption;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::ApplicationComponentRecord;
use smithy_server_runtime::MultiServiceRouter;

use crate::application_component_details::{emulator_with, get_component};
use crate::import_and_assess::{at, call, Answer};

/// This service's message for an id that resolves to no resource. Spelled out here rather
/// than imported from the crate, so a wrong message fails this suite instead of agreeing
/// with itself.
const INVALID_RESOURCE_ID: &str = "Invalid resourceId: ";

/// `StrategyRecommendation`'s value for a strategy the service itself recommends, from
/// `smithy_query(query="shape",shape="StrategyRecommendation")` — the one of its four
/// values that ranks a strategy as recommended rather than merely considered.
const RECOMMENDED: &str = "recommended";

/// Every value `Strategy` declares, from `smithy_query(query="shape",shape="Strategy")`.
const STRATEGIES: &[&str] = &[
    "Rehost",
    "Retirement",
    "Refactor",
    "Replatform",
    "Retain",
    "Relocate",
    "Repurchase",
];

/// Every value `TargetDestination` declares, from
/// `smithy_query(query="shape",shape="TargetDestination")`.
const TARGET_DESTINATIONS: &[&str] = &[
    "None specified",
    "AWS Elastic BeanStalk",
    "AWS Fargate",
    "Amazon Elastic Cloud Compute (EC2)",
    "Amazon Elastic Container Service (ECS)",
    "Amazon Elastic Kubernetes Service (EKS)",
    "Aurora MySQL",
    "Aurora PostgreSQL",
    "Amazon Relational Database Service on MySQL",
    "Amazon Relational Database Service on PostgreSQL",
    "Amazon DocumentDB",
    "Amazon DynamoDB",
    "Amazon Relational Database Service",
    "Babelfish for Aurora PostgreSQL",
];

/// Every value `TransformationToolName` declares, from
/// `smithy_query(query="shape",shape="TransformationToolName")`.
const TOOL_NAMES: &[&str] = &[
    "App2Container",
    "Porting Assistant For .NET",
    "End of Support Migration",
    "Windows Web Application Migration Assistant",
    "Application Migration Service",
    "Strategy Recommendation Support",
    "In Place Operating System Upgrade",
    "Schema Conversion Tool",
    "Database Migration Service",
    "Native SQL Server Backup/Restore",
];

/// The three ways a stored choice can carry its preferred flag: set either way, or absent.
const PREFERRED_FLAGS: &[Option<bool>] = &[None, Some(true), Some(false)];

/// The id row `row` is seeded under: a `ResourceId` (`^[0-9a-b]+`, at most 44 characters)
/// like the ones an ingestion mints, which is also what `ApplicationComponentId`
/// (`[0-9a-zA-Z-]+`, at most 44 characters) admits.
fn id_for(row: usize) -> String {
    format!("{row:03}ab")
}

/// One `RecommendationSet` as a row expects to read it back: a strategy, a destination and
/// a transformation tool, each present only where the record holds it.
#[derive(Clone, Copy)]
struct Recommendation {
    strategy: Option<&'static str>,
    destination: Option<&'static str>,
    tool: Option<&'static str>,
}

/// A strategy option the account chose for a component — the `StrategyOption` an update
/// stores, with the flag it carried.
#[derive(Clone, Copy)]
struct Preference {
    strategy: Option<&'static str>,
    destination: Option<&'static str>,
    tool: Option<&'static str>,
    is_preferred: Option<bool>,
}

/// One row: what the component's record holds, and so what the read must report.
struct Case {
    what: String,
    /// `RecommendationSet$strategy` as the record holds it — the service's own
    /// recommendation for this component.
    strategy: Option<&'static str>,
    /// `RecommendationSet$targetDestination` as the record holds it.
    destination: Option<&'static str>,
    /// The option the account prefers, or `None` for a component nobody chose one for.
    preference: Option<Preference>,
}

impl Case {
    /// The component record this row seeds: the two recommendation members and the stored
    /// preference, and nothing else that could stand in for them.
    fn record(&self, id: &str) -> ApplicationComponentRecord {
        ApplicationComponentRecord {
            id: id.to_string(),
            name: Some(format!("component-{id}")),
            strategy: self.strategy.map(str::to_string),
            target_destination: self.destination.map(str::to_string),
            preferred_strategy: self.preference.map(|preference| StrategyOption {
                strategy: preference.strategy.map(str::to_string),
                tool_name: preference.tool.map(str::to_string),
                target_destination: preference.destination.map(str::to_string),
                is_preferred: preference.is_preferred,
            }),
            ..Default::default()
        }
    }

    /// The service's own recommendation for this component, which it holds only when it
    /// holds a strategy or a destination for it. It carries no transformation tool:
    /// nothing in this emulator picks one.
    fn analyzed(&self) -> Option<Recommendation> {
        (self.strategy.is_some() || self.destination.is_some()).then_some(Recommendation {
            strategy: self.strategy,
            destination: self.destination,
            tool: None,
        })
    }

    /// The recommendation the stored preference selects: the same three things under the
    /// option's own names.
    fn preferred(&self) -> Option<Recommendation> {
        self.preference.map(|preference| Recommendation {
            strategy: preference.strategy,
            destination: preference.destination,
            tool: preference.tool,
        })
    }

    /// Whether the account chose the very strategy the service recommends — the same
    /// strategy and the same destination — so the two are one strategy rather than two.
    fn preference_is_recommended(&self) -> bool {
        match (self.analyzed(), self.preferred()) {
            (Some(analyzed), Some(preferred)) => {
                analyzed.strategy == preferred.strategy
                    && analyzed.destination == preferred.destination
            }
            _ => false,
        }
    }

    /// The strategies this row must read back, in order: each entry as the pointers (under
    /// that entry) and values the answer must carry at them.
    ///
    /// This restates the rule rather than calling into the crate, so a handler that
    /// reports a canned list, drops the preference or merges two different strategies
    /// fails here instead of agreeing with itself.
    fn expected(&self) -> Vec<Vec<(String, Value)>> {
        let mut entries: Vec<Vec<(String, Value)>> = Vec::new();
        if let (Some(preference), Some(selected)) = (self.preference, self.preferred()) {
            let status = if self.preference_is_recommended() {
                json!(RECOMMENDED)
            } else {
                // Nothing recommended the account's own option, so it carries no
                // StrategyRecommendation status.
                Value::Null
            };
            entries.push(entry_members(
                &selected,
                status,
                preference.is_preferred.unwrap_or(true),
            ));
        }
        if let Some(analyzed) = self
            .analyzed()
            .filter(|_| !self.preference_is_recommended())
        {
            entries.push(entry_members(&analyzed, json!(RECOMMENDED), false));
        }
        entries
    }
}

/// The pointers, relative to one reported strategy, at which that strategy must carry its
/// recommendation, its status and its preferred flag.
fn entry_members(
    recommendation: &Recommendation,
    status: Value,
    is_preferred: bool,
) -> Vec<(String, Value)> {
    let text = |value: Option<&str>| value.map_or(Value::Null, |value| json!(value));
    let mut members = vec![
        (
            "recommendation/strategy".to_string(),
            text(recommendation.strategy),
        ),
        (
            "recommendation/targetDestination".to_string(),
            text(recommendation.destination),
        ),
        ("status".to_string(), status),
        ("isPreferred".to_string(), json!(is_preferred)),
    ];
    match recommendation.tool {
        Some(tool) => {
            members.push((
                "recommendation/transformationTool/name".to_string(),
                json!(tool),
            ));
            // The caller named a tool, not a tool catalog entry, so the tool carries
            // neither a description nor an installation link.
            members.push((
                "recommendation/transformationTool/description".to_string(),
                Value::Null,
            ));
            members.push((
                "recommendation/transformationTool/tranformationToolInstallationLink".to_string(),
                Value::Null,
            ));
        }
        None => members.push((
            "recommendation/transformationTool".to_string(),
            Value::Null,
        )),
    }
    members
}

/// `GET /get-applicationcomponent-strategies/{applicationComponentId}`.
async fn get_strategies(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-applicationcomponent-strategies/{id}"),
        None,
    )
    .await
}

/// `POST /update-applicationcomponent-config/`.
async fn update(router: &MultiServiceRouter, request: Value) -> Answer {
    call(
        router,
        "POST",
        "/update-applicationcomponent-config/",
        Some(request),
    )
    .await
}

/// How many strategies a read reported. A missing member counts as none, so an empty list
/// and an omitted one are not distinguished: the count is what the rule is about.
fn reported_count(answer: &Answer) -> usize {
    match at(&answer.body, "/applicationComponentStrategies") {
        Value::Array(items) => items.len(),
        _ => 0,
    }
}

/// The rows: one per `TargetDestination`, each crossed with the four ways a record can
/// hold a recommendation and a preference, then the partial records and the empty one.
fn table() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for (index, destination) in TARGET_DESTINATIONS.iter().enumerate() {
        let strategy = STRATEGIES[index % STRATEGIES.len()];
        let tool = TOOL_NAMES[index % TOOL_NAMES.len()];
        let flag = PREFERRED_FLAGS[index % PREFERRED_FLAGS.len()];
        let other_strategy = STRATEGIES[(index + 1) % STRATEGIES.len()];
        let other_destination = TARGET_DESTINATIONS[(index + 1) % TARGET_DESTINATIONS.len()];

        cases.push(Case {
            what: format!("the service recommends {strategy} / {destination}, nobody chose"),
            strategy: Some(strategy),
            destination: Some(destination),
            preference: None,
        });
        cases.push(Case {
            what: format!(
                "the account chose {strategy} / {destination} / {tool} (isPreferred \
                 {flag:?}), nothing recommended"
            ),
            strategy: None,
            destination: None,
            preference: Some(Preference {
                strategy: Some(strategy),
                destination: Some(destination),
                tool: Some(tool),
                is_preferred: flag,
            }),
        });
        cases.push(Case {
            what: format!(
                "the service recommends {strategy} / {destination} and the account chose \
                 {other_strategy} / {other_destination} (isPreferred {flag:?})"
            ),
            strategy: Some(strategy),
            destination: Some(destination),
            preference: Some(Preference {
                strategy: Some(other_strategy),
                destination: Some(other_destination),
                tool: Some(tool),
                is_preferred: flag,
            }),
        });
        cases.push(Case {
            what: format!(
                "the account chose the recommended {strategy} / {destination} (isPreferred \
                 {flag:?})"
            ),
            strategy: Some(strategy),
            destination: Some(destination),
            preference: Some(Preference {
                strategy: Some(strategy),
                destination: Some(destination),
                tool: (index % 2 == 0).then_some(tool),
                is_preferred: flag,
            }),
        });
    }

    cases.push(Case {
        what: "a recommendation naming a strategy and no destination".to_string(),
        strategy: Some("Relocate"),
        destination: None,
        preference: None,
    });
    cases.push(Case {
        what: "a recommendation naming a destination and no strategy".to_string(),
        strategy: None,
        destination: Some("Amazon DocumentDB"),
        preference: None,
    });
    cases.push(Case {
        what: "a choice naming a strategy only".to_string(),
        strategy: None,
        destination: None,
        preference: Some(Preference {
            strategy: Some("Retirement"),
            destination: None,
            tool: None,
            is_preferred: Some(true),
        }),
    });
    cases.push(Case {
        what: "a choice naming a destination only".to_string(),
        strategy: None,
        destination: None,
        preference: Some(Preference {
            strategy: None,
            destination: Some("Babelfish for Aurora PostgreSQL"),
            tool: None,
            is_preferred: None,
        }),
    });
    cases.push(Case {
        what: "a choice naming a tool only, beside a recommendation".to_string(),
        strategy: Some("Refactor"),
        destination: Some("AWS Fargate"),
        preference: Some(Preference {
            strategy: None,
            destination: None,
            tool: Some("Schema Conversion Tool"),
            is_preferred: Some(false),
        }),
    });
    cases.push(Case {
        what: "a component nothing recommended and nobody chose for".to_string(),
        strategy: None,
        destination: None,
        preference: None,
    });
    cases
}

/// Assert the read reported exactly the strategies `case` holds, and nothing beyond them.
fn assert_strategies(answer: &Answer, case: &Case) {
    let what = &case.what;
    assert_eq!(
        answer.status, 200,
        "case {what:?}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    let expected = case.expected();
    assert_eq!(
        reported_count(answer),
        expected.len(),
        "case {what:?}: expected {} strategies, got {:?}",
        expected.len(),
        answer.body
    );
    for (index, members) in expected.iter().enumerate() {
        for (member, value) in members {
            let pointer = format!("/applicationComponentStrategies/{index}/{member}");
            let actual = at(&answer.body, &pointer);
            assert_eq!(
                &actual, value,
                "case {what:?}: expected {pointer} to be {value:?}, got {actual:?} in {:?}",
                answer.body
            );
        }
    }
    let beyond = format!("/applicationComponentStrategies/{}", expected.len());
    assert_eq!(
        at(&answer.body, &beyond),
        Value::Null,
        "case {what:?}: expected no strategy at {beyond}, got {:?}",
        answer.body
    );
}

/// Every row is reported from its own component's record: the preference first, the
/// service's own recommendation when it is a different strategy, both as one entry when
/// the account chose the recommended one, and no strategy at all for a component holding
/// neither.
///
/// Every row is seeded before any of them is read, so a row that reported another row's
/// strategies fails here.
#[tokio::test]
async fn the_strategies_are_the_stored_recommendation_and_the_stored_preference() {
    let cases = table();

    // The table really does span each catalog it draws on, so no value is covered by
    // accident.
    for (catalog, values, seen) in [
        (
            "Strategy",
            STRATEGIES,
            cases
                .iter()
                .flat_map(|case| {
                    [
                        case.strategy,
                        case.preference.and_then(|preference| preference.strategy),
                    ]
                })
                .flatten()
                .collect::<Vec<_>>(),
        ),
        (
            "TargetDestination",
            TARGET_DESTINATIONS,
            cases
                .iter()
                .flat_map(|case| {
                    [
                        case.destination,
                        case.preference.and_then(|preference| preference.destination),
                    ]
                })
                .flatten()
                .collect::<Vec<_>>(),
        ),
        (
            "TransformationToolName",
            TOOL_NAMES,
            cases
                .iter()
                .filter_map(|case| case.preference.and_then(|preference| preference.tool))
                .collect::<Vec<_>>(),
        ),
    ] {
        for value in values {
            assert!(
                seen.contains(value),
                "the table must cover every {catalog} value, and does not carry {value:?}"
            );
        }
    }

    let rows: Vec<(String, &Case)> = cases
        .iter()
        .enumerate()
        .map(|(row, case)| (id_for(row), case))
        .collect();
    let router = emulator_with(
        rows.iter()
            .map(|(id, case)| case.record(id))
            .collect::<Vec<_>>(),
    );

    for (id, case) in &rows {
        let answer = get_strategies(&router, id).await;
        assert_strategies(&answer, case);
    }
}

/// An `applicationComponentId` that names no component of this account's portfolio is
/// refused with the modelled 404 rather than answered with an empty strategy list —
/// whatever the id looks like, and whether or not the store holds other components.
#[tokio::test]
async fn an_id_naming_no_component_is_refused() {
    let seeded: Vec<ApplicationComponentRecord> = table()
        .iter()
        .enumerate()
        .map(|(row, case)| case.record(&id_for(row)))
        .collect();
    for (fixture, router) in [
        ("a store holding every row", emulator_with(seeded)),
        ("an account holding no component", emulator_with(Vec::new())),
    ] {
        for (what, id) in [
            ("an id no record was seeded under", "9999a"),
            ("a one-character id", "b"),
            (
                "an id ApplicationComponentId admits that no ResourceId could be",
                "App-Component-01",
            ),
            ("an id at the 44-character maximum", &"0".repeat(44)[..]),
        ] {
            let answer = get_strategies(&router, id).await;
            assert_eq!(
                answer.status, 404,
                "case {fixture:?}/{what:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {fixture:?}/{what:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                INVALID_RESOURCE_ID,
                "case {fixture:?}/{what:?}: expected the service's {INVALID_RESOURCE_ID:?}, got \
                 {:?}",
                answer.body
            );
        }
    }
}

/// The read changes nothing a later caller sees: the same request answers the same thing
/// however many times it is made, reading one component does not disturb the next, reading
/// a component that holds no strategy does not bring one into being, and the component's
/// detail read is untouched by it.
#[tokio::test]
async fn reading_the_strategies_changes_nothing() {
    let cases = table();
    let rows: Vec<(String, &Case)> = cases
        .iter()
        .enumerate()
        .map(|(row, case)| (id_for(row), case))
        .collect();
    let router = emulator_with(
        rows.iter()
            .map(|(id, case)| case.record(id))
            .collect::<Vec<_>>(),
    );
    let unknown = "0b0b";

    for (id, case) in &rows {
        let what = &case.what;
        let first = get_strategies(&router, id).await;
        let detail_before = get_component(&router, id).await;
        let missing = get_strategies(&router, unknown).await;
        let again = get_strategies(&router, id).await;
        let detail_after = get_component(&router, id).await;

        assert_eq!(
            first.body, again.body,
            "case {what:?}: expected two identical reads to answer the same thing, got {:?} then \
             {:?}",
            first.body, again.body
        );
        assert_eq!(
            detail_before.body, detail_after.body,
            "case {what:?}: expected the detail read to be untouched by the strategies read, got \
             {:?} then {:?}",
            detail_before.body, detail_after.body
        );
        assert_eq!(
            missing.status, 404,
            "case {what:?}: expected a read of {unknown:?} to stay a 404, got {} with {:?}",
            missing.status, missing.body
        );
        assert_strategies(&again, case);
    }
}

/// The whole way round: a preference stored through `UpdateApplicationComponentConfig`
/// over HTTP is what this read reports afterwards, beside the service's own recommendation
/// — for each way the option can name its members.
#[tokio::test]
async fn a_preference_stored_through_the_update_is_what_the_read_reports() {
    for (row, flag) in PREFERRED_FLAGS.iter().enumerate() {
        let id = id_for(row);
        let recommended = Case {
            what: format!("a component recommended before the update (isPreferred {flag:?})"),
            strategy: Some(STRATEGIES[row % STRATEGIES.len()]),
            destination: Some(TARGET_DESTINATIONS[row % TARGET_DESTINATIONS.len()]),
            preference: None,
        };
        let router = emulator_with(vec![recommended.record(&id)]);

        let before = get_strategies(&router, &id).await;
        assert_strategies(&before, &recommended);

        let chosen = Preference {
            strategy: Some(STRATEGIES[(row + 2) % STRATEGIES.len()]),
            destination: Some(TARGET_DESTINATIONS[(row + 2) % TARGET_DESTINATIONS.len()]),
            tool: Some(TOOL_NAMES[row % TOOL_NAMES.len()]),
            is_preferred: *flag,
        };
        let mut option = serde_json::Map::new();
        option.insert("strategy".to_string(), json!(chosen.strategy));
        option.insert(
            "targetDestination".to_string(),
            json!(chosen.destination),
        );
        option.insert("toolName".to_string(), json!(chosen.tool));
        if let Some(is_preferred) = chosen.is_preferred {
            option.insert("isPreferred".to_string(), json!(is_preferred));
        }
        let stored = update(
            &router,
            json!({
                "applicationComponentId": id,
                "strategyOption": Value::Object(option),
            }),
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "case {:?}: expected the update to be accepted, got {} with {:?}",
            recommended.what, stored.status, stored.body
        );

        let after = get_strategies(&router, &id).await;
        assert_strategies(
            &after,
            &Case {
                what: format!("the stored choice read back (isPreferred {flag:?})"),
                strategy: recommended.strategy,
                destination: recommended.destination,
                preference: Some(chosen),
            },
        );
    }
}
