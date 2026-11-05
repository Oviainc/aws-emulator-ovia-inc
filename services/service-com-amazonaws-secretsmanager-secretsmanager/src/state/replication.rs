// SPDX-License-Identifier: CC0-1.0

//! Cross-region replication: what a replica of a secret IS in this store, and
//! the whole lifecycle of one.
//!
//! `ReplicateSecretToRegions` "replicates the secret to a new Regions"
//! (this service's Smithy model) — it creates a SEPARATE secret in each Region
//! it adds. That secret carries its own ARN, "the same as the original primary
//! secret except the Region is changed"
//! (`StopReplicationToReplicaResponse$ARN`), it is read in its own Region, and it
//! is not written there: every write goes to the primary, and what the replica
//! holds IS what the primary holds. So a replica here is a stored [`Secret`]
//! marked with the Region of the primary it tracks ([`Secret::replica_of`]),
//! holding no copy of that primary's content: a read of it projects the primary
//! ([`ComAmazonawsSecretsmanagerSecretsmanagerState::view`]), so a write to the
//! primary is visible in the replica Region with nothing to propagate and nothing
//! to go stale, and the record itself costs the same whatever the secret holds.
//!
//! * the primary's `ReplicationStatus` is the view OF those secrets — one entry
//!   per Region, with the `StatusType` its creation reached and the
//!   `StatusMessage` that status says about it;
//! * a replica the service could not create is recorded `Failed` with the reason
//!   and no secret exists in that Region;
//! * `RemoveRegionsFromReplication` deletes the replica, and
//!   `StopReplicationToReplica` unlinks it, taking the primary's content as its
//!   own so that the secret it had become stands on its own.

use std::collections::BTreeSet;

use super::*;

/// What `ReplicationStatusType$StatusMessage` reports for a replica that was
/// created: replication into that Region succeeded, which is the half of the
/// status pair a client reads to tell a usable replica from a broken one.
pub const REPLICATION_SUCCEEDED_MESSAGE: &str = "Replication succeeded";

/// What it reports for a replica the destination Region already held a secret of
/// this name in: "Status message such as 'Secret with this name already exists in
/// this region'" (`ReplicationStatusType$StatusMessage`, this service's Smithy
/// model) — the one condition the model itself words, and the one
/// `ForceOverwriteReplicaSecret` ("Specifies whether to overwrite a secret with
/// the same name in the destination Region") resolves.
pub const REPLICA_NAME_COLLISION_MESSAGE: &str =
    "Secret with this name already exists in this region";

/// The `InvalidParameterException` message for an operation that would write to a
/// secret which, in the Region the request is signed for, is a REPLICA.
///
/// A replica tracks its primary, so the primary is where a value, a tag, a policy
/// or a schedule is changed — which is what the second sentence tells the caller
/// to do: make the call in the primary's Region. This is the mirror of the
/// refusal [`NOT_A_REPLICA_MESSAGE`] states for the promotion call made on a
/// secret that is not a replica: the same error class, and one fixed text naming
/// the class of secret the call is not permitted on, carrying no secret name, ARN
/// or Region of its own.
pub const NOT_PERMITTED_ON_REPLICA_MESSAGE: &str =
    "Operation not permitted on a replica secret. Call must be made in primary secret's region.";

/// The `InvalidParameterException` message `DeleteSecret` refuses a primary that
/// still has replicas with: "You can't delete a primary secret that is replicated
/// to other Regions. You must first delete the replicas using
/// RemoveRegionsFromReplication, and then delete the primary secret."
/// (`DeleteSecret`, this service's Smithy model.)
///
/// The refusal names the secret it is about and the Regions that are in the way,
/// since those are what the caller has to remove: the ARN of the secret the
/// request resolved to, and the Regions of the replicas it still holds, in the
/// order the secret holds them.
#[must_use]
pub fn primary_is_replicated_message(arn: &str, regions: &[String]) -> String {
    format!(
        "You can't delete secret {arn} that still has replica regions [{}].",
        regions.join(", ")
    )
}

/// How a replica states the reason its creation failed on the KMS key its Region
/// named: replication failed, and then what the service could not do with that
/// key.
///
/// A replica holds the secret's value encrypted under its own Region's key, so a
/// key that cannot be used is a value that cannot be encrypted — which is why the
/// reason is the very message a write refused under that key carries
/// (`EncryptionFailure`, see [`envelope::key_refusal`]), relayed KMS answer and
/// all, behind this clause.
pub const REPLICATION_FAILED_PREFIX: &str = "Replication failed:";

/// The `StatusMessage` of a replica whose Region's KMS key refused to be used,
/// `refusal` being what the service could not do with that key.
#[must_use]
pub fn replica_key_failure_message(refusal: &str) -> String {
    format!("{REPLICATION_FAILED_PREFIX} {refusal}")
}

/// The ARN the replica of the secret `primary_arn` carries in `region`: "the same
/// as the original primary secret except the Region is changed"
/// (`StopReplicationToReplicaResponse$ARN`, this service's Smithy model), so the
/// six random characters the primary's ARN ends in belong to the replica too and
/// the two ARNs differ in the Region field alone.
///
/// An ARN with no Region field to replace is left as it is: there is nothing to
/// change, and inventing an ARN would name a resource nothing else addresses.
#[must_use]
pub fn replica_arn_of(primary_arn: &str, region: &str) -> String {
    let mut fields: Vec<&str> = primary_arn.split(':').collect();
    const REGION_FIELD: usize = 3;
    if fields.len() <= REGION_FIELD {
        return primary_arn.to_string();
    }
    fields[REGION_FIELD] = region;
    fields.join(":")
}

/// The Region an ARN names, which for a secret's ARN is the Region the secret is
/// homed in — and so the Region its version values were sealed in.
#[must_use]
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|field| !field.is_empty())
}

impl Secret {
    /// The Regions this secret still has replicas in, in the order it holds them:
    /// what a caller has to remove before the secret itself can be deleted.
    pub fn replica_regions(&self) -> Vec<String> {
        self.replicas
            .iter()
            .map(|replica| replica.region.clone())
            .collect()
    }

    /// The secret the Region `region` holds while this secret is replicated
    /// there: its own ARN, its own KMS key and that Region's own access stamp,
    /// marked as tracking this secret's Region.
    ///
    /// It holds NO COPY of what it replicates. A replica's content IS the
    /// primary's while it tracks it — every version's value, the description, the
    /// tags, the rotation configuration — so a read of the replica projects the
    /// primary ([`content_of`]): a write to the primary is visible in the replica
    /// Region with nothing to propagate and nothing to go stale, and the record
    /// costs the same whatever the secret holds. What it does carry is what is
    /// per-Region about a replica: "the date that the secret was last accessed in
    /// the Region" (`ReplicationStatusType$LastAccessedDate`, this service's
    /// Smithy model) is a fact about the replica rather than about the primary, so
    /// a record already held there keeps its stamp.
    ///
    /// A replica is never itself scheduled for deletion: the deletion of a
    /// replica is `RemoveRegionsFromReplication`, which removes it outright.
    ///
    /// [`content_of`]: ComAmazonawsSecretsmanagerSecretsmanagerState::content_of
    pub(crate) fn replica_record(
        &self,
        region: &str,
        kms_key_id: &str,
        held: Option<&Secret>,
    ) -> Secret {
        Secret {
            arn: replica_arn_of(&self.arn, region),
            // The key the replica is encrypted with is the replica's own; a
            // replica left on the Amazon Web Services managed key names no key,
            // exactly as a secret created without one does.
            kms_key_id: Some(kms_key_id)
                .filter(|key| !names_default_kms_key(key))
                .map(str::to_string),
            last_accessed_date: held.and_then(|held| held.last_accessed_date),
            replica_of: Some(self.region.clone()),
            ..Secret::new(
                self.name.clone(),
                String::new(),
                region.to_string(),
                self.created_date,
            )
        }
    }
}

/// A secret as one Region answers for it: the record addressed, and the record its
/// content comes from.
///
/// The two are the same record for every secret but a replica, which holds no
/// content of its own: a read of a replica answers with the replica's identity and
/// the primary's content (see [`Secret::replica_record`]).
#[derive(Debug, Clone, Copy)]
pub struct SecretView<'a> {
    /// The secret the request addressed, which its identity — ARN, Region and KMS
    /// key — and its per-Region state are read from.
    pub secret: &'a Secret,
    /// Where that secret's content lives: its versions, description, tags,
    /// rotation configuration and resource policy.
    pub content: &'a Secret,
}

impl ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// Where `secret`'s content lives: the primary it tracks while it is a
    /// replica, else the secret itself. A replica whose primary this store no
    /// longer holds answers for itself, so no read follows a link to nothing.
    pub fn content_of<'a>(&'a self, secret: &'a Secret) -> &'a Secret {
        match &secret.replica_of {
            Some(primary_region) => self
                .secrets
                .get(&SecretKey::new(primary_region.clone(), secret.name.clone()))
                .unwrap_or(secret),
            None => secret,
        }
    }

    /// The secret a `SecretId` addresses in `region`, with the record its content
    /// comes from.
    pub fn view(&self, region: &str, secret_id: &str) -> Option<SecretView<'_>> {
        self.get(region, secret_id).map(|secret| SecretView {
            secret,
            content: self.content_of(secret),
        })
    }

    /// The same view of the secret at `key`.
    pub fn view_of(&self, key: &SecretKey) -> Option<SecretView<'_>> {
        self.secrets.get(key).map(|secret| SecretView {
            secret,
            content: self.content_of(secret),
        })
    }

    /// How many MORE secrets this store would hold if the secret `primary` names
    /// were replicated to `regions`: a Region that already holds that secret's
    /// replica holds the record replication refreshes rather than a second one, so
    /// it adds nothing.
    ///
    /// This is what [`SECRET_QUOTA`] bounds. A replica is a secret of this store
    /// like any other, so replication GROWS it and is held to the same bound
    /// `CreateSecret` holds a create to — which is what keeps the resident state
    /// finite however many Regions a caller names, and whatever Region it signs
    /// for.
    pub fn secrets_added_by_replicating(&self, primary: &SecretKey, regions: &[&str]) -> usize {
        let mut fresh: BTreeSet<&str> = BTreeSet::new();
        for region in regions {
            let destination = SecretKey::new(*region, primary.name.clone());
            let refreshes = self
                .secrets
                .get(&destination)
                .is_some_and(|held| held.replica_of.as_deref() == Some(primary.region.as_str()));
            if !refreshes {
                fresh.insert(region);
            }
        }
        fresh.len()
    }

    /// Replicate the secret `primary` names into `region`, and answer the
    /// `StatusMessage` of the failure it was recorded with, if it failed.
    ///
    /// `key_refusal` is what the replica's KMS key refused, which the caller asks
    /// KMS for before this is called: a replica whose key cannot be used is not
    /// created at all and is recorded `Failed` with that reason. The other
    /// failure this store can meet is a secret of this name the destination
    /// Region already holds, which `force_overwrite` is what overwrites
    /// (`ReplicateSecretToRegionsRequest$ForceOverwriteReplicaSecret`) — a
    /// replica of THIS secret is not such a secret: replicating into a Region
    /// that already holds one starts that replica's creation over.
    ///
    /// ONE Region is touched: the primary's entry for it, and the secret that
    /// Region holds under this name. Nothing else in the store is read or
    /// written, so a request naming several Regions costs what those Regions cost.
    pub fn replicate(
        &mut self,
        primary: &SecretKey,
        region: &str,
        kms_key_id: Option<&str>,
        force_overwrite: bool,
        key_refusal: Option<String>,
        at: SystemTime,
    ) -> Option<String> {
        let destination = SecretKey::new(region, primary.name.clone());
        let held = self.secrets.get(&destination);
        let refreshes =
            held.is_some_and(|held| held.replica_of.as_deref() == Some(primary.region.as_str()));
        let occupied = held.is_some() && !refreshes;
        let failure = key_refusal.or_else(|| {
            (occupied && !force_overwrite).then(|| REPLICA_NAME_COLLISION_MESSAGE.to_string())
        });
        let record = self.secrets.get(primary).map(|secret| {
            secret.replica_record(
                region,
                kms_key_id
                    .filter(|key| !key.is_empty())
                    .unwrap_or(DEFAULT_KMS_KEY_ALIAS),
                held.filter(|_| refreshes),
            )
        });
        if let Some(secret) = self.secrets.get_mut(primary) {
            secret.replicate_to(region, kms_key_id, failure.clone(), at);
        }
        match record.filter(|_| failure.is_none()) {
            // The destination's own secret, where the caller asked for the
            // overwrite that stands in the way, is replaced by the replica.
            Some(record) => {
                self.remove(&destination);
                self.insert(record);
            }
            // The replica was not created, so the Region holds no secret of this
            // secret's: one that had been replicated there is gone, and a secret
            // of the Region's own is left exactly as it was.
            None => {
                if refreshes {
                    self.remove(&destination);
                }
            }
        }
        failure
    }

    /// Stop replicating the secret `primary` names into `region`, reporting
    /// whether a replica was held there.
    ///
    /// "When you delete a replica, it is deleted immediately" (`DeleteSecret`,
    /// this service's Smithy model), so the secret the Region held is removed
    /// with the entry for it.
    pub fn remove_replica_region(&mut self, primary: &SecretKey, region: &str) -> bool {
        let removed = self
            .secrets
            .get_mut(primary)
            .is_some_and(|secret| secret.remove_replica(region));
        let destination = SecretKey::new(region, primary.name.clone());
        if self
            .secrets
            .get(&destination)
            .is_some_and(|held| held.replica_of.as_deref() == Some(primary.region.as_str()))
        {
            self.remove(&destination);
        }
        removed
    }

    /// Promote the replica the key names to a secret of its own, answering its
    /// ARN; `None` for a key that names no replica.
    ///
    /// "Removes the link between the replica secret and the primary secret and
    /// promotes the replica to a primary secret in the replica Region"
    /// (`StopReplicationToReplica`, this service's Smithy model): the secret keeps
    /// its ARN and takes the content it had been reading from the primary as its
    /// own — the versions and their staging labels, the description, the tags, the
    /// rotation configuration and the policy — and stops tracking the primary,
    /// which stops replicating to it. The values stay sealed for the ARN they were
    /// sealed under ([`Secret::sealed_under`]), which is the only encryption
    /// context they open under.
    pub fn promote_replica(&mut self, key: &SecretKey, at: SystemTime) -> Option<String> {
        let primary_region = self.secrets.get(key)?.replica_of.clone()?;
        let primary_key = SecretKey::new(primary_region, key.name.clone());
        let content = self.secrets.get(&primary_key).cloned();
        if let Some(primary) = self.secrets.get_mut(&primary_key) {
            primary.remove_replica(&key.region);
        }
        let promoted = self.secrets.get_mut(key)?;
        if let Some(content) = content {
            promoted.sealed_under = Some(content.value_context_arn().to_string());
            promoted.description = content.description;
            promoted.secret_type = content.secret_type;
            promoted.owning_service = content.owning_service;
            promoted.tags = content.tags;
            promoted.rotation_enabled = content.rotation_enabled;
            promoted.rotation_lambda_arn = content.rotation_lambda_arn;
            promoted.rotation_rules = content.rotation_rules;
            promoted.external_rotation_metadata = content.external_rotation_metadata;
            promoted.external_rotation_role_arn = content.external_rotation_role_arn;
            promoted.last_rotated_date = content.last_rotated_date;
            promoted.next_rotation_date = content.next_rotation_date;
            promoted.pending_rotation_version_id = content.pending_rotation_version_id;
            promoted.resource_policy = content.resource_policy;
            promoted.versions = content.versions;
        }
        promoted.replica_of = None;
        promoted.last_changed_date = at;
        Some(promoted.arn.clone())
    }
}
