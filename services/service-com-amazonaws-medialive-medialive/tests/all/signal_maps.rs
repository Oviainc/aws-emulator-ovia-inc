// SPDX-License-Identifier: CC0-1.0

//! MediaLive signal-map discovery and the wording of a not-found refusal, end to
//! end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each. The
//! wire harness (`medialive`, `call`, `error_code`) is the crate's shared one in
//! `wire`, and the clock wait is the channel suite's — reused, not copied.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::MAX_SIGNAL_MAPS;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::wire::{call, error_code, medialive, ACCOUNT};

/// The `message` member of a restJson1 error body (`jsonName: "message"` on every
/// one of this service's error shapes).
fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A case name as a resource name: a signal map's `Name` targets
/// `__stringMin1Max255PatternS`, whose pattern admits no whitespace, and it also
/// travels in `GetSignalMap`'s URI path.
fn slug(case: &str) -> String {
    case.replace(' ', "-")
}

async fn create_signal_map(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/prod/signal-maps", Some(body)).await
}

async fn get_signal_map(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/prod/signal-maps/{identifier}"),
        None,
    )
    .await
}

/// Build `names.len()` channels, linking the ones `links` names as
/// `(follower, primary)` index pairs, and answer their ARNs in creation order.
async fn channels_with_links(
    router: &MultiServiceRouter,
    names: &[&str],
    links: &[(usize, usize)],
) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let mut body = json!({ "name": name });
        if let Some((_, primary)) = links.iter().find(|(follower, _)| *follower == index) {
            // `LinkedChannelType` values come from
            // `smithy_query(query="shape", shape="LinkedChannelType")`:
            // FOLLOWING_CHANNEL | PRIMARY_CHANNEL.
            body["linkedChannelSettings"] = json!({
                "followerChannelSettings": {
                    "linkedChannelType": "FOLLOWING_CHANNEL",
                    "primaryChannelArn": arns[*primary],
                }
            });
        }
        let (status, created) = call(router, "POST", "/prod/channels", Some(body)).await;
        assert_eq!(
            status, 201,
            "CreateChannel {name:?}: expected 201, got {status}: {created}"
        );
        arns.push(
            created["channel"]["arn"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    arns
}

/// One row of the discovery table: a graph to build and what discovery must
/// find when it is entered from one of its channels.
struct Topology {
    case: &'static str,
    /// The channels to create, in order.
    names: &'static [&'static str],
    /// `(follower, primary)` index pairs; a follower's primary is created first.
    links: &'static [(usize, usize)],
    /// Which channel the signal map's discovery entry point names.
    entry: usize,
    /// How many resources discovery must find.
    resources: usize,
    /// The entry point's source and destination neighbour counts.
    neighbors: (usize, usize),
}

/// Rule: `CreateSignalMap` resolves `DiscoveryEntryPointArn` against the
/// resources that exist, stores a signal map whose `MediaResourceMap` is the
/// graph reached by walking the links those resources declare, reports it as
/// `CREATE_IN_PROGRESS` until the clock makes discovery due and
/// `CREATE_COMPLETE` with the discovered graph afterwards, and the stored signal
/// map reads back identically through `GetSignalMap`.
///
/// The rows differ along the SHAPE of the graph — how many resources it holds,
/// which one the request entered it from, and which direction the links run —
/// because that is what the rule generalises over. The last row carries a
/// channel that no link reaches, which discovery must therefore leave out.
#[tokio::test]
async fn signal_map_discovery_maps_the_graph_its_entry_point_reaches() {
    let table = [
        Topology {
            case: "lone channel",
            names: &["solo"],
            links: &[],
            entry: 0,
            resources: 1,
            neighbors: (0, 0),
        },
        Topology {
            case: "follower entered",
            names: &["lead", "follower"],
            links: &[(1, 0)],
            entry: 1,
            resources: 2,
            neighbors: (1, 0),
        },
        Topology {
            case: "primary entered",
            names: &["lead", "follower"],
            links: &[(1, 0)],
            entry: 0,
            resources: 2,
            neighbors: (0, 1),
        },
        Topology {
            case: "two followers",
            names: &["lead", "first", "second"],
            links: &[(1, 0), (2, 0)],
            entry: 0,
            resources: 3,
            neighbors: (0, 2),
        },
        Topology {
            case: "unlinked channel left out",
            names: &["solo", "unrelated"],
            links: &[],
            entry: 0,
            resources: 1,
            neighbors: (0, 0),
        },
    ];

    for Topology {
        case,
        names,
        links,
        entry,
        resources,
        neighbors: (sources, destinations),
    } in table
    {
        let router = medialive();
        let arns = channels_with_links(&router, names, links).await;
        let entry_arn = arns[entry].clone();
        let name = format!("map-{}", slug(case));

        let (status, created) = create_signal_map(
            &router,
            json!({
                "name": name,
                "description": format!("discovered from {case}"),
                "discoveryEntryPointArn": entry_arn,
                "tags": { "suite": "signal-maps" },
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected CreateSignalMap to answer 201, got {status}: {created}"
        );

        let id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            (7..=11).contains(&id.chars().count()),
            "case {case:?}: expected an id of the modelled length \
             (__stringMin7Max11PatternAws097), got {:?}",
            created["id"]
        );
        assert_eq!(
            created["arn"],
            json!(format!("arn:aws:medialive:us-west-2:{ACCOUNT}:signal-map:{id}")),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            created["arn"]
        );
        assert_eq!(
            created["discoveryEntryPointArn"],
            json!(entry_arn),
            "case {case:?}: expected the stored entry point, got {:?}",
            created["discoveryEntryPointArn"]
        );
        // Discovery is asynchronous, so a caller acting straight after the
        // create still sees the transient status and no graph.
        assert_eq!(
            created["status"],
            json!("CREATE_IN_PROGRESS"),
            "case {case:?}: expected a freshly created signal map to be CREATE_IN_PROGRESS, got {:?}",
            created["status"]
        );
        assert_eq!(
            created["mediaResourceMap"],
            Value::Null,
            "case {case:?}: expected no discovered graph before discovery is due, got {:?}",
            created["mediaResourceMap"]
        );

        wait_for_the_clock();
        let (status, read) = get_signal_map(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap to answer 200, got {status}: {read}"
        );
        assert_eq!(
            read["status"],
            json!("CREATE_COMPLETE"),
            "case {case:?}: expected discovery to have settled, got {:?}",
            read["status"]
        );
        assert_eq!(
            read["name"],
            json!(name),
            "case {case:?}: expected the stored name, got {:?}",
            read["name"]
        );
        assert_eq!(
            read["description"],
            json!(format!("discovered from {case}")),
            "case {case:?}: expected the stored description, got {:?}",
            read["description"]
        );
        assert_eq!(
            read["tags"],
            json!({ "suite": "signal-maps" }),
            "case {case:?}: expected the stored tags, got {:?}",
            read["tags"]
        );
        assert!(
            !read["lastDiscoveredAt"].is_null(),
            "case {case:?}: expected a discovery timestamp once discovery settled, got {:?}",
            read["lastDiscoveredAt"]
        );

        let discovered = read["mediaResourceMap"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            discovered.len(),
            resources,
            "case {case:?}: expected {resources} discovered resources, got {:?}",
            read["mediaResourceMap"]
        );
        assert!(
            discovered.contains_key(&entry_arn),
            "case {case:?}: expected the entry point itself to be discovered, got {:?}",
            discovered.keys().collect::<Vec<_>>()
        );
        for key in discovered.keys() {
            assert!(
                arns.contains(key),
                "case {case:?}: expected every discovered key to be a created channel's ARN, got {key:?}"
            );
        }
        // A neighbour a discovered resource names is itself discovered: the map
        // is the closure of the walk, not a single hop.
        for (key, resource) in &discovered {
            for side in ["sources", "destinations"] {
                for neighbor in resource[side].as_array().cloned().unwrap_or_default() {
                    let arn = neighbor["arn"].as_str().unwrap_or_default().to_string();
                    assert!(
                        discovered.contains_key(&arn),
                        "case {case:?}: {key:?}'s {side} names {arn:?}, which is not in the map"
                    );
                }
            }
        }
        let entry_resource = &discovered[&entry_arn];
        assert_eq!(
            entry_resource["sources"].as_array().map(Vec::len),
            Some(sources),
            "case {case:?}: expected {sources} sources for the entry point, got {:?}",
            entry_resource["sources"]
        );
        assert_eq!(
            entry_resource["destinations"].as_array().map(Vec::len),
            Some(destinations),
            "case {case:?}: expected {destinations} destinations for the entry point, got {:?}",
            entry_resource["destinations"]
        );
        assert_eq!(
            entry_resource["name"],
            json!(names[entry]),
            "case {case:?}: expected the discovered resource to carry the channel's name, got {:?}",
            entry_resource["name"]
        );

        // `Identifier` is "A signal map's identifier. Can be either be its id or
        // current name", so the name reads back the same signal map.
        let (status, by_name) = get_signal_map(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetSignalMap by name to answer 200, got {status}: {by_name}"
        );
        assert_eq!(
            by_name["id"],
            json!(id),
            "case {case:?}: expected the name to read back the same signal map, got {:?}",
            by_name["id"]
        );
    }
}

/// Rule: a discovery entry point that names no resource the signal map's graph
/// can be built from is refused with `NotFoundException` (404) and no signal map
/// is created; one that is not an AWS resource ARN at all is refused with
/// `BadRequestException` (400).
///
/// The rows differ along WHY the ARN resolves to no graph resource — the
/// resource id, the region, the account, the resource type and the service it
/// names, including a resource type this store DOES hold but builds no workflow
/// graph from — because the rule is about resolution, not about any one of those
/// segments. The issue's recorded case is the first row.
#[tokio::test]
async fn an_unresolvable_discovery_entry_point_creates_no_signal_map() {
    let router = medialive();
    let arns = channels_with_links(&router, &["existing"], &[]).await;
    let existing = arns[0].clone();
    let id = existing.rsplit(':').next().unwrap_or_default().to_string();

    // An input that really exists: a resource of this service, held by this very
    // store, that is a member of no workflow graph here.
    let (status, created_input) = call(
        &router,
        "POST",
        "/prod/inputs",
        Some(json!({
            "name": "stored-input",
            "type": "URL_PULL",
            "sources": [{ "url": "https://example.com/a.m3u8" }],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateInput: expected 201, got {status}: {created_input}"
    );
    let stored_input_arn = created_input["input"]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, entry-point ARN that resolves to no graph resource)
    let unresolvable: [(&str, String); 6] = [
        (
            "channel id that was never created",
            format!("arn:aws:medialive:us-west-2:{ACCOUNT}:channel:9999999"),
        ),
        (
            "existing channel id in another region",
            format!("arn:aws:medialive:eu-west-1:{ACCOUNT}:channel:{id}"),
        ),
        (
            "existing channel id in another account",
            format!("arn:aws:medialive:us-west-2:111122223333:channel:{id}"),
        ),
        (
            "another resource type of this service",
            format!("arn:aws:medialive:us-west-2:{ACCOUNT}:input:{id}"),
        ),
        ("an input this store really holds", stored_input_arn),
        (
            "another service's resource",
            format!("arn:aws:mediapackage:us-west-2:{ACCOUNT}:channels/demo"),
        ),
    ];

    for (case, entry_point) in unresolvable {
        let name = format!("unresolvable-{}", slug(case));
        let (status, body) = create_signal_map(
            &router,
            json!({ "name": name, "discoveryEntryPointArn": entry_point }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an entry point naming no resource, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert!(
            error_message(&body).contains(&entry_point),
            "case {case:?}: expected the message to name the requested ARN, got {:?}",
            error_message(&body)
        );

        // Nothing was left behind: the name the refused request asked for still
        // names no signal map.
        let (status, left_behind) = get_signal_map(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the refused create to leave no signal map, got {status}: {left_behind}"
        );
    }

    // (case, value that is not an AWS resource ARN)
    let malformed: [(&str, &str); 4] = [
        ("not an ARN", "not-an-arn"),
        ("too few segments", "arn:aws:medialive:us-west-2"),
        ("empty partition", "arn::medialive:us-west-2:000000000000:channel:1"),
        ("resource only", "channel:1234567"),
    ];

    for (case, entry_point) in malformed {
        let (status, body) = create_signal_map(
            &router,
            json!({ "name": format!("malformed-{}", slug(case)), "discoveryEntryPointArn": entry_point }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for an entry point that is not an ARN, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
    }
}

/// Rule: a not-found refusal's message is composed from the request — the
/// resource type, then the identifier the request named, then `not found`.
///
/// The rows span every operation of this service that resolves an identifier of
/// its own (the four channel operations, `DescribeInput`, `GetSignalMap`) and a
/// set of identifiers differing in length and character class, because the rule
/// generalises over both. The issue's recorded case is the `describe` row of
/// `999999999`.
#[tokio::test]
async fn a_not_found_message_names_the_resource_type_then_the_identifier() {
    // (case, resource type in the message, method, uri) — every implemented
    // operation that resolves an identifier of its own.
    let operations: [(&str, &str, &str, &str); 5] = [
        ("describe channel", "Channel", "GET", "/prod/channels/{id}"),
        ("start channel", "Channel", "POST", "/prod/channels/{id}/start"),
        ("stop channel", "Channel", "POST", "/prod/channels/{id}/stop"),
        ("delete channel", "Channel", "DELETE", "/prod/channels/{id}"),
        ("describe input", "Input", "GET", "/prod/inputs/{id}"),
    ];
    let identifiers = ["999999999", "0000000", "1234567", "no-such-resource"];

    let router = medialive();
    for id in identifiers {
        for (case, resource_type, method, uri) in operations {
            let (status, body) = call(&router, method, &uri.replace("{id}", id), None).await;
            assert_eq!(
                status, 404,
                "case {case:?} on {id:?}: expected 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(status, &body),
                "NotFoundException",
                "case {case:?} on {id:?}: expected NotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!("{resource_type} {id} not found"),
                "case {case:?} on {id:?}: expected the message to read \
                 \"{resource_type} {id} not found\", got {:?}",
                error_message(&body)
            );
        }
    }

    for identifier in ["0000000", "no-such-map"] {
        let (status, body) = get_signal_map(&router, identifier).await;
        assert_eq!(
            status, 404,
            "case {identifier:?}: expected 404 for an unknown signal map, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {identifier:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Signal map {identifier} not found"),
            "case {identifier:?}: expected the message to read \
             \"Signal map {identifier} not found\", got {:?}",
            error_message(&body)
        );
    }
}

/// What the second create of a row must answer.
enum Expectation {
    /// The signal map the first create made.
    SameSignalMap,
    /// A second signal map of its own.
    AnotherSignalMap,
    /// A refusal: the name is already in use.
    Conflict,
}

/// One row of the repeated-create table.
struct Retry {
    case: &'static str,
    first_token: Option<&'static str>,
    second_token: Option<&'static str>,
    second_name: &'static str,
    expectation: Expectation,
}

/// Rule: `CreateSignalMap`'s `RequestId` is an `@idempotencyToken`, so two
/// creates carrying the same one yield one signal map; a `Name` is unique within
/// the region, so a second create taking a name already in use is a conflict;
/// and a group identifier that names no template group is refused rather than
/// answered with group ids nothing created.
#[tokio::test]
async fn repeated_creates_and_unresolvable_group_identifiers_are_refused() {
    let router = medialive();
    let arns = channels_with_links(&router, &["entry"], &[]).await;
    let entry_point = arns[0].clone();

    // (case, first request id, second request id, second name, expectation)
    let table = [
        Retry {
            case: "same token",
            first_token: Some("token-a"),
            second_token: Some("token-a"),
            second_name: "second",
            expectation: Expectation::SameSignalMap,
        },
        Retry {
            case: "different tokens",
            first_token: Some("token-a"),
            second_token: Some("token-b"),
            second_name: "second",
            expectation: Expectation::AnotherSignalMap,
        },
        Retry {
            case: "name already used",
            first_token: None,
            second_token: None,
            second_name: "first",
            expectation: Expectation::Conflict,
        },
    ];

    for Retry {
        case,
        first_token,
        second_token,
        second_name,
        expectation,
    } in table
    {
        let router = medialive();
        let arns = channels_with_links(&router, &["entry"], &[]).await;
        let entry = arns[0].clone();

        let mut first = json!({ "name": "first", "discoveryEntryPointArn": entry });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let (status, created) = create_signal_map(&router, first).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the first create to answer 201, got {status}: {created}"
        );
        let first_id = created["id"].as_str().unwrap_or_default().to_string();

        let mut second = json!({ "name": second_name, "discoveryEntryPointArn": entry });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }
        let (status, again) = create_signal_map(&router, second).await;

        match expectation {
            Expectation::SameSignalMap => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected a retried create to answer 201, got {status}: {again}"
                );
                assert_eq!(
                    again["id"],
                    json!(first_id),
                    "case {case:?}: expected the retry to return the first signal map, got {:?}",
                    again["id"]
                );
            }
            Expectation::AnotherSignalMap => {
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected a distinct create to answer 201, got {status}: {again}"
                );
                assert_ne!(
                    again["id"],
                    json!(first_id),
                    "case {case:?}: expected a second signal map, got {:?}",
                    again["id"]
                );
            }
            Expectation::Conflict => {
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected a reused name to be refused, got {status}: {again}"
                );
                assert_eq!(
                    error_code(status, &again),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {again}"
                );
            }
        }
    }

    // A group identifier resolves against the groups this store really holds:
    // both template-group creates are implemented here, so a signal map may
    // name a group by id or by current name and the response reports the
    // RESOLVED id, while an identifier naming no stored group of that type is
    // unresolvable. The alarm side of that rule is covered over a store that
    // really holds alarm groups by `signal_map_listing`.
    let (status, group) = call(
        &router,
        "POST",
        "/prod/eventbridge-rule-template-groups",
        Some(json!({ "name": "rule-group" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateEventBridgeRuleTemplateGroup: expected 201, got {status}: {group}"
    );
    let group_id = group["id"].as_str().unwrap_or_default().to_string();

    // (case, member, identifier, resolved ids the response must report)
    let resolvable: [(&str, &str, &str, &str); 2] = [
        (
            "eventbridge group by id",
            "eventBridgeRuleTemplateGroupIdentifiers",
            &group_id,
            &group_id,
        ),
        (
            "eventbridge group by name",
            "eventBridgeRuleTemplateGroupIdentifiers",
            "rule-group",
            &group_id,
        ),
    ];
    for (case, member, identifier, resolved) in resolvable {
        let mut body = json!({
            "name": format!("grouped-{}", slug(case)),
            "discoveryEntryPointArn": entry_point,
        });
        body[member] = json!([identifier]);
        let (status, created) = create_signal_map(&router, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected an identifier naming a stored group to be accepted, got {status}: {created}"
        );
        assert_eq!(
            created["eventBridgeRuleTemplateGroupIds"],
            json!([resolved]),
            "case {case:?}: expected the resolved group id, got {:?}",
            created["eventBridgeRuleTemplateGroupIds"]
        );
    }

    // (case, the member carrying an identifier that resolves to no group)
    let unresolvable: [(&str, &str, &str); 3] = [
        (
            "alarm group by name",
            "cloudWatchAlarmTemplateGroupIdentifiers",
            "group-1",
        ),
        (
            // Even the id of a group of ANOTHER type resolves to no alarm group.
            "alarm group by a stored eventbridge group's id",
            "cloudWatchAlarmTemplateGroupIdentifiers",
            &group_id,
        ),
        (
            "eventbridge group that was never created",
            "eventBridgeRuleTemplateGroupIdentifiers",
            "no-such-group",
        ),
    ];
    for (case, member, identifier) in unresolvable {
        let mut body = json!({
            "name": format!("grouped-{}", slug(case)),
            "discoveryEntryPointArn": entry_point,
        });
        body[member] = json!([identifier]);
        let (status, refused) = create_signal_map(&router, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected an identifier naming no template group to be refused, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {refused}"
        );
        assert!(
            error_message(&refused).contains(identifier),
            "case {case:?}: expected the message to name the identifier, got {:?}",
            error_message(&refused)
        );
    }
}

/// Rule: the signal-map store is bounded, so a create loop is refused with the
/// operation's modelled throttle rather than growing the emulator's heap.
#[tokio::test]
async fn the_signal_map_store_is_bounded() {
    let router = medialive();
    let arns = channels_with_links(&router, &["entry"], &[]).await;
    let entry_point = arns[0].clone();

    for index in 0..MAX_SIGNAL_MAPS {
        let (status, created) = create_signal_map(
            &router,
            json!({
                "name": format!("map-{index}"),
                "discoveryEntryPointArn": entry_point,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {index}: expected a create below the cap to answer 201, got {status}: {created}"
        );
    }

    let (status, refused) = create_signal_map(
        &router,
        json!({
            "name": "one-too-many",
            "discoveryEntryPointArn": entry_point,
        }),
    )
    .await;
    assert_eq!(
        status, 429,
        "expected the create past the cap to be throttled, got {status}: {refused}"
    );
    assert_eq!(
        error_code(status, &refused),
        "TooManyRequestsException",
        "expected TooManyRequestsException, got {refused}"
    );
}
