// SPDX-License-Identifier: CC0-1.0

//! The error document MediaLive answers when an identifier resolves to no
//! EventBridge rule TEMPLATE and to no rule-template GROUP, end to end over the
//! restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! RULE: a not-found refusal whose missing resource is of the EventBridge
//! rule-template hierarchy (a rule template or a rule-template group) is
//! answered as the modelled `NotFoundException` at HTTP 404 that states NO
//! message — the error document reports its `message` member with no value (an
//! explicit JSON null, which is what a client reads back as "no message")
//! instead of a synthesized description — whichever operation resolved the
//! identifier and whichever form the identifier took. `NotFoundException$Message`
//! is optional and its documentation is a placeholder
//! (`smithy_query(query="shape", shape="NotFoundException")`), so the model
//! states no message rule for it; the recorded behaviour of this hierarchy is
//! that no message is stated.
//!
//! The rest of the response envelope is untouched — `code`, `statusCode`,
//! `retryable`, `requestId` and `time` are still reported — and every OTHER
//! MediaLive resource family keeps composing its own message, which the last
//! test pins so this is not a blanket removal.
//!
//! The suite is a TABLE over what the rule generalises over: the OPERATION that
//! resolved the identifier (both levels' get/update/delete, the template create
//! and listing, and the two signal-map operations that resolve a rule-template
//! group), the LEVEL of the hierarchy (template, group), the FORM of the
//! identifier (an id-shaped value, a name, a name outside ASCII, the id and the
//! name of a resource of another family) and WHY it resolves to nothing (never
//! created, or deleted). The recorded case — reading a rule template straight
//! after its delete — is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{entry_point, percent_encoded};
use crate::wire::{call, error_code, medialive, ACCOUNT, DEFAULT_REGION as REGION};

/// The collections of the hierarchy under test (`smithy_query(query="digest")`:
/// POST/GET /prod/eventbridge-rule-templates, POST/GET
/// /prod/eventbridge-rule-template-groups).
const TEMPLATES: &str = "/prod/eventbridge-rule-templates";
const GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// The collections of the two OTHER monitor families the control rows read
/// (`smithy_query(query="digest")`).
const ALARM_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// The envelope members a MediaLive failure carries beside `message`, which this
/// rule leaves alone (see `error_envelope`).
const ENVELOPE_MEMBERS: [&str; 5] = ["code", "statusCode", "retryable", "requestId", "time"];

/// One request whose refusal reports a resource of the rule-template hierarchy
/// missing: the operation, and its `(method, uri, body)` built from the
/// identifier that resolves to nothing and from a channel ARN a signal map can
/// be discovered from.
struct Probe {
    /// The operation, as the model names it.
    op: &'static str,
    /// Which level of the hierarchy the identifier is resolved against.
    resolves: Level,
    request: fn(&str, &str) -> (&'static str, String, Option<Value>),
}

/// The level of the hierarchy an identifier names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// A rule template (`Identifier` of the template operations).
    Template,
    /// A rule-template group (`Identifier` of the group operations, and the
    /// `GroupIdentifier` / `EventBridgeRuleTemplateGroupIdentifier(s)` members).
    Group,
}

/// The body `CreateEventBridgeRuleTemplate` needs beside the group identifier:
/// an `EventBridgeRuleTemplateEventType` value and one target of the kind
/// `EventBridgeRuleTemplateTarget$Arn` documents ("Target ARNs must be either an
/// SNS topic or CloudWatch log group").
fn create_template_body(group_identifier: &str) -> Value {
    json!({
        "name": "probe-template",
        "groupIdentifier": group_identifier,
        "eventType": "MEDIALIVE_CHANNEL_ALERT",
        "eventTargets": [{ "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:alerts") }],
    })
}

/// Every operation of this service that reports a rule template or a
/// rule-template group missing (`smithy_query(query="digest")` for the URIs and
/// the members).
const PROBES: [Probe; 10] = [
    Probe {
        op: "GetEventBridgeRuleTemplate",
        resolves: Level::Template,
        request: |identifier, _entry| {
            (
                "GET",
                format!("{TEMPLATES}/{}", percent_encoded(identifier)),
                None,
            )
        },
    },
    Probe {
        op: "UpdateEventBridgeRuleTemplate",
        resolves: Level::Template,
        request: |identifier, _entry| {
            (
                "PATCH",
                format!("{TEMPLATES}/{}", percent_encoded(identifier)),
                Some(json!({ "description": "after the delete" })),
            )
        },
    },
    Probe {
        op: "DeleteEventBridgeRuleTemplate",
        resolves: Level::Template,
        request: |identifier, _entry| {
            (
                "DELETE",
                format!("{TEMPLATES}/{}", percent_encoded(identifier)),
                None,
            )
        },
    },
    Probe {
        op: "GetEventBridgeRuleTemplateGroup",
        resolves: Level::Group,
        request: |identifier, _entry| {
            (
                "GET",
                format!("{GROUPS}/{}", percent_encoded(identifier)),
                None,
            )
        },
    },
    Probe {
        op: "UpdateEventBridgeRuleTemplateGroup",
        resolves: Level::Group,
        request: |identifier, _entry| {
            (
                "PATCH",
                format!("{GROUPS}/{}", percent_encoded(identifier)),
                Some(json!({ "description": "after the delete" })),
            )
        },
    },
    Probe {
        op: "DeleteEventBridgeRuleTemplateGroup",
        resolves: Level::Group,
        request: |identifier, _entry| {
            (
                "DELETE",
                format!("{GROUPS}/{}", percent_encoded(identifier)),
                None,
            )
        },
    },
    Probe {
        op: "CreateEventBridgeRuleTemplate",
        resolves: Level::Group,
        request: |identifier, _entry| {
            (
                "POST",
                TEMPLATES.to_string(),
                Some(create_template_body(identifier)),
            )
        },
    },
    Probe {
        op: "ListEventBridgeRuleTemplates",
        resolves: Level::Group,
        request: |identifier, _entry| {
            (
                "GET",
                format!("{TEMPLATES}?groupIdentifier={}", percent_encoded(identifier)),
                None,
            )
        },
    },
    Probe {
        op: "CreateSignalMap",
        resolves: Level::Group,
        request: |identifier, entry| {
            (
                "POST",
                SIGNAL_MAPS.to_string(),
                Some(json!({
                    "name": "probe-map",
                    "discoveryEntryPointArn": entry,
                    "eventBridgeRuleTemplateGroupIdentifiers": [identifier],
                })),
            )
        },
    },
    Probe {
        op: "ListSignalMaps",
        resolves: Level::Group,
        request: |identifier, _entry| {
            (
                "GET",
                format!(
                    "{SIGNAL_MAPS}?eventBridgeRuleTemplateGroupIdentifier={}",
                    percent_encoded(identifier)
                ),
                None,
            )
        },
    },
];

/// Assert the whole rule on one answer: 404, the modelled discriminator, NO
/// `message` member, and the response envelope still complete.
fn assert_messageless_not_found(case: &str, op: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}/{op}: expected HTTP 404, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "NotFoundException",
        "case {case:?}/{op}: expected the modelled NotFoundException, got {body}"
    );
    assert_eq!(
        body.get("message"),
        Some(&Value::Null),
        "case {case:?}/{op}: expected the error document to report the message member \
         with no value, got {:?}",
        body.get("message")
    );
    for member in ENVELOPE_MEMBERS {
        assert!(
            body.get(member).is_some(),
            "case {case:?}/{op}: expected the response envelope to still report {member}, got {body}"
        );
    }
}

/// Run every probe of `level` against `identifier`, asserting the rule on each.
async fn probe_all(
    router: &MultiServiceRouter,
    case: &str,
    entry: &str,
    levels: &[Level],
    identifier: &str,
) {
    for probe in &PROBES {
        if !levels.contains(&probe.resolves) {
            continue;
        }
        let (method, uri, body) = (probe.request)(identifier, entry);
        let (status, answer) = call(router, method, &uri, body).await;
        assert_messageless_not_found(case, probe.op, status, &answer);
    }
}

/// Rule: an identifier that resolves to no rule template and to no rule-template
/// group is refused with a 404 `NotFoundException` whose `message` member is
/// absent, from every operation that resolves such an identifier.
///
/// The rows are the FORMS an identifier takes — `Identifier` is documented as
/// "its id or current name", so both forms are covered, plus a name outside
/// ASCII (which `__stringMin1Max255PatternS` admits: its pattern excludes only
/// whitespace) and the id and the name of a live resource of ANOTHER family,
/// which name no resource of this hierarchy either.
#[tokio::test]
async fn an_unresolvable_rule_template_identifier_is_404_with_no_message() {
    // (case, the identifier that resolves to nothing)
    let never_created: [(&str, &str); 4] = [
        // The shape of a minted id (`__stringMin7Max11PatternAws097`), which is
        // the form the recorded case used.
        ("an id nothing was ever minted for", "6000000"),
        ("a name nothing holds", "no-such-template"),
        ("a name outside ASCII", "存在しない"),
        // An AWS-provided resource's id form ("AWS provided templates have ids
        // that start with `aws-`") that this store holds nothing for.
        ("an aws- prefixed id", "aws-nothing"),
    ];

    for (case, identifier) in never_created {
        let router = medialive();
        let entry = entry_point(&router, &format!("entry-{}", identifier.len())).await;
        probe_all(
            &router,
            case,
            &entry,
            &[Level::Template, Level::Group],
            identifier,
        )
        .await;
    }

    // The id and the name of a live resource of another family: an alarm
    // template group exists, but it is no rule template and no rule-template
    // group, so the hierarchy answers it exactly as it answers an id nothing
    // minted.
    let router = medialive();
    let entry = entry_point(&router, "entry-other-family").await;
    let (status, alarm_group) = call(
        &router,
        "POST",
        ALARM_GROUPS,
        Some(json!({ "name": "other-family" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the alarm template group to be created, got {status}: {alarm_group}"
    );
    let alarm_id = alarm_group["id"].as_str().unwrap_or_default().to_string();
    assert!(
        !alarm_id.is_empty(),
        "setup: expected the alarm template group to report an id, got {alarm_group}"
    );
    for (case, identifier) in [
        ("an alarm template group's id", alarm_id.as_str()),
        ("an alarm template group's name", "other-family"),
    ] {
        probe_all(
            &router,
            case,
            &entry,
            &[Level::Template, Level::Group],
            identifier,
        )
        .await;
    }
}

/// Rule, on the path the recording took: the identifier of a rule template (and
/// then of its group) whose DELETE this service accepted resolves to nothing, so
/// every operation that resolves it answers the same messageless 404 — the
/// deleted id is not a special case with wording of its own.
///
/// The recorded case is the first group of assertions here:
/// `DeleteEventBridgeRuleTemplate` followed by `GetEventBridgeRuleTemplate` on
/// that id. The rows after it extend the same identifier to the group level and
/// to every other operation that resolves one.
#[tokio::test]
async fn a_deleted_rule_template_or_group_is_404_with_no_message() {
    let router = medialive();
    let entry = entry_point(&router, "entry-deleted").await;

    let (status, group) = call(&router, "POST", GROUPS, Some(json!({ "name": "hierarchy" }))).await;
    assert_eq!(
        status, 201,
        "setup: expected the rule template group to be created, got {status}: {group}"
    );
    let group_id = group["id"].as_str().unwrap_or_default().to_string();

    let (status, template) = call(
        &router,
        "POST",
        TEMPLATES,
        Some(json!({
            "name": "watcher",
            "groupIdentifier": group_id,
            "eventType": "MEDIALIVE_CHANNEL_ALERT",
            "eventTargets": [{ "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:alerts") }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the rule template to be created, got {status}: {template}"
    );
    let template_id = template["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        template["groupId"],
        json!(group_id),
        "setup: expected the template to report the group it was created in, got {template}"
    );

    // The control: while the template is live, its reader answers it.
    let (status, read) = call(&router, "GET", &format!("{TEMPLATES}/{template_id}"), None).await;
    assert_eq!(
        status, 200,
        "setup: expected the live template to be readable, got {status}: {read}"
    );

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("{TEMPLATES}/{template_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "DeleteEventBridgeRuleTemplate: expected 204, got {status}: {deleted}"
    );

    // The recorded case, and its siblings at the same level: by the template's
    // id and by the name it used to hold.
    for (case, identifier) in [
        ("a deleted template by id", template_id.as_str()),
        ("a deleted template by its former name", "watcher"),
    ] {
        probe_all(&router, case, &entry, &[Level::Template], identifier).await;
    }

    // The group is empty again now, so it deletes, and its own identifier dies
    // the same way — for the group operations and for every member that names a
    // group.
    let (status, deleted) = call(&router, "DELETE", &format!("{GROUPS}/{group_id}"), None).await;
    assert_eq!(
        status, 204,
        "DeleteEventBridgeRuleTemplateGroup: expected 204, got {status}: {deleted}"
    );
    for (case, identifier) in [
        ("a deleted group by id", group_id.as_str()),
        ("a deleted group by its former name", "hierarchy"),
    ] {
        probe_all(&router, case, &entry, &[Level::Group], identifier).await;
    }
}

/// Rule, from the other side: the absent `message` belongs to the two monitor
/// TEMPLATE hierarchies, so every resource family OUTSIDE them still composes
/// its own message from the request — the resource's label, the identifier the
/// request named, then `not found`.
///
/// The rows span the families whose recorded 404s DO carry a message: the
/// classic channel and input API, and the signal map, reached both by its own
/// `Identifier` and through the `SignalMapIdentifier` filter of
/// `ListEventBridgeRuleTemplates` — an operation of a hierarchy that is
/// resolving a resource of another family, which is where the two rules meet.
#[tokio::test]
async fn another_family_s_not_found_still_states_its_message() {
    let router = medialive();

    // (case, method, uri, the message the family composes)
    let rows: [(&str, &str, String, String); 4] = [
        (
            "DescribeChannel",
            "GET",
            "/prod/channels/9999999".to_string(),
            "Channel 9999999 not found".to_string(),
        ),
        (
            "DescribeInput",
            "GET",
            "/prod/inputs/9999999".to_string(),
            "Input 9999999 not found".to_string(),
        ),
        (
            "GetSignalMap",
            "GET",
            format!("{SIGNAL_MAPS}/no-such-map"),
            "Signal map no-such-map not found".to_string(),
        ),
        (
            "ListEventBridgeRuleTemplates filtered by a signal map",
            "GET",
            format!("{TEMPLATES}?signalMapIdentifier=no-such-map"),
            "Signal map no-such-map not found".to_string(),
        ),
    ];

    for (case, method, uri, expected) in rows {
        let (status, body) = call(&router, method, &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected the modelled NotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(expected),
            "case {case:?}: expected the message {expected:?}, got {:?}",
            body.get("message")
        );
    }
}
