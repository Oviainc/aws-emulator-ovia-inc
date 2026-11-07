// SPDX-License-Identifier: CC0-1.0

//! The input device slice of this service's state.
//!
//! One file for the whole slice — the record, its bounds, the device-id form a
//! claim accepts, the store's methods and the output projection — so the slice
//! can be read, and merged, on its own. The store itself is the `input_devices`
//! member of [`ComAmazonawsMedialiveMedialiveState`] in the parent module,
//! beside the channels and inputs, so every operation that reaches a device
//! reads the one copy.
//!
//! An input device is not CREATED by an API call the way a channel is: it is a
//! physical AWS Elemental Link unit, and `ClaimDevice` is what brings one into
//! an account ("Send a request to claim an AWS Elemental device that you have
//! purchased from a third-party vendor. After the request succeeds, you will own
//! the device"). This emulator has no hardware inventory to consult, so a claim
//! of a well-formed device id is what registers the device here — and every
//! member a response carries is then either stored on the record or derived from
//! the rest of the store ([`ComAmazonawsMedialiveMedialiveState::input_device_summary`]),
//! never built out of the request that read it.
//!
//! The members the real service reads OFF the device (its MAC address, its
//! serial number, its network settings, the HD/UHD settings that describe the
//! video it is producing, the availability zone its attachment allocates) are
//! NOT invented here: no device is connected to this emulator, so they stay
//! absent and `ConnectionState` is the `DISCONNECTED` the model gives that
//! situation.
//!
//! A device's pending TRANSFER is one optional record on the device it is a
//! transfer of ([`InputDeviceTransferRecord`]): `TransferInputDevice` records it
//! ("Start an input device transfer to another AWS account. After you make the
//! request, the other account must accept or reject the transfer"), and
//! `ListInputDeviceTransfers` reports it, filtered by the direction the request
//! asks for. It lives ON the device record rather than in a map beside the
//! devices, because a transfer is a transfer OF one device and at most one is
//! pending on a device at a time, so a reader of the device and a reader of the
//! transfer cannot drift apart.
//!
//! A device's RUN STATE is one stored flag on the record: whether the device is
//! producing video right now. `StartInputDevice` sets it and `StopInputDevice`
//! clears it, and it is reported as the `DeviceState` of the device's own
//! type settings — the one member of those settings this service knows without
//! a unit to read the rest off, because it is the state the commands the
//! service accepted put the device in rather than a characteristic of the video
//! a connected unit is producing.
//!
//! A device's CONFIGURATION is the part of its settings the CALLER chooses:
//! `UpdateInputDevice` "[u]pdates the parameters for the input device", and the
//! members its `InputDeviceConfigurableSettings` carries (the source to
//! activate, the maximum bitrate, the buffer size, and for a UHD device the
//! codec, the input resolution, the audio pair profiles and the MediaConnect
//! flow the device is attached to) are stored on the record
//! ([`InputDeviceConfiguration`]) and reported in the settings of the device's
//! own type. They are the only members of those settings this service knows
//! beside the run state: every other one is measured off the video a connected
//! unit is producing. The NAME and the AVAILABILITY ZONE an update sets are
//! MediaLive's own record of the device rather than device settings, so they sit
//! beside the configuration rather than in it.
//!
//! A configuration change is SENT to the device, which is the second thing the
//! clock owes a device here: `DeviceSettingsSyncState` is "SYNCED means the
//! device has updated its configuration. SYNCING means that it has not updated
//! its configuration", so a device an update has just configured reports
//! `SYNCING` until the stored instant comes due and `SYNCED` again afterwards.
//!
//! A device's MAINTENANCE WINDOW is one stored instant on the record: the
//! moment the window the clock owes closes. `StartInputDeviceMaintenanceWindow`
//! opens it, which is what puts the device's `DeviceUpdateStatus` in `UPDATING`,
//! and [`ComAmazonawsMedialiveMedialiveState::settle`] — which every operation
//! runs first — is what closes it once it comes due, back to `UP_TO_DATE`. The
//! instant is stored and never slept on.
//!
//! A device's REBOOT is the third thing the clock owes it:
//! `RebootInputDevice` "[s]end[s] a reboot command to the specified input
//! device", and the record keeps the instant that reboot completes beside
//! whether the device resumes producing video when it does
//! ([`InputDeviceRecord::begin_reboot`]). A rebooting device is producing
//! nothing, so a reboot puts its `InputDeviceState` in `IDLE` for as long as it
//! lasts, and a reboot the request FORCED is the one that resumes the stream it
//! interrupted once it completes ("[i]f the device was streaming prior to the
//! reboot, the device will resume streaming when the reboot completes"); a
//! start or a stop issued in the meantime is the device's run state from then
//! on and cancels that resumption. A reboot does NOT move the device's
//! `InputDeviceConnectionState`: no Link unit is connected to this emulator, so
//! a completed reboot has nothing to reconnect.
//!
//! A pending transfer waits on the RECIPIENT's decision rather than on the
//! clock, so `settle` never touches it: `AcceptInputDeviceTransfer` is what ends
//! one ([`InputDeviceRecord::accept_transfer`]), and only for the transfer the
//! asking account is the recipient of — "the ownership of the device will
//! transfer to your AWS account". This emulator serves one account, so the
//! transfer it can accept is one addressed to
//! [`engine::emulator_account_id`], and one addressed to any other account id
//! stays pending for a recipient that never calls.
//!
//! REJECTING is the recipient's OTHER decision
//! ([`InputDeviceRecord::reject_transfer`]): "[r]eject the transfer of the
//! specified input device to your AWS account" ends the very same pending
//! transfer the accept would have ended, and is therefore open to the very same
//! account — the recipient — so both decisions resolve a transfer through the
//! slice's one rule ([`InputDeviceRecord::end_incoming_transfer`]). What differs
//! is where the device ends up: an accepted device is the recipient's and a
//! rejected one stays the sender's. Either way nothing is pending on it
//! afterwards, so the device is transferable again and the other decision has
//! nothing left to resolve.
//!
//! The SENDER's own way out of a pending transfer is
//! `CancelInputDeviceTransfer` ([`InputDeviceRecord::cancel_transfer`]):
//! "[c]ancel an input device transfer that you have requested" ends the
//! transfer this account started, so the device stays this account's with
//! nothing pending on it and can be transferred again.
//!
//! A device's latest THUMBNAIL is one optional record on the device it is a
//! thumbnail of ([`InputDeviceThumbnailRecord`]): the image data, the media type
//! it is in and the instant the device last updated it — which is exactly what
//! `DescribeInputDeviceThumbnail` reports ("the thumbnail that the Link device
//! has most recently sent to MediaLive"). Its length and its entity tag are
//! DERIVED from that data on every read, never stored beside it, so a reported
//! `ContentLength` and `ETag` cannot disagree with the bytes they describe.
//!
//! A thumbnail reaches the store exactly one way,
//! [`ComAmazonawsMedialiveMedialiveState::record_input_device_thumbnail`], which
//! is the Link unit SENDING one. No unit is connected to this emulator — the
//! same reason the device's MAC address, serial number and network settings stay
//! absent — so no operation in this service calls it and a device claimed
//! through the API has no thumbnail, which is what the read then reports: the
//! empty payload its `Body` member defaults to, and none of the metadata that
//! describes an image that was never sent.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    InputDeviceConfigurableSettings, InputDeviceHdSettings, InputDeviceMediaConnectSettings,
    InputDeviceSummary, InputDeviceUhdAudioChannelPairConfig, InputDeviceUhdSettings,
    TransferringInputDeviceSummary, CONTENT_TYPE_VALUES,
};

use super::{ComAmazonawsMedialiveMedialiveState, TRANSIENT_STATE_SPAN};

/// The `DeviceUpdateStatus` values this slice reports, spelled exactly as the
/// model declares them (`smithy_query(query="shape",
/// shape="DeviceUpdateStatus")`: UP_TO_DATE | NOT_UP_TO_DATE | UPDATING). The
/// one the model declares but this slice never reports is `NOT_UP_TO_DATE`: it
/// says the real service is holding newer software for the device, and this
/// emulator ships no device software, so it never claims an update is waiting.
pub mod device_update_status {
    pub const UP_TO_DATE: &str = "UP_TO_DATE";
    pub const UPDATING: &str = "UPDATING";
}

/// The `InputDeviceConnectionState` values this slice reports, spelled exactly
/// as the model declares them (`smithy_query(query="shape",
/// shape="InputDeviceConnectionState")`: DISCONNECTED | CONNECTED). A claimed
/// device reports `DISCONNECTED`, because no physical Link unit is connected to
/// this emulator; `CONNECTED` is not claimed for a device that is not there.
pub mod input_device_connection_state {
    pub const DISCONNECTED: &str = "DISCONNECTED";
}

/// The `DeviceSettingsSyncState` values this slice reports, spelled exactly as
/// the model declares them (`smithy_query(query="shape",
/// shape="DeviceSettingsSyncState")`: SYNCED | SYNCING). `SYNCED` means "the
/// device has updated its configuration", which is the state of a device this
/// store holds no pending configuration change for; `SYNCING` means "it has not
/// updated its configuration", which is the state of one an update has just
/// configured, until the clock makes the sync due.
pub mod device_settings_sync_state {
    pub const SYNCED: &str = "SYNCED";
    pub const SYNCING: &str = "SYNCING";
}

/// The `InputDeviceState` values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="InputDeviceState")`: IDLE | STREAMING).
/// The enum is "The state of the input device": `STREAMING` while the device is
/// producing video, `IDLE` while it is not. Both are reported by this slice —
/// they are the two states a start and a stop move a device between.
pub mod input_device_state {
    pub const IDLE: &str = "IDLE";
    pub const STREAMING: &str = "STREAMING";
}

/// The `RebootInputDeviceForce` values, spelled exactly as the model declares
/// them (`smithy_query(query="shape", shape="RebootInputDeviceForce")`: NO |
/// YES). `YES` is the one a reboot reads: forcing a reboot is what makes the
/// device "resume streaming when the reboot completes", and `NO` — like the
/// request that leaves the optional member out — asks for no such resumption,
/// so it is the absence of the force rather than a value of its own.
pub mod reboot_input_device_force {
    pub const YES: &str = "YES";
}

/// The `InputDeviceType` values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="InputDeviceType")`: HD | UHD).
pub mod input_device_type {
    pub const HD: &str = "HD";
    pub const UHD: &str = "UHD";
}

/// The `InputDeviceTransferType` values, spelled exactly as the model declares
/// them (`smithy_query(query="shape", shape="InputDeviceTransferType")`:
/// OUTGOING | INCOMING). The enum's own documentation fixes which direction each
/// one names: "INCOMING for an input device that is being transferred to you,
/// OUTGOING for an input device that you are transferring to another AWS
/// account".
///
/// `ListInputDeviceTransfers` takes the direction as a `__string` rather than as
/// this enum, so the service is what refuses a value outside it — hence
/// [`ALL`](input_device_transfer_type::ALL), the one list of the accepted
/// spellings, which both the refusal's wording and the direction filter read.
pub mod input_device_transfer_type {
    pub const OUTGOING: &str = "OUTGOING";
    pub const INCOMING: &str = "INCOMING";

    /// Every value the enum declares, in the model's order.
    pub const ALL: [&str; 2] = [OUTGOING, INCOMING];
}

/// The `InputDeviceOutputType` values this slice derives, spelled exactly as the
/// model declares them (`smithy_query(query="shape",
/// shape="InputDeviceOutputType")`: NONE | MEDIALIVE_INPUT |
/// MEDIACONNECT_FLOW). All three are derived from the rest of the device's
/// state: `MEDIACONNECT_FLOW` "if this device is the source for a MediaConnect
/// flow", which is a flow an update attached to it; `MEDIALIVE_INPUT` "if this
/// device is the source for a MediaLive input", which is an input of this store
/// that ingests from it; and `NONE` for a device attached to neither.
pub mod input_device_output_type {
    pub const NONE: &str = "NONE";
    pub const MEDIALIVE_INPUT: &str = "MEDIALIVE_INPUT";
    pub const MEDIACONNECT_FLOW: &str = "MEDIACONNECT_FLOW";
}

/// The resolutions a Link device's source can be configured to, spelled as the
/// `InputResolution` member documents them: "Choose the resolution of the Link
/// device's source (HD or UHD)". The member is a `__string` rather than an enum,
/// so the SERVICE is what refuses a value outside it — hence one list, which
/// both the refusal's wording and the check read.
pub mod input_resolution {
    pub const ALL: [&str; 2] = ["HD", "UHD"];
}

/// How many audio configurations a device's `AudioChannelPairs` carries: "An
/// array of eight audio configurations, one for each audio pair in the source".
pub const MAX_AUDIO_CHANNEL_PAIRS: usize = 8;

/// The lowest audio pair id a configuration may name: "The ID for one audio pair
/// configuration, a value from 1 to 8".
pub const FIRST_AUDIO_CHANNEL_PAIR_ID: i32 = 1;

/// The highest audio pair id a configuration may name. See
/// [`FIRST_AUDIO_CHANNEL_PAIR_ID`].
pub const LAST_AUDIO_CHANNEL_PAIR_ID: i32 = 8;

/// The id prefix of an HD Link device. Every operation that names a device
/// documents the id's form by example — `InputDeviceId` is "The unique ID of the
/// input device. For example, hd-123456789abcdef" — and the prefix is what says
/// which `InputDeviceType` the unit is, so it is also where a claimed device's
/// `Type` comes from.
pub const HD_DEVICE_ID_PREFIX: &str = "hd-";

/// The id prefix of a UHD Link device, the other member of the `InputDeviceType`
/// enum. See [`HD_DEVICE_ID_PREFIX`].
pub const UHD_DEVICE_ID_PREFIX: &str = "uhd-";

/// The most input devices this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: the ids are caller-chosen,
/// so without a cap a claim loop would grow the emulator's heap without bound.
/// `ClaimDevice` models `TooManyRequestsException`, which is the refusal once the
/// store is full. Nothing in this service un-claims a device, so the ceiling is
/// terminal for a process rather than something a delete frees again — which is
/// what the refusal says.
pub const MAX_INPUT_DEVICES: usize = 100;

/// The `InputDeviceType` the device id `id` names, or `None` when `id` is not of
/// the form every input device operation documents (an `hd-` or `uhd-` prefix
/// followed by the unit's identifier).
///
/// One definition of that form for the whole slice, so the claim that accepts an
/// id and the `Type` a later read reports cannot disagree about it.
///
/// The PREFIX is matched case-insensitively, because it is printed on the unit's
/// label as much as it is typed; the id itself is NOT folded. A device id is the
/// unit's own identifier rather than a name this service assigns, so the claim
/// stores it exactly as the request spelled it and a later read answers to that
/// spelling — the same way every other id in this store is matched.
pub fn input_device_type_for_id(id: &str) -> Option<&'static str> {
    let lowered = id.to_ascii_lowercase();
    for (prefix, device_type) in [
        (UHD_DEVICE_ID_PREFIX, input_device_type::UHD),
        (HD_DEVICE_ID_PREFIX, input_device_type::HD),
    ] {
        if let Some(unit) = lowered.strip_prefix(prefix) {
            if !unit.is_empty() {
                return Some(device_type);
            }
        }
    }
    None
}

/// The message a claim refused for an id that is not a device id carries: the id
/// the request named and the form the operation documents for it. Built from the
/// request, so it names what the caller must change.
pub fn not_a_device_id_message(id: &str) -> String {
    format!(
        "{id} is not an input device id: an id starts with {HD_DEVICE_ID_PREFIX} \
         or {UHD_DEVICE_ID_PREFIX} followed by the device's identifier"
    )
}

/// The longest `TransferMessage` a transfer accepts, in characters: the member's
/// own documentation is the constraint ("An optional message for the recipient.
/// Maximum 280 characters."), and the model carries no `@length` for it, so the
/// service is what enforces it.
pub const MAX_TRANSFER_MESSAGE_CHARS: usize = 280;

/// The number of digits in the account id a transfer names: `TargetCustomerId`
/// is "The AWS account ID (12 digits) for the recipient of the device transfer".
pub const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is an AWS account id, the form `TargetCustomerId` documents
/// ([`ACCOUNT_ID_DIGITS`] decimal digits).
pub fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_DIGITS && value.bytes().all(|digit| digit.is_ascii_digit())
}

/// The message a transfer refused for a `TargetCustomerId` that is not an
/// account id carries: the value the request named and the form the member
/// documents. Built from the request, so it names what the caller must change.
pub fn not_an_account_id_message(value: &str) -> String {
    format!(
        "{value} is not an AWS account ID: TargetCustomerId is the recipient's \
         {ACCOUNT_ID_DIGITS}-digit AWS account ID"
    )
}

/// One transfer of one input device, pending until the other account accepts or
/// rejects it (or the sender cancels it).
///
/// The members are the ones `TransferInputDevice` records and
/// `TransferringInputDeviceSummary` reports back; the summary's `TransferType`
/// is NOT among them, because the direction is not a property of the transfer
/// but of the side reading it (see
/// [`ComAmazonawsMedialiveMedialiveState::input_device_transfer_summary`]).
#[derive(Debug, Clone)]
pub struct InputDeviceTransferRecord {
    /// "The AWS account ID (12 digits) for the recipient of the device
    /// transfer", absent when the request named none.
    pub target_customer_id: Option<String>,
    /// "The target AWS region to transfer the device", absent when the request
    /// named none.
    pub target_region: Option<String>,
    /// "An optional message for the recipient", which the summary reports as
    /// `Message` — "The optional message that the sender has attached to the
    /// transfer".
    pub message: Option<String>,
}

/// The largest thumbnail this emulator keeps for one device, in bytes.
///
/// A ceiling, not a published quota: a thumbnail is pushed INTO the store rather
/// than created by an API call, so without a cap one sender could grow the
/// emulator's heap without bound. A Link unit's preview image is a small JPEG, so
/// a mebibyte is room for one many times over.
pub const MAX_INPUT_DEVICE_THUMBNAIL_BYTES: usize = 1024 * 1024;

/// The latest thumbnail one input device has sent.
///
/// The members are the ones the device SENDS: the image data, the media type it
/// is in, and the instant the device last updated it. The `ContentLength` and the
/// `ETag` a read reports are NOT among them — both are derived from the data (see
/// [`InputDeviceThumbnailRecord::content_length`] and
/// [`InputDeviceThumbnailRecord::etag`]), so neither can drift from the bytes it
/// describes.
#[derive(Debug, Clone)]
pub struct InputDeviceThumbnailRecord {
    /// "The binary data for the thumbnail that the Link device has most recently
    /// sent to MediaLive".
    pub body: Vec<u8>,
    /// The media type that data is in, which a read reports as the thumbnail's
    /// `ContentType` ("Specifies the media type of the thumbnail"). One of the
    /// values the model's own enum declares, which
    /// [`ComAmazonawsMedialiveMedialiveState::record_input_device_thumbnail`] is
    /// what holds it to.
    pub content_type: String,
    /// "The date and time the thumbnail was last updated at the device" — the
    /// device's instant, carried with the image rather than read off this
    /// service's clock, because it is when the UNIT updated it.
    pub last_modified: SystemTime,
}

impl InputDeviceThumbnailRecord {
    /// The thumbnail `body`, in the media type `content_type`, as the device last
    /// updated it at `last_modified`.
    pub fn new(body: Vec<u8>, content_type: &str, last_modified: SystemTime) -> Self {
        Self {
            body,
            content_type: content_type.to_string(),
            last_modified,
        }
    }

    /// "The length of the content": the length of the data itself, derived on
    /// every read so it is the length of what the response actually carries.
    ///
    /// Saturating rather than wrapping, so a body longer than `i64::MAX` could
    /// only ever under-report; nothing this store accepts comes near it
    /// ([`MAX_INPUT_DEVICE_THUMBNAIL_BYTES`]).
    pub fn content_length(&self) -> i64 {
        i64::try_from(self.body.len()).unwrap_or(i64::MAX)
    }

    /// "The unique, cacheable version of this thumbnail": an entity tag over the
    /// image data.
    ///
    /// Derived from the CONTENT, so it is stable across reads of one thumbnail
    /// and differs whenever the data differs — which is what makes it cacheable
    /// (a client that holds this tag holds that exact image). A digest is all
    /// that takes, which the `sha2` and `hex` crates this crate already depends
    /// on provide. Quoted, because an entity tag travels on the wire as a quoted
    /// string.
    pub fn etag(&self) -> String {
        use ::sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(&self.body);
        format!("\"{}\"", ::hex::encode(hasher.finalize()))
    }
}

/// The configuration an update has applied to one input device: the members of
/// `InputDeviceConfigurableSettings` a caller chooses, as they stand.
///
/// They are held in the shapes a READ reports them in
/// (`InputDeviceMediaConnectSettings`, `InputDeviceUhdAudioChannelPairConfig`) —
/// the configurable shapes carry the same members under the same names — so the
/// projection reports what was stored rather than re-deriving it per reader.
#[derive(Debug, Clone, Default)]
pub struct InputDeviceConfiguration {
    /// "The input source that you want to use", reported as the settings'
    /// `ConfiguredInput`.
    pub configured_input: Option<String>,
    /// "The maximum bitrate in bits per second".
    pub max_bitrate: Option<i32>,
    /// "The Link device's buffer size (latency) in milliseconds (ms)".
    pub latency_ms: Option<i32>,
    /// "Choose the codec for the video that the device produces" — UHD only.
    pub codec: Option<String>,
    /// The MediaConnect flow the device is attached to, or `None` while it is
    /// attached to none — UHD only.
    pub mediaconnect: Option<InputDeviceMediaConnectSettings>,
    /// The audio pair configurations the device produces as output — UHD only.
    pub audio_channel_pairs: Option<Vec<InputDeviceUhdAudioChannelPairConfig>>,
    /// "Choose the resolution of the Link device's source (HD or UHD)" — UHD
    /// only.
    pub input_resolution: Option<String>,
}

impl InputDeviceConfiguration {
    /// Apply the settings one update carried.
    ///
    /// Member by member: a member the request omits leaves the stored value
    /// alone, because an update names what it changes. The MediaConnect
    /// attachment is the one member whose documentation makes a PRESENT value a
    /// wholesale replacement — "To attach this device to a MediaConnect flow,
    /// specify these parameters. To detach an existing flow, enter {} for the
    /// value of mediaconnectSettings" — so settings carrying the member with
    /// none of its own members detach the flow.
    fn apply(&mut self, settings: InputDeviceConfigurableSettings) {
        if let Some(configured_input) = settings.configured_input {
            self.configured_input = Some(configured_input);
        }
        if let Some(max_bitrate) = settings.max_bitrate {
            self.max_bitrate = Some(max_bitrate);
        }
        if let Some(latency_ms) = settings.latency_ms {
            self.latency_ms = Some(latency_ms);
        }
        if let Some(codec) = settings.codec {
            self.codec = Some(codec);
        }
        if let Some(mediaconnect) = settings.mediaconnect_settings {
            let attached = mediaconnect.flow_arn.is_some()
                || mediaconnect.role_arn.is_some()
                || mediaconnect.secret_arn.is_some()
                || mediaconnect.source_name.is_some();
            self.mediaconnect = attached.then_some(InputDeviceMediaConnectSettings {
                flow_arn: mediaconnect.flow_arn,
                role_arn: mediaconnect.role_arn,
                secret_arn: mediaconnect.secret_arn,
                source_name: mediaconnect.source_name,
            });
        }
        if let Some(pairs) = settings.audio_channel_pairs {
            self.audio_channel_pairs = Some(
                pairs
                    .into_iter()
                    .map(|pair| InputDeviceUhdAudioChannelPairConfig {
                        id: pair.id,
                        profile: pair.profile,
                    })
                    .collect(),
            );
        }
        if let Some(input_resolution) = settings.input_resolution {
            self.input_resolution = Some(input_resolution);
        }
    }
}

/// The member of `InputDeviceConfigurableSettings` only a UHD device can be
/// configured with that `settings` names, or `None` when it names none.
///
/// Each member's own documentation is what puts it here: `Codec`,
/// `InputResolution` and `MediaconnectSettings` are each "Only UHD devices can
/// specify this parameter", and `AudioChannelPairs` "applies only to UHD
/// devices" ("For an HD device, you configure the audio by setting up audio
/// selectors in the channel configuration"). They are spelled as the members
/// are, because the refusal names what the caller must drop.
fn uhd_only_member(settings: &InputDeviceConfigurableSettings) -> Option<&'static str> {
    [
        ("Codec", settings.codec.is_some()),
        (
            "MediaconnectSettings",
            settings.mediaconnect_settings.is_some(),
        ),
        ("AudioChannelPairs", settings.audio_channel_pairs.is_some()),
        ("InputResolution", settings.input_resolution.is_some()),
    ]
    .into_iter()
    .find_map(|(member, named)| named.then_some(member))
}

/// The message an update refused for configuring one audio pair twice carries:
/// the pair the request named more than once, and the rule the member's own
/// documentation states ("An array of eight audio configurations, one for each
/// audio pair in the source"). Built from the request, so it names what the
/// caller must change.
///
/// Worded as the other audio-pair refusals of
/// [`configurable_settings_error`] are, so the member's bounds cannot be
/// reported three ways.
pub fn repeated_audio_channel_pair_message(id: i32) -> String {
    format!(
        "AudioChannelPairs names audio pair {id} more than once; a source carries one audio \
         configuration for each audio pair"
    )
}

/// Why `settings` cannot configure a device of type `device_type`, worded for a
/// refusal to report, or `None` when they can.
///
/// The rule lives in the slice rather than in the handler, so the bounds the
/// settings' own documentation states are checked in one place: the members only
/// a UHD device has ([`uhd_only_member`]), the audio pairs a source has
/// ([`MAX_AUDIO_CHANNEL_PAIRS`], [`FIRST_AUDIO_CHANNEL_PAIR_ID`]) and the one
/// configuration each of those pairs carries
/// ([`repeated_audio_channel_pair_message`]), and the resolutions a Link source
/// can be set to ([`input_resolution`]). Built from the request, so each message
/// names what the caller must change.
pub fn configurable_settings_error(
    device_type: &str,
    settings: &InputDeviceConfigurableSettings,
) -> Option<String> {
    if device_type != input_device_type::UHD {
        if let Some(member) = uhd_only_member(settings) {
            return Some(format!(
                "{member} applies only to an input device whose Type is {}",
                input_device_type::UHD
            ));
        }
    }
    if let Some(pairs) = &settings.audio_channel_pairs {
        if pairs.len() > MAX_AUDIO_CHANNEL_PAIRS {
            return Some(format!(
                "AudioChannelPairs carries {} configurations; a source has {MAX_AUDIO_CHANNEL_PAIRS} audio pairs",
                pairs.len()
            ));
        }
        // The list configures the pairs of ONE source, "one for each audio pair
        // in the source", so each pair it names is a pair it configures once: a
        // second configuration for a pair the list already names leaves that
        // pair's profile undecided, which is not a configuration the member
        // describes.
        let mut configured: Vec<i32> = Vec::with_capacity(pairs.len());
        for id in pairs.iter().filter_map(|pair| pair.id) {
            if !(FIRST_AUDIO_CHANNEL_PAIR_ID..=LAST_AUDIO_CHANNEL_PAIR_ID).contains(&id) {
                return Some(format!(
                    "AudioChannelPairs names audio pair {id}; an audio pair id is a value from \
                     {FIRST_AUDIO_CHANNEL_PAIR_ID} to {LAST_AUDIO_CHANNEL_PAIR_ID}"
                ));
            }
            if configured.contains(&id) {
                return Some(repeated_audio_channel_pair_message(id));
            }
            configured.push(id);
        }
    }
    if let Some(resolution) = &settings.input_resolution {
        if !input_resolution::ALL.contains(&resolution.as_str()) {
            return Some(format!(
                "InputResolution is one of {}, got {resolution}",
                input_resolution::ALL.join(", ")
            ));
        }
    }
    None
}

/// The message an update refused for naming the settings of a device type the
/// device is not carries: the member the request named, the device and the type
/// it actually is. Built from the request, so it names what the caller must
/// change.
///
/// Each settings member names the type it configures — `HdDeviceSettings` is
/// "The settings that you want to apply to the HD input device" and
/// `UhdDeviceSettings` "to the UHD input device" — so the other member names no
/// settings this device has.
pub fn settings_member_mismatch_message(member: &str, id: &str, device_type: &str) -> String {
    format!("{member} does not apply to input device {id}, whose Type is {device_type}")
}

/// One claimed input device.
///
/// The members mirror the model's `InputDeviceSummary` shape minus the ones that
/// are derived or that this emulator has no device to read them off (see the
/// module docs): `DeviceUpdateStatus` comes out of the maintenance window below,
/// and `OutputType` and `MedialiveInputArns` out of the inputs this store holds.
#[derive(Debug, Clone)]
pub struct InputDeviceRecord {
    /// The device id the claim named, which is also the key it is stored under.
    pub id: String,
    pub arn: String,
    /// The name an update gives the device ("A name that you specify for the
    /// input device"), absent until one does.
    pub name: Option<String>,
    /// The Availability Zone an update associates with the device ("The
    /// Availability Zone you want associated with this input device"), absent
    /// until one does. MediaLive's own record of where the device's attachment
    /// lives rather than something read off the unit, which is why an update is
    /// what sets it.
    pub availability_zone: Option<String>,
    /// The device's `InputDeviceType`, from the id it was claimed with
    /// ([`input_device_type_for_id`]).
    pub r#type: String,
    /// The device's `InputDeviceConnectionState`.
    pub connection_state: String,
    /// The configuration an update has applied to the device, which is what its
    /// own type's settings report beside the run state.
    ///
    /// PRIVATE, so the only way in is
    /// [`InputDeviceRecord::apply_configuration`] and the only way to read it is
    /// [`InputDeviceRecord::configuration`].
    configuration: InputDeviceConfiguration,
    /// The instant the device has taken the configuration an update sent it, or
    /// `None` while it holds no unsynchronised change. Stored, never slept on.
    ///
    /// PRIVATE, so the only way in is
    /// [`InputDeviceRecord::apply_configuration`] and
    /// [`InputDeviceRecord::settle`], and the only way to read it is
    /// [`InputDeviceRecord::device_settings_sync_state`].
    settings_sync_due: Option<SystemTime>,
    /// The device's own tag map, which is the one the tag operations reach
    /// through [`ComAmazonawsMedialiveMedialiveState::resource_tags`]: there is
    /// no second copy for a read to drift from.
    pub tags: HashMap<String, String>,
    /// The instant the open maintenance window closes, or `None` while none is
    /// open. Stored, never slept on.
    ///
    /// PRIVATE, so the only way in is
    /// [`InputDeviceRecord::begin_maintenance_window`] and
    /// [`InputDeviceRecord::settle`], and the only way to read it is
    /// [`InputDeviceRecord::device_update_status`].
    maintenance_window_close: Option<SystemTime>,
    /// The transfer pending on this device, or `None` while none is.
    ///
    /// PRIVATE, so the only way in is
    /// [`InputDeviceRecord::begin_transfer`] and the only way to read it is
    /// [`InputDeviceRecord::pending_transfer`].
    transfer: Option<InputDeviceTransferRecord>,
    /// Whether the device is producing video right now, which is what its
    /// `InputDeviceState` reports.
    ///
    /// PRIVATE, so the only way in is
    /// [`InputDeviceRecord::start_streaming`],
    /// [`InputDeviceRecord::stop_streaming`] and
    /// [`InputDeviceRecord::begin_reboot`], and the only way to read it is
    /// [`InputDeviceRecord::device_state`].
    streaming: bool,
    /// The instant the reboot this device is going through completes, or `None`
    /// while no reboot is pending on it. Stored, never slept on.
    ///
    /// PRIVATE, so the only way in is [`InputDeviceRecord::begin_reboot`] and
    /// [`InputDeviceRecord::settle`], and the only way to read it is
    /// [`InputDeviceRecord::is_rebooting`].
    reboot_complete: Option<SystemTime>,
    /// Whether the device resumes producing video when that reboot completes,
    /// which a FORCED reboot of a streaming device is what sets.
    ///
    /// PRIVATE, for the same reason, and cleared by a start or a stop: an
    /// explicit command is the device's run state from then on.
    resumes_streaming_after_reboot: bool,
    /// The latest thumbnail this device has sent, or `None` while it has sent
    /// none.
    ///
    /// PRIVATE, so the only way in is
    /// [`ComAmazonawsMedialiveMedialiveState::record_input_device_thumbnail`]
    /// and the only way to read it is [`InputDeviceRecord::thumbnail`].
    thumbnail: Option<InputDeviceThumbnailRecord>,
}

impl InputDeviceRecord {
    /// Whether a maintenance window is open on this device right now.
    pub fn is_in_maintenance_window(&self) -> bool {
        self.maintenance_window_close.is_some()
    }

    /// The device's `DeviceUpdateStatus` ("The status of software on the input
    /// device"): `UPDATING` exactly while a maintenance window is open — the
    /// window is what "give[s] the device up to two hours to install software" —
    /// and `UP_TO_DATE` otherwise, since this emulator holds no software update
    /// for a device.
    pub fn device_update_status(&self) -> &'static str {
        if self.is_in_maintenance_window() {
            device_update_status::UPDATING
        } else {
            device_update_status::UP_TO_DATE
        }
    }

    /// The configuration an update has applied to this device.
    pub fn configuration(&self) -> &InputDeviceConfiguration {
        &self.configuration
    }

    /// Of the two settings members an `UpdateInputDevice` request carries, the
    /// one that names THIS device's settings, beside the name of the other one
    /// when the request named it too.
    ///
    /// Each member names the type it configures — `HdDeviceSettings` is "The
    /// settings that you want to apply to the HD input device" and
    /// `UhdDeviceSettings` "to the UHD input device" — so the member for the
    /// other type names no settings this device has, and the caller refuses it.
    /// The choice lives here, with the type the device was claimed with, so no
    /// operation decides it a second way.
    pub fn settings_of_this_type(
        &self,
        hd_device_settings: Option<InputDeviceConfigurableSettings>,
        uhd_device_settings: Option<InputDeviceConfigurableSettings>,
    ) -> (Option<InputDeviceConfigurableSettings>, Option<&'static str>) {
        if self.r#type == input_device_type::UHD {
            (
                uhd_device_settings,
                hd_device_settings.map(|_| "HdDeviceSettings"),
            )
        } else {
            (
                hd_device_settings,
                uhd_device_settings.map(|_| "UhdDeviceSettings"),
            )
        }
    }

    /// The device's `DeviceSettingsSyncState` ("The status of the action to
    /// synchronize the device configuration"): `SYNCING` while the device has
    /// not taken the configuration an update sent it — "If you change the
    /// configuration of the input device (for example, the maximum bitrate),
    /// MediaLive sends the new data to the device. The device might not update
    /// itself immediately" — and `SYNCED` once it has, which is also the state
    /// of a device no update has configured.
    pub fn device_settings_sync_state(&self) -> &'static str {
        if self.settings_sync_due.is_some() {
            device_settings_sync_state::SYNCING
        } else {
            device_settings_sync_state::SYNCED
        }
    }

    /// Apply the configuration one update carried to this device, which is then
    /// `SYNCING` until the clock makes the synchronisation due
    /// ([`TRANSIENT_STATE_SPAN`], the emulator's own span for the same reason as
    /// every other transient state here: AWS documents no duration for a device
    /// taking a configuration). The instant is stored and never slept on, and it
    /// is deliberately non-zero, so a caller that reads the device straight
    /// after the update still observes the `SYNCING` the change put it in.
    ///
    /// A later update restarts it: each change is sent to the device when it is
    /// made, so the device is synchronised a span after the LATEST one.
    pub fn apply_configuration(
        &mut self,
        settings: InputDeviceConfigurableSettings,
        now: SystemTime,
    ) {
        self.configuration.apply(settings);
        self.settings_sync_due = Some(now + TRANSIENT_STATE_SPAN);
    }

    /// Open a maintenance window on this device, which the clock closes once
    /// [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// IDEMPOTENT: a window already open keeps the instant it was opened with,
    /// so a repeated start neither extends it nor restarts the clock.
    ///
    /// The span is the emulator's own, as it is for every other transient state
    /// here: AWS documents only a BOUND on the install the window allows
    /// ("Starting a maintenance window will give the device up to two hours to
    /// install software"), never how long the device stays in `UPDATING`, and a
    /// span that long would leave every device of a test run stuck there. It is
    /// deliberately non-zero, so a caller that reads the device straight after
    /// the call still observes the `UPDATING` the window put it in.
    pub fn begin_maintenance_window(&mut self, now: SystemTime) {
        if self.is_in_maintenance_window() {
            return;
        }
        self.maintenance_window_close = Some(now + TRANSIENT_STATE_SPAN);
    }

    /// Close the maintenance window, complete the configuration
    /// synchronisation, and finish the reboot, the clock has already made due.
    /// Each is owed at its stored instant whether or not anybody called, so
    /// applying them in a reporting operation changes nothing a later caller
    /// would have seen.
    fn settle(&mut self, now: SystemTime) {
        if self
            .maintenance_window_close
            .is_some_and(|close| now >= close)
        {
            self.maintenance_window_close = None;
        }
        if self.settings_sync_due.is_some_and(|due| now >= due) {
            self.settings_sync_due = None;
        }
        if self.reboot_complete.is_some_and(|complete| now >= complete) {
            self.reboot_complete = None;
            // A reboot that interrupted a stream it was forced to interrupt is
            // the one that resumes it: "the device will resume streaming when
            // the reboot completes". Taken rather than read, so the resumption
            // is owed once, by the reboot that recorded it.
            if ::std::mem::take(&mut self.resumes_streaming_after_reboot) {
                self.streaming = true;
            }
        }
    }

    /// Whether the device is producing video right now.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// The device's `InputDeviceState` ("The state of the input device"):
    /// `STREAMING` while the device is producing video and `IDLE` while it is
    /// not. A device nothing has started is `IDLE`, which is the state of a Link
    /// unit that has been claimed and told to do nothing yet.
    pub fn device_state(&self) -> &'static str {
        if self.is_streaming() {
            input_device_state::STREAMING
        } else {
            input_device_state::IDLE
        }
    }

    /// Put the device in `STREAMING`: it is producing video from now until a
    /// stop ends it.
    ///
    /// IDEMPOTENT — a device already streaming is left streaming, because a
    /// start asks for exactly the state it is already in. There is no transient
    /// state between the two: `InputDeviceState` declares only `IDLE` and
    /// `STREAMING`, so the clock owes this device nothing.
    ///
    /// It cancels the resumption a pending reboot owed the device: an explicit
    /// command is the device's run state from then on, so the reboot does not
    /// move it again when it completes.
    pub fn start_streaming(&mut self) {
        self.streaming = true;
        self.resumes_streaming_after_reboot = false;
    }

    /// Put the device in `IDLE`: it is no longer producing video.
    ///
    /// IDEMPOTENT, and cancelling a pending reboot's resumption, for the same
    /// reasons [`InputDeviceRecord::start_streaming`] is and does.
    pub fn stop_streaming(&mut self) {
        self.streaming = false;
        self.resumes_streaming_after_reboot = false;
    }

    /// Whether a reboot is pending on this device right now.
    pub fn is_rebooting(&self) -> bool {
        self.reboot_complete.is_some()
    }

    /// Send the device a reboot command, which the clock completes once
    /// [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// A rebooting unit is running no video, so the reboot stops the device
    /// producing it — "[i]f the device is streaming, it will stop streaming and
    /// begin rebooting" — and `force` is what decides where it comes back: a
    /// forced reboot of a streaming device resumes that stream when it
    /// completes ("[i]f the device was streaming prior to the reboot, the device
    /// will resume streaming when the reboot completes"), while a reboot nobody
    /// forced leaves the device idle for a start to move again.
    ///
    /// The span is the emulator's own, as it is for every other transient state
    /// here: AWS documents only that "[t]he device will begin rebooting within a
    /// few seconds of sending the command", never how long a reboot takes. It is
    /// deliberately non-zero, so a caller that reads the device straight after
    /// the call still observes the device the reboot took off the air.
    ///
    /// A reboot while one is pending is the LATEST command: it restarts the
    /// span, and resumes the stream afterwards only if the device was producing
    /// video when it arrived — which a device the earlier reboot already took
    /// off the air was not.
    pub fn begin_reboot(&mut self, force: bool, now: SystemTime) {
        self.resumes_streaming_after_reboot = force && self.is_streaming();
        self.streaming = false;
        self.reboot_complete = Some(now + TRANSIENT_STATE_SPAN);
    }

    /// The transfer pending on this device, or `None` while none is.
    pub fn pending_transfer(&self) -> Option<&InputDeviceTransferRecord> {
        self.transfer.as_ref()
    }

    /// Whether a transfer is pending on this device right now.
    pub fn is_transferring(&self) -> bool {
        self.transfer.is_some()
    }

    /// Record `transfer` as the transfer pending on this device.
    ///
    /// The caller resolves the conflict first: a device a transfer is already
    /// pending on is not transferred again ("the other account must accept or
    /// reject the transfer" before anything else happens to it), which is the
    /// operation's own modelled `ConflictException`.
    pub fn begin_transfer(&mut self, transfer: InputDeviceTransferRecord) {
        self.transfer = Some(transfer);
    }

    /// Accept the transfer pending on this device on behalf of the account this
    /// emulator serves, and report what that resolved to.
    ///
    /// Only an INCOMING transfer is accepted — "the ownership of the device will
    /// transfer to your AWS account" is what the operation does, and a transfer
    /// addressed to another account is not waiting on this one. Which transfers
    /// are incoming is the slice's one definition of the direction
    /// ([`transfer_has_direction`]), the same one the listing filters by.
    /// Accepting is what ENDS the transfer: the device is the recipient's from
    /// then on, so nothing is pending on it any more and a second accept has
    /// nothing left to resolve.
    pub fn accept_transfer(&mut self) -> TransferAcceptance {
        match self.end_incoming_transfer() {
            Ok(()) => TransferAcceptance::Accepted,
            Err(IncomingTransferRefusal::NotBeingTransferred) => {
                TransferAcceptance::NotBeingTransferred
            }
            Err(IncomingTransferRefusal::NotTheRecipient(recipient)) => {
                TransferAcceptance::NotTheRecipient(recipient)
            }
        }
    }

    /// Reject the transfer pending on this device on behalf of the account this
    /// emulator serves, and report what that resolved to.
    ///
    /// The mirror of [`InputDeviceRecord::accept_transfer`]: "[r]eject the
    /// transfer of the specified input device to your AWS account" is the other
    /// decision the SAME pending transfer waits on, so it is open to the same
    /// account — the recipient the transfer names — through the same rule
    /// ([`InputDeviceRecord::end_incoming_transfer`]). Rejecting is what ENDS
    /// the transfer: the device never changes hands, so it stays the sender's
    /// with nothing pending on it, and a second reject has nothing left to
    /// resolve.
    pub fn reject_transfer(&mut self) -> TransferRejection {
        match self.end_incoming_transfer() {
            Ok(()) => TransferRejection::Rejected,
            Err(IncomingTransferRefusal::NotBeingTransferred) => {
                TransferRejection::NotBeingTransferred
            }
            Err(IncomingTransferRefusal::NotTheRecipient(recipient)) => {
                TransferRejection::NotTheRecipient(recipient)
            }
        }
    }

    /// End the INCOMING transfer pending on this device, whatever decision the
    /// recipient took on it, or report why no decision could be taken.
    ///
    /// The slice's ONE rule for resolving a transfer from the recipient's side,
    /// so accepting and rejecting cannot disagree about which transfer the
    /// recipient may decide: only an incoming one ([`transfer_has_direction`],
    /// the same definition the listing filters by), and resolving it is what
    /// clears it off the device.
    fn end_incoming_transfer(&mut self) -> Result<(), IncomingTransferRefusal> {
        let Some(transfer) = &self.transfer else {
            return Err(IncomingTransferRefusal::NotBeingTransferred);
        };
        if !transfer_has_direction(transfer, input_device_transfer_type::INCOMING) {
            return Err(IncomingTransferRefusal::NotTheRecipient(transfer_recipient(
                transfer.target_customer_id.as_deref(),
            )));
        }
        self.transfer = None;
        Ok(())
    }

    /// Cancel the transfer pending on this device on behalf of the account that
    /// REQUESTED it, and report what that resolved to.
    ///
    /// "Cancel an input device transfer that you have requested" is what the
    /// operation does, so the transfer a cancel ends is one this account SENT —
    /// the direction the model calls "an input device that you are transferring
    /// to another AWS account", read through the slice's one definition of a
    /// transfer's direction ([`transfer_has_direction`]) rather than a second
    /// rule beside it. Cancelling is what ENDS the transfer: nothing waits on
    /// the recipient's decision any more, so the device stays this account's and
    /// can be transferred again, and a second cancel has nothing left to end.
    pub fn cancel_transfer(&mut self) -> TransferCancellation {
        let requested_here = self.transfer.as_ref().is_some_and(|transfer| {
            transfer_has_direction(transfer, input_device_transfer_type::OUTGOING)
        });
        if !requested_here {
            return TransferCancellation::NotBeingTransferred;
        }
        self.transfer = None;
        TransferCancellation::Cancelled
    }

    /// The latest thumbnail this device has sent, or `None` while it has sent
    /// none — which is every device of this emulator, because no Link unit is
    /// connected to it to send one (see the module docs).
    pub fn thumbnail(&self) -> Option<&InputDeviceThumbnailRecord> {
        self.thumbnail.as_ref()
    }
}

/// A freshly claimed device: the id the claim named, the ARN built from it, and
/// the state of a Link unit nothing has configured or connected yet.
pub fn new_input_device(id: String, arn: String, device_type: &str) -> InputDeviceRecord {
    InputDeviceRecord {
        id,
        arn,
        name: None,
        availability_zone: None,
        r#type: device_type.to_string(),
        connection_state: input_device_connection_state::DISCONNECTED.to_string(),
        // Nothing has configured a freshly claimed device, so it holds no
        // unsynchronised change either.
        configuration: InputDeviceConfiguration::default(),
        settings_sync_due: None,
        tags: HashMap::new(),
        maintenance_window_close: None,
        transfer: None,
        // A claimed device is producing nothing until a start says so, and
        // nothing has told it to reboot.
        streaming: false,
        reboot_complete: None,
        resumes_streaming_after_reboot: false,
        // And it has sent no image until it sends one.
        thumbnail: None,
    }
}

/// What accepting a device's pending transfer resolved to
/// ([`InputDeviceRecord::accept_transfer`]).
///
/// The rule lives on the store rather than in the handler, so the operations
/// that resolve a transfer cannot disagree about which transfer an account may
/// accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferAcceptance {
    /// The transfer was incoming and is now complete: ownership of the device is
    /// the recipient's — the account this emulator serves — and nothing is
    /// pending on it any more.
    Accepted,
    /// No transfer is pending on this device, so there is none to accept.
    NotBeingTransferred,
    /// The pending transfer names a different recipient, so the account asking
    /// is its sender rather than the account it waits on. Carries the recipient
    /// ALREADY WORDED for a refusal to report ([`transfer_recipient`]), so no
    /// caller can word it a second way.
    NotTheRecipient(String),
}

/// What rejecting a device's pending transfer resolved to
/// ([`InputDeviceRecord::reject_transfer`]).
///
/// Beside [`TransferAcceptance`] rather than shared with it: the two decisions
/// read the same transfer through the same rule, but they are different
/// decisions, and each operation reports its own in its own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferRejection {
    /// The transfer was incoming and is now over, refused: the device stays the
    /// SENDER's and nothing is pending on it any more.
    Rejected,
    /// No transfer is pending on this device, so there is none to reject.
    NotBeingTransferred,
    /// The pending transfer names a different recipient, so the account asking
    /// is its sender rather than the account it waits on. Carries the recipient
    /// ALREADY WORDED for a refusal to report ([`transfer_recipient`]), for the
    /// same reason [`TransferAcceptance::NotTheRecipient`] does.
    NotTheRecipient(String),
}

/// Why the recipient's decision on a pending transfer could not be taken
/// ([`InputDeviceRecord::end_incoming_transfer`]).
///
/// Private to the slice: a caller reads the refusal through the
/// [`TransferAcceptance`] or [`TransferRejection`] its own operation reports.
enum IncomingTransferRefusal {
    /// No transfer is pending on the device at all.
    NotBeingTransferred,
    /// A transfer is pending, but it names another account as its recipient.
    NotTheRecipient(String),
}

/// What cancelling a device's pending transfer resolved to
/// ([`InputDeviceRecord::cancel_transfer`]).
///
/// On the store beside [`TransferAcceptance`], for the same reason: the rule for
/// which transfer a cancel may end belongs to the slice that holds the
/// transfers, not to the handler that calls it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferCancellation {
    /// The transfer this account had requested is cancelled: nothing is pending
    /// on the device any more and it is still this account's.
    Cancelled,
    /// There is no transfer of this device for this account to cancel.
    NotBeingTransferred,
}

/// How a refusal about a pending transfer names the recipient that transfer is
/// waiting on: the account id it named, or that it named none (the
/// `TargetCustomerId` member is optional in the model).
///
/// ONE helper, so the operations that refuse something about a pending transfer
/// — `AcceptInputDeviceTransfer` and `RejectInputDeviceTransfer` — cannot word
/// its recipient two ways. Private to the slice: every caller reads it through
/// the [`TransferAcceptance::NotTheRecipient`] or
/// [`TransferRejection::NotTheRecipient`] it is carried in.
fn transfer_recipient(target_customer_id: Option<&str>) -> String {
    target_customer_id.map_or_else(
        || "no account".to_string(),
        |target| format!("account {target}"),
    )
}

/// Whether the pending transfer `transfer` has the direction `transfer_type`
/// names, read from THIS account's side.
///
/// `OUTGOING` is "an input device that you are transferring to another AWS
/// account", which is every transfer this store holds: `TransferInputDevice` is
/// the only thing that records one and it transfers a device this account owns.
/// `INCOMING` is "an input device that is being transferred to you", so it is a
/// transfer whose RECIPIENT is this account — this emulator serves the single
/// account `engine::emulator_account_id()`, so a transfer addressed to that
/// account id is one being transferred to you as much as it is one you sent.
/// Any other spelling names no direction at all.
///
/// A transfer addressed to this emulator's own account is therefore reported by
/// BOTH directions, which is the consequence of serving one account: on real
/// AWS the sender and the recipient are different accounts, and each sees the
/// one transfer from its own side only. Reporting it from both sides is the
/// faithful reading available here — the alternative, calling a transfer this
/// account started `INCOMING` only, would hide it from the sender that started
/// it.
fn transfer_has_direction(transfer: &InputDeviceTransferRecord, transfer_type: &str) -> bool {
    match transfer_type {
        input_device_transfer_type::OUTGOING => true,
        input_device_transfer_type::INCOMING => {
            transfer.target_customer_id.as_deref() == Some(::engine::emulator_account_id())
        }
        _ => false,
    }
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every device transition the clock has already made due: the
    /// maintenance windows it has closed, the configuration changes it has
    /// synchronised and the reboots it has completed.
    pub(super) fn settle_input_devices(&mut self, now: SystemTime) {
        for device in self.input_devices.values_mut() {
            device.settle(now);
        }
    }

    /// Whether the device store is full, so `ClaimDevice` must refuse rather
    /// than register another device.
    pub fn input_devices_are_full(&self) -> bool {
        self.input_devices.len() >= MAX_INPUT_DEVICES
    }

    /// Register the device `id`, with the ARN and `InputDeviceType` the claim
    /// resolved for it, and report whether this store now holds it.
    ///
    /// The registration rule lives here rather than in the handler, so the two
    /// halves of it cannot drift from the store they apply to:
    ///
    /// * IDEMPOTENT — a device this account already owns is left exactly as it
    ///   is (its name, its tags and any open maintenance window), because the
    ///   claim's promise, that "after the request succeeds, you will own the
    ///   device", already holds for it;
    /// * BOUNDED — a device the store has no room for is not registered, and
    ///   `false` is what the claim answers its modelled throttle to.
    pub fn claim_input_device(&mut self, id: String, arn: String, device_type: &str) -> bool {
        if self.input_devices.contains_key(&id) {
            return true;
        }
        if self.input_devices_are_full() {
            return false;
        }
        let device = new_input_device(id.clone(), arn, device_type);
        self.input_devices.insert(id, device);
        true
    }

    /// The `TransferringInputDeviceSummary` projection of the transfer pending
    /// on the device `id`, as the direction `transfer_type` reads it, or `None`
    /// when this account has claimed no device with that id, when no transfer is
    /// pending on it, or when the pending transfer is not of that direction.
    ///
    /// The one projection of a pending transfer, so every operation that reports
    /// one reads the same record the same way — and the one place the direction
    /// filter is applied, so what a listing reports and what it calls the
    /// transfer cannot disagree.
    pub fn input_device_transfer_summary(
        &self,
        id: &str,
        transfer_type: &str,
    ) -> Option<TransferringInputDeviceSummary> {
        let device = self.input_devices.get(id)?;
        let transfer = device.pending_transfer()?;
        if !transfer_has_direction(transfer, transfer_type) {
            return None;
        }
        Some(TransferringInputDeviceSummary {
            id: Some(device.id.clone()),
            message: transfer.message.clone(),
            target_customer_id: transfer.target_customer_id.clone(),
            // The direction the READER asked for, which is the side the
            // transfer was reported to: the same transfer is outgoing for its
            // sender and incoming for its recipient.
            transfer_type: Some(transfer_type.to_string()),
        })
    }

    /// The device this account owns under `id`, to write through, or `None` when
    /// no claim has registered that id.
    ///
    /// Every MUTATION of a device resolves its id here, so an id this account
    /// has not claimed is the operation's own not-found rather than a device
    /// conjured out of the request.
    pub fn input_device_mut(&mut self, id: &str) -> Option<&mut InputDeviceRecord> {
        self.input_devices.get_mut(id)
    }

    /// The device this account owns under `id`, to read, or `None` when no claim
    /// has registered that id.
    ///
    /// The read side of [`ComAmazonawsMedialiveMedialiveState::input_device_mut`]:
    /// a reader that needs the RECORD rather than its
    /// [`InputDeviceSummary`] projection (the thumbnail read, which reports the
    /// image the device sent rather than the device) resolves its id here, so an
    /// id this account has not claimed is the operation's own not-found.
    pub fn input_device(&self, id: &str) -> Option<&InputDeviceRecord> {
        self.input_devices.get(id)
    }

    /// Record `thumbnail` as the latest thumbnail of the device `id`, answering
    /// whether a device with that id took it.
    ///
    /// REPLACING: a device sends one thumbnail after another and the read reports
    /// "the thumbnail that the Link device has most recently sent", so the latest
    /// one is what the record keeps.
    ///
    /// It refuses, answering `false`, what the service could not report back: a
    /// device this account has not claimed, a media type outside the one the
    /// model's `ContentType` enum declares (a response member may only carry a
    /// value the model declares), and an image over
    /// [`MAX_INPUT_DEVICE_THUMBNAIL_BYTES`].
    ///
    /// The real service receives these FROM the connected Link unit; no unit is
    /// connected to this emulator, so no operation in this service calls this and
    /// a device claimed through the API has no thumbnail (see the module docs). It
    /// is the ONE way a thumbnail reaches a device, so
    /// `DescribeInputDeviceThumbnail` reports what was sent rather than anything
    /// it invented.
    pub fn record_input_device_thumbnail(
        &mut self,
        id: &str,
        thumbnail: InputDeviceThumbnailRecord,
    ) -> bool {
        if !CONTENT_TYPE_VALUES.contains(&thumbnail.content_type.as_str())
            || thumbnail.body.len() > MAX_INPUT_DEVICE_THUMBNAIL_BYTES
        {
            return false;
        }
        let Some(device) = self.input_devices.get_mut(id) else {
            return false;
        };
        device.thumbnail = Some(thumbnail);
        true
    }

    /// The ARNs of the inputs currently ingesting from the device `id` —
    /// `MedialiveInputArns`, "An array of the ARNs for the MediaLive inputs
    /// attached to the device".
    ///
    /// Derived from the input store on every read, so it tracks every later
    /// create, update and delete rather than being recorded when the device was
    /// claimed. An input whose delete this service has ACCEPTED is attached no
    /// longer, which is the one cut point every derived "currently using" list
    /// of this store applies
    /// ([`ComAmazonawsMedialiveMedialiveState::inputs_using_security_group`]):
    /// the input is being torn down, so the device stops reporting it at that
    /// instant rather than one transient span later.
    fn inputs_using_input_device(&self, id: &str) -> Vec<String> {
        self.inputs
            .values()
            .filter(|input| !input.is_delete_accepted())
            .filter(|input| {
                input
                    .input_devices
                    .iter()
                    .any(|device| device.id.as_deref() == Some(id))
            })
            .map(|input| input.arn.clone())
            .collect()
    }

    /// The `InputDeviceSummary` projection of one claimed device, or `None` when
    /// this account has claimed no device with that id.
    ///
    /// Resolution only: the projection itself is
    /// [`ComAmazonawsMedialiveMedialiveState::input_device_summary_of`], so a
    /// reader that already holds the record (the listing, walking the store) and
    /// one that only holds an id (`DescribeInputDevice`) cannot report a device
    /// differently.
    pub fn input_device_summary(&self, id: &str) -> Option<InputDeviceSummary> {
        Some(self.input_device_summary_of(self.input_devices.get(id)?))
    }

    /// The `InputDeviceSummary` projection of one claimed device record.
    ///
    /// The ONE projection of a device, so every operation that reports one
    /// (`DescribeInputDevice` through [`input_device_output`], and
    /// `ListInputDevices`) reads the same record the same way.
    pub fn input_device_summary_of(&self, device: &InputDeviceRecord) -> InputDeviceSummary {
        let id = device.id.as_str();
        let medialive_input_arns = self.inputs_using_input_device(id);
        let configuration = device.configuration();
        // "The output attachment type of the input device. Specifies
        // MEDIACONNECT_FLOW if this device is the source for a MediaConnect
        // flow. Specifies MEDIALIVE_INPUT if this device is the source for a
        // MediaLive input": a device an update attached to a flow is that
        // flow's source, a device some input ingests from is that input's
        // source, and one attached to neither is attached to nothing.
        let output_type = if configuration.mediaconnect.is_some() {
            input_device_output_type::MEDIACONNECT_FLOW
        } else if medialive_input_arns.is_empty() {
            input_device_output_type::NONE
        } else {
            input_device_output_type::MEDIALIVE_INPUT
        };
        // The device's run state is the one member of a device's settings this
        // service knows without a unit to read the rest off: it is the state the
        // start and stop commands the service accepted put the device in, not a
        // characteristic of the video a connected unit is producing. It is
        // reported in the settings of the device's own TYPE, which is what each
        // member documents ("Settings that describe an input device that is type
        // HD" / "... type UHD"), beside the configuration an update applied;
        // every remaining member is measured off the unit, and no unit is
        // connected to this emulator.
        let device_state = Some(device.device_state().to_string());
        let is_hd = device.r#type == input_device_type::HD;
        InputDeviceSummary {
            arn: Some(device.arn.clone()),
            connection_state: Some(device.connection_state.clone()),
            device_settings_sync_state: Some(device.device_settings_sync_state().to_string()),
            device_update_status: Some(device.device_update_status().to_string()),
            hd_device_settings: is_hd.then(|| InputDeviceHdSettings {
                configured_input: configuration.configured_input.clone(),
                device_state: device_state.clone(),
                max_bitrate: configuration.max_bitrate,
                latency_ms: configuration.latency_ms,
                ..InputDeviceHdSettings::default()
            }),
            uhd_device_settings: (!is_hd).then(|| InputDeviceUhdSettings {
                configured_input: configuration.configured_input.clone(),
                device_state,
                max_bitrate: configuration.max_bitrate,
                latency_ms: configuration.latency_ms,
                codec: configuration.codec.clone(),
                // "Returned only if the outputType is MEDIACONNECT_FLOW", which
                // is exactly when a flow is attached.
                mediaconnect_settings: configuration.mediaconnect.clone(),
                audio_channel_pairs: configuration.audio_channel_pairs.clone(),
                input_resolution: configuration.input_resolution.clone(),
                ..InputDeviceUhdSettings::default()
            }),
            id: Some(device.id.clone()),
            // Hardware properties of a unit this emulator does not have.
            mac_address: None,
            serial_number: None,
            network_settings: None,
            availability_zone: device.availability_zone.clone(),
            name: device.name.clone(),
            r#type: Some(device.r#type.clone()),
            tags: Some(device.tags.clone()),
            // "Returned only if the outputType is MEDIALIVE_INPUT."
            medialive_input_arns: (output_type == input_device_output_type::MEDIALIVE_INPUT)
                .then_some(medialive_input_arns),
            output_type: Some(output_type.to_string()),
        }
    }
}

/// Project an `InputDeviceSummary` onto one of the structurally identical
/// per-operation FLAT input-device output shapes (`DescribeInputDeviceOutput`
/// and `UpdateInputDeviceOutput`; the rest when their own issues land).
///
/// One macro rather than a hand-written copy of the same 16-member move per
/// operation, so the operations that report a device cannot drift from each
/// other.
macro_rules! input_device_output {
    ($out:ident, $device:expr) => {{
        let device: $crate::ops::types::InputDeviceSummary = $device;
        $out {
            arn: device.arn,
            connection_state: device.connection_state,
            device_settings_sync_state: device.device_settings_sync_state,
            device_update_status: device.device_update_status,
            hd_device_settings: device.hd_device_settings,
            id: device.id,
            mac_address: device.mac_address,
            name: device.name,
            network_settings: device.network_settings,
            serial_number: device.serial_number,
            r#type: device.r#type,
            uhd_device_settings: device.uhd_device_settings,
            tags: device.tags,
            availability_zone: device.availability_zone,
            medialive_input_arns: device.medialive_input_arns,
            output_type: device.output_type,
        }
    }};
}

pub(crate) use input_device_output;
