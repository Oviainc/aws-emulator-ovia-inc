// SPDX-License-Identifier: CC0-1.0

//! The `ChannelSecurityGroups` reference a channel write stores, end to end over
//! the restJson1 wire.
//!
//! Rule: every id a channel configuration names has to resolve to a resource this
//! account holds, so an operation that writes a channel's configuration
//! (`CreateChannel`, `UpdateChannel`) refuses one whose `ChannelSecurityGroups`
//! entry names no input security group this account holds — the same way it
//! already refuses an `InputAttachments` entry naming no input of this account:
//! the modelled `UnprocessableEntityException` (422), one `ValidationErrors`
//! entry per unresolvable entry, naming the element path it was found at, and
//! nothing written.
//!
//! `ChannelSecurityGroups` is "A list of IDs for all the Input Security Groups
//! attached to the channel" (`smithy_query(query="doc",
//! shape="com.amazonaws.medialive#CreateChannelRequest",
//! member="ChannelSecurityGroups")`), and the group side derives
//! `InputSecurityGroup$Channels` ("The list of channels currently using this
//! Input Security Group as their channel security group") from it — so an
//! accepted write is checked from BOTH sides here: the channel reports the groups
//! it named, and each named group reports that channel.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the one entry naming an unminted id is ONE row, and the
//! other rows are what the rule generalises over — what makes a reference
//! unresolvable (never minted, minted and then deleted, named at all), where in
//! the list the unresolvable entry sits, how many problems one request carries,
//! which member carries them, how many entries an accepted list has, and whether
//! the request asked to be performed or only checked.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    missing_reference_id_message, unknown_reference_message, CHANNEL_CONTAINS_ERRORS_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::support::{attachable_input, channel_encoder_settings, input_security_group};
use crate::wire::{call, error_code, error_message, medialive};

/// An id of the shape this service mints for an input security group
/// (`state::FIRST_INPUT_SECURITY_GROUP_ID` and up), that no
/// `CreateInputSecurityGroup` ever minted in the account under test.
const UNMINTED_GROUP_ID: &str = "3999999";

/// A second such id, for the row carrying two unresolvable entries at once.
const OTHER_UNMINTED_GROUP_ID: &str = "3999998";

/// An id of the shape this service mints for an input, that no `CreateInput`
/// ever minted in the account under test.
const UNMINTED_INPUT_ID: &str = "9999999";

/// One problem a refusal reports: the element path it was found at, and the
/// message that says what it is.
type Problem = (String, String);

/// `CreateChannel` with `body`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/prod/channels", Some(body)).await
}

/// `UpdateChannel` on `id` with `body`.
async fn update(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("/prod/channels/{id}"), Some(body)).await
}

/// `DescribeChannel` on `id`.
async fn describe(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/prod/channels/{id}"), None).await
}

/// `DescribeInputSecurityGroup` on `id`.
async fn describe_group(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/prod/inputSecurityGroups/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInputSecurityGroup({id}): got {status}: {body}"
    );
    body
}

/// One input security group this account holds, deleted again — a reference that
/// resolved once and does not any more.
async fn deleted_group(router: &MultiServiceRouter) -> String {
    let id = input_security_group(router).await;
    let (status, body) = call(
        router,
        "DELETE",
        &format!("/prod/inputSecurityGroups/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: DeleteInputSecurityGroup({id}): got {status}: {body}"
    );
    id
}

/// How many channels this account holds, as `ListChannels` reports them.
async fn channel_count(router: &MultiServiceRouter) -> usize {
    let (status, body) = call(router, "GET", "/prod/channels", None).await;
    assert_eq!(status, 200, "ListChannels: got {status}: {body}");
    body["channels"].as_array().map_or(0, Vec::len)
}

/// The `(elementPath, errorMessage)` pairs a refusal reported, in the order it
/// reported them.
fn reported_problems(body: &Value) -> Vec<Problem> {
    body["validationErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|problem| {
            (
                problem["elementPath"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                problem["errorMessage"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// Assert `body` is the modelled 422 refusal carrying exactly `expected`, one
/// entry per problem, in that order.
fn assert_unprocessable(case: &str, status: u16, body: &Value, expected: &[Problem]) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected status 422, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body}"
    );
    assert_eq!(
        error_message(body),
        CHANNEL_CONTAINS_ERRORS_MESSAGE,
        "case {case:?}: expected the message {CHANNEL_CONTAINS_ERRORS_MESSAGE:?}, got {:?}",
        error_message(body)
    );
    assert_eq!(
        reported_problems(body),
        expected,
        "case {case:?}: expected the problems {expected:?}, got {:?}",
        body["validationErrors"]
    );
}

/// The problem a `channelSecurityGroups` entry at `index` naming the
/// unresolvable id `id` carries.
fn unresolvable_group(index: usize, id: &str) -> Problem {
    (
        format!("channelSecurityGroups/{index}"),
        unknown_reference_message("channelSecurityGroups", "input security group", id),
    )
}

/// The problem a `channelSecurityGroups` entry at `index` naming no group at all
/// carries.
fn nameless_group(index: usize) -> Problem {
    (
        format!("channelSecurityGroups/{index}"),
        missing_reference_id_message("channelSecurityGroups"),
    )
}

/// The problem an `inputAttachments` entry at `index` naming the unresolvable id
/// `id` carries — the reference member this rule already applied to, here to pin
/// that one request carrying both reports both.
fn unresolvable_attachment(index: usize, id: &str) -> Problem {
    (
        format!("inputAttachments/{index}/inputId"),
        unknown_reference_message("inputAttachments", "input", id),
    )
}

/// Rule: `CreateChannel` refuses a configuration whose `ChannelSecurityGroups`
/// names a group this account does not hold, and creates NO channel.
#[tokio::test]
async fn a_create_refuses_a_channel_security_group_it_cannot_resolve() {
    let router = medialive();
    let held = input_security_group(&router).await;
    let gone = deleted_group(&router).await;
    let attachable = attachable_input(&router, "attachable").await;

    // (case, the request body, the problems the refusal must report, in order)
    let table: Vec<(&str, Value, Vec<Problem>)> = vec![
        (
            "an id nothing minted",
            json!({
                "name": "unminted-group",
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [UNMINTED_GROUP_ID],
            }),
            vec![unresolvable_group(0, UNMINTED_GROUP_ID)],
        ),
        (
            "a group this account held and deleted",
            json!({
                "name": "deleted-group",
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [gone.clone()],
            }),
            vec![unresolvable_group(0, &gone)],
        ),
        (
            "an entry naming no group at all",
            json!({
                "name": "nameless-group",
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [""],
            }),
            vec![nameless_group(0)],
        ),
        (
            "an unresolvable entry after a held one",
            json!({
                "name": "second-entry",
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [held.clone(), UNMINTED_GROUP_ID],
            }),
            vec![unresolvable_group(1, UNMINTED_GROUP_ID)],
        ),
        (
            "two unresolvable entries",
            json!({
                "name": "two-ghosts",
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [UNMINTED_GROUP_ID, OTHER_UNMINTED_GROUP_ID],
            }),
            vec![
                unresolvable_group(0, UNMINTED_GROUP_ID),
                unresolvable_group(1, OTHER_UNMINTED_GROUP_ID),
            ],
        ),
        (
            // Both reference members of one configuration, so the refusal
            // reports every problem the configuration carries rather than the
            // first one it meets.
            "an unresolvable attachment and an unresolvable group",
            json!({
                "name": "both-members",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [
                    { "inputAttachmentName": "ghost", "inputId": UNMINTED_INPUT_ID },
                ],
                "channelSecurityGroups": [UNMINTED_GROUP_ID],
            }),
            vec![
                unresolvable_attachment(0, UNMINTED_INPUT_ID),
                unresolvable_group(0, UNMINTED_GROUP_ID),
            ],
        ),
        (
            // `DryRun` asks for the request to be CHECKED, so it earns the same
            // refusal as the request that asked to be performed.
            "a checked-only create",
            json!({
                "name": "checked-only",
                "dryRun": true,
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [UNMINTED_GROUP_ID],
            }),
            vec![unresolvable_group(0, UNMINTED_GROUP_ID)],
        ),
        (
            // The resolvable entry of a refused request is not written either:
            // the whole configuration is refused, so the held group goes on
            // reporting no channel (asserted after the table).
            "a held entry beside an unresolvable one, with an attachment",
            json!({
                "name": "mixed",
                "encoderSettings": channel_encoder_settings(),
                "inputAttachments": [
                    { "inputAttachmentName": "held", "inputId": attachable.clone() },
                ],
                "channelSecurityGroups": [held.clone(), ""],
            }),
            vec![nameless_group(1)],
        ),
    ];

    for (case, body, expected) in table {
        let before = channel_count(&router).await;
        let (status, refused) = create(&router, body).await;
        assert_unprocessable(case, status, &refused, &expected);
        assert_eq!(
            refused.get("channel"),
            None,
            "case {case:?}: expected no channel in the refusal, got {refused}"
        );
        assert_eq!(
            channel_count(&router).await,
            before,
            "case {case:?}: expected the refused create to store nothing, got {} channels",
            channel_count(&router).await
        );
    }

    // The group side agrees: no refused create left a channel claiming the held
    // group.
    let group = describe_group(&router, &held).await;
    assert_eq!(
        group["channels"],
        json!([]),
        "expected no channel to be using the held group, got {:?}",
        group["channels"]
    );
}

/// Rule (the other side of the same rule): a `ChannelSecurityGroups` list whose
/// every entry resolves is stored, reads back through `DescribeChannel`, and each
/// named group reports the channel in `InputSecurityGroup$Channels`.
///
/// The rows differ along how many entries the list has and how many distinct
/// groups they name, which is what the rule generalises over.
#[tokio::test]
async fn a_create_accepts_the_channel_security_groups_it_can_resolve() {
    // (case, how many entries, which of the held groups each entry names)
    let table: [(&str, &[usize]); 5] = [
        ("no group at all", &[]),
        ("one group", &[0]),
        ("two groups", &[0, 1]),
        ("the same group twice", &[0, 0]),
        ("three groups", &[0, 1, 2]),
    ];

    for (case, slots) in table {
        let router = medialive();
        let mut held = Vec::new();
        for _ in 0..3 {
            held.push(input_security_group(&router).await);
        }
        let named: Vec<String> = slots.iter().map(|slot| held[*slot].clone()).collect();

        let (status, created) = create(
            &router,
            json!({
                "name": case,
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": named.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created}"
        );
        let id = created["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, read) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel: got {status}: {read}"
        );
        assert_eq!(
            read["channelSecurityGroups"],
            json!(named),
            "case {case:?}: expected the declared groups {named:?}, got {:?}",
            read["channelSecurityGroups"]
        );

        // The mirror derivation: a group the channel named reports that channel,
        // and one it did not name reports none.
        for (slot, group_id) in held.iter().enumerate() {
            let group = describe_group(&router, group_id).await;
            let expected = if slots.contains(&slot) {
                json!([id.clone()])
            } else {
                json!([])
            };
            assert_eq!(
                group["channels"], expected,
                "case {case:?}: expected group {group_id} to report {expected:?}, got {:?}",
                group["channels"]
            );
        }
    }
}

/// Rule: `UpdateChannel` applies the same resolution to the configuration the
/// update WOULD leave on the channel — an update may no more leave a channel
/// naming a group this account does not hold than a create may create one — and
/// a refused update leaves the stored channel exactly as it was.
///
/// The rows differ along what makes the named group unresolvable (never minted,
/// minted and then deleted, named at all), where in the list it sits, and whether
/// the update asked to be performed or only checked.
#[tokio::test]
async fn an_update_refuses_a_channel_security_group_it_cannot_resolve() {
    let router = medialive();
    let held = input_security_group(&router).await;
    let gone = deleted_group(&router).await;

    // (case, the patch, the problems the refusal must report, in order)
    let table: Vec<(&str, Value, Vec<Problem>)> = vec![
        (
            "an id nothing minted",
            json!({ "channelSecurityGroups": [UNMINTED_GROUP_ID] }),
            vec![unresolvable_group(0, UNMINTED_GROUP_ID)],
        ),
        (
            "a group this account held and deleted",
            json!({ "channelSecurityGroups": [gone.clone()] }),
            vec![unresolvable_group(0, &gone)],
        ),
        (
            "an entry naming no group at all",
            json!({ "channelSecurityGroups": [""] }),
            vec![nameless_group(0)],
        ),
        (
            "an unresolvable entry after the held one",
            json!({ "channelSecurityGroups": [held.clone(), UNMINTED_GROUP_ID] }),
            vec![unresolvable_group(1, UNMINTED_GROUP_ID)],
        ),
        (
            "a checked-only update",
            json!({ "dryRun": true, "channelSecurityGroups": [UNMINTED_GROUP_ID] }),
            vec![unresolvable_group(0, UNMINTED_GROUP_ID)],
        ),
        (
            "a renamed channel whose groups do not resolve",
            json!({ "name": "renamed", "channelSecurityGroups": [UNMINTED_GROUP_ID] }),
            vec![unresolvable_group(0, UNMINTED_GROUP_ID)],
        ),
    ];

    for (case, patch, expected) in table {
        let (status, created) = create(
            &router,
            json!({
                "name": "baseline",
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [held.clone()],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: seeding the channel: got {status}: {created}"
        );
        let id = created["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // An update is accepted from a channel that has stopped, which is the
        // `IDLE` the create's transient state settles into.
        wait_for_the_clock();

        let (status, refused) = update(&router, &id, patch).await;
        assert_unprocessable(case, status, &refused, &expected);

        let (status, read) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel: got {status}: {read}"
        );
        assert_eq!(
            read["channelSecurityGroups"],
            json!([held.clone()]),
            "case {case:?}: expected the refused update to leave the groups alone, got {:?}",
            read["channelSecurityGroups"]
        );
        assert_eq!(
            read["name"],
            json!("baseline"),
            "case {case:?}: expected the refused update to have written nothing, got {:?}",
            read["name"]
        );
    }
}

/// Rule (the other side of the update): an update whose `ChannelSecurityGroups`
/// all resolve is applied and reads back — and an update that names no group at
/// all keeps the ones the channel had, which is what makes the check one on the
/// configuration the update WOULD leave rather than on the request alone.
#[tokio::test]
async fn an_update_accepts_the_channel_security_groups_it_can_resolve() {
    let router = medialive();
    let first = input_security_group(&router).await;
    let second = input_security_group(&router).await;

    // (case, the patch, the groups the channel must report afterwards)
    let table: Vec<(&str, Value, Value)> = vec![
        (
            "no group member at all",
            json!({ "name": "renamed" }),
            json!([first.clone()]),
        ),
        (
            "another held group replaces it",
            json!({ "channelSecurityGroups": [second.clone()] }),
            json!([second.clone()]),
        ),
        (
            "both held groups",
            json!({ "channelSecurityGroups": [first.clone(), second.clone()] }),
            json!([first.clone(), second.clone()]),
        ),
        (
            "an empty list detaches every group",
            json!({ "channelSecurityGroups": [] }),
            json!([]),
        ),
    ];

    for (case, patch, expected) in table {
        let (status, created) = create(
            &router,
            json!({
                "name": "baseline",
                "encoderSettings": channel_encoder_settings(),
                "channelSecurityGroups": [first.clone()],
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: seeding the channel: got {status}: {created}"
        );
        let id = created["channel"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        wait_for_the_clock();

        let (status, updated) = update(&router, &id, patch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {updated}"
        );

        let (status, read) = describe(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel: got {status}: {read}"
        );
        assert_eq!(
            read["channelSecurityGroups"], expected,
            "case {case:?}: expected the groups {expected:?}, got {:?}",
            read["channelSecurityGroups"]
        );

        // The group side agrees with what the update stored: a group the updated
        // channel names reports it, one it no longer names does not.
        for group_id in [&first, &second] {
            let group = describe_group(&router, group_id).await;
            let names_it = expected
                .as_array()
                .is_some_and(|groups| groups.iter().any(|group| group == &json!(group_id)));
            let reported = group["channels"]
                .as_array()
                .is_some_and(|channels| channels.iter().any(|channel| channel == &json!(id)));
            assert_eq!(
                reported, names_it,
                "case {case:?}: expected group {group_id} to report the channel: {names_it}, got {:?}",
                group["channels"]
            );
        }

        // Each row starts from its own channel, so the group derivations above
        // are about this row's channel alone. A channel is deleted once it has
        // stopped, which is the `IDLE` the update's transient `UPDATING` settles
        // back into.
        wait_for_the_clock();
        let (status, deleted) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteChannel: got {status}: {deleted}"
        );
        wait_for_the_clock();
    }
}
