// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-sagemakeredge-amazonsagemakeredge service.
//!
//! Edge Manager's control plane — device fleets, registered devices and edge deployment
//! plans — lives in the `sagemaker` service, not in this data-plane service. So this
//! store holds the records the three data-plane operations read and write, plus the seam
//! a control plane (or a fixture) uses to put them there:
//!
//! * a fleet holds its devices and the deployments seeded for the fleet,
//! * a device holds the registration document recorded for it, the values its agent last
//!   reported through `SendHeartbeat`, and the deployment results it acknowledged.
//!
//! `GetDeployments` for a device is the fleet's deployments minus the ones that device
//! has already acknowledged, so an acknowledged deployment is never handed back.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{Definition, DeploymentModel, DeploymentResult, EdgeMetric, Model};

/// How long the agent is told to cache a registration answer, in seconds, as the string
/// `CacheTTL` is modelled with.
///
/// AWS documents no value for this ("The amount of time, in seconds, that the
/// registration status is stored on the device's cache before it is refreshed"), so this
/// is the emulator's own choice; it is recorded on the device record at registration, and
/// a control plane or fixture that dictates another value through [`register_device`]
/// overrides it.
///
/// [`register_device`]: ComAmazonawsSagemakeredgeAmazonsagemakeredgeState::register_device
pub const DEFAULT_CACHE_TTL_SECONDS: &str = "300";

/// The `Status` the registration document of a device this service holds carries.
pub const REGISTERED: &str = "REGISTERED";

/// The `Status` the registration document of a device this service does not hold carries.
pub const NOT_REGISTERED: &str = "NOT_REGISTERED";

/// Bounds on request-keyed growth. Every map in this store is keyed by a name a request
/// chooses, so each one is capped and the store REFUSES to grow past its cap instead of
/// growing for as long as a caller keeps inventing names. These are the emulator's own
/// bounds (AWS's adjustable Edge Manager quotas are not modelled), set far above any
/// plausible fixture: the fleet set, a fleet's device set, a fleet's seeded deployments,
/// and one device's accumulated acknowledgements.
pub const MAX_FLEETS: usize = 64;
/// See [`MAX_FLEETS`].
pub const MAX_DEVICES_PER_FLEET: usize = 256;
/// See [`MAX_FLEETS`].
pub const MAX_DEPLOYMENTS_PER_FLEET: usize = 64;
/// See [`MAX_FLEETS`].
pub const MAX_DEPLOYMENT_RESULTS_PER_DEVICE: usize = 256;

/// A record the store refuses to create because it would grow past one of the bounds
/// above. A handler answers it as a refusal, never by growing anyway.
#[derive(Debug, Clone)]
pub struct CapacityExceeded {
    /// Which bound was reached, and for which name.
    pub message: String,
}

impl CapacityExceeded {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl ::std::fmt::Display for CapacityExceeded {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Per-service state for com-amazonaws-sagemakeredge-amazonsagemakeredge. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSagemakeredgeAmazonsagemakeredgeState {
    /// fleet name -> fleet record.
    fleets: HashMap<String, Fleet>,
}

/// One device fleet: its devices, and the deployments seeded for the fleet.
#[derive(Debug, Default)]
pub struct Fleet {
    /// The fleet's name, as the agent names it on the wire.
    pub name: String,
    /// device name -> device record. Devices are per-fleet: the same device name in two
    /// fleets is two devices.
    pub devices: HashMap<String, Device>,
    /// The deployments outstanding for this fleet, in the order they were seeded. Each is
    /// handed to every device of the fleet until that device acknowledges it.
    pub deployments: Vec<Deployment>,
}

/// One device of a fleet.
#[derive(Debug, Clone)]
pub struct Device {
    /// The device's name, unique within its fleet.
    pub name: String,
    /// The fleet this device belongs to.
    pub fleet_name: String,
    /// Returned verbatim by `GetDeviceRegistration`; recorded when the record is created
    /// and never recomputed, so two reads answer identically.
    pub registration: String,
    /// Returned verbatim by `GetDeviceRegistration` as `CacheTTL`.
    pub cache_ttl: String,
    /// When this device's record was created.
    pub registered_at: SystemTime,
    /// The agent version the last `SendHeartbeat` reported.
    pub agent_version: Option<String>,
    /// When the last `SendHeartbeat` arrived.
    pub last_heartbeat_at: Option<SystemTime>,
    /// The models the last `SendHeartbeat` reported as loaded on the device.
    pub models: Vec<Model>,
    /// The agent operating metrics the last `SendHeartbeat` reported.
    pub agent_metrics: Vec<EdgeMetric>,
    /// deployment name -> the result the agent reported for it. A deployment with an
    /// entry here has been acknowledged by this device.
    pub deployment_results: HashMap<String, DeploymentResultRecord>,
}

/// One deployment seeded for a fleet, as `GetDeployments` reports it.
#[derive(Debug, Clone, Default)]
pub struct Deployment {
    /// Unique within the fleet; the key an agent acknowledges the deployment by.
    pub deployment_name: String,
    /// `DeploymentType` (see `DEPLOYMENT_TYPE_VALUES`).
    pub deployment_type: Option<String>,
    /// `FailureHandlingPolicy` (see `FAILURE_HANDLING_POLICY_VALUES`).
    pub failure_handling_policy: Option<String>,
    /// What the deployment asks the device to do, model by model.
    pub definitions: Vec<Definition>,
}

/// The result an agent reported for one deployment: its acknowledgement of that
/// deployment.
#[derive(Debug, Clone, Default)]
pub struct DeploymentResultRecord {
    /// `DeploymentStatus` as the agent reported it (see `DEPLOYMENT_STATUS_VALUES`).
    pub status: Option<String>,
    /// The agent's free-text explanation of the status.
    pub status_message: Option<String>,
    /// When the agent started the deployment.
    pub start_time: Option<::aws_smithy_types::DateTime>,
    /// When the agent finished the deployment.
    pub end_time: Option<::aws_smithy_types::DateTime>,
    /// The per-model outcome the agent reported.
    pub models: Vec<DeploymentModel>,
}

impl DeploymentResultRecord {
    /// Record what the agent reported in a `SendHeartbeat` `DeploymentResult`.
    ///
    /// `DeploymentName` is the key this record is stored under, so it is not duplicated
    /// in the record itself.
    pub fn from_reported(result: DeploymentResult) -> Self {
        Self {
            status: result.deployment_status,
            status_message: result.deployment_status_message,
            start_time: result.deployment_start_time,
            end_time: result.deployment_end_time,
            models: result.deployment_models.unwrap_or_default(),
        }
    }
}

impl Device {
    /// A device record, as both registration paths create it.
    fn new(
        fleet_name: &str,
        device_name: &str,
        registration: String,
        cache_ttl: String,
        registered_at: SystemTime,
    ) -> Self {
        Self {
            name: device_name.to_string(),
            fleet_name: fleet_name.to_string(),
            registration,
            cache_ttl,
            registered_at,
            agent_version: None,
            last_heartbeat_at: None,
            models: Vec::new(),
            agent_metrics: Vec::new(),
            deployment_results: HashMap::new(),
        }
    }

    /// Whether a result for `deployment_name` can be recorded on this device without
    /// growing its acknowledgements past [`MAX_DEPLOYMENT_RESULTS_PER_DEVICE`]. Replacing
    /// the result of a deployment already acknowledged always can: it grows nothing.
    pub fn has_room_for_result(&self, deployment_name: &str) -> bool {
        self.deployment_results.contains_key(deployment_name)
            || self.deployment_results.len() < MAX_DEPLOYMENT_RESULTS_PER_DEVICE
    }
}

/// What one `SendHeartbeat` reported about a device, as the store records it.
#[derive(Debug, Clone)]
pub struct Heartbeat {
    /// The agent version reported.
    pub agent_version: String,
    /// When the heartbeat arrived.
    pub at: SystemTime,
    /// The models the agent reported as loaded, which replace the ones held.
    pub models: Vec<Model>,
    /// The agent metrics reported, which replace the ones held.
    pub agent_metrics: Vec<EdgeMetric>,
    /// `(DeploymentName, the result reported for it)` — the device's acknowledgement.
    pub deployment_result: Option<(String, DeploymentResultRecord)>,
}

impl ComAmazonawsSagemakeredgeAmazonsagemakeredgeState {
    /// The fleet record named `fleet_name`, CREATING it when this is the first time the
    /// fleet is named — unless that would grow the fleet set past [`MAX_FLEETS`].
    ///
    /// Private, and named for the insert it performs: only the three write paths below
    /// (`register_device`, `record_heartbeat`, `seed_deployment`) may reach it, so a read
    /// path cannot create an empty fleet and break the invariant that a read records
    /// nothing. A reader uses [`Self::fleet`] or [`Self::device`].
    fn fleet_or_create(&mut self, fleet_name: &str) -> Result<&mut Fleet, CapacityExceeded> {
        if !self.fleets.contains_key(fleet_name) && self.fleets.len() >= MAX_FLEETS {
            return Err(CapacityExceeded::new(format!(
                "this emulator holds its bound of {MAX_FLEETS} device fleets, \
                 so device fleet {fleet_name:?} cannot be recorded"
            )));
        }
        Ok(self
            .fleets
            .entry(fleet_name.to_string())
            .or_insert_with(|| Fleet {
                name: fleet_name.to_string(),
                ..Fleet::default()
            }))
    }

    /// The fleet named `fleet_name`, or `None` when no fleet of that name is held.
    pub fn fleet(&self, fleet_name: &str) -> Option<&Fleet> {
        self.fleets.get(fleet_name)
    }

    /// The device `device_name` of fleet `fleet_name`, or `None` when it is not held.
    pub fn device(&self, fleet_name: &str, device_name: &str) -> Option<&Device> {
        self.fleets.get(fleet_name)?.devices.get(device_name)
    }

    /// The device record of `(fleet_name, device_name)`, created by `create` when the
    /// fleet does not hold it yet. `create` is called ONLY for a record that is really
    /// created, and only after both bounds leave room for it — the device bound is
    /// checked BEFORE the fleet record is created, so a refused call leaves the store
    /// exactly as it was rather than leaving an empty fleet behind.
    fn device_entry(
        &mut self,
        fleet_name: &str,
        device_name: &str,
        create: impl FnOnce() -> Device,
    ) -> Result<&mut Device, CapacityExceeded> {
        if self.fleets.get(fleet_name).is_some_and(|fleet| {
            !fleet.devices.contains_key(device_name)
                && fleet.devices.len() >= MAX_DEVICES_PER_FLEET
        }) {
            return Err(CapacityExceeded::new(format!(
                "device fleet {fleet_name:?} holds this emulator's bound of \
                 {MAX_DEVICES_PER_FLEET} devices, so device {device_name:?} cannot be recorded"
            )));
        }
        Ok(self
            .fleet_or_create(fleet_name)?
            .devices
            .entry(device_name.to_string())
            .or_insert_with(create))
    }

    /// Record a device of a fleet with the registration document and cache TTL given, and
    /// hand back its record. Creates the fleet when needed; leaves an existing device
    /// untouched, so a registration document is recorded once and read back unchanged.
    ///
    /// This is the seam the Edge Manager control plane (`sagemaker`'s device-fleet APIs)
    /// or a fixture registers a device through — the data-plane operations in `src/ops/`
    /// never mint a registration document on a read.
    pub fn register_device(
        &mut self,
        fleet_name: &str,
        device_name: &str,
        registration: impl Into<String>,
        cache_ttl: impl Into<String>,
    ) -> Result<&mut Device, CapacityExceeded> {
        self.device_entry(fleet_name, device_name, || {
            Device::new(
                fleet_name,
                device_name,
                registration.into(),
                cache_ttl.into(),
                SystemTime::now(),
            )
        })
    }

    /// Record what one `SendHeartbeat` reported against its device, creating the device
    /// record (with this service's own registration document and cache TTL) when the
    /// agent is reporting it for the first time.
    ///
    /// Every bound is checked BEFORE anything is written, so a refused heartbeat leaves
    /// the store exactly as it was.
    pub fn record_heartbeat(
        &mut self,
        fleet_name: &str,
        device_name: &str,
        region: &str,
        heartbeat: Heartbeat,
    ) -> Result<(), CapacityExceeded> {
        if let Some((deployment_name, _)) = heartbeat.deployment_result.as_ref() {
            if self
                .device(fleet_name, device_name)
                .is_some_and(|device| !device.has_room_for_result(deployment_name))
            {
                return Err(CapacityExceeded::new(format!(
                    "device {device_name:?} of device fleet {fleet_name:?} holds this \
                     emulator's bound of {MAX_DEPLOYMENT_RESULTS_PER_DEVICE} deployment \
                     results, so the result for {deployment_name:?} cannot be recorded"
                )));
            }
        }

        let device = self.device_entry(fleet_name, device_name, || {
            // Minted only for a record that is really created, so a later heartbeat never
            // rewrites the document a reader has already cached, and the document's
            // RegisteredAt is the instant the record carries.
            let registered_at = SystemTime::now();
            Device::new(
                fleet_name,
                device_name,
                registration_document(fleet_name, device_name, region, registered_at),
                DEFAULT_CACHE_TTL_SECONDS.to_string(),
                registered_at,
            )
        })?;

        // The heartbeat reports the device's CURRENT state, so the reported model and
        // metric lists replace the ones held. Deployment results instead accumulate (and
        // are bounded above): an acknowledgement is a fact about a past deployment.
        device.agent_version = Some(heartbeat.agent_version);
        device.last_heartbeat_at = Some(heartbeat.at);
        device.models = heartbeat.models;
        device.agent_metrics = heartbeat.agent_metrics;
        if let Some((deployment_name, result)) = heartbeat.deployment_result {
            device.deployment_results.insert(deployment_name, result);
        }
        Ok(())
    }

    /// Seed a deployment into a fleet, creating the fleet when needed. A deployment whose
    /// name the fleet already carries is replaced, so the name stays unique in the fleet
    /// and a replacement grows nothing; a new one is refused past
    /// [`MAX_DEPLOYMENTS_PER_FLEET`].
    ///
    /// The seam that stands in for `sagemaker`'s CreateEdgeDeploymentPlan: `GetDeployments`
    /// only ever reports what is seeded here.
    pub fn seed_deployment(
        &mut self,
        fleet_name: &str,
        deployment: Deployment,
    ) -> Result<(), CapacityExceeded> {
        // The deployment bound is checked BEFORE the fleet record is created, as
        // `device_entry` checks the device bound: a refused seed leaves the store exactly
        // as it was rather than leaving an empty fleet behind that would also consume one
        // of the MAX_FLEETS slots.
        if self.fleets.get(fleet_name).is_some_and(|held| {
            !held
                .deployments
                .iter()
                .any(|d| d.deployment_name == deployment.deployment_name)
                && held.deployments.len() >= MAX_DEPLOYMENTS_PER_FLEET
        }) {
            return Err(CapacityExceeded::new(format!(
                "device fleet {fleet_name:?} holds this emulator's bound of \
                 {MAX_DEPLOYMENTS_PER_FLEET} deployments, so deployment {:?} \
                 cannot be seeded",
                deployment.deployment_name
            )));
        }

        let fleet = self.fleet_or_create(fleet_name)?;
        match fleet
            .deployments
            .iter_mut()
            .find(|held| held.deployment_name == deployment.deployment_name)
        {
            Some(held) => *held = deployment,
            // The check above left room for this one: either the fleet is new (so it
            // holds no deployment at all) or it holds fewer than the bound.
            None => fleet.deployments.push(deployment),
        }
        Ok(())
    }

    /// The deployments still outstanding for one device: the deployments of its fleet,
    /// in seeded order, minus every one that device has already acknowledged with a
    /// `SendHeartbeat` `DeploymentResult`.
    ///
    /// A device this service does not hold has nothing outstanding — no deployment is
    /// invented for it.
    pub fn outstanding_deployments(&self, fleet_name: &str, device_name: &str) -> Vec<Deployment> {
        let Some(fleet) = self.fleets.get(fleet_name) else {
            return Vec::new();
        };
        let Some(device) = fleet.devices.get(device_name) else {
            return Vec::new();
        };
        fleet
            .deployments
            .iter()
            .filter(|deployment| {
                !device
                    .deployment_results
                    .contains_key(&deployment.deployment_name)
            })
            .cloned()
            .collect()
    }
}

/// The registration document recorded for a device this service holds: the device's
/// identity in this emulator, the instant its record was created, and its status.
///
/// Recorded once, when the device record is created, and returned verbatim afterwards.
pub fn registration_document(
    fleet_name: &str,
    device_name: &str,
    region: &str,
    registered_at: SystemTime,
) -> String {
    let arn = ::engine::emulator_arn(
        "sagemaker",
        region,
        &format!("device-fleet/{fleet_name}/device/{device_name}"),
    );
    ::serde_json::json!({
        "Status": REGISTERED,
        "DeviceName": device_name,
        "DeviceFleetName": fleet_name,
        "DeviceArn": arn,
        "RegisteredAt": iso8601(registered_at),
    })
    .to_string()
}

/// The registration document for a device this service does not hold: the identity that
/// was asked about, and the fact that it is not registered.
pub fn unregistered_document(fleet_name: &str, device_name: &str) -> String {
    ::serde_json::json!({
        "Status": NOT_REGISTERED,
        "DeviceName": device_name,
        "DeviceFleetName": fleet_name,
    })
    .to_string()
}

/// `instant` as an ISO-8601 instant, the form a Smithy `date-time` renders in.
fn iso8601(instant: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// The region a request was signed for, which this service's device ARNs carry.
///
/// Lives here, beside the ARN and timestamp helpers, rather than in an operation file.
/// An unsigned request names no region, so the emulator's default (`us-east-1`, the
/// region `engine::emulator_arn` is documented against) stands in.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSagemakeredgeAmazonsagemakeredgeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSagemakeredgeAmazonsagemakeredgeState>)
}
