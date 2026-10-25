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
//! A device's MAINTENANCE WINDOW is one stored instant on the record: the
//! moment the window the clock owes closes. `StartInputDeviceMaintenanceWindow`
//! opens it, which is what puts the device's `DeviceUpdateStatus` in `UPDATING`,
//! and [`ComAmazonawsMedialiveMedialiveState::settle`] — which every operation
//! runs first — is what closes it once it comes due, back to `UP_TO_DATE`. The
//! instant is stored and never slept on.
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
    InputDeviceHdSettings, InputDeviceSummary, InputDeviceUhdSettings,
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
/// store holds no pending configuration change for.
pub mod device_settings_sync_state {
    pub const SYNCED: &str = "SYNCED";
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
/// MEDIACONNECT_FLOW). `MEDIACONNECT_FLOW` is not among them: it reports a
/// MediaConnect flow the real service provisions for a UHD device, which this
/// emulator does not provision.
pub mod input_device_output_type {
    pub const NONE: &str = "NONE";
    pub const MEDIALIVE_INPUT: &str = "MEDIALIVE_INPUT";
}

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
    /// The device's `InputDeviceType`, from the id it was claimed with
    /// ([`input_device_type_for_id`]).
    pub r#type: String,
    /// The device's `InputDeviceConnectionState`.
    pub connection_state: String,
    /// The device's `DeviceSettingsSyncState`.
    pub device_settings_sync_state: String,
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
    /// [`InputDeviceRecord::start_streaming`] and
    /// [`InputDeviceRecord::stop_streaming`], and the only way to read it is
    /// [`InputDeviceRecord::device_state`].
    streaming: bool,
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

    /// Close the maintenance window the clock has already made due. Owed at the
    /// stored instant whether or not anybody called, so applying it in a
    /// reporting operation changes nothing a later caller would have seen.
    fn settle(&mut self, now: SystemTime) {
        if self
            .maintenance_window_close
            .is_some_and(|close| now >= close)
        {
            self.maintenance_window_close = None;
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
    pub fn start_streaming(&mut self) {
        self.streaming = true;
    }

    /// Put the device in `IDLE`: it is no longer producing video.
    ///
    /// IDEMPOTENT, for the same reason [`InputDeviceRecord::start_streaming`]
    /// is.
    pub fn stop_streaming(&mut self) {
        self.streaming = false;
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
        let Some(transfer) = &self.transfer else {
            return TransferAcceptance::NotBeingTransferred;
        };
        if !transfer_has_direction(transfer, input_device_transfer_type::INCOMING) {
            return TransferAcceptance::NotTheRecipient(transfer_recipient(
                transfer.target_customer_id.as_deref(),
            ));
        }
        self.transfer = None;
        TransferAcceptance::Accepted
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
        r#type: device_type.to_string(),
        connection_state: input_device_connection_state::DISCONNECTED.to_string(),
        device_settings_sync_state: device_settings_sync_state::SYNCED.to_string(),
        tags: HashMap::new(),
        maintenance_window_close: None,
        transfer: None,
        // A claimed device is producing nothing until a start says so.
        streaming: false,
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

/// How a refusal about a pending transfer names the recipient that transfer is
/// waiting on: the account id it named, or that it named none (the
/// `TargetCustomerId` member is optional in the model).
///
/// ONE helper, so the operations that refuse something about a pending transfer
/// — `AcceptInputDeviceTransfer` today, `RejectInputDeviceTransfer` and
/// `CancelInputDeviceTransfer` when their own issues land — cannot word its
/// recipient two ways. Private to the slice: every caller reads it through the
/// [`TransferAcceptance::NotTheRecipient`] it is carried in.
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
    /// Materialise every maintenance window the clock has already closed.
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
    /// claimed. An input whose delete has been accepted is no longer attached,
    /// the same reading of a current reference the rest of this state applies.
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
        // "The output attachment type of the input device": a device some input
        // ingests from is that input's source, and one nothing ingests from is
        // attached to nothing.
        let output_type = if medialive_input_arns.is_empty() {
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
        // HD" / "... type UHD"); every other member of those settings is read
        // off the unit, and no unit is connected to this emulator.
        let device_state = Some(device.device_state().to_string());
        let is_hd = device.r#type == input_device_type::HD;
        InputDeviceSummary {
            arn: Some(device.arn.clone()),
            connection_state: Some(device.connection_state.clone()),
            device_settings_sync_state: Some(device.device_settings_sync_state.clone()),
            device_update_status: Some(device.device_update_status().to_string()),
            hd_device_settings: is_hd.then(|| InputDeviceHdSettings {
                device_state: device_state.clone(),
                ..InputDeviceHdSettings::default()
            }),
            uhd_device_settings: (!is_hd).then(|| InputDeviceUhdSettings {
                device_state,
                ..InputDeviceUhdSettings::default()
            }),
            id: Some(device.id.clone()),
            // Hardware properties of a unit this emulator does not have.
            mac_address: None,
            serial_number: None,
            network_settings: None,
            availability_zone: None,
            name: device.name.clone(),
            r#type: Some(device.r#type.clone()),
            tags: Some(device.tags.clone()),
            // "Returned only if the outputType is MEDIALIVE_INPUT."
            medialive_input_arns: (!medialive_input_arns.is_empty())
                .then_some(medialive_input_arns),
            output_type: Some(output_type.to_string()),
        }
    }
}

/// Project an `InputDeviceSummary` onto one of the structurally identical
/// per-operation FLAT input-device output shapes (`DescribeInputDeviceOutput`
/// today; `UpdateInputDeviceOutput` and the rest when their own issues land).
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
