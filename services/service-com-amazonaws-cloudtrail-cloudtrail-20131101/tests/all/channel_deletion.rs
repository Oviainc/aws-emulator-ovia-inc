// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteChannel`.
//!
//! Every case goes over the wire through the shared harness (`tests/all/emulator.rs`):
//! an `awsJson1_1` `POST /` with this service's published `X-Amz-Target` prefix,
//! dispatched through the same `MultiServiceRouter` the production router serves, so
//! what is asserted here is what a client sees — the HTTP status, the error code and the
//! message.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `DeleteChannel` releases exactly the channel its identifier addresses, and the
//!   identifier is either the channel ARN or the bare id CloudTrail minted for it
//!   (`DeleteChannelRequest$Channel`: "The ARN or the UUID value of the channel that you
//!   want to delete");
//! * a channel is a regional resource — its ARN carries a Region and an account — so an
//!   identifier addresses the channel of that id in the ARN's own Region, or, for a bare
//!   id, in the Region the call was made in;
//! * an identifier that names no stored channel is `ChannelNotFoundException` ("thrown
//!   when CloudTrail cannot find the specified channel"), and deleting is not
//!   idempotent: the second delete of one channel is that refusal. A bare identifier is
//!   read as a channel id whatever its syntax, so an id CloudTrail never minted is an
//!   ABSENT channel;
//! * an `arn:`-prefixed identifier that is not a channel ARN can name no channel and is
//!   `ChannelARNInvalidException` ("thrown when the specified value of ChannelARN is not
//!   valid"), and a refused request deletes nothing.
//!
//! `GetChannel` is still a scaffold stub (its own issue,
//! `20261022T230533Z-cloudtrail-getchannel-stub.md`), so a delete is read back through
//! the registry these cases seed — the same store the handler writes — and through the
//! second delete's refusal, which is the same fact over the wire.

use serde_json::json;
use std::collections::BTreeMap;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{Destination, Tag};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::channels::{
    self, ChannelKey, ChannelRecord,
};

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin a channel to the Region that holds it.
const OTHER_REGION: &str = "us-west-2";

/// The status `ChannelNotFoundException` renders with.
///
/// The model puts `@httpError 404` on the shape
/// (`smithy_query(query="shape", shape="ChannelNotFoundException")`), but CloudTrail
/// answers HTTP 400 for it on the wire — the recorded refusal for this operation is
/// `400` with `__type: ChannelNotFoundException` — so the handler states that status
/// through `scaffolded::Scaffolded::modeled_with_status`, the way this service's other
/// not-found codes are stated in `src/ops/describe_query.rs`.
const CHANNEL_NOT_FOUND_STATUS: u16 = 400;

/// The status `ChannelARNInvalidException` renders with: the `@httpError` trait on
/// `com.amazonaws.cloudtrail#ChannelARNInvalidException` (`httpError: 400`), unchanged.
const CHANNEL_ARN_INVALID_STATUS: u16 = 400;

/// What these cases read out of a reply, beside the status, the code and the message
/// the shared harness already exposes: a channel refusal's TEXT is built from the
/// request, which makes the text part of the contract.
impl Reply {
    /// Assert this reply is the modelled failure `code` with HTTP `status` AND the exact
    /// `message`.
    fn expect_error_message(&self, case: &str, status: u16, code: &str, message: &str) {
        assert_eq!(
            (self.status, self.error_code(), self.message()),
            (status, code, message),
            "case {case:?}: expected HTTP {status} {code} {message:?}, got {self:?}"
        );
    }
}

/// The channel calls and the registry reads these cases make, on the shared harness.
impl Emulator {
    /// Call `DeleteChannel` from `region` with `channel` as the identifier.
    async fn delete_channel_in(&self, region: &str, channel: &str) -> Reply {
        self.call_in("DeleteChannel", region, &json!({"Channel": channel}))
            .await
    }

    /// Call `DeleteChannel` from [`HOME_REGION`].
    async fn delete_channel(&self, channel: &str) -> Reply {
        self.delete_channel_in(HOME_REGION, channel).await
    }

    /// Put a channel in `region` into the registry and hand back its ARN — what
    /// `CreateChannel` will do once it is implemented (its own issue,
    /// `20261022T230533Z-cloudtrail-createchannel-stub.md`).
    ///
    /// `destinations` and `tags` are counts, so a row can differ in the SIZE of what the
    /// channel carries without repeating the shape.
    fn seed_channel(
        &self,
        region: &str,
        channel_id: &str,
        name: &str,
        destinations: usize,
        tags: usize,
    ) -> String {
        assert!(
            channels::is_minted_channel_id(channel_id),
            "seed {channel_id:?}: a stored channel carries an id CloudTrail minted"
        );
        let channel_arn = channels::channel_arn(region, channel_id);
        let record = ChannelRecord {
            channel_id: channel_id.to_string(),
            name: name.to_string(),
            channel_arn: channel_arn.clone(),
            // `CreateChannelRequest$Source`: "Custom" is the source of a channel a
            // caller owns.
            source: "Custom".to_string(),
            destinations: (0..destinations)
                .map(|index| Destination {
                    // `DestinationType` as the model enumerates it.
                    r#type: "EVENT_DATA_STORE".to_string(),
                    location: format!(
                        "arn:aws:cloudtrail:{region}:000000000000:eventdatastore/\
                         0000000{index}-0000-4000-8000-000000000000"
                    ),
                })
                .collect(),
            tags: (0..tags)
                .map(|index| Tag {
                    key: format!("key-{index}"),
                    value: Some(format!("value-{index}")),
                })
                .collect(),
            home_region: region.to_string(),
        };
        self.channels_mut(move |channels| {
            channels.insert((channel_id.to_string(), region.to_string()), record);
        });
        channel_arn
    }

    /// Every channel key the registry holds, in registry order.
    fn channel_keys(&self) -> Vec<ChannelKey> {
        let mut keys = Vec::new();
        self.channels_mut(|channels| keys.extend(channels.keys().cloned()));
        keys
    }

    /// Reach the channel registry the handler writes — the SAME store, since the engine
    /// is shared.
    fn channels_mut<R>(
        &self,
        with: impl FnOnce(&mut BTreeMap<ChannelKey, ChannelRecord>) -> R,
    ) -> R {
        let state = self.state();
        let mut store = state.lock();
        with(&mut store.channels)
    }
}

/// A channel id: the `8-4-4-4-12` hex UUID CloudTrail mints, built from `seed` so the
/// rows below differ in the VALUE of the id without repeating its shape.
fn channel_id(seed: &str) -> String {
    format!("{seed}-30fe-4581-8bd3-351632c2be72")
}

/// RULE: `DeleteChannel` releases exactly the channel its identifier addresses — by
/// channel ARN or by bare id, out of the Region the ARN pins or the Region the call was
/// made in — and leaves every other channel in the registry.
///
/// The rows differ in the FORM of the identifier (ARN or bare id), in the Region the
/// channel lives in, in the Region the call is made from, in the id value, and in how
/// many destinations and tags the channel carries.
#[tokio::test]
async fn deleting_a_channel_releases_exactly_that_channel() {
    let emulator = Emulator::new();

    // (case, region the channel lives in, id seed, destinations, tags, address by ARN)
    let rows: Vec<(&str, &str, &str, usize, usize, bool)> = vec![
        ("by ARN, one destination", HOME_REGION, "39032a83", 1, 0, true),
        ("by bare id", HOME_REGION, "4ac9f5c1", 1, 2, false),
        (
            "by ARN in another Region",
            OTHER_REGION,
            "5bd0e6d2",
            3,
            1,
            true,
        ),
        (
            "by bare id, many destinations and tags",
            HOME_REGION,
            "6ce1f7e3",
            5,
            4,
            false,
        ),
        ("by ARN, no tags at all", HOME_REGION, "7df208f4", 2, 0, true),
    ];

    // Seed every row's channel first, so each delete has to pick its own out of a
    // registry that holds the others.
    let mut seeded: Vec<(&str, String, String, &str)> = Vec::new();
    for (case, region, seed, destinations, tags, by_arn) in &rows {
        let id = channel_id(seed);
        let arn =
            emulator.seed_channel(region, &id, &format!("channel-{seed}"), *destinations, *tags);
        let identifier = if *by_arn { arn } else { id.clone() };
        seeded.push((case, id, identifier, region));
    }
    // One more channel sharing an id with a seeded one but living in another Region:
    // deleting in one Region must not touch it.
    let shared_id = channel_id("8ef319a5");
    emulator.seed_channel(HOME_REGION, &shared_id, "shared-home", 1, 0);
    emulator.seed_channel(OTHER_REGION, &shared_id, "shared-other", 1, 0);

    for (case, id, identifier, region) in &seeded {
        let before = emulator.channel_keys();
        // A bare id names a channel in the caller's own Region, so a channel living in
        // another Region is addressed from there.
        let deleted = emulator.delete_channel_in(region, identifier).await;
        deleted.expect_ok(case);

        let after = emulator.channel_keys();
        let key: ChannelKey = ((*id).clone(), (*region).to_string());
        let expected: Vec<ChannelKey> = before.iter().filter(|k| **k != key).cloned().collect();
        assert_eq!(
            after, expected,
            "case {case:?}: deleting {identifier} must release exactly {key:?}, got {after:?}"
        );

        // Deleting is not idempotent: the channel is gone, so the second call is the
        // not-found refusal.
        let again = emulator.delete_channel_in(region, identifier).await;
        again.expect_error_message(
            case,
            CHANNEL_NOT_FOUND_STATUS,
            "ChannelNotFoundException",
            &format!("The channel {identifier} was not found."),
        );
    }

    // The two same-id channels in different Regions are untouched by all of the above.
    assert_eq!(
        emulator.channel_keys(),
        vec![
            (shared_id.clone(), HOME_REGION.to_string()),
            (shared_id.clone(), OTHER_REGION.to_string()),
        ],
        "the channels no row addressed must survive every delete"
    );
}

/// RULE: an identifier that names no stored channel is `ChannelNotFoundException`, and
/// the refused request changes nothing. An id CloudTrail never minted is one of those:
/// a bare identifier is read as a channel id whatever its syntax, because only an ARN
/// has a format to get wrong.
///
/// The rows differ in the form of the identifier, in whether the id has the shape of a
/// minted one at all, in the Region the ARN pins, in the account the ARN names and in
/// the case of the id's hex digits.
#[tokio::test]
async fn an_identifier_naming_no_channel_is_not_found() {
    let emulator = Emulator::new();
    let present = channel_id("39032a83");
    let present_arn = emulator.seed_channel(HOME_REGION, &present, "present-channel", 1, 1);
    let before = emulator.channel_keys();

    let absent = channel_id("ffffffff");
    let rows: Vec<(&str, String)> = vec![
        ("a bare id no channel carries", absent.clone()),
        (
            "an ARN in the calling Region for an absent channel",
            channels::channel_arn(HOME_REGION, &absent),
        ),
        (
            "the ARN of a channel that lives in another Region",
            channels::channel_arn(OTHER_REGION, &present),
        ),
        (
            "an ARN naming another account",
            format!("arn:aws:cloudtrail:{HOME_REGION}:111122223333:channel/{present}"),
        ),
        (
            "the stored id in upper-case hex, which is a different id",
            present.to_uppercase(),
        ),
        (
            "a bare identifier that is not a minted id at all",
            "my-channel".to_string(),
        ),
        (
            "a bare identifier one UUID group short",
            "39032a83-30fe-4581-351632c2be72".to_string(),
        ),
        (
            "a bare identifier with a non-hex digit",
            "39032a8z-30fe-4581-8bd3-351632c2be72".to_string(),
        ),
        (
            "an ARN whose channel id is not a minted one",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:channel/not-a-minted-id"),
        ),
    ];

    for (case, identifier) in &rows {
        let reply = emulator.delete_channel(identifier).await;
        reply.expect_error_message(
            case,
            CHANNEL_NOT_FOUND_STATUS,
            "ChannelNotFoundException",
            &format!("The channel {identifier} was not found."),
        );
        assert_eq!(
            emulator.channel_keys(),
            before,
            "case {case:?}: a refused delete must release nothing, got {:?}",
            emulator.channel_keys()
        );
    }

    // The control: the channel those identifiers failed to name is still deletable by
    // the identifier that does name it.
    emulator
        .delete_channel(&present_arn)
        .await
        .expect_ok("the control");
}

/// The recorded instance this issue was filed on, as ONE row of the rule above: the
/// corpus unit's own channel ARN, called in the Region it was signed for, against a
/// registry that holds no channel.
#[tokio::test]
async fn the_recorded_unknown_channel_is_answered_the_way_the_service_answers_it() {
    let emulator = Emulator::new();
    // `cloudtrail-channel-notfound-error-paths#task#6` and
    // `cloudtrail-lake-read-surface-and-error-contracts#task#9`.
    let recorded =
        "arn:aws:cloudtrail:us-west-2:000000000000:channel/39032a83-30fe-4581-8bd3-351632c2be72";
    emulator
        .delete_channel_in(OTHER_REGION, recorded)
        .await
        .expect_error_message(
            "the recorded unknown-channel ARN",
            CHANNEL_NOT_FOUND_STATUS,
            "ChannelNotFoundException",
            &format!("The channel {recorded} was not found."),
        );
}

/// RULE: an `arn:`-prefixed identifier that is not a channel ARN can name no channel at
/// all and is refused `ChannelARNInvalidException`; nothing is deleted.
///
/// The rows differ in WHICH part of the ARN form is broken: the partition, the service,
/// the field count, the account field, the resource prefix and the resource id. Every
/// row satisfies the modelled `ChannelArn` constraints (len 3..256,
/// `^[a-zA-Z0-9._/\-:]+$`), so the boundary validator passes it to the handler.
#[tokio::test]
async fn an_arn_that_is_not_a_channel_arn_is_refused() {
    let emulator = Emulator::new();
    let present = channel_id("39032a83");
    let present_arn = emulator.seed_channel(HOME_REGION, &present, "present-channel", 2, 1);
    let before = emulator.channel_keys();

    let rows: Vec<(&str, String)> = vec![
        (
            "an ARN in another partition",
            present_arn.replacen("arn:aws:", "arn:aws-us-gov:", 1),
        ),
        (
            "an ARN whose resource is not channel/<id>",
            format!("arn:aws:cloudtrail:{OTHER_REGION}:000000000000:channelwrong"),
        ),
        (
            "an ARN naming another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:dashboard/{present}"),
        ),
        (
            "an ARN naming another service",
            format!("arn:aws:s3:{HOME_REGION}:000000000000:channel/{present}"),
        ),
        (
            "an ARN with an empty account field",
            format!("arn:aws:cloudtrail:{HOME_REGION}::channel/{present}"),
        ),
        (
            "an ARN with one field too few",
            format!("arn:aws:cloudtrail:{HOME_REGION}:channel/{present}"),
        ),
        (
            "an ARN with one field too many",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:extra:channel/{present}"),
        ),
        (
            "an ARN with an empty channel id",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:channel/"),
        ),
    ];

    for (case, identifier) in &rows {
        let reply = emulator.delete_channel(identifier).await;
        reply.expect_error_message(
            case,
            CHANNEL_ARN_INVALID_STATUS,
            "ChannelARNInvalidException",
            &format!("The format of the channel ARN {identifier} is not valid."),
        );
        assert_eq!(
            emulator.channel_keys(),
            before,
            "case {case:?}: a refused delete must release nothing, got {:?}",
            emulator.channel_keys()
        );
    }
}
