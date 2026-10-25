// SPDX-License-Identifier: CC0-1.0

//! `GetServerStrategies` over HTTP: the strategies it reports for one server of the
//! portfolio, as a function of what that server's record holds.
//!
//! The rule under test is the one the operation states for ANY server, not for one
//! request: the answer is the strategies the record itself holds — the strategy option the
//! account stored as its preference for the server, then the recommendation the service
//! holds for it when that is a different strategy — each reported with whether it is the
//! preferred one, and with the recommended status exactly for the one the service
//! recommends; a server holding neither reports an empty list; and the read computes all
//! of that without writing anything, so a later caller sees what it saw.
//!
//! The table below therefore spans the class the rule generalises over — the record
//! holding no recommendation, a strategy only, a destination only, or both; no preference,
//! a preference naming one member, three members, the same strategy and destination as the
//! recommendation, or another one; an explicit `isPreferred` of either value — rather than
//! the one server shape an issue cites. Its rows differ in how many strategies come back
//! (none, one, two) and in their order, which is what a case keyed to a single stored
//! shape would get wrong.
//!
//! A server enters this service only through an import file task's ingestion (filed
//! separately as the import-loop issue), so the records are seeded through the shared
//! engine with the harness `server_inventory` uses — the same `servers` map that ingestion
//! writes, carrying the preference `UpdateServerConfig` stores there — and every assertion
//! is then driven through the router, so the handler, its refusals and the serialized
//! shapes are exercised exactly as a client reaches them.

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::StrategyOption;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::ServerRecord;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, Answer};
use crate::server_inventory::{emulator_with, server};

/// Every value `StrategyRecommendation` admits, from
/// `smithy_query(query="shape",shape="StrategyRecommendation")` — the catalog
/// `ServerStrategy$status` draws on, so a reported status outside it fails here.
const STRATEGY_RECOMMENDATION_VALUES: &[&str] =
    &["recommended", "viableOption", "notRecommended", "potential"];

/// The one of those values this emulator reports: the status of a strategy the service
/// itself recommends. The other three rank strategies, which nothing here does.
const RECOMMENDED: &str = "recommended";

/// Every value `Strategy` admits, from `smithy_query(query="shape",shape="Strategy")`,
/// so the values the table names can be checked to be declared ones.
const STRATEGIES: &[&str] = &[
    "Refactor",
    "Rehost",
    "Relocate",
    "Replatform",
    "Repurchase",
    "Retain",
    "Retirement",
];

/// Every value `TargetDestination` admits, from
/// `smithy_query(query="shape",shape="TargetDestination")`.
const TARGET_DESTINATIONS: &[&str] = &[
    "AWS Elastic BeanStalk",
    "AWS Fargate",
    "Amazon DocumentDB",
    "Amazon DynamoDB",
    "Amazon Elastic Cloud Compute (EC2)",
    "Amazon Elastic Container Service (ECS)",
    "Amazon Elastic Kubernetes Service (EKS)",
    "Amazon Relational Database Service",
    "Amazon Relational Database Service on MySQL",
    "Amazon Relational Database Service on PostgreSQL",
    "Aurora MySQL",
    "Aurora PostgreSQL",
    "Babelfish for Aurora PostgreSQL",
    "None specified",
];

/// Every value `TransformationToolName` admits, from
/// `smithy_query(query="shape",shape="TransformationToolName")`.
const TOOL_NAMES: &[&str] = &[
    "App2Container",
    "Application Migration Service",
    "Database Migration Service",
    "End of Support Migration",
    "In Place Operating System Upgrade",
    "Native SQL Server Backup/Restore",
    "Porting Assistant For .NET",
    "Schema Conversion Tool",
    "Strategy Recommendation Support",
    "Windows Web Application Migration Assistant",
];

/// The published bounds of `serverId`: `@length(min: 1, max: 27)`
/// (`smithy_query(query="operation",operation="GetServerStrategies")` -> `serverId`),
/// spelled out independently of any crate constant.
const SERVER_ID_MIN: usize = 1;
const SERVER_ID_MAX: usize = 27;

/// The id of the `index`th server of the table's fixture: a `ResourceId` (`^[0-9a-b]+`)
/// short enough to also be a `ServerId` (at most 27 characters).
fn id_for(index: usize) -> String {
    format!("{index:02}a")
}

/// One entry the read has to report, member for member.
#[derive(Debug, Clone)]
struct Expected {
    strategy: Option<&'static str>,
    destination: Option<&'static str>,
    tool: Option<&'static str>,
    /// `ServerStrategy$status`, absent for a strategy nothing recommended.
    status: Option<&'static str>,
    is_preferred: bool,
}

/// One server of the fixture: what its record holds, and the strategies the read must then
/// report for it, in order.
struct Case {
    what: &'static str,
    /// `RecommendationSet$strategy` the service holds for the server.
    recommended_strategy: Option<&'static str>,
    /// `RecommendationSet$targetDestination` the service holds for the server.
    recommended_destination: Option<&'static str>,
    /// The `StrategyOption` the account stored as its preference, as
    /// `UpdateServerConfig` persists it.
    preference: Option<StrategyOption>,
    expected: Vec<Expected>,
}

/// A stored preference naming only the members it is given.
fn preference(
    strategy: Option<&str>,
    destination: Option<&str>,
    tool: Option<&str>,
    is_preferred: Option<bool>,
) -> StrategyOption {
    StrategyOption {
        strategy: strategy.map(str::to_string),
        tool_name: tool.map(str::to_string),
        target_destination: destination.map(str::to_string),
        is_preferred,
    }
}

/// Every row of the fixture: the record shapes the rule has to hold over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "a server nothing recommended and nobody chose a preference for",
            recommended_strategy: None,
            recommended_destination: None,
            preference: None,
            expected: Vec::new(),
        },
        Case {
            what: "a recommendation of a strategy alone, no preference",
            recommended_strategy: Some("Repurchase"),
            recommended_destination: None,
            preference: None,
            expected: vec![Expected {
                strategy: Some("Repurchase"),
                destination: None,
                tool: None,
                status: Some(RECOMMENDED),
                is_preferred: false,
            }],
        },
        Case {
            what: "a recommendation of a destination alone, no preference",
            recommended_strategy: None,
            recommended_destination: Some("Babelfish for Aurora PostgreSQL"),
            preference: None,
            expected: vec![Expected {
                strategy: None,
                destination: Some("Babelfish for Aurora PostgreSQL"),
                tool: None,
                status: Some(RECOMMENDED),
                is_preferred: false,
            }],
        },
        Case {
            what: "a recommendation of a strategy and a destination, no preference",
            recommended_strategy: Some("Relocate"),
            recommended_destination: Some("Amazon Elastic Cloud Compute (EC2)"),
            preference: None,
            expected: vec![Expected {
                strategy: Some("Relocate"),
                destination: Some("Amazon Elastic Cloud Compute (EC2)"),
                tool: None,
                status: Some(RECOMMENDED),
                is_preferred: false,
            }],
        },
        Case {
            what: "a preference naming three members, nothing recommended",
            recommended_strategy: None,
            recommended_destination: None,
            preference: Some(preference(
                Some("Replatform"),
                Some("Amazon Elastic Kubernetes Service (EKS)"),
                Some("End of Support Migration"),
                None,
            )),
            expected: vec![Expected {
                strategy: Some("Replatform"),
                destination: Some("Amazon Elastic Kubernetes Service (EKS)"),
                tool: Some("End of Support Migration"),
                status: None,
                is_preferred: true,
            }],
        },
        Case {
            what: "a preference naming a tool alone, nothing recommended",
            recommended_strategy: None,
            recommended_destination: None,
            preference: Some(preference(
                None,
                None,
                Some("In Place Operating System Upgrade"),
                None,
            )),
            expected: vec![Expected {
                strategy: None,
                destination: None,
                tool: Some("In Place Operating System Upgrade"),
                status: None,
                is_preferred: true,
            }],
        },
        Case {
            what: "a preference naming the strategy and destination the service recommends",
            recommended_strategy: Some("Rehost"),
            recommended_destination: Some("Amazon Elastic Cloud Compute (EC2)"),
            preference: Some(preference(
                Some("Rehost"),
                Some("Amazon Elastic Cloud Compute (EC2)"),
                None,
                Some(true),
            )),
            expected: vec![Expected {
                strategy: Some("Rehost"),
                destination: Some("Amazon Elastic Cloud Compute (EC2)"),
                tool: None,
                status: Some(RECOMMENDED),
                is_preferred: true,
            }],
        },
        Case {
            what: "the recommended strategy and destination preferred WITH a tool",
            recommended_strategy: Some("Retain"),
            recommended_destination: Some("None specified"),
            preference: Some(preference(
                Some("Retain"),
                Some("None specified"),
                Some("Strategy Recommendation Support"),
                None,
            )),
            expected: vec![Expected {
                strategy: Some("Retain"),
                destination: Some("None specified"),
                tool: Some("Strategy Recommendation Support"),
                status: Some(RECOMMENDED),
                is_preferred: true,
            }],
        },
        Case {
            what: "a preference naming another strategy than the recommended one",
            recommended_strategy: Some("Retirement"),
            recommended_destination: Some("Aurora MySQL"),
            preference: Some(preference(
                Some("Refactor"),
                Some("Amazon DocumentDB"),
                Some("Schema Conversion Tool"),
                None,
            )),
            expected: vec![
                Expected {
                    strategy: Some("Refactor"),
                    destination: Some("Amazon DocumentDB"),
                    tool: Some("Schema Conversion Tool"),
                    status: None,
                    is_preferred: true,
                },
                Expected {
                    strategy: Some("Retirement"),
                    destination: Some("Aurora MySQL"),
                    tool: None,
                    status: Some(RECOMMENDED),
                    is_preferred: false,
                },
            ],
        },
        Case {
            what: "a preference naming the recommended strategy for another destination",
            recommended_strategy: Some("Replatform"),
            recommended_destination: Some("Aurora PostgreSQL"),
            preference: Some(preference(
                Some("Replatform"),
                Some("Amazon Relational Database Service on PostgreSQL"),
                None,
                None,
            )),
            expected: vec![
                Expected {
                    strategy: Some("Replatform"),
                    destination: Some("Amazon Relational Database Service on PostgreSQL"),
                    tool: None,
                    status: None,
                    is_preferred: true,
                },
                Expected {
                    strategy: Some("Replatform"),
                    destination: Some("Aurora PostgreSQL"),
                    tool: None,
                    status: Some(RECOMMENDED),
                    is_preferred: false,
                },
            ],
        },
        Case {
            what: "an option stored as NOT preferred, beside another recommendation",
            recommended_strategy: Some("Rehost"),
            recommended_destination: Some("AWS Fargate"),
            preference: Some(preference(
                Some("Relocate"),
                Some("Amazon Relational Database Service"),
                Some("Application Migration Service"),
                Some(false),
            )),
            expected: vec![
                Expected {
                    strategy: Some("Relocate"),
                    destination: Some("Amazon Relational Database Service"),
                    tool: Some("Application Migration Service"),
                    status: None,
                    is_preferred: false,
                },
                Expected {
                    strategy: Some("Rehost"),
                    destination: Some("AWS Fargate"),
                    tool: None,
                    status: Some(RECOMMENDED),
                    is_preferred: false,
                },
            ],
        },
        Case {
            what: "an option stored as NOT preferred, naming what the service recommends",
            recommended_strategy: Some("Refactor"),
            recommended_destination: Some("AWS Elastic BeanStalk"),
            preference: Some(preference(
                Some("Refactor"),
                Some("AWS Elastic BeanStalk"),
                None,
                Some(false),
            )),
            expected: vec![Expected {
                strategy: Some("Refactor"),
                destination: Some("AWS Elastic BeanStalk"),
                tool: None,
                status: Some(RECOMMENDED),
                is_preferred: false,
            }],
        },
    ]
}

/// The record each case seeds, under the id its reads use.
fn record(index: usize, case: &Case) -> ServerRecord {
    ServerRecord {
        strategy: case.recommended_strategy.map(str::to_string),
        target_destination: case.recommended_destination.map(str::to_string),
        preferred_strategy: case.preference.clone(),
        ..server(&id_for(index))
    }
}

async fn strategies(router: &MultiServiceRouter, server_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-server-strategies/{server_id}"),
        None,
    )
    .await
}

async fn inventory(router: &MultiServiceRouter) -> Answer {
    call(router, "POST", "/list-servers", Some(json!({}))).await
}

/// Assert `answer` reports exactly `expected`, in order, member for member.
fn assert_reported(answer: &Answer, what: &str, expected: &[Expected]) {
    assert_eq!(
        answer.status, 200,
        "case {what:?}: a server of the portfolio is reported, got {} {:?}",
        answer.status, answer.body
    );
    let reported = match at(&answer.body, "/serverStrategies") {
        Value::Array(items) => items,
        other => panic!("case {what:?}: serverStrategies is always a list, got {other:?}"),
    };
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {what:?}: expected {} strategy/strategies, got {:?}",
        expected.len(),
        answer.body
    );

    for (index, want) in expected.iter().enumerate() {
        let value = |text: Option<&str>| text.map_or(Value::Null, |text| json!(text));
        for (member, want) in [
            ("/recommendation/strategy", value(want.strategy)),
            ("/recommendation/targetDestination", value(want.destination)),
            ("/recommendation/transformationTool/name", value(want.tool)),
            // The caller named a tool, not a tool catalog entry: no description and no
            // installation link are part of a choice or of a recommendation.
            ("/recommendation/transformationTool/description", Value::Null),
            (
                "/recommendation/transformationTool/tranformationToolInstallationLink",
                Value::Null,
            ),
            ("/status", value(want.status)),
            ("/isPreferred", json!(want.is_preferred)),
            // Nothing here attributes an application component to a strategy of a
            // server, so no count is reported.
            ("/numberOfApplicationComponents", Value::Null),
        ] {
            let pointer = format!("/serverStrategies/{index}{member}");
            let actual = at(&answer.body, &pointer);
            assert_eq!(
                actual, want,
                "case {what:?}: expected {want:?} at {pointer}, got {actual:?} in {:?}",
                answer.body
            );
        }
        if let Some(status) = want.status {
            assert!(
                STRATEGY_RECOMMENDATION_VALUES.contains(&status),
                "case {what:?}: a reported status must be one StrategyRecommendation \
                 declares, got {status:?}"
            );
        }
    }
}

/// Every value the table names is one its catalog declares, so a row cannot pin a value
/// the model does not admit.
fn assert_values_are_declared(cases: &[Case]) {
    for case in cases {
        let what = case.what;
        for strategy in case
            .recommended_strategy
            .into_iter()
            .chain(case.preference.as_ref().and_then(|o| o.strategy.as_deref()))
        {
            assert!(
                STRATEGIES.contains(&strategy),
                "case {what:?}: Strategy does not declare {strategy:?}"
            );
        }
        for destination in case.recommended_destination.into_iter().chain(
            case.preference
                .as_ref()
                .and_then(|o| o.target_destination.as_deref()),
        ) {
            assert!(
                TARGET_DESTINATIONS.contains(&destination),
                "case {what:?}: TargetDestination does not declare {destination:?}"
            );
        }
        if let Some(tool) = case.preference.as_ref().and_then(|o| o.tool_name.as_deref()) {
            assert!(
                TOOL_NAMES.contains(&tool),
                "case {what:?}: TransformationToolName does not declare {tool:?}"
            );
        }
    }
}

/// The read reports the strategies the server's own record holds — the preference first,
/// then the service's own recommendation when that is a different strategy — for every
/// record shape of the table.
///
/// All of the servers are seeded before any of them is read, so a row that reported
/// another row's record fails here.
#[tokio::test]
async fn the_read_reports_the_strategies_the_record_holds() {
    let cases = cases();
    assert_values_are_declared(&cases);

    // The table really does span the counts and the orderings the rule decides.
    for count in [0_usize, 1, 2] {
        assert!(
            cases.iter().any(|case| case.expected.len() == count),
            "the table must hold a server reporting {count} strategy/strategies"
        );
    }
    assert!(
        cases
            .iter()
            .any(|case| case.expected.len() == 1 && case.expected[0].is_preferred),
        "the table must hold a server whose one strategy is a preference"
    );
    assert!(
        cases
            .iter()
            .any(|case| case.expected.len() == 1 && case.expected[0].status == Some(RECOMMENDED)),
        "the table must hold a server whose one strategy is the service's recommendation"
    );

    let router = emulator_with(
        cases
            .iter()
            .enumerate()
            .map(|(index, case)| record(index, case))
            .collect(),
    );

    for (index, case) in cases.iter().enumerate() {
        let answer = strategies(&router, &id_for(index)).await;
        assert_reported(&answer, case.what, &case.expected);
    }
}

/// The read changes nothing a later caller sees: each row answers the same thing twice,
/// the inventory reads the same before and after, and a refused read creates no server.
#[tokio::test]
async fn reading_the_strategies_writes_nothing() {
    let cases = cases();
    let router = emulator_with(
        cases
            .iter()
            .enumerate()
            .map(|(index, case)| record(index, case))
            .collect(),
    );

    let before = inventory(&router).await;

    for (index, case) in cases.iter().enumerate() {
        let id = id_for(index);
        let first = strategies(&router, &id).await;
        let second = strategies(&router, &id).await;
        assert_eq!(
            first.body, second.body,
            "case {:?}: two identical reads answer the same thing, got {:?} then {:?}",
            case.what, first.body, second.body
        );
        assert_reported(&second, case.what, &case.expected);
    }

    // A read of a server the portfolio does not hold is a refusal that creates nothing:
    // the second attempt is refused the same way, and the inventory is unchanged.
    for attempt in 1..=2 {
        let refused = strategies(&router, "99b").await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (404, "ResourceNotFoundException"),
            "attempt {attempt}: an unheld server stays unheld, got {} {:?}",
            refused.status,
            refused.body
        );
    }

    let after = inventory(&router).await;
    assert_eq!(
        after.body, before.body,
        "the inventory reads the same before and after every read, got {:?} then {:?}",
        before.body, after.body
    );
}

/// `serverId` is held to its published bounds and to what the portfolio holds: an id AT
/// either bound is read, one past the maximum is refused with 400 `ValidationException`,
/// and an id of any shape naming no server of the portfolio is the modelled 404.
#[tokio::test]
async fn a_server_id_is_held_to_its_bounds_and_to_the_portfolio() {
    let shortest = "b".repeat(SERVER_ID_MIN);
    let longest = "a".repeat(SERVER_ID_MAX);
    let seeded = vec![
        ServerRecord {
            strategy: Some("Rehost".to_string()),
            ..server(&shortest)
        },
        ServerRecord {
            target_destination: Some("Amazon DynamoDB".to_string()),
            ..server(&longest)
        },
    ];
    let router = emulator_with(seeded);

    for (what, id, expected) in [
        (
            "one character, at the ServerId minimum",
            &shortest,
            Expected {
                strategy: Some("Rehost"),
                destination: None,
                tool: None,
                status: Some(RECOMMENDED),
                is_preferred: false,
            },
        ),
        (
            "27 characters, AT the ServerId maximum",
            &longest,
            Expected {
                strategy: None,
                destination: Some("Amazon DynamoDB"),
                tool: None,
                status: Some(RECOMMENDED),
                is_preferred: false,
            },
        ),
    ] {
        let answer = strategies(&router, id).await;
        assert_reported(&answer, what, &[expected]);
    }

    let past = "a".repeat(SERVER_ID_MAX + 1);
    let refused = strategies(&router, &past).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "ValidationException"),
        "an id one character past the ServerId maximum is refused, got {} {:?}",
        refused.status,
        refused.body
    );

    let longest_unseeded = "b".repeat(SERVER_ID_MAX);
    for (what, id) in [
        ("an id of the fixture's shape, never seeded", "0b"),
        ("another unseeded id", "1a"),
        (
            "an id 27 characters long, never seeded",
            longest_unseeded.as_str(),
        ),
    ] {
        let answer = strategies(&router, id).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {what:?}: a server the portfolio does not hold is the modelled 404, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
