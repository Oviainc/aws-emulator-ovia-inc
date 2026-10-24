// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-snowball-awsiesnowballjobmanagementservice service.
//!
//! Besides the state struct itself this module holds the helpers the job and address
//! handlers share — id minting in the modelled formats, the record-to-modelled-shape
//! conversions and the list cursor codec — so the handlers agree on one implementation
//! of each instead of carrying a copy apiece that can drift apart.

use std::collections::HashMap;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Address, DeviceConfiguration, JobListEntry, JobMetadata, JobResource, Notification,
    OnDeviceServiceConfiguration, PickupDetails, ShippingDetails, TaxDocuments,
};

/// What a handler says when the engine it was constructed with does not carry this
/// service's store.
///
/// The router always builds the emulator on `engine::InMemoryEngine`, so this is
/// unreachable there; a handler must still answer an error rather than construct a
/// private engine, whose writes nobody else would ever see. This service's model
/// declares no server-side error on any of these operations, so it travels on the
/// framework's refusal channel (`Scaffolded::validation`) rather than as a 501, which
/// would claim the operation is unimplemented.
pub const STATE_UNAVAILABLE: &str =
    "the Snowball emulator's state store is not available on this engine";

/// The page size `ListJobs` uses when the caller declares none.
///
/// `MaxResults` is modelled `range 0..100` (`smithy_query(query="operation",
/// operation="ListJobs")`), so 100 is the largest page the member admits.
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The most addresses this store holds.
///
/// Bounds the memory one emulator process can be made to hold through repeated
/// `CreateAddress` calls: an `Address` carries eleven strings, each modelled at most
/// 1,024 characters, so a full store is a few tens of megabytes. `CreateAddress` refuses
/// past it rather than growing without limit. Nothing in this API deletes an address, so
/// the ceiling is the only thing that bounds the store.
pub const MAX_STORED_ADDRESSES: usize = 1_000;

/// The most jobs this store holds.
///
/// Bounds the same way for `CreateJob`, which persists every supplied input against the
/// job it mints. `CancelJob` moves a job to `Cancelled` but — as in Snowball itself — does
/// not remove it, so the ceiling is what keeps repeated job creation from growing the
/// process without limit.
pub const MAX_STORED_JOBS: usize = 1_000;

/// The lifecycle states a job can be in: the whole `JobState` enum
/// (`shared_types::JOB_STATE_VALUES`), so the cancellation rule below is expressed
/// over the modelled set rather than over the two states this slice can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    New,
    PreparingAppliance,
    PreparingShipment,
    InTransitToCustomer,
    WithCustomer,
    InTransitToAWS,
    WithAWSSortingFacility,
    WithAWS,
    InProgress,
    Complete,
    Cancelled,
    Listing,
    Pending,
}

impl JobState {
    /// This state's wire value, exactly as `JobState` spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::PreparingAppliance => "PreparingAppliance",
            Self::PreparingShipment => "PreparingShipment",
            Self::InTransitToCustomer => "InTransitToCustomer",
            Self::WithCustomer => "WithCustomer",
            Self::InTransitToAWS => "InTransitToAWS",
            Self::WithAWSSortingFacility => "WithAWSSortingFacility",
            Self::WithAWS => "WithAWS",
            Self::InProgress => "InProgress",
            Self::Complete => "Complete",
            Self::Cancelled => "Cancelled",
            Self::Listing => "Listing",
            Self::Pending => "Pending",
        }
    }

    /// Whether `CancelJob` is permitted in this state.
    ///
    /// The model documents the window on `CancelJob`: "You can only cancel a job before
    /// its `JobState` value changes to `PreparingAppliance`" — so `New`, the state a job
    /// is created in, is the only one from which a job can still be cancelled.
    pub fn is_cancellable(self) -> bool {
        matches!(self, Self::New)
    }
}

/// One stored shipping address: every `Address` member this service keeps, so
/// `DescribeAddress` rebuilds the submitted address instead of echoing a request.
///
/// `PrefectureOrDistrict` and `Landmark` are deliberately absent: the model documents
/// both as "This field is no longer used and the value is ignored", so a submitted
/// value is not stored and not reported back.
#[derive(Debug, Clone, Default)]
pub struct StoredAddress {
    pub address_id: String,
    pub name: Option<String>,
    pub company: Option<String>,
    pub street1: Option<String>,
    pub street2: Option<String>,
    pub street3: Option<String>,
    pub city: Option<String>,
    pub state_or_province: Option<String>,
    pub country: Option<String>,
    pub postal_code: Option<String>,
    pub phone_number: Option<String>,
    pub is_restricted: bool,
    pub address_type: Option<String>,
}

impl StoredAddress {
    /// This record as the modelled `Address`, including the id the service minted.
    pub fn to_modeled(&self) -> Address {
        Address {
            address_id: Some(self.address_id.clone()),
            name: self.name.clone(),
            company: self.company.clone(),
            street1: self.street1.clone(),
            street2: self.street2.clone(),
            street3: self.street3.clone(),
            city: self.city.clone(),
            state_or_province: self.state_or_province.clone(),
            prefecture_or_district: None,
            landmark: None,
            country: self.country.clone(),
            postal_code: self.postal_code.clone(),
            phone_number: self.phone_number.clone(),
            is_restricted: self.is_restricted,
            r#type: self.address_type.clone(),
        }
    }
}

/// One stored job: the state the service minted for it plus every `CreateJob` input it
/// was given, so `DescribeJob` and `ListJobs` report what was persisted.
///
/// A record is either a plain job, the *main* job of an export order, or one of that
/// order's *job parts* — the three things this API's job ids name. [`Self::job_part_ids`]
/// is what tells them apart.
#[derive(Debug, Clone)]
pub struct StoredJob {
    pub job_id: String,
    pub job_state: JobState,
    pub job_type: Option<String>,
    pub snowball_type: Option<String>,
    pub snowball_capacity_preference: Option<String>,
    pub shipping_option: Option<String>,
    pub creation_date: SystemTime,
    pub description: Option<String>,
    pub address_id: Option<String>,
    pub forwarding_address_id: Option<String>,
    pub role_arn: Option<String>,
    pub kms_key_arn: Option<String>,
    pub resources: Option<JobResource>,
    pub notification: Option<Notification>,
    pub on_device_service_configuration: Option<OnDeviceServiceConfiguration>,
    /// The cluster this job is a node of. Always `None` for now: `CreateCluster` is
    /// still a 501 stub, so no cluster exists for a job to join and `CreateJob` refuses
    /// every `ClusterId` it is given. The field is where that id will live once the
    /// cluster operations are implemented.
    pub cluster_id: Option<String>,
    pub long_term_pricing_id: Option<String>,
    pub tax_documents: Option<TaxDocuments>,
    pub device_configuration: Option<DeviceConfiguration>,
    pub remote_management: Option<String>,
    pub impact_level: Option<String>,
    pub pickup_details: Option<PickupDetails>,
    /// This job's job parts, in the order they were created: the device-bearing jobs an
    /// export order was split into. Empty for a job part and for every job that is not an
    /// export order's main job.
    pub job_part_ids: Vec<String>,
}

impl StoredJob {
    /// Whether this record is a main job.
    ///
    /// `JobListEntry$IsMaster`: "A main job represents a successful request to create an
    /// export job. Main jobs aren't associated with any Snowballs. Instead, each main job
    /// will have at least one job part, and each job part is associated with a Snowball."
    /// Holding at least one job part is therefore exactly what makes a job a main job,
    /// which is why this is derived from [`Self::job_part_ids`] rather than stored beside
    /// it: a main job with no parts, or a part-bearing job that reports itself as a plain
    /// job, cannot be represented.
    pub fn is_master(&self) -> bool {
        !self.job_part_ids.is_empty()
    }

    /// This record as the modelled `JobMetadata` `DescribeJob` reports.
    ///
    /// The members that describe work no device has done yet are absent rather than
    /// zeroed: `DataTransferProgress` is documented as available only while a job is
    /// `InProgress`, `JobLogInfo` only once a transfer has produced a report, and
    /// `SnowballId` only once a device is associated with the job — which for a main job
    /// is never ("Main jobs aren't associated with any Snowballs") and for every other
    /// job, job parts included, is once an appliance has been prepared for it, i.e. once
    /// it has left `New`.
    pub fn to_job_metadata(&self) -> JobMetadata {
        JobMetadata {
            job_id: Some(self.job_id.clone()),
            job_state: Some(self.job_state.as_wire().to_string()),
            job_type: self.job_type.clone(),
            snowball_type: self.snowball_type.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            resources: self.resources.clone(),
            description: self.description.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            role_arn: self.role_arn.clone(),
            address_id: self.address_id.clone(),
            // The shipping speed the job was created with. Both shipments are absent
            // until a device actually moves: a job in `New` has no tracking number.
            shipping_details: self.shipping_option.clone().map(|option| ShippingDetails {
                shipping_option: Some(option),
                inbound_shipment: None,
                outbound_shipment: None,
            }),
            snowball_capacity_preference: self.snowball_capacity_preference.clone(),
            notification: self.notification.clone(),
            data_transfer_progress: None,
            job_log_info: None,
            cluster_id: self.cluster_id.clone(),
            forwarding_address_id: self.forwarding_address_id.clone(),
            tax_documents: self.tax_documents.clone(),
            device_configuration: self.device_configuration.clone(),
            remote_management: self.remote_management.clone(),
            long_term_pricing_id: self.long_term_pricing_id.clone(),
            on_device_service_configuration: self.on_device_service_configuration.clone(),
            impact_level: self.impact_level.clone(),
            pickup_details: self.pickup_details.clone(),
            snowball_id: None,
        }
    }

    /// This record as the modelled `JobListEntry` `ListJobs` pages over.
    pub fn to_job_list_entry(&self) -> JobListEntry {
        JobListEntry {
            job_id: Some(self.job_id.clone()),
            job_state: Some(self.job_state.as_wire().to_string()),
            // The main job of an export order is reported as the main job; its job parts
            // and every plain job are not (`JobListEntry$IsMaster`).
            is_master: self.is_master(),
            job_type: self.job_type.clone(),
            snowball_type: self.snowball_type.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            description: self.description.clone(),
        }
    }
}

/// The region this request was signed for: the SigV4 credential scope is the only region
/// a request carries.
///
/// An empty string when the request carries no scope to read a region from — a request
/// this emulator cannot place in any Region, which `CreateAddress` treats as a Region
/// with no recorded shipping area rather than as an unserviceable one.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_default()
}

/// Mint an id in the `<prefix><8-4-4-4-12 lowercase hex>` form this service's modelled
/// id patterns admit.
///
/// The 32 hex digits are the leading 16 bytes of a SHA-256 over the store's own
/// monotonic sequence number and the instant the record is created, so an id is the
/// service's own, is distinct from every other id this process mints, and is not a count
/// a caller can read a resource total out of.
fn mint_id(prefix: &str, seq: u64, now: SystemTime) -> String {
    let nanos = now
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(seq.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = ::hex::encode(hasher.finalize());
    format!(
        "{prefix}{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// The `ListJobs` cursor for a page that starts at `job_id`.
///
/// Hex of the id: the token carries nothing but the key the next page resumes at, and a
/// cursor is only ever handed back unchanged. It is not a capability — `ListJobs`
/// resumes only from a job the store actually holds, and refuses anything else — so the
/// encoding's job is to keep the token out of a caller's business logic, not to be
/// unguessable.
pub fn encode_cursor(job_id: &str) -> String {
    ::hex::encode(job_id.as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None` when
/// the token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// Per-service state for com-amazonaws-snowball-awsiesnowballjobmanagementservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState {
    /// Every address `CreateAddress` stored, keyed by the `AddressId` it minted.
    pub addresses: HashMap<String, StoredAddress>,
    /// Every job `CreateJob` stored, keyed by the `JobId` it minted.
    pub jobs: HashMap<String, StoredJob>,
    /// The ids in `jobs` in creation order, so `ListJobs` pages deterministically.
    pub job_order: Vec<String>,
    /// How many addresses this store has ever minted an id for, and how many jobs: the
    /// per-store sequence [`mint_id`] hashes, so two records never share an id.
    next_address_seq: u64,
    next_job_seq: u64,
}

impl ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState {
    /// Whether this store can hold another address, i.e. is below
    /// [`MAX_STORED_ADDRESSES`].
    pub fn can_store_another_address(&self) -> bool {
        self.addresses.len() < MAX_STORED_ADDRESSES
    }

    /// Whether this store can hold `additional` more jobs without passing
    /// [`MAX_STORED_JOBS`].
    ///
    /// An order is stored whole or not at all — an export order is a main job plus its
    /// job parts — so the ceiling is asked about the whole order rather than one record.
    pub fn can_store_jobs(&self, additional: usize) -> bool {
        self.jobs.len() + additional <= MAX_STORED_JOBS
    }

    /// Mint the next `AddressId`: the 40-character form the modelled pattern
    /// `^ADID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` admits.
    pub fn mint_address_id(&mut self) -> String {
        self.next_address_seq += 1;
        mint_id("ADID", self.next_address_seq, SystemTime::now())
    }

    /// Mint the next `JobId`: the 39-character form the modelled pattern
    /// `^(M|J)ID[0-9a-f]{8}-…$` admits, in the `JID` form a job associated with a device
    /// carries — a plain job, or one job part of an export order.
    pub fn mint_job_id(&mut self) -> String {
        self.next_job_seq += 1;
        mint_id("JID", self.next_job_seq, SystemTime::now())
    }

    /// Mint the next main job's `JobId`: the same 39-character form in the `MID` variant
    /// the modelled pattern's `M` alternative is for — the id of the main job an export
    /// order's job parts hang off, which is associated with no device.
    pub fn mint_main_job_id(&mut self) -> String {
        self.next_job_seq += 1;
        mint_id("MID", self.next_job_seq, SystemTime::now())
    }

    /// Store `address` under its minted id, or answer `false` when this store is at
    /// [`MAX_STORED_ADDRESSES`].
    ///
    /// The ceiling is enforced HERE, at the insertion point, so a handler cannot exceed
    /// it by forgetting to ask [`Self::can_store_another_address`] first.
    pub fn insert_address(&mut self, address: StoredAddress) -> bool {
        if !self.can_store_another_address() {
            return false;
        }
        self.addresses.insert(address.address_id.clone(), address);
        true
    }

    /// Store a whole order as the newest jobs — one plain job, or an export order's main
    /// job followed by its job parts — or answer `false`, storing nothing, when this store
    /// cannot hold all of it without passing [`MAX_STORED_JOBS`].
    ///
    /// The ceiling is enforced HERE, at the insertion point, and over the whole order, so
    /// a create can neither exceed it by forgetting to ask [`Self::can_store_jobs`] first
    /// nor leave a main job behind with the parts it needs unstored.
    pub fn insert_job_order(&mut self, jobs: Vec<StoredJob>) -> bool {
        if !self.can_store_jobs(jobs.len()) {
            return false;
        }
        for job in jobs {
            self.job_order.push(job.job_id.clone());
            self.jobs.insert(job.job_id.clone(), job);
        }
        true
    }

    /// The job parts of `job_id`, in the order they were created, or an empty slice's
    /// worth when that job has none (a job part, or any job that is not a main job).
    ///
    /// A part id a main job names always resolves: nothing in this API removes a job.
    pub fn job_parts(&self, job_id: &str) -> Vec<&StoredJob> {
        self.jobs
            .get(job_id)
            .map(|job| {
                job.job_part_ids
                    .iter()
                    .filter_map(|part_id| self.jobs.get(part_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every stored job in creation order — the order `ListJobs` pages over.
    pub fn jobs_in_creation_order(&self) -> Vec<&StoredJob> {
        self.job_order
            .iter()
            .filter_map(|job_id| self.jobs.get(job_id))
            .collect()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSnowballAwsiesnowballjobmanagementserviceState>)
}
