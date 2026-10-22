// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-securitylake-securitylake service.
//!
//! Every value the data-lake and subscriber operations answer with is read back out of
//! the structs below, and every ARN is minted through the helpers here so that
//! `CreateDataLake`/`CreateSubscriber` and the readers that follow them cannot drift.

use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

#[allow(unused_imports)]
use crate::ops::types::*;

/// The Region an unsigned request is attributed to. A signed request carries its own
/// Region in the SigV4 credential scope; see [`caller_region`].
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `sigv4Name` of this service (`smithy_query service_info`).
const ARN_SERVICE: &str = "securitylake";

/// `DataLakeStatus` members, from `DATA_LAKE_STATUS_VALUES` in `ops::types::shared_types`
/// (the model's `com.amazonaws.securitylake#DataLakeStatus` enum).
pub const DATA_LAKE_STATUS_INITIALIZED: &str = "INITIALIZED";
/// See [`DATA_LAKE_STATUS_INITIALIZED`].
pub const DATA_LAKE_STATUS_COMPLETED: &str = "COMPLETED";

/// `SubscriberStatus` member, from `SUBSCRIBER_STATUS_VALUES` in `ops::types::shared_types`.
pub const SUBSCRIBER_STATUS_ACTIVE: &str = "ACTIVE";

/// `AccessType` members, from `ACCESS_TYPE_VALUES` in `ops::types::shared_types`.
pub const ACCESS_TYPE_S3: &str = "S3";
/// See [`ACCESS_TYPE_S3`].
pub const ACCESS_TYPE_LAKEFORMATION: &str = "LAKEFORMATION";

/// How long a freshly created data lake stays `INITIALIZED` before the clock owes it
/// `COMPLETED`.
///
/// CHOSEN, not documented: AWS publishes no duration for the create to finish. The span
/// is deliberately non-zero so a caller that reads straight after `CreateDataLake` still
/// observes the transient `INITIALIZED` status, and it is stored as a due instant rather
/// than slept on. Public so a test can key its timing off the same value.
pub const DATA_LAKE_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// What Security Lake answers a subscriber-scoped call with when the account has Security
/// Lake enabled in NO Region at all.
///
/// The wire REQUIREMENT is the pairing `ResourceNotFoundException` / HTTP 404 that
/// `smithy_query operation="ListSubscribers"` lists for every subscriber operation; this
/// string is the documented remedy wording that goes with it ("enable Security Lake, then
/// try again") and is held in one place so the gate reads the same from every handler.
pub const NOT_ENABLED_IN_ANY_REGION_MESSAGE: &str = "The request failed because Security Lake isn't enabled for your account in any Regions. Enable Security Lake for your account and then try again.";

/// The most data lakes this emulator holds at once.
///
/// A BOUND the emulator enforces, not a number AWS publishes. `DataLakeConfiguration$region`
/// is a caller-supplied string, so the store would otherwise grow for as long as a client
/// keeps inventing Region names. Security Lake holds at most one data lake per Region and
/// AWS runs far fewer Regions than this, so no legitimate client reaches the bound;
/// `CreateDataLake` refuses past it instead of growing (and may still UPDATE a Region it
/// already holds, which adds no entry).
pub const MAX_DATA_LAKES: usize = 64;

/// The most subscribers this emulator holds per Region.
///
/// A BOUND the emulator enforces, not a number AWS publishes (it is in the same order as
/// the per-Region subscriber quota Security Lake documents). `CreateSubscriber` mints a
/// new record on every call, so without a cap the store would grow for as long as a client
/// keeps calling; past the bound the call is refused instead. Deleting a subscriber frees
/// its slot.
pub const MAX_SUBSCRIBERS_PER_REGION: usize = 50;

/// One Security Lake data lake, as this emulator holds it. One per Region.
#[derive(Debug, Clone)]
pub struct DataLake {
    pub data_lake_arn: String,
    pub region: String,
    pub s3_bucket_arn: String,
    pub encryption_configuration: Option<DataLakeEncryptionConfiguration>,
    pub lifecycle_configuration: Option<DataLakeLifecycleConfiguration>,
    pub replication_configuration: Option<DataLakeReplicationConfiguration>,
    /// Stored but not yet surfaced: `DataLakeResource` carries neither, and the operations
    /// that report them — `UpdateDataLake` (which echoes the role) and
    /// `ListTagsForResource` — are still 501. Kept so those fixes read what the create
    /// persisted rather than inventing it.
    pub meta_store_manager_role_arn: String,
    pub create_status: String,
    /// The instant the `INITIALIZED` -> `COMPLETED` transition this create started falls
    /// due. [`ComAmazonawsSecuritylakeSecuritylakeState::settle`] materialises it.
    pub completes_at: SystemTime,
    pub update_status: Option<DataLakeUpdateStatus>,
    /// See [`DataLake::meta_store_manager_role_arn`]: surfaced by `ListTagsForResource`,
    /// which is still 501.
    pub tags: Vec<Tag>,
}

impl DataLake {
    /// The modelled `DataLakeResource` for this stored data lake.
    pub fn to_resource(&self) -> DataLakeResource {
        DataLakeResource {
            data_lake_arn: self.data_lake_arn.clone(),
            region: self.region.clone(),
            s3_bucket_arn: Some(self.s3_bucket_arn.clone()),
            encryption_configuration: self.encryption_configuration.clone(),
            lifecycle_configuration: self.lifecycle_configuration.clone(),
            replication_configuration: self.replication_configuration.clone(),
            create_status: Some(self.create_status.clone()),
            update_status: self.update_status.clone(),
        }
    }
}

/// One Security Lake subscriber, as this emulator holds it, identified by its
/// `subscriberId`.
#[derive(Debug, Clone)]
pub struct Subscriber {
    pub subscriber_id: String,
    pub subscriber_arn: String,
    /// The Region this subscriber was created in. Security Lake is regional, so a
    /// subscriber is only visible to callers of its own Region.
    pub region: String,
    pub subscriber_identity: AwsIdentity,
    pub subscriber_name: String,
    pub subscriber_description: Option<String>,
    pub sources: Vec<LogSourceResource>,
    pub access_types: Vec<String>,
    pub role_arn: Option<String>,
    pub s3_bucket_arn: Option<String>,
    pub subscriber_endpoint: Option<String>,
    pub subscriber_status: String,
    pub resource_share_arn: Option<String>,
    pub resource_share_name: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl Subscriber {
    /// The modelled `SubscriberResource` for this stored subscriber.
    pub fn to_resource(&self) -> SubscriberResource {
        SubscriberResource {
            subscriber_id: self.subscriber_id.clone(),
            subscriber_arn: self.subscriber_arn.clone(),
            subscriber_identity: self.subscriber_identity.clone(),
            subscriber_name: self.subscriber_name.clone(),
            subscriber_description: self.subscriber_description.clone(),
            sources: self.sources.clone(),
            // An absent list stays absent on the wire rather than becoming `[]`.
            access_types: (!self.access_types.is_empty()).then(|| self.access_types.clone()),
            role_arn: self.role_arn.clone(),
            s3_bucket_arn: self.s3_bucket_arn.clone(),
            subscriber_endpoint: self.subscriber_endpoint.clone(),
            subscriber_status: Some(self.subscriber_status.clone()),
            resource_share_arn: self.resource_share_arn.clone(),
            resource_share_name: self.resource_share_name.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }
}

/// Per-service state for com-amazonaws-securitylake-securitylake. A handler reaches it through [`service_state`].
///
/// Both collections are `Vec`s in CREATION ORDER — the stable order `ListSubscribers` pages
/// over — and both are capped ([`MAX_DATA_LAKES`], [`MAX_SUBSCRIBERS_PER_REGION`]), so the
/// linear lookups below are bounded by those caps rather than by anything a caller sends.
///
/// The account id is not held here: it is the one canonical emulator account,
/// `::engine::emulator_account_id()`, which every ARN helper below reads.
#[derive(Debug, Default)]
pub struct ComAmazonawsSecuritylakeSecuritylakeState {
    /// At most one data lake per Region, in creation order.
    data_lakes: Vec<DataLake>,
    /// Subscribers in creation order.
    subscribers: Vec<Subscriber>,
    /// Counter feeding [`ComAmazonawsSecuritylakeSecuritylakeState::new_subscriber_id`], so
    /// two ids minted in the same nanosecond still differ.
    subscriber_seq: u64,
}

impl ComAmazonawsSecuritylakeSecuritylakeState {
    /// Materialise every state change the CLOCK has already made due.
    ///
    /// Today that is exactly one: a data lake whose `INITIALIZED` -> `COMPLETED`
    /// transition fell due at or before `now`. It was owed at that instant whether or not
    /// anybody called, so a reporting operation may apply it before answering.
    pub fn settle(&mut self, now: SystemTime) {
        for lake in &mut self.data_lakes {
            if lake.create_status == DATA_LAKE_STATUS_INITIALIZED && now >= lake.completes_at {
                lake.create_status = DATA_LAKE_STATUS_COMPLETED.to_string();
            }
        }
    }

    /// How many data lakes are stored.
    pub fn data_lake_count(&self) -> usize {
        self.data_lakes.len()
    }

    /// Whether this account has Security Lake enabled in ANY Region.
    ///
    /// Enabling Security Lake in a Region IS creating that Region's data lake
    /// (`CreateDataLake`), and deleting the last one disables it again, so the stored
    /// data lakes are the whole of the account's enablement state. The subscriber
    /// operations consult this before serving: an account with no data lake anywhere has
    /// no subscriber collection to read or write.
    pub fn enabled_in_any_region(&self) -> bool {
        !self.data_lakes.is_empty()
    }

    /// Every stored data lake, in creation order.
    pub fn data_lakes(&self) -> &[DataLake] {
        &self.data_lakes
    }

    /// The stored data lake of `region`, if this emulator holds one.
    pub fn data_lake(&self, region: &str) -> Option<&DataLake> {
        self.data_lakes.iter().find(|lake| lake.region == region)
    }

    /// The stored data lake of `region`, for update.
    pub fn data_lake_mut(&mut self, region: &str) -> Option<&mut DataLake> {
        self.data_lakes.iter_mut().find(|lake| lake.region == region)
    }

    /// Insert a brand-new data lake for `region`, built from `configuration`.
    pub fn insert_data_lake(
        &mut self,
        configuration: &DataLakeConfiguration,
        meta_store_manager_role_arn: &str,
        tags: &[Tag],
        now: SystemTime,
    ) {
        let region = configuration.region.clone();
        self.data_lakes.push(DataLake {
            data_lake_arn: data_lake_arn(&region),
            s3_bucket_arn: s3_bucket_arn(&region),
            region,
            encryption_configuration: configuration.encryption_configuration.clone(),
            lifecycle_configuration: configuration.lifecycle_configuration.clone(),
            replication_configuration: configuration.replication_configuration.clone(),
            meta_store_manager_role_arn: meta_store_manager_role_arn.to_string(),
            create_status: DATA_LAKE_STATUS_INITIALIZED.to_string(),
            completes_at: now + DATA_LAKE_COMPLETION_DELAY,
            update_status: None,
            tags: tags.to_vec(),
        });
    }

    /// Remove the data lakes of `regions`, and the subscribers that subscribed to them, in
    /// one pass each. The creation order of what remains is preserved.
    pub fn remove_data_lakes(&mut self, regions: &HashSet<&str>) {
        self.data_lakes
            .retain(|lake| !regions.contains(lake.region.as_str()));
        self.subscribers
            .retain(|subscriber| !regions.contains(subscriber.region.as_str()));
    }

    /// The stored subscriber `subscriber_id` names, but only when it belongs to `region`.
    pub fn subscriber_in(&self, region: &str, subscriber_id: &str) -> Option<&Subscriber> {
        self.subscribers
            .iter()
            .find(|subscriber| subscriber.subscriber_id == subscriber_id)
            .filter(|subscriber| subscriber.region == region)
    }

    /// Every stored subscriber of `region`, in creation order.
    pub fn subscribers_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a Subscriber> + Clone {
        self.subscribers
            .iter()
            .filter(move |subscriber| subscriber.region == region)
    }

    /// Store `subscriber` at the end of the creation order.
    pub fn insert_subscriber(&mut self, subscriber: Subscriber) {
        self.subscribers.push(subscriber);
    }

    /// Remove the subscriber `subscriber_id` names from `region`, keeping the creation
    /// order of the rest. `false` when `region` holds no such subscriber.
    pub fn remove_subscriber(&mut self, region: &str, subscriber_id: &str) -> bool {
        let found = self.subscribers.iter().position(|subscriber| {
            subscriber.subscriber_id == subscriber_id && subscriber.region == region
        });
        match found {
            Some(index) => {
                self.subscribers.remove(index);
                true
            }
            None => false,
        }
    }

    /// A fresh `subscriberId` in the shape the model's `UUID` demands
    /// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), with RFC 9562's
    /// version-4 and variant nibbles set.
    ///
    /// The 16 bytes come from a SHA-256 digest (the `sha2` crate does the hashing) over a
    /// per-store counter, the creation instant in nanoseconds and the Region, so no two
    /// calls can produce the same id; this function only lays those bytes out in the
    /// canonical 8-4-4-4-12 form.
    pub fn new_subscriber_id(&mut self, now: SystemTime, region: &str) -> String {
        self.subscriber_seq += 1;
        let nanos = now
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default();

        let mut hasher = Sha256::new();
        hasher.update(self.subscriber_seq.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        hasher.update(region.as_bytes());
        let digest = hasher.finalize();

        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // RFC 9562 section 5.4: the version nibble is 4 and the variant bits are 0b10.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;

        let hex = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }
}

/// The modelled `ResourceNotFoundException` a subscriber-scoped call is refused with when
/// [`ComAmazonawsSecuritylakeSecuritylakeState::enabled_in_any_region`] is false.
///
/// `resourceName`/`resourceType` name ONE missing resource, and this precondition is about
/// the account rather than any single data lake, so both stay absent and the payload
/// carries only `message`.
pub fn not_enabled_in_any_region() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(NOT_ENABLED_IN_ANY_REGION_MESSAGE.to_string()),
        resource_name: None,
        resource_type: None,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecuritylakeSecuritylakeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecuritylakeSecuritylakeState>)
}

/// The Region this request was signed for, from its SigV4 credential scope.
///
/// Security Lake is a regional service and the emulator serves every Region on one
/// endpoint, so the scope is the only thing that says which Region a subscriber call
/// means. An unsigned request is attributed to [`DEFAULT_REGION`].
pub fn caller_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:securitylake:<region>:<account>:data-lake/default`.
pub fn data_lake_arn(region: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, "data-lake/default")
}

/// `arn:aws:securitylake:<region>:<account>:subscriber/<subscriberId>`.
pub fn subscriber_arn(region: &str, subscriber_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("subscriber/{subscriber_id}"))
}

/// `arn:aws:s3:::aws-security-data-lake-<region>-<suffix>` — the bucket Security Lake
/// creates for a Region's data lake. S3 is a global namespace, so the ARN has no Region
/// or account segment; the suffix is derived from the account and Region so the same
/// Region always reports the same bucket.
pub fn s3_bucket_arn(region: &str) -> String {
    format!(
        "arn:aws:s3:::aws-security-data-lake-{region}-{}",
        bucket_suffix(region)
    )
}

/// The IAM role Security Lake creates for an `S3`-access subscriber.
pub fn subscriber_role_arn(subscriber_id: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/AmazonSecurityLake-{subscriber_id}",
        ::engine::emulator_account_id()
    )
}

/// The RAM resource share Security Lake creates for a `LAKEFORMATION`-access subscriber.
pub fn resource_share_arn(region: &str, subscriber_id: &str) -> String {
    format!(
        "arn:aws:ram:{region}:{}:resource-share/{subscriber_id}",
        ::engine::emulator_account_id()
    )
}

/// The RAM resource share NAME for a `LAKEFORMATION`-access subscriber. The shape of this
/// string is fixed by `SubscriberResource$resourceShareName`'s pattern
/// (`^LakeFormation(?:-V[0-9]+)-([a-zA-Z0-9]+)-([\\\w\-_:/.@=+]*)$`).
pub fn resource_share_name(subscriber_name: &str) -> String {
    format!(
        "LakeFormation-V1-{}-{subscriber_name}",
        ::engine::emulator_account_id()
    )
}

/// The opaque `nextToken` that resumes `ListSubscribers` at `subscriber_id`, which sat at
/// `index` in the page order the token was issued from.
///
/// Carrying BOTH lets a resume stay exact while the anchor is still stored, and fall back
/// to the position it held when a `DeleteSubscriber` removed it between pages, rather than
/// breaking an in-flight paginator.
///
/// Hex, so the token is opaque to a caller and needs no escaping in the `nextToken` query
/// parameter.
pub fn encode_next_token(index: usize, subscriber_id: &str) -> String {
    hex::encode(format!("{index}:{subscriber_id}"))
}

/// The `(index, subscriber id)` an opaque `nextToken` names, or `None` when the token is
/// not one this service minted.
pub fn decode_next_token(token: &str) -> Option<(usize, String)> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (index, subscriber_id) = decoded.split_once(':')?;
    let index = index.parse::<usize>().ok()?;
    (!subscriber_id.is_empty()).then(|| (index, subscriber_id.to_string()))
}

/// A stable 12-character suffix for a Region's data-lake bucket, so the bucket ARN looks
/// like the random suffix AWS appends yet is the same on every read.
fn bucket_suffix(region: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(::engine::emulator_account_id().as_bytes());
    hasher.update(b"/");
    hasher.update(region.as_bytes());
    let digest = hex::encode(hasher.finalize());
    digest[..12].to_string()
}
