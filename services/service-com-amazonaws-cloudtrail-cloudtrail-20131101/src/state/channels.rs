// SPDX-License-Identifier: CC0-1.0

//! The CloudTrail Lake **channel** registry and the rules every channel operation
//! shares.
//!
//! A channel identifier is a channel ARN or the bare id CloudTrail minted for it
//! (`DeleteChannelRequest$Channel` / `GetChannelRequest$Channel`: "The ARN or the UUID
//! value of the channel"), and the ARN carries the account that owns the channel and the
//! Region it lives in. Reading those two forms, and the wording a channel operation
//! refuses a malformed identifier and an absent channel with, live here — beside the
//! registry they resolve against — so `CreateChannel`, `GetChannel`, `UpdateChannel`,
//! `ListChannels` and `DeleteChannel` cannot drift from one another.
//!
//! The registry itself is a field of [`super::ComAmazonawsCloudtrailCloudtrail20131101State`],
//! the one store this service keeps; this module is where its rules are written, the way
//! `src/service/rpc_target.rs` holds the target reading `src/service.rs` applies.

use super::{ComAmazonawsCloudtrailCloudtrail20131101State, SERVICE_NAMESPACE};
use crate::ops::types::{Destination, Tag};

/// The resource prefix of a channel ARN:
/// `arn:aws:cloudtrail:<region>:<account>:channel/<uuid>`. The format is the one the
/// model doc for `CloudTrailARNInvalidException` publishes: "The following is the
/// format of a channel ARN: arn:aws:cloudtrail:us-east-2:123456789012:channel/01234567890".
const CHANNEL_RESOURCE_PREFIX: &str = "channel/";

/// The resource TYPE of that ARN — [`CHANNEL_RESOURCE_PREFIX`] without its separator —
/// which is what `::service_common::arn` reads the resource field by.
const CHANNEL_RESOURCE_TYPE: &str = "channel";

/// The partition every ARN this emulator mints carries: `::engine::emulator_arn` builds
/// `arn:aws:<service>:<region>:<account>:<resource>`, so an ARN built in another
/// partition names a resource no call here can reference.
const AWS_PARTITION: &str = "aws";

/// The group lengths of the UUID CloudTrail mints a channel id as (`8-4-4-4-12` hex
/// digits), which is the `<uuid>` of `DeleteChannelRequest$Channel` ("The ARN or the
/// UUID value of the channel").
///
/// It is what a MINTED id looks like, not a rule a caller-supplied id is checked
/// against: an id CloudTrail never minted names no channel, which is an absent channel
/// and not a malformed identifier. Only the ARN FORM is checked, because only an ARN
/// has a format to get wrong.
pub const CHANNEL_ID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// The registry key: a channel id and the Region the channel lives in.
///
/// A channel ARN carries a Region, so a channel is a regional resource and its id
/// alone is not a key. The id comes FIRST so the `BTreeMap` iterates id-ordered,
/// which is the order `ListChannels` pages through.
pub type ChannelKey = (String, String);

/// One persisted CloudTrail Lake channel: what the create request was given, plus
/// the values the service mints (the channel id and the ARN built from it).
#[derive(Debug, Clone, Default)]
pub struct ChannelRecord {
    /// The UUID CloudTrail minted for this channel — the registry's key, and the
    /// `<uuid>` of its ARN.
    pub channel_id: String,
    pub name: String,
    pub channel_arn: String,
    pub source: String,
    pub destinations: Vec<Destination>,
    pub tags: Vec<Tag>,
    pub home_region: String,
}

/// A caller-supplied channel identifier, in either of the two forms the model
/// documents for `DeleteChannelRequest$Channel` / `GetChannelRequest$Channel` ("The
/// ARN or the UUID value of the channel").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelIdentifier {
    /// A bare channel id, which names a channel of the caller's own account in the
    /// caller's own Region.
    Id(String),
    /// A channel ARN, which also names the account that owns the channel and the
    /// Region it lives in.
    Arn {
        account: String,
        region: String,
        id: String,
    },
}

impl ChannelIdentifier {
    /// The channel id this identifier names — the registry's key.
    pub fn id(&self) -> &str {
        match self {
            Self::Id(id) => id,
            Self::Arn { id, .. } => id,
        }
    }

    /// The Region this identifier pins the channel to, if it pins one.
    pub fn region(&self) -> Option<&str> {
        match self {
            Self::Id(_) => None,
            Self::Arn { region, .. } => Some(region),
        }
    }

    /// The account this identifier pins the channel to, if it pins one.
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::Id(_) => None,
            Self::Arn { account, .. } => Some(account),
        }
    }
}

/// What a well-formed channel identifier addresses.
///
/// The registry holds the channels of the emulator's own account
/// ([`::engine::emulator_account_id`]), so an identifier naming another account is
/// well formed and still names no channel — which is a different refusal from one
/// that is not a channel identifier at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelAddress {
    /// The registry key the identifier addresses.
    Key(ChannelKey),
    /// A channel of another account, which this registry never holds.
    AnotherAccount,
}

/// The ARN of the channel whose id is `channel_id`, in `region`.
pub fn channel_arn(region: &str, channel_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_NAMESPACE,
        region,
        &format!("{CHANNEL_RESOURCE_PREFIX}{channel_id}"),
    )
}

/// Whether `value` has the shape of an id CloudTrail minted: the UUID of
/// [`CHANNEL_ID_GROUP_LENGTHS`] groups of hex digits.
///
/// Only a MINTER needs this (`CreateChannel`, to mint one, and a caller that wants to
/// know whether an id could be one): resolving an identifier does not, because an id of
/// any other shape simply names no channel.
pub fn is_minted_channel_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == CHANNEL_ID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(CHANNEL_ID_GROUP_LENGTHS)
            .all(|(group, length)| {
                group.len() == length && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// Read a caller-supplied channel identifier as either a channel ARN or a bare channel
/// id. `None` is a value that is neither, so the operation refuses it with
/// `ChannelARNInvalidException`.
///
/// A value is read as an ARN as soon as it starts with `arn:`, and then it must BE a
/// channel ARN, because an ARN is the only form that has a format to get wrong: the ARN
/// itself is read through `::service_common::arn`, the one ARN parse the services share
/// (the same one `super::parse_taggable_arn` reads a tagging ARN with), and this reader
/// adds what the shared parse leaves to the service — the partition this emulator mints
/// ([`AWS_PARTITION`]), a Region and an account to address a regional resource with, and
/// the `channel` resource type. Anything else is read as a bare channel id whatever its
/// syntax: an id CloudTrail never minted names no channel, which is an absent channel,
/// not a malformed identifier.
pub fn parse_channel_identifier(value: &str) -> Option<ChannelIdentifier> {
    if value.starts_with("arn:") {
        let arn = ::service_common::arn::parse_of(value, SERVICE_NAMESPACE)?;
        if arn.partition != AWS_PARTITION || arn.region.is_empty() || arn.account.is_empty() {
            return None;
        }
        let id = ::service_common::arn::resource_id_of_type(value, CHANNEL_RESOURCE_TYPE)?;
        if id.is_empty() {
            return None;
        }
        return Some(ChannelIdentifier::Arn {
            account: arn.account.to_string(),
            region: arn.region.to_string(),
            id: id.to_string(),
        });
    }
    if value.is_empty() {
        return None;
    }
    Some(ChannelIdentifier::Id(value.to_string()))
}

/// The `ChannelARNInvalidException` text for an `arn:` value that is not a channel ARN.
///
/// Built from the request, so a caller can see which value was refused; every channel
/// operation words it the same way. The tagging path words its own channel-ARN refusal
/// differently (`super::TaggableArnRejection::ChannelArn`, "The channel ARN `<id>` is
/// invalid.") because that is what the `RemoveTags` recording shows, while this is what
/// the `GetChannel` recording shows; the two converge when a recording pins one wording
/// for both paths.
pub fn channel_arn_invalid_message(identifier: &str) -> String {
    format!("The format of the channel ARN {identifier} is not valid.")
}

/// The `ChannelNotFoundException` text for a well-formed identifier that names no
/// stored channel. Built from the request, like [`channel_arn_invalid_message`], and the
/// same sentence `super::TaggableArn::not_found_message` answers a tagging call with.
pub fn channel_not_found_message(identifier: &str) -> String {
    format!("The channel {identifier} was not found.")
}

impl ComAmazonawsCloudtrailCloudtrail20131101State {
    /// What a caller-supplied channel identifier addresses: the channel of that id in
    /// the Region the identifier pins — the ARN's own Region, or, for a bare id, the
    /// Region the call was made in — of the account the ARN names.
    ///
    /// `None` is an identifier that is not a channel identifier at all.
    pub fn addressed_channel(
        &self,
        identifier: &str,
        request_region: &str,
    ) -> Option<ChannelAddress> {
        let parsed = parse_channel_identifier(identifier)?;
        if parsed
            .account()
            .is_some_and(|account| account != ::engine::emulator_account_id())
        {
            return Some(ChannelAddress::AnotherAccount);
        }
        Some(ChannelAddress::Key((
            parsed.id().to_string(),
            parsed.region().unwrap_or(request_region).to_string(),
        )))
    }
}
