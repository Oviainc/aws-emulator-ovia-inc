// SPDX-License-Identifier: CC0-1.0

//! Stored buckets and the access keys that grant programmatic access to them,
//! together with the one object store a bucket's objects are provisioned in.

use super::*;

/// The `BucketState$code` of a bucket that is serving, from that member's own
/// documentation: "`OK` - The bucket is in a running state." (its only other
/// value, `Unknown`, says "Creation of the bucket might have timed-out", which no
/// bucket this store holds did).
pub const BUCKET_STATE_OK: &str = "OK";

/// `Bucket$objectVersioning` when versioning was asked for, from that member's
/// own documentation: "`Enabled` - Object versioning is enabled."
pub const OBJECT_VERSIONING_ENABLED: &str = "Enabled";

/// `Bucket$objectVersioning` when versioning was enabled and then turned off,
/// from that member's own documentation: "`Suspended` - Object versioning was
/// previously enabled but is currently suspended."
///
/// What `UpdateBucket` records for a request asking to suspend versioning of the
/// bucket's objects — see [`object_versioning_from_update`].
pub const OBJECT_VERSIONING_SUSPENDED: &str = "Suspended";

/// `Bucket$objectVersioning` when versioning was never asked for, from that
/// member's own documentation: "`NeverEnabled` - Object versioning has never been
/// enabled."
pub const OBJECT_VERSIONING_NEVER_ENABLED: &str = "NeverEnabled";

/// The `AccessRules$getObject` of a new bucket: the modelled `AccessType` value
/// `private`, which that member documents as "Sets all objects in the bucket to
/// private, making them readable only by you or anyone you give access to" — the
/// permission a bucket has until `UpdateBucket` widens it.
pub const ACCESS_TYPE_PRIVATE: &str = "private";

/// The `SetResourceAccessForBucket$access` value that gives a Lightsail resource
/// access to a bucket, from that member's own documentation: "`allow` - Allows
/// access to the bucket and its objects."
pub const RESOURCE_BUCKET_ACCESS_ALLOW: &str = "allow";

/// The `SetResourceAccessForBucket$access` value that takes that access away
/// again, from that member's own documentation: "`deny` - Denies access to the
/// bucket and its objects. Use this setting to remove access for a resource
/// previously set to allow."
pub const RESOURCE_BUCKET_ACCESS_DENY: &str = "deny";

/// The `AccountLevelBpaSync$status` this emulator reports, from that member's own
/// documentation: "`NeverSynced` - Synchronization has not yet happened." Nothing
/// here fetches an Amazon S3 account-level block-public-access configuration, so
/// no synchronization has happened and none is claimed.
pub const ACCOUNT_LEVEL_BPA_NEVER_SYNCED: &str = "NeverSynced";

/// The `AccountLevelBpaSync$message` that goes with [`ACCOUNT_LEVEL_BPA_NEVER_SYNCED`],
/// from the modelled `BPAStatusMessage` documentation: "`SYNC_ON_HOLD` - The
/// synchronization has not yet happened. This status message occurs immediately
/// after you create your first Lightsail bucket."
pub const ACCOUNT_LEVEL_BPA_SYNC_ON_HOLD: &str = "SYNC_ON_HOLD";

/// The `AccessKey$status` of a bucket access key that is valid, from that
/// member's own documentation: "A status of `Active` means that the key is valid,
/// while `Inactive` means it is not." One of the two values of the modelled
/// `StatusType` enum the member targets.
pub const ACCESS_KEY_STATUS_ACTIVE: &str = "Active";

/// The `AccessKeyLastUsed$region` and `$serviceName` of an access key nothing has
/// signed a request with, from those members' own documentation: "This value is
/// `N/A` if the access key has not been used."
pub const ACCESS_KEY_LAST_USED_NEVER: &str = "N/A";

/// The most access keys one bucket may hold, from the modelled `AccessKey`
/// documentation: "You can have a maximum of two access keys per bucket."
///
/// An AWS limit, not an emulator guard: a create past it is refused and mints
/// nothing.
pub const MAX_BUCKET_ACCESS_KEYS: usize = 2;

/// The most buckets this emulator will hold, across every Region together.
///
/// Emulator resource guard on the store as a whole, NOT an AWS quota (which is
/// per Region and per account). A create that would exceed it is refused with
/// the modelled `InvalidInputException`.
pub const MAX_BUCKETS: usize = 100;

/// How many buckets one `GetBuckets` page carries.
///
/// CHOSEN: Lightsail publishes no page size for this operation, only that the
/// response carries a `nextPageToken` when more buckets remain.
pub const BUCKET_PAGE_SIZE: usize = 10;

/// The URL of the bucket `name`, as announced to a caller that reached this
/// emulator at `endpoint` (its `Host`/`:authority`).
///
/// `Bucket$url` is "The URL of the bucket", so it has to be an address the caller
/// can actually reach — and what a caller reaches a Lightsail bucket WITH is an
/// S3 client signed with one of the bucket's access keys ("Access keys grant full
/// programmatic access to the specified bucket and its objects"). AWS publishes
/// that address as a DNS name in the Amazon S3 zone that backs the bucket; this
/// emulator owns no DNS zone, and a literal AWS hostname would send the next
/// client out of the emulator entirely (see [`request_endpoint`]), so the address
/// is the PATH-STYLE S3 address of the bucket under the endpoint this request
/// arrived at — `https://<endpoint>/<bucket>` — which is exactly where this
/// emulator's Amazon S3 data plane answers for the bucket this service
/// provisioned there (see [`claim_bucket_object_store`]).
///
/// No Region in the address: a bucket's name alone addresses it in the one object
/// namespace an S3 endpoint serves, which is also why that name has to be free in
/// it before a bucket can be created under it.
///
/// `https`, because the emulator serves one TLS listener
/// (`emulator_server::serve_emulator_until_shutdown`) and no plaintext one, so
/// that is the scheme its own endpoint answers on.
pub fn bucket_url(endpoint: &str, name: &str) -> String {
    format!("https://{endpoint}/{name}")
}

/// What this service records in the Amazon S3 store as the provisioner of a
/// bucket's object store, so a reader of the account's OWN S3 buckets can leave
/// it out (AWS holds a Lightsail bucket in a service-managed account, not in the
/// caller's).
pub const BUCKET_OBJECT_STORE_PROVISIONER: &str = ARN_SERVICE;

/// Claim the object store of a bucket named `name` in `region`: stand the bucket
/// up in this emulator's ONE object store, Amazon S3's own, so the bucket's
/// objects can be written and read with the access keys this service mints for
/// it. `versioned` is whether the bucket was created with object versioning
/// enabled, which is a property of how that store keeps the bucket's objects.
///
/// A Lightsail bucket IS an S3-compatible object store, and the thing that serves
/// the S3 object protocol in this emulator is the Amazon S3 service. So a bucket
/// this service creates is provisioned there rather than copied into a second
/// object map of this service's own: `PutObject`, `GetObject`, `ListObjectsV2`
/// and `DeleteObject` against the bucket are then served from the one store, and
/// what this service reports about the bucket's contents is read from the same
/// place.
///
/// Answers what the claim did ([`BucketCreation::NameTaken`] when the one bucket
/// namespace already holds that name, so the caller refuses the create rather
/// than taking over a bucket that is not its own), or `None` when that store
/// cannot be reached at all.
///
/// LOCK ORDER: this takes the S3 store's lock and holds no other while it does,
/// and it hands back an owned value rather than a guard — so it must be called
/// with NO guard of this service's store held. Keeping the cross-service access
/// one-directional and guard-free is what makes a lock cycle between the two
/// stores unreachable (the S3 service never reaches into this one).
pub fn claim_bucket_object_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    region: &str,
    versioned: bool,
) -> Option<BucketCreation> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    let claimed = store.provision_bucket(name, region, BUCKET_OBJECT_STORE_PROVISIONER);
    // `Bucket$objectVersioning` is `Enabled` when the create asked for versioning of the
    // bucket's objects, and that is what the OBJECT store has to do about it: "All
    // objects added to the bucket receive a unique version ID". A bucket created without
    // it keeps no versions, which is the state a fresh bucket is in there.
    //
    // The bucket is there: this guard is still held from the claim that just recorded it.
    // A store that nevertheless does not hold it would make the control plane report
    // versioning its data plane does not do, so say so rather than passing it over.
    if versioned && claimed == BucketCreation::Created {
        match store.bucket_mut(name) {
            Some(bucket) => bucket.set_versioning(
                ::service_com_amazonaws_s3_amazons3::state::VERSIONING_ENABLED_STATUS,
                None,
            ),
            None => ::tracing::warn!(
                bucket_name = %name,
                "the object store did not hold the bucket it had just recorded, so object \
                 versioning was not enabled on it"
            ),
        }
    }
    Some(claimed)
}

/// Release the object store of the bucket named `name`, with every object it
/// still held: what a delete of the bucket does, because "when you delete your
/// bucket, the bucket name is released and can be reused for a new bucket"
/// (`DeleteBucket`).
///
/// `true` when the object store held such a bucket, `false` when it did not, and
/// `None` when that store cannot be reached at all. The same LOCK ORDER as
/// [`claim_bucket_object_store`].
pub fn release_bucket_object_store<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
) -> Option<bool> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    Some(store.remove_bucket(name))
}

/// Record the object versioning of the bucket named `name` in its object store:
/// what makes the bucket's objects actually be kept as versions, or stop being
/// kept, once `UpdateBucket` has changed the bucket's `objectVersioning`.
///
/// The control plane's value and the object store's are two halves of ONE
/// setting — "All objects added to the bucket receive a unique version ID" — so a
/// change to the first is carried to the second the way
/// [`claim_bucket_object_store`] carries a create's.
///
/// `true` when the object store held such a bucket, `false` when it did not, and
/// `None` when that store cannot be reached at all. The same LOCK ORDER as
/// [`claim_bucket_object_store`].
pub fn set_bucket_object_store_versioning<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
    versioned: bool,
) -> Option<bool> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    let Some(bucket) = store.bucket_mut(name) else {
        return Some(false);
    };
    bucket.set_versioning(
        if versioned {
            ::service_com_amazonaws_s3_amazons3::state::VERSIONING_ENABLED_STATUS
        } else {
            ::service_com_amazonaws_s3_amazons3::state::VERSIONING_SUSPENDED_STATUS
        },
        None,
    );
    Some(true)
}

/// How many object keys the bucket named `name` holds in its object store — the
/// keys with a current object plus the keys only noncurrent versions are left of,
/// since a stored version is an object the bucket still holds.
///
/// KEYS, not versions: a key several versions are stored of counts once here. What
/// the bucket's storage metrics are over is the versions themselves and their
/// bytes — see [`bucket_object_storage`].
///
/// A pure READ of the object store, for the one thing this service's control
/// plane has to know about a bucket's contents: whether the bucket is empty, which
/// is what decides a `DeleteBucket` without `forceDelete`. A name the object store
/// holds no bucket for holds no objects, and answers `0`: there is nothing there to
/// block a delete.
///
/// `None` when the object store cannot be reached at all. The same LOCK ORDER as
/// [`claim_bucket_object_store`].
pub fn bucket_object_count<Eng: Send + Sync + 'static>(engine: &Eng, name: &str) -> Option<usize> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    Some(
        store
            .bucket(name)
            .map_or(0, |bucket| bucket.version_keys().len()),
    )
}

/// What a bucket's object store holds, measured over every object version it
/// keeps: the two quantities a bucket's storage metrics are computed from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BucketStorage {
    /// How many objects the bucket holds, every stored version counted —
    /// a noncurrent version is an object the bucket still holds.
    ///
    /// Delete markers are NOT among them: a marker holds no object and no bytes,
    /// which is why the object store keeps it as a version of its own
    /// (`::service_com_amazonaws_s3_amazons3::state::StoredVersion`).
    pub objects: u64,
    /// How many bytes those objects' payloads are, summed over the same set.
    pub bytes: u64,
}

/// Measure what the bucket named `name` holds in its object store right now:
/// the object versions it keeps and the bytes they are.
///
/// The one place this service measures a bucket's contents, for the storage
/// metrics `GetBucketMetricData` reports — the same object store
/// [`bucket_object_count`] reads whether the bucket is empty from, because that
/// store is the only place that knows what a bucket holds.
///
/// A bucket this emulator holds no in-progress multipart upload PARTS for
/// contributes none: the object store records an upload's settled members but no
/// part of one, so there is nothing of a part to count or to size.
///
/// A name the object store holds no bucket for holds nothing, and measures zero.
/// A pure READ: `None` only when that store cannot be reached at all. The same
/// LOCK ORDER as [`claim_bucket_object_store`].
pub fn bucket_object_storage<Eng: Send + Sync + 'static>(
    engine: &Eng,
    name: &str,
) -> Option<BucketStorage> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let store = state.lock();
    let Some(bucket) = store.bucket(name) else {
        return Some(BucketStorage::default());
    };
    let mut measured = BucketStorage::default();
    for key in bucket.version_keys() {
        for version in bucket.object_versions(key) {
            if let Some(object) = version.as_object() {
                measured.objects = measured.objects.saturating_add(1);
                measured.bytes = measured
                    .bytes
                    .saturating_add(u64::try_from(object.body.len()).unwrap_or(u64::MAX));
            }
        }
    }
    Some(measured)
}

/// The ARN of the bucket `id` in `region`. A bucket is regional, so its ARN
/// carries the region it was created in, and the resource portion mirrors the
/// `Instance/<uuid>` form the modelled `Instance$arn` documentation shows with
/// this resource's own `ResourceType` value.
pub fn bucket_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{BUCKET_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The `objectVersioning` a bucket reports for the `enableObjectVersioning` its
/// create request carried: an absent flag is not a request to version, which is
/// the `NeverEnabled` case.
pub fn object_versioning_from_request(enable_object_versioning: Option<bool>) -> &'static str {
    if enable_object_versioning.unwrap_or(false) {
        OBJECT_VERSIONING_ENABLED
    } else {
        OBJECT_VERSIONING_NEVER_ENABLED
    }
}

/// The two values an update may ask a bucket's object versioning to become, from
/// the `UpdateBucketRequest$versioning` documentation: "`Enabled` - Enables
/// versioning of objects in the specified bucket" and "`Suspended` - Suspends
/// versioning of objects in the specified bucket".
///
/// `NeverEnabled` is deliberately NOT among them: it is the state of a bucket
/// versioning was never asked for, not something a request can ask a bucket to
/// go back to.
pub const BUCKET_VERSIONING_REQUEST_VALUES: &[&str] =
    &[OBJECT_VERSIONING_ENABLED, OBJECT_VERSIONING_SUSPENDED];

/// The `objectVersioning` a bucket reports once an update asked for
/// `versioning`, or `None` when that is not one of the values such a request may
/// carry ([`BUCKET_VERSIONING_REQUEST_VALUES`]).
///
/// The answer is the catalog's own spelling, so what the bucket reports is a
/// modelled value rather than however the request spelled it.
pub fn object_versioning_from_update(versioning: &str) -> Option<&'static str> {
    BUCKET_VERSIONING_REQUEST_VALUES
        .iter()
        .find(|value| **value == versioning)
        .copied()
}

/// How many digits an Amazon Web Services account ID is: the emulator's own
/// account (`engine::emulator_account_id`) is one of them, and so is every
/// account a caller can name in a request.
pub const AWS_ACCOUNT_ID_DIGITS: usize = 12;

/// Whether `value` is an Amazon Web Services account ID — [`AWS_ACCOUNT_ID_DIGITS`]
/// decimal digits and nothing else.
///
/// The one place this crate decides that, for every member that takes an account
/// of its own rather than a resource of this one.
pub fn is_aws_account_id(value: &str) -> bool {
    value.len() == AWS_ACCOUNT_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The account-level block-public-access synchronization status `GetBuckets`
/// reports.
///
/// This emulator fetches nothing from Amazon S3, so the honest answer is the
/// never-synchronized one: no `lastSyncedAt` (the member documents "This value is
/// null when the account-level BPA configuration has not been synchronized"), and
/// account-level BPA therefore affects no Lightsail bucket here.
pub fn account_level_bpa_sync() -> AccountLevelBpaSync {
    AccountLevelBpaSync {
        status: Some(ACCOUNT_LEVEL_BPA_NEVER_SYNCED.to_string()),
        last_synced_at: None,
        message: Some(ACCOUNT_LEVEL_BPA_SYNC_ON_HOLD.to_string()),
        bpa_impacts_lightsail: Some(false),
    }
}

/// One access key of a bucket: what `CreateBucketAccessKey` minted and
/// `GetBucketAccessKeys` reports.
///
/// The key's SECRET is deliberately NOT a field here. Both the id and the secret
/// are minted by the emulator's credential store, which is the only thing that
/// creates credential material, and the secret "is returned only in response to
/// the `CreateBucketAccessKey` action" — so the create hands it to its caller and
/// this record keeps only the id, exactly as AWS keeps no retrievable copy.
#[derive(Debug, Clone)]
pub struct StoredBucketAccessKey {
    /// The id the credential store minted, which is this key's identity and the
    /// `accessKeyId` every read reports.
    pub access_key_id: String,
    /// One of the modelled `StatusType` values — see [`ACCESS_KEY_STATUS_ACTIVE`].
    pub status: String,
    pub created_at_ms: i64,
}

impl StoredBucketAccessKey {
    /// This record as the modelled `AccessKey`, for a read of the bucket's keys.
    ///
    /// No `secretAccessKey`: `GetBucketAccessKeys` "does not return the secret
    /// access key value of an access key", and nothing here holds one to return.
    ///
    /// `lastUsed` reports the never-used form the member documents (`N/A` region
    /// and service name, null `lastUsedDate`): a request signed with this key is
    /// served by the emulator's Amazon S3 data plane, which keeps no record of
    /// WHICH credential signed it — nothing in this emulator records the use of a
    /// credential — so there is no use instant to report, and claiming one for
    /// work nothing recorded would be worse than saying the key has not been used.
    pub fn view(&self) -> AccessKey {
        AccessKey {
            access_key_id: Some(self.access_key_id.clone()),
            secret_access_key: None,
            status: Some(self.status.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            last_used: Some(AccessKeyLastUsed {
                last_used_date: None,
                region: Some(ACCESS_KEY_LAST_USED_NEVER.to_string()),
                service_name: Some(ACCESS_KEY_LAST_USED_NEVER.to_string()),
            }),
        }
    }

    /// This record as the modelled `AccessKey` answered by the create that minted
    /// it: the one response that carries `secret_access_key`, and the one that
    /// carries NO `lastUsed` — the modelled `AccessKeyLastUsed` documents that
    /// "this object does not include data in the response of a
    /// `CreateBucketAccessKey` action".
    pub fn created_view(&self, secret_access_key: String) -> AccessKey {
        AccessKey {
            secret_access_key: Some(secret_access_key),
            last_used: None,
            ..self.view()
        }
    }
}

/// One stored bucket: the object-storage resource `CreateBucket` created.
///
/// What the caller asked for — name, bundle, tags, whether to version objects —
/// is kept verbatim; the ARN, support code, creation time, URL and state are
/// assigned by this store from the request plus its own allocators.
#[derive(Debug, Clone)]
pub struct StoredBucket {
    /// The bucket name the request named, kept verbatim — it is half of this
    /// record's store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The bundle currently applied, an id of [`BUCKET_BUNDLE_SPECS`].
    pub bundle_id: String,
    /// One of [`OBJECT_VERSIONING_ENABLED`], [`OBJECT_VERSIONING_SUSPENDED`] or
    /// [`OBJECT_VERSIONING_NEVER_ENABLED`].
    pub object_versioning: String,
    /// When this bucket's bundle was last changed; `None` on one that has never
    /// been moved. A bucket's bundle can be updated only once within a monthly
    /// billing cycle, so the instant of that change is what says whether another
    /// one is available — see [`StoredBucket::able_to_update_bundle`].
    pub bundle_updated_at_ms: Option<i64>,
    /// The anonymous access permissions of the bucket's objects.
    pub access_rules: AccessRules,
    /// Accounts given read-only access to the bucket.
    pub readonly_access_accounts: Vec<String>,
    /// The Lightsail instances `SetResourceAccessForBucket` gave access to the
    /// bucket.
    pub resources_receiving_access: Vec<ResourceReceivingAccess>,
    /// Where the bucket's access logs are written, and whether they are written
    /// at all.
    pub access_log_config: BucketAccessLogConfig,
    /// The cross-origin resource sharing rules of the bucket.
    pub cors_rules: Vec<BucketCorsRule>,
    /// The access keys `CreateBucketAccessKey` minted for this bucket, in the
    /// order they were created — at most [`MAX_BUCKET_ACCESS_KEYS`] of them.
    pub access_keys: Vec<StoredBucketAccessKey>,
    /// One of the modelled `BucketState$code` values — see [`BUCKET_STATE_OK`].
    pub state_code: String,
}

impl StoredBucket {
    /// This record as the modelled `Bucket`, announced to a caller that reached
    /// this emulator at `endpoint` (its `Host`/`:authority`, as
    /// [`request_endpoint`] reads it).
    ///
    /// `url` is the one member that depends on the request: it is the address the
    /// caller is told to reach this bucket on, so it has to be the endpoint THIS
    /// request arrived at (see [`bucket_url`]). A request that names no host at
    /// all names no endpoint to announce, so `url` is omitted rather than filled
    /// with an address nothing serves.
    ///
    /// `include_connected_resources` and `include_cors` are the `GetBuckets`
    /// flags of the same names: `resourcesReceivingAccess` is reported only when
    /// the caller asked for the connected resources, and `cors` only when it
    /// asked for the CORS configuration ("This field is only included in the
    /// response when CORS configuration is requested", per `Bucket$cors`).
    pub fn view(
        &self,
        endpoint: Option<&str>,
        include_connected_resources: bool,
        include_cors: bool,
    ) -> Bucket {
        Bucket {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_BUCKET.to_string()),
            tags: Some(self.tags.clone()),
            bundle_id: Some(self.bundle_id.clone()),
            url: endpoint.map(|endpoint| bucket_url(endpoint, &self.name)),
            object_versioning: Some(self.object_versioning.clone()),
            able_to_update_bundle: Some(self.able_to_update_bundle(now_ms())),
            access_rules: Some(self.access_rules.clone()),
            readonly_access_accounts: Some(self.readonly_access_accounts.clone()),
            resources_receiving_access: include_connected_resources
                .then(|| self.resources_receiving_access.clone()),
            state: Some(BucketState {
                code: Some(self.state_code.clone()),
                // `BucketState$message` "describes the state of the bucket"; a
                // running bucket has nothing to explain.
                message: None,
            }),
            access_log_config: Some(self.access_log_config.clone()),
            cors: include_cors.then(|| BucketCorsConfig {
                rules: Some(self.cors_rules.clone()),
            }),
        }
    }

    /// Whether the one bundle change this bucket's monthly billing cycle allows
    /// is available at `now_ms` — the `ableToUpdateBundle` a read reports.
    pub fn able_to_update_bundle(&self, now_ms: i64) -> bool {
        bundle_change_available(self.bundle_updated_at_ms, now_ms)
    }

    /// Give the resource `name` of kind `resource_type` access to this bucket
    /// (`allow`), or take that access away again (`deny`).
    ///
    /// A resource either receives access or it does not, so this is a SET keyed
    /// on the resource's name: a grant to a resource that already has access
    /// adds no second entry, and a revoke of one that has none removes nothing.
    /// `SetResourceAccessForBucket$access` documents `deny` as exactly that
    /// inverse — "use this setting to remove access for a resource previously
    /// set to allow" — so one call of each kind is enough to reach either state
    /// and repeating it changes nothing. Entries are kept in the order access
    /// was given, so every read reports the same sequence; the set is bounded by
    /// the resources this store holds, since a name appears in it at most once.
    ///
    /// Answers whether the set CHANGED, so a caller that has to log what it
    /// changed (the release a resource's removal performs — see
    /// [`ComAmazonawsLightsailLightsail20161128State::remove_instance`]) can tell
    /// a grant it released from one that was never held.
    pub fn set_resource_access(&mut self, name: &str, resource_type: &str, allow: bool) -> bool {
        let held = self
            .resources_receiving_access
            .iter()
            .position(|resource| resource.name.as_deref() == Some(name));
        match (allow, held) {
            (true, None) => {
                self.resources_receiving_access
                    .push(ResourceReceivingAccess {
                        name: Some(name.to_string()),
                        resource_type: Some(resource_type.to_string()),
                    });
                true
            }
            (false, Some(index)) => {
                self.resources_receiving_access.remove(index);
                true
            }
            // Already in the state the request asks for: nothing to change.
            (true, Some(_)) | (false, None) => false,
        }
    }
}
