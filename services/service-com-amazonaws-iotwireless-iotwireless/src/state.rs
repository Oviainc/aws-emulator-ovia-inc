// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotwireless-iotwireless service.
//!
//! Every implemented operation computes its answer from the records held here; the
//! handlers reach this store through [`service_state`] and nothing else.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    ConflictException, LoRaWANDevice, LoRaWANDeviceProfile, LoRaWANFuotaTask, LoRaWANGateway,
    LoRaWANMulticast, LoRaWANServiceProfile, ResourceNotFoundException, SidewalkDevice, Tag,
    UpdateWirelessGatewayTaskCreate, ValidationException,
};

/// The ARN service namespace (`arn:aws:iotwireless:...`) and the SigV4 signing name
/// this service publishes.
pub const ARN_SERVICE: &str = "iotwireless";

/// The region an ARN carries when a request names none (unsigned, or signed without a
/// parseable credential scope). `us-east-1` is the default every generated emulator
/// presents.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to stamp into an ARN for this request: the SigV4 credential scope's
/// region, falling back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:iotwireless:<region>:<account>:Destination/<name>`.
pub fn destination_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("Destination/{name}"))
}

/// `arn:aws:iotwireless:<region>:<account>:WirelessDevice/<id>`.
pub fn wireless_device_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("WirelessDevice/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:DeviceProfile/<id>`.
pub fn device_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("DeviceProfile/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:ServiceProfile/<id>`.
pub fn service_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("ServiceProfile/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:MulticastGroup/<id>`.
pub fn multicast_group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("MulticastGroup/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:FuotaTask/<id>`.
pub fn fuota_task_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("FuotaTask/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:WirelessGateway/<id>`.
pub fn wireless_gateway_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("WirelessGateway/{id}"))
}

/// `arn:aws:iotwireless:<region>:<account>:WirelessGatewayTaskDefinition/<id>`.
pub fn wireless_gateway_task_definition_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("WirelessGatewayTaskDefinition/{id}"),
    )
}

/// `ExpressionType`'s modelled values, as the model-generated
/// [`crate::ops::types::EXPRESSION_TYPE_VALUES`] lists them.
pub use crate::ops::types::EXPRESSION_TYPE_VALUES as EXPRESSION_TYPES;

/// `WirelessDeviceType`'s modelled values, as the model-generated
/// [`crate::ops::types::WIRELESS_DEVICE_TYPE_VALUES`] lists them.
pub use crate::ops::types::WIRELESS_DEVICE_TYPE_VALUES as WIRELESS_DEVICE_TYPES;

/// The identifier kinds `GetWirelessDevice` accepts, as the model-generated
/// [`crate::ops::types::WIRELESS_DEVICE_ID_TYPE_VALUES`] lists them.
pub use crate::ops::types::WIRELESS_DEVICE_ID_TYPE_VALUES as WIRELESS_DEVICE_ID_TYPES;

/// The identifier kinds `GetWirelessGateway` accepts, as the model-generated
/// [`crate::ops::types::WIRELESS_GATEWAY_ID_TYPE_VALUES`] lists them.
pub use crate::ops::types::WIRELESS_GATEWAY_ID_TYPE_VALUES as WIRELESS_GATEWAY_ID_TYPES;

/// `PositioningConfigStatus`'s modelled values, as the model-generated
/// [`crate::ops::types::POSITIONING_CONFIG_STATUS_VALUES`] lists them.
pub use crate::ops::types::POSITIONING_CONFIG_STATUS_VALUES as POSITIONING_CONFIG_STATUSES;

/// `DeviceProfileType`'s modelled values, as the model-generated
/// [`crate::ops::types::DEVICE_PROFILE_TYPE_VALUES`] lists them.
pub use crate::ops::types::DEVICE_PROFILE_TYPE_VALUES as DEVICE_PROFILE_TYPES;

/// The characters a resource name may not carry, as every `Name` member's model doc
/// states it ("The following special characters aren't accepted: <>^#~$").
pub const FORBIDDEN_NAME_CHARS: &[char] = &['<', '>', '^', '#', '~', '$'];

/// The first character of `name` this service refuses, or `None` when every character
/// is accepted.
pub fn forbidden_name_char(name: &str) -> Option<char> {
    name.chars().find(|c| FORBIDDEN_NAME_CHARS.contains(c))
}

/// The marker every `NextToken` this service mints carries, so a well-formed string a
/// client made up cannot pass as a resume point.
const PAGE_TOKEN_MARKER: &str = "iotwireless/v1/";

/// Encode a list's resume point — the key of the LAST item the page returned — as the
/// opaque `NextToken` a client echoes back.
///
/// Hex rather than the raw key so the token stays opaque: a client cannot read a
/// resource name out of it, and it survives a URI query parameter unescaped. The
/// encoded payload carries [`PAGE_TOKEN_MARKER`], which [`decode_page_token`] requires,
/// so only a token this service issued resumes a list.
pub fn encode_page_token(last_key: &str) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_MARKER}{last_key}").as_bytes())
}

/// The resume point a `NextToken` names, or `None` when the token is not one this
/// service minted (the caller gets a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    decoded
        .strip_prefix(PAGE_TOKEN_MARKER)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

/// A destination: the rule or MQTT topic a device's uplinks are routed to.
#[derive(Debug, Clone)]
pub struct Destination {
    pub name: String,
    pub arn: String,
    pub expression: String,
    /// `ExpressionType`: `RuleName` | `MqttTopic`.
    pub expression_type: String,
    pub description: Option<String>,
    pub role_arn: String,
    pub tags: Vec<Tag>,
}

/// The sentence this service's 404 carries when the destination a request addresses is
/// not one the account holds: fixed text, with no part of it taken from the request or
/// from the store, so the refusal discloses nothing about which names exist.
pub const DESTINATION_NOT_FOUND_MESSAGE: &str = "Destination doesn't exist";

/// The `ResourceNotFoundException` body for a destination the account does not hold.
///
/// One function, so every operation that addresses a destination by name refuses a
/// missing one with the SAME body: [`DESTINATION_NOT_FOUND_MESSAGE`] and the two
/// optional members the model merely permits (`ResourceId`, `ResourceType`) left unset,
/// which is how this service answers the destination lookup.
pub fn destination_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(DESTINATION_NOT_FOUND_MESSAGE.to_string()),
        resource_id: None,
        resource_type: None,
    }
}

/// A device profile: the radio parameters a wireless device of one technology shares.
///
/// `profile_type` is what `DeviceProfileType` names, derived from which technology
/// member the create carried; `SidewalkCreateDeviceProfile` models no members, so a
/// Sidewalk profile's only stored settings are that it IS a Sidewalk profile.
#[derive(Debug, Clone)]
pub struct DeviceProfile {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    /// `DeviceProfileType`: `LoRaWAN` | `Sidewalk`.
    pub profile_type: String,
    pub lorawan: Option<LoRaWANDeviceProfile>,
    pub tags: Vec<Tag>,
}

/// The sentence this service's 409 carries when a create claims a `DevEui` another
/// device in the account already holds: FIXED text with no variable part, so a client
/// can tell this conflict from the idempotency-token conflict the same operation raises
/// with the same error code, and so the refusal discloses nothing about the device that
/// holds the identifier.
pub const DUPLICATE_DEV_EUI_MESSAGE: &str = "Device with given DevEui already exists";

/// The `ConflictException` body for a `DevEui` already registered in the account.
///
/// One function, so every operation that claims a `DevEui` refuses a duplicate with the
/// same body: [`DUPLICATE_DEV_EUI_MESSAGE`] and the two optional members the model
/// merely permits (`ResourceId`, `ResourceType`) left unset, which keeps the owning
/// device's identity out of a refusal served to a caller that cannot read it.
pub fn duplicate_dev_eui() -> ConflictException {
    ConflictException {
        message: Some(DUPLICATE_DEV_EUI_MESSAGE.to_string()),
        resource_id: None,
        resource_type: None,
    }
}

/// The LoRaWAN specification version the 1.1 provisioning members (`OtaaV1_1`,
/// `AbpV1_1`) carry key material for.
/// Source: `LoRaWANDevice`'s members, `smithy_query(query="shape",
/// shape="LoRaWANDevice")` ("OTAA device object for v1.1", "ABP device object for
/// create APIs for v1.1").
pub const LORAWAN_VERSION_1_1: &str = "1.1";

/// The LoRaWAN specification version the 1.0.x provisioning members (`OtaaV1_0_x`,
/// `AbpV1_0_x`) carry key material for, at the granularity provisioning is defined at:
/// 1.0.x key material is the same across the 1.0 revisions.
/// Source: the same `LoRaWANDevice` members ("for v1.0.x").
pub const LORAWAN_VERSION_1_0: &str = "1.0";

/// The specification version family a `MacVersion` names: its first two dot-separated
/// components, since a device profile's `MacVersion` is a revision of a family
/// (`1.0.3` is a revision of `1.0`) and provisioning is version-specific only down to
/// the family.
///
/// `MacVersion` is a free-form string in the model (`length 0..64`), so an
/// unrecognisable value is its own family and matches only itself.
pub fn mac_version_family(mac_version: &str) -> &str {
    match mac_version.match_indices('.').nth(1) {
        Some((second_dot, _)) => &mac_version[..second_dot],
        None => mac_version,
    }
}

/// Every LoRaWAN specification version the provisioning members of `lorawan` carry key
/// material for, in the order the model declares them.
pub fn provisioning_versions(lorawan: &LoRaWANDevice) -> Vec<&'static str> {
    let mut versions = Vec::new();
    if lorawan.otaa_v1_1.is_some() || lorawan.abp_v1_1.is_some() {
        versions.push(LORAWAN_VERSION_1_1);
    }
    if lorawan.otaa_v1_0_x.is_some() || lorawan.abp_v1_0_x.is_some() {
        versions.push(LORAWAN_VERSION_1_0);
    }
    versions
}

/// The referenced profile's `MacVersion` when it cannot provision the key material
/// `lorawan` carries, and `None` when the two agree.
///
/// A device profile declares the MAC version the devices it provisions run, and each
/// provisioning member a create carries declares the version ITS key material is for,
/// so EVERY member the create carries must name the profile's version family — a create
/// carrying material for two families cannot agree with a profile that declares one. A
/// profile that declares no `MacVersion` (absent, or the empty string the model's
/// `length 0..64` permits), and a create that carries no provisioning member, constrain
/// nothing.
pub fn mismatched_mac_version(lorawan: &LoRaWANDevice, profile: &DeviceProfile) -> Option<String> {
    let mac_version = profile
        .lorawan
        .as_ref()?
        .mac_version
        .as_deref()
        .filter(|declared| !declared.is_empty())?;
    let family = mac_version_family(mac_version);
    provisioning_versions(lorawan)
        .into_iter()
        .any(|requested| requested != family)
        .then(|| mac_version.to_string())
}

/// The `ValidationException` body for key material the referenced device profile's
/// `MacVersion` cannot provision: fixed text whose only variable part is that stored
/// `MacVersion`.
pub fn mac_version_mismatch(mac_version: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Validation error: the input profile version is not match macVersion defines in \
             deviceProfile: {mac_version}"
        )),
    }
}

/// A service profile: the LoRaWAN network-server settings a set of wireless devices
/// shares.
///
/// `lorawan` holds the settings as the resource carries them — the create's own members
/// with the defaults its model docs publish filled in for the ones it omitted — so every
/// reader projects one stored record rather than re-deriving a default of its own.
#[derive(Debug, Clone)]
pub struct ServiceProfile {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub lorawan: Option<LoRaWANServiceProfile>,
    pub tags: Vec<Tag>,
}

/// The `TxPowerIndexMin` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.TxPowerIndexMin`'s model doc ("Default: 0").
pub const DEFAULT_TX_POWER_INDEX_MIN: i32 = 0;

/// The `TxPowerIndexMax` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.TxPowerIndexMax`'s model doc ("Default: 15").
pub const DEFAULT_TX_POWER_INDEX_MAX: i32 = 15;

/// The `NbTransMin` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.NbTransMin`'s model doc ("Default: 0").
pub const DEFAULT_NB_TRANS_MIN: i32 = 0;

/// The `NbTransMax` a service profile carries when its create named none.
/// Source: `LoRaWANServiceProfile.NbTransMax`'s model doc ("Default: 3").
pub const DEFAULT_NB_TRANS_MAX: i32 = 3;

/// The settings a created service profile carries: the ones the request named, and the
/// default its model doc publishes for each one it left out.
///
/// One function, so the stored resource — and therefore every reader of it — carries
/// the same settings whether or not the create spelled them out.
pub fn service_profile_settings(requested: &LoRaWANServiceProfile) -> LoRaWANServiceProfile {
    LoRaWANServiceProfile {
        tx_power_index_min: Some(
            requested
                .tx_power_index_min
                .unwrap_or(DEFAULT_TX_POWER_INDEX_MIN),
        ),
        tx_power_index_max: Some(
            requested
                .tx_power_index_max
                .unwrap_or(DEFAULT_TX_POWER_INDEX_MAX),
        ),
        nb_trans_min: Some(requested.nb_trans_min.unwrap_or(DEFAULT_NB_TRANS_MIN)),
        nb_trans_max: Some(requested.nb_trans_max.unwrap_or(DEFAULT_NB_TRANS_MAX)),
        ..requested.clone()
    }
}

/// A wireless device registered against a destination.
#[derive(Debug, Clone)]
pub struct WirelessDevice {
    pub id: String,
    pub arn: String,
    /// `WirelessDeviceType`: `LoRaWAN` | `Sidewalk`.
    pub device_type: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub destination_name: String,
    pub lorawan: Option<LoRaWANDevice>,
    pub sidewalk: Option<SidewalkDevice>,
    /// `PositioningConfigStatus`: `Enabled` | `Disabled`.
    pub positioning: Option<String>,
    /// Set by `AssociateWirelessDeviceWithThing`, which is still scaffolded, so a
    /// device created today carries none.
    pub thing_name: Option<String>,
    pub thing_arn: Option<String>,
    /// The FUOTA task this device belongs to, set by `AssociateWirelessDeviceWithFuotaTask`
    /// and cleared by its disassociate — both still scaffolded, so a device created
    /// today belongs to none. `ListWirelessDevices?fuotaTaskId=` selects on it.
    pub fuota_task_id: Option<String>,
    /// The multicast group this device belongs to, set by
    /// `AssociateWirelessDeviceWithMulticastGroup` and cleared by its disassociate —
    /// both still scaffolded. `ListWirelessDevices?multicastGroupId=` selects on it.
    pub multicast_group_id: Option<String>,
    pub tags: Vec<Tag>,
    /// When the device was registered. Reported by `GetWirelessDeviceStatistics` and
    /// the import-task operations, which are still scaffolded, so no implemented
    /// operation reads it yet.
    pub created_at: SystemTime,
    /// Set when an uplink is received; the uplink path is not emulated, so this stays
    /// `None` rather than reporting traffic that never happened.
    pub last_uplink_received_at: Option<String>,
}

/// A wireless gateway: the LoRaWAN radio configuration of one provisioned gateway.
#[derive(Debug, Clone)]
pub struct WirelessGateway {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The gateway configuration the create carried, reported back by the gateway's
    /// reader. `CreateWirelessGateway` marks `LoRaWAN` required, so every stored gateway
    /// has one.
    pub lorawan: LoRaWANGateway,
    /// Set by `AssociateWirelessGatewayWithThing`, which is still scaffolded, so a
    /// gateway created today carries none.
    pub thing_name: Option<String>,
    pub thing_arn: Option<String>,
    pub tags: Vec<Tag>,
}

/// A wireless gateway task definition: the firmware update one gateway task applies.
///
/// `update` is the settings the create carried, reported back unchanged by the
/// definition's reader; `auto_create_tasks` is the modelled flag that says whether the
/// service creates a task for every gateway on the named current version by itself,
/// rather than only when `CreateWirelessGatewayTask` asks for one.
#[derive(Debug, Clone)]
pub struct WirelessGatewayTaskDefinition {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub auto_create_tasks: bool,
    pub update: Option<UpdateWirelessGatewayTaskCreate>,
    pub tags: Vec<Tag>,
}

/// The `ResourceNotFoundException` body for a wireless gateway task definition the
/// account does not hold.
///
/// One function, so every operation that addresses a task definition by `Id` — its
/// reader, and the task operations that reference one — refuses a missing one with the
/// same body.
pub fn wireless_gateway_task_definition_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("No wireless gateway task definition with Id {id}")),
        resource_id: Some(id.to_string()),
        resource_type: Some("WirelessGatewayTaskDefinition".to_string()),
    }
}

/// A multicast group: the LoRaWAN downlink settings a set of wireless devices shares.
///
/// The membership itself lives on the devices (`WirelessDevice::multicast_group_id`),
/// so the counts `GetMulticastGroup` reports are DERIVED from the device store rather
/// than kept here as a second copy that could disagree with it.
#[derive(Debug, Clone)]
pub struct MulticastGroup {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The LoRaWAN settings the create carried, reported back by the group's reader.
    pub lorawan: LoRaWANMulticast,
    pub tags: Vec<Tag>,
    /// When the group was created, reported as `GetMulticastGroup`'s `CreatedAt`.
    pub created_at: SystemTime,
}

/// A FUOTA task: the firmware image a set of wireless devices is updated with, and the
/// fragmentation settings the update is sent down with.
///
/// The membership itself lives on the devices (`WirelessDevice::fuota_task_id`), like a
/// multicast group's, so nothing here is a second copy of it.
#[derive(Debug, Clone)]
pub struct FuotaTask {
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// The LoRaWAN settings the create carried (`RfRegion`), reported back by the
    /// task's reader.
    pub lorawan: Option<LoRaWANFuotaTask>,
    /// The S3 URI of the firmware image. `CreateFuotaTask` marks it required, so every
    /// stored task has one.
    pub firmware_update_image: String,
    /// The role the firmware update is applied under. Required, like the image.
    pub firmware_update_role: String,
    pub redundancy_percent: Option<i32>,
    pub fragment_size_bytes: Option<i32>,
    pub fragment_interval_ms: Option<i32>,
    /// The base64 file descriptor the create carried, reported back unchanged.
    pub descriptor: Option<String>,
    pub tags: Vec<Tag>,
    /// `FuotaTaskStatus`, as [`FUOTA_TASK_STATUS_PENDING`] documents the value a
    /// created task carries.
    pub status: String,
    /// When the task was created, reported as `GetFuotaTask`'s `CreatedAt`.
    pub created_at: SystemTime,
    /// When the FUOTA session started, reported as `GetFuotaTask`'s `LoRaWAN.StartTime`.
    /// A session starts only on `StartFuotaTask`, which is still scaffolded, so a task
    /// created today has none.
    pub start_time: Option<SystemTime>,
}

/// The `FuotaTaskStatus` a created FUOTA task carries: nothing has asked for its
/// session yet, so it is waiting to be started.
///
/// One of the modelled values [`crate::ops::types::FUOTA_TASK_STATUS_VALUES`] lists;
/// the task leaves it only when an operation moves it (the session operations are still
/// scaffolded).
pub const FUOTA_TASK_STATUS_PENDING: &str = "Pending";

/// The `ResourceNotFoundException` body for a FUOTA task the account does not hold.
///
/// One function, so every operation that addresses a FUOTA task by `Id` refuses a
/// missing one with the same body.
pub fn fuota_task_not_found(id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("FuotaTask {id} not found")),
        resource_id: Some(id.to_string()),
        resource_type: Some("FuotaTask".to_string()),
    }
}

/// What an `@idempotencyToken` create remembers: which resource the token minted, and
/// a fingerprint of the parameters it was minted with.
///
/// `CreateDestination`/`CreateWirelessDevice` document that replaying a token with the
/// SAME parameters completes successfully, while replaying it with DIFFERENT parameters
/// is an HTTP 409 conflict — which needs both halves.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The key of the resource the first call created (a destination name, a device id).
    pub resource_key: String,
    /// Fingerprint of the create parameters, the token excluded.
    pub fingerprint: String,
}

/// A tag set as one comparable, order-independent sequence of `(key, value)` pairs.
///
/// Every `@idempotencyToken` create fingerprints its parameters to decide whether a
/// replayed token carries "the same parameters", and a tag set is unordered on the wire,
/// so two requests that differ only in tag order carry the same parameters. One
/// function, so every create orders them the same way.
pub fn sorted_tag_pairs(tags: &[Tag]) -> Vec<(&str, &str)> {
    let mut pairs: Vec<(&str, &str)> = tags
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    pairs.sort_unstable();
    pairs
}

/// The most destinations this emulator stores.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop cannot
/// grow the emulator's heap without bound. Neither create operation models a quota
/// error (`smithy_query(query="operation", operation="CreateDestination")` lists
/// AccessDenied / Conflict / InternalServer / ResourceNotFound / Throttling /
/// Validation), so a full store refuses with `ValidationException`.
pub const MAX_DESTINATIONS: usize = 100;

/// The most wireless devices this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`].
pub const MAX_WIRELESS_DEVICES: usize = 250;

/// The most device profiles this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateDeviceProfile` models no quota error either
/// (`smithy_query(query="operation", operation="CreateDeviceProfile")` lists
/// AccessDenied / Conflict / InternalServer / Throttling / Validation), so a full store
/// refuses with `ValidationException`.
pub const MAX_DEVICE_PROFILES: usize = 100;

/// The most service profiles this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateServiceProfile` models no quota error either
/// (`smithy_query(query="operation", operation="CreateServiceProfile")` lists
/// AccessDenied / Conflict / InternalServer / Throttling / Validation), so a full store
/// refuses with `ValidationException`.
pub const MAX_SERVICE_PROFILES: usize = 100;

/// The most multicast groups this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateMulticastGroup` models no quota error either
/// (`smithy_query(query="operation", operation="CreateMulticastGroup")` lists
/// AccessDenied / Conflict / InternalServer / ResourceNotFound / Throttling /
/// Validation), so a full store refuses with `ValidationException`.
pub const MAX_MULTICAST_GROUPS: usize = 100;

/// The most FUOTA tasks this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateFuotaTask` models no quota error either
/// (`smithy_query(query="operation", operation="CreateFuotaTask")` lists AccessDenied /
/// Conflict / InternalServer / ResourceNotFound / Throttling / Validation), so a full
/// store refuses with `ValidationException`.
pub const MAX_FUOTA_TASKS: usize = 100;

/// The most `ClientRequestToken` entries kept for `CreateFuotaTask` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_FUOTA_TASK_TOKENS: usize = MAX_FUOTA_TASKS;

/// The most wireless gateways this emulator stores. A ceiling, for the same reason as
/// [`MAX_DESTINATIONS`]; `CreateWirelessGateway` models no quota error either
/// (`smithy_query(query="operation", operation="CreateWirelessGateway")` lists
/// AccessDenied / Conflict / InternalServer / Throttling / Validation), so a full store
/// refuses with `ValidationException`.
pub const MAX_WIRELESS_GATEWAYS: usize = 100;

/// The most wireless gateway task definitions this emulator stores. A ceiling, for the
/// same reason as [`MAX_DESTINATIONS`]; `CreateWirelessGatewayTaskDefinition` models no
/// quota error either (`smithy_query(query="operation",
/// operation="CreateWirelessGatewayTaskDefinition")` lists AccessDenied / Conflict /
/// InternalServer / ResourceNotFound / Throttling / Validation), so a full store refuses
/// with `ValidationException`.
pub const MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS: usize = 100;

/// The most `ClientRequestToken` entries kept for
/// `CreateWirelessGatewayTaskDefinition` idempotency. Bounded for the same reason as
/// [`MAX_DESTINATION_TOKENS`].
pub const MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS: usize =
    MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS;

/// The most `ClientRequestToken` entries kept for `CreateDestination` idempotency.
///
/// At most one token is recorded per stored destination and a token is dropped with the
/// destination it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the resources they name.
pub const MAX_DESTINATION_TOKENS: usize = MAX_DESTINATIONS;

/// The most `ClientRequestToken` entries kept for `CreateWirelessDevice` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_WIRELESS_DEVICE_TOKENS: usize = MAX_WIRELESS_DEVICES;

/// The most `ClientRequestToken` entries kept for `CreateDeviceProfile` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_DEVICE_PROFILE_TOKENS: usize = MAX_DEVICE_PROFILES;

/// The most `ClientRequestToken` entries kept for `CreateServiceProfile` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_SERVICE_PROFILE_TOKENS: usize = MAX_SERVICE_PROFILES;

/// The most `ClientRequestToken` entries kept for `CreateMulticastGroup` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_MULTICAST_GROUP_TOKENS: usize = MAX_MULTICAST_GROUPS;

/// The most `ClientRequestToken` entries kept for `CreateWirelessGateway` idempotency.
/// Bounded for the same reason as [`MAX_DESTINATION_TOKENS`].
pub const MAX_WIRELESS_GATEWAY_TOKENS: usize = MAX_WIRELESS_GATEWAYS;

/// Per-service state for com-amazonaws-iotwireless-iotwireless. A handler reaches it through [`service_state`].
///
/// The resource maps are `BTreeMap`s so every list enumerates in one stable, sorted key
/// order — which is what makes a pagination token that names the last key returned
/// resume exactly where the previous page stopped.
///
/// Every store here is BOUNDED: the resource maps by [`MAX_DESTINATIONS`] /
/// [`MAX_WIRELESS_DEVICES`] / [`MAX_DEVICE_PROFILES`] / [`MAX_SERVICE_PROFILES`] /
/// [`MAX_MULTICAST_GROUPS`] / [`MAX_FUOTA_TASKS`] /
/// [`MAX_WIRELESS_GATEWAYS`] / [`MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS`] (the
/// creates refuse once full), `dev_eui_index` by one entry per stored device,
/// `gateway_eui_index` by one entry per stored gateway, and the
/// token maps by [`MAX_DESTINATION_TOKENS`] / [`MAX_WIRELESS_DEVICE_TOKENS`] /
/// [`MAX_DEVICE_PROFILE_TOKENS`] / [`MAX_SERVICE_PROFILE_TOKENS`] /
/// [`MAX_MULTICAST_GROUP_TOKENS`] / [`MAX_FUOTA_TASK_TOKENS`] /
/// [`MAX_WIRELESS_GATEWAY_TOKENS`] /
/// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS`] with oldest-entry
/// eviction. The token maps are private so every write goes through
/// [`Self::record_destination_token`] / [`Self::record_wireless_device_token`] /
/// [`Self::record_device_profile_token`] / [`Self::record_service_profile_token`] /
/// [`Self::record_multicast_group_token`] / [`Self::record_fuota_task_token`] /
/// [`Self::record_wireless_gateway_token`] /
/// [`Self::record_wireless_gateway_task_definition_token`] and
/// cannot bypass the cap.
#[derive(Debug, Default)]
pub struct ComAmazonawsIotwirelessIotwirelessState {
    /// Destinations by `Name`. Bounded by [`MAX_DESTINATIONS`].
    pub destinations: BTreeMap<String, Destination>,
    /// Wireless devices by `Id`. Bounded by [`MAX_WIRELESS_DEVICES`].
    pub wireless_devices: BTreeMap<String, WirelessDevice>,
    /// Device profiles by `Id`. Bounded by [`MAX_DEVICE_PROFILES`].
    pub device_profiles: BTreeMap<String, DeviceProfile>,
    /// Service profiles by `Id`. Bounded by [`MAX_SERVICE_PROFILES`].
    pub service_profiles: BTreeMap<String, ServiceProfile>,
    /// Multicast groups by `Id`. Bounded by [`MAX_MULTICAST_GROUPS`].
    pub multicast_groups: BTreeMap<String, MulticastGroup>,
    /// FUOTA tasks by `Id`. Bounded by [`MAX_FUOTA_TASKS`].
    pub fuota_tasks: BTreeMap<String, FuotaTask>,
    /// Wireless gateways by `Id`. Bounded by [`MAX_WIRELESS_GATEWAYS`].
    pub wireless_gateways: BTreeMap<String, WirelessGateway>,
    /// Wireless gateway task definitions by `Id`. Bounded by
    /// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITIONS`].
    pub wireless_gateway_task_definitions: BTreeMap<String, WirelessGatewayTaskDefinition>,
    /// `GatewayEui` -> gateway id, so `GetWirelessGateway?identifierType=GatewayEui`
    /// resolves without scanning and a duplicate `GatewayEui` is refusable. At most one
    /// entry per stored gateway.
    pub gateway_eui_index: BTreeMap<String, String>,
    /// `DevEui` -> device id, so `GetWirelessDevice?identifierType=DevEui` resolves
    /// without scanning and a duplicate `DevEui` is refusable. At most one entry per
    /// stored device, and the entry is released when the device is deleted.
    pub dev_eui_index: BTreeMap<String, String>,
    /// `CreateDestination` client request tokens, bounded by
    /// [`MAX_DESTINATION_TOKENS`]. Private: write through
    /// [`Self::record_destination_token`], read through [`Self::destination_token`].
    destination_tokens: HashMap<String, IdempotentCreate>,
    /// The order [`Self::destination_tokens`] was written in, so the cap evicts the
    /// oldest token rather than an arbitrary one.
    destination_token_order: VecDeque<String>,
    /// `CreateWirelessDevice` client request tokens, bounded by
    /// [`MAX_WIRELESS_DEVICE_TOKENS`]. Private, like the destination tokens.
    wireless_device_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::wireless_device_tokens`].
    wireless_device_token_order: VecDeque<String>,
    /// `CreateDeviceProfile` client request tokens, bounded by
    /// [`MAX_DEVICE_PROFILE_TOKENS`]. Private, like the destination tokens.
    device_profile_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::device_profile_tokens`].
    device_profile_token_order: VecDeque<String>,
    /// `CreateServiceProfile` client request tokens, bounded by
    /// [`MAX_SERVICE_PROFILE_TOKENS`]. Private, like the destination tokens.
    service_profile_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::service_profile_tokens`].
    service_profile_token_order: VecDeque<String>,
    /// `CreateMulticastGroup` client request tokens, bounded by
    /// [`MAX_MULTICAST_GROUP_TOKENS`]. Private, like the destination tokens.
    multicast_group_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::multicast_group_tokens`].
    multicast_group_token_order: VecDeque<String>,
    /// `CreateFuotaTask` client request tokens, bounded by [`MAX_FUOTA_TASK_TOKENS`].
    /// Private, like the destination tokens.
    fuota_task_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::fuota_task_tokens`].
    fuota_task_token_order: VecDeque<String>,
    /// `CreateWirelessGateway` client request tokens, bounded by
    /// [`MAX_WIRELESS_GATEWAY_TOKENS`]. Private, like the destination tokens.
    wireless_gateway_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::wireless_gateway_tokens`].
    wireless_gateway_token_order: VecDeque<String>,
    /// `CreateWirelessGatewayTaskDefinition` client request tokens, bounded by
    /// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS`]. Private, like the destination
    /// tokens.
    wireless_gateway_task_definition_tokens: HashMap<String, IdempotentCreate>,
    /// Write order for [`Self::wireless_gateway_task_definition_tokens`].
    wireless_gateway_task_definition_token_order: VecDeque<String>,
}

/// Record `token` in `tokens`, evicting the oldest recorded token first if that would
/// push the map past `cap`.
///
/// Shared by every token map so the caps are enforced by one piece of code: the
/// order queue and the map are only ever written together, here.
///
/// The cap cannot bind in practice, which is why evicting the oldest entry is a safe
/// last resort rather than a behaviour change: at most one token is recorded per stored
/// resource, a resource's tokens are dropped with it where its delete is implemented
/// (`forget_*_tokens_for`, which the destination and wireless-device deletes call —
/// the device-profile, multicast-group and wireless-gateway deletes are still
/// scaffolded, so those three
/// maps release a token only when the whole store is dropped), and each
/// token cap equals its resource ceiling — so the map can only reach `cap` when every
/// entry in it names a resource the store accepted and the resource store is itself
/// full, which the create already refuses before it gets here. Eviction exists so the
/// bound holds whatever a future caller does; if it ever did evict a live resource's
/// token, the consequence is only that a replay of that token is treated as a fresh
/// create (a 409 for a destination whose name is taken, a second device otherwise).
fn record_token(
    tokens: &mut HashMap<String, IdempotentCreate>,
    order: &mut VecDeque<String>,
    cap: usize,
    token: String,
    record: IdempotentCreate,
) {
    // Only a NEW token grows the map, so re-recording an existing one must not evict a
    // live entry — least of all the token being re-recorded.
    if !tokens.contains_key(&token) {
        while tokens.len() >= cap {
            let Some(oldest) = order.pop_front() else { break };
            tokens.remove(&oldest);
        }
    }
    if tokens.insert(token.clone(), record).is_none() {
        order.push_back(token);
    }
}

/// Drop every token in `tokens` that minted `resource_key`.
fn forget_tokens_for(
    tokens: &mut HashMap<String, IdempotentCreate>,
    order: &mut VecDeque<String>,
    resource_key: &str,
) {
    tokens.retain(|_, record| record.resource_key != resource_key);
    let live = &*tokens;
    order.retain(|token| live.contains_key(token));
}

/// A fresh v4 uuid `existing` does not already hold.
///
/// Every resource id this service mints (`WirelessDeviceId`, `DeviceProfileId`) is a
/// uuid, so they all come from the `uuid` crate's v4 generator through here; the loop
/// re-draws rather than ever handing back an id already in the store.
fn mint_unused_uuid<T>(existing: &BTreeMap<String, T>) -> String {
    loop {
        let id = ::uuid::Uuid::new_v4().to_string();
        if !existing.contains_key(&id) {
            return id;
        }
    }
}

impl ComAmazonawsIotwirelessIotwirelessState {
    /// Mint a fresh, unused `WirelessDeviceId`.
    pub fn mint_wireless_device_id(&mut self) -> String {
        mint_unused_uuid(&self.wireless_devices)
    }

    /// Mint a fresh, unused `DeviceProfileId`.
    pub fn mint_device_profile_id(&mut self) -> String {
        mint_unused_uuid(&self.device_profiles)
    }

    /// Mint a fresh, unused `ServiceProfileId`.
    pub fn mint_service_profile_id(&mut self) -> String {
        mint_unused_uuid(&self.service_profiles)
    }

    /// What a `CreateServiceProfile` `ClientRequestToken` already minted, if anything.
    pub fn service_profile_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.service_profile_tokens.get(token)
    }

    /// Record `token` as having minted the service profile named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_SERVICE_PROFILE_TOKENS`].
    pub fn record_service_profile_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.service_profile_tokens,
            &mut self.service_profile_token_order,
            MAX_SERVICE_PROFILE_TOKENS,
            token,
            record,
        );
    }

    /// Mint a fresh, unused `MulticastGroupId`.
    pub fn mint_multicast_group_id(&mut self) -> String {
        mint_unused_uuid(&self.multicast_groups)
    }

    /// Mint a fresh, unused `FuotaTaskId`.
    pub fn mint_fuota_task_id(&mut self) -> String {
        mint_unused_uuid(&self.fuota_tasks)
    }

    /// What a `CreateFuotaTask` `ClientRequestToken` already minted, if anything.
    pub fn fuota_task_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.fuota_task_tokens.get(token)
    }

    /// Record `token` as having minted the FUOTA task named by `record`, evicting the
    /// oldest token first if that would push the map past [`MAX_FUOTA_TASK_TOKENS`].
    pub fn record_fuota_task_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.fuota_task_tokens,
            &mut self.fuota_task_token_order,
            MAX_FUOTA_TASK_TOKENS,
            token,
            record,
        );
    }

    /// Mint a fresh, unused `WirelessGatewayId`.
    pub fn mint_wireless_gateway_id(&mut self) -> String {
        mint_unused_uuid(&self.wireless_gateways)
    }

    /// What a `CreateWirelessGateway` `ClientRequestToken` already minted, if anything.
    pub fn wireless_gateway_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.wireless_gateway_tokens.get(token)
    }

    /// Record `token` as having minted the gateway named by `record`, evicting the oldest
    /// token first if that would push the map past [`MAX_WIRELESS_GATEWAY_TOKENS`].
    pub fn record_wireless_gateway_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.wireless_gateway_tokens,
            &mut self.wireless_gateway_token_order,
            MAX_WIRELESS_GATEWAY_TOKENS,
            token,
            record,
        );
    }

    /// Mint a fresh, unused `WirelessGatewayTaskDefinitionId`.
    pub fn mint_wireless_gateway_task_definition_id(&mut self) -> String {
        mint_unused_uuid(&self.wireless_gateway_task_definitions)
    }

    /// What a `CreateWirelessGatewayTaskDefinition` `ClientRequestToken` already minted,
    /// if anything.
    pub fn wireless_gateway_task_definition_token(
        &self,
        token: &str,
    ) -> Option<&IdempotentCreate> {
        self.wireless_gateway_task_definition_tokens.get(token)
    }

    /// Record `token` as having minted the task definition named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS`].
    pub fn record_wireless_gateway_task_definition_token(
        &mut self,
        token: String,
        record: IdempotentCreate,
    ) {
        record_token(
            &mut self.wireless_gateway_task_definition_tokens,
            &mut self.wireless_gateway_task_definition_token_order,
            MAX_WIRELESS_GATEWAY_TASK_DEFINITION_TOKENS,
            token,
            record,
        );
    }

    /// How many stored wireless devices belong to multicast group `id`.
    ///
    /// The membership is held by the devices, so this counts them rather than reading a
    /// tally: every operation that reports a group's size answers from the same place
    /// the associate/disassociate operations write.
    pub fn devices_in_multicast_group(&self, id: &str) -> usize {
        self.wireless_devices
            .values()
            .filter(|device| device.multicast_group_id.as_deref() == Some(id))
            .count()
    }

    /// What a `CreateMulticastGroup` `ClientRequestToken` already minted, if anything.
    pub fn multicast_group_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.multicast_group_tokens.get(token)
    }

    /// Record `token` as having minted the multicast group named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_MULTICAST_GROUP_TOKENS`].
    pub fn record_multicast_group_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.multicast_group_tokens,
            &mut self.multicast_group_token_order,
            MAX_MULTICAST_GROUP_TOKENS,
            token,
            record,
        );
    }

    /// What a `CreateDestination` `ClientRequestToken` already minted, if anything.
    pub fn destination_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.destination_tokens.get(token)
    }

    /// Record `token` as having minted the destination named by `record`, evicting the
    /// oldest token first if that would push the map past [`MAX_DESTINATION_TOKENS`].
    pub fn record_destination_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.destination_tokens,
            &mut self.destination_token_order,
            MAX_DESTINATION_TOKENS,
            token,
            record,
        );
    }

    /// Drop every idempotency token that minted destination `name`, so a token never
    /// outlives the resource it names.
    pub fn forget_destination_tokens_for(&mut self, name: &str) {
        forget_tokens_for(
            &mut self.destination_tokens,
            &mut self.destination_token_order,
            name,
        );
    }

    /// What a `CreateWirelessDevice` `ClientRequestToken` already minted, if anything.
    pub fn wireless_device_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.wireless_device_tokens.get(token)
    }

    /// Record `token` as having minted the device named by `record`, evicting the oldest
    /// token first if that would push the map past [`MAX_WIRELESS_DEVICE_TOKENS`].
    pub fn record_wireless_device_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.wireless_device_tokens,
            &mut self.wireless_device_token_order,
            MAX_WIRELESS_DEVICE_TOKENS,
            token,
            record,
        );
    }

    /// Drop every idempotency token that minted device `id`, so a token never outlives
    /// the resource it names.
    pub fn forget_wireless_device_tokens_for(&mut self, id: &str) {
        forget_tokens_for(
            &mut self.wireless_device_tokens,
            &mut self.wireless_device_token_order,
            id,
        );
    }

    /// What a `CreateDeviceProfile` `ClientRequestToken` already minted, if anything.
    pub fn device_profile_token(&self, token: &str) -> Option<&IdempotentCreate> {
        self.device_profile_tokens.get(token)
    }

    /// Record `token` as having minted the device profile named by `record`, evicting
    /// the oldest token first if that would push the map past
    /// [`MAX_DEVICE_PROFILE_TOKENS`].
    pub fn record_device_profile_token(&mut self, token: String, record: IdempotentCreate) {
        record_token(
            &mut self.device_profile_tokens,
            &mut self.device_profile_token_order,
            MAX_DEVICE_PROFILE_TOKENS,
            token,
            record,
        );
    }

    /// The device a `GetWirelessDevice` identifier resolves to, or `None`.
    ///
    /// `identifier_type` is the modelled `WirelessDeviceIdType`; an unknown value
    /// resolves to nothing (the boundary validator already refuses one the enum does
    /// not list).
    pub fn find_device(&self, identifier_type: &str, identifier: &str) -> Option<&WirelessDevice> {
        match identifier_type {
            "WirelessDeviceId" => self.wireless_devices.get(identifier),
            "DevEui" => self
                .dev_eui_index
                .get(identifier)
                .and_then(|id| self.wireless_devices.get(id)),
            "ThingName" => self
                .wireless_devices
                .values()
                .find(|d| d.thing_name.as_deref() == Some(identifier)),
            "SidewalkManufacturingSn" => self.wireless_devices.values().find(|d| {
                d.sidewalk
                    .as_ref()
                    .and_then(|s| s.sidewalk_manufacturing_sn.as_deref())
                    == Some(identifier)
            }),
            _ => None,
        }
    }

    /// The gateway a `GetWirelessGateway` identifier resolves to, or `None`.
    ///
    /// `identifier_type` is the modelled `WirelessGatewayIdType`; an unknown value
    /// resolves to nothing (the boundary validator already refuses one the enum does
    /// not list).
    pub fn find_gateway(
        &self,
        identifier_type: &str,
        identifier: &str,
    ) -> Option<&WirelessGateway> {
        match identifier_type {
            "WirelessGatewayId" => self.wireless_gateways.get(identifier),
            "GatewayEui" => self
                .gateway_eui_index
                .get(identifier)
                .and_then(|id| self.wireless_gateways.get(id)),
            "ThingName" => self
                .wireless_gateways
                .values()
                .find(|gateway| gateway.thing_name.as_deref() == Some(identifier)),
            _ => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotwirelessIotwirelessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotwirelessIotwirelessState>)
}
