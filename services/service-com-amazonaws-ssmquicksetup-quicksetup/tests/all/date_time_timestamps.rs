// SPDX-License-Identifier: CC0-1.0

//! Every timestamp this service puts on the wire, and the schema the codec reads its
//! rendering from.
//!
//! RULE. A `timestamp` member whose model carries `@timestampFormat("date-time")` is
//! rendered by this restJson1 service as an RFC 3339 / ISO-8601 STRING, never as the
//! protocol's default epoch-seconds JSON number. Every timestamp member of this model
//! carries that trait — `GetConfigurationManagerOutput$CreatedAt`/`$LastModifiedAt`,
//! `GetConfigurationOutput$CreatedAt`/`$LastModifiedAt`, `ConfigurationSummary$CreatedAt`
//! and `StatusSummary$LastUpdatedAt` (`smithy_query(query="shape", shape="...")` reports
//! `"timestampFormat": "date-time"` on each) — so the rule holds for every response of
//! every operation of this service. The INSTANT each one names is the resource's own and
//! is not part of the wire contract; only the form is.
//!
//! The first suite observes the rule the way a client does: buffered restJson1 requests
//! through the same router the production binary assembles, with every response body
//! walked RECURSIVELY so a stamp nested in a status summary or in a list element is
//! checked exactly like a top-level one. Its table spans what the rule generalises over —
//! the number of configuration definitions, which optional members the create carries,
//! the size of the parameter map, the Quick Setup type, and both sides of the clock-owed
//! deployment transition (which is what rewrites a `LastUpdatedAt`) — rather than the one
//! `GetConfigurationManager` call the issue cited. It covers all five response shapes that
//! can carry a stamp: the Get, the Get of a deployed configuration, and both listings.
//!
//! The second suite pins the same property one level down, at the member schema the JSON
//! codec actually looks the format up on (`vendor/smithy-schema-server/src/json.rs::
//! write_timestamp` -> `Schema::timestamp_format()`). It fails loudly if a shape ever goes
//! back to a macro-built schema that drops the trait.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ssmquicksetup_quicksetup::ops::types::{
    ConfigurationSummary, GetConfigurationManagerOutput, GetConfigurationOutput, StatusSummary,
};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::TRANSITION_SPAN;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// Every member name this service renders as a `date-time`: the complete set of timestamp
/// members in `smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json`, each carrying
/// `@timestampFormat("date-time")`. No non-timestamp member of this model carries any of
/// these names, so a body key of one of them is always a modelled timestamp.
const DATE_TIME_MEMBERS: &[&str] = &["CreatedAt", "LastModifiedAt", "LastUpdatedAt"];

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// Send one request signed for `ssm-quicksetup` in [`REGION`] and read back
/// `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// An `@httpLabel` value as one path segment: the separators an ARN or an id carries are
/// percent-encoded, the way an AWS SDK encodes a label.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The instant `value` names, after asserting it is the RFC 3339 string the model
/// requires. A JSON number — what the protocol's epoch-seconds default renders, and what
/// the defect produced — fails here.
fn date_time_at(case: &str, path: &str, value: &Value) -> f64 {
    let rendered = match value {
        Value::String(rendered) => rendered.clone(),
        other => panic!(
            "case {case:?}: {path} must be an RFC 3339 date-time STRING, got the JSON {} \
             {other}",
            match other {
                Value::Number(_) => "number",
                Value::Bool(_) => "boolean",
                Value::Null => "null",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
                Value::String(_) => "string",
            }
        ),
    };
    // RFC 3339's own shape: a date, the `T` separator and a UTC `Z` offset. Checked before
    // the parse so a value the parser happens to accept in another form still fails.
    assert!(
        rendered.contains('T') && rendered.ends_with('Z') && rendered.len() >= 20,
        "case {case:?}: {path} must be an RFC 3339 UTC date-time, got {rendered:?}"
    );
    let parsed = DateTime::from_str(&rendered, Format::DateTime).unwrap_or_else(|e| {
        panic!("case {case:?}: {path} must parse as a date-time, got {rendered:?}: {e}")
    });
    // Round trip: the rendering the service chose is the one the modelled format produces.
    let reparsed = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {path} must re-render: {e}"));
    assert_eq!(
        reparsed, rendered,
        "case {case:?}: {path} must already be in the modelled date-time form, got \
         {rendered:?} which re-renders as {reparsed:?}"
    );
    parsed.as_secs_f64()
}

/// Every modelled timestamp in `body`, keyed by its JSON path, with the instant each one
/// names — after asserting each is a `date-time` string and not a number.
///
/// The walk is recursive, so `$.StatusSummaries[0].LastUpdatedAt` and
/// `$.ConfigurationsList[1].StatusSummaries[0].LastUpdatedAt` are checked exactly like
/// `$.CreatedAt`.
fn timestamps_of(case: &str, body: &Value) -> BTreeMap<String, f64> {
    let mut found = BTreeMap::new();
    walk(case, "$", body, &mut found);
    found
}

fn walk(case: &str, path: &str, value: &Value, found: &mut BTreeMap<String, f64>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                let child = format!("{path}.{name}");
                if DATE_TIME_MEMBERS.contains(&name.as_str()) {
                    found.insert(child.clone(), date_time_at(case, &child, member));
                } else {
                    walk(case, &child, member, found);
                }
            }
        }
        Value::Array(elements) => {
            for (index, element) in elements.iter().enumerate() {
                walk(case, &format!("{path}[{index}]"), element, found);
            }
        }
        _ => {}
    }
}

/// Assert `body` carries exactly the timestamp paths `expected` names, each a `date-time`
/// string, and hand the instants back.
fn checked_timestamps(case: &str, body: &Value, expected: &[String]) -> BTreeMap<String, f64> {
    let found = timestamps_of(case, body);
    let paths: Vec<&str> = found.keys().map(String::as_str).collect();
    let expected: Vec<&str> = expected.iter().map(String::as_str).collect();
    assert_eq!(
        paths, expected,
        "case {case:?}: expected the timestamp members {expected:?}, got {paths:?} in {body}"
    );
    found
}

/// One `CreateConfigurationManager` row.
struct ManagerCase {
    name: &'static str,
    /// `(Type, extra parameter count)` per configuration definition.
    definitions: &'static [(&'static str, usize)],
    manager_name: Option<&'static str>,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    /// Whether to let the clock-owed deployment transition come due before reading, so
    /// the rule is observed over both the transient and the settled status summaries.
    settled: bool,
}

/// The rows differ along what the rule generalises over: how many definitions a manager
/// holds (so how many configurations, status summaries and list elements the responses
/// carry), which optional members the create names, how large a parameter map is, which
/// published Quick Setup type is deployed, and whether the clock-owed status transition
/// has come due — because the rule is about the RENDERING of a timestamp and so must hold
/// whatever else the request says.
const MANAGER_CASES: &[ManagerCase] = &[
    ManagerCase {
        name: "one definition, named, described and tagged, read while deploying",
        definitions: &[("AWSQuickSetupType-SSMHostMgmt", 1)],
        manager_name: Some("stamps"),
        description: Some("one definition"),
        tags: &[("env", "dev")],
        settled: false,
    },
    ManagerCase {
        name: "three definitions, no name, no description, no tags, read when settled",
        definitions: &[
            ("AWSQuickSetupType-SSMOpsCenter", 0),
            ("AWSQuickSetupType-Scheduler", 4),
            ("AWSQuickSetupType-DHMC", 2),
        ],
        manager_name: None,
        description: None,
        tags: &[],
        settled: true,
    },
    ManagerCase {
        name: "two definitions, named only, a large parameter map, read when settled",
        definitions: &[
            ("AWSQuickSetupType-PatchPolicy", 12),
            ("AWSQuickSetupType-ResourceExplorer", 0),
        ],
        manager_name: Some("m"),
        description: None,
        tags: &[("owner", "team-a"), ("cost-center", "42")],
        settled: true,
    },
];

/// The create body for one row. Every definition names exactly one organizational unit
/// and one Region, so each deploys exactly one configuration.
fn create_body(case: &ManagerCase) -> Value {
    let definitions: Vec<Value> = case
        .definitions
        .iter()
        .map(|(type_, extra)| {
            let mut parameters: Map<String, Value> = (0..*extra)
                .map(|index| (format!("Key{index}"), json!(index.to_string())))
                .collect();
            parameters.insert("TargetOrganizationalUnits".to_string(), json!("ou-1"));
            parameters.insert("TargetRegions".to_string(), json!(REGION));
            json!({ "Type": type_, "Parameters": Value::Object(parameters) })
        })
        .collect();
    let mut body = json!({ "ConfigurationDefinitions": definitions });
    if let Some(name) = case.manager_name {
        body["Name"] = json!(name);
    }
    if let Some(description) = case.description {
        body["Description"] = json!(description);
    }
    if !case.tags.is_empty() {
        let tags: Map<String, Value> = case
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect();
        body["Tags"] = Value::Object(tags);
    }
    body
}

/// The status-summary stamp paths a response carries under `prefix`, for a resource with
/// `summaries` summaries.
fn summary_paths(prefix: &str, summaries: usize) -> Vec<String> {
    (0..summaries)
        .map(|index| format!("{prefix}.StatusSummaries[{index}].LastUpdatedAt"))
        .collect()
}

/// The number of status summaries a resource of this service reports: one per status type
/// the service tracks, which is what the handlers project from the stored record.
fn summary_count(case: &str, value: &Value) -> usize {
    value
        .get("StatusSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected StatusSummaries in {value}"))
        .len()
}

#[tokio::test]
async fn every_modelled_timestamp_is_a_date_time_string_on_every_response_that_carries_one() {
    let router = emulator();

    for row in MANAGER_CASES {
        let case = row.name;
        let (status, created) = call(
            &router,
            "POST",
            "/configurationManager",
            Some(&create_body(row)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateConfigurationManager must succeed, got {status} / {created}"
        );
        // `CreateConfigurationManagerOutput` models only `ManagerArn`
        // (`smithy_query(query="shape", shape="CreateConfigurationManagerOutput")`), so the
        // create response is expected to carry no timestamp at all.
        checked_timestamps(case, &created, &[]);
        let manager_arn = created
            .get("ManagerArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {created}"))
            .to_string();

        if row.settled {
            // The move out of the transient deployment status is owed to the clock: wait
            // out the service's own span rather than guessing one, so this row reads the
            // summaries the settle path rewrote.
            tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
        }

        // GetConfigurationManager: the two manager stamps, plus one per status summary.
        let (status, manager) = call(
            &router,
            "GET",
            &format!("/configurationManager/{}", label(&manager_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetConfigurationManager must succeed, got {status} / {manager}"
        );
        let summaries = summary_count(case, &manager);
        assert!(
            summaries > 0,
            "case {case:?}: a manager reports at least one status summary, got {manager}"
        );
        let mut expected = vec!["$.CreatedAt".to_string(), "$.LastModifiedAt".to_string()];
        expected.extend(summary_paths("$", summaries));
        expected.sort();
        let manager_stamps = checked_timestamps(case, &manager, &expected);

        // ListConfigurationManagers: `ConfigurationManagerSummary` models no stamp of its
        // own (`smithy_query(query="shape", shape="ConfigurationManagerSummary")`), so the
        // only stamps there are its status summaries'.
        let (status, listed_managers) = call(
            &router,
            "POST",
            "/listConfigurationManagers",
            Some(&json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListConfigurationManagers must succeed, got {status} / \
             {listed_managers}"
        );
        let managers = listed_managers
            .get("ConfigurationManagersList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a ConfigurationManagersList in {listed_managers}")
            })
            .clone();
        let mut expected: Vec<String> = managers
            .iter()
            .enumerate()
            .flat_map(|(index, summary)| {
                summary_paths(
                    &format!("$.ConfigurationManagersList[{index}]"),
                    summary_count(case, summary),
                )
            })
            .collect();
        expected.sort();
        checked_timestamps(case, &listed_managers, &expected);

        // ListConfigurations: `ConfigurationSummary$CreatedAt` plus its status summaries'.
        let (status, listed) = call(
            &router,
            "POST",
            "/listConfigurations",
            Some(&json!({ "ManagerArn": manager_arn })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListConfigurations must succeed, got {status} / {listed}"
        );
        let configurations = listed
            .get("ConfigurationsList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConfigurationsList in {listed}"))
            .clone();
        assert_eq!(
            configurations.len(),
            row.definitions.len(),
            "case {case:?}: every definition here targets one account and Region, so one \
             configuration each was deployed, got {listed}"
        );
        let mut expected: Vec<String> = configurations
            .iter()
            .enumerate()
            .flat_map(|(index, summary)| {
                let prefix = format!("$.ConfigurationsList[{index}]");
                let mut paths = vec![format!("{prefix}.CreatedAt")];
                paths.extend(summary_paths(&prefix, summary_count(case, summary)));
                paths
            })
            .collect();
        expected.sort();
        let listed_stamps = checked_timestamps(case, &listed, &expected);

        // GetConfiguration, per deployed configuration: both configuration stamps plus
        // its status summaries'.
        for (index, summary) in configurations.iter().enumerate() {
            let id = summary
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected an Id, got {summary}"));
            let (status, configuration) = call(
                &router,
                "GET",
                &format!("/getConfiguration/{}", label(id)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetConfiguration({id}) must succeed, got {status} / \
                 {configuration}"
            );
            let mut expected = vec!["$.CreatedAt".to_string(), "$.LastModifiedAt".to_string()];
            expected.extend(summary_paths("$", summary_count(case, &configuration)));
            expected.sort();
            let stamps = checked_timestamps(case, &configuration, &expected);

            // The instant is not part of the wire contract, but it is the resource's own:
            // the Get and the listing report ONE CreatedAt for one configuration, so the
            // rendering is of persisted state rather than of the clock at serialize time.
            assert_eq!(
                stamps["$.CreatedAt"],
                listed_stamps[&format!("$.ConfigurationsList[{index}].CreatedAt")],
                "case {case:?}: the Get and the listing must report one CreatedAt for \
                 configuration {id}, got {configuration}"
            );
            assert!(
                stamps["$.LastModifiedAt"] >= stamps["$.CreatedAt"],
                "case {case:?}: LastModifiedAt {} must not precede CreatedAt {}, got \
                 {configuration}",
                stamps["$.LastModifiedAt"],
                stamps["$.CreatedAt"]
            );
        }

        assert!(
            manager_stamps["$.LastModifiedAt"] >= manager_stamps["$.CreatedAt"],
            "case {case:?}: LastModifiedAt {} must not precede CreatedAt {}, got {manager}",
            manager_stamps["$.LastModifiedAt"],
            manager_stamps["$.CreatedAt"]
        );
    }
}

/// Every shape of this service that declares a timestamp member, with that shape's member
/// names in the model's member order and the subset of them that are timestamps.
///
/// The catalog is the complete set: it is every structure of
/// `smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json` with a `timestamp` member,
/// as `smithy_query(query="shape", shape=...)` reports them.
fn timestamp_shapes() -> Vec<(&'static str, &'static Schema, &'static [&'static str])> {
    vec![
        (
            "GetConfigurationManagerOutput",
            &GetConfigurationManagerOutput::SCHEMA,
            &["CreatedAt", "LastModifiedAt"],
        ),
        (
            "GetConfigurationOutput",
            &GetConfigurationOutput::SCHEMA,
            &["CreatedAt", "LastModifiedAt"],
        ),
        (
            "ConfigurationSummary",
            &ConfigurationSummary::SCHEMA,
            &["CreatedAt"],
        ),
        ("StatusSummary", &StatusSummary::SCHEMA, &["LastUpdatedAt"]),
    ]
}

/// RULE, at the schema the codec reads: every timestamp member of every shape in the
/// catalog reports `date-time`, and no other member reports a timestamp format at all.
/// This is the property the wire suite observes, pinned where the JSON codec looks it up.
#[test]
fn every_modelled_timestamp_member_reports_the_date_time_format() {
    for (shape, schema, modelled) in timestamp_shapes() {
        let members = schema.members();
        let timestamps: Vec<&str> = members
            .iter()
            .filter(|member| matches!(member.shape_type(), ShapeType::Timestamp))
            .map(|member| member.member_name().unwrap_or("<unnamed>"))
            .collect();
        assert_eq!(
            timestamps,
            modelled.to_vec(),
            "case {shape:?}: expected the modelled timestamp members {modelled:?}, got \
             {timestamps:?}"
        );

        for (position, member) in members.iter().enumerate() {
            let name = member.member_name().unwrap_or("<unnamed>");
            // The codec dispatches on the index, so it has to be the member's position.
            assert_eq!(
                member.member_index(),
                Some(position),
                "case {shape:?}.{name}: member_index must equal the member's position, got {:?}",
                member.member_index()
            );
            let format = member.timestamp_format().map(|carried| carried.format());
            if matches!(member.shape_type(), ShapeType::Timestamp) {
                assert_eq!(
                    format,
                    Some(TimestampFormat::DateTime),
                    "case {shape:?}.{name}: a `@timestampFormat(\"date-time\")` member must \
                     report date-time, got {format:?}"
                );
            } else {
                assert_eq!(
                    format, None,
                    "case {shape:?}.{name}: only a timestamp member carries a timestamp \
                     format, got {format:?}"
                );
            }
        }
    }
}
