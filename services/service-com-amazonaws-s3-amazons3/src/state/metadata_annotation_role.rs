// SPDX-License-Identifier: CC0-1.0

//! Whether the IAM role a metadata configuration names for its ANNOTATION table lets S3
//! Metadata read annotations through it.
//!
//! "If you include an `AnnotationTableConfiguration` with an IAM role, the role must have a
//! trust policy that allows the Amazon S3 metadata service to assume it, and a permissions
//! policy that grants the actions needed to read annotations from your bucket"
//! (`CreateBucketMetadataConfiguration`'s own documentation): the annotation table is filled
//! THROUGH that role, so the role is what decides whether the table can be created at all.
//!
//! The verdict is a PROVISIONING OUTCOME, not a refusal: the create and the update that name
//! the table are served, and the table itself is recorded
//! [`super::METADATA_TABLE_STATUS_FAILED`] carrying [`ANNOTATION_ROLE_PERMISSIONS_CODE`],
//! which is what `GetBucketMetadataConfiguration` reports as that table's `TableStatus` and
//! `Error` — exactly where the published provisioning failures of the journal and inventory
//! tables leave those two ([`super::metadata_inventory_table_failure`]).
//!
//! IAM is reached the way any emulated service reaches another, through the in-process
//! callback, by the reading every service resolving a caller-supplied role shares
//! (`service_common::iam_role`) — the same reading this service's replication role takes
//! ([`super::replication::role_access`]). Nothing is withheld that IAM did not answer: a
//! value that names no role of this account, an answer this emulator could not read and an
//! emulator with no IAM mounted all leave the table to be created.

/// The service principal S3 Metadata assumes an annotation role AS, and so the one such a
/// role's trust policy has to admit for the table to be filled through it: the
/// `ErrorDetails$ErrorCode` documentation names `metadata.s3.amazonaws.com` as the S3
/// service principal of a metadata configuration's tables
/// (`smithy_query(query="doc",shape="ErrorDetails",member="ErrorCode")`).
pub const METADATA_SERVICE_PRINCIPAL: &str = "metadata.s3.amazonaws.com";

/// The `ErrorDetails$ErrorCode` an annotation table carries when the role it would be filled
/// through does not grant S3 Metadata what reading annotations needs.
pub const ANNOTATION_ROLE_PERMISSIONS_CODE: &str = "InsufficientAnnotationRolePermissions";

/// See [`ANNOTATION_ROLE_PERMISSIONS_CODE`] — the sentence that code carries.
///
/// Worded as the published V2 provisioning failures of the other tables are
/// (`smithy_query(query="doc",shape="ErrorDetails",member="ErrorCode")`, which lists no entry
/// for the annotation table): what is wrong, then "make sure that" the requirement — which is
/// the one the create states for this role, "the role must have a trust policy that allows
/// the Amazon S3 metadata service to assume it, and a permissions policy that grants the
/// actions needed to read annotations from your bucket" — and then what every entry in that
/// list closes with, because a table that failed is replaced by re-creating the
/// configuration.
pub const ANNOTATION_ROLE_PERMISSIONS_MESSAGE: &str =
    "The IAM role for the annotation table doesn't have sufficient permissions. Make sure \
     that the role has a trust policy that allows the Amazon S3 metadata service to assume \
     it, and a permissions policy that grants the actions needed to read annotations from \
     your bucket. To create a new metadata table, you must delete the metadata configuration \
     for this bucket, and then create a new metadata configuration.";

/// Why an annotation table provisioned NOW under `role` could not be created, or `None` when
/// nothing IAM answered stops it.
///
/// ONE rule for every call that provisions an annotation table — the create that names a
/// whole configuration and the update that enables the table on one — so the two cannot
/// drift apart on it. `role` is the role that would be in effect for the table (the one the
/// call names, or failing that the one the configuration already carries); a call that leaves
/// the table with no role at all never reaches here, because an enabled annotation table
/// always has one.
///
/// Asked BEFORE this service's own guard is taken, so no state lock is held across the hop
/// into IAM.
pub async fn provisioning_failure(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    role: Option<&str>,
) -> Option<super::MetadataTableFailure> {
    let role = role?;
    ::service_common::iam_role::role_is_unassumable(
        ctx,
        &super::request_region(ctx.parts()),
        role,
        METADATA_SERVICE_PRINCIPAL,
    )
    .await
    .then_some(super::MetadataTableFailure {
        code: ANNOTATION_ROLE_PERMISSIONS_CODE,
        message: ANNOTATION_ROLE_PERMISSIONS_MESSAGE,
    })
}
