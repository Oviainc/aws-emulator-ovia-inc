// SPDX-License-Identifier: CC0-1.0

//! Mutating a resource whose delete this service has already accepted, end to
//! end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! RULE: a delete never drops a MediaLive record — it keeps it readable in the
//! terminal `DELETED` state its own state enum models — but the id it names is
//! dead: an operation that CHANGES that resource's configuration resolves the id
//! to a live resource or answers its modelled `NotFoundException` (HTTP 404) and
//! writes nothing, exactly as it answers an id no resource ever had. The issue's
//! recorded case (`UpdateInput` on a deleted input) is ONE row.
//!
//! The rows differ along what the rule generalises over: the RESOURCE KIND (its
//! own id space, its own delete, with and without a transient `DELETING`
//! window), and, for each kind, the OPERATION doing the mutating — the
//! resource's own update, the schedule a channel owns, and the two tag writers
//! that reach a resource by ARN.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::percent_encoded;
use crate::wire::{call, error_code, error_message, medialive};

/// One request that MUTATES the resource under test, built from its id and its
/// ARN: the method, the URI and the body a client sends.
///
/// Every one of these is a mutation of the SAME resource the row deletes —
/// either by its id (`@httpLabel`) or by its ARN (the tag writers' `ResourceArn`
/// label) — so the rule applies to each of them identically.
struct Mutation {
    /// The operation, as the model names it.
    op: &'static str,
    /// Its request: `(method, uri, body)` from `(id, arn)`.
    request: fn(&str, &str) -> (&'static str, String, Option<Value>),
    /// The status the SAME request gets while the resource is still live, which
    /// is what makes the 404 afterwards about the deletion and nothing else.
    live_status: u16,
}

/// `CreateTags` as a row's mutation.
const CREATE_TAGS: Mutation = Mutation {
    op: "CreateTags",
    request: create_tags,
    live_status: 204,
};

/// `DeleteTags` as a row's mutation.
const DELETE_TAGS: Mutation = Mutation {
    op: "DeleteTags",
    request: delete_tags,
    live_status: 204,
};

/// One resource kind, with the operations that mutate it.
struct Row {
    case: &'static str,
    /// `CreateX`: the collection URI and the body, plus the member the created
    /// resource is wrapped in (`None` for a flat response).
    create: (&'static str, fn() -> Value, Option<&'static str>),
    /// The read that reports the resource, from its id, and the member it wraps
    /// it in. A deleted record stays readable through it.
    read: (fn(&str) -> String, Option<&'static str>),
    /// `DeleteX`: the URI from the id, and the status it answers.
    delete: (fn(&str) -> String, u16),
    /// Whether the delete is only accepted once the resource has settled (a
    /// channel is deletable once it has stopped).
    settle_before_delete: bool,
    /// Whether this resource's state enum gives the delete a transient
    /// `DELETING` step — where it does, the id is already dead for mutation
    /// inside that window.
    transient_delete: bool,
    mutations: &'static [Mutation],
}

/// `CreateTags` on the resource's ARN: a mutation of that resource's tags.
fn create_tags(_id: &str, arn: &str) -> (&'static str, String, Option<Value>) {
    (
        "POST",
        format!("/prod/tags/{}", percent_encoded(arn)),
        Some(json!({ "tags": { "phase": "after" } })),
    )
}

/// `DeleteTags` on the resource's ARN. `TagKeys` is an `@httpQuery` list, so the
/// key travels as a `tagKeys` parameter.
fn delete_tags(_id: &str, arn: &str) -> (&'static str, String, Option<Value>) {
    (
        "DELETE",
        format!(
            "/prod/tags/{}?tagKeys=team",
            percent_encoded(arn)
        ),
        None,
    )
}

/// Rule: every operation that changes a MediaLive resource answers its modelled
/// `NotFoundException` (404) once a delete has been accepted for that id, and
/// writes nothing — while the deleted record itself stays readable in its
/// terminal state.
#[tokio::test]
async fn a_mutation_on_a_resource_whose_delete_was_accepted_is_not_found() {
    let table: [Row; 4] = [
        Row {
            case: "input",
            create: (
                "/prod/inputs",
                || {
                    json!({
                        "name": "doomed",
                        "type": "URL_PULL",
                        "sources": [{ "url": "https://example.com/a.m3u8" }],
                        "tags": { "team": "video" },
                    })
                },
                Some("input"),
            ),
            read: (|id| format!("/prod/inputs/{id}"), None),
            delete: (|id| format!("/prod/inputs/{id}"), 200),
            settle_before_delete: false,
            transient_delete: true,
            mutations: &[
                Mutation {
                    op: "UpdateInput",
                    request: |id, _arn| {
                        (
                            "PUT",
                            format!("/prod/inputs/{id}"),
                            Some(json!({ "name": "renamed-after-delete" })),
                        )
                    },
                    live_status: 200,
                },
                CREATE_TAGS,
                DELETE_TAGS,
            ],
        },
        Row {
            case: "channel",
            create: (
                "/prod/channels",
                || json!({ "name": "doomed-channel", "tags": { "team": "video" } }),
                Some("channel"),
            ),
            read: (|id| format!("/prod/channels/{id}"), None),
            delete: (|id| format!("/prod/channels/{id}"), 200),
            // "a channel is deletable once it has stopped": the create's
            // transient CREATING has to settle into IDLE first.
            settle_before_delete: true,
            transient_delete: true,
            mutations: &[
                Mutation {
                    op: "BatchUpdateSchedule",
                    request: |id, _arn| {
                        (
                            "PUT",
                            format!("/prod/channels/{id}/schedule"),
                            Some(json!({
                                "creates": {
                                    "scheduleActions": [{
                                        "actionName": "after-delete",
                                        "scheduleActionSettings": {
                                            "timedMetadataSettings": { "id3": "QUJD" },
                                        },
                                        "scheduleActionStartSettings": {
                                            "immediateModeScheduleActionStartSettings": {},
                                        },
                                    }],
                                },
                            })),
                        )
                    },
                    live_status: 200,
                },
                Mutation {
                    op: "DeleteSchedule",
                    request: |id, _arn| {
                        ("DELETE", format!("/prod/channels/{id}/schedule"), None)
                    },
                    live_status: 200,
                },
                CREATE_TAGS,
                DELETE_TAGS,
            ],
        },
        Row {
            case: "input security group",
            create: (
                "/prod/inputSecurityGroups",
                || {
                    json!({
                        "whitelistRules": [{ "cidr": "10.0.0.0/16" }],
                        "tags": { "team": "video" },
                    })
                },
                Some("securityGroup"),
            ),
            read: (|id| format!("/prod/inputSecurityGroups/{id}"), None),
            delete: (|id| format!("/prod/inputSecurityGroups/{id}"), 200),
            settle_before_delete: false,
            // `InputSecurityGroupState` models no DELETING step, so the delete
            // is done with the call.
            transient_delete: false,
            mutations: &[
                Mutation {
                    op: "UpdateInputSecurityGroup",
                    request: |id, _arn| {
                        (
                            "PUT",
                            format!("/prod/inputSecurityGroups/{id}"),
                            Some(json!({ "whitelistRules": [{ "cidr": "192.0.2.0/24" }] })),
                        )
                    },
                    live_status: 200,
                },
                CREATE_TAGS,
                DELETE_TAGS,
            ],
        },
        Row {
            case: "sdi source",
            create: (
                "/prod/sdiSources",
                || json!({ "name": "doomed-sdi", "type": "SINGLE", "tags": { "team": "video" } }),
                Some("sdiSource"),
            ),
            read: (|id| format!("/prod/sdiSources/{id}"), Some("sdiSource")),
            // The status this delete answers: 200, not the model's 202 — the
            // delete is done with the call (see `DeleteSdiSource`'s
            // `SUCCESS_STATUS` and the note above it).
            delete: (|id| format!("/prod/sdiSources/{id}"), 200),
            settle_before_delete: false,
            transient_delete: false,
            mutations: &[
                Mutation {
                    op: "UpdateSdiSource",
                    request: |id, _arn| {
                        (
                            "PUT",
                            format!("/prod/sdiSources/{id}"),
                            Some(json!({ "name": "renamed-after-delete" })),
                        )
                    },
                    live_status: 200,
                },
                CREATE_TAGS,
                DELETE_TAGS,
            ],
        },
    ];

    for row in table {
        let router = medialive();
        let (case, mutations) = (row.case, row.mutations);

        let (collection, body, member) = row.create;
        let (status, created) = call(&router, "POST", collection, Some(body())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: create: expected 201, got {status}: {created}"
        );
        let resource = member.map_or_else(|| created.clone(), |name| created[name].clone());
        let id = resource["id"].as_str().unwrap_or_default().to_string();
        let arn = resource["arn"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && !arn.is_empty(),
            "case {case:?}: expected the create to report an id and an ARN, got {created}"
        );

        // The control: every one of these requests is one the LIVE resource
        // accepts, so the refusals below are about the deletion and not about a
        // request the service would have refused anyway.
        for mutation in mutations {
            let (method, uri, body) = (mutation.request)(&id, &arn);
            let (status, answer) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, mutation.live_status,
                "case {case:?}/{}: expected {} while the resource is live, got {status}: {answer}",
                mutation.op, mutation.live_status
            );
        }

        // What the resource looks like one instant before the delete, read back
        // out of the store through its own reader and its tag list.
        let (read_uri, read_member) = row.read;
        let before = read(&router, case, &read_uri(&id), read_member).await;
        let tags_before = listed_tags(&router, case, &arn).await;

        if row.settle_before_delete {
            wait_for_the_clock();
        }

        let (delete_uri, delete_status) = row.delete;
        let (status, deleted) = call(&router, "DELETE", &delete_uri(&id), None).await;
        assert_eq!(
            status, delete_status,
            "case {case:?}: delete: expected {delete_status}, got {status}: {deleted}"
        );

        // Inside the transient DELETING window the delete has been ACCEPTED, so
        // the id is already dead for mutation.
        if row.transient_delete {
            for mutation in mutations {
                refused(&router, case, "DELETING", mutation, &id, &arn).await;
            }
            wait_for_the_clock();
        }

        // ... and in the terminal state it keeps.
        for mutation in mutations {
            refused(&router, case, "DELETED", mutation, &id, &arn).await;
        }

        // The record is still readable, in the terminal DELETED state, and every
        // refusal above wrote nothing: every member but `State` reads back
        // exactly as it did before the delete, tags included.
        let after = read(&router, case, &read_uri(&id), read_member).await;
        assert_eq!(
            after["state"],
            json!("DELETED"),
            "case {case:?}: expected the deleted record to report the terminal DELETED, got {:?}",
            after["state"]
        );
        for (name, value) in before.as_object().expect("the read body is an object") {
            if name == "state" {
                continue;
            }
            assert_eq!(
                &after[name], value,
                "case {case:?}: expected the refused mutations to leave {name} at {value:?}, got {:?}",
                after[name]
            );
        }
        assert_eq!(
            listed_tags(&router, case, &arn).await,
            tags_before,
            "case {case:?}: expected the refused tag writes to leave the tags alone"
        );
    }
}

/// Rule: a `DeleteSchedule` or `BatchUpdateSchedule` refused on a deleted
/// channel leaves that channel's schedule exactly as it was — the refusal is
/// taken before anything is written, so the actions the channel held are still
/// the actions `DescribeSchedule` reports.
#[tokio::test]
async fn a_refused_schedule_mutation_leaves_the_schedule_it_named_alone() {
    let router = medialive();
    let (status, created) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "scheduled-then-deleted" })),
    )
    .await;
    assert_eq!(status, 201, "CreateChannel: got {status}: {created}");
    let id = created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Two actions, so the schedule the refusals must not touch is not empty.
    let (status, applied) = call(
        &router,
        "PUT",
        &format!("/prod/channels/{id}/schedule"),
        Some(json!({
            "creates": {
                "scheduleActions": [
                    {
                        "actionName": "kept-fixed",
                        "scheduleActionSettings": {
                            "scte35ReturnToNetworkSettings": { "spliceEventId": 7 },
                        },
                        "scheduleActionStartSettings": {
                            "fixedModeScheduleActionStartSettings": {
                                "time": "2030-01-01T00:00:00Z",
                            },
                        },
                    },
                    {
                        "actionName": "kept-immediate",
                        "scheduleActionSettings": {
                            "timedMetadataSettings": { "id3": "QUJD" },
                        },
                        "scheduleActionStartSettings": {
                            "immediateModeScheduleActionStartSettings": {},
                        },
                    },
                ],
            },
        })),
    )
    .await;
    assert_eq!(status, 200, "BatchUpdateSchedule: got {status}: {applied}");

    let schedule_before = reported_actions(&router, "live", &id).await;
    assert_eq!(
        schedule_before.len(),
        2,
        "expected the two created actions in the schedule, got {schedule_before:?}"
    );

    wait_for_the_clock();
    let (status, deleting) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(status, 200, "DeleteChannel: got {status}: {deleting}");
    wait_for_the_clock();

    // (case, method, body) — the two schedule mutations, each refused.
    let table: [(&str, &str, Option<Value>); 2] = [
        (
            "BatchUpdateSchedule adding an action",
            "PUT",
            Some(json!({
                "creates": {
                    "scheduleActions": [{
                        "actionName": "must-not-be-stored",
                        "scheduleActionSettings": {
                            "timedMetadataSettings": { "id3": "QUJD" },
                        },
                        "scheduleActionStartSettings": {
                            "immediateModeScheduleActionStartSettings": {},
                        },
                    }],
                },
            })),
        ),
        (
            "BatchUpdateSchedule deleting an action",
            "PUT",
            Some(json!({ "deletes": { "actionNames": ["kept-immediate"] } })),
        ),
    ];

    for (case, method, body) in table {
        let (status, refusal) = call(
            &router,
            method,
            &format!("/prod/channels/{id}/schedule"),
            body,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 on a deleted channel, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {refusal}"
        );
    }

    let (status, refusal) = call(
        &router,
        "DELETE",
        &format!("/prod/channels/{id}/schedule"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"DeleteSchedule\": expected 404 on a deleted channel, got {status}: {refusal}"
    );
    assert_eq!(
        error_code(status, &refusal),
        "NotFoundException",
        "case \"DeleteSchedule\": expected NotFoundException, got {refusal}"
    );

    assert_eq!(
        reported_actions(&router, "deleted", &id).await,
        schedule_before,
        "expected the refused schedule mutations to have written nothing"
    );
}

/// Rule: the refusal's message is composed from the REQUEST — the member the id
/// arrived in, as the wire names it (`inputId`), the id itself, then `already
/// deleted` — which is the form the real service's recorded refusal carries, so
/// a client that reads the message learns which id it named.
///
/// The rows differ along the input SHAPE and the member the refused update
/// names, because the message is about the id the request carried and not about
/// what the update tried to change.
#[tokio::test]
async fn a_deleted_inputs_refusal_names_the_input_id_it_was_given() {
    // (case, the CreateInput body, the refused UpdateInput body)
    let table: [(&str, Value, Value); 3] = [
        (
            "url pull, renamed",
            json!({
                "name": "pull",
                "type": "URL_PULL",
                "sources": [{ "url": "https://example.com/a.m3u8" }],
            }),
            json!({ "name": "dead-rename" }),
        ),
        (
            "rtmp push, destinations replaced",
            json!({
                "name": "push",
                "type": "RTMP_PUSH",
                "destinations": [{ "streamName": "live/a" }],
            }),
            json!({ "destinations": [{ "streamName": "live/b" }] }),
        ),
        (
            "sdi input, no member named at all",
            json!({
                "name": "sdi",
                "type": "SDI",
                "sdiSources": ["sdi-a"],
                "inputNetworkLocation": "ON_PREMISES",
            }),
            json!({}),
        ),
    ];

    for (case, create_body, update_body) in table {
        let router = medialive();
        let (status, created) = call(&router, "POST", "/prod/inputs", Some(create_body)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateInput: expected 201, got {status}: {created}"
        );
        let id = created["input"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, deleting) = call(&router, "DELETE", &format!("/prod/inputs/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteInput: expected 200, got {status}: {deleting}"
        );
        wait_for_the_clock();

        let (status, refusal) = call(
            &router,
            "PUT",
            &format!("/prod/inputs/{id}"),
            Some(update_body),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 on the deleted input, got {status}: {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            format!("inputId={id} already deleted"),
            "case {case:?}: expected the message composed from the request, got {:?}",
            error_message(&refusal)
        );
    }
}

/// One mutation, refused: the operation's modelled `NotFoundException` with HTTP
/// 404, naming the identifier the request used.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    phase: &str,
    mutation: &Mutation,
    id: &str,
    arn: &str,
) {
    let (method, uri, body) = (mutation.request)(id, arn);
    let (status, refusal) = call(router, method, &uri, body).await;
    assert_eq!(
        status, 404,
        "case {case:?}/{}: expected 404 while the resource is {phase}, got {status}: {refusal}",
        mutation.op
    );
    assert_eq!(
        error_code(status, &refusal),
        "NotFoundException",
        "case {case:?}/{}: expected the modelled NotFoundException while the resource is {phase}, got {refusal}",
        mutation.op
    );
    let message = error_message(&refusal);
    assert!(
        message.contains(id) || message.contains(arn),
        "case {case:?}/{}: expected the refusal to name the identifier it refused, got {message:?}",
        mutation.op
    );
}

/// One resource, read back through its own reader, with the read asserted to
/// have succeeded: a deleted record stays readable.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    member: Option<&str>,
) -> Value {
    let (status, body) = call(router, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GET {uri}: expected 200, got {status}: {body}"
    );
    member.map_or(body.clone(), |name| body[name].clone())
}

/// The tags `ListTagsForResource` reports for `arn` — the READ path, which keeps
/// resolving a deleted resource.
async fn listed_tags(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/prod/tags/{}", percent_encoded(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource({arn}): expected 200, got {status}: {body}"
    );
    body["tags"].clone()
}

/// The action names `DescribeSchedule` reports for a channel, in order.
async fn reported_actions(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<String> {
    let (status, body) = call(
        router,
        "GET",
        &format!("/prod/channels/{id}/schedule"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSchedule({id}): expected 200, got {status}: {body}"
    );
    body["scheduleActions"]
        .as_array()
        .map(|actions| {
            actions
                .iter()
                .map(|action| action["actionName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}
