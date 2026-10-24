// SPDX-License-Identifier: CC0-1.0

//! `UpdateServerConfig` and the read that reports what it stored, end to end over HTTP.
//!
//! The rule under test is the one the operation states for ANY server of the portfolio,
//! not for one request: the `strategyOption` a call names becomes the strategy option the
//! account prefers for THAT server, stored member for member, and `GetServerStrategies`
//! reports it back as that server's preferred strategy; a call naming no option changes
//! nothing; a `serverId` naming no server of the portfolio is refused with the modelled
//! 404 `ResourceNotFoundException`; and an option carrying a value its enumeration does
//! not declare is refused with 400 `ValidationException`, leaving the stored preference as
//! it was.
//!
//! Each table below therefore spans the class the rule generalises over — every `Strategy`
//! value, every `TargetDestination`, every `TransformationToolName`, an option naming one
//! member and an option naming three, an explicit `isPreferred` of either value, and a
//! server the service already holds a recommendation for and one it does not — rather
//! than the single update the issue cited.
//!
//! A server enters this service only through an import file task's ingestion of the import
//! file, which is filed separately (the import-loop issue). So these cases seed the
//! inventory records through the shared engine with the harness `server_inventory` uses —
//! the same `servers` map ingestion writes — and then drive every assertion through the
//! router, so the handlers, their refusals and the serialized shapes are exercised exactly
//! as a client reaches them.

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::ServerRecord;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, Answer};
use crate::server_inventory::{emulator_with, server};

/// Every value `Strategy` admits, from `smithy_query(query="shape",shape="Strategy")`.
/// Spelled out here so the table below can be checked to cover the whole catalog rather
/// than the one strategy the issue cited.
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

/// `StrategyRecommendation`'s value for a strategy the service recommends, from
/// `smithy_query(query="shape",shape="StrategyRecommendation")` — the status a
/// recommendation the service holds is reported with.
const RECOMMENDED: &str = "recommended";

/// The published bounds of `ServerId`: `@length(min: 1, max: 27)`
/// (`smithy_query(query="operation",operation="UpdateServerConfig")` -> `serverId`),
/// spelled out independently of any crate constant.
const SERVER_ID_MIN: usize = 1;
const SERVER_ID_MAX: usize = 27;

/// The id of the `index`th server of a table's fixture: a `ResourceId` (`^[0-9a-b]+`),
/// short enough to also be a `ServerId` (at most 27 characters).
fn id_for(index: usize) -> String {
    format!("{index:02}b")
}

/// The `strategyOption` a request carries, with every member it does not name left out —
/// so a row naming one member sends exactly one member.
fn strategy_option(
    strategy: Option<&str>,
    destination: Option<&str>,
    tool: Option<&str>,
    is_preferred: Option<bool>,
) -> Value {
    let mut option = serde_json::Map::new();
    if let Some(strategy) = strategy {
        option.insert("strategy".to_string(), json!(strategy));
    }
    if let Some(destination) = destination {
        option.insert("targetDestination".to_string(), json!(destination));
    }
    if let Some(tool) = tool {
        option.insert("toolName".to_string(), json!(tool));
    }
    if let Some(is_preferred) = is_preferred {
        option.insert("isPreferred".to_string(), json!(is_preferred));
    }
    Value::Object(option)
}

async fn update(router: &MultiServiceRouter, server_id: &str, option: Option<Value>) -> Answer {
    let mut request = json!({"serverId": server_id});
    if let Some(option) = option {
        request["strategyOption"] = option;
    }
    call(router, "POST", "/update-server-config/", Some(request)).await
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

/// How many strategies an answer reports.
fn reported_count(answer: &Answer) -> usize {
    match at(&answer.body, "/serverStrategies") {
        Value::Array(items) => items.len(),
        _ => 0,
    }
}

/// Assert that the `index`th reported strategy is the recommendation `strategy`,
/// `destination` and `tool` name, member for member, with every member they do not name
/// absent.
fn assert_recommendation(
    answer: &Answer,
    index: usize,
    what: &str,
    strategy: Option<&str>,
    destination: Option<&str>,
    tool: Option<&str>,
) {
    let expected = |value: Option<&str>| value.map_or(Value::Null, |value| json!(value));
    for (member, expected) in [
        ("strategy", expected(strategy)),
        ("targetDestination", expected(destination)),
        ("transformationTool/name", expected(tool)),
        // No tool catalog entry was named, only a tool: the description and the
        // installation link of one are not part of a caller's choice.
        ("transformationTool/description", Value::Null),
        (
            "transformationTool/tranformationToolInstallationLink",
            Value::Null,
        ),
    ] {
        let pointer = format!("/serverStrategies/{index}/recommendation/{member}");
        let actual = at(&answer.body, &pointer);
        assert_eq!(
            actual, expected,
            "case {what:?}: expected {member} {expected:?} at {pointer}, got {actual:?} in {:?}",
            answer.body
        );
    }
}

/// One update: the option a request names, and what the read then has to report.
struct Case {
    what: String,
    strategy: Option<&'static str>,
    destination: Option<&'static str>,
    tool: Option<&'static str>,
    is_preferred: Option<bool>,
}

impl Case {
    fn option(&self) -> Value {
        strategy_option(
            self.strategy,
            self.destination,
            self.tool,
            self.is_preferred,
        )
    }

    /// `isPreferred` is "set to true if the recommendation is set as preferred", so an
    /// option stored through this operation is preferred unless the call said otherwise.
    fn reported_as_preferred(&self) -> bool {
        self.is_preferred.unwrap_or(true)
    }
}

/// The updates every round-trip case runs: every value of each of the three catalogs the
/// option draws on, options naming one member and options naming three, and an explicit
/// `isPreferred` of either value.
fn preference_cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for (index, strategy) in STRATEGIES.iter().enumerate() {
        cases.push(Case {
            what: format!("the strategy {strategy:?}, with a destination and a tool"),
            strategy: Some(strategy),
            destination: Some(TARGET_DESTINATIONS[index % TARGET_DESTINATIONS.len()]),
            tool: Some(TOOL_NAMES[index % TOOL_NAMES.len()]),
            is_preferred: Some(true),
        });
    }
    for (index, destination) in TARGET_DESTINATIONS.iter().enumerate() {
        cases.push(Case {
            what: format!("the destination {destination:?}, with a strategy"),
            strategy: Some(STRATEGIES[index % STRATEGIES.len()]),
            destination: Some(destination),
            tool: None,
            is_preferred: None,
        });
    }
    for tool in TOOL_NAMES {
        cases.push(Case {
            what: format!("the tool {tool:?} alone"),
            strategy: None,
            destination: None,
            tool: Some(tool),
            is_preferred: None,
        });
    }
    cases.push(Case {
        what: "a strategy alone".to_string(),
        strategy: Some("Retain"),
        destination: None,
        tool: None,
        is_preferred: None,
    });
    cases.push(Case {
        what: "a destination alone".to_string(),
        strategy: None,
        destination: Some("Aurora PostgreSQL"),
        tool: None,
        is_preferred: None,
    });
    cases.push(Case {
        what: "an option the call marked as NOT preferred".to_string(),
        strategy: Some("Relocate"),
        destination: Some("Amazon Elastic Cloud Compute (EC2)"),
        tool: Some("Application Migration Service"),
        is_preferred: Some(false),
    });
    cases
}

/// Every update stores the option it named against its own server, and the read reports
/// exactly that option back — for every value of each catalog the option draws on.
///
/// All of the servers are updated before any of them is read, so a row that reported
/// another row's option (or that overwrote it) fails here.
#[tokio::test]
async fn an_update_stores_the_strategy_option_the_read_reports() {
    let cases = preference_cases();

    for strategy in STRATEGIES {
        assert!(
            cases.iter().any(|case| case.strategy == Some(*strategy)),
            "the table must cover the whole Strategy catalog, and {strategy:?} is missing"
        );
    }
    for destination in TARGET_DESTINATIONS {
        assert!(
            cases.iter().any(|case| case.destination == Some(*destination)),
            "the table must cover the whole TargetDestination catalog, and {destination:?} \
             is missing"
        );
    }
    for tool in TOOL_NAMES {
        assert!(
            cases.iter().any(|case| case.tool == Some(*tool)),
            "the table must cover the whole TransformationToolName catalog, and {tool:?} is \
             missing"
        );
    }

    let router = emulator_with((0..cases.len()).map(|index| server(&id_for(index))).collect());

    for (index, case) in cases.iter().enumerate() {
        let answer = update(&router, &id_for(index), Some(case.option())).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the update of a server of the portfolio succeeds, got {} {:?}",
            case.what, answer.status, answer.body
        );
    }

    for (index, case) in cases.iter().enumerate() {
        let answer = strategies(&router, &id_for(index)).await;
        let what = &case.what;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: the read of a server of the portfolio succeeds, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_count(&answer),
            1,
            "case {what:?}: the stored option is the server's one strategy, got {:?}",
            answer.body
        );
        assert_recommendation(
            &answer,
            0,
            what,
            case.strategy,
            case.destination,
            case.tool,
        );
        assert_eq!(
            at(&answer.body, "/serverStrategies/0/isPreferred"),
            json!(case.reported_as_preferred()),
            "case {what:?}: expected isPreferred {}, got {:?}",
            case.reported_as_preferred(),
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/serverStrategies/0/status"),
            Value::Null,
            "case {what:?}: nothing recommended the caller's own option, so it carries no \
             StrategyRecommendation status, got {:?}",
            answer.body
        );
        assert_eq!(
            at(&answer.body, "/serverStrategies/0/numberOfApplicationComponents"),
            Value::Null,
            "case {what:?}: this service holds no application component to count, got {:?}",
            answer.body
        );
    }
}

/// The preference is one per server: the last update of a server is what it holds, and
/// updating one server leaves every other server's preference alone.
#[tokio::test]
async fn the_last_update_of_a_server_is_the_one_it_holds() {
    let router = emulator_with(vec![server("0a"), server("1a")]);

    let rounds = [
        ("a first option", Some("Rehost"), Some("Amazon Elastic Cloud Compute (EC2)"), None),
        (
            "a second option, naming another strategy and a tool",
            Some("Replatform"),
            Some("Amazon Elastic Container Service (ECS)"),
            Some("App2Container"),
        ),
        ("a third option, naming only a strategy", Some("Retirement"), None, None),
    ];

    for (what, strategy, destination, tool) in rounds {
        let answer = update(
            &router,
            "0a",
            Some(strategy_option(strategy, destination, tool, None)),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: the update succeeds, got {} {:?}",
            answer.status, answer.body
        );

        let got = strategies(&router, "0a").await;
        assert_eq!(
            reported_count(&got),
            1,
            "case {what:?}: an update replaces the preference rather than adding one, got {:?}",
            got.body
        );
        assert_recommendation(&got, 0, what, strategy, destination, tool);

        let untouched = strategies(&router, "1a").await;
        assert_eq!(
            reported_count(&untouched),
            0,
            "case {what:?}: the server nobody updated still holds no strategy, got {:?}",
            untouched.body
        );
    }
}

/// An update that names no strategy option changes nothing: the call succeeds and the
/// server's preference is exactly what it was — absent when none was stored, and the
/// earlier option when one was.
#[tokio::test]
async fn an_update_naming_no_strategy_option_changes_nothing() {
    let probes: Vec<(&str, Option<Value>)> = vec![
        ("no strategyOption at all", None),
        ("a strategyOption naming nothing", Some(json!({}))),
        (
            "a strategyOption carrying only isPreferred",
            Some(strategy_option(None, None, None, Some(true))),
        ),
        (
            "a strategyOption carrying only a false isPreferred",
            Some(strategy_option(None, None, None, Some(false))),
        ),
    ];

    for (what, option) in probes {
        let router = emulator_with(vec![server("0a"), server("1a")]);

        // On a server holding no preference: nothing is stored, so nothing is reported.
        let answer = update(&router, "0a", option.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: an update that asks for no change still succeeds, got {} {:?}",
            answer.status, answer.body
        );
        let got = strategies(&router, "0a").await;
        assert_eq!(
            reported_count(&got),
            0,
            "case {what:?}: no option was named, so the server holds none, got {:?}",
            got.body
        );

        // On a server holding one: the stored option survives the call.
        let stored = strategy_option(
            Some("Refactor"),
            Some("AWS Fargate"),
            Some("Porting Assistant For .NET"),
            None,
        );
        let first = update(&router, "1a", Some(stored)).await;
        assert_eq!(
            first.status, 200,
            "case {what:?}: the first update succeeds, got {} {:?}",
            first.status, first.body
        );
        let answer = update(&router, "1a", option).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: the second update succeeds, got {} {:?}",
            answer.status, answer.body
        );
        let got = strategies(&router, "1a").await;
        assert_eq!(
            reported_count(&got),
            1,
            "case {what:?}: the stored preference survives an update naming none, got {:?}",
            got.body
        );
        assert_recommendation(
            &got,
            0,
            what,
            Some("Refactor"),
            Some("AWS Fargate"),
            Some("Porting Assistant For .NET"),
        );
    }
}

/// What a server reports beside the preference: the recommendation the service itself
/// holds for it, which the update does not touch.
///
/// A preference naming the same strategy and destination as that recommendation is ONE
/// strategy, so it is reported once — carrying the recommended status, because it is also
/// what the service recommends.
#[tokio::test]
async fn the_preference_and_the_services_own_recommendation_are_both_reported() {
    let analyzed = |id: &str, strategy: &str, destination: &str| ServerRecord {
        strategy: Some(strategy.to_string()),
        target_destination: Some(destination.to_string()),
        ..server(id)
    };
    let router = emulator_with(vec![
        analyzed("0a", "Rehost", "Amazon Elastic Cloud Compute (EC2)"),
        analyzed("1a", "Rehost", "Amazon Elastic Cloud Compute (EC2)"),
        analyzed("2a", "Replatform", "Aurora MySQL"),
    ]);

    // A server nobody chose a preference for: its one strategy is the service's own
    // recommendation, recommended and not preferred.
    let only_recommended = strategies(&router, "0a").await;
    assert_eq!(
        reported_count(&only_recommended),
        1,
        "a server with a recommendation and no preference reports that one strategy, got {:?}",
        only_recommended.body
    );
    assert_recommendation(
        &only_recommended,
        0,
        "the service's own recommendation",
        Some("Rehost"),
        Some("Amazon Elastic Cloud Compute (EC2)"),
        None,
    );
    assert_eq!(
        at(&only_recommended.body, "/serverStrategies/0/status"),
        json!(RECOMMENDED),
        "the service's own recommendation carries the recommended status, got {:?}",
        only_recommended.body
    );
    assert_eq!(
        at(&only_recommended.body, "/serverStrategies/0/isPreferred"),
        json!(false),
        "nobody chose it, so it is not preferred, got {:?}",
        only_recommended.body
    );

    // A preference naming what the service already recommends: one strategy, preferred
    // AND recommended.
    let same = update(
        &router,
        "1a",
        Some(strategy_option(
            Some("Rehost"),
            Some("Amazon Elastic Cloud Compute (EC2)"),
            None,
            Some(true),
        )),
    )
    .await;
    assert_eq!(
        same.status, 200,
        "preferring the recommended strategy succeeds, got {} {:?}",
        same.status, same.body
    );
    let got = strategies(&router, "1a").await;
    assert_eq!(
        reported_count(&got),
        1,
        "one strategy preferred and recommended is reported once, got {:?}",
        got.body
    );
    assert_eq!(
        at(&got.body, "/serverStrategies/0/status"),
        json!(RECOMMENDED),
        "it is still what the service recommends, got {:?}",
        got.body
    );
    assert_eq!(
        at(&got.body, "/serverStrategies/0/isPreferred"),
        json!(true),
        "and it is now the preferred one, got {:?}",
        got.body
    );

    // A preference naming another strategy: two strategies, the preferred one first.
    let other = update(
        &router,
        "2a",
        Some(strategy_option(
            Some("Refactor"),
            Some("Amazon DynamoDB"),
            Some("Schema Conversion Tool"),
            None,
        )),
    )
    .await;
    assert_eq!(
        other.status, 200,
        "preferring another strategy succeeds, got {} {:?}",
        other.status, other.body
    );
    let got = strategies(&router, "2a").await;
    assert_eq!(
        reported_count(&got),
        2,
        "the preference and the service's own recommendation are two strategies, got {:?}",
        got.body
    );
    assert_recommendation(
        &got,
        0,
        "the preferred strategy",
        Some("Refactor"),
        Some("Amazon DynamoDB"),
        Some("Schema Conversion Tool"),
    );
    assert_eq!(
        at(&got.body, "/serverStrategies/0/isPreferred"),
        json!(true),
        "the preferred strategy is reported as preferred, got {:?}",
        got.body
    );
    assert_eq!(
        at(&got.body, "/serverStrategies/0/status"),
        Value::Null,
        "nothing recommended the caller's own option, got {:?}",
        got.body
    );
    assert_recommendation(
        &got,
        1,
        "the service's own recommendation",
        Some("Replatform"),
        Some("Aurora MySQL"),
        None,
    );
    assert_eq!(
        at(&got.body, "/serverStrategies/1/status"),
        json!(RECOMMENDED),
        "the service's own recommendation keeps the recommended status, got {:?}",
        got.body
    );
    assert_eq!(
        at(&got.body, "/serverStrategies/1/isPreferred"),
        json!(false),
        "another option is preferred, so this one is not, got {:?}",
        got.body
    );
}

/// A `serverId` that names no server of this account's portfolio is refused with the
/// modelled 404 — by the update and by the read alike — and the refused update creates
/// nothing.
#[tokio::test]
async fn a_server_id_naming_no_server_is_refused() {
    let router = emulator_with(vec![server("0a")]);

    let unknown = vec![
        ("one character, at the ServerId minimum", "b".repeat(SERVER_ID_MIN)),
        ("27 characters, AT the ServerId maximum", "a".repeat(SERVER_ID_MAX)),
        ("an id of the fixture's shape, never issued", "99b".to_string()),
    ];

    for (what, server_id) in unknown {
        let refused = update(
            &router,
            &server_id,
            Some(strategy_option(Some("Rehost"), None, None, None)),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {what:?}: the update of a server the portfolio does not hold is a 404, got \
             {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {what:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );

        let read = strategies(&router, &server_id).await;
        assert_eq!(
            read.status, 404,
            "case {what:?}: and so is the read, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_code(),
            "ResourceNotFoundException",
            "case {what:?}: expected ResourceNotFoundException, got {:?}",
            read.body
        );
    }

    // Nothing was created by the refusals: the inventory is still the one seeded server.
    let listed = inventory(&router).await;
    assert_eq!(
        at(&listed.body, "/serverInfos"),
        json!([{"id": "0a"}]),
        "a refused update creates no server, got {:?}",
        listed.body
    );
}

/// A request at each published bound of `serverId` is accepted — the refusals above answer
/// a genuine violation, not every request — and one character past the maximum is refused
/// by the boundary validator.
#[tokio::test]
async fn a_server_id_at_a_published_bound_is_accepted() {
    let shortest = "b".repeat(SERVER_ID_MIN);
    let longest = "a".repeat(SERVER_ID_MAX);
    let router = emulator_with(vec![server(&shortest), server(&longest)]);

    for (what, server_id) in [
        ("one character, at the ServerId minimum", &shortest),
        ("27 characters, AT the ServerId maximum", &longest),
    ] {
        let answer = update(
            &router,
            server_id,
            Some(strategy_option(Some("Retain"), None, None, None)),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: an id within the bound is accepted, got {} {:?}",
            answer.status, answer.body
        );
        let got = strategies(&router, server_id).await;
        assert_recommendation(&got, 0, what, Some("Retain"), None, None);
    }

    let past = "a".repeat(SERVER_ID_MAX + 1);
    let refused = update(
        &router,
        &past,
        Some(strategy_option(Some("Retain"), None, None, None)),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "an id one character past the ServerId maximum is refused, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "expected ValidationException, got {:?}",
        refused.body
    );
}

/// A `strategyOption` carrying a value its enumeration does not declare is refused with
/// 400 `ValidationException`, and the refusal leaves the stored preference as it was.
#[tokio::test]
async fn a_value_no_enumeration_declares_is_refused() {
    let router = emulator_with(vec![server("0a")]);

    let stored = strategy_option(
        Some("Rehost"),
        Some("Amazon Elastic Cloud Compute (EC2)"),
        Some("App2Container"),
        None,
    );
    let first = update(&router, "0a", Some(stored)).await;
    assert_eq!(
        first.status, 200,
        "the preference this test protects is stored first, got {} {:?}",
        first.status, first.body
    );

    let refusals: Vec<(&str, Value)> = vec![
        (
            "a strategy Strategy does not declare",
            strategy_option(Some("Rehosting"), None, None, None),
        ),
        (
            "a strategy declared in another casing",
            strategy_option(Some("rehost"), None, None, None),
        ),
        (
            "a destination TargetDestination does not declare",
            strategy_option(None, Some("EC2"), None, None),
        ),
        (
            "a tool TransformationToolName does not declare",
            strategy_option(None, None, Some("App2Containers"), None),
        ),
        (
            "an empty strategy",
            strategy_option(Some(""), None, None, None),
        ),
        (
            "one declared value beside one that is not",
            strategy_option(Some("Replatform"), Some("Aurora"), None, None),
        ),
    ];

    for (what, option) in refusals {
        let refused = update(&router, "0a", Some(option)).await;
        assert_eq!(
            refused.status, 400,
            "case {what:?}: expected a 400 refusal, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {:?}",
            refused.body
        );

        let got = strategies(&router, "0a").await;
        assert_eq!(
            reported_count(&got),
            1,
            "case {what:?}: the refusal stored nothing, got {:?}",
            got.body
        );
        assert_recommendation(
            &got,
            0,
            what,
            Some("Rehost"),
            Some("Amazon Elastic Cloud Compute (EC2)"),
            Some("App2Container"),
        );
    }
}

/// A server the service holds no recommendation for and nobody chose a preference for
/// reports an empty list of strategies: a 200 with no strategy, not a refusal and not a
/// canned one.
#[tokio::test]
async fn a_server_with_nothing_to_report_reports_no_strategy() {
    let router = emulator_with(vec![server("0a"), server("1b")]);

    for server_id in ["0a", "1b"] {
        let answer = strategies(&router, server_id).await;
        assert_eq!(
            answer.status, 200,
            "case {server_id:?}: a server of the portfolio is reported, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(&answer.body, "/serverStrategies"),
            json!([]),
            "case {server_id:?}: nothing recommended a strategy for it, got {:?}",
            answer.body
        );
    }
}

/// Reading the strategies does not change them: the same read answers the same thing
/// twice, and the update writes only the preference — the inventory reports the same
/// `ServerDetail` after it as before.
#[tokio::test]
async fn reading_the_strategies_changes_nothing() {
    let router = emulator_with(vec![ServerRecord {
        name: Some("web-01".to_string()),
        strategy: Some("Rehost".to_string()),
        target_destination: Some("Amazon Elastic Cloud Compute (EC2)".to_string()),
        data_collection_status: Some("dataCollectionTaskCompleted".to_string()),
        ..server("0a")
    }]);

    let before = inventory(&router).await;
    let updated = update(
        &router,
        "0a",
        Some(strategy_option(
            Some("Refactor"),
            Some("Amazon DynamoDB"),
            None,
            None,
        )),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "the update succeeds, got {} {:?}",
        updated.status, updated.body
    );

    let first = strategies(&router, "0a").await;
    let second = strategies(&router, "0a").await;
    assert_eq!(
        first.body, second.body,
        "two identical reads answer the same thing, got {:?} then {:?}",
        first.body, second.body
    );

    // The update stored a preference, which is not a member of `ServerDetail`: the
    // inventory still reports the recommendation the service holds for the server.
    let after = inventory(&router).await;
    assert_eq!(
        after.body, before.body,
        "the inventory reports the service's own recommendation before and after, got {:?} \
         then {:?}",
        before.body, after.body
    );
}
