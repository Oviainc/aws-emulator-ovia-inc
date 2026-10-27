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
//!   holds — an `InputAttachment`'s `InputId` is "The ID of the input", and an id
//!   no input of this account answers to would leave the channel claiming an
//!   attachment to an input no read of the account can show (and the input-side
//!   derivation of the channels attached to an input matching nothing).
//!
//! Each problem found is reported as ONE
//! `UnprocessableEntityException$ValidationErrors` entry ("A collection of
//! validation error responses") naming the element path it was found at
//! (`ValidationError$ElementPath` is "Path to the source of the error") — the
//! channel this service already reports a request it cannot process through
//! (`BatchUpdateSchedule`, `CreateChannelPlacementGroup`).

use super::{
    missing_reference_id_message, required_channel_configuration_message, unknown_reference_message,
    validation_problem, ComAmazonawsMedialiveMedialiveState,
};
use crate::ops::types::{EncoderSettings, InputAttachment, ValidationError};

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
}

/// The member a channel's encoder settings arrive in, as the wire spells it.
const ENCODER_SETTINGS: &str = "encoderSettings";

/// The member a channel's input attachments arrive in, as the wire spells it.
const INPUT_ATTACHMENTS: &str = "inputAttachments";

/// What an `InputAttachment`'s `InputId` names, for the refusal that reports an
/// id this account holds no such resource for.
const INPUT: &str = "input";

impl ComAmazonawsMedialiveMedialiveState {
    /// Every problem `configuration` carries, in the order the configuration
    /// declares its members and its entries — empty for a configuration this
    /// service can run, which is the only one a write stores.
    ///
    /// A reference is resolved through [`Self::live_input`], the same lookup
    /// every mutation of an input goes through: an input whose delete this
    /// service has accepted is no longer a resource of this account a new
    /// attachment can name.
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

        problems
    }
}
