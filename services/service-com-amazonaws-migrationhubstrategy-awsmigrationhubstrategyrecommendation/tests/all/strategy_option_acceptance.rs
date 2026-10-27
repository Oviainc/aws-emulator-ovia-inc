// SPDX-License-Identifier: CC0-1.0

//! The ONE `strategyOption` acceptance rule, driven through BOTH config updates of this
//! service, end to end over HTTP.
//!
//! The rule is a property of the modelled `StrategyOption` shape, not of one operation:
//! each of its three enum-targeting members (`strategy`, `targetDestination`, `toolName`)
//! is accepted only with a value its own enumeration declares — otherwise the request is
//! refused with 400 `ValidationException` and nothing is stored — and an option whose
//! every one of those three is absent selects no strategy, no destination and no tool, so
//! there is nothing to store and a preference an earlier call stored survives the request.
//! `isPreferred` carries no selection of its own, so an option bearing only it is that
//! case.
//!
//! Both operations that hold a preference against a resource implement that one rule —
//! `UpdateServerConfig` against a server (read back through `GetServerStrategies`) and
//! `UpdateApplicationComponentConfig` against an application component (read back through
//! `GetApplicationComponentStrategies`) — so every row below is driven through BOTH and
//! the two verdicts are required to agree: one undeclared value cannot be refused by one
//! update and stored by the other, and one selection-less option cannot be a no-op for one
//! and a write for the other.
//!
//! The table spans what the rule generalises over rather than one request: every `Strategy`
//! value alone, every `TargetDestination` alone, every `TransformationToolName` alone, all
//! three together, an explicit `isPreferred` of either value, an option naming nothing, an
//! option naming only `isPreferred`, and — per member — the family of undeclared values
//! `enum_support` derives from the catalog itself, including a value a SIBLING enumeration
//! of this service declares.
//!
//! A server and an application component enter this service only through the account's
//! data being brought in, so the portfolio is seeded through the shared engine — the same
//! `servers` and `application_components` maps an ingestion writes — while every update,
//! every read and every assertion goes through the router, exactly as a client reaches
//! them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord,
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState, ServerRecord,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::enum_support::undeclared_values;
use crate::import_and_assess::{at, call, Answer, TEST_REGION};

/// This service's one sentence for a request carrying a value it does not accept, spelled
/// out here rather than imported from the crate so a wrong message fails this suite
/// instead of agreeing with itself.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The wire error code a refusal of such a request carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Every value `Strategy` declares, from `smithy_query(query="shape",shape="Strategy")`.
const STRATEGIES: &[&str] = &[
    "Refactor",
    "Rehost",
    "Relocate",
    "Replatform",
    "Repurchase",
    "Retain",
    "Retirement",
];

/// Every value `TargetDestination` declares, from
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

/// Every value `TransformationToolName` declares, from
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

/// The id of the one server and the one application component every row runs against. A
/// `ResourceId` (`^[0-9a-b]+`), which both `ServerId` (at most 27 characters) and
/// `ApplicationComponentId` (`[0-9a-zA-Z-]+`, at most 44) also admit.
const SERVER: &str = "0a";
const COMPONENT: &str = "1a";

/// The preference stored before a selection-less row, so "nothing to store" is observable
/// as the earlier option surviving rather than only as an absence.
fn earlier_option() -> Value {
    option(
        Some("Refactor"),
        Some("AWS Fargate"),
        Some("Porting Assistant For .NET"),
        None,
    )
}

/// A `strategyOption` carrying exactly the members a row names, with every other left out
/// — so a row naming one member sends one member.
fn option(
    strategy: Option<&str>,
    destination: Option<&str>,
    tool: Option<&str>,
    is_preferred: Option<bool>,
) -> Value {
    let mut members = serde_json::Map::new();
    if let Some(strategy) = strategy {
        members.insert("strategy".to_string(), json!(strategy));
    }
    if let Some(destination) = destination {
        members.insert("targetDestination".to_string(), json!(destination));
    }
    if let Some(tool) = tool {
        members.insert("toolName".to_string(), json!(tool));
    }
    if let Some(is_preferred) = is_preferred {
        members.insert("isPreferred".to_string(), json!(is_preferred));
    }
    Value::Object(members)
}

/// One emulator over one shared engine, holding one server and one application component
/// in the Region every request here is signed for — the same maps an ingestion writes.
fn portfolio() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let region = store.region_entry(TEST_REGION);
        region.servers.insert(
            SERVER.to_string(),
            ServerRecord {
                id: SERVER.to_string(),
                ..Default::default()
            },
        );
        region.application_components.insert(
            COMPONENT.to_string(),
            ApplicationComponentRecord {
                id: COMPONENT.to_string(),
                ..Default::default()
            },
        );
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Which of the two config updates a probe is driven through: the operation that stores the
/// preference, and the read that reports what it stored.
#[derive(Clone, Copy, Debug)]
enum Target {
    Server,
    Component,
}

impl Target {
    /// Both of them, so every row is driven through each.
    const ALL: [Target; 2] = [Target::Server, Target::Component];

    fn name(self) -> &'static str {
        match self {
            Target::Server => "UpdateServerConfig",
            Target::Component => "UpdateApplicationComponentConfig",
        }
    }

    /// Store `option` as this resource's preference. `None` sends no `strategyOption` at
    /// all.
    async fn update(self, router: &MultiServiceRouter, option: Option<Value>) -> Answer {
        let (uri, mut body) = match self {
            Target::Server => ("/update-server-config/", json!({"serverId": SERVER})),
            Target::Component => (
                "/update-applicationcomponent-config/",
                json!({"applicationComponentId": COMPONENT}),
            ),
        };
        if let Some(option) = option {
            body["strategyOption"] = option;
        }
        call(router, "POST", uri, Some(body)).await
    }

    /// Read the preference back through the operation that reports it.
    async fn read(self, router: &MultiServiceRouter) -> Answer {
        let uri = match self {
            Target::Server => format!("/get-server-strategies/{SERVER}"),
            Target::Component => format!("/get-applicationcomponent-strategies/{COMPONENT}"),
        };
        call(router, "GET", &uri, None).await
    }

    /// Where the reported strategies live in this read's answer.
    fn listing(self) -> &'static str {
        match self {
            Target::Server => "/serverStrategies",
            Target::Component => "/applicationComponentStrategies",
        }
    }
}

/// The one strategy a read reports, member for member: the three members a caller can
/// select and the flag the read carries beside them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Preference {
    strategy: Value,
    destination: Value,
    tool: Value,
    is_preferred: Value,
}

impl Preference {
    /// The preference a request carrying `option` has to leave stored: its three
    /// selectable members as sent, and `isPreferred` — "set to true if the recommendation
    /// is set as preferred" — defaulting to preferred when the call did not say.
    fn as_sent(option: &Value) -> Self {
        Self {
            strategy: at(option, "/strategy"),
            destination: at(option, "/targetDestination"),
            tool: at(option, "/toolName"),
            is_preferred: match at(option, "/isPreferred") {
                Value::Null => json!(true),
                flag => flag,
            },
        }
    }
}

/// The single strategy a read reports, or `None` when the resource holds no strategy at
/// all.
fn reported_preference(target: Target, answer: &Answer) -> Option<Preference> {
    let listing = target.listing();
    match at(&answer.body, listing) {
        Value::Array(items) if items.len() == 1 => Some(Preference {
            strategy: at(&answer.body, &format!("{listing}/0/recommendation/strategy")),
            destination: at(
                &answer.body,
                &format!("{listing}/0/recommendation/targetDestination"),
            ),
            tool: at(
                &answer.body,
                &format!("{listing}/0/recommendation/transformationTool/name"),
            ),
            is_preferred: at(&answer.body, &format!("{listing}/0/isPreferred")),
        }),
        Value::Array(items) if items.is_empty() => None,
        other => panic!("{} reported {other:?}, not one strategy", target.name()),
    }
}

/// One enum-targeting member of `StrategyOption`: its name, the catalog its own target
/// enumeration declares, and the sibling catalogs a "legal for the service, not for this
/// member" row is drawn from.
struct EnumMember {
    member: &'static str,
    declared: &'static [&'static str],
    siblings: &'static [&'static [&'static str]],
}

/// What one of the two updates answered for a row: who answered, with what status, and
/// what the read then reported as stored.
struct Outcome {
    who: &'static str,
    status: u16,
    stored: Option<Preference>,
}

/// What the shared rule says about one `strategyOption`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The option selects a strategy, so it becomes the resource's preference.
    Stored,
    /// The option selects nothing, so there is nothing to store.
    NothingToStore,
    /// The option carries a value no enumeration of the shape declares.
    Refused,
}

/// One row of the table: the option a request carries and what the rule says about it.
struct Row {
    what: String,
    /// `None` sends no `strategyOption` member at all.
    option: Option<Value>,
    verdict: Verdict,
}

/// Every row the rule has to answer the same way through either update.
fn table() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // Each declared value of each of the three catalogs, named alone, so the whole of
    // every catalog the rule admits is covered rather than the one value a report cites.
    for strategy in STRATEGIES {
        rows.push(Row {
            what: format!("the strategy {strategy:?} alone"),
            option: Some(option(Some(strategy), None, None, None)),
            verdict: Verdict::Stored,
        });
    }
    for destination in TARGET_DESTINATIONS {
        rows.push(Row {
            what: format!("the destination {destination:?} alone"),
            option: Some(option(None, Some(destination), None, None)),
            verdict: Verdict::Stored,
        });
    }
    for tool in TOOL_NAMES {
        rows.push(Row {
            what: format!("the tool {tool:?} alone"),
            option: Some(option(None, None, Some(tool), None)),
            verdict: Verdict::Stored,
        });
    }

    // All three members at once, with an explicit `isPreferred` of either value.
    for (index, is_preferred) in [None, Some(true), Some(false)].into_iter().enumerate() {
        rows.push(Row {
            what: format!("all three members with isPreferred {is_preferred:?}"),
            option: Some(option(
                Some(STRATEGIES[index % STRATEGIES.len()]),
                Some(TARGET_DESTINATIONS[index % TARGET_DESTINATIONS.len()]),
                Some(TOOL_NAMES[index % TOOL_NAMES.len()]),
                is_preferred,
            )),
            verdict: Verdict::Stored,
        });
    }

    // The selection-less options: no member of the shape, or only the member that carries
    // no selection of its own.
    rows.push(Row {
        what: "no strategyOption at all".to_string(),
        option: None,
        verdict: Verdict::NothingToStore,
    });
    rows.push(Row {
        what: "a strategyOption naming nothing".to_string(),
        option: Some(json!({})),
        verdict: Verdict::NothingToStore,
    });
    for is_preferred in [true, false] {
        rows.push(Row {
            what: format!("a strategyOption carrying only isPreferred {is_preferred}"),
            option: Some(option(None, None, None, Some(is_preferred))),
            verdict: Verdict::NothingToStore,
        });
    }

    // One undeclared value per member, from the family `enum_support` derives from the
    // catalog itself — including a value a sibling enumeration of this service declares,
    // which is legal for the service but not for this member.
    let members = [
        EnumMember {
            member: "strategy",
            declared: STRATEGIES,
            siblings: &[TARGET_DESTINATIONS, TOOL_NAMES],
        },
        EnumMember {
            member: "targetDestination",
            declared: TARGET_DESTINATIONS,
            siblings: &[STRATEGIES, TOOL_NAMES],
        },
        EnumMember {
            member: "toolName",
            declared: TOOL_NAMES,
            siblings: &[STRATEGIES, TARGET_DESTINATIONS],
        },
    ];
    for member in members {
        for (kind, value) in undeclared_values(member.declared, member.siblings) {
            // Sent beside a legal value of a DIFFERENT member and an `isPreferred`, so the
            // row cannot pass by the request selecting nothing at all.
            let mut carrier = option(None, None, None, Some(true));
            carrier["strategy"] = json!(STRATEGIES[0]);
            carrier[member.member] = json!(value);
            rows.push(Row {
                what: format!("{} carrying {kind} ({value:?})", member.member),
                option: Some(carrier),
                verdict: Verdict::Refused,
            });
        }
    }

    rows
}

/// The table covers the whole of each catalog the rule admits, so no value can be left
/// unexercised by a row list that drifted.
#[test]
fn the_table_covers_every_declared_value() {
    let rows = table();
    let sends = |member: &str, value: &str| {
        rows.iter().any(|row| {
            row.verdict == Verdict::Stored
                && row
                    .option
                    .as_ref()
                    .is_some_and(|option| at(option, &format!("/{member}")) == json!(value))
        })
    };
    for (member, declared) in [
        ("strategy", STRATEGIES),
        ("targetDestination", TARGET_DESTINATIONS),
        ("toolName", TOOL_NAMES),
    ] {
        for value in declared {
            assert!(
                sends(member, value),
                "the table must send every declared {member} value, and {value:?} is missing"
            );
        }
    }
}

/// The shared rule, driven through BOTH config updates: every row is answered the same way
/// by each, and what an accepted option stores is read back through the operation that
/// reports it.
#[tokio::test]
async fn one_strategy_option_rule_serves_both_config_updates() {
    for row in table() {
        let mut outcomes: Vec<Outcome> = Vec::new();

        for target in Target::ALL {
            let what = &row.what;
            let who = target.name();
            // A fresh portfolio per probe, holding a preference an earlier call stored, so
            // "nothing to store" is observable as that option surviving.
            let router = portfolio();
            let seeded = target.update(&router, Some(earlier_option())).await;
            assert_eq!(
                seeded.status, 200,
                "case {what:?} on {who}: the earlier update succeeds, got {} {:?}",
                seeded.status, seeded.body
            );
            let before = reported_preference(target, &target.read(&router).await);
            assert!(
                before.is_some(),
                "case {what:?} on {who}: the earlier update stored a preference, got \
                 {before:?}"
            );

            let answer = target.update(&router, row.option.clone()).await;
            let after = reported_preference(target, &target.read(&router).await);

            match row.verdict {
                Verdict::Stored | Verdict::NothingToStore => assert_eq!(
                    answer.status, 200,
                    "case {what:?} on {who}: the option is accepted, got {} {:?}",
                    answer.status, answer.body
                ),
                Verdict::Refused => {
                    assert_eq!(
                        answer.status, 400,
                        "case {what:?} on {who}: a value no enumeration of the shape \
                         declares is refused, got {} {:?}",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        answer.error_code(),
                        VALIDATION_EXCEPTION,
                        "case {what:?} on {who}: expected {VALIDATION_EXCEPTION}, got {:?}",
                        answer.body
                    );
                    assert_eq!(
                        answer.string("/message"),
                        INVALID_REQUEST_BODY,
                        "case {what:?} on {who}: expected the message \
                         {INVALID_REQUEST_BODY:?}, got {:?}",
                        answer.body
                    );
                }
            }

            match row.verdict {
                // The option named a selection, so it replaced the earlier preference
                // member for member, `isPreferred` defaulting to preferred.
                Verdict::Stored => {
                    let sent = row.option.clone().unwrap_or_else(|| json!({}));
                    let expected = Preference::as_sent(&sent);
                    assert_eq!(
                        after,
                        Some(expected.clone()),
                        "case {what:?} on {who}: expected the stored preference \
                         {expected:?}, got {after:?}"
                    );
                }
                // Nothing was selected, and a refusal stores nothing: either way the
                // earlier preference is exactly what it was.
                Verdict::NothingToStore | Verdict::Refused => assert_eq!(
                    after, before,
                    "case {what:?} on {who}: the stored preference is unchanged, expected \
                     {before:?}, got {after:?}"
                ),
            }

            outcomes.push(Outcome {
                who,
                status: answer.status,
                stored: after,
            });
        }

        // The two updates implement ONE rule, so neither the status nor what was stored
        // may differ between them.
        let first = &outcomes[0];
        for other in &outcomes[1..] {
            assert_eq!(
                other.status, first.status,
                "case {:?}: {} answered {} and {} answered {} for one rule",
                row.what, first.who, first.status, other.who, other.status
            );
            assert_eq!(
                other.stored, first.stored,
                "case {:?}: {} stored {:?} and {} stored {:?} for one rule",
                row.what, first.who, first.stored, other.who, other.stored
            );
        }
    }
}
