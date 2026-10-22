// SPDX-License-Identifier: CC0-1.0

//! The MediaLive monitor template DELETES, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables over the THREE monitor template resources whose delete
//! is implemented (the cloudwatch alarm template group, the eventbridge rule
//! template group and the eventbridge rule template): the recorded case in the
//! issue is one row of each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::percent_encoded;
use crate::wire::{call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION as REGION};

/// `Name` targets `__stringMin1Max255PatternS` on every monitor template
/// resource (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplateGroup")`), so 255 characters is its
/// published maximum.
const MAX_NAME: usize = 255;

/// One monitor template resource: the collection its create is bound to, which
/// its reader and its delete address an instance under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `/prod/cloudwatch-alarm-template-groups`
    /// (`smithy_query(query="operation",
    /// operation="DeleteCloudWatchAlarmTemplateGroup")`).
    AlarmGroup,
    /// `/prod/eventbridge-rule-template-groups`
    /// (`smithy_query(query="operation",
    /// operation="DeleteEventBridgeRuleTemplateGroup")`).
    RuleGroup,
    /// `/prod/eventbridge-rule-templates` (`smithy_query(query="operation",
    /// operation="DeleteEventBridgeRuleTemplate")`).
    RuleTemplate,
}

impl Kind {
    /// The collection URI this resource's create, reader and delete are bound
    /// to.
    fn collection(self) -> &'static str {
        match self {
            Kind::AlarmGroup => "/prod/cloudwatch-alarm-template-groups",
            Kind::RuleGroup => "/prod/eventbridge-rule-template-groups",
            Kind::RuleTemplate => "/prod/eventbridge-rule-templates",
        }
    }

    /// Every kind this suite covers, so a row can span all of them.
    const ALL: [Kind; 3] = [Kind::AlarmGroup, Kind::RuleGroup, Kind::RuleTemplate];
}

/// The body that creates one resource of `kind` called `name`.
///
/// An eventbridge rule TEMPLATE belongs to a group, so its create carries the
/// `GroupIdentifier` of the group `parent` names; its `EventType` is one of the
/// values `EventBridgeRuleTemplateEventType` declares and its one target is an
/// SNS topic ARN, the kind `EventBridgeRuleTemplateTarget$Arn` documents.
fn create_body(kind: Kind, name: &str, parent: &str) -> Value {
    match kind {
        Kind::AlarmGroup | Kind::RuleGroup => json!({ "name": name }),
        Kind::RuleTemplate => json!({
            "name": name,
            "groupIdentifier": parent,
            "eventType": "MEDIALIVE_CHANNEL_STATE_CHANGE",
            "eventTargets": [{ "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:alerts") }],
        }),
    }
}

/// The status a create of `kind` answers: a group create answers 201, and so
/// does a template create (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplate")`: code 201).
const CREATED: u16 = 201;

/// The status a monitor template delete answers: every one of the three is
/// modelled `code 204`.
const DELETED: u16 = 204;

async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    kind: Kind,
    name: &str,
    parent: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "POST",
        kind.collection(),
        Some(create_body(kind, name, parent)),
    )
    .await
}

async fn get_in(
    router: &MultiServiceRouter,
    region: &str,
    kind: Kind,
    identifier: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GET",
        &format!("{}/{}", kind.collection(), percent_encoded(identifier)),
        None,
    )
    .await
}

async fn delete_in(
    router: &MultiServiceRouter,
    region: &str,
    kind: Kind,
    identifier: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "DELETE",
        &format!("{}/{}", kind.collection(), percent_encoded(identifier)),
        None,
    )
    .await
}

/// Create the group an eventbridge rule template needs, and answer the
/// identifier its create must name. The group kinds need no parent, so they get
/// an identifier nothing reads.
async fn parent_of(router: &MultiServiceRouter, kind: Kind, label: &str) -> String {
    if kind != Kind::RuleTemplate {
        return String::new();
    }
    let name = format!("parent-{label}");
    let (status, group) = create_in(router, REGION, Kind::RuleGroup, &name, "").await;
    assert_eq!(
        status, CREATED,
        "setup {label}: expected the parent group to be created, got {status}: {group}"
    );
    name
}

/// One row of the name table: how the name is shaped, and how to build one of
/// that shape for a given row.
type NameShape = (&'static str, fn(usize) -> String);

/// A name that is plainly within every bound.
fn plain(index: usize) -> String {
    format!("doomed-{index}")
}

/// A name of exactly [`MAX_NAME`] characters: the published maximum of the shape
/// `Name` targets.
fn at_the_name_maximum(index: usize) -> String {
    let head = format!("doomed-{index}-");
    format!("{head}{}", "n".repeat(MAX_NAME - head.chars().count()))
}

/// A name outside ASCII, which travels percent-encoded in the delete's
/// `@httpLabel`.
fn outside_ascii(index: usize) -> String {
    format!("削除対象-{index}")
}

/// The delete's own body must carry nothing: the operation's output has no
/// members, so a 204 answers with an empty payload.
fn assert_no_body(case: &str, body: &Value) {
    assert!(
        body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: expected the delete to answer no body, got {body}"
    );
}

/// Rule: a successful delete of a resource REMOVES it from the service's state,
/// so every later read of it — by either identifier form the model documents —
/// answers the operation's modelled `NotFoundException` with HTTP 404, a second
/// delete of the same identifier answers that same 404, and the name the resource
/// held is free for a new resource to take.
///
/// The rows span what the rule generalises over: the resource TYPE (all three
/// monitor template resources whose delete is implemented), the identifier FORM
/// the delete names the resource by ("Can be either be its id or current name")
/// and the form and size of the name itself, including the published maximum of
/// the shape `Name` targets and a name outside ASCII.
#[tokio::test]
async fn a_deleted_template_resource_stops_being_readable() {
    // (how the name is shaped, how to build one)
    let names: [NameShape; 3] = [
        ("plain name", plain),
        ("name at the 255-character maximum", at_the_name_maximum),
        ("non-ascii name", outside_ascii),
    ];

    let mut row = 0usize;
    for kind in Kind::ALL {
        for by_name in [false, true] {
            for (shape, build) in names {
                row += 1;
                let form = if by_name { "name" } else { "id" };
                let case = format!("{kind:?} deleted by {form}, {shape}");
                let name = build(row);

                let router = medialive();
                let parent = parent_of(&router, kind, &row.to_string()).await;

                let (status, created) = create_in(&router, REGION, kind, &name, &parent).await;
                assert_eq!(
                    status, CREATED,
                    "case {case:?}: expected the create to answer {CREATED}, got {status}: {created}"
                );
                let id = created["id"].as_str().unwrap_or_default().to_string();
                assert!(
                    !id.is_empty(),
                    "case {case:?}: expected the create to report a minted id, got {created}"
                );

                // Both identifier forms read the resource while it exists.
                for identifier in [id.as_str(), name.as_str()] {
                    let (status, read) = get_in(&router, REGION, kind, identifier).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected reading {identifier:?} before the delete to answer 200, got {status}: {read}"
                    );
                }

                // The delete itself, by the form this row names.
                let identifier = if by_name { name.clone() } else { id.clone() };
                let (status, body) = delete_in(&router, REGION, kind, &identifier).await;
                assert_eq!(
                    status, DELETED,
                    "case {case:?}: expected the delete to answer {DELETED}, got {status}: {body}"
                );
                assert_no_body(&case, &body);

                // The resource is GONE: neither identifier form reads it, and
                // the refusal is the modelled 404, not an empty success.
                for missed in [id.as_str(), name.as_str()] {
                    let (status, read) = get_in(&router, REGION, kind, missed).await;
                    assert_eq!(
                        status, 404,
                        "case {case:?}: expected reading {missed:?} after the delete to answer 404, got {status}: {read}"
                    );
                    assert_eq!(
                        error_code(status, &read),
                        "NotFoundException",
                        "case {case:?}: expected NotFoundException for {missed:?}, got {read}"
                    );
                }

                // A second delete has nothing left to remove.
                let (status, again) = delete_in(&router, REGION, kind, &identifier).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the second delete to answer 404, got {status}: {again}"
                );
                assert_eq!(
                    error_code(status, &again),
                    "NotFoundException",
                    "case {case:?}: expected NotFoundException from the second delete, got {again}"
                );

                // The name the deleted resource held is free again, and the
                // resource that takes it is a NEW one.
                let (status, remade) = create_in(&router, REGION, kind, &name, &parent).await;
                assert_eq!(
                    status, CREATED,
                    "case {case:?}: expected the freed name to be reusable, got {status}: {remade}"
                );
                assert_ne!(
                    remade["id"],
                    json!(id),
                    "case {case:?}: expected a freshly minted id, got the deleted resource's {id:?}"
                );
                let (status, read) = get_in(&router, REGION, kind, &name).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the recreated resource to read back, got {status}: {read}"
                );
                assert_eq!(
                    read, remade,
                    "case {case:?}: expected the recreated resource as created, got {read}"
                );
            }
        }
    }
}

/// Rule: a resource is visible, and deletable, only through the region it was
/// created in, so a delete signed for another region misses with the modelled 404
/// and removes nothing.
#[tokio::test]
async fn a_delete_signed_for_another_region_removes_nothing() {
    // The region each row's resource lives in is [`REGION`]; these are the
    // regions the stray delete is signed for. Nothing about them matters except
    // that they differ from it.
    let elsewhere = ["eu-central-1", "ap-south-1"];

    for kind in Kind::ALL {
        for other in elsewhere {
            let case = format!("{kind:?} deleted from {other}");
            let router = medialive();
            let parent = parent_of(&router, kind, other).await;
            let name = format!("regional-{other}");

            let (status, created) = create_in(&router, REGION, kind, &name, &parent).await;
            assert_eq!(
                status, CREATED,
                "case {case:?}: expected the create to answer {CREATED}, got {status}: {created}"
            );
            let id = created["id"].as_str().unwrap_or_default().to_string();

            for identifier in [id.as_str(), name.as_str()] {
                let (status, refused) = delete_in(&router, other, kind, identifier).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected deleting {identifier:?} from {other} to answer 404, got {status}: {refused}"
                );
                assert_eq!(
                    error_code(status, &refused),
                    "NotFoundException",
                    "case {case:?}: expected NotFoundException, got {refused}"
                );
            }

            // The owning region's resource is exactly as it was.
            let (status, read) = get_in(&router, REGION, kind, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the resource to survive, got {status}: {read}"
            );
            assert_eq!(
                read, created,
                "case {case:?}: expected the stray deletes to change nothing, got {read}"
            );
        }
    }
}

/// Rule: a group delete whose documented precondition is unmet is refused with
/// the operation's modelled `ConflictException` (HTTP 409) and removes nothing —
/// the group and everything holding it stay exactly as they were — and the delete
/// succeeds once the caller has met it.
///
/// `DeleteEventBridgeRuleTemplateGroup` documents "You must ... ensure its
/// existing templates are moved to another group or deleted", so the rows differ
/// along the NUMBER of templates the group still holds, up to the member count
/// the store admits.
#[tokio::test]
async fn a_group_delete_is_refused_while_it_still_holds_templates() {
    for held in [1usize, 2, 5] {
        let case = format!("group holding {held} template(s)");
        let router = medialive();

        let group = "crowded-group";
        let (status, created) = create_in(&router, REGION, Kind::RuleGroup, group, "").await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: expected the group to be created, got {status}: {created}"
        );

        let mut templates: Vec<String> = Vec::new();
        for index in 0..held {
            let name = format!("member-{index}");
            let (status, template) =
                create_in(&router, REGION, Kind::RuleTemplate, &name, group).await;
            assert_eq!(
                status, CREATED,
                "case {case:?}: expected member {index} to be created, got {status}: {template}"
            );
            templates.push(template["id"].as_str().unwrap_or_default().to_string());
        }

        let (status, refused) = delete_in(&router, REGION, Kind::RuleGroup, group).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the group delete to be refused, got {status}: {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );

        // Nothing was removed: the group and every member still read back.
        let (status, read) = get_in(&router, REGION, Kind::RuleGroup, group).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the refused delete to leave the group, got {status}: {read}"
        );
        assert_eq!(
            read, created,
            "case {case:?}: expected the group unchanged, got {read}"
        );
        for id in &templates {
            let (status, read) = get_in(&router, REGION, Kind::RuleTemplate, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected member {id} to survive, got {status}: {read}"
            );
        }

        // Meeting the precondition — deleting the members — lets the group go.
        for id in &templates {
            let (status, body) = delete_in(&router, REGION, Kind::RuleTemplate, id).await;
            assert_eq!(
                status, DELETED,
                "case {case:?}: expected member {id} to be deleted, got {status}: {body}"
            );
        }
        let (status, body) = delete_in(&router, REGION, Kind::RuleGroup, group).await;
        assert_eq!(
            status, DELETED,
            "case {case:?}: expected the emptied group to be deleted, got {status}: {body}"
        );
        assert_no_body(&case, &body);

        let (status, read) = get_in(&router, REGION, Kind::RuleGroup, group).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted group to read 404, got {status}: {read}"
        );
    }
}

/// Rule: a group delete whose documented precondition is unmet is refused with
/// the operation's modelled `ConflictException` (HTTP 409) and removes nothing —
/// here the precondition both group deletes document, "You must detach this group
/// from all signal maps".
///
/// The rows are the two group resources that carry it, each attached through its
/// own `CreateSignalMap` member.
#[tokio::test]
async fn a_group_delete_is_refused_while_a_signal_map_names_it() {
    // (the group kind, the CreateSignalMap member that attaches a group of that
    // kind — `smithy_query(query="operation", operation="CreateSignalMap")`)
    let table: [(Kind, &str); 2] = [
        (
            Kind::AlarmGroup,
            "cloudWatchAlarmTemplateGroupIdentifiers",
        ),
        (
            Kind::RuleGroup,
            "eventBridgeRuleTemplateGroupIdentifiers",
        ),
    ];

    for (kind, member) in table {
        let case = format!("{kind:?} named by a signal map through {member}");
        let router = medialive();

        // A signal map's discovery needs an entry point that exists, so the
        // store gets a channel to enter the graph from.
        let (status, channel) = call_in_region(
            &router,
            REGION,
            "POST",
            "/prod/channels",
            Some(json!({ "name": "entry" })),
        )
        .await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: expected the entry-point channel, got {status}: {channel}"
        );
        let entry_point = channel["channel"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let group = "attached-group";
        let (status, created) = create_in(&router, REGION, kind, group, "").await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: expected the group to be created, got {status}: {created}"
        );
        let group_id = created["id"].as_str().unwrap_or_default().to_string();

        let mut body = json!({
            "name": "watcher",
            "discoveryEntryPointArn": entry_point,
        });
        body[member] = json!([group_id]);
        let (status, map) =
            call_in_region(&router, REGION, "POST", "/prod/signal-maps", Some(body)).await;
        assert_eq!(
            status, CREATED,
            "case {case:?}: expected the signal map to be created, got {status}: {map}"
        );

        // (case, the identifier form the delete names the group by)
        for identifier in [group_id.as_str(), group] {
            let (status, refused) = delete_in(&router, REGION, kind, identifier).await;
            assert_eq!(
                status, 409,
                "case {case:?}: expected deleting {identifier:?} to be refused, got {status}: {refused}"
            );
            assert_eq!(
                error_code(status, &refused),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {refused}"
            );
        }

        // Nothing was removed: the group reads back exactly as it was created.
        let (status, read) = get_in(&router, REGION, kind, &group_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the attached group to survive, got {status}: {read}"
        );
        assert_eq!(
            read, created,
            "case {case:?}: expected the group unchanged, got {read}"
        );
    }
}

/// Rule: deleting a resource leaves every other resource in the store readable,
/// so a delete removes the one resource its identifier named and nothing else.
#[tokio::test]
async fn a_delete_removes_only_the_resource_it_named() {
    let router = medialive();

    // Two groups of each kind, and two templates in the first rule group, so
    // every kind has a sibling the delete must not touch.
    let (status, group) = create_in(&router, REGION, Kind::RuleGroup, "holder", "").await;
    assert_eq!(status, CREATED, "setup: the holding group got {status}: {group}");

    // (the kind, the names of the two resources of that kind)
    let plan: [(Kind, [&str; 2]); 3] = [
        (Kind::AlarmGroup, ["alarm-one", "alarm-two"]),
        (Kind::RuleGroup, ["rule-group-one", "rule-group-two"]),
        (Kind::RuleTemplate, ["template-one", "template-two"]),
    ];

    // (kind, name, id) of everything created, so the survivors can be re-read.
    let mut stored: Vec<(Kind, String, String)> = Vec::new();
    for (kind, names) in plan {
        for name in names {
            let (status, created) = create_in(&router, REGION, kind, name, "holder").await;
            assert_eq!(
                status, CREATED,
                "setup {kind:?} {name:?}: expected {CREATED}, got {status}: {created}"
            );
            stored.push((
                kind,
                name.to_string(),
                created["id"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }

    // Delete the FIRST resource of each kind; the second of each kind, and every
    // resource of the other kinds, must survive untouched.
    let mut gone: Vec<(Kind, String)> = Vec::new();
    for (kind, names) in plan {
        let case = format!("{kind:?} {:?} deleted", names[0]);
        let (status, body) = delete_in(&router, REGION, kind, names[0]).await;
        assert_eq!(
            status, DELETED,
            "case {case:?}: expected {DELETED}, got {status}: {body}"
        );
        gone.push((kind, names[0].to_string()));

        for (other_kind, name, id) in &stored {
            let deleted = gone
                .iter()
                .any(|(kind, deleted)| kind == other_kind && deleted == name);
            let (status, read) = get_in(&router, REGION, *other_kind, id).await;
            let expected = if deleted { 404 } else { 200 };
            assert_eq!(
                status, expected,
                "case {case:?}: expected reading {other_kind:?} {name:?} to answer {expected}, got {status}: {read}"
            );
        }
    }
}
