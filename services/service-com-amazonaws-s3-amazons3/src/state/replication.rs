// SPDX-License-Identifier: CC0-1.0

//! S3's REPLICATION ENGINE: the half of a bucket's replication configuration that ACTS.
//!
//! [`super::Bucket::replication`] is what `PutBucketReplication` records and
//! `GetBucketReplication` reports back — a configuration. This module is what that
//! configuration DOES: "Amazon S3 can replicate objects to one or more destination buckets"
//! (`PutBucketReplication`'s own documentation), so a write into a bucket carrying a
//! replication configuration copies the version it wrote to the destination each covering
//! rule names, and the source version then reports what that copying did
//! (`GetObjectOutput$ReplicationStatus`).
//!
//! The rules this module implements, each from the published semantics of replication:
//!
//! * only an ENABLED rule replicates — "Specifies whether the rule is enabled"
//!   (`ReplicationRule$Status`);
//! * every copy is made UNDER THE ROLE the configuration names — "the Identity and Access
//!   Management (IAM) role that Amazon S3 assumes when replicating objects"
//!   (`ReplicationConfiguration$Role`) — so a role whose trust policy does not let the
//!   Amazon S3 service principal assume it replicates nothing at all and the source version
//!   reports the failure ([`RoleAccess`]);
//! * a rule covers the keys its `Prefix`, or its `Filter`, selects: "An object key name
//!   prefix that identifies the object or objects to which the rule applies"
//!   (`ReplicationRule$Prefix`) and "a filter that identifies the subset of objects to
//!   which the replication rule applies" (`ReplicationRuleFilter`), whose `Tag` "applies
//!   only to objects that have the tag in their tag set";
//! * a covered version is copied BYTE-IDENTICALLY to the destination bucket the rule names,
//!   under the same key and the same version id, where it is a REPLICA
//!   ([`super::REPLICATION_STATUS_REPLICA`]);
//! * a DELETE MARKER is replicated only where the rule asks for it — "Specifies whether
//!   Amazon S3 replicates delete markers" (`DeleteMarkerReplication$Status`) — and the
//!   permanent delete of one VERSION is never replicated ("Amazon S3 doesn't replicate the
//!   delete if the request specifies an object version ID", the delete-marker replication
//!   rules), which is why only the marker path reaches here;
//! * nothing already in the bucket is copied: replication acts on the write that happens
//!   while a rule is in force, so an object written before the configuration was set stays
//!   where it is ("This parameter is no longer supported. To replicate existing objects…",
//!   `ReplicationRule$ExistingObjectReplication`);
//! * a replica is not itself replicated, so a configuration pointing back at the source
//!   cannot copy a replica home again: the copy is recorded through
//!   [`super::ComAmazonawsS3Amazons3State::put_replica`], which stores it and replicates
//!   nothing, while replication is carried out only for the version a WRITE recorded.
//!
//! The copy is made inside the call that records the source version — the store is in hand
//! and no second actor is needed — and only the STATUS the source reports is asynchronous:
//! it reads [`super::REPLICATION_STATUS_PENDING`] until
//! [`super::REPLICATION_SETTLES_AFTER`] past the write, which is the span this emulator
//! gives AWS's "Amazon S3 replicates objects asynchronously". Nothing here sleeps and
//! nothing here is a background task.

use super::{ComAmazonawsS3Amazons3State, Object, ObjectReplication, ResourceTag};
use crate::ops::types::{ReplicationRule, ReplicationRuleFilter};

/// The `ReplicationRuleStatus` value of a rule that replicates
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ReplicationRuleStatus")` publishes
/// `[Enabled, Disabled]`).
pub const RULE_ENABLED: &str = "Enabled";

/// The `DeleteMarkerReplicationStatus` value that asks for delete markers to be replicated
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#DeleteMarkerReplicationStatus")`
/// publishes `[Enabled, Disabled]`).
pub const DELETE_MARKER_REPLICATION_ENABLED: &str = "Enabled";

/// The service principal Amazon S3's replication assumes a role AS, and so the one a
/// replication role's trust policy has to admit for S3 to be able to assume it: the role is
/// "the Identity and Access Management (IAM) role that Amazon S3 assumes when replicating
/// objects" (`ReplicationConfiguration$Role`), and S3 reaches `sts:AssumeRole` as its own
/// service principal.
pub const REPLICATION_SERVICE_PRINCIPAL: &str = "s3.amazonaws.com";

/// Whether the role a bucket's replication configuration names is one Amazon S3 can assume
/// — the role EVERY copy that configuration calls for is made under.
///
/// Read once per write ([`role_access`]) and handed to the two places that act on a
/// configuration ([`super::ComAmazonawsS3Amazons3State::replicate_write`] and
/// [`super::ComAmazonawsS3Amazons3State::replicate_delete_marker`]), so one write cannot
/// copy a version under a role another part of the same write found unassumable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleAccess {
    /// Nothing IAM answered withholds the role from S3: the copies the configuration calls
    /// for are made, which is also the answer for a bucket that replicates nothing at all.
    Assumable,
    /// IAM holds the role the configuration names and the trust policy it carries does not
    /// admit [`REPLICATION_SERVICE_PRINCIPAL`]: S3 cannot assume it, so no copy can be made
    /// under it and the replication of every covered version fails.
    Withheld,
}

/// Whether Amazon S3 can assume the role the bucket named `bucket` replicates under,
/// resolved against the account's IAM records.
///
/// A replication is carried out by S3 assuming that role, so the role is what decides
/// whether the copies a configuration calls for can be made at all — which is why this is
/// asked per WRITE rather than when the configuration is stored: the trust policy in force
/// at the moment of the copy is the one that governs it.
///
/// IAM is reached the way any emulated service reaches another, through the in-process
/// callback, by the reading every service resolving a caller-supplied role shares
/// (`service_common::iam_role`). The store is locked only for the read of the stored ARN and
/// released before that hop, so this service's own guard is never held across it — and
/// nothing is withheld that IAM did not answer: a role IAM holds nothing under, an answer
/// this emulator could not read and an emulator with no IAM mounted all leave the
/// replication to proceed.
pub async fn role_access<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    bucket: &str,
) -> RoleAccess {
    let Some(state) = super::service_state(engine) else {
        return RoleAccess::Assumable;
    };
    let role = {
        let store = state.lock();
        store
            .bucket(bucket)
            .and_then(super::Bucket::replication)
            .map(|configuration| configuration.role.clone())
    };
    // A bucket carrying no replication configuration replicates nothing, so it names no
    // role to resolve and pays no hop for one.
    let Some(role) = role else {
        return RoleAccess::Assumable;
    };
    if ::service_common::iam_role::held_role_is_unassumable(
        ctx,
        &super::request_region(ctx.parts()),
        &role,
        REPLICATION_SERVICE_PRINCIPAL,
    )
    .await
    {
        RoleAccess::Withheld
    } else {
        RoleAccess::Assumable
    }
}

/// The bucket name the `Destination$Bucket` ARN `arn` names — "the Amazon Resource Name
/// (ARN) of the bucket where you want Amazon S3 to store the results", whose resource part
/// is the bare bucket name (the form `PutBucketReplication` refuses anything but, through
/// [`super::is_arn_of`]).
///
/// Read through the ONE shared ARN parse (`service_common::arn`), the same parse that
/// judged the member when it was stored, so this module does not split an ARN its own way.
/// `None` for a value that is not a resource ARN at all, which a stored configuration
/// cannot carry.
pub fn destination_bucket(arn: &str) -> Option<&str> {
    let resource = ::service_common::arn::parse(arn)?.resource;
    (!resource.is_empty()).then_some(resource)
}

/// Whether `rule` covers the object at `key` carrying `tags` — the subset of objects the
/// rule applies to, and so the versions it replicates.
///
/// `Prefix` and `Filter` are the two forms of the same selection ("`Filter` must specify
/// exactly one `Prefix`, `Tag`, or an `And` child element", `ReplicationRuleFilter`), and a
/// rule carrying neither covers the whole bucket ("To include all objects in a bucket,
/// specify an empty string", `ReplicationRule$Prefix`).
pub fn covers(rule: &ReplicationRule, key: &str, tags: &[ResourceTag]) -> bool {
    if rule.status != RULE_ENABLED {
        return false;
    }
    if let Some(prefix) = rule.prefix.as_deref() {
        return key.starts_with(prefix);
    }
    match rule.filter.as_ref() {
        None => true,
        Some(filter) => filter_covers(filter, key, tags),
    }
}

/// Whether `filter` selects the object at `key` carrying `tags`: its one `Prefix`, its one
/// `Tag`, or the `And` that carries both — "the filters determine the subset of objects to
/// which the rule applies" (`ReplicationRuleFilter$And`), so EVERY filter the container
/// carries has to hold.
fn filter_covers(filter: &ReplicationRuleFilter, key: &str, tags: &[ResourceTag]) -> bool {
    let prefix_holds = filter
        .prefix
        .as_deref()
        .is_none_or(|prefix| key.starts_with(prefix));
    let tag_holds = filter
        .tag
        .as_ref()
        .is_none_or(|tag| super::carries_tag(tags, tag));
    let and_holds = filter.and.as_ref().is_none_or(|and| {
        let prefix = and
            .prefix
            .as_deref()
            .is_none_or(|prefix| key.starts_with(prefix));
        let every_tag = and
            .tags
            .iter()
            .flatten()
            .all(|tag| super::carries_tag(tags, tag));
        prefix && every_tag
    });
    prefix_holds && tag_holds && and_holds
}

/// Whether the rule asks for its delete markers to be replicated.
///
/// A rule that names no `DeleteMarkerReplication` does not: the container is what "specifies
/// whether Amazon S3 replicates delete markers" (`DeleteMarkerReplication`), and a rule
/// without it leaves the destination unmarked.
pub fn replicates_delete_markers(rule: &ReplicationRule) -> bool {
    rule.delete_marker_replication
        .as_ref()
        .and_then(|container| container.status.as_deref())
        .is_some_and(|status| status == DELETE_MARKER_REPLICATION_ENABLED)
}

/// One destination a covering rule replicates to: where the copy goes, the storage class it
/// is stored under, and whether that rule replicates delete markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicationTarget {
    /// The name of the destination bucket the rule's `Destination$Bucket` ARN names.
    pub bucket: String,
    /// `Destination$StorageClass` — "the storage class to use when replicating objects …
    /// By default, Amazon S3 uses the storage class of the source object to create the
    /// object replica", so `None` is "keep the source's class" rather than a default class.
    pub storage_class: Option<String>,
    /// Whether the rule asks for its delete markers to be replicated
    /// ([`replicates_delete_markers`]).
    pub delete_markers: bool,
    /// `ReplicationRule$Priority`, as [`rule_priority`] reads it — what decides between two
    /// rules that would replicate the same object to the same destination.
    pub priority: i32,
}

/// The precedence `rule` takes where two rules would replicate one object to one
/// destination: "the priority indicates which rule has precedence whenever two or more
/// replication rules conflict" (`ReplicationRule$Priority`), with a higher value taking
/// precedence.
///
/// A rule naming none is ordered BELOW every rule that does, so a configuration that
/// prioritises one of two overlapping rules has that one applied.
pub fn rule_priority(rule: &ReplicationRule) -> i32 {
    rule.priority.unwrap_or(i32::MIN)
}

/// The destinations the configuration of the bucket named `source` replicates the object at
/// `key` (carrying `tags`) to — empty for a bucket with no configuration, and for a key no
/// enabled rule covers.
///
/// ONE target per destination bucket: two rules naming the same destination for one object
/// conflict, and "Amazon S3 will attempt to replicate objects according to all replication
/// rules. However, if there are two or more rules with the same destination bucket, then
/// objects will be replicated according to the rule with the highest priority"
/// (`ReplicationRule$Priority`) — so the covering rule with the highest priority is the one
/// whose storage class and delete-marker setting apply, and rules naming DIFFERENT
/// destinations all apply.
///
/// Read-only: a plain report over the stored configuration, which is what lets both the
/// write path and the delete path ask the same question.
pub fn destinations(
    store: &ComAmazonawsS3Amazons3State,
    source: &str,
    key: &str,
    tags: &[ResourceTag],
) -> Vec<ReplicationTarget> {
    let Some(configuration) = store.bucket(source).and_then(super::Bucket::replication) else {
        return Vec::new();
    };
    let mut targets: Vec<ReplicationTarget> = Vec::new();
    for rule in configuration.rules.iter().filter(|rule| covers(rule, key, tags)) {
        let Some(bucket) = destination_bucket(&rule.destination.bucket) else {
            continue;
        };
        let target = ReplicationTarget {
            bucket: bucket.to_string(),
            storage_class: rule.destination.storage_class.clone(),
            delete_markers: replicates_delete_markers(rule),
            priority: rule_priority(rule),
        };
        match targets
            .iter_mut()
            .find(|held| held.bucket == target.bucket)
        {
            // The rule with the higher priority wins the conflict; the first of two rules
            // of equal priority keeps it, which is the order the configuration names them
            // in.
            Some(held) if held.priority < target.priority => *held = target,
            Some(_) => {}
            None => targets.push(target),
        }
    }
    targets
}

/// The REPLICA of `source_version`, stored under `storage_class` when the rule named one.
///
/// "The object in the destination bucket is a replica"
/// (`HeadObjectOutput$ReplicationStatus`) — and a replica is byte-identical to its source,
/// which is why nothing here is recomputed: the entity tag, the checksum, the encryption
/// configuration, the metadata and the tag set are the source version's own. The storage
/// class is the rule's where it names one and the source's where it does not
/// (`Destination$StorageClass`). The members a LATER call places on a stored version (its
/// Object Lock retention and legal hold, its restore, its annotations and its ACL) are not
/// copied: they belong to the version they were placed on.
pub fn replica_of(
    source_version: &Object,
    storage_class: Option<&str>,
    at: std::time::SystemTime,
) -> Object {
    Object {
        replication: Some(ObjectReplication::replica(at)),
        storage_class: storage_class
            .map(str::to_string)
            .unwrap_or_else(|| source_version.storage_class.clone()),
        retention: None,
        legal_hold: None,
        restore: None,
        annotations: std::collections::BTreeMap::new(),
        acl: None,
        ..source_version.clone()
    }
}
