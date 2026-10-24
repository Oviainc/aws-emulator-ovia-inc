// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplicationComponentConfig` and the two reads that report what it stored, end to
//! end over HTTP.
//!
//! The rule under test is the one the operation states for ANY application component of
//! the portfolio, not for one request: every member a call names is written onto THAT
//! component's record and the reads beside it answer exactly what was written —
//! `GetApplicationComponentDetails` the `appType`, the `inclusionStatus`, the
//! `secretsManagerKey` (as the database configuration's secret name) and the
//! `sourceCodeList` (as the component's source code repositories, member for member and in
//! order), and `GetApplicationComponentStrategies` the `strategyOption` as the strategy the
//! account prefers — while a member the call leaves out is left exactly as it was, an
//! `applicationComponentId` naming no component is refused with 404
//! `ResourceNotFoundException`, and a value its enumeration does not declare (or one
//! outside a declared length bound) is refused with 400 `ValidationException`, storing
//! nothing.
//!
//! `configureOnly` is the one member that is not stored but acted on: it is documented "If
//! it is set to true, the source code and/or database credentials are updated. If it is set
//! to false, the source code and/or database credentials are updated and an analysis is
//! initiated", so a call that configures source code and/or database credentials leaves the
//! component's `SrcCodeOrDbAnalysisStatus` at `CONFIGURED` when it asked for configuration
//! only and at `ANALYSIS_TO_BE_SCHEDULED` when it asked for an analysis too, and a call that
//! configures neither leaves that status alone. Nothing in this emulator analyses source
//! code or a database, so the queued analysis never turns into a result — pinned below by
//! reading the component again after the span this service's clock-owed transitions use.
//!
//! Each table therefore spans the class the rule generalises over rather than one update:
//! every `AppType`, every `InclusionStatus`, every `VersionControl`, every `Strategy`,
//! every `TargetDestination`, every `TransformationToolName`, source code lists of zero,
//! one, two and three entries with members present and absent, a secret at each published
//! bound of `SecretsManagerKey` and one past it, `configureOnly` of either value and
//! absent, and a request naming nothing but the id.
//!
//! An application component enters this service only through the account's data being
//! brought in (the import-loop issue, filed separately), so these cases seed the component
//! records through the shared engine with the harness `application_component_details` uses
//! — the same `application_components` map an ingestion writes — and then drive every
//! assertion through the router, so the handlers, their refusals and the serialized shapes
//! are exercised exactly as a client reaches them.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ApplicationComponentRecord, COMPLETION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::application_component_details::{emulator_with, get_component};
use crate::enum_support::undeclared_values;
use crate::import_and_assess::{at, call, Answer};

/// This service's one sentence for a request carrying a value it does not accept, and its
/// one sentence for an id that resolves to no resource. Both spelled out here rather than
/// imported from the crate, so a wrong message fails this suite instead of agreeing with
/// itself.
const INVALID_REQUEST_BODY: &str = "Invalid request body";
const INVALID_RESOURCE_ID: &str = "Invalid resourceId: ";

/// The two `SrcCodeOrDbAnalysisStatus` values `configureOnly` chooses between, from
/// `smithy_query(query="shape",shape="SrcCodeOrDbAnalysisStatus")`, and one more this suite
/// seeds a component with so an untouched status is observable.
const CONFIGURED: &str = "CONFIGURED";
const ANALYSIS_TO_BE_SCHEDULED: &str = "ANALYSIS_TO_BE_SCHEDULED";
const ANALYSIS_SUCCESS: &str = "ANALYSIS_SUCCESS";

/// Every value `AppType` declares, from `smithy_query(query="shape",shape="AppType")`.
const APP_TYPES: &[&str] = &[
    "DotNetFramework",
    "Java",
    "SQLServer",
    "IIS",
    "Oracle",
    "Other",
    "Tomcat",
    "JBoss",
    "Spring",
    "Mongo DB",
    "DB2",
    "Maria DB",
    "MySQL",
    "Sybase",
    "PostgreSQLServer",
    "Cassandra",
    "IBM WebSphere",
    "Oracle WebLogic",
    "Visual Basic",
    "Unknown",
    "DotnetCore",
    "Dotnet",
];

/// Every value `InclusionStatus` declares, from
/// `smithy_query(query="shape",shape="InclusionStatus")`.
const INCLUSION_STATUSES: &[&str] = &["excludeFromAssessment", "includeInAssessment"];

/// Every value `VersionControl` declares — the enumeration a `SourceCode` entry's
/// `versionControl` targets, from `smithy_query(query="shape",shape="VersionControl")`.
const VERSION_CONTROLS: &[&str] = &["GITHUB", "GITHUB_ENTERPRISE", "AZURE_DEVOPS_GIT"];

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

/// The catalogs this suite draws a "legal value of the service, just not of this member"
/// row from, for the enum-refusal table.
const CATALOGS: &[&[&str]] = &[
    APP_TYPES,
    INCLUSION_STATUSES,
    STRATEGIES,
    TARGET_DESTINATIONS,
    TOOL_NAMES,
    VERSION_CONTROLS,
];

/// The published bounds of the request's length-constrained members, from
/// `smithy_query(query="operation",operation="UpdateApplicationComponentConfig")` and
/// `smithy_query(query="shape",shape="SourceCode")`: `SecretsManagerKey` is
/// `@length(min: 1, max: 512)`, `SourceVersion` `@length(min: 1, max: 40)` and `Location`
/// `@length(min: 1, max: 128)`.
const SECRET_MIN: usize = 1;
const SECRET_MAX: usize = 512;
const SOURCE_VERSION_MAX: usize = 40;
const LOCATION_MAX: usize = 128;

/// One enum-targeting member of this request: how to send a value in it, and the catalog
/// its target enumeration declares.
struct EnumMember {
    /// The member path, named in every assertion.
    member: &'static str,
    /// A request carrying the given value in that member, every other member legal.
    request: fn(&str) -> Value,
    /// Every value the member's target enumeration declares.
    declared: &'static [&'static str],
}

/// The id of the `row`th component a table seeds: a `ResourceId` (`^[0-9a-b]+`), which is
/// also what `ApplicationComponentId` (`[0-9a-zA-Z-]+`, at most 44 characters) admits.
fn id_for(row: usize) -> String {
    format!("{row:02}ab")
}

/// One `sourceCodeList` entry a request carries: the four members `SourceCode` declares,
/// each present or absent.
#[derive(Clone, Debug, Default)]
struct Source {
    version_control: Option<&'static str>,
    source_version: Option<String>,
    location: Option<String>,
    project_name: Option<String>,
}

impl Source {
    /// The entry as the request sends it, with every member it does not name left out.
    fn request(&self) -> Value {
        let mut entry = serde_json::Map::new();
        if let Some(version_control) = self.version_control {
            entry.insert("versionControl".to_string(), json!(version_control));
        }
        if let Some(source_version) = &self.source_version {
            entry.insert("sourceVersion".to_string(), json!(source_version));
        }
        if let Some(location) = &self.location {
            entry.insert("location".to_string(), json!(location));
        }
        if let Some(project_name) = &self.project_name {
            entry.insert("projectName".to_string(), json!(project_name));
        }
        Value::Object(entry)
    }

    /// Where each member of this entry has to turn up in the stored
    /// `SourceCodeRepository`, as `(member name, value)`.
    ///
    /// `SourceCode` and `SourceCodeRepository` document the same four things word for word
    /// — the repository name for the source code, its branch, the type of repository to
    /// use and the project name — so `location` is reported as `repository`,
    /// `sourceVersion` as `branch` and `versionControl` as `versionControlType`.
    fn reported(&self) -> Vec<(&'static str, Value)> {
        let text = |value: &Option<String>| match value {
            Some(value) => json!(value),
            None => Value::Null,
        };
        vec![
            ("repository", text(&self.location)),
            ("branch", text(&self.source_version)),
            (
                "versionControlType",
                self.version_control
                    .map_or(Value::Null, |value| json!(value)),
            ),
            ("projectName", text(&self.project_name)),
        ]
    }
}

/// The `row`th source code configuration of a table row: its `versionControl` walks the
/// `VersionControl` catalog and its optional members are present on some entries and
/// absent on others.
fn source(row: usize, entry: usize) -> Source {
    Source {
        version_control: Some(VERSION_CONTROLS[(row + entry) % VERSION_CONTROLS.len()]),
        source_version: entry
            .is_multiple_of(2)
            .then(|| format!("release-{row}-{entry}")),
        location: Some(format!("repo-{row}-{entry}")),
        project_name: (!entry.is_multiple_of(3)).then(|| format!("project-{row}")),
    }
}

/// One update: the members a request names, and so what the reads then have to report.
#[derive(Clone, Debug, Default)]
struct Case {
    what: String,
    app_type: Option<&'static str>,
    inclusion_status: Option<&'static str>,
    secret: Option<String>,
    /// `None` names no `sourceCodeList` at all; `Some(vec![])` names an empty one.
    sources: Option<Vec<Source>>,
    strategy: Option<&'static str>,
    destination: Option<&'static str>,
    tool: Option<&'static str>,
    is_preferred: Option<bool>,
    configure_only: Option<bool>,
}

impl Case {
    /// The request this case sends against `id`, with every member it does not name left
    /// out.
    fn request(&self, id: &str) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("applicationComponentId".to_string(), json!(id));
        if let Some(app_type) = self.app_type {
            body.insert("appType".to_string(), json!(app_type));
        }
        if let Some(inclusion_status) = self.inclusion_status {
            body.insert("inclusionStatus".to_string(), json!(inclusion_status));
        }
        if let Some(secret) = &self.secret {
            body.insert("secretsManagerKey".to_string(), json!(secret));
        }
        if let Some(sources) = &self.sources {
            let entries: Vec<Value> = sources.iter().map(Source::request).collect();
            body.insert("sourceCodeList".to_string(), Value::Array(entries));
        }
        if let Some(option) = self.strategy_option() {
            body.insert("strategyOption".to_string(), option);
        }
        if let Some(configure_only) = self.configure_only {
            body.insert("configureOnly".to_string(), json!(configure_only));
        }
        Value::Object(body)
    }

    /// The `strategyOption` this case sends, or `None` when it names no member of one.
    fn strategy_option(&self) -> Option<Value> {
        let mut option = serde_json::Map::new();
        if let Some(strategy) = self.strategy {
            option.insert("strategy".to_string(), json!(strategy));
        }
        if let Some(destination) = self.destination {
            option.insert("targetDestination".to_string(), json!(destination));
        }
        if let Some(tool) = self.tool {
            option.insert("toolName".to_string(), json!(tool));
        }
        if let Some(is_preferred) = self.is_preferred {
            option.insert("isPreferred".to_string(), json!(is_preferred));
        }
        (!option.is_empty()).then_some(Value::Object(option))
    }

    /// Whether the option this case sends selects a strategy, so there is a preference to
    /// store: `isPreferred` alone selects none.
    fn names_a_strategy(&self) -> bool {
        self.strategy.is_some() || self.destination.is_some() || self.tool.is_some()
    }

    /// `isPreferred` is "set to true if the recommendation is set as preferred", so an
    /// option stored through this operation is preferred unless the call said otherwise.
    fn reported_as_preferred(&self) -> bool {
        self.is_preferred.unwrap_or(true)
    }

    /// Whether this request configures the source code and/or the database credentials —
    /// the two things `configureOnly` chooses between configuring and configuring then
    /// analysing.
    fn configures_anything(&self) -> bool {
        self.sources.is_some() || self.secret.is_some()
    }

    /// The `analysisStatus` the component carries after this update, given the one it
    /// carried before.
    fn expected_analysis_status(&self, before: Option<&str>) -> Value {
        if !self.configures_anything() {
            return before.map_or(Value::Null, |status| json!(status));
        }
        match self.configure_only {
            Some(true) => json!(CONFIGURED),
            _ => json!(ANALYSIS_TO_BE_SCHEDULED),
        }
    }
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

/// How many strategies a read reported.
fn reported_strategy_count(answer: &Answer) -> usize {
    match at(&answer.body, "/applicationComponentStrategies") {
        Value::Array(items) => items.len(),
        _ => 0,
    }
}

/// The table every round-trip case runs: one row per `AppType`, crossing the other
/// dimensions at their own periods, then one row per remaining catalog value, the published
/// length bounds, a request naming nothing but the id and one naming an empty source code
/// list.
fn table() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for (index, app_type) in APP_TYPES.iter().enumerate() {
        cases.push(Case {
            what: format!(
                "appType {app_type:?} with {} source code entr(ies)",
                index % 4
            ),
            app_type: Some(app_type),
            inclusion_status: Some(INCLUSION_STATUSES[index % INCLUSION_STATUSES.len()]),
            secret: index
                .is_multiple_of(3)
                .then(|| format!("database-secret-{index}")),
            sources: Some((0..index % 4).map(|entry| source(index, entry)).collect()),
            strategy: Some(STRATEGIES[index % STRATEGIES.len()]),
            destination: Some(TARGET_DESTINATIONS[index % TARGET_DESTINATIONS.len()]),
            tool: Some(TOOL_NAMES[index % TOOL_NAMES.len()]),
            is_preferred: match index % 3 {
                0 => None,
                1 => Some(true),
                _ => Some(false),
            },
            configure_only: match index % 3 {
                0 => None,
                1 => Some(true),
                _ => Some(false),
            },
        });
    }
    for (index, destination) in TARGET_DESTINATIONS.iter().enumerate() {
        cases.push(Case {
            what: format!("the destination {destination:?} alone"),
            destination: Some(destination),
            ..Default::default()
        });
        let _ = index;
    }
    for tool in TOOL_NAMES {
        cases.push(Case {
            what: format!("the tool {tool:?} with a strategy"),
            strategy: Some("Rehost"),
            tool: Some(tool),
            ..Default::default()
        });
    }
    for version_control in VERSION_CONTROLS {
        cases.push(Case {
            what: format!("one source code entry under {version_control:?}"),
            sources: Some(vec![Source {
                version_control: Some(version_control),
                source_version: Some("main".to_string()),
                location: Some("https://example.invalid/repo".to_string()),
                project_name: Some("project".to_string()),
            }]),
            configure_only: Some(true),
            ..Default::default()
        });
    }
    cases.push(Case {
        what: format!("a secret at the {SECRET_MIN}-character minimum"),
        secret: Some("k".repeat(SECRET_MIN)),
        ..Default::default()
    });
    cases.push(Case {
        what: format!("a secret at the {SECRET_MAX}-character maximum"),
        secret: Some("k".repeat(SECRET_MAX)),
        configure_only: Some(true),
        ..Default::default()
    });
    cases.push(Case {
        what: "a source code entry at the published maxima".to_string(),
        sources: Some(vec![Source {
            version_control: Some("GITHUB"),
            source_version: Some("v".repeat(SOURCE_VERSION_MAX)),
            location: Some("l".repeat(LOCATION_MAX)),
            project_name: Some("project".to_string()),
        }]),
        ..Default::default()
    });
    cases.push(Case {
        what: "a source code entry naming nothing but a location".to_string(),
        sources: Some(vec![Source {
            location: Some("bare-repo".to_string()),
            ..Default::default()
        }]),
        ..Default::default()
    });
    cases.push(Case {
        what: "an empty sourceCodeList".to_string(),
        sources: Some(Vec::new()),
        ..Default::default()
    });
    cases.push(Case {
        what: "an option carrying only isPreferred".to_string(),
        is_preferred: Some(true),
        ..Default::default()
    });
    cases.push(Case {
        what: "a request naming nothing but the component id".to_string(),
        ..Default::default()
    });
    cases
}

/// Assert the detail read reports exactly what `case` stored against a component that held
/// nothing but its id.
fn assert_detail(answer: &Answer, case: &Case, id: &str) {
    let what = &case.what;
    let prefix = "/applicationComponentDetail";
    let expected = |value: Option<&str>| value.map_or(Value::Null, |value| json!(value));
    let secret = case.secret.as_deref();
    for (pointer, expected) in [
        (format!("{prefix}/id"), json!(id)),
        (format!("{prefix}/appType"), expected(case.app_type)),
        (
            format!("{prefix}/inclusionStatus"),
            expected(case.inclusion_status),
        ),
        (
            format!("{prefix}/databaseConfigDetail/secretName"),
            expected(secret),
        ),
        (
            format!("{prefix}/analysisStatus"),
            case.expected_analysis_status(None),
        ),
        // The update stores a preference, never the service's own recommendation: a
        // component nothing analysed still reports none.
        (format!("{prefix}/recommendationSet"), Value::Null),
    ] {
        let actual = at(&answer.body, &pointer);
        assert_eq!(
            actual, expected,
            "case {what:?}: expected {pointer} to be {expected:?}, got {actual:?} in {:?}",
            answer.body
        );
    }

    match case.sources.as_deref() {
        None | Some([]) => {
            let actual = at(&answer.body, &format!("{prefix}/sourceCodeRepositories"));
            assert_eq!(
                actual,
                Value::Null,
                "case {what:?}: a component configured with no repository reports none, got \
                 {actual:?}"
            );
        }
        Some(entries) => {
            let reported = at(&answer.body, &format!("{prefix}/sourceCodeRepositories"));
            let count = match &reported {
                Value::Array(items) => items.len(),
                _ => 0,
            };
            assert_eq!(
                count,
                entries.len(),
                "case {what:?}: expected {} stored repositor(ies), got {reported:?}",
                entries.len()
            );
            for (index, entry) in entries.iter().enumerate() {
                for (member, expected) in entry.reported() {
                    let pointer = format!("{prefix}/sourceCodeRepositories/{index}/{member}");
                    let actual = at(&answer.body, &pointer);
                    assert_eq!(
                        actual, expected,
                        "case {what:?}: expected {pointer} to be {expected:?}, got {actual:?}"
                    );
                }
            }
        }
    }
}

/// Assert the strategies read reports exactly the preference `case` stored against a
/// component nothing recommended anything for.
fn assert_preference(answer: &Answer, case: &Case) {
    let what = &case.what;
    if !case.names_a_strategy() {
        assert_eq!(
            reported_strategy_count(answer),
            0,
            "case {what:?}: nothing selected a strategy, so the component holds none, got {:?}",
            answer.body
        );
        return;
    }
    assert_eq!(
        reported_strategy_count(answer),
        1,
        "case {what:?}: the stored option is the component's one strategy, got {:?}",
        answer.body
    );
    let expected = |value: Option<&str>| value.map_or(Value::Null, |value| json!(value));
    for (member, expected) in [
        ("recommendation/strategy", expected(case.strategy)),
        (
            "recommendation/targetDestination",
            expected(case.destination),
        ),
        ("recommendation/transformationTool/name", expected(case.tool)),
        // The caller named a tool, not a tool catalog entry.
        ("recommendation/transformationTool/description", Value::Null),
        (
            "recommendation/transformationTool/tranformationToolInstallationLink",
            Value::Null,
        ),
        ("isPreferred", json!(case.reported_as_preferred())),
        // Nothing recommended the caller's own option, so it carries no
        // StrategyRecommendation status.
        ("status", Value::Null),
    ] {
        let pointer = format!("/applicationComponentStrategies/0/{member}");
        let actual = at(&answer.body, &pointer);
        assert_eq!(
            actual, expected,
            "case {what:?}: expected {pointer} to be {expected:?}, got {actual:?} in {:?}",
            answer.body
        );
    }
}

/// Every JSON pointer at which `value` holds a leaf (a scalar, a null, or an empty
/// container), so two answers can be compared position by position rather than member by
/// member.
fn leaf_pointers(value: &Value, prefix: &str, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(members) if !members.is_empty() => {
            for (name, member) in members {
                leaf_pointers(member, &format!("{prefix}/{name}"), out);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                leaf_pointers(item, &format!("{prefix}/{index}"), out);
            }
        }
        _ => {
            out.insert(prefix.to_string());
        }
    }
}

/// Assert two answers agree at every position except the ones under `allowed`.
fn assert_unchanged_except(what: &str, before: &Answer, after: &Answer, allowed: &[&str]) {
    let mut pointers: BTreeSet<String> = BTreeSet::new();
    leaf_pointers(&before.body, "", &mut pointers);
    leaf_pointers(&after.body, "", &mut pointers);
    for pointer in pointers {
        if allowed.iter().any(|prefix| pointer.starts_with(prefix)) {
            continue;
        }
        let was = at(&before.body, &pointer);
        let now = at(&after.body, &pointer);
        assert_eq!(
            was, now,
            "case {what:?}: expected {pointer} to be left as it was ({was:?}), got {now:?}"
        );
    }
}

/// A component the account's data already brought in, holding something in every member an
/// update can write and in the ones it must leave alone.
fn preconfigured(id: &str) -> ApplicationComponentRecord {
    ApplicationComponentRecord {
        id: id.to_string(),
        name: Some("payments-api".to_string()),
        app_type: Some("Java".to_string()),
        resource_sub_type: Some("Process".to_string()),
        inclusion_status: Some("includeInAssessment".to_string()),
        analysis_status: Some(ANALYSIS_SUCCESS.to_string()),
        status_message: Some("the ingestion's own note".to_string()),
        strategy: Some("Replatform".to_string()),
        target_destination: Some("AWS Fargate".to_string()),
        os_version: Some("Ubuntu 20.04".to_string()),
        os_driver: Some("driver-1".to_string()),
        database_secret_name: Some("ingested-secret".to_string()),
        runtime_status: Some("ANALYSIS_SUCCESS".to_string()),
        associated_server_ids: vec!["0b".to_string(), "1b".to_string()],
        ..Default::default()
    }
}

/// Every update stores the members it named against its own component, and both reads
/// report exactly those members back — over every catalog value and every list size the
/// rule generalises over.
///
/// All of the components are updated before any of them is read, so a row that reported
/// another row's configuration (or that overwrote it) fails here.
#[tokio::test]
async fn an_update_stores_every_member_the_reads_report() {
    let cases = table();

    // The table really does span each catalog the request draws on.
    for (catalog, name, covered) in [
        (
            APP_TYPES,
            "AppType",
            cases
                .iter()
                .filter_map(|case| case.app_type)
                .collect::<BTreeSet<&str>>(),
        ),
        (
            INCLUSION_STATUSES,
            "InclusionStatus",
            cases.iter().filter_map(|case| case.inclusion_status).collect(),
        ),
        (
            STRATEGIES,
            "Strategy",
            cases.iter().filter_map(|case| case.strategy).collect(),
        ),
        (
            TARGET_DESTINATIONS,
            "TargetDestination",
            cases.iter().filter_map(|case| case.destination).collect(),
        ),
        (
            TOOL_NAMES,
            "TransformationToolName",
            cases.iter().filter_map(|case| case.tool).collect(),
        ),
        (
            VERSION_CONTROLS,
            "VersionControl",
            cases
                .iter()
                .flat_map(|case| case.sources.iter().flatten())
                .filter_map(|source| source.version_control)
                .collect(),
        ),
    ] {
        for value in catalog {
            assert!(
                covered.contains(value),
                "the table must cover the whole {name} catalog, and {value:?} is missing from \
                 {covered:?}"
            );
        }
    }

    let router = emulator_with(
        (0..cases.len())
            .map(|row| ApplicationComponentRecord {
                id: id_for(row),
                ..Default::default()
            })
            .collect(),
    );

    for (row, case) in cases.iter().enumerate() {
        let answer = update(&router, case.request(&id_for(row))).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the update of a component of the portfolio succeeds, got {} with {:?}",
            case.what, answer.status, answer.body
        );
        assert_eq!(
            answer.body,
            Value::Object(serde_json::Map::new()),
            "case {:?}: UpdateApplicationComponentConfigResponse models no member, got {:?}",
            case.what,
            answer.body
        );
    }

    for (row, case) in cases.iter().enumerate() {
        let id = id_for(row);
        let detail = get_component(&router, &id).await;
        assert_eq!(
            detail.status, 200,
            "case {:?}: the detail read succeeds, got {} with {:?}",
            case.what, detail.status, detail.body
        );
        assert_detail(&detail, case, &id);

        let strategies = get_strategies(&router, &id).await;
        assert_eq!(
            strategies.status, 200,
            "case {:?}: the strategies read succeeds, got {} with {:?}",
            case.what, strategies.status, strategies.body
        );
        assert_preference(&strategies, case);
    }
}

/// A member the request leaves out is left exactly as it was: each round names ONE member
/// and nothing else in either read moves, beyond the analysis status the documented
/// `configureOnly` rule owes a request that configured source code or credentials.
#[tokio::test]
async fn a_member_the_request_leaves_out_is_left_as_it_was() {
    let id = "0ab";
    let router = emulator_with(vec![preconfigured(id)]);
    let detail_prefix = "/applicationComponentDetail";

    let rounds: Vec<(&str, Value, Vec<String>, Vec<String>)> = vec![
        (
            "only appType",
            json!({"appType": "Tomcat"}),
            vec![format!("{detail_prefix}/appType")],
            Vec::new(),
        ),
        (
            "only inclusionStatus",
            json!({"inclusionStatus": "excludeFromAssessment"}),
            vec![format!("{detail_prefix}/inclusionStatus")],
            Vec::new(),
        ),
        (
            "only secretsManagerKey",
            json!({"secretsManagerKey": "rotated-secret"}),
            vec![
                format!("{detail_prefix}/databaseConfigDetail"),
                format!("{detail_prefix}/analysisStatus"),
            ],
            Vec::new(),
        ),
        (
            "only sourceCodeList",
            json!({"sourceCodeList": [{"versionControl": "GITHUB", "location": "repo-9"}]}),
            vec![
                format!("{detail_prefix}/sourceCodeRepositories"),
                format!("{detail_prefix}/analysisStatus"),
            ],
            Vec::new(),
        ),
        (
            "only strategyOption",
            json!({"strategyOption": {"strategy": "Retain"}}),
            Vec::new(),
            vec!["/applicationComponentStrategies".to_string()],
        ),
        (
            "only configureOnly, which configures nothing",
            json!({"configureOnly": true}),
            Vec::new(),
            Vec::new(),
        ),
        (
            "nothing but the component id",
            json!({}),
            Vec::new(),
            Vec::new(),
        ),
    ];

    for (what, mut request, detail_changes, strategy_changes) in rounds {
        let detail_before = get_component(&router, id).await;
        let strategies_before = get_strategies(&router, id).await;

        request["applicationComponentId"] = json!(id);
        let answer = update(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: the update succeeds, got {} with {:?}",
            answer.status, answer.body
        );

        let detail_after = get_component(&router, id).await;
        let strategies_after = get_strategies(&router, id).await;
        let detail_allowed: Vec<&str> = detail_changes.iter().map(String::as_str).collect();
        let strategy_allowed: Vec<&str> = strategy_changes.iter().map(String::as_str).collect();
        assert_unchanged_except(what, &detail_before, &detail_after, &detail_allowed);
        assert_unchanged_except(
            what,
            &strategies_before,
            &strategies_after,
            &strategy_allowed,
        );

        // The member the request DID name took the value it named.
        if let Some(app_type) = request.get("appType") {
            assert_eq!(
                at(&detail_after.body, &format!("{detail_prefix}/appType")),
                *app_type,
                "case {what:?}: expected the named appType to be stored, got {:?}",
                detail_after.body
            );
        }
        if let Some(inclusion_status) = request.get("inclusionStatus") {
            assert_eq!(
                at(
                    &detail_after.body,
                    &format!("{detail_prefix}/inclusionStatus")
                ),
                *inclusion_status,
                "case {what:?}: expected the named inclusionStatus to be stored, got {:?}",
                detail_after.body
            );
        }
        if let Some(secret) = request.get("secretsManagerKey") {
            assert_eq!(
                at(
                    &detail_after.body,
                    &format!("{detail_prefix}/databaseConfigDetail/secretName")
                ),
                *secret,
                "case {what:?}: expected the named secret to be stored, got {:?}",
                detail_after.body
            );
        }
        if let Some(sources) = request.get("sourceCodeList").and_then(Value::as_array) {
            assert_eq!(
                at(
                    &detail_after.body,
                    &format!("{detail_prefix}/sourceCodeRepositories/0/repository")
                ),
                sources[0]["location"],
                "case {what:?}: expected the named source code entry to be stored, got {:?}",
                detail_after.body
            );
        }
    }
}

/// `configureOnly` decides the component's source-code-or-database analysis status, and
/// only for a request that configures one of them.
#[tokio::test]
async fn configure_only_decides_the_analysis_status() {
    let entry = Source {
        version_control: Some("GITHUB"),
        source_version: Some("main".to_string()),
        location: Some("https://example.invalid/repo".to_string()),
        project_name: None,
    };

    let mut cases: Vec<Case> = Vec::new();
    for configure_only in [None, Some(true), Some(false)] {
        for (configures, secret, sources) in [
            ("neither", None, None),
            (
                "the database credentials",
                Some("a-secret".to_string()),
                None,
            ),
            ("the source code", None, Some(vec![entry.clone()])),
            (
                "an empty source code list",
                None,
                Some(Vec::<Source>::new()),
            ),
            (
                "both",
                Some("a-secret".to_string()),
                Some(vec![entry.clone()]),
            ),
        ] {
            cases.push(Case {
                what: format!("configureOnly {configure_only:?} configuring {configures}"),
                secret,
                sources,
                configure_only,
                ..Default::default()
            });
        }
    }

    let router = emulator_with(
        (0..cases.len())
            .map(|row| ApplicationComponentRecord {
                id: id_for(row),
                // A status the ingestion recorded, so a row the rule leaves alone is
                // observably left alone rather than merely absent.
                analysis_status: Some(ANALYSIS_SUCCESS.to_string()),
                ..Default::default()
            })
            .collect(),
    );

    for (row, case) in cases.iter().enumerate() {
        let id = id_for(row);
        let answer = update(&router, case.request(&id)).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the update succeeds, got {} with {:?}",
            case.what, answer.status, answer.body
        );

        let detail = get_component(&router, &id).await;
        let expected = case.expected_analysis_status(Some(ANALYSIS_SUCCESS));
        let actual = at(&detail.body, "/applicationComponentDetail/analysisStatus");
        assert_eq!(
            actual, expected,
            "case {:?}: expected analysisStatus {expected:?}, got {actual:?} in {:?}",
            case.what, detail.body
        );
    }
}

/// An analysis this emulator queued is never turned into a result: nothing here analyses
/// source code or a database, so the status stays where the update put it however long a
/// caller waits — well past the span this service's clock-owed transitions use.
#[tokio::test]
async fn a_queued_analysis_never_becomes_a_result() {
    let id = "0ab";
    let router = emulator_with(vec![ApplicationComponentRecord {
        id: id.to_string(),
        ..Default::default()
    }]);

    let answer = update(
        &router,
        json!({
            "applicationComponentId": id,
            "secretsManagerKey": "a-secret",
            "sourceCodeList": [{"versionControl": "GITHUB", "location": "repo"}],
            "configureOnly": false,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the update that initiates an analysis succeeds, got {} with {:?}",
        answer.status, answer.body
    );

    let pointer = "/applicationComponentDetail/analysisStatus";
    let straight_after = get_component(&router, id).await;
    assert_eq!(
        at(&straight_after.body, pointer),
        json!(ANALYSIS_TO_BE_SCHEDULED),
        "the analysis is queued, got {:?}",
        straight_after.body
    );

    tokio::time::sleep(COMPLETION_SPAN + Duration::from_millis(200)).await;
    let later = get_component(&router, id).await;
    assert_eq!(
        at(&later.body, pointer),
        json!(ANALYSIS_TO_BE_SCHEDULED),
        "nothing analyses source code or a database here, so no result appears, got {:?}",
        later.body
    );
    assert_eq!(
        at(&later.body, "/applicationComponentDetail/resultList"),
        Value::Null,
        "no analysis ran, so no result is reported, got {:?}",
        later.body
    );
}

/// An `applicationComponentId` that names no component of this account's portfolio is
/// refused, whatever the id looks like — and the update neither creates it nor touches the
/// component beside it.
#[tokio::test]
async fn an_id_naming_no_component_is_refused_and_creates_nothing() {
    let known = "0ab";
    let router = emulator_with(vec![preconfigured(known)]);
    let before = get_component(&router, known).await;

    for (what, id) in [
        ("an id no record was seeded under", "9999a"),
        ("a one-character id", "b"),
        (
            "an id ApplicationComponentId admits that no ResourceId could be",
            "App-Component-01",
        ),
        ("an id at the 44-character maximum", &"0".repeat(44)[..]),
    ] {
        let answer = update(
            &router,
            json!({
                "applicationComponentId": id,
                "appType": "Java",
                "inclusionStatus": "includeInAssessment",
                "secretsManagerKey": "a-secret",
                "strategyOption": {"strategy": "Rehost"},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {what:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {what:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_RESOURCE_ID,
            "case {what:?}: expected the service's {INVALID_RESOURCE_ID:?}, got {:?}",
            answer.body
        );

        let still_missing = get_component(&router, id).await;
        assert_eq!(
            still_missing.status, 404,
            "case {what:?}: the refused update must not create the component, got {} with {:?}",
            still_missing.status, still_missing.body
        );
    }

    let after = get_component(&router, known).await;
    assert_unchanged_except("the component beside the refusals", &before, &after, &[]);
}

/// A value its own enumeration does not declare, or one outside a declared length bound, is
/// refused with the service's one 400 — and the component is left exactly as it was, even
/// when every other member of the same request was legal.
#[tokio::test]
async fn a_value_this_service_does_not_accept_is_refused_and_stores_nothing() {
    let id = "0ab";
    let router = emulator_with(vec![preconfigured(id)]);
    let detail_before = get_component(&router, id).await;
    let strategies_before = get_strategies(&router, id).await;

    // Each enum-targeting member of this request, from
    // `smithy_query(query="operation",operation="UpdateApplicationComponentConfig")` read
    // against `smithy_query(query="shape",shape="StrategyOption,SourceCode")`.
    let members: Vec<EnumMember> = vec![
        EnumMember {
            member: "appType",
            request: |value| json!({"appType": value}),
            declared: APP_TYPES,
        },
        EnumMember {
            member: "inclusionStatus",
            request: |value| json!({"inclusionStatus": value}),
            declared: INCLUSION_STATUSES,
        },
        EnumMember {
            member: "strategyOption.strategy",
            request: |value| json!({"appType": "Java", "strategyOption": {"strategy": value}}),
            declared: STRATEGIES,
        },
        EnumMember {
            member: "strategyOption.targetDestination",
            request: |value| json!({"strategyOption": {"targetDestination": value}}),
            declared: TARGET_DESTINATIONS,
        },
        EnumMember {
            member: "strategyOption.toolName",
            request: |value| json!({"strategyOption": {"toolName": value}}),
            declared: TOOL_NAMES,
        },
        EnumMember {
            member: "sourceCodeList[0].versionControl",
            request: |value| {
                json!({
                    "secretsManagerKey": "a-secret",
                    "sourceCodeList": [{"versionControl": value, "location": "repo"}],
                })
            },
            declared: VERSION_CONTROLS,
        },
    ];

    for EnumMember {
        member,
        request,
        declared,
    } in members
    {
        let rows = undeclared_values(declared, CATALOGS);
        assert!(
            !rows.is_empty(),
            "case {member:?}: the table must probe at least one undeclared value"
        );
        for (shape, value) in rows {
            let what = format!("{member} = {value:?} ({shape})");
            let mut body = request(&value);
            body["applicationComponentId"] = json!(id);
            let answer = update(&router, body).await;
            assert_eq!(
                answer.status, 400,
                "case {what:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {what:?}: expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.string("/message"),
                INVALID_REQUEST_BODY,
                "case {what:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
                answer.body
            );
        }
    }

    // The published bounds of `SecretsManagerKey`: a secret one character past either end
    // is refused the same way, so a bound is a bound rather than a suggestion.
    for (what, secret) in [
        ("a secret one character short of the minimum", String::new()),
        (
            "a secret one character past the maximum",
            "k".repeat(SECRET_MAX + 1),
        ),
    ] {
        let answer = update(
            &router,
            json!({"applicationComponentId": id, "secretsManagerKey": secret}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_REQUEST_BODY,
            "case {what:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
            answer.body
        );
    }

    let detail_after = get_component(&router, id).await;
    let strategies_after = get_strategies(&router, id).await;
    assert_unchanged_except(
        "a component only refused requests reached",
        &detail_before,
        &detail_after,
        &[],
    );
    assert_unchanged_except(
        "a component only refused requests reached",
        &strategies_before,
        &strategies_after,
        &[],
    );
}

/// The preference an update stores is reported BESIDE the service's own recommendation: a
/// different option is a second strategy, and an option naming the same strategy and
/// destination is one strategy carrying the recommended status.
#[tokio::test]
async fn the_preference_is_reported_beside_the_services_own_recommendation() {
    let id = "0ab";
    let router = emulator_with(vec![preconfigured(id)]);
    let recommended = "recommended";

    // The seeded component is recommended Replatform to AWS Fargate.
    let before = get_strategies(&router, id).await;
    assert_eq!(
        reported_strategy_count(&before),
        1,
        "before any update the component holds only the service's recommendation, got {:?}",
        before.body
    );
    assert_eq!(
        at(&before.body, "/applicationComponentStrategies/0/status"),
        json!(recommended),
        "the service's own recommendation carries the recommended status, got {:?}",
        before.body
    );
    assert_eq!(
        at(&before.body, "/applicationComponentStrategies/0/isPreferred"),
        json!(false),
        "nobody chose it, so it is not preferred, got {:?}",
        before.body
    );

    // A preference naming ANOTHER strategy is a second one, reported first.
    let answer = update(
        &router,
        json!({
            "applicationComponentId": id,
            "strategyOption": {"strategy": "Rehost", "targetDestination": "Aurora MySQL"},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the update succeeds, got {} with {:?}",
        answer.status, answer.body
    );
    let two = get_strategies(&router, id).await;
    assert_eq!(
        reported_strategy_count(&two),
        2,
        "a preference the service did not recommend is a second strategy, got {:?}",
        two.body
    );
    for (pointer, expected) in [
        ("/applicationComponentStrategies/0/recommendation/strategy", json!("Rehost")),
        ("/applicationComponentStrategies/0/isPreferred", json!(true)),
        ("/applicationComponentStrategies/0/status", Value::Null),
        (
            "/applicationComponentStrategies/1/recommendation/strategy",
            json!("Replatform"),
        ),
        ("/applicationComponentStrategies/1/isPreferred", json!(false)),
        ("/applicationComponentStrategies/1/status", json!(recommended)),
    ] {
        let actual = at(&two.body, pointer);
        assert_eq!(
            actual, expected,
            "expected {pointer} to be {expected:?}, got {actual:?} in {:?}",
            two.body
        );
    }

    // A preference naming the strategy the service recommends is ONE strategy, carrying
    // both.
    let answer = update(
        &router,
        json!({
            "applicationComponentId": id,
            "strategyOption": {"strategy": "Replatform", "targetDestination": "AWS Fargate"},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the second update succeeds, got {} with {:?}",
        answer.status, answer.body
    );
    let one = get_strategies(&router, id).await;
    assert_eq!(
        reported_strategy_count(&one),
        1,
        "the preference and the recommendation are one strategy, got {:?}",
        one.body
    );
    for (pointer, expected) in [
        (
            "/applicationComponentStrategies/0/recommendation/strategy",
            json!("Replatform"),
        ),
        ("/applicationComponentStrategies/0/isPreferred", json!(true)),
        ("/applicationComponentStrategies/0/status", json!(recommended)),
    ] {
        let actual = at(&one.body, pointer);
        assert_eq!(
            actual, expected,
            "expected {pointer} to be {expected:?}, got {actual:?} in {:?}",
            one.body
        );
    }
}

/// The two reads change nothing a later caller sees: reading a component twice answers the
/// same thing, and reading one that holds no strategy does not bring one into being.
#[tokio::test]
async fn reading_the_strategies_changes_nothing() {
    let bare = "1ab";
    let configured = "0ab";
    let router = emulator_with(vec![
        preconfigured(configured),
        ApplicationComponentRecord {
            id: bare.to_string(),
            ..Default::default()
        },
    ]);

    for id in [configured, bare] {
        let first = get_strategies(&router, id).await;
        let again = get_strategies(&router, id).await;
        assert_eq!(
            first.status, 200,
            "component {id:?}: the read succeeds, got {} with {:?}",
            first.status, first.body
        );
        assert_eq!(
            first.body, again.body,
            "component {id:?}: expected two identical reads to answer the same thing, got {:?} \
             then {:?}",
            first.body, again.body
        );
    }
    assert_eq!(
        reported_strategy_count(&get_strategies(&router, bare).await),
        0,
        "a component nothing recommended and nobody chose for holds no strategy"
    );

    let missing = get_strategies(&router, "7777b").await;
    assert_eq!(
        missing.status, 404,
        "an id naming no component is a 404, got {} with {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.string("/message"),
        INVALID_RESOURCE_ID,
        "expected the service's {INVALID_RESOURCE_ID:?}, got {:?}",
        missing.body
    );
}
