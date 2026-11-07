// SPDX-License-Identifier: CC0-1.0

//! The `AWS::S3::Bucket` provider's half of provisioning that is NOT its create: how a
//! CHANGED declaration reaches a bucket that already exists, and how that provider words a
//! refusal Amazon S3 answered.
//!
//! The create, the delete and the drift read for this type are [`super::super::provisioner`]'s
//! arms, where every catalogued type's are. These two are here because that file is far past
//! the length a change to this repository may touch, and a type's provider needs both halves
//! — so the parts added now live beside the algorithm that drives them, reached through the
//! same seam every per-type module is ([`super::Owner`]).
//!
//! ## A changed declaration
//!
//! A bucket is not re-created to change it. "You can use Amazon S3 to set, read and delete"
//! each of a bucket's sub-resources on the bucket that exists, so a property whose own `Put`
//! call Amazon S3 publishes is applied by THAT call to the bucket the stack created, and the
//! resource keeps its identity ([`update`]):
//!
//! * `Tags` are a `PutBucketTagging` — "sets the tags for a bucket" — reconciled where every
//!   resource's tag set is ([`retag`]), because the set a bucket ends up carrying is the
//!   declaration's tags plus the ones CloudFormation propagates to it, and those change with
//!   no declaration changing at all;
//! * `VersioningConfiguration` is a `PutBucketVersioning`, the one call that writes the
//!   versioning state of a bucket that exists.
//!
//! Every other property is reported as NOT applied, exactly as a type with no update arm at
//! all is ([`super::Owner::update`]): the change is the caller's and nothing here applied it,
//! so the deployment says so rather than recording a configuration Amazon S3 was never asked
//! for.
//!
//! ## A refusal this provider reports
//!
//! What CloudFormation quotes in a failed resource's `ResourceStatusReason` is the message
//! the resource's own PROVIDER reported ([`super::handler_failure_reason`]). This provider
//! reports a name already in use as the bucket name it asked for, followed by the fixed
//! service diagnostic its own SDK call carries ([`handler_message`]) — so the event says
//! WHICH bucket collided, which the service's own sentence about the caller already owning it
//! does not.
//!
//! WHICH type these two halves belong to is [`super::Owner`]'s to say: the arms that reach
//! them are the ones matching the catalogue's own `AWS::S3::Bucket` provisioner, so the type
//! is named once, where the catalogue names it.

use ::serde_json::Value;

use super::{Created, Refusal};
use crate::state::provisioner::{s3_client, tags_of};
use crate::state::StoredResource;

/// The declaration's own tag set, reconciled by [`retag`] rather than by [`update`].
const TAGS: &str = "Tags";

/// The declaration's versioning state, written by `PutBucketVersioning`.
const VERSIONING_CONFIGURATION: &str = "VersioningConfiguration";

/// The one member that configuration carries.
const VERSIONING_STATUS: &str = "Status";

/// The properties a changed declaration of a bucket that exists is APPLIED for: the ones
/// above, each by the one Amazon S3 call that writes it on a live bucket. A declaration that
/// changes anything else is reported as not applied.
const APPLIED_PROPERTIES: &[&str] = &[TAGS, VERSIONING_CONFIGURATION];

/// The `HandlerErrorCode` class a name already in use falls in
/// ([`super::HANDLER_ERROR_CLASSES`]), which is what this provider words its own sentence
/// for.
const ALREADY_EXISTS: &str = "AlreadyExists";

/// Apply a CHANGED declaration to a bucket that already exists.
///
/// `Ok(Some(…))` once every property that changed has been applied — the tags included, which
/// [`retag`] applies in the same deployment, immediately after this call
/// ([`super::reconcile`]) — and `Ok(None)` where the declaration changed a property no call
/// here writes, which the deployment reports as a change that was not applied.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let changed = changed_properties(&resource.properties, declared);
    if !changed.iter().all(|property| {
        APPLIED_PROPERTIES.contains(&property.as_str())
    }) {
        return Ok(None);
    }
    if changed.iter().any(|property| property == VERSIONING_CONFIGURATION) {
        put_versioning(ctx, region, &resource.physical_resource_id, declared).await?;
    }
    Ok(Some(Created::default()))
}

/// "PutBucketVersioning — sets the versioning state of an existing bucket": the state the
/// declaration now asks for, sent for the bucket the stack created.
///
/// A declaration that no longer carries the configuration asks for no state at all, and
/// Amazon S3 publishes no call that un-sets one — the two states a bucket can be put in are
/// `Enabled` and `Suspended` — so nothing is sent for it and the bucket keeps the state it
/// has. The value itself is the template's: Amazon S3's own deserializer is what judges it
/// against `BucketVersioningStatus`, exactly as it does on the create.
async fn put_versioning(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    declared: &Value,
) -> Result<(), Refusal> {
    use ::aws_sdk_s3::types::{BucketVersioningStatus, VersioningConfiguration};

    let Some(status) = declared
        .get(VERSIONING_CONFIGURATION)
        .and_then(|configuration| configuration.get(VERSIONING_STATUS))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    s3_client(ctx, region)
        .put_bucket_versioning()
        .bucket(bucket)
        .versioning_configuration(
            VersioningConfiguration::builder()
                .status(BucketVersioningStatus::from(status))
                .build(),
        )
        .send()
        .await
        .map_err(|error| refusal_of(&error))?;
    Ok(())
}

/// Bring the tags of a bucket the stack ALREADY has back into step with the set it must
/// carry: the declaration's own `Tags` plus the ones CloudFormation propagates to it
/// ([`crate::state::provisioner::tags_of`], the one reading of that set).
///
/// `PutBucketTagging` "sets the tags for a bucket", replacing the whole set, and
/// `DeleteBucketTagging` "removes the entire tag set" — which is what a bucket that must end
/// up carrying none is left with. IDEMPOTENT: the set in force is read first
/// (`GetBucketTagging`), so a bucket whose tags already agree with it has nothing written.
pub(super) async fn retag(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
) -> Result<(), Refusal> {
    use ::aws_sdk_s3::types::{Tag, Tagging};

    let client = s3_client(ctx, region);
    let bucket = &resource.physical_resource_id;
    let wanted = tags_of(declared, &applicable(propagated));
    // "NoSuchTagSet" is what Amazon S3 answers for a bucket carrying no tag set at all,
    // which is a bucket whose tags are the empty set rather than a read that failed.
    let in_force: Vec<(String, String)> = match client
        .get_bucket_tagging()
        .bucket(bucket)
        .send()
        .await
    {
        Ok(answer) => answer
            .tag_set()
            .iter()
            .map(|tag| (tag.key().to_string(), tag.value().to_string()))
            .collect(),
        Err(_) => Vec::new(),
    };
    if tag_sets_agree(&in_force, &wanted) {
        return Ok(());
    }
    if wanted.is_empty() {
        client
            .delete_bucket_tagging()
            .bucket(bucket)
            .send()
            .await
            .map_err(|error| refusal_of(&error))?;
        return Ok(());
    }
    let mut tag_set = Vec::with_capacity(wanted.len());
    for (key, value) in &wanted {
        tag_set.push(
            Tag::builder()
                .key(key)
                .value(value)
                .build()
                .map_err(|error| Refusal {
                    code: UNIDENTIFIED_FAILURE.to_string(),
                    message: format!("the tag {key} cannot be sent to Amazon S3: {error}"),
                })?,
        );
    }
    let tagging = Tagging::builder()
        .set_tag_set(Some(tag_set))
        .build()
        .map_err(|error| Refusal {
            code: UNIDENTIFIED_FAILURE.to_string(),
            message: format!("the tag set for {bucket} cannot be sent: {error}"),
        })?;
    client
        .put_bucket_tagging()
        .bucket(bucket)
        .tagging(tagging)
        .send()
        .await
        .map_err(|error| refusal_of(&error))?;
    Ok(())
}

/// AWS's reserved tag-key namespace and the tags an owning service's tagging call will take
/// are read through the ONE predicate the module beside this one holds
/// ([`super::is_reserved_tag_key`], [`super::applicable_tags`]), so this provider and the
/// nested-stack one cannot come to read the rule differently.
use super::{applicable_tags as applicable, tag_sets_agree};

/// The properties whose VALUE the two documents state differently, in a stable order: what a
/// changed declaration asks this provider to apply.
///
/// Over the union of both documents' members, so a property the declaration DROPPED is a
/// change as much as one it added.
fn changed_properties(deployed: &Value, declared: &Value) -> Vec<String> {
    let mut changed: Vec<String> = Vec::new();
    let mut consider = |property: &String| {
        if deployed.get(property) != declared.get(property)
            && !changed.iter().any(|held| held == property)
        {
            changed.push(property.clone());
        }
    };
    for document in [deployed, declared] {
        if let Some(members) = document.as_object() {
            for property in members.keys() {
                consider(property);
            }
        }
    }
    changed
}

/// The error code this module reports where nothing on the other end identified the problem,
/// the same one [`super::super::provisioner`] reports it under.
const UNIDENTIFIED_FAILURE: &str = "InternalFailure";

/// An SDK error as the [`Refusal`] the stack reports: the code Amazon S3 NAMED and the
/// message it worded, kept apart so the caller branches on the code.
fn refusal_of<E>(error: &E) -> Refusal
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::fmt::Display,
{
    Refusal {
        code: error.code().unwrap_or(UNIDENTIFIED_FAILURE).to_string(),
        message: error
            .message()
            .map_or_else(|| error.to_string(), ToString::to_string),
    }
}

/// How THIS provider words a refusal Amazon S3 answered, which is the message CloudFormation
/// quotes in the failed bucket's status reason.
///
/// A name already in use is reported as the NAME the declaration asked for, followed by the
/// diagnostic the provider's own call carries: the service it called, and the status and
/// request id its answer identified the call by — which for this provider's report of a
/// collision are the fixed values below, since the collision is found before any call it
/// could number. Every other refusal is reported as Amazon S3 worded it.
///
/// `bucket` is the bucket the declaration asked for: the resource's physical id, which for
/// this type IS the bucket name ("Ref returns the bucket name").
pub(super) fn handler_message(class: &str, bucket: &str, message: &str) -> String {
    if class == ALREADY_EXISTS {
        return format!(
            "{bucket} already exists (Service: {COLLISION_SERVICE}, Status Code: \
             {COLLISION_STATUS}, Request ID: {COLLISION_REQUEST_ID})"
        );
    }
    message.to_string()
}

/// The service, status and request id this provider's collision report states: Amazon S3's
/// own short name, and the two values its report of a collision carries in place of a call it
/// numbered.
const COLLISION_SERVICE: &str = "S3";
const COLLISION_STATUS: &str = "0";
const COLLISION_REQUEST_ID: &str = "null";

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a changed declaration is applied only where a call here writes the property —
    /// the rows differ along WHICH property changed and in which direction (added, changed,
    /// dropped).
    #[test]
    fn only_the_properties_this_provider_writes_are_reported_as_applied() {
        let rows: &[(&str, Value, Value, bool)] = &[
            (
                "a tag set added to a bucket that carried none",
                ::serde_json::json!({"BucketName": "assets"}),
                ::serde_json::json!({"BucketName": "assets", "Tags": [{"Key": "owner", "Value": "platform"}]}),
                true,
            ),
            (
                "a tag VALUE changed",
                ::serde_json::json!({"Tags": [{"Key": "owner", "Value": "platform"}]}),
                ::serde_json::json!({"Tags": [{"Key": "owner", "Value": "data"}]}),
                true,
            ),
            (
                "a tag set dropped",
                ::serde_json::json!({"Tags": [{"Key": "owner", "Value": "platform"}]}),
                ::serde_json::json!({}),
                true,
            ),
            (
                "versioning enabled on a bucket that had none",
                ::serde_json::json!({}),
                ::serde_json::json!({"VersioningConfiguration": {"Status": "Enabled"}}),
                true,
            ),
            (
                "the tags and the versioning state together",
                ::serde_json::json!({"Tags": []}),
                ::serde_json::json!({
                    "Tags": [{"Key": "owner", "Value": "platform"}],
                    "VersioningConfiguration": {"Status": "Suspended"}
                }),
                true,
            ),
            (
                "a property no call here writes",
                ::serde_json::json!({}),
                ::serde_json::json!({"AccessControl": "PublicRead"}),
                false,
            ),
            (
                "that property beside one this provider does write",
                ::serde_json::json!({}),
                ::serde_json::json!({
                    "Tags": [{"Key": "owner", "Value": "platform"}],
                    "WebsiteConfiguration": {"IndexDocument": "index.html"}
                }),
                false,
            ),
        ];
        for (case, deployed, declared, applied) in rows {
            let changed = changed_properties(deployed, declared);
            let all_applied = changed
                .iter()
                .all(|property| APPLIED_PROPERTIES.contains(&property.as_str()));
            assert_eq!(
                all_applied, *applied,
                "case {case:?}: expected applied={applied}, got {all_applied} over {changed:?}"
            );
        }
    }

    /// RULE: the collision this provider reports names the bucket it asked for; every other
    /// refusal is reported as Amazon S3 worded it.
    #[test]
    fn a_collision_is_reported_as_the_bucket_name_it_asked_for() {
        let rows: &[(&str, &str, &str, &str, &str)] = &[
            (
                "a name another caller already owns",
                ALREADY_EXISTS,
                "team-assets",
                "Your previous request to create the named bucket succeeded and you already \
                 own it.",
                "team-assets already exists (Service: S3, Status Code: 0, Request ID: null)",
            ),
            (
                "a name the account itself already owns",
                ALREADY_EXISTS,
                "other-assets",
                "The requested bucket name is not available.",
                "other-assets already exists (Service: S3, Status Code: 0, Request ID: null)",
            ),
            (
                "a refusal of another class",
                "InvalidRequest",
                "team-assets",
                "The specified bucket is not valid.",
                "The specified bucket is not valid.",
            ),
            (
                "a read Amazon S3 will not serve",
                "AccessDenied",
                "team-assets",
                "Access Denied",
                "Access Denied",
            ),
        ];
        for (case, class, bucket, message, expected) in rows {
            let reported = handler_message(class, bucket, message);
            assert_eq!(
                reported, *expected,
                "case {case:?}: expected {expected:?}, got {reported:?}"
            );
        }
    }
}
