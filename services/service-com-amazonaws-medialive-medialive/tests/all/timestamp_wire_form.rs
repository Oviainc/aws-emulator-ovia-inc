// SPDX-License-Identifier: CC0-1.0

//! The wire FORM of this service's modelled instants and tag maps, end to end over
//! the restJson1 wire.
//!
//! Two rules, each asserted over a table that spans the class rather than the one
//! recorded request:
//!
//! 1. A response member whose modelled target carries `@timestampFormat("date-time")`
//!    — in this service every timestamp member, all of them targeting
//!    `__timestampIso8601`
//!    (`smithy_query(query="shape",shape="__timestampIso8601")` reports
//!    `"timestampFormat": "date-time"`) — travels as an RFC 3339 / ISO-8601 date-time
//!    STRING, never as the restJson1 default of an epoch-seconds JSON number, and the
//!    instant it names is the instant the call happened.
//! 2. A `TagMap` member of a taggable resource is always present, as the map it is
//!    modelled as: a resource created with no tags reports the EMPTY map rather than
//!    omitting the member.
//!
//! The rows differ along what the rules generalise over: the resource (cloudwatch
//! alarm template group, eventbridge rule template group, eventbridge rule template,
//! signal map), the operation reporting it (a create, the read that follows it, and a
//! list, which reports a summary shape of its own), which timestamp members the shape
//! carries, and whether the request supplied tags at all and how many.

use std::time::SystemTime;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::{entry_point, epoch_seconds, member, reported_tags, timestamp, unix_seconds};
use crate::wire::{call, medialive, ACCOUNT, DEFAULT_REGION};

/// The collection URIs of the four taggable, timestamped resources this service
/// implements (`smithy_query(query="operation", operation="…")` for each one's
/// create: POST /prod/cloudwatch-alarm-template-groups,
/// /prod/eventbridge-rule-template-groups, /prod/eventbridge-rule-templates and
/// /prod/signal-maps).
const ALARM_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const RULE_GROUPS: &str = "/prod/eventbridge-rule-template-groups";
const RULE_TEMPLATES: &str = "/prod/eventbridge-rule-templates";
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// One instant a response reports, read as the instant its date-time string names.
///
/// The string form is asserted HERE, naming the value the response carried, and the
/// instant is then read through the crate's one wire-timestamp reader
/// ([`crate::support::timestamp`]).
fn reported_instant(case: &str, name: &str, body: &Value) -> aws_smithy_types::DateTime {
    let value = member(body, name);
    assert!(
        value.is_string(),
        "case {case:?}: {name} must be an RFC 3339 date-time string, never the \
         restJson1 default of an epoch-seconds number, got {value:?}"
    );
    timestamp(case, name, body)
}

/// One row: a resource to create, and the timestamp members its create and its read
/// both report.
struct Row {
    case: &'static str,
    /// The collection the create posts to; the read is `<collection>/<id>`.
    collection: &'static str,
    /// The create's request body.
    body: Value,
    /// The success status the create's operation declares.
    created_status: u16,
    /// The members of the response shape that target `__timestampIso8601`.
    stamps: &'static [&'static str],
}

/// A template group to create eventbridge rule templates in, answering its id.
async fn rule_group(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(router, "POST", RULE_GROUPS, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "setup: expected CreateEventBridgeRuleTemplateGroup to answer 201, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// Rule: every response member modelled as a `date-time` timestamp is reported as an
/// RFC 3339 string naming the instant of the call, by the create that stamps it and
/// by the read that reports it afterwards; and `Tags` is always reported as a map.
#[tokio::test]
async fn created_and_read_resources_report_date_time_strings_and_a_tag_map() {
    let router = medialive();
    let entry = entry_point(&router, "timestamp-entry").await;
    let group = rule_group(&router, "timestamp-group").await;

    // Every `EventBridgeRuleTemplateTarget$Arn` must be an SNS topic or a CloudWatch
    // log group (`smithy_query(query="shape", shape="EventBridgeRuleTemplateTarget")`).
    let topic = format!("arn:aws:sns:{DEFAULT_REGION}:{ACCOUNT}:timestamps");

    let table = vec![
        Row {
            case: "alarm template group, untagged",
            collection: ALARM_GROUPS,
            body: json!({ "name": "alarm-group-untagged" }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
        Row {
            case: "alarm template group, three tags and a description",
            collection: ALARM_GROUPS,
            body: json!({
                "name": "alarm-group-tagged",
                "description": "stamped on create",
                "tags": { "owner": "video", "env": "test", "cost-centre": "1234" },
            }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
        Row {
            case: "rule template group, untagged",
            collection: RULE_GROUPS,
            body: json!({ "name": "rule-group-untagged" }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
        Row {
            case: "rule template group, one tag",
            collection: RULE_GROUPS,
            body: json!({ "name": "rule-group-tagged", "tags": { "owner": "video" } }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
        Row {
            case: "rule template, untagged and without targets",
            collection: RULE_TEMPLATES,
            body: json!({
                "name": "rule-template-untagged",
                "eventType": "MEDIALIVE_CHANNEL_ALERT",
                "groupIdentifier": group.clone(),
            }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
        Row {
            case: "rule template, tagged and with a target",
            collection: RULE_TEMPLATES,
            body: json!({
                "name": "rule-template-tagged",
                "eventType": "SIGNAL_MAP_ACTIVE_ALARM",
                "groupIdentifier": group.clone(),
                "eventTargets": [{ "arn": topic }],
                "tags": { "owner": "video", "env": "test" },
            }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
        Row {
            case: "signal map, untagged",
            collection: SIGNAL_MAPS,
            body: json!({
                "name": "signal-map-untagged",
                "discoveryEntryPointArn": entry.clone(),
            }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
        Row {
            case: "signal map, tagged",
            collection: SIGNAL_MAPS,
            body: json!({
                "name": "signal-map-tagged",
                "description": "a mapped workflow",
                "discoveryEntryPointArn": entry.clone(),
                "tags": { "suite": "timestamps" },
            }),
            created_status: 201,
            stamps: &["createdAt", "modifiedAt"],
        },
    ];

    for Row {
        case,
        collection,
        body,
        created_status,
        stamps,
    } in table
    {
        let before = SystemTime::now();
        let (status, created) = call(&router, "POST", collection, Some(body.clone())).await;
        let after = SystemTime::now();
        assert_eq!(
            status, created_status,
            "case {case:?}: expected {created_status} from the create, got {status}: {created}"
        );

        // The stamp is the instant of THIS call: a date-time string that parses is
        // not enough, it has to name the window the call ran in. The window is
        // widened by a second each way, because a rendering may drop sub-second
        // precision.
        let (opened, closed) = (unix_seconds(before) - 1.0, unix_seconds(after) + 1.0);
        for name in stamps {
            let stamp = reported_instant(case, name, &created);
            assert!(
                (opened..=closed).contains(&epoch_seconds(stamp)),
                "case {case:?}: expected {name} within the call's window \
                 [{opened}, {closed}], got {}",
                epoch_seconds(stamp)
            );
        }

        assert_eq!(
            member(&created, "tags"),
            reported_tags(body.get("tags")),
            "case {case:?}: expected Tags to be reported as a map, got {:?}",
            member(&created, "tags")
        );

        // The read that follows reports the SAME instants, in the same form: the
        // projection a create answers with and the one a read answers with are one
        // reading of one stored resource.
        let id = created["id"].as_str().unwrap_or_default().to_string();
        let (status, read) = call(&router, "GET", &format!("{collection}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading {id:?} back, got {status}: {read}"
        );
        for name in stamps {
            assert_eq!(
                member(&read, name),
                member(&created, name),
                "case {case:?}: expected the read's {name} to be the create's \
                 {:?}, got {:?}",
                member(&created, name),
                member(&read, name)
            );
            // Read through the same reader, so the read's form is pinned too.
            reported_instant(case, name, &read);
        }
        assert_eq!(
            member(&read, "tags"),
            member(&created, "tags"),
            "case {case:?}: expected the read to report the create's tags, got {:?}",
            member(&read, "tags")
        );
    }
}

/// Rule: the same wire form holds for a timestamp the CLOCK fills in later
/// (`SignalMapResponse$LastDiscoveredAt`, reported once discovery is due) and for the
/// summary shape a listing reports (`SignalMapSummary`, whose `CreatedAt` is
/// `@required` and whose `ModifiedAt` is not).
#[tokio::test]
async fn clock_filled_and_listed_instants_are_date_time_strings_too() {
    let router = medialive();
    let entry = entry_point(&router, "listing-entry").await;

    // (case, the tags the create supplies)
    let table: [(&str, Option<Value>); 3] = [
        ("untagged", None),
        ("one tag", Some(json!({ "owner": "video" }))),
        ("empty tag map", Some(json!({}))),
    ];

    let mut created_maps: Vec<(&str, Value)> = Vec::new();
    for (case, tags) in table {
        let mut body = json!({
            "name": format!("listed-{}", case.replace(' ', "-")),
            "discoveryEntryPointArn": entry.clone(),
        });
        if let Some(tags) = tags.as_ref() {
            body["tags"] = tags.clone();
        }
        let (status, created) = call(&router, "POST", SIGNAL_MAPS, Some(body.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateSignalMap to answer 201, got {status}: {created}"
        );
        assert_eq!(
            member(&created, "tags"),
            reported_tags(tags.as_ref()),
            "case {case:?}: expected Tags to be reported as a map, got {:?}",
            member(&created, "tags")
        );
        // Discovery is asynchronous, so nothing has been discovered yet.
        assert_eq!(
            member(&created, "lastDiscoveredAt"),
            Value::Null,
            "case {case:?}: expected no lastDiscoveredAt before discovery is due, got {:?}",
            member(&created, "lastDiscoveredAt")
        );
        created_maps.push((case, created));
    }

    // Let the clock make discovery due, which is what fills `LastDiscoveredAt` in.
    wait_for_the_clock();

    for (case, created) in &created_maps {
        let id = created["id"].as_str().unwrap_or_default();
        let (status, read) = call(&router, "GET", &format!("{SIGNAL_MAPS}/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading the signal map back, got {status}: {read}"
        );
        for name in ["createdAt", "modifiedAt", "lastDiscoveredAt"] {
            reported_instant(case, name, &read);
        }
        // The discovered graph is a MAP OF STRUCTURES, which the serializer writes
        // against the map member's own schema: pin its wire shape on a non-empty map
        // — a JSON object keyed by the discovered resource's ARN whose every value is
        // the `MediaResource` object, not a scalar or a list.
        let graph = member(&read, "mediaResourceMap");
        let entries = graph
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a mediaResourceMap object, got {graph:?}"));
        assert!(
            !entries.is_empty(),
            "case {case:?}: expected the discovered graph to carry the entry point, got {graph:?}"
        );
        for (arn, resource) in entries {
            assert!(
                arn.starts_with("arn:aws:medialive:"),
                "case {case:?}: expected a mediaResourceMap keyed by resource ARN, got {arn:?}"
            );
            assert!(
                resource.is_object(),
                "case {case:?}: expected the {arn:?} entry to be a MediaResource object, got {resource:?}"
            );
        }
        let discovered = reported_instant(case, "lastDiscoveredAt", &read);
        let stamped = reported_instant(case, "createdAt", &read);
        assert!(
            discovered.secs() >= stamped.secs(),
            "case {case:?}: expected lastDiscoveredAt at or after createdAt, got \
             {discovered:?} and {stamped:?}"
        );
    }

    let (status, listed) = call(&router, "GET", SIGNAL_MAPS, None).await;
    assert_eq!(
        status, 200,
        "expected ListSignalMaps to answer 200, got {status}: {listed}"
    );
    let summaries = listed["signalMaps"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("expected a signalMaps list, got {listed}"));
    assert_eq!(
        summaries.len(),
        created_maps.len(),
        "expected every created signal map to be listed, got {listed}"
    );
    for summary in &summaries {
        let case = summary["name"].as_str().unwrap_or_default().to_string();
        for name in ["createdAt", "modifiedAt"] {
            reported_instant(&case, name, summary);
        }
        assert!(
            member(summary, "tags").is_object(),
            "case {case:?}: expected the summary's Tags to be a map, got {:?}",
            member(summary, "tags")
        );
    }
}
