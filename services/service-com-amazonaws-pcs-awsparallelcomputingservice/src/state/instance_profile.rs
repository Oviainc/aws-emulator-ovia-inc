// SPDX-License-Identifier: CC0-1.0

//! The IAM instance profile a compute node group's instances assume, as the service that
//! owns instance profiles reports it.
//!
//! An instance profile is not a resource of this service: it is IAM's. A client creates one
//! with `iam:CreateInstanceProfile` and then names it in
//! `CreateComputeNodeGroup$iamInstanceProfileArn`, "the Amazon Resource Name (ARN) of the
//! IAM instance profile used to pass an IAM role when launching EC2 instances" (that
//! member's own documentation) — so IAM is the ONE answer to "does this account hold that
//! instance profile", and the reference a request carries is resolved there and nowhere
//! else. IAM is reached the way any emulated service reaches another, over the in-process
//! callback through the tree's one construction of that hop
//! (`service_common::iam_client`), so this crate names no other service's crate and nothing
//! leaves the process.
//!
//! WHEN the reference is resolved is part of the behaviour: a create resolves what it
//! REFERENCES in another service before it resolves the resource of its own the request
//! names, so a request naming both a profile that does not exist and a cluster that does
//! not exist is answered about the profile. The read and delete paths of a compute node
//! group reference no profile and keep resolving their cluster first.
//!
//! A refusal grounded in IAM's own answer carries that answer through, in the shape a
//! service reports a failure of a call it made on the caller's behalf
//! ([`carried_through`]); a value that names no instance profile of this account is refused
//! before anybody is asked, in this service's own words
//! ([`not_of_this_account_message`]).
//!
//! Nothing is cached and nothing is stored here: a profile created a moment ago resolves at
//! once, and one that was deleted stops resolving at once.

use ::aws_sdk_iam::error::ProvideErrorMetadata;

use super::{service_validation, ValidationException};

/// The service the tail of a carried-through refusal names, as the SDK of the service that
/// answered names itself.
const IAM_SERVICE: &str = "Iam";

/// The status the tail reports when IAM's own answer carried none: `NoSuchEntity` is
/// modelled `@httpError(404)`, which is the status the answer this reads came with.
const NOT_FOUND_STATUS: u16 = 404;

/// How many attempts the tail of a carried-through refusal reports.
///
/// The client this service asks IAM with is built with retries disabled
/// (`service_common::iam_client`), so the call behind the refusal was attempted exactly
/// once; the count is a fact about that call rather than a value to pick.
const SDK_ATTEMPT_COUNT: u32 = 1;

/// The sentence this service refuses a value that names no instance profile of this account
/// with, built from the REQUEST: its one variable part is the value the caller sent, so the
/// caller is told which of its references names nothing.
///
/// Nobody was asked about such a value, so nothing of another service's answer is stated
/// for it: an instance profile of ANOTHER account is not one this account's instances can be
/// handed, and an ARN of another IAM entity type names a different kind of resource
/// altogether.
fn not_of_this_account_message(arn: &str) -> String {
    format!("The instance profile {arn} isn't an IAM instance profile of this account.")
}

/// The message IAM answered, carried through with the tail AWS appends to a failure of a
/// call a service made on the caller's behalf: the service that answered, the status it
/// answered with, the id of that call, and how many attempts were made to reach it.
///
/// A caller (or its tooling) correlates the refusal with the call behind it by that tail, so
/// it is part of the message rather than an addition to it. The request id is drawn afresh
/// per call (`service_common::query_error_envelope::new_request_id`) because that is what a
/// request id IS: nothing in the request decides it.
fn carried_through(message: &str, status: u16) -> String {
    format!(
        "{message} (Service: {IAM_SERVICE}, Status Code: {status}, Request ID: {}) \
         (SDK Attempt Count: {SDK_ATTEMPT_COUNT})",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// The modelled `ValidationException` (HTTP 400) a create naming `arn` is refused with, or
/// `None` when the create may proceed.
///
/// `Some` where the reference resolves to no instance profile this account holds: the value
/// names no instance profile of this account at all, or IAM answered that it holds no such
/// entity under the name the ARN carries.
///
/// `None` — refuse nothing — for every answer that is NOT such a report: a hop that could
/// not be made at all is this emulator failing to ask rather than IAM reporting an
/// unresolvable reference, so the create proceeds. That covers an emulator with no IAM
/// mounted, where nothing is known about the profile and a refusal would be assumed rather
/// than grounded, and an answer this emulator could not read, since what a refusal states
/// is what IAM answered.
pub async fn violation(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    arn: &str,
) -> Option<ValidationException> {
    // An instance profile is named to IAM by the name it holds it under; the ARN's path is
    // not part of that name.
    let Some(name) = ::service_common::iam_role::instance_profile_name(arn) else {
        return Some(service_validation(not_of_this_account_message(arn)));
    };
    let answer = ::service_common::iam_client::iam_client(ctx, region)
        .get_instance_profile()
        .instance_profile_name(name)
        .send()
        .await;
    let Err(error) = answer else {
        return None;
    };
    // IAM reporting that this account holds no such instance profile is the one answer a
    // refusal is grounded in; anything else is this emulator failing to ask.
    if !profile_is_absent(&error) {
        // At `warn`: a deployment where this hop fails is one where this gate does not run
        // at all, which is worth seeing, and it IS why a reference was not resolved.
        ::tracing::warn!(
            %region,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the instance profile a compute node group named could not be resolved against \
             IAM"
        );
        return None;
    }
    let reported = error.as_service_error().and_then(ProvideErrorMetadata::message)?;
    let status = error
        .raw_response()
        .map_or(NOT_FOUND_STATUS, |response| response.status().as_u16());
    Some(service_validation(carried_through(reported, status)))
}

/// Whether an IAM `GetInstanceProfile` failure is IAM reporting that the profile does not
/// exist (`NoSuchEntity`), as opposed to the call not reaching IAM.
fn profile_is_absent(
    error: &::aws_sdk_iam::error::SdkError<
        ::aws_sdk_iam::operation::get_instance_profile::GetInstanceProfileError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error.as_service_error().is_some_and(
        ::aws_sdk_iam::operation::get_instance_profile::GetInstanceProfileError::is_no_such_entity_exception,
    )
}
