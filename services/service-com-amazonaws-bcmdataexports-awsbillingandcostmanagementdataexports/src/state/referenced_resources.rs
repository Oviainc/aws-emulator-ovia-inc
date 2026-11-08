// SPDX-License-Identifier: CC0-1.0

//! Resolving the resources OF OTHER SERVICES a submitted export definition names, against
//! the services that hold them.
//!
//! An export definition names two resources this service does not own:
//!
//! * the destination bucket — "The name of the Amazon S3 bucket used as the destination of
//!   a data export file", together with "The Amazon Web Services account ID that owns the
//!   S3 bucket used as the destination for the data export" (the modelled docs for
//!   `S3Destination$S3Bucket` and `$S3BucketOwner`), which live in Amazon S3;
//! * the billing view a table's data is scoped to, named by the `BILLING_VIEW_ARN` table
//!   property (see [`super::catalog::PROPERTY_BILLING_VIEW_ARN`]) — a free-form property
//!   whose value the caller writes rather than picks, and which lives in AWS Billing.
//!
//! Each is resolved where the service that owns it holds it, BEFORE anything is persisted:
//! an export delivered to a bucket that is not there, to a bucket owned by another account
//! than the request states, or scoped to a billing view that does not exist, could never be
//! served, so the caller would otherwise hold an ARN for an export that silently never
//! delivers. Both operations that accept an export definition — `CreateExport` and
//! `UpdateExport`, which "[updates] an existing data export by overwriting all export
//! parameters" — judge a submitted definition through [`validate_referenced_resources`], so
//! a replacement is resolved exactly as a create's definition is and a refused call leaves
//! whatever was stored before exactly as it was.
//!
//! Each owning service is reached the way the framework documents a handler reaching
//! another emulated service ([`smithy_server_runtime::OperationContext::callback`]): an AWS
//! SDK client over the in-process callback's HTTP client and credentials, so nothing here
//! opens a socket or leaves the host. The question is put with the operation that service
//! documents for it — Amazon S3's `HeadBucket` ("You can use this operation to determine if
//! a bucket exists"), carrying the request's own expected bucket owner, and AWS Billing's
//! `GetBillingView`.
//!
//! A refusal is grounded in what the owning service ANSWERED. Every other outcome — an
//! emulator assembled without that service, or any other failure to ask it — establishes
//! nothing about the resource, so it refuses nothing: this reading GATES on the resource
//! existing and keeps only the value the request named, and `services/common/src/network.rs`
//! states which resolutions refuse on silence and which do not.
//!
//! The one caller-supplied value that reaches a client's AUTHORITY is the signing Region,
//! narrowed first ([`service_common::arn::region_label`]), so no request can re-point a
//! client at a host of its own choosing: it reaches this emulator's own services and
//! nothing else.

use std::collections::BTreeSet;

use super::catalog::PROPERTY_BILLING_VIEW_ARN;
use super::destination::{S3_BUCKET_FIELD, S3_BUCKET_OWNER_FIELD};
// The member a refused billing view names is the one a refused table-property entry already
// names: the table configuration the request wrote the property at.
use super::query::TABLE_CONFIGURATIONS_FIELD;
use super::{clip_for_message, field_validation_failed, BUCKET_PERMISSION_VALIDATION_FAILED};
use crate::ops::types::{DataQuery, Export, S3Destination, ValidationException};

/// The HTTP status Amazon S3 answers a request whose stated bucket owner is not the
/// bucket's: "If the account ID that you provide does not match the actual owner of the
/// bucket, the request fails with the HTTP status code 403 Forbidden (access denied)" (the
/// modelled doc for `HeadBucketRequest$ExpectedBucketOwner`).
const FORBIDDEN: u16 = 403;

/// What the owning service says about the destination an export would be delivered to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationVerdict {
    /// Amazon S3 holds the bucket, and holds it for the owner the request stated (or the
    /// request stated none).
    Resolved,
    /// Amazon S3 reports that it holds no bucket under the name the request named.
    NoSuchBucket,
    /// Amazon S3 reports that the account the request named as the bucket's owner is not
    /// its owner.
    NotOwnedByStatedAccount,
    /// Nothing was established: there was no object store to ask, or it could not be
    /// asked. A refusal has to be grounded in an answer, so this refuses nothing.
    Unresolved,
}

/// Resolve every resource of another service `export` names, the way both operations that
/// accept an export definition must: `Ok(())` for a definition whose referenced resources
/// resolve (or about which nothing could be established), and the modelled
/// `ValidationException` (HTTP 400) for one the owning service reports it cannot serve.
///
/// Nothing here reads or writes this service's store, so a refusal leaves whatever was
/// stored before exactly as it was.
pub async fn validate_referenced_resources(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    export: &Export,
) -> Result<(), ValidationException> {
    match destination_bucket_verdict(ctx, &export.destination_configurations.s3_destination).await
    {
        DestinationVerdict::Resolved | DestinationVerdict::Unresolved => {}
        DestinationVerdict::NoSuchBucket => return Err(no_such_destination_bucket()),
        DestinationVerdict::NotOwnedByStatedAccount => {
            return Err(destination_not_owned_by_stated_account())
        }
    }
    if let Some(view) = unresolved_billing_view(ctx, &export.data_query).await {
        return Err(unknown_billing_view(&view));
    }
    Ok(())
}

/// What Amazon S3 says about the bucket `s3` names, and about the owner the same
/// destination states for it.
///
/// The expected bucket owner travels with the question rather than being compared here, so
/// the account that owns a bucket is the one the service HOLDING the bucket reports — the
/// same confused-deputy guard every caller of that store is judged by.
pub async fn destination_bucket_verdict(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    s3: &S3Destination,
) -> DestinationVerdict {
    if s3.s3_bucket.is_empty() {
        // The modelled `S3Bucket` admits the empty string (`@length(0, 1024)`), which names
        // no bucket to ask about: no object store holds one under it.
        return DestinationVerdict::NoSuchBucket;
    }
    // Which Region the bucket is looked up in decides nothing here — the emulator serves one
    // object store over the in-process callback — so a value outside the Region grammar falls
    // back to this service's own Region rather than putting caller text in the authority.
    let region = ::service_common::arn::region_label(&s3.s3_region).unwrap_or(super::SERVICE_REGION);
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in the
        // path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials the callback
        // hands over are used directly, and they only sign this in-process call.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    let mut question = ::aws_sdk_s3::Client::from_conf(config)
        .head_bucket()
        .bucket(&s3.s3_bucket);
    if let Some(owner) = &s3.s3_bucket_owner {
        question = question.expected_bucket_owner(owner);
    }
    match question.send().await {
        // Amazon S3 holds the bucket, for the owner the request stated if it stated one.
        Ok(_) => DestinationVerdict::Resolved,
        // Amazon S3 says it holds no such bucket: the refusal is grounded in its answer.
        Err(error) if reported_absent(&error) => DestinationVerdict::NoSuchBucket,
        // A request that STATED an owner and was answered the documented status for a
        // stated owner that is not the bucket's was answered about that owner.
        Err(error) if s3.s3_bucket_owner.is_some() && answered_forbidden(&error) => {
            DestinationVerdict::NotOwnedByStatedAccount
        }
        // Any other answer is this emulator failing to ask, not Amazon S3 reporting
        // anything about the bucket, so it refuses nothing — and it is worth a warning,
        // because a deployment with no object store to ask is one where this resolution
        // does not run. The caller's own value is rendered in its escaped `Debug` form, so
        // a bucket name carrying a newline cannot forge a log line.
        Err(error) => {
            ::tracing::warn!(
                bucket = ?s3.s3_bucket,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an export's destination bucket could not be resolved against Amazon S3"
            );
            DestinationVerdict::Unresolved
        }
    }
}

/// Whether an Amazon S3 `HeadBucket` failure is that service reporting that the bucket does
/// not exist (`NotFound`, the only error that operation models), as opposed to the call not
/// reaching it at all.
fn reported_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}

/// Whether Amazon S3 answered [`FORBIDDEN`], the status it documents for a request whose
/// stated bucket owner is not the bucket's.
///
/// Read off the status of the response that service sent, because a `HEAD` carries no body:
/// the status is the whole of what it answered.
fn answered_forbidden(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == FORBIDDEN)
}

/// The first `BILLING_VIEW_ARN` value `query` STATES that AWS Billing reports is no billing
/// view, or `None` when every stated value resolves (or nothing could be established).
///
/// Only the values the REQUEST states are resolved: a property the request leaves unsaid
/// takes the value the catalog publishes as its default, which for this property is the
/// account's own primary billing view — a view every account holds, minted by the service
/// that owns it rather than named by the caller.
///
/// The stated values are resolved IN SORTED ORDER and each one once, so a request naming
/// several unresolvable views is refused for the same one of them every time: the caller's
/// own map is a `HashMap`, whose iteration order is neither the request's nor stable between
/// two maps carrying the same entries.
///
/// How many questions this asks is bounded by the CATALOG, not by the request: every table
/// key of a submitted `TableConfigurations` is resolved against the published tables before
/// this runs ([`super::resolve_data_query`], which both operations call first and which
/// refuses a key no table publishes), so one request can state at most one value per
/// published table, and identical values collapse into one question.
pub async fn unresolved_billing_view(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    query: &DataQuery,
) -> Option<String> {
    let stated: BTreeSet<&str> = query
        .table_configurations
        .iter()
        .flatten()
        .filter_map(|(_, properties)| properties.get(PROPERTY_BILLING_VIEW_ARN))
        .map(String::as_str)
        .collect();
    if stated.is_empty() {
        return None;
    }
    // A billing view is a resource of the partition rather than of a Region ("Billing is a
    // global service"), so the client addresses the one endpoint this service's own Region
    // serves: no caller-supplied value reaches this authority at all.
    let region = super::SERVICE_REGION;
    let config = ::aws_sdk_billing::Config::builder()
        .behavior_version(::aws_sdk_billing::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_billing::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://billing.{region}.amazonaws.com"))
        // In-process, for the reasons stated on the object-store client above.
        .retry_config(::aws_sdk_billing::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_billing::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_billing::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_billing::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    let billing = ::aws_sdk_billing::Client::from_conf(config);
    for arn in stated {
        match billing.get_billing_view().arn(arn).send().await {
            // AWS Billing holds the view.
            Ok(_) => {}
            // AWS Billing says the value names no view of this account — either no view is
            // stored under it, or it is not an ARN that could name one: the refusal is
            // grounded in its answer.
            Err(error) if reported_no_such_view(&error) => return Some(arn.to_string()),
            // Any other answer is this emulator failing to ask, so it refuses nothing.
            Err(error) => {
                ::tracing::warn!(
                    billing_view = ?arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "an export's billing view could not be resolved against AWS Billing"
                );
            }
        }
    }
    None
}

/// Whether an AWS Billing `GetBillingView` failure is that service reporting that the value
/// names no billing view of this account — no view stored under the ARN
/// (`ResourceNotFoundException`), or a value it cannot read as one of its ARNs at all
/// (`ValidationException`) — as opposed to the call not reaching it, or reaching it and
/// being refused for a reason that says nothing about whether the view exists.
fn reported_no_such_view(
    error: &::aws_sdk_billing::error::SdkError<
        ::aws_sdk_billing::operation::get_billing_view::GetBillingViewError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error.as_service_error().is_some_and(|service| {
        service.is_resource_not_found_exception() || service.is_validation_exception()
    })
}

/// The modelled 400 for a destination whose bucket Amazon S3 reports it does not hold.
///
/// The `Message` is the recorded sentence for the destination check this service performs
/// before it accepts a definition ([`BUCKET_PERMISSION_VALIDATION_FAILED`]): a bucket that
/// is not there carries no permissions to validate, so it is the same answer to the
/// caller — the destination's permissions were checked and did not pass. The member at
/// fault is the destination's `S3Bucket`, which is what the caller changes (either by
/// naming a bucket that exists or by creating this one).
fn no_such_destination_bucket() -> ValidationException {
    field_validation_failed(
        S3_BUCKET_FIELD,
        BUCKET_PERMISSION_VALIDATION_FAILED.to_string(),
        "Must name an Amazon S3 bucket that exists.".to_string(),
    )
}

/// The modelled 400 for a destination whose stated `S3BucketOwner` is not the account
/// Amazon S3 reports as the bucket's owner.
///
/// Same recorded sentence, for the same reason: the destination's permissions were checked
/// against the bucket the request named and did not pass. The member at fault is the
/// `S3BucketOwner` the request stated, which is the value the caller changes.
fn destination_not_owned_by_stated_account() -> ValidationException {
    field_validation_failed(
        S3_BUCKET_OWNER_FIELD,
        BUCKET_PERMISSION_VALIDATION_FAILED.to_string(),
        "Must name the Amazon Web Services account that owns the destination bucket."
            .to_string(),
    )
}

/// The modelled 400 for a `BILLING_VIEW_ARN` table-property value that names no billing
/// view of this account.
///
/// The sentence is built from the request — it echoes the offending value, clipped to the
/// message member's own modelled bound — because that value is the only part of the request
/// the caller has to change, and it follows the form this service already reports a named
/// resource it could not resolve in (`Table <name> not found`, see
/// [`super::catalog::unknown_table`]).
fn unknown_billing_view(arn: &str) -> ValidationException {
    field_validation_failed(
        TABLE_CONFIGURATIONS_FIELD,
        format!("Billing view {} not found", clip_for_message(arn)),
        format!("{PROPERTY_BILLING_VIEW_ARN} must name a billing view of this account."),
    )
}
