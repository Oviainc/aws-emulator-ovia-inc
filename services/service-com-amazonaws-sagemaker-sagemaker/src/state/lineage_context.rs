// SPDX-License-Identifier: CC0-1.0

//! The lineage-context slice of this service's state: the [`Context`] record
//! `CreateContext` writes and every context reader replays, with its bound and ARN kind.

use super::*;

/// The ARN resource kind a lineage context carries, i.e. the `<kind>` in [`resource_arn`].
/// Fixed by `ContextArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:context/`), and named once here so
/// the create, the reader, the update and the delete mint and match the same ARN.
pub const CONTEXT_KIND: &str = "context";

/// See [`MODEL_QUOTA`]. Bounds the lineage-context store the same way, and is what
/// `CreateContext` answers `ResourceLimitExceeded` past; every record it holds is released
/// by `DeleteContext`.
pub const CONTEXT_QUOTA: usize = 1_000;

/// A lineage context created by `CreateContext`, as `DescribeContext` replays it and
/// `UpdateContext` edits it.
///
/// `properties` is the map the context HOLDS, empty while nothing has put an entry in it
/// (see this module's note on an entity's own collections). It is a `BTreeMap` so the map
/// serialises in one stable order whatever order the request arrived in. `created_by` /
/// `last_modified_by` are stored by the call that set them rather than rebuilt per read,
/// so every later read reports the identity that actually created or last edited the
/// context.
#[derive(Debug, Clone)]
pub struct Context {
    pub name: String,
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so a read does not have to parse the ARN to report the Region's lineage
    /// group.
    pub region: String,
    pub source: crate::ops::types::ContextSource,
    pub context_type: String,
    pub description: Option<String>,
    pub properties: BTreeMap<String, String>,
    pub created_by: UserContext,
    pub last_modified_by: UserContext,
    /// Stored so a later `ListTags` reports what the create carried; `DescribeContext`
    /// models no `Tags` member, so this is the one reader that surfaces it.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}
