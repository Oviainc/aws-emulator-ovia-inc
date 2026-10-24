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
//! A device's MAINTENANCE WINDOW is one stored instant on the record: the
//! moment the window the clock owes closes. `StartInputDeviceMaintenanceWindow`
//! opens it, which is what puts the device's `DeviceUpdateStatus` in `UPDATING`,
//! and [`ComAmazonawsMedialiveMedialiveState::settle`] — which every operation
//! runs first — is what closes it once it comes due, back to `UP_TO_DATE`. The
//! instant is stored and never slept on.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::InputDeviceSummary;

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

/// The `InputDeviceType` values, spelled exactly as the model declares them
/// (`smithy_query(query="shape", shape="InputDeviceType")`: HD | UHD).
pub mod input_device_type {
    pub const HD: &str = "HD";
    pub const UHD: &str = "UHD";
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

    /// The device this account owns under `id`, to write through, or `None` when
    /// no claim has registered that id.
    ///
    /// Every MUTATION of a device resolves its id here, so an id this account
    /// has not claimed is the operation's own not-found rather than a device
    /// conjured out of the request.
    pub fn input_device_mut(&mut self, id: &str) -> Option<&mut InputDeviceRecord> {
        self.input_devices.get_mut(id)
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
    /// The one projection of a device, so every operation that reports one
    /// (`DescribeInputDevice` through [`input_device_output`], and the listing
    /// when its own issue lands) reads the same record the same way.
    pub fn input_device_summary(&self, id: &str) -> Option<InputDeviceSummary> {
        let device = self.input_devices.get(id)?;
        let medialive_input_arns = self.inputs_using_input_device(id);
        // "The output attachment type of the input device": a device some input
        // ingests from is that input's source, and one nothing ingests from is
        // attached to nothing.
        let output_type = if medialive_input_arns.is_empty() {
            input_device_output_type::NONE
        } else {
            input_device_output_type::MEDIALIVE_INPUT
        };
        Some(InputDeviceSummary {
            arn: Some(device.arn.clone()),
            connection_state: Some(device.connection_state.clone()),
            device_settings_sync_state: Some(device.device_settings_sync_state.clone()),
            device_update_status: Some(device.device_update_status().to_string()),
            // The settings that describe the video a Link unit is producing are
            // read off the unit, and no unit is connected to this emulator.
            hd_device_settings: None,
            uhd_device_settings: None,
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
        })
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
