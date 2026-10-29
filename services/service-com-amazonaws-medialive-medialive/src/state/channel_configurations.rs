// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every operation that WRITES a channel's configuration validates
//! that configuration against, BEFORE anything is stored.
//!
//! `CreateChannel` and `UpdateChannel` both write the members a channel encodes
//! and ingests through, so the rule lives here rather than in either handler: the
//! two cannot then accept a configuration the other refuses, exactly as
//! [`super::ComAmazonawsMedialiveMedialiveState::input_configuration_error`] is
//! the one rule `CreateInput` and `UpdateInput` share.
//!
//! What the rule checks:
//!
//! * a channel configuration has to declare the encoder settings the channel
//!   encodes through (`EncoderSettings` is "Encoder Settings"), so a request that
//!   declares none describes no channel this service could run;
//! * every id the configuration names has to resolve to a resource this account
//!   holds — an `InputAttachment`'s `InputId` is "The ID of the input", a
//!   `ChannelSecurityGroups` entry is one of the "IDs for all the Input Security
//!   Groups attached to the channel", and `AnywhereSettings` carries "The ID of
//!   the cluster for the channel" and "The ID of the channel placement group for
//!   the channel" — and an id no such resource of this account answers to would
//!   leave the channel claiming a resource no read of the account can show (and
//!   the resource-side derivation of the channels using it —
//!   [`super::ComAmazonawsMedialiveMedialiveState::channels_using_security_group`],
//!   [`super::ComAmazonawsMedialiveMedialiveState::channels_in_cluster`],
//!   [`super::ComAmazonawsMedialiveMedialiveState::channels_in_channel_placement_group`]
//!   — matching nothing);
//! * a reference that is only reachable THROUGH another is resolved against the
//!   resource that one names: a channel placement group is named by
//!   (`ClusterId`, `ChannelPlacementGroupId`) together
//!   ([`super::ComAmazonawsMedialiveMedialiveState::channel_placement_group`]),
//!   so a group id is resolved inside the cluster the same settings name and
//!   settings that name a group but no cluster name a group through no cluster.
//!
//! Each problem found is reported as ONE
//! `UnprocessableEntityException$ValidationErrors` entry ("A collection of
//! validation error responses") naming the element path it was found at
//! (`ValidationError$ElementPath` is "Path to the source of the error") — the
//! channel this service already reports a request it cannot process through
//! (`BatchUpdateSchedule`, `CreateChannelPlacementGroup`).

use super::{
    blank_reference_id_message, missing_reference_id_message,
    required_channel_configuration_message, unknown_reference_message, validation_problem,
    ComAmazonawsMedialiveMedialiveState,
};
use crate::ops::types::{AnywhereSettings, EncoderSettings, InputAttachment, ValidationError};

/// The channel configuration a write would leave on a channel.
///
/// Borrowed, and assembled by the CALLER: a create's configuration is the
/// request's own, an update's is the members its request names over the ones the
/// stored channel already had, and the same rule checks both.
#[derive(Debug)]
pub struct ChannelConfiguration<'a> {
    /// `EncoderSettings` — what the channel encodes.
    pub encoder_settings: Option<&'a EncoderSettings>,
    /// `InputAttachments` — "List of input attachments for channel".
    pub input_attachments: &'a [InputAttachment],
    /// `ChannelSecurityGroups` — "A list of IDs for all the Input Security
    /// Groups attached to the channel".
    pub channel_security_groups: &'a [String],
    /// `AnywhereSettings` — where the channel runs: the cluster and the channel
    /// placement group inside it.
    pub anywhere_settings: Option<&'a AnywhereSettings>,
}

/// The member a channel's encoder settings arrive in, as the wire spells it.
const ENCODER_SETTINGS: &str = "encoderSettings";

/// The member a channel's input attachments arrive in, as the wire spells it.
const INPUT_ATTACHMENTS: &str = "inputAttachments";

/// What an `InputAttachment`'s `InputId` names, for the refusal that reports an
/// id this account holds no such resource for.
const INPUT: &str = "input";

/// The member a channel's input security groups arrive in, as the wire spells
/// it.
const CHANNEL_SECURITY_GROUPS: &str = "channelSecurityGroups";

/// What a `ChannelSecurityGroups` entry names, worded as the input side words
/// the same reference ([`super::ComAmazonawsMedialiveMedialiveState::input_configuration_error`]).
const INPUT_SECURITY_GROUP: &str = "input security group";

/// The member a channel's Elemental Anywhere placement arrives in, as the wire
/// spells it — the prefix of the element path each of its two ids is reported
/// at.
const ANYWHERE_SETTINGS: &str = "anywhereSettings";

/// The `AnywhereSettings` member naming the cluster the channel runs in, as the
/// wire spells it.
const CLUSTER_ID: &str = "clusterId";

/// What `AnywhereSettings$ClusterId` names, for the refusal that reports an id
/// this account holds no such resource for.
const CLUSTER: &str = "cluster";

/// The `AnywhereSettings` member naming the group the channel is placed in, as
/// the wire spells it.
const CHANNEL_PLACEMENT_GROUP_ID: &str = "channelPlacementGroupId";

/// What `AnywhereSettings$ChannelPlacementGroupId` names, worded as the group
/// side words the same resource.
const CHANNEL_PLACEMENT_GROUP: &str = "channel placement group";

impl ComAmazonawsMedialiveMedialiveState {
    /// Every problem `configuration` carries, in the order the configuration
    /// declares its members and its entries — empty for a configuration this
    /// service can run, which is the only one a write stores.
    ///
    /// A reference is resolved through the same lookup every other write that
    /// references the resource goes through — [`Self::live_input`] for an
    /// attachment, [`Self::live_input_security_group`] for a channel security
    /// group, [`Self::live_cluster`] and [`Self::channel_placement_group`] for
    /// the Elemental Anywhere placement (the lookups `CreateNode` and
    /// `CreateChannelPlacementGroup` resolve the same two ids through) — so a
    /// resource whose delete this service has accepted is no longer one of this
    /// account a new reference can name.
    pub fn channel_configuration_problems(
        &self,
        configuration: &ChannelConfiguration<'_>,
    ) -> Vec<ValidationError> {
        let mut problems: Vec<ValidationError> = Vec::new();

        if configuration.encoder_settings.is_none() {
            problems.push(validation_problem(
                ENCODER_SETTINGS,
                &required_channel_configuration_message(ENCODER_SETTINGS),
            ));
        }

        for (index, attachment) in configuration.input_attachments.iter().enumerate() {
            let path = format!("{INPUT_ATTACHMENTS}/{index}/inputId");
            match attachment.input_id.as_deref().filter(|id| !id.is_empty()) {
                // An attachment that names no input at all attaches nothing, so
                // it is the same problem as one naming an id this account does
                // not hold, reported the way every other reference list reports
                // an entry with no id.
                None => problems.push(validation_problem(
                    &path,
                    &missing_reference_id_message(INPUT_ATTACHMENTS),
                )),
                Some(id) if self.live_input(id).is_none() => problems.push(validation_problem(
                    &path,
                    &unknown_reference_message(INPUT_ATTACHMENTS, INPUT, id),
                )),
                Some(_) => {}
            }
        }

        for (index, group) in configuration.channel_security_groups.iter().enumerate() {
            let path = format!("{CHANNEL_SECURITY_GROUPS}/{index}");
            if group.is_empty() {
                problems.push(validation_problem(
                    &path,
                    &missing_reference_id_message(CHANNEL_SECURITY_GROUPS),
                ));
            } else if self.live_input_security_group(group).is_none() {
                problems.push(validation_problem(
                    &path,
                    &unknown_reference_message(
                        CHANNEL_SECURITY_GROUPS,
                        INPUT_SECURITY_GROUP,
                        group,
                    ),
                ));
            }
        }

        if let Some(settings) = configuration.anywhere_settings {
            self.collect_anywhere_problems(settings, &mut problems);
        }

        problems
    }

    /// Every problem the `AnywhereSettings` of a channel configuration carries,
    /// appended to `problems`.
    ///
    /// The cluster is resolved first, through the same
    /// [`Self::live_cluster`] every write addressed to a cluster goes through,
    /// and the group is then resolved INSIDE it through the same
    /// [`Self::channel_placement_group`] a read of that group goes through — a
    /// group is named by both ids together, so a group of another cluster
    /// resolves no better than one the account never created, and a group whose
    /// delete this service has accepted is no longer one a new placement can
    /// name. A group id whose cluster did not resolve is not reported a second
    /// time: the cluster problem already names what the caller must change.
    fn collect_anywhere_problems(
        &self,
        settings: &AnywhereSettings,
        problems: &mut Vec<ValidationError>,
    ) {
        let named_cluster = settings.cluster_id.as_deref();
        let named_group = settings.channel_placement_group_id.as_deref();
        let cluster_path = format!("{ANYWHERE_SETTINGS}/{CLUSTER_ID}");

        let cluster = match named_cluster.filter(|id| !id.is_empty()) {
            // Settings that carry a blank cluster id, or that place the channel
            // in a group without naming the cluster that group lives in, name
            // no cluster at all — and a channel with no cluster is no channel
            // this account's clusters can report.
            None if named_cluster.is_some() || named_group.is_some() => {
                problems.push(validation_problem(
                    &cluster_path,
                    &blank_reference_id_message(CLUSTER_ID),
                ));
                None
            }
            None => None,
            Some(id) if self.live_cluster(id).is_none() => {
                problems.push(validation_problem(
                    &cluster_path,
                    &unknown_reference_message(CLUSTER_ID, CLUSTER, id),
                ));
                None
            }
            Some(id) => Some(id),
        };

        let Some(group_id) = named_group else {
            return;
        };
        let group_path = format!("{ANYWHERE_SETTINGS}/{CHANNEL_PLACEMENT_GROUP_ID}");
        if group_id.is_empty() {
            problems.push(validation_problem(
                &group_path,
                &blank_reference_id_message(CHANNEL_PLACEMENT_GROUP_ID),
            ));
            return;
        }
        let Some(cluster_id) = cluster else {
            return;
        };
        if self
            .channel_placement_group(cluster_id, group_id)
            .is_none_or(|group| group.is_delete_accepted())
        {
            problems.push(validation_problem(
                &group_path,
                &unknown_reference_message(
                    CHANNEL_PLACEMENT_GROUP_ID,
                    CHANNEL_PLACEMENT_GROUP,
                    group_id,
                ),
            ));
        }
    }
}
