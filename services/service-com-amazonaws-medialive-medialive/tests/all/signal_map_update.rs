// SPDX-License-Identifier: CC0-1.0

//! MediaLive `StartUpdateSignalMap`, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: one recorded call is ONE row of each. The wire harness
//! (`medialive`, `call`, `error_code`, `error_message`) is the crate's shared one
//! in `wire`, the signal-map readers are the monitor-deployment suite's and the
//! clock wait is the channel suite's — reused, not copied.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::monitor_deployment::{get_signal_map, listed_summary, start_monitor_deployment};
use crate::support::{entry_point, epoch_seconds, timestamp};
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT, DEFAULT_REGION};

/// The collection URI `CreateSignalMap` is bound to, and the prefix of the one
/// `StartUpdateSignalMap` PATCHes (`smithy_query(query="operation",
/// operation="CreateSignalMap,StartUpdateSignalMap")`: POST /prod/signal-maps,
/// PATCH /prod/signal-maps/{Identifier}).
const SIGNAL_MAPS: &str = "/prod/signal-maps";

/// The collection URIs of the two template-group resources a signal map can be
/// associated with (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplateGroup,CreateEventBridgeRuleTemplateGroup")`).
const ALARM_GROUPS: &str = "/prod/cloudwatch-alarm-template-groups";
const RULE_GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// `StartUpdateSignalMap` on `identifier`, with `body` sent verbatim (every
/// input member but the `@httpLabel` `Identifier` travels in the payload).
async fn start_update(
    router: &MultiServiceRouter,
    identifier: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("{SIGNAL_MAPS}/{identifier}"),
        Some(body),
    )
    .await
}

/// One template group on `uri`, answering its minted id.
async fn create_group(router: &MultiServiceRouter, uri: &str, name: &str) -> String {
    let (status, created) = call(router, "POST", uri, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "setup: expected the group {name:?} on {uri} to answer 201, got {status}: {created}"
    );
    created["id"].as_str().unwrap_or_default().to_string()
}

/// The store every row starts from: a signal map whose discovery has settled, a
/// follower of its entry point created AFTER that discovery (so only a
/// rediscovery finds it), and an unrelated channel to move the entry point to.
struct Fixture {
    id: String,
    name: String,
    arn: String,
    created_at: String,
    /// The `LastDiscoveredAt` the settled create left behind.
    last_discovered_at: String,
    /// The channel the signal map was created on — the single resource its
    /// stored graph holds.
    stored_entry: String,
    /// The follower of `stored_entry`, created after the signal map.
    follower: String,
    /// An unrelated channel, whose graph is exactly itself.
    other_entry: String,
}

/// The description every fixture is created with, so a row that sends no
/// `Description` can be checked to have left it alone.
const CREATED_DESCRIPTION: &str = "before-the-update";

/// The tags every fixture is created with: no update member carries tags, so
/// they must survive every row.
fn created_tags() -> Value {
    json!({ "suite": "signal-map-update" })
}

async fn fixture(router: &MultiServiceRouter, case: &str, name: &str) -> Fixture {
    let stored_entry = entry_point(router, &format!("{name}-entry")).await;
    let (status, created) = call(
        router,
        "POST",
        SIGNAL_MAPS,
        Some(json!({
            "name": name,
            "description": CREATED_DESCRIPTION,
            "discoveryEntryPointArn": stored_entry,
            "tags": created_tags(),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected CreateSignalMap to answer 201, got {status}: {created}"
    );
    let id = created["id"].as_str().unwrap_or_default().to_string();
    let arn = created["arn"].as_str().unwrap_or_default().to_string();
    let created_at = created["createdAt"].as_str().unwrap_or_default().to_string();

    // A follower of the entry point, created after the signal map: a stored
    // graph is the snapshot its discovery took, so only a rediscovery finds it.
    // The linked-channel values come from `smithy_query(query="shape",
    // shape="LinkedChannelType")`: FOLLOWING_CHANNEL | PRIMARY_CHANNEL.
    let (status, linked) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": format!("{name}-follower"),
            "linkedChannelSettings": {
                "followerChannelSettings": {
                    "linkedChannelType": "FOLLOWING_CHANNEL",
                    "primaryChannelArn": stored_entry,
                }
            },
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected the follower channel to answer 201, got {status}: {linked}"
    );
    let follower = linked["channel"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let other_entry = entry_point(router, &format!("{name}-other")).await;

    wait_for_the_clock();
    let (status, settled) = get_signal_map(router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetSignalMap to answer 200, got {status}: {settled}"
    );
    assert_eq!(
        settled["status"],
        json!("CREATE_COMPLETE"),
        "case {case:?}: expected the create's discovery to have settled, got {:?}",
        settled["status"]
    );
    assert_eq!(
        graph_keys(case, &settled),
        vec![stored_entry.clone()],
        "case {case:?}: expected the stored graph to hold the entry point alone"
    );

    Fixture {
        id,
        name: name.to_string(),
        arn,
        created_at,
        last_discovered_at: settled["lastDiscoveredAt"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        stored_entry,
        follower,
        other_entry,
    }
}

/// The ARNs a response's `MediaResourceMap` is keyed by, in sorted order.
fn graph_keys(case: &str, body: &Value) -> Vec<String> {
    let mut keys: Vec<String> = body["mediaResourceMap"]
        .as_object()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a mediaResourceMap object, got {:?}",
                body["mediaResourceMap"]
            )
        })
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// Which `DiscoveryEntryPointArn` a row's request carries.
#[derive(Clone, Copy, PartialEq)]
enum Entry {
    /// The member is left out, so the signal map keeps the entry point it has.
    Omitted,
    /// The entry point the signal map already has — unchanged, so it discovers
    /// nothing on its own.
    Stored,
    /// A different channel, which is a CHANGED entry point.
    Other,
}

/// One row of the update table: which members the request carries, and what the
/// signal map must look like once the update it starts has settled.
struct Update {
    case: &'static str,
    /// Whether the request renames the signal map.
    rename: bool,
    description: Option<&'static str>,
    entry: Entry,
    force_rediscovery: Option<bool>,
    /// Whether the request associates the signal map with a template group of
    /// each kind.
    groups: bool,
    /// Whether the call addresses the signal map by its current name rather
    /// than its id (`Identifier` is "either be its id or current name").
    by_name: bool,
    /// Whether the update rediscovers the graph, which is what moves
    /// `LastDiscoveredAt`.
    rediscovers: bool,
    /// The channels the graph must hold once the update has settled, as fixture
    /// aliases.
    graph: &'static [&'static str],
}

/// The ARNs the fixture's channel aliases name.
fn aliased(fixture: &Fixture, aliases: &[&str]) -> Vec<String> {
    let mut arns: Vec<String> = aliases
        .iter()
        .map(|alias| match *alias {
            "entry" => fixture.stored_entry.clone(),
            "follower" => fixture.follower.clone(),
            "other" => fixture.other_entry.clone(),
            unknown => panic!("the table names no channel {unknown:?}"),
        })
        .collect();
    arns.sort();
    arns
}

/// Rule: `StartUpdateSignalMap` initiates an update of the signal map its
/// `Identifier` resolves to — replacing every member the request carried and
/// leaving every member it omitted as it was — answers 202 with that signal map
/// as the store now holds it in `UPDATE_IN_PROGRESS`, and the clock settles that
/// into `UPDATE_COMPLETE`; the graph is rediscovered when the request supplies a
/// CHANGED entry point or forces a rediscovery, and only then does
/// `LastDiscoveredAt` move.
///
/// The rows differ along what the rule generalises over: WHICH members the
/// request carries (none, a name, a description, template groups, an entry
/// point), whether the entry point it carries is the stored one or a different
/// one, whether a rediscovery is forced and how, and whether the call addresses
/// the signal map by its id or by its name.
#[tokio::test]
async fn an_update_replaces_the_members_the_request_carried_and_keeps_the_rest() {
    let table = [
        Update {
            case: "nothing but the identifier",
            rename: false,
            description: None,
            entry: Entry::Omitted,
            force_rediscovery: None,
            groups: false,
            by_name: false,
            rediscovers: false,
            graph: &["entry"],
        },
        Update {
            case: "rename only",
            rename: true,
            description: None,
            entry: Entry::Omitted,
            force_rediscovery: None,
            groups: false,
            by_name: true,
            rediscovers: false,
            graph: &["entry"],
        },
        Update {
            case: "description only",
            rename: false,
            description: Some("after-the-update"),
            entry: Entry::Omitted,
            force_rediscovery: None,
            groups: false,
            by_name: false,
            rediscovers: false,
            graph: &["entry"],
        },
        Update {
            case: "empty description",
            rename: false,
            description: Some(""),
            entry: Entry::Omitted,
            force_rediscovery: None,
            groups: false,
            by_name: false,
            rediscovers: false,
            graph: &["entry"],
        },
        Update {
            case: "the stored entry point without forcing",
            rename: false,
            description: None,
            entry: Entry::Stored,
            force_rediscovery: Some(false),
            groups: false,
            by_name: false,
            rediscovers: false,
            graph: &["entry"],
        },
        Update {
            case: "forced rediscovery of the stored entry point",
            rename: false,
            description: None,
            entry: Entry::Stored,
            force_rediscovery: Some(true),
            groups: false,
            by_name: false,
            rediscovers: true,
            graph: &["entry", "follower"],
        },
        Update {
            case: "forced rediscovery with no entry point in the request",
            rename: true,
            description: None,
            entry: Entry::Omitted,
            force_rediscovery: Some(true),
            groups: false,
            by_name: true,
            rediscovers: true,
            graph: &["entry", "follower"],
        },
        Update {
            case: "changed entry point",
            rename: false,
            description: Some("moved-to-another-channel"),
            entry: Entry::Other,
            force_rediscovery: None,
            groups: false,
            by_name: false,
            rediscovers: true,
            graph: &["other"],
        },
        Update {
            case: "changed entry point addressed by name",
            rename: true,
            description: None,
            entry: Entry::Other,
            force_rediscovery: Some(false),
            groups: false,
            by_name: true,
            rediscovers: true,
            graph: &["other"],
        },
        Update {
            case: "template groups and a rename",
            rename: true,
            description: Some("grouped"),
            entry: Entry::Omitted,
            force_rediscovery: None,
            groups: true,
            by_name: false,
            rediscovers: false,
            graph: &["entry"],
        },
    ];

    for Update {
        case,
        rename,
        description,
        entry,
        force_rediscovery,
        groups,
        by_name,
        rediscovers,
        graph,
    } in table
    {
        let router = medialive();
        let name = format!("update-{}", case.replace(' ', "-"));
        let fixture = fixture(&router, case, &name).await;

        let mut body = json!({});
        let expected_name = if rename {
            let renamed = format!("{name}-renamed");
            body["name"] = json!(renamed);
            renamed
        } else {
            fixture.name.clone()
        };
        let expected_description = match description {
            Some(text) => {
                body["description"] = json!(text);
                text.to_string()
            }
            None => CREATED_DESCRIPTION.to_string(),
        };
        let expected_entry = match entry {
            Entry::Omitted => fixture.stored_entry.clone(),
            Entry::Stored => {
                body["discoveryEntryPointArn"] = json!(fixture.stored_entry);
                fixture.stored_entry.clone()
            }
            Entry::Other => {
                body["discoveryEntryPointArn"] = json!(fixture.other_entry);
                fixture.other_entry.clone()
            }
        };
        if let Some(force) = force_rediscovery {
            body["forceRediscovery"] = json!(force);
        }
        let (expected_alarm_ids, expected_rule_ids) = if groups {
            let alarm = create_group(&router, ALARM_GROUPS, &format!("{name}-alarms")).await;
            let rule = create_group(&router, RULE_GROUPS, &format!("{name}-rules")).await;
            // A group identifier is "either be its id or current name", so one
            // of each form travels here and both must resolve to a group ID.
            body["cloudWatchAlarmTemplateGroupIdentifiers"] = json!([alarm]);
            body["eventBridgeRuleTemplateGroupIdentifiers"] = json!([format!("{name}-rules")]);
            (json!([alarm]), json!([rule]))
        } else {
            (Value::Null, Value::Null)
        };

        let identifier = if by_name {
            fixture.name.clone()
        } else {
            fixture.id.clone()
        };
        let (status, started) = start_update(&router, &identifier, body.clone()).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected StartUpdateSignalMap to answer 202 for {body}, \
             got {status}: {started}"
        );
        assert_eq!(
            started["status"],
            json!("UPDATE_IN_PROGRESS"),
            "case {case:?}: expected the update to be running, got {:?}",
            started["status"]
        );
        // The identity of the resource is untouched by an update of it.
        assert_eq!(
            started["id"],
            json!(fixture.id),
            "case {case:?}: expected the response to be the signal map {:?}, got {:?}",
            fixture.id,
            started["id"]
        );
        assert_eq!(
            started["arn"],
            json!(fixture.arn),
            "case {case:?}: expected the ARN to be unchanged, got {:?}",
            started["arn"]
        );
        assert_eq!(
            started["createdAt"],
            json!(fixture.created_at),
            "case {case:?}: expected createdAt to be unchanged, got {:?}",
            started["createdAt"]
        );
        assert_eq!(
            started["name"],
            json!(expected_name),
            "case {case:?}: expected name {expected_name:?}, got {:?}",
            started["name"]
        );
        assert_eq!(
            started["description"],
            json!(expected_description),
            "case {case:?}: expected description {expected_description:?}, got {:?}",
            started["description"]
        );
        assert_eq!(
            started["discoveryEntryPointArn"],
            json!(expected_entry),
            "case {case:?}: expected discoveryEntryPointArn {expected_entry:?}, got {:?}",
            started["discoveryEntryPointArn"]
        );
        assert_eq!(
            started["tags"],
            created_tags(),
            "case {case:?}: expected the tags the create stored to survive, got {:?}",
            started["tags"]
        );
        assert_eq!(
            started["cloudWatchAlarmTemplateGroupIds"], expected_alarm_ids,
            "case {case:?}: expected cloudWatchAlarmTemplateGroupIds {expected_alarm_ids}, got {:?}",
            started["cloudWatchAlarmTemplateGroupIds"]
        );
        assert_eq!(
            started["eventBridgeRuleTemplateGroupIds"], expected_rule_ids,
            "case {case:?}: expected eventBridgeRuleTemplateGroupIds {expected_rule_ids}, got {:?}",
            started["eventBridgeRuleTemplateGroupIds"]
        );
        // While the update runs the signal map still maps the graph its last
        // completed discovery found, discovered at the instant it completed.
        assert_eq!(
            graph_keys(case, &started),
            vec![fixture.stored_entry.clone()],
            "case {case:?}: expected the graph to be the one the create discovered while the \
             update runs"
        );
        assert_eq!(
            started["lastDiscoveredAt"],
            json!(fixture.last_discovered_at),
            "case {case:?}: expected lastDiscoveredAt to stand until a rediscovery completes, \
             got {:?}",
            started["lastDiscoveredAt"]
        );
        let summary = listed_summary(&router, case, &fixture.id).await;
        assert_eq!(
            summary["status"],
            json!("UPDATE_IN_PROGRESS"),
            "case {case:?}: expected the summary to report the running update, got {:?}",
            summary["status"]
        );

        // The update is asynchronous, so it completes on the clock and not
        // because something read the signal map.
        let (status, polled) = get_signal_map(&router, &fixture.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {polled}"
        );
        assert_eq!(
            polled["status"],
            json!("UPDATE_IN_PROGRESS"),
            "case {case:?}: expected reading the signal map not to complete its update, got {:?}",
            polled["status"]
        );

        wait_for_the_clock();
        let (status, settled) = get_signal_map(&router, &expected_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated name to read the signal map back, \
             got {status}: {settled}"
        );
        assert_eq!(
            settled["id"],
            json!(fixture.id),
            "case {case:?}: expected the name to resolve the same signal map, got {:?}",
            settled["id"]
        );
        assert_eq!(
            settled["status"],
            json!("UPDATE_COMPLETE"),
            "case {case:?}: expected the update to have settled, got {:?}",
            settled["status"]
        );
        assert_eq!(
            graph_keys(case, &settled),
            aliased(&fixture, graph),
            "case {case:?}: expected the graph {graph:?} once the update settled"
        );
        assert_eq!(
            settled["description"],
            json!(expected_description),
            "case {case:?}: expected the stored description to read back, got {:?}",
            settled["description"]
        );
        assert_eq!(
            settled["discoveryEntryPointArn"],
            json!(expected_entry),
            "case {case:?}: expected the stored entry point to read back, got {:?}",
            settled["discoveryEntryPointArn"]
        );
        assert_eq!(
            settled["tags"],
            created_tags(),
            "case {case:?}: expected the tags to read back, got {:?}",
            settled["tags"]
        );
        assert_eq!(
            settled["cloudWatchAlarmTemplateGroupIds"], expected_alarm_ids,
            "case {case:?}: expected the resolved alarm group ids to read back, got {:?}",
            settled["cloudWatchAlarmTemplateGroupIds"]
        );
        assert_eq!(
            settled["eventBridgeRuleTemplateGroupIds"], expected_rule_ids,
            "case {case:?}: expected the resolved rule group ids to read back, got {:?}",
            settled["eventBridgeRuleTemplateGroupIds"]
        );

        // `LastDiscoveredAt` moves only when the graph was rediscovered; the
        // update itself always moves `ModifiedAt`, which stays at or after the
        // creation instant.
        let rediscovered_at = settled["lastDiscoveredAt"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if rediscovers {
            assert_ne!(
                rediscovered_at, fixture.last_discovered_at,
                "case {case:?}: expected a rediscovery to stamp a new lastDiscoveredAt"
            );
        } else {
            assert_eq!(
                rediscovered_at, fixture.last_discovered_at,
                "case {case:?}: expected no rediscovery to leave lastDiscoveredAt alone"
            );
        }
        let created_at = epoch_seconds(timestamp(case, "createdAt", &settled));
        let modified_at = epoch_seconds(timestamp(case, "modifiedAt", &settled));
        assert!(
            modified_at >= created_at,
            "case {case:?}: expected modifiedAt {modified_at} at or after createdAt {created_at}"
        );
        let summary = listed_summary(&router, case, &fixture.id).await;
        assert_eq!(
            summary["status"],
            json!("UPDATE_COMPLETE"),
            "case {case:?}: expected the summary to report the settled update, got {:?}",
            summary["status"]
        );
        assert_eq!(
            summary["name"],
            json!(expected_name),
            "case {case:?}: expected the summary to report the stored name, got {:?}",
            summary["name"]
        );
    }
}

/// One row of the refusal table: the request to make and the refusal it must
/// draw.
struct Refusal {
    case: &'static str,
    identifier: String,
    body: Value,
    status: u16,
    code: &'static str,
    /// A fragment the message must carry, when the class of refusal composes its
    /// message out of the request.
    message: Option<String>,
}

/// Rule: an update whose request cannot be satisfied is refused and changes
/// nothing — the modelled 404 for an identifier (of the signal map, of a
/// discovery entry point or of a template group) that resolves to nothing, the
/// modelled 400 for a `DiscoveryEntryPointArn` that is not an AWS resource ARN at
/// all, and the modelled 409 for a name another signal map holds.
///
/// The rows differ along WHY the request cannot be satisfied and along which
/// identifier form it got wrong, because the rule is about resolution and not
/// about any one value.
#[tokio::test]
async fn a_refused_update_leaves_the_signal_map_as_it_was() {
    let router = medialive();
    let fixture = fixture(&router, "refusals", "refusals").await;

    // A second signal map, so a rename can collide with a name that is taken.
    let taken_entry = entry_point(&router, "taken-entry").await;
    let (status, taken) = call(
        &router,
        "POST",
        SIGNAL_MAPS,
        Some(json!({ "name": "already-taken", "discoveryEntryPointArn": taken_entry })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected the second CreateSignalMap to answer 201, got {status}: {taken}"
    );

    let unresolvable_channel = format!("arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:channel:9999999");
    let other_service = format!("arn:aws:mediapackage:{DEFAULT_REGION}:{ACCOUNT}:channels/demo");
    let table = [
        Refusal {
            case: "a signal-map id nobody minted",
            identifier: "4999999".to_string(),
            body: json!({ "description": "no such map" }),
            status: 404,
            code: "NotFoundException",
            message: Some("4999999".to_string()),
        },
        Refusal {
            case: "a channel's id",
            identifier: "1000000".to_string(),
            body: json!({}),
            status: 404,
            code: "NotFoundException",
            message: Some("1000000".to_string()),
        },
        Refusal {
            case: "a name no signal map carries",
            identifier: "no-such-map".to_string(),
            body: json!({ "name": "renamed-by-a-miss" }),
            status: 404,
            code: "NotFoundException",
            message: Some("no-such-map".to_string()),
        },
        Refusal {
            case: "the signal map's ARN rather than an identifier",
            identifier: fixture.arn.replace(':', "%3A"),
            body: json!({}),
            status: 404,
            code: "NotFoundException",
            message: None,
        },
        Refusal {
            case: "an entry point that is not an ARN",
            identifier: fixture.id.clone(),
            body: json!({ "discoveryEntryPointArn": "not-an-arn" }),
            status: 400,
            code: "BadRequestException",
            message: Some("not-an-arn".to_string()),
        },
        Refusal {
            case: "an entry point with too few ARN segments",
            identifier: fixture.id.clone(),
            body: json!({ "discoveryEntryPointArn": "arn:aws:medialive:us-west-2" }),
            status: 400,
            code: "BadRequestException",
            message: None,
        },
        Refusal {
            case: "an entry point that is a bare resource",
            identifier: fixture.name.clone(),
            body: json!({ "discoveryEntryPointArn": "channel:1234567" }),
            status: 400,
            code: "BadRequestException",
            message: None,
        },
        Refusal {
            case: "an entry point naming a channel nobody created",
            identifier: fixture.id.clone(),
            body: json!({ "discoveryEntryPointArn": unresolvable_channel.clone() }),
            status: 404,
            code: "NotFoundException",
            message: Some(unresolvable_channel),
        },
        Refusal {
            case: "an entry point in another service",
            identifier: fixture.name.clone(),
            body: json!({ "discoveryEntryPointArn": other_service.clone() }),
            status: 404,
            code: "NotFoundException",
            message: Some(other_service),
        },
        Refusal {
            case: "a forced rediscovery of an entry point that no longer resolves",
            identifier: fixture.id.clone(),
            body: json!({
                "discoveryEntryPointArn": format!("arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:channel:1000009"),
                "forceRediscovery": true,
            }),
            status: 404,
            code: "NotFoundException",
            message: None,
        },
        Refusal {
            case: "a name another signal map holds",
            identifier: fixture.id.clone(),
            body: json!({ "name": "already-taken" }),
            status: 409,
            code: "ConflictException",
            message: Some("Name already in use".to_string()),
        },
        Refusal {
            case: "an alarm template group identifier naming no group",
            identifier: fixture.id.clone(),
            body: json!({ "cloudWatchAlarmTemplateGroupIdentifiers": ["no-such-group"] }),
            status: 404,
            code: "NotFoundException",
            message: None,
        },
        Refusal {
            case: "a rule template group identifier naming no group",
            identifier: fixture.id.clone(),
            body: json!({ "eventBridgeRuleTemplateGroupIdentifiers": ["no-such-group"] }),
            status: 404,
            code: "NotFoundException",
            message: None,
        },
    ];

    for Refusal {
        case,
        identifier,
        body,
        status: expected_status,
        code,
        message,
    } in table
    {
        let (status, refused) = start_update(&router, &identifier, body.clone()).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status} for {body}, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            code,
            "case {case:?}: expected {code}, got {refused}"
        );
        if let Some(fragment) = message {
            assert!(
                error_message(&refused).contains(&fragment),
                "case {case:?}: expected the message to carry {fragment:?}, got {:?}",
                error_message(&refused)
            );
        }

        // Nothing the refused call named was changed: the signal map still
        // carries what the create stored, in the status the create left it in.
        let (status, untouched) = get_signal_map(&router, &fixture.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {untouched}"
        );
        assert_eq!(
            untouched["status"],
            json!("CREATE_COMPLETE"),
            "case {case:?}: expected the refused call to start no update, got {:?}",
            untouched["status"]
        );
        assert_eq!(
            untouched["name"],
            json!(fixture.name),
            "case {case:?}: expected the name to be untouched, got {:?}",
            untouched["name"]
        );
        assert_eq!(
            untouched["description"],
            json!(CREATED_DESCRIPTION),
            "case {case:?}: expected the description to be untouched, got {:?}",
            untouched["description"]
        );
        assert_eq!(
            untouched["discoveryEntryPointArn"],
            json!(fixture.stored_entry),
            "case {case:?}: expected the entry point to be untouched, got {:?}",
            untouched["discoveryEntryPointArn"]
        );
        assert_eq!(
            untouched["cloudWatchAlarmTemplateGroupIds"],
            Value::Null,
            "case {case:?}: expected no group association to have been written, got {:?}",
            untouched["cloudWatchAlarmTemplateGroupIds"]
        );
        assert_eq!(
            graph_keys(case, &untouched),
            vec![fixture.stored_entry.clone()],
            "case {case:?}: expected the stored graph to be untouched"
        );
    }
}

/// Rule: a signal map whose own lifecycle action is still in flight is not in a
/// state another one may take it out of, so a second update, a monitor
/// deployment and a delete are each refused with `ConflictException` (409) while
/// it runs, and each is served once the clock has settled that action.
///
/// The rows are the two lifecycle actions a signal map runs — the discovery its
/// create starts and the update this operation starts — because the rule is
/// about an action being in flight, not about which one it is.
#[tokio::test]
async fn a_running_lifecycle_action_refuses_the_next_one_until_it_settles() {
    // (case, whether the signal map is left in its create's discovery)
    for (case, during_create) in [("during the create", true), ("during an update", false)] {
        let router = medialive();
        let name = format!("busy-{}", case.replace(' ', "-"));
        let running_status = if during_create {
            // A signal map created and read straight away is still discovering.
            let stored_entry = entry_point(&router, &format!("{name}-entry")).await;
            let (status, created) = call(
                &router,
                "POST",
                SIGNAL_MAPS,
                Some(json!({ "name": name, "discoveryEntryPointArn": stored_entry })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected CreateSignalMap to answer 201, got {status}: {created}"
            );
            "CREATE_IN_PROGRESS"
        } else {
            let fixture = fixture(&router, case, &name).await;
            let (status, started) =
                start_update(&router, &fixture.id, json!({ "description": "busy" })).await;
            assert_eq!(
                status, 202,
                "case {case:?}: expected the first update to answer 202, got {status}: {started}"
            );
            "UPDATE_IN_PROGRESS"
        };

        let (status, refused) = start_update(&router, &name, json!({ "forceRediscovery": true })).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 while {running_status}, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        assert!(
            error_message(&refused).contains(running_status),
            "case {case:?}: expected the message to name the status it is in, got {:?}",
            error_message(&refused)
        );

        let (status, deployment) = start_monitor_deployment(&router, &name, None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a monitor deployment to be refused while {running_status}, \
             got {status}: {deployment}"
        );
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("{SIGNAL_MAPS}/{name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a delete to be refused while {running_status}, \
             got {status}: {deleted}"
        );

        // The same calls are served once the clock has settled the action, so
        // the refusal is about the state and not about the request.
        wait_for_the_clock();
        let (status, served) = start_update(&router, &name, json!({ "forceRediscovery": true })).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the same update to answer 202 once settled, \
             got {status}: {served}"
        );
        wait_for_the_clock();
        let (status, deployment) = start_monitor_deployment(&router, &name, None).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected a monitor deployment of an updated signal map to answer 202, \
             got {status}: {deployment}"
        );
    }
}
