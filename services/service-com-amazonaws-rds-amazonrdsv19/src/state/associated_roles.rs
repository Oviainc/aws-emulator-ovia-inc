// SPDX-License-Identifier: CC0-1.0

//! The IAM roles associated with a DB instance or a DB cluster: the rules
//! `AddRoleToDBInstance`, `AddRoleToDBCluster`, `RemoveRoleFromDBInstance` and
//! `RemoveRoleFromDBCluster` share.
//!
//! An association is what lets a database engine act under a caller-supplied role for ONE
//! feature of that engine — loading from and unloading to Amazon S3, invoking a Lambda
//! function, calling an Aurora machine-learning service — so the four operations all answer
//! the same two questions before they write: does the ARN name a role this account's IAM
//! holds that `rds.amazonaws.com` may assume, and is the `FeatureName` one the addressed
//! resource's ENGINE VERSION publishes. Both questions have one answer per request
//! whichever resource the request addressed, so they are stated once here rather than twice
//! in the instance and cluster handlers.
//!
//! The feature names are not a list of this module's own: they are the
//! `DBEngineVersion$SupportedFeatureNames` this service already publishes for that engine
//! version (`FeatureName` documents them as "the feature for the DB instance that the IAM
//! role is to be associated with. For information about supported feature names, see
//! DBEngineVersion"), read from the one catalogue
//! [`crate::ops::describe_db_engine_versions`] reports them from, so a client that read the
//! list and a request that names one of its entries cannot disagree.

use super::*;

/// The service principal a role must trust for Amazon RDS to assume it on a DB instance's
/// or DB cluster's behalf: the principal `rds.amazonaws.com`, which is what the trust
/// policy of every role in "Setting up IAM roles to access Amazon S3" (Amazon RDS User
/// Guide) grants `sts:AssumeRole` to.
pub const RDS_SERVICE_PRINCIPAL: &str = "rds.amazonaws.com";

/// The service principal a role must trust for an Amazon RDS snapshot EXPORT to assume it:
/// "export.rds.amazonaws.com", the principal the trust policy in "Exporting DB snapshot
/// data to Amazon S3" (Amazon RDS User Guide) grants `sts:AssumeRole` to.
pub const EXPORT_SERVICE_PRINCIPAL: &str = "export.rds.amazonaws.com";

/// How many IAM roles one DB instance or DB cluster may have associated with it at once:
/// the "IAM roles per DB instance" / "IAM roles per DB cluster" quota Amazon RDS publishes
/// (Quotas and constraints for Amazon RDS, Amazon RDS User Guide), which is what the
/// modelled `DBInstanceRoleQuotaExceededFault` / `DBClusterRoleQuotaExceededFault` reports
/// having reached.
pub const MAX_ASSOCIATED_ROLES: usize = 5;

/// The `SupportedFeatureNames` of the engine version a resource runs — the `FeatureName`
/// values an association on it may name.
///
/// Resolved through the published catalogue, exactly as `DescribeDBEngineVersions` resolves
/// it: a request that names no version reads the engine's default version's list, which is
/// the version such a resource was created with. An engine the catalogue publishes nothing
/// for has no published list at all, which is reported as an empty one — there is then no
/// feature name this service can say is supported.
#[must_use]
pub fn supported_feature_names(engine: &str, engine_version: Option<&str>) -> &'static [&'static str] {
    crate::ops::describe_db_engine_versions::resolve_version(engine, engine_version)
        .ok()
        .flatten()
        .map_or(&[], |version| version.profile.feature_names)
}

/// Whether `feature_name` is one the engine version this resource runs publishes.
///
/// Compared exactly: a feature name is a published identifier (`s3Import`, `S3_INTEGRATION`,
/// `SQLSERVER_BACKUP_RESTORE`), and the catalogue states each one in the case AWS publishes
/// it in, so a value in another case names no published feature.
#[must_use]
pub fn feature_name_is_supported(
    engine: &str,
    engine_version: Option<&str>,
    feature_name: &str,
) -> bool {
    supported_feature_names(engine, engine_version).contains(&feature_name)
}

/// What a request naming a `FeatureName` the addressed resource's engine version does not
/// publish is refused with.
///
/// The sentence names the value that was rejected and the names that engine version does
/// publish, because the caller has to fix the request from this text alone — and those names
/// are the `SupportedFeatureNames` of that version, so the text and the catalogue a client
/// reads cannot disagree.
#[must_use]
pub fn unsupported_feature_name_message(
    engine: &str,
    engine_version: Option<&str>,
    feature_name: &str,
) -> String {
    let supported = supported_feature_names(engine, engine_version);
    if supported.is_empty() {
        return format!(
            "The feature name {feature_name} is not supported for engine {engine}; that engine \
             publishes no feature an IAM role can be associated for."
        );
    }
    format!(
        "The feature name {feature_name} is not supported for engine {engine}; supported \
         feature names are {}.",
        supported.join(", ")
    )
}

/// What a request naming a role Amazon RDS cannot assume is refused with: the role's trust
/// policy has to grant `sts:AssumeRole` to the service principal that will act under it.
///
/// Named with the principal the trust policy must carry, because that is the part of the
/// role the caller has to change.
#[must_use]
pub fn role_not_trusted_message(role_arn: &str, service_principal: &str) -> String {
    format!(
        "The IAM role {role_arn} cannot be assumed by Amazon RDS. Add {service_principal} to \
         the trust relationship of the role and try again."
    )
}

/// Whether `roles` already carries an association of `role_arn` for `feature_name`.
///
/// The pair is the identity of an association — one role can serve two features of a
/// resource, and two roles can serve two features — which is what the already-exists faults
/// of both add operations report ("The specified RoleArn or FeatureName value is already
/// associated with the DB instance"). A `FeatureName` the request left out compares as the
/// association that names none.
#[must_use]
pub fn holds_association(
    roles: &[(String, Option<String>)],
    role_arn: &str,
    feature_name: Option<&str>,
) -> bool {
    roles.iter().any(|(held_arn, held_feature)| {
        held_arn == role_arn && held_feature.as_deref() == feature_name
    })
}

/// How Amazon RDS words the already-exists fault of an association: it names the role and
/// the feature the resource already has that role associated for.
#[must_use]
pub fn role_already_associated_message(
    resource: &str,
    identifier: &str,
    role_arn: &str,
    feature_name: Option<&str>,
) -> String {
    match feature_name {
        Some(feature) => format!(
            "The IAM role {role_arn} is already associated with the {resource} {identifier} for \
             the feature {feature}."
        ),
        None => format!(
            "The IAM role {role_arn} is already associated with the {resource} {identifier}."
        ),
    }
}

/// How Amazon RDS words the role-not-found fault of a disassociation: the role the request
/// named is not associated with that resource, so there is nothing to remove.
#[must_use]
pub fn role_not_associated_message(
    resource: &str,
    identifier: &str,
    role_arn: &str,
    feature_name: Option<&str>,
) -> String {
    match feature_name {
        Some(feature) => format!(
            "The IAM role {role_arn} is not associated with the {resource} {identifier} for the \
             feature {feature}."
        ),
        None => {
            format!("The IAM role {role_arn} is not associated with the {resource} {identifier}.")
        }
    }
}

/// How Amazon RDS words the quota fault of an association: the resource already carries
/// every role it may.
#[must_use]
pub fn role_quota_message(resource: &str, identifier: &str) -> String {
    format!(
        "The {resource} {identifier} already has {MAX_ASSOCIATED_ROLES} IAM roles associated \
         with it, which is the maximum."
    )
}

/// The `(RoleArn, FeatureName)` pairs of a stored DB cluster's associations, which is the
/// form [`holds_association`] reads them in.
#[must_use]
pub fn cluster_role_pairs(roles: &[DBClusterRole]) -> Vec<(String, Option<String>)> {
    roles
        .iter()
        .map(|role| {
            (
                role.role_arn.clone().unwrap_or_default(),
                role.feature_name.clone(),
            )
        })
        .collect()
}

/// The same pairs for a stored DB instance's associations.
#[must_use]
pub fn instance_role_pairs(roles: &[DBInstanceRole]) -> Vec<(String, Option<String>)> {
    roles
        .iter()
        .map(|role| {
            (
                role.role_arn.clone().unwrap_or_default(),
                role.feature_name.clone(),
            )
        })
        .collect()
}
