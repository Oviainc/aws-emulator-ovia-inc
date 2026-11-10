// SPDX-License-Identifier: CC0-1.0

//! The tag slice of this service's state: how an ARN resolves to a stored record, and
//! the tag set that record carries.
//!
//! Tags are NOT a collection of their own — every taggable record already owns its
//! `tags` map (a create request's `tags` land there), so the three tagging operations
//! read and write the resource's own record and a deleted resource takes its tags with
//! it. What they share, and what lives here, is the resolution step: an `Arn` names one
//! resource of one kind, and only a record this account and Region actually holds is
//! taggable.

use std::collections::BTreeMap;

use crate::ops::types::ResourceNotFoundException;

use super::{
    ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState, ARN_NAMESPACE,
    RESOURCE_TYPE_DB_CLUSTER, RESOURCE_TYPE_DB_INSTANCE, RESOURCE_TYPE_DB_PARAMETER_GROUP,
};

/// The `resourceType` this service reports on `ResourceNotFoundException` for a DB
/// backup.
///
/// Like [`super::RESOURCE_TYPE_DB_PARAMETER_GROUP`], the member is modelled as a plain
/// `String` rather than as the `ResourceType` enum, so a backup's resource type is
/// reportable even though that enum lists only the two compute resources.
pub const RESOURCE_TYPE_DB_BACKUP: &str = "DB_BACKUP";

/// How many tags one resource holds.
///
/// The bound is the modelled maximum of the tag map a resource reports —
/// `ListTagsForResource`'s `tags` targets `ResponseTagMap`, `@length max 200`
/// (`smithy_query(query="shape", shape="ResponseTagMap")`) — so a resource that held
/// more could not be read back inside its own model. It is also what stops repeated
/// merges growing a record's tag map without end.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The kinds of resource this service's `Arn` addresses, one per resource-type segment
/// the modelled `Arn` pattern allows:
/// `(db\-instance|db\-cluster|db\-parameter\-group|db\-backup)`
/// (`smithy_query(query="shape", shape="Arn")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggableResourceKind {
    DbInstance,
    DbCluster,
    DbParameterGroup,
    DbBackup,
}

impl TaggableResourceKind {
    /// The resource-type segment an ARN of this kind carries.
    pub fn arn_segment(self) -> &'static str {
        match self {
            Self::DbInstance => "db-instance",
            Self::DbCluster => "db-cluster",
            Self::DbParameterGroup => "db-parameter-group",
            Self::DbBackup => "db-backup",
        }
    }

    /// The kind an ARN's resource-type segment names, or `None` for a segment this
    /// service has no resource for.
    pub fn from_arn_segment(segment: &str) -> Option<Self> {
        [
            Self::DbInstance,
            Self::DbCluster,
            Self::DbParameterGroup,
            Self::DbBackup,
        ]
        .into_iter()
        .find(|kind| kind.arn_segment() == segment)
    }

    /// The `resourceType` a refusal about a resource of this kind reports.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::DbInstance => RESOURCE_TYPE_DB_INSTANCE,
            Self::DbCluster => RESOURCE_TYPE_DB_CLUSTER,
            Self::DbParameterGroup => RESOURCE_TYPE_DB_PARAMETER_GROUP,
            Self::DbBackup => RESOURCE_TYPE_DB_BACKUP,
        }
    }
}

/// The kind of resource an ARN names and the id after its `/`, or `None` when the string
/// is not an ARN of this service naming one of its resource kinds.
///
/// The modelled `Arn` pattern is checked before a handler runs, so a well-formed request
/// always parses here; the `Option` is what keeps the resolution total rather than
/// panicking on a string the boundary could not have constrained (an ARN reached from
/// stored state, say).
pub fn parse_resource_arn(arn: &str) -> Option<(TaggableResourceKind, &str)> {
    // `arn:<partition>:timestream-influxdb:<region>:<account>:<type>/<id>`, the six
    // colon-separated segments of the modelled pattern.
    let mut segments = arn.splitn(6, ':');
    (segments.next()? == "arn").then_some(())?;
    let _partition = segments.next()?;
    (segments.next()? == ARN_NAMESPACE).then_some(())?;
    let _region = segments.next()?;
    let _account = segments.next()?;
    let (segment, id) = segments.next()?.split_once('/')?;
    Some((TaggableResourceKind::from_arn_segment(segment)?, id))
}

/// The refusal a tagging call on an ARN that names no resource this account holds gets.
///
/// `resourceId` is the ARN the request named — the only identifier a tagging call
/// carries — and `resourceType` is the kind that ARN addresses, both `@required` members
/// of the shape.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "Resource cannot be found.".to_string(),
        resource_id: resource_arn.to_string(),
        resource_type: parse_resource_arn(resource_arn)
            .map(|(kind, _)| kind.resource_type().to_string())
            .unwrap_or_default(),
    }
}

/// The refusal a merge that would take a resource past [`MAX_TAGS_PER_RESOURCE`] tags
/// gets; `requested` is how many tags the resource would end up carrying.
///
/// The fixed text is how this service words a quota refusal — the modelled
/// `ServiceQuotaExceededException` is documented "The request exceeds the service quota"
/// — followed by which resource and which bound, taken from the request and the stored
/// record.
pub fn tag_quota_exceeded_message(resource_arn: &str, requested: usize) -> String {
    format!(
        "The request exceeds the service quota for your account: {resource_arn} would hold \
         {requested} tags, more than the maximum of {MAX_TAGS_PER_RESOURCE} tags per resource."
    )
}

impl ComAmazonawsTimestreaminfluxdbAmazontimestreaminfluxdbState {
    /// The stored tag set of the resource `resource_arn` names, or `None` when this
    /// account and Region holds no such resource.
    ///
    /// Resolution is by the record's OWN `arn`: the ARN's resource-type segment picks the
    /// collection, its id segment the record, and the record's stored ARN must then equal
    /// the one asked for — which is what makes the partition, Region and account segments
    /// count, so an id that exists in one Region is not addressable through another's ARN.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        let (kind, id) = parse_resource_arn(resource_arn)?;
        match kind {
            TaggableResourceKind::DbInstance => self
                .db_instances
                .get(id)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggableResourceKind::DbCluster => self
                .db_clusters
                .get(id)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggableResourceKind::DbParameterGroup => self
                .db_parameter_groups
                .get(id)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            // A DB backup is a resource of this service's model that no operation here
            // creates yet (the backup operations are still scaffolds), so the store holds
            // none and no backup ARN resolves to one. Nothing stands in for a backup that
            // does not exist: a tagging call on one is refused as not found.
            TaggableResourceKind::DbBackup => None,
        }
    }

    /// The stored tag set of the resource `resource_arn` names, writable, or `None` when
    /// this account and Region holds no such resource. Resolves exactly as
    /// [`Self::resource_tags`] does.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        let (kind, id) = parse_resource_arn(resource_arn)?;
        match kind {
            TaggableResourceKind::DbInstance => self
                .db_instances
                .get_mut(id)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::DbCluster => self
                .db_clusters
                .get_mut(id)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::DbParameterGroup => self
                .db_parameter_groups
                .get_mut(id)
                .filter(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggableResourceKind::DbBackup => None,
        }
    }
}
