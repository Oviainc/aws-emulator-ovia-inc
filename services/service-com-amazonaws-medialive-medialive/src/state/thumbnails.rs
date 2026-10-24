// SPDX-License-Identifier: CC0-1.0

//! The thumbnail slice of this service's state — the frames a channel's encoder
//! pipelines captured, which is what `DescribeThumbnails` reports the latest of.
//!
//! One file for the whole slice — the record, its bound, the reader a channel
//! answers through and the store's one writer — so the slice can be read, and
//! merged, on its own. The log itself is the `thumbnails` member of
//! [`ChannelRecord`] in the parent module, because that is what owns it: a
//! channel's frames go away with the channel, and no operation reaches a frame
//! without naming its channel.
//!
//! What a read reports is DERIVED, never recorded: the latest frame by capture
//! time ([`ChannelRecord::latest_thumbnail`]), reported only while the pipeline
//! it came from is up and the channel's thumbnail feature is on
//! ([`ChannelRecord::thumbnails_enabled`], out of the stored
//! `EncoderSettings$ThumbnailConfiguration`). Every member of a reported
//! `Thumbnail` is the stored one ([`thumbnail_detail_output`]) — the pipeline id
//! included, so a detail names the pipeline the frame was captured FROM rather
//! than the one a request asked about.
//!
//! NOTHING in this emulator captures a frame, and nothing here invents one: the
//! real service generates them in the encoder pipelines it runs for a channel
//! (`ThumbnailConfiguration$State`: "The feature generates thumbnails of the
//! incoming video in each pipeline in the channel"), and this emulator encodes no
//! media. So a channel created here carries an empty capture log, and
//! `DescribeThumbnails` reports that log — whatever is in it — rather than
//! fabricated image data.
//! [`ComAmazonawsMedialiveMedialiveState::capture_channel_thumbnail`] is the one
//! way a frame gets into the log, bounded by [`MAX_CHANNEL_THUMBNAILS`], on the
//! same terms as the cluster slice's own alert log.

use std::time::SystemTime;

use crate::ops::types::{Thumbnail, ThumbnailDetail, THUMBNAIL_TYPE_VALUES};

use super::{ChannelRecord, ComAmazonawsMedialiveMedialiveState};

/// The `ThumbnailState` value this slice uses, spelled exactly as the model
/// declares it (`smithy_query(query="shape", shape="ThumbnailState")`: AUTO |
/// DISABLED).
///
/// `DISABLED` is deliberately absent: it is the ABSENCE of `AUTO` that turns the
/// feature off (the `ThumbnailConfiguration$State` doc: "AUTO turns the feature
/// on, DISABLE turns the feature off"), so a channel whose `EncoderSettings`
/// carry no `ThumbnailConfiguration` at all is off for the same reason as one
/// that carries `DISABLED`, and only the one value that turns it ON is named
/// here.
pub mod thumbnail_state {
    pub const AUTO: &str = "AUTO";
}

/// The most captured thumbnail frames this emulator keeps for ONE channel.
///
/// A ceiling for the same reason as [`super::MAX_CLUSTER_ALERTS`]: a capture log
/// that grew without bound would grow the emulator's heap without bound. The
/// OLDEST frame is dropped once the log is full, because what a read reports is
/// the LATEST frame of a pipeline, so the recent end of the log is the useful
/// one.
pub const MAX_CHANNEL_THUMBNAILS: usize = 20;

/// One thumbnail frame captured from one pipeline of a running channel.
///
/// The members mirror the model's `Thumbnail` shape, with the pipeline the frame
/// came from beside them: `ThumbnailDetail` is "Thumbnail details for one
/// pipeline of a running channel", so which pipeline a frame belongs to is part
/// of the record rather than of the request that reads it.
#[derive(Debug, Clone)]
pub struct ThumbnailRecord {
    /// The pipeline this frame was captured from, as
    /// `PipelineDetail$PipelineId` numbers it (0 for `PIPELINE_0`).
    pub pipeline: i32,
    /// "Thumbnail Type" — one of the modelled `ThumbnailType` values
    /// (`smithy_query(query="shape", shape="ThumbnailType")`: UNSPECIFIED |
    /// CURRENT_ACTIVE), which is what `DescribeThumbnails`' own `ThumbnailType`
    /// selects on.
    pub thumbnail_type: String,
    /// "The binary data for the latest thumbnail", as the model's `__string`
    /// carries it. Absent on a frame whose capturer reported none; never
    /// invented here, because this emulator encodes no media of its own.
    pub body: Option<String>,
    /// "The content type for the latest thumbnail".
    pub content_type: Option<String>,
    /// "Time stamp for the latest thumbnail" — the instant the frame was
    /// captured, which is what orders the log and so what makes one frame the
    /// latest.
    pub captured_at: SystemTime,
}

impl ThumbnailRecord {
    /// A frame of `thumbnail_type` captured from `pipeline` at `captured_at`,
    /// carrying no image data of its own: whatever captured it fills in
    /// [`Self::body`] and [`Self::content_type`].
    pub fn captured(
        pipeline: i32,
        thumbnail_type: impl Into<String>,
        captured_at: SystemTime,
    ) -> Self {
        Self {
            pipeline,
            thumbnail_type: thumbnail_type.into(),
            body: None,
            content_type: None,
            captured_at,
        }
    }
}

/// Project one captured [`ThumbnailRecord`] onto the model's `ThumbnailDetail`:
/// the pipeline the frame came from and the frame itself, in the model's
/// `Thumbnail` shape.
///
/// Every member is the stored one — the pipeline id included, so a detail names
/// the pipeline the frame was CAPTURED from rather than the one the request
/// asked about. One frame per detail: the operation is documented "Describe the
/// latest thumbnails data", so what a pipeline contributes is its latest frame
/// of the requested kind.
pub fn thumbnail_detail_output(thumbnail: &ThumbnailRecord) -> ThumbnailDetail {
    ThumbnailDetail {
        pipeline_id: Some(thumbnail.pipeline.to_string()),
        thumbnails: Some(vec![Thumbnail {
            body: thumbnail.body.clone(),
            content_type: thumbnail.content_type.clone(),
            thumbnail_type: Some(thumbnail.thumbnail_type.clone()),
            time_stamp: Some(super::timestamp(thumbnail.captured_at)),
        }]),
    }
}

impl ChannelRecord {
    /// Whether this channel's thumbnail feature is on: its
    /// `EncoderSettings$ThumbnailConfiguration` carries `State` `AUTO` (that
    /// member's own doc: "Enables the thumbnail feature. The feature generates
    /// thumbnails of the incoming video in each pipeline in the channel. AUTO
    /// turns the feature on, DISABLE turns the feature off").
    ///
    /// A channel whose encoder settings carry no `ThumbnailConfiguration` at all
    /// has the feature off, which is why this is derived from the stored settings
    /// rather than from a flag of its own.
    pub fn thumbnails_enabled(&self) -> bool {
        self.encoder_settings
            .as_ref()
            .and_then(|settings| settings.thumbnail_configuration.as_ref())
            .is_some_and(|configuration| configuration.state == thumbnail_state::AUTO)
    }

    /// The latest frame of `thumbnail_type` this channel has captured from
    /// `pipeline`, which is what `DescribeThumbnails` reports ("Describe the
    /// latest thumbnails data").
    ///
    /// Latest BY CAPTURE TIME rather than by position in the log, so a frame
    /// recorded out of order does not become the latest one.
    ///
    /// Reported only while the pipeline the frame came from is one of the
    /// channel's healthy ones ([`ChannelRecord::healthy_pipelines`]) and the
    /// feature is still on ([`Self::thumbnails_enabled`]): a `ThumbnailDetail`
    /// is "Thumbnail details for one pipeline of a RUNNING channel", so a
    /// channel that has stopped and a pipeline that is down right now report
    /// nothing, however many frames the log still holds for them. This is a
    /// READ: the log itself is the capture history and is not pruned by
    /// reporting it.
    pub fn latest_thumbnail(
        &self,
        pipeline: i32,
        thumbnail_type: &str,
    ) -> Option<&ThumbnailRecord> {
        if !self.thumbnails_enabled() || !self.healthy_pipelines().contains(&pipeline) {
            return None;
        }
        self.thumbnails
            .iter()
            .filter(|thumbnail| {
                thumbnail.pipeline == pipeline && thumbnail.thumbnail_type == thumbnail_type
            })
            .max_by_key(|thumbnail| thumbnail.captured_at)
    }
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Record `thumbnail` as captured from the channel `channel_id` names,
    /// answering whether that channel took it.
    ///
    /// Refused (`false`) unless the capture could really have happened: the
    /// channel exists, its thumbnail feature is on
    /// ([`ChannelRecord::thumbnails_enabled`]), the frame's kind is a modelled
    /// `ThumbnailType`, and the pipeline it came from is one of the channel's
    /// healthy ones — the feature "generates thumbnails of the incoming video in
    /// each pipeline in the channel", so a pipeline that is not up right now (a
    /// channel that is not running, or a pipeline a restart took down) captures
    /// nothing and may record nothing.
    ///
    /// Bounded, evicting write: a channel keeps at most
    /// [`MAX_CHANNEL_THUMBNAILS`] frames and drops the oldest first, so a
    /// channel's capture history cannot grow the emulator's heap without bound.
    /// The clamp itself is [`ChannelRecord::push_thumbnail_bounded`], beside the
    /// private log it bounds, which is the only way a frame reaches that log.
    ///
    /// The real service captures these frames in the encoder pipelines it runs
    /// for a channel; this emulator encodes no media, so no operation in it calls
    /// this and a channel created through `CreateChannel` has an empty log (see
    /// the module docs). It is the ONE way a frame reaches the log, so
    /// `DescribeThumbnails` reports what was captured rather than anything it
    /// invented.
    pub fn capture_channel_thumbnail(
        &mut self,
        channel_id: &str,
        thumbnail: ThumbnailRecord,
    ) -> bool {
        let Some(channel) = self.channels.get_mut(channel_id) else {
            return false;
        };
        if !channel.thumbnails_enabled()
            || !THUMBNAIL_TYPE_VALUES.contains(&thumbnail.thumbnail_type.as_str())
            || !channel.healthy_pipelines().contains(&thumbnail.pipeline)
        {
            return false;
        }
        channel.push_thumbnail_bounded(thumbnail);
        true
    }
}
