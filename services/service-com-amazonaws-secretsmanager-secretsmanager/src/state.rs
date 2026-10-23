// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-secretsmanager-secretsmanager service.
//!
//! The shape of the resource this service stores: a secret is identified by its
//! name, carries an ARN that ends in the name plus a hyphen and six random
//! characters, and owns an insertion-ordered list of versions. The staging
//! labels (`AWSCURRENT`, `AWSPREVIOUS`, and any label a caller names) live on
//! the versions, and a label is attached to at most one version of a secret at a
//! time; a version that carries no label is deprecated and is left out of
//! `VersionIdsToStages` / `SecretVersionsToStages`.
//!
//! Everything a handler needs to resolve, mutate or project a secret is a method
//! here, so the per-operation files hold only that operation's own rules.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{ReplicationStatusType, Tag};

/// The staging label on the current value of a secret.
pub const STAGE_CURRENT: &str = "AWSCURRENT";

/// The staging label that moves to the version `AWSCURRENT` was taken from.
pub const STAGE_PREVIOUS: &str = "AWSPREVIOUS";

/// Secrets Manager appends "a hyphen and six random characters after the secret
/// name at the end of the ARN" (`CreateSecretRequest$Name`, this service's
/// Smithy model).
pub const ARN_SUFFIX_LEN: usize = 6;

/// The recovery window `DeleteSecret` applies when the caller names none: "If
/// you don't use either, then by default Secrets Manager uses a 30 day recovery
/// window." (`DeleteSecretRequest$RecoveryWindowInDays`, this service's Smithy
/// model.)
pub const DEFAULT_RECOVERY_WINDOW_DAYS: i64 = 30;

/// "The number of days from 7 to 30 that Secrets Manager waits before
/// permanently deleting the secret." (`DeleteSecretRequest$RecoveryWindowInDays`,
/// this service's Smithy model.)
pub const MIN_RECOVERY_WINDOW_DAYS: i64 = 7;

/// The upper bound of the same documented range.
pub const MAX_RECOVERY_WINDOW_DAYS: i64 = 30;

/// Seconds in a day, for turning a recovery window into an instant.
const SECONDS_PER_DAY: u64 = 86_400;

/// How many secrets the store holds before `CreateSecret` refuses with the
/// modelled `LimitExceededException`.
///
/// Secrets Manager publishes an adjustable per-Region secret quota, and this
/// emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store, not a claim about any account's
/// limit. It bounds the WHOLE store rather than one region's share of it: the
/// region a secret is homed in comes from the request, so a per-region bound
/// would still let a caller grow the store without end by varying the region it
/// signs for. Every record it holds is released by `DeleteSecret`, and it sits
/// far above any workflow an emulator serves.
pub const SECRET_QUOTA: usize = 1_000;

/// How many versions one secret holds before a further `PutSecretValue` has to
/// make room by evicting a deprecated one.
///
/// Secrets Manager removes a version once it carries no staging label, so
/// nothing a caller can reach by stage depends on a deprecated version
/// surviving; keeping a bounded tail of them is what lets a deprecated
/// `VersionId` still be read for a while, as the real service does. The bound is
/// the emulator's own, and it is what stops a loop of `PutSecretValue` calls
/// against one secret from growing the store without end.
pub const VERSIONS_PER_SECRET: usize = 100;

/// The region a resource is placed in when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many tags one secret carries before a further `TagResource` is refused.
///
/// The AWS tagging quota is 50 tags per resource ("Service quotas for Tagging",
/// Amazon Web Services General Reference, referenced from `TagResource`'s own
/// documentation in this service's Smithy model). It is what stops a loop of
/// `TagResource` calls against one secret from growing the store without end.
pub const TAGS_PER_SECRET: usize = 50;

/// The prefix AWS reserves for its own tags: a caller may neither create nor
/// modify a tag whose key starts with it ("Service quotas for Tagging", Amazon
/// Web Services General Reference).
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// How many replica Regions one secret holds before a further replication
/// request is refused, and the longest `AddReplicaRegions` list a request may
/// carry.
///
/// A replica lives in an AWS Region, and the set of Regions is finite and
/// published — but `RegionType` is only a length-bounded pattern
/// (`^([a-z]+-)+\d+$`), and this emulator validates no Region catalogue, so a
/// caller could otherwise mint unbounded distinct Region strings and grow one
/// secret's replica list without end. This bound is therefore the emulator's
/// OWN, chosen above the number of Regions AWS publishes, and it is what keeps
/// the resident state (and the per-request walk in [`settle_replicas`]) finite.
/// `AddReplicaRegionListType` declares no maximum length, so the request's own
/// list is bounded by the same constant before anything is counted or stored.
///
/// [`settle_replicas`]: Secret::settle_replicas
pub const REPLICAS_PER_SECRET: usize = 64;

/// The KMS key a replica is encrypted with when the request names none: "The
/// ARN, key ID, or alias of the KMS key to encrypt the secret. If you don't
/// include this field, Secrets Manager uses `aws/secretsmanager`."
/// (`ReplicaRegionType$KmsKeyId`, this service's Smithy model.) The alias form
/// is what the service reports back in a `ReplicationStatusType`.
pub const DEFAULT_KMS_KEY_ALIAS: &str = "alias/aws/secretsmanager";

/// `ReplicationStatusType$Status` while the replica is being created
/// (`StatusType`, this service's Smithy model).
pub const REPLICA_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ReplicationStatusType$Status` once the replica exists.
pub const REPLICA_STATUS_IN_SYNC: &str = "InSync";

/// How long a fresh replica stays `InProgress` before the clock owes it the move
/// to `InSync`.
///
/// Secrets Manager documents no duration for creating a replica, so this span is
/// the emulator's OWN: long enough that a caller polling straight after
/// `ReplicateSecretToRegions` still observes `InProgress`, short enough that a
/// client waiting for `InSync` does not wait on a human timescale.
pub const REPLICA_SYNC_DELAY: Duration = Duration::from_millis(750);

/// The characters a secret name may carry besides ASCII letters and digits:
/// "The secret name can contain ASCII letters, numbers, and the following
/// characters: /_+=.@-" (`CreateSecretRequest$Name`, this service's Smithy
/// model). The model declares no pattern for `NameType`, so the documented
/// charset is enforced in the handler.
pub const SECRET_NAME_EXTRA_CHARS: &str = "/_+=.@-";

/// Whether `name` is a secret name this service accepts.
pub fn is_valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SECRET_NAME_EXTRA_CHARS.contains(c))
}

/// The region this request is addressed to, taken from its SigV4 credential
/// scope so an ARN reports the region the caller actually signed for.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Render a stored instant as the wire timestamp type.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The instant a recovery window of `days` started at `from` elapses.
///
/// TOTAL for any value a request can carry. `DeleteSecret` already refuses a
/// window outside the documented 7..30 range with the modelled
/// `InvalidParameterException`, so the clamp here is defence in depth; it keeps
/// the multiplication and the panicking `SystemTime + Duration` out of reach of
/// any caller, whatever a future caller of this function passes.
pub fn after_days(from: SystemTime, days: i64) -> SystemTime {
    let days = days.clamp(MIN_RECOVERY_WINDOW_DAYS, MAX_RECOVERY_WINDOW_DAYS);
    let window = ::std::time::Duration::from_secs(
        u64::try_from(days)
            .unwrap_or(0)
            .saturating_mul(SECONDS_PER_DAY),
    );
    from.checked_add(window).unwrap_or(from)
}

/// A fresh version id. A UUID satisfies `SecretVersionIdType`'s modelled length
/// bound of 32..64 characters.
pub fn new_version_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The six random characters that end a secret's ARN.
pub fn new_arn_suffix() -> String {
    // The alphabet Secrets Manager draws the ARN suffix from: ASCII letters and
    // digits.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    ::uuid::Uuid::new_v4().as_bytes()[..ARN_SUFFIX_LEN]
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// Encode a pagination cursor — the name of the first secret of the NEXT page —
/// as the opaque `NextToken` a client hands back unchanged.
pub fn encode_next_token(name: &str) -> String {
    ::hex::encode(name.as_bytes())
}

/// Read a cursor produced by [`encode_next_token`]. `None` for a token this
/// service did not mint, which the caller answers with
/// `InvalidNextTokenException`.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// A tag as stored: both halves are optional, exactly as the modelled `Tag`
/// shape has them, so a tag round-trips through state unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTag {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl StoredTag {
    /// The stored form of a modelled `Tag`.
    pub fn from_modeled(tag: &Tag) -> Self {
        Self {
            key: tag.key.clone(),
            value: tag.value.clone(),
        }
    }
}

/// One replica of a secret: the Region it is in, the KMS key it is encrypted
/// with, and the `StatusType` its creation has reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replica {
    pub region: String,
    pub kms_key_id: String,
    /// One of [`REPLICA_STATUS_IN_PROGRESS`] or [`REPLICA_STATUS_IN_SYNC`].
    pub status: String,
    /// The instant the `InProgress` -> `InSync` move the clock owes this replica
    /// becomes due; `None` once it has been made. Stored rather than slept on,
    /// so nothing blocks a request.
    pub in_sync_due: Option<SystemTime>,
}

impl Replica {
    /// A replica whose creation has just started in `region`.
    fn creating(region: String, kms_key_id: String, at: SystemTime) -> Self {
        Self {
            region,
            kms_key_id,
            status: REPLICA_STATUS_IN_PROGRESS.to_string(),
            in_sync_due: at.checked_add(REPLICA_SYNC_DELAY),
        }
    }

    /// Materialise the move to `InSync` once the clock has made it due. Returns
    /// whether anything changed, so a caller can tell a settled replica from an
    /// untouched one.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.in_sync_due.is_none_or(|due| due > now) {
            return false;
        }
        self.status = REPLICA_STATUS_IN_SYNC.to_string();
        self.in_sync_due = None;
        true
    }
}

/// How many tags a secret already carrying `existing` would carry once `tags`
/// were attached, which is what [`TAGS_PER_SECRET`] bounds: a key already
/// attached is replaced, not added, so it does not grow the list.
///
/// Linear in both lists: the keys go into a set rather than being rescanned per
/// input tag, so counting a long request list costs no more than reading it.
pub fn tag_count_after_attaching(existing: &[StoredTag], tags: &[Tag]) -> usize {
    let mut keys: BTreeSet<Option<&String>> = existing.iter().map(|tag| tag.key.as_ref()).collect();
    keys.extend(tags.iter().map(|tag| tag.key.as_ref()));
    keys.len()
}

/// How many replicas a secret already holding `existing` would hold once
/// `regions` were replicated to, which is what [`REPLICAS_PER_SECRET`] bounds: a
/// Region already replicated to is replaced, not added.
pub fn replica_count_after_adding(existing: &[Replica], regions: &[&str]) -> usize {
    let mut seen: BTreeSet<&str> = existing
        .iter()
        .map(|replica| replica.region.as_str())
        .collect();
    seen.extend(regions.iter().copied());
    seen.len()
}

/// One version of a secret: its value, and the staging labels pointing at it.
#[derive(Debug, Clone)]
pub struct SecretVersion {
    pub version_id: String,
    pub secret_string: Option<String>,
    pub secret_binary: Option<Vec<u8>>,
    pub version_stages: Vec<String>,
    pub created_date: SystemTime,
}

impl SecretVersion {
    /// Whether this version carries `stage`.
    pub fn has_stage(&self, stage: &str) -> bool {
        self.version_stages.iter().any(|held| held == stage)
    }

    /// Whether this version already holds exactly the value in a request — the
    /// test an `@idempotencyToken` replay turns on.
    pub fn holds_same_value(
        &self,
        secret_string: Option<&String>,
        secret_binary: Option<&Vec<u8>>,
    ) -> bool {
        self.secret_string.as_ref() == secret_string && self.secret_binary.as_ref() == secret_binary
    }

    /// The version's staging labels for the wire, omitted while it carries none
    /// (`SecretVersionStagesType` is modelled with a minimum length of 1).
    pub fn stages_out(&self) -> Option<Vec<String>> {
        (!self.version_stages.is_empty()).then(|| self.version_stages.clone())
    }
}

/// A stored secret: its metadata and every version it still holds.
#[derive(Debug, Clone)]
pub struct Secret {
    pub name: String,
    pub arn: String,
    pub region: String,
    pub description: Option<String>,
    pub kms_key_id: Option<String>,
    /// The `Type` of an externally-held secret; `None` for a secret this
    /// service stores itself.
    pub secret_type: Option<String>,
    /// The service that created the secret, for a service-linked secret;
    /// `None` for one a caller created.
    pub owning_service: Option<String>,
    pub tags: Vec<StoredTag>,
    pub created_date: SystemTime,
    pub last_changed_date: SystemTime,
    pub last_accessed_date: Option<SystemTime>,
    /// The instant the secret is scheduled to be permanently deleted;
    /// `Some(..)` means it is scheduled for deletion.
    pub deleted_date: Option<SystemTime>,
    pub rotation_enabled: bool,
    /// The replicas of this secret, in the order their Regions were first
    /// replicated to. Empty for a secret that does not participate in
    /// cross-region replication.
    pub replicas: Vec<Replica>,
    /// Versions in the order they were created.
    pub versions: Vec<SecretVersion>,
}

impl Secret {
    /// A new secret with no versions, owned by `region`.
    pub fn new(name: String, arn: String, region: String, at: SystemTime) -> Self {
        Self {
            name,
            arn,
            region,
            description: None,
            kms_key_id: None,
            secret_type: None,
            owning_service: None,
            tags: Vec::new(),
            created_date: at,
            last_changed_date: at,
            last_accessed_date: None,
            deleted_date: None,
            rotation_enabled: false,
            replicas: Vec::new(),
            versions: Vec::new(),
        }
    }

    /// Whether a `DeleteSecret` call has scheduled this secret for deletion.
    pub fn scheduled_for_deletion(&self) -> bool {
        self.deleted_date.is_some()
    }

    /// The version with this id, deprecated ones included.
    pub fn version(&self, version_id: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.version_id == version_id)
    }

    /// The one version carrying `stage`.
    pub fn version_with_stage(&self, stage: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.has_stage(stage))
    }

    /// Every version that still carries a staging label, mapped to its labels.
    /// A version with no label is deprecated and is left out, which is what
    /// makes this map the live staging state rather than a version history.
    pub fn versions_to_stages(&self) -> Option<HashMap<String, Vec<String>>> {
        let map: HashMap<String, Vec<String>> = self
            .versions
            .iter()
            .filter(|v| !v.version_stages.is_empty())
            .map(|v| (v.version_id.clone(), v.version_stages.clone()))
            .collect();
        (!map.is_empty()).then_some(map)
    }

    /// The secret's tags for the wire, omitted while it carries none.
    pub fn tags_out(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| {
            self.tags
                .iter()
                .map(|tag| Tag {
                    key: tag.key.clone(),
                    value: tag.value.clone(),
                })
                .collect()
        })
    }

    /// Attach `tags` to the secret, appending the keys it does not carry yet and
    /// replacing the value of each key it does.
    ///
    /// `TagResource` "appends tags to the existing list of tags", and a tag's
    /// key is its identity ("The key identifier, or name, of the tag",
    /// `Tag$Key`), so a key that is already attached is updated in place rather
    /// than attached twice. Tagging modifies the secret's metadata, which is
    /// what `LastChangedDate` reports ("The last date and time that this secret
    /// was modified in any way").
    pub fn attach_tags(&mut self, tags: &[Tag], at: SystemTime) {
        for tag in tags {
            let stored = StoredTag::from_modeled(tag);
            match self
                .tags
                .iter_mut()
                .find(|existing| existing.key == stored.key)
            {
                Some(existing) => existing.value = stored.value,
                None => self.tags.push(stored),
            }
        }
        self.last_changed_date = at;
    }

    /// Detach every tag whose key is named, leaving the rest in place.
    ///
    /// `UntagResource` "is idempotent. If a requested tag is not attached to the
    /// secret, no error is returned and the secret metadata is unchanged", so a
    /// key that matches nothing is not an error — and when nothing matches,
    /// `LastChangedDate` does not move either.
    pub fn detach_tags(&mut self, keys: &[String], at: SystemTime) {
        let before = self.tags.len();
        self.tags
            .retain(|tag| tag.key.as_ref().is_none_or(|key| !keys.contains(key)));
        if self.tags.len() != before {
            self.last_changed_date = at;
        }
    }

    /// How many tags this secret would carry once `tags` were attached, which is
    /// what [`TAGS_PER_SECRET`] bounds.
    pub fn tag_count_after(&self, tags: &[Tag]) -> usize {
        tag_count_after_attaching(&self.tags, tags)
    }

    /// The replica held for `region`, if any.
    pub fn replica(&self, region: &str) -> Option<&Replica> {
        self.replicas.iter().find(|replica| replica.region == region)
    }

    /// How many replicas this secret would hold once `regions` were replicated
    /// to, which is what [`REPLICAS_PER_SECRET`] bounds.
    pub fn replica_count_after(&self, regions: &[&str]) -> usize {
        replica_count_after_adding(&self.replicas, regions)
    }

    /// Record a replication attempt into `region`.
    ///
    /// The replica starts `InProgress`, which the clock moves to `InSync` (see
    /// [`settle_replicas`]); a Region that already holds a replica OF THIS
    /// SECRET is replicated again, which starts its creation over rather than
    /// adding a second entry for the same Region.
    ///
    /// Nothing here reports `Failed`: that status is for a replica the service
    /// could not create, and the conflict
    /// `ReplicateSecretToRegionsRequest$ForceOverwriteReplicaSecret` resolves
    /// ("whether to overwrite a secret with the same name in the destination
    /// Region") is a DIFFERENT secret of this name in the destination Region,
    /// which this store cannot hold — a name identifies at most one secret in
    /// it. A status for a failure nothing suffered would be fabricated, so the
    /// emulator reports none.
    ///
    /// [`settle_replicas`]: Secret::settle_replicas
    pub fn replicate_to(&mut self, region: &str, kms_key_id: Option<&str>, at: SystemTime) {
        let key = kms_key_id
            .filter(|key| !key.is_empty())
            .unwrap_or(DEFAULT_KMS_KEY_ALIAS)
            .to_string();
        let fresh = Replica::creating(region.to_string(), key, at);
        match self
            .replicas
            .iter_mut()
            .find(|replica| replica.region == region)
        {
            Some(existing) => *existing = fresh,
            None => self.replicas.push(fresh),
        }
    }

    /// Stop replicating to `region`, reporting whether a replica was held there.
    pub fn remove_replica(&mut self, region: &str) -> bool {
        let before = self.replicas.len();
        self.replicas.retain(|replica| replica.region != region);
        self.replicas.len() != before
    }

    /// Materialise every replica transition the clock has already made due,
    /// reporting whether any moved. This is the move from `InProgress` to
    /// `InSync` that was owed at this instant whether or not anybody called, so
    /// a reporting operation may perform it before it answers.
    pub fn settle_replicas(&mut self, now: SystemTime) -> bool {
        let mut settled = false;
        for replica in &mut self.replicas {
            settled |= replica.settle(now);
        }
        settled
    }

    /// The secret's replication view for the wire — one `ReplicationStatusType`
    /// per replica — omitted while the secret has no replicas.
    ///
    /// `StatusMessage` is "Status message such as 'Secret with this name already
    /// exists in this region'" — the reason a replica was not created, which
    /// nothing in this emulator produces (see [`replicate_to`]), so it is
    /// omitted. `LastAccessedDate` is "the date that the secret was last
    /// accessed in the Region. This field is omitted if the secret has never
    /// been retrieved in the Region": this emulator serves `GetSecretValue` from
    /// the primary, so no call has ever retrieved a replica in its own Region.
    ///
    /// [`replicate_to`]: Secret::replicate_to
    pub fn replication_status_out(&self) -> Option<Vec<ReplicationStatusType>> {
        (!self.replicas.is_empty()).then(|| self.replication_status())
    }

    /// The same view, as a list that is EMPTY rather than absent for a secret
    /// with no replicas — what `RemoveRegionsFromReplication` answers with,
    /// since its `ReplicationStatus` is "the status of replicas for this secret
    /// after you remove Regions" and "none left" is that status.
    pub fn replication_status(&self) -> Vec<ReplicationStatusType> {
        self.replicas
            .iter()
            .map(|replica| ReplicationStatusType {
                region: Some(replica.region.clone()),
                kms_key_id: Some(replica.kms_key_id.clone()),
                status: Some(replica.status.clone()),
                status_message: None,
                last_accessed_date: None,
            })
            .collect()
    }

    /// The secret's `PrimaryRegion` for the wire.
    ///
    /// `PrimaryRegion` is part of the replication view — "The Region the secret
    /// is in. If a secret is replicated to other Regions, the replicas are
    /// listed in ReplicationStatus" — and Secrets Manager "only returns fields
    /// that have a value in the response", so a secret that is not a replication
    /// primary has no primary Region to report and the member is omitted. It
    /// comes back as soon as the secret has a replica, and goes away again once
    /// the last one is removed.
    pub fn primary_region_out(&self) -> Option<String> {
        (!self.replicas.is_empty()).then(|| self.region.clone())
    }

    /// Whether another version fits under [`VERSIONS_PER_SECRET`], given the
    /// staging labels the new version would claim.
    ///
    /// Below the bound there is always room. At it, a version every one of whose
    /// labels this put takes away is left deprecated, and [`put_version`] evicts
    /// it to make room — except the version `AWSCURRENT` is taken from, which
    /// [`put_version`] relabels `AWSPREVIOUS` instead of deprecating, so it
    /// frees nothing. With no version to free — a caller naming a fresh label
    /// every time — the caller is refused with the modelled quota error rather
    /// than growing the store.
    ///
    /// [`put_version`]: Secret::put_version
    pub fn has_room_for_version(&self, claimed: &[String]) -> bool {
        if self.versions.len() < VERSIONS_PER_SECRET {
            return true;
        }
        let demotes = claimed.iter().any(|stage| stage == STAGE_CURRENT)
            && !claimed.iter().any(|stage| stage == STAGE_PREVIOUS);
        self.versions.iter().any(|existing| {
            let freed = existing
                .version_stages
                .iter()
                .all(|held| claimed.contains(held));
            freed && !(demotes && existing.has_stage(STAGE_CURRENT))
        })
    }

    /// Add `version` and move the staging labels it claims onto it.
    ///
    /// A label belongs to one version at a time, so every label the new version
    /// claims is taken off whichever version held it; and when the new version
    /// claims `AWSCURRENT`, `AWSPREVIOUS` moves to the version `AWSCURRENT` was
    /// taken from — unless the caller placed `AWSPREVIOUS` on the new version
    /// itself, which would otherwise leave the label on two versions. A version
    /// left with no label stays stored (it is still addressable by `VersionId`)
    /// but is deprecated, and is the first thing evicted once the secret holds
    /// more than [`VERSIONS_PER_SECRET`] versions.
    pub fn put_version(&mut self, version: SecretVersion, at: SystemTime) {
        let claimed = version.version_stages.clone();
        let demoted = if claimed.iter().any(|stage| stage == STAGE_CURRENT) {
            self.version_with_stage(STAGE_CURRENT)
                .map(|current| current.version_id.clone())
        } else {
            None
        };
        for existing in &mut self.versions {
            existing.version_stages.retain(|held| !claimed.contains(held));
        }
        if let Some(demoted) = demoted.filter(|_| !claimed.iter().any(|s| s == STAGE_PREVIOUS)) {
            for existing in &mut self.versions {
                existing.version_stages.retain(|held| held != STAGE_PREVIOUS);
            }
            if let Some(existing) = self
                .versions
                .iter_mut()
                .find(|v| v.version_id == demoted)
            {
                existing.version_stages.push(STAGE_PREVIOUS.to_string());
            }
        }
        self.versions.push(version);
        self.last_changed_date = at;
        // Keep the secret bounded: evict the oldest deprecated versions, the
        // ones no staging label reaches, until the count is back under the
        // bound. A labelled version is never evicted, which is why a put that
        // frees none is refused by `has_room_for_version` before it gets here.
        while self.versions.len() > VERSIONS_PER_SECRET {
            let oldest_deprecated = self
                .versions
                .iter()
                .position(|existing| existing.version_stages.is_empty());
            match oldest_deprecated {
                Some(index) => {
                    self.versions.remove(index);
                }
                None => break,
            }
        }
    }
}

/// Per-service state for com-amazonaws-secretsmanager-secretsmanager. A handler reaches it through [`service_state`].
///
/// Secrets are held in ONE store rather than one per region: the region a secret
/// is homed in is recorded on it (and reported as `PrimaryRegion` once the
/// secret has a replica), but a `SecretId` resolves across the whole store, so a
/// secret created under one signed region is visible to a caller signing for
/// another. That is a simplification — the real service's namespace is
/// per-region — and it is the same store the quota in [`SECRET_QUOTA`] bounds.
#[derive(Debug, Default)]
pub struct ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// Secrets by name, in name order so `ListSecrets` pages deterministically.
    pub secrets: BTreeMap<String, Secret>,
    /// Full ARN to secret name, so a `SecretId` is accepted in either form.
    pub arn_index: HashMap<String, String>,
}

impl ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// Store a secret and index its ARN.
    ///
    /// Replacing a name drops the ARN index entry of the secret it replaces, so
    /// no ARN that names nothing is left resolvable.
    pub fn insert(&mut self, secret: Secret) {
        self.arn_index
            .insert(secret.arn.clone(), secret.name.clone());
        if let Some(replaced) = self.secrets.insert(secret.name.clone(), secret) {
            let still_current = self
                .secrets
                .get(&replaced.name)
                .is_some_and(|current| current.arn == replaced.arn);
            if !still_current {
                self.arn_index.remove(&replaced.arn);
            }
        }
    }

    /// Remove a secret by name, dropping its ARN index entry with it.
    pub fn remove(&mut self, name: &str) -> Option<Secret> {
        let removed = self.secrets.remove(name)?;
        self.arn_index.remove(&removed.arn);
        Some(removed)
    }

    /// The stored name a `SecretId` names: the bare secret name, the full ARN,
    /// or the partial ARN (the ARN without the trailing hyphen and six random
    /// characters, which a caller that never saw the full ARN can still build).
    pub fn resolve(&self, secret_id: &str) -> Option<String> {
        if self.secrets.contains_key(secret_id) {
            return Some(secret_id.to_string());
        }
        if let Some(name) = self.arn_index.get(secret_id) {
            return Some(name.clone());
        }
        // The partial-ARN scan walks the name-ordered map, not the ARN hash
        // map, so the match it picks does not depend on hash iteration order.
        self.secrets
            .values()
            .find(|secret| partial_arn(&secret.arn) == secret_id)
            .map(|secret| secret.name.clone())
    }

    /// The secret a `SecretId` names.
    pub fn get(&self, secret_id: &str) -> Option<&Secret> {
        let name = self.resolve(secret_id)?;
        self.secrets.get(&name)
    }

    /// The secret a `SecretId` names, for mutation.
    pub fn get_mut(&mut self, secret_id: &str) -> Option<&mut Secret> {
        let name = self.resolve(secret_id)?;
        self.secrets.get_mut(&name)
    }

    /// Permanently remove every secret whose recovery window has already
    /// elapsed. This is the deletion the clock has already made due — it was
    /// owed at this instant whether or not anybody called — so a reporting
    /// operation materialises it before it answers, and never reports a secret
    /// the service would already have deleted.
    pub fn expire_due(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .secrets
            .values()
            .filter(|secret| secret.deleted_date.is_some_and(|at| at <= now))
            .map(|secret| secret.name.clone())
            .collect();
        for name in due {
            self.remove(&name);
        }
    }

    /// Materialise every replica transition the clock has already made due,
    /// across every secret the store holds. Like [`expire_due`], this is a
    /// change that was owed at this instant whether or not anybody called, so an
    /// operation that reports a replication view performs it before it answers
    /// and never reports a replica as still being created once its span has
    /// elapsed.
    ///
    /// [`expire_due`]: ComAmazonawsSecretsmanagerSecretsmanagerState::expire_due
    pub fn settle_replicas_due(&mut self, now: SystemTime) {
        for secret in self.secrets.values_mut() {
            secret.settle_replicas(now);
        }
    }
}

/// An ARN with the trailing hyphen and six random characters removed; the ARN
/// unchanged when it does not end that way.
fn partial_arn(arn: &str) -> &str {
    match arn.len().checked_sub(ARN_SUFFIX_LEN + 1) {
        Some(cut) if arn.as_bytes().get(cut) == Some(&b'-') => &arn[..cut],
        _ => arn,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecretsmanagerSecretsmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecretsmanagerSecretsmanagerState>)
}
