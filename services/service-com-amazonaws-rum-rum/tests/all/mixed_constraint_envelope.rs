// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a request that breaks modelled constraints of BOTH kinds at
//! once.
//!
//! The rule pinned: a constraint refusal of this service states one clause per failing
//! (member, requirement) in ONE envelope, whichever layer applied the constraint — the
//! framework's boundary walk, which runs in front of every handler, or this service itself,
//! which applies the constraints that walk leaves unapplied (a `@pattern` written with
//! look-around, and every constraint a shape reached as a map KEY declares). The clauses are
//! stated in the order the input DECLARES the members they are about, and nothing is
//! written. So a caller that fixes the clauses it was told about is not refused a second
//! time on a clause the first answer withheld, which is what the second request of every row
//! below checks: with the boundary-applied member repaired, the answer is exactly the clauses
//! the first answer already stated for the other kind.
//!
//! The rows differ along what the rule generalises over: the operation (four app-monitor
//! operations and one of the ARN-addressed tagging frontend, which words a refusal in its own
//! error vocabulary), which kind of constraint the boundary applies (a `@range` at either
//! bound, a `@length` on a string, on a list ITSELF and on a list's element, an enum value
//! set), which kind this service applies (a look-around `@pattern` on a scalar member, on a
//! list's element, on a map's VALUE, and a `@length` on a map's KEY), whether the member this
//! service judges is declared BEFORE or AFTER the one the boundary refuses, whether the two
//! clauses sit at one member or at two, and how many clauses one envelope carries (two and
//! three).
//!
//! Every pattern, bound and value set below is cited from the model
//! (`smithy_query(query="shape", shape=…)` and `query="operation"` against
//! `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`) and is
//! spelled out here rather than read back out of the crate, so the test pins the rendering
//! instead of agreeing with it by construction.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, emulator, message, percent_encoded, Reply, ACCOUNT, REGION};

/// The wire error code an app-monitor call refused on a modelled constraint answers with:
/// the `ValidationException` this service's model declares (`@httpError(400)`).
const VALIDATION_CODE: &str = "ValidationException";

/// The wire error code RUM's ARN-addressed tagging frontend answers a refused request with.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The app monitor every row addresses, created before the table runs.
const MONITOR: &str = "mixed-clauses";

/// `AppMonitorDomain`'s `@pattern` as a refusal of this service quotes it: the modelled
/// expression without the `^` / `$` pair the model writes around it to demand a whole-string
/// match. Source: `smithy_query(query="shape", shape="AppMonitorDomain")`.
const DOMAIN_PATTERN: &str = r"(localhost)$|^((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$|(?=^[a-zA-Z0-9\.\*-]{4,253}$)(?!.*\.-)(?!.*-\.)(?!.*\.\.)(?!.*[^\.]{64,})^(\*\.)?(?![-\.\*])[^\*]{1,}\.(\*|(?!.*--)(?=.*[a-zA-Z])[^\*]{1,}[^\*-])";

/// `AppMonitorName`'s `@pattern`, quoted the same way.
/// Source: `smithy_query(query="shape", shape="AppMonitorName")`.
const NAME_PATTERN: &str = r"(?!\.)[\.\-_#A-Za-z0-9]+";

/// `TagKey`'s `@pattern`, quoted the same way, for the app-monitor frontend's clause.
/// Source: `smithy_query(query="shape", shape="TagMap")`, key.
const TAG_KEY_PATTERN: &str = r"(?!aws:)[a-zA-Z+-=._:/]+";

/// `TagKey`'s `@pattern` as the MODEL declares it — anchored. The ARN-addressed tagging
/// frontend keeps the framework's own clause, which quotes the expression as the model
/// writes it. Source: the same shape.
const MODELLED_TAG_KEY_PATTERN: &str = r"^(?!aws:)[a-zA-Z+-=._:/]+$";

/// `DimensionName`'s `@pattern`. It carries no `^`/`$` pair around the WHOLE expression
/// (there is no trailing `$`), so a refusal quotes it verbatim.
/// Source: `smithy_query(query="shape", shape="DimensionKeysMap")`, value.
const DIMENSION_NAME_PATTERN: &str = r"^(?!:).*[^\s].*";

/// The modelled bounds the rows below stand at one side of.
/// Sources: `smithy_query(query="shape", shape="AppMonitorDomain,AppMonitorDomainList,
/// TagMap,DimensionKeysMap,MetricDefinitionRequest")` and
/// `query="operation", operation="UpdateRumMetricDefinition"`.
const MAX_DOMAINS: usize = 5;
const MIN_DOMAIN_LENGTH: usize = 1;
const MAX_DOMAIN_LENGTH: usize = 253;
const MAX_TAG_VALUE_LENGTH: usize = 256;
const MAX_TAG_KEY_LENGTH: usize = 128;
const MAX_DIMENSION_KEY_LENGTH: usize = 280;
const MIN_METRIC_DEFINITION_ID_LENGTH: usize = 1;

/// `MetricDestination`'s admitted values in the order this service's refusals list them.
/// Source: `smithy_query(query="shape", shape="MetricDestination")` for the membership.
const METRIC_DESTINATION_LISTED: [&str; 2] = ["Evidently", "CloudWatch"];

/// One clause of the envelope: the value submitted, the member's dotted path and the
/// requirement it failed.
fn clause(value: &str, member: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The whole message an envelope over `clauses` states, in that order.
fn envelope(clauses: &[String]) -> String {
    let plural = if clauses.len() == 1 { "error" } else { "errors" };
    format!(
        "{} validation {plural} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// The requirement a `@pattern` violation states.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// The requirement a `@length` maximum states.
fn max_length(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement a `@length` minimum states.
fn min_length(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The requirement a `@range` maximum states, as the bound renders.
fn max_value(max: &str) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The requirement a `@range` minimum states.
fn min_value(min: &str) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement an enum violation states, over the values this service lists.
fn enum_set() -> String {
    format!(
        "Member must satisfy enum value set: [{}]",
        METRIC_DESTINATION_LISTED.join(", ")
    )
}

/// The whole constraint set `AppMonitorDomain` declares, as a clause at a LIST member
/// brackets it: the `@length` maximum, its minimum, then the `@pattern`.
fn domain_element_set() -> String {
    format!(
        "Member must satisfy constraint: [{}, {}, {}]",
        max_length(MAX_DOMAIN_LENGTH),
        min_length(MIN_DOMAIN_LENGTH),
        pattern_requirement(DOMAIN_PATTERN)
    )
}

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives it.
fn arn_of(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}")
}

/// A metric definition the service stores, carrying the `DimensionKeys` entry `entry`.
fn definition_with(entry: Value) -> Value {
    json!({"Name": "CartSize", "Namespace": "MyApp/Cart", "DimensionKeys": entry})
}

/// One row: a request breaking one boundary-applied and one self-applied constraint, the
/// whole envelope it must be refused with, and the SAME request with only the
/// boundary-applied member repaired — the second round trip a single-kind answer would have
/// cost — with the clauses that one must state.
struct MixedRow {
    case: &'static str,
    method: &'static str,
    path: String,
    both: Option<Value>,
    code: &'static str,
    clauses: Vec<String>,
    /// The request with the boundary-applied member repaired: its path (a member bound to
    /// the query string travels there) and body.
    repaired_path: String,
    repaired: Option<Value>,
    remaining: Vec<String>,
}

/// Every row of the table.
fn mixed_rows() -> Vec<MixedRow> {
    let monitor_path = format!("/appmonitor/{MONITOR}");
    let metrics_path = format!("/rummetrics/{MONITOR}/metrics");
    let tags_path = format!("/tags/{}", percent_encoded(&arn_of(MONITOR)));
    let long_tag_value = "v".repeat(MAX_TAG_VALUE_LENGTH + 1);
    let long_dimension_key = "p".repeat(MAX_DIMENSION_KEY_LENGTH + 1);
    let long_tag_key = "k".repeat(MAX_TAG_KEY_LENGTH + 1);
    // One domain more than `AppMonitorDomainList`'s `@length` admits, the last of them
    // outside the element shape's pattern.
    let too_many: Vec<String> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|label| format!("{label}.example.com"))
        .chain(["bad_domain".to_string()])
        .collect();
    vec![
        MixedRow {
            case: "UpdateAppMonitor: a domain list element outside its pattern, and a \
                   session sample rate past its range maximum",
            method: "PATCH",
            path: monitor_path.clone(),
            both: Some(json!({
                "DomainList": ["bad_domain"],
                "AppMonitorConfiguration": {"SessionSampleRate": 5},
            })),
            code: VALIDATION_CODE,
            clauses: vec![
                clause("[bad_domain]", "domainList", &domain_element_set()),
                clause(
                    "5",
                    "appMonitorConfiguration.sessionSampleRate",
                    &max_value("1"),
                ),
            ],
            repaired_path: monitor_path.clone(),
            repaired: Some(json!({
                "DomainList": ["bad_domain"],
                "AppMonitorConfiguration": {"SessionSampleRate": 0.5},
            })),
            remaining: vec![clause("[bad_domain]", "domainList", &domain_element_set())],
        },
        MixedRow {
            case: "UpdateAppMonitor: a scalar domain outside its pattern, and a session \
                   sample rate below its range minimum",
            method: "PATCH",
            path: monitor_path.clone(),
            both: Some(json!({
                "Domain": "ex..ample.com",
                "AppMonitorConfiguration": {"SessionSampleRate": -0.5},
            })),
            code: VALIDATION_CODE,
            clauses: vec![
                clause(
                    "ex..ample.com",
                    "domain",
                    &pattern_requirement(DOMAIN_PATTERN),
                ),
                clause(
                    "-0.5",
                    "appMonitorConfiguration.sessionSampleRate",
                    &min_value("0"),
                ),
            ],
            repaired_path: monitor_path.clone(),
            repaired: Some(json!({
                "Domain": "ex..ample.com",
                "AppMonitorConfiguration": {"SessionSampleRate": 0},
            })),
            remaining: vec![clause(
                "ex..ample.com",
                "domain",
                &pattern_requirement(DOMAIN_PATTERN),
            )],
        },
        MixedRow {
            case: "UpdateAppMonitor: one list, two clauses — the list's own length maximum \
                   and its element shape's pattern",
            method: "PATCH",
            path: monitor_path.clone(),
            both: Some(json!({"DomainList": too_many})),
            code: VALIDATION_CODE,
            clauses: vec![
                clause(
                    &format!("[{}]", too_many.join(", ")),
                    "domainList",
                    &max_length(MAX_DOMAINS),
                ),
                clause(
                    &format!("[{}]", too_many.join(", ")),
                    "domainList",
                    &domain_element_set(),
                ),
            ],
            repaired_path: monitor_path.clone(),
            repaired: Some(json!({"DomainList": ["a.example.com", "bad_domain"]})),
            remaining: vec![clause(
                "[a.example.com, bad_domain]",
                "domainList",
                &domain_element_set(),
            )],
        },
        MixedRow {
            case: "CreateAppMonitor: a name outside its pattern and a reserved tag key, \
                   either side of a tag value past its length maximum",
            method: "POST",
            path: "/appmonitor".to_string(),
            both: Some(json!({
                "Name": ".bad",
                "Domain": "example.com",
                "Tags": {"aws:owner": "x", "Keeper": long_tag_value},
            })),
            code: VALIDATION_CODE,
            clauses: vec![
                clause(".bad", "name", &pattern_requirement(NAME_PATTERN)),
                clause(
                    &long_tag_value,
                    "tags.Keeper",
                    &max_length(MAX_TAG_VALUE_LENGTH),
                ),
                clause(
                    "aws:owner",
                    "tags.aws:owner",
                    &pattern_requirement(TAG_KEY_PATTERN),
                ),
            ],
            repaired_path: "/appmonitor".to_string(),
            repaired: Some(json!({
                "Name": ".bad",
                "Domain": "example.com",
                "Tags": {"aws:owner": "x", "Keeper": "kept"},
            })),
            remaining: vec![
                clause(".bad", "name", &pattern_requirement(NAME_PATTERN)),
                clause(
                    "aws:owner",
                    "tags.aws:owner",
                    &pattern_requirement(TAG_KEY_PATTERN),
                ),
            ],
        },
        MixedRow {
            case: "UpdateRumMetricDefinition: a destination outside the enum, declared \
                   BEFORE the dimension name outside its pattern",
            method: "PATCH",
            path: metrics_path.clone(),
            both: Some(json!({
                "Destination": "Nowhere",
                "MetricDefinitionId": "0000",
                "MetricDefinition": definition_with(json!({"metadata.pageId": ":PageId"})),
            })),
            code: VALIDATION_CODE,
            clauses: vec![
                clause("Nowhere", "destination", &enum_set()),
                clause(
                    ":PageId",
                    "metricDefinition.dimensionKeys.metadata.pageId",
                    &pattern_requirement(DIMENSION_NAME_PATTERN),
                ),
            ],
            repaired_path: metrics_path.clone(),
            repaired: Some(json!({
                "Destination": METRIC_DESTINATION_LISTED[1],
                "MetricDefinitionId": "0000",
                "MetricDefinition": definition_with(json!({"metadata.pageId": ":PageId"})),
            })),
            remaining: vec![clause(
                ":PageId",
                "metricDefinition.dimensionKeys.metadata.pageId",
                &pattern_requirement(DIMENSION_NAME_PATTERN),
            )],
        },
        MixedRow {
            case: "UpdateRumMetricDefinition: a dimension name outside its pattern, \
                   declared BEFORE the definition id below its length minimum",
            method: "PATCH",
            path: metrics_path.clone(),
            both: Some(json!({
                "Destination": METRIC_DESTINATION_LISTED[1],
                "MetricDefinitionId": "",
                "MetricDefinition": definition_with(json!({"metadata.pageId": ":PageId"})),
            })),
            code: VALIDATION_CODE,
            clauses: vec![
                clause(
                    ":PageId",
                    "metricDefinition.dimensionKeys.metadata.pageId",
                    &pattern_requirement(DIMENSION_NAME_PATTERN),
                ),
                clause(
                    "",
                    "metricDefinitionId",
                    &min_length(MIN_METRIC_DEFINITION_ID_LENGTH),
                ),
            ],
            repaired_path: metrics_path.clone(),
            repaired: Some(json!({
                "Destination": METRIC_DESTINATION_LISTED[1],
                "MetricDefinitionId": "0000",
                "MetricDefinition": definition_with(json!({"metadata.pageId": ":PageId"})),
            })),
            remaining: vec![clause(
                ":PageId",
                "metricDefinition.dimensionKeys.metadata.pageId",
                &pattern_requirement(DIMENSION_NAME_PATTERN),
            )],
        },
        MixedRow {
            case: "BatchCreateRumMetricDefinitions: a destination outside the enum, and a \
                   dimension key past its length maximum inside the second definition",
            method: "POST",
            path: metrics_path.clone(),
            both: Some(json!({
                "Destination": "Nowhere",
                "MetricDefinitions": [
                    {"Name": "JsErrorCount"},
                    definition_with(json!({long_dimension_key.clone(): "PageId"})),
                ],
            })),
            code: VALIDATION_CODE,
            clauses: vec![
                clause("Nowhere", "destination", &enum_set()),
                clause(
                    &long_dimension_key,
                    &format!("metricDefinitions.2.member.dimensionKeys.{long_dimension_key}"),
                    &max_length(MAX_DIMENSION_KEY_LENGTH),
                ),
            ],
            repaired_path: metrics_path.clone(),
            repaired: Some(json!({
                "Destination": METRIC_DESTINATION_LISTED[1],
                "MetricDefinitions": [
                    {"Name": "JsErrorCount"},
                    definition_with(json!({long_dimension_key.clone(): "PageId"})),
                ],
            })),
            remaining: vec![clause(
                &long_dimension_key,
                &format!("metricDefinitions.2.member.dimensionKeys.{long_dimension_key}"),
                &max_length(MAX_DIMENSION_KEY_LENGTH),
            )],
        },
        MixedRow {
            // The ARN-addressed tagging frontend: the SAME envelope rule, in that
            // frontend's own error vocabulary — its wire code, and the framework's clause,
            // which quotes a `@pattern` as the model writes it.
            case: "UntagResource: a named key past TagKey's length maximum, and a reserved \
                   key at the next position",
            method: "DELETE",
            path: format!(
                "{tags_path}?tagKeys={}&tagKeys={}",
                percent_encoded(&long_tag_key),
                percent_encoded("aws:owner")
            ),
            both: None,
            code: BAD_REQUEST_CODE,
            clauses: vec![
                clause(
                    &long_tag_key,
                    "tagKeys.1.member",
                    &max_length(MAX_TAG_KEY_LENGTH),
                ),
                clause(
                    "aws:owner",
                    "tagKeys.2.member",
                    &pattern_requirement(MODELLED_TAG_KEY_PATTERN),
                ),
            ],
            repaired_path: format!(
                "{tags_path}?tagKeys=Env&tagKeys={}",
                percent_encoded("aws:owner")
            ),
            repaired: None,
            remaining: vec![clause(
                "aws:owner",
                "tagKeys.2.member",
                &pattern_requirement(MODELLED_TAG_KEY_PATTERN),
            )],
        },
        MixedRow {
            // The other operation of that frontend, refused on the tag MAP — a member the
            // app-monitor operations carry too, so which frontend words the clauses is the
            // OPERATION's answer and not the member's: the same code and the same clause
            // the repaired request below is answered with by the handler.
            case: "TagResource: a tag value past its length maximum, and a reserved key in \
                   the same map",
            method: "POST",
            path: tags_path.clone(),
            both: Some(json!({"Tags": {"aws:owner": "x", "Keeper": long_tag_value}})),
            code: BAD_REQUEST_CODE,
            clauses: vec![
                clause(
                    &long_tag_value,
                    "tags.Keeper",
                    &max_length(MAX_TAG_VALUE_LENGTH),
                ),
                clause(
                    "aws:owner",
                    "tags.aws:owner",
                    &pattern_requirement(MODELLED_TAG_KEY_PATTERN),
                ),
            ],
            repaired_path: tags_path.clone(),
            repaired: Some(json!({"Tags": {"aws:owner": "x", "Keeper": "kept"}})),
            remaining: vec![clause(
                "aws:owner",
                "tags.aws:owner",
                &pattern_requirement(MODELLED_TAG_KEY_PATTERN),
            )],
        },
    ]
}

/// Assert `reply` is the refusal `expected` under `code`, with the modelled 400.
fn assert_refused(case: &str, reply: &Reply, code: &str, expected: &str) {
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.error_code(case);
    assert_eq!(
        answered, code,
        "case {case:?}: expected the error code {code:?}, got {answered:?}"
    );
    let body = reply.json(case);
    let stated = message(&body).unwrap_or_default();
    assert_eq!(
        stated, expected,
        "case {case:?}: expected the message {expected:?}, got {stated:?}"
    );
}

/// The app monitor every row addresses, with a domain authority and no tags.
async fn create_monitor(router: &MultiServiceRouter) {
    let reply = call(
        router,
        "POST",
        "/appmonitor",
        Some(json!({"Name": MONITOR, "Domain": "before.example.com"})),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "the setup create must answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// RULE: one request breaking a boundary-applied constraint AND one this service applies
/// itself is answered with ONE envelope stating a clause for each, in the order the input
/// declares the members — and repairing the boundary-applied member alone leaves exactly the
/// clauses that answer already stated, so no clause was withheld for a second round trip.
#[tokio::test]
async fn both_kinds_of_constraint_are_stated_in_one_envelope() {
    let (router, _engine) = emulator();
    create_monitor(&router).await;

    for row in mixed_rows() {
        let reply = call(&router, row.method, &row.path, row.both.clone()).await;
        assert_refused(row.case, &reply, row.code, &envelope(&row.clauses));

        let repaired = format!("{} (the boundary-applied member repaired)", row.case);
        let second = call(
            &router,
            row.method,
            &row.repaired_path,
            row.repaired.clone(),
        )
        .await;
        assert_refused(&repaired, &second, row.code, &envelope(&row.remaining));
    }
}

/// RULE: the envelope is sized by the MEMBERS a request broke and not by how much of one
/// member offends — a list far longer than its modelled maximum, every element of it outside
/// the element shape's pattern, is still the two clauses that member can state: the list's
/// own `@length` (the boundary walk's) and the element shape's constraint set (this
/// service's).
#[tokio::test]
async fn one_list_is_two_clauses_however_many_of_its_elements_offend() {
    let (router, _engine) = emulator();
    create_monitor(&router).await;

    let case = "an update naming 200 domains, every one of them outside the pattern";
    let domains: Vec<String> = (0..200).map(|index| format!("bad_domain_{index}")).collect();
    let reply = call(
        &router,
        "PATCH",
        &format!("/appmonitor/{MONITOR}"),
        Some(json!({"DomainList": domains})),
    )
    .await;
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let answered = reply.error_code(case);
    assert_eq!(
        answered, VALIDATION_CODE,
        "case {case:?}: expected the error code {VALIDATION_CODE:?}, got {answered:?}"
    );
    let body = reply.json(case);
    let stated = message(&body).unwrap_or_default();
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "case {case:?}: expected an envelope over exactly 2 clauses, got {stated:?}"
    );
    for requirement in [max_length(MAX_DOMAINS), domain_element_set()] {
        assert!(
            stated.contains(&requirement),
            "case {case:?}: expected the envelope to state {requirement:?}, got {stated:?}"
        );
    }
}

/// RULE: a refused request writes nothing, whichever layer found the clauses — the monitor
/// every row addressed keeps the domain authority it was created with, holds no tag, and the
/// create row's monitor was never stored.
#[tokio::test]
async fn a_request_refused_on_either_kind_writes_nothing() {
    let (router, _engine) = emulator();
    create_monitor(&router).await;

    for row in mixed_rows() {
        let reply = call(&router, row.method, &row.path, row.both.clone()).await;
        assert_eq!(
            reply.status,
            400,
            "case {:?}: expected HTTP 400, got {} {:?}",
            row.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    let case = "the stored app monitor after every refusal";
    let read = call(&router, "GET", &format!("/appmonitor/{MONITOR}"), None).await;
    let monitor = read.json(case);
    assert_eq!(
        monitor["AppMonitor"]["Domain"].as_str(),
        Some("before.example.com"),
        "case {case:?}: a refused request must leave the stored domain alone, got {:?}",
        monitor["AppMonitor"]["Domain"]
    );
    let tags = monitor["AppMonitor"]["Tags"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    assert_eq!(
        tags, 0,
        "case {case:?}: a refused request must write no tag, got {:?}",
        monitor["AppMonitor"]["Tags"]
    );

    let created = call(&router, "GET", "/appmonitor/.bad", None).await;
    assert_eq!(
        created.status,
        404,
        "case {case:?}: the refused create must have stored no monitor, got {} {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}
