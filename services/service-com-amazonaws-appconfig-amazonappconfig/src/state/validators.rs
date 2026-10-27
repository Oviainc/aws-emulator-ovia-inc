// SPDX-License-Identifier: CC0-1.0

//! A configuration profile's validators: the check the configuration data has to pass
//! before a call that puts it in service may proceed.
//!
//! "A validator provides a syntactic or semantic check to ensure the configuration that
//! you want to deploy functions as intended. To validate your application configuration
//! data, you provide a schema or an Amazon Web Services Lambda function that runs against
//! the configuration. The configuration deployment or update can only proceed when the
//! configuration data is valid" (`smithy_query(query="doc",shape="Validator")`). So a
//! profile's `Validators` are not metadata that merely round-trips: an operation AppConfig
//! documents as validating a configuration runs EVERY validator the profile carries against
//! the candidate version's content before it does anything, and a validator that reports
//! the content non-compliant refuses that call with `BadRequestException` (HTTP 400),
//! leaving nothing behind.
//!
//! A validator's `Content` is "either the JSON Schema content or the Amazon Resource Name
//! (ARN) of an Lambda function"
//! (`smithy_query(query="doc",shape="Validator",member="Content")`), one per
//! `ValidatorType`:
//!
//! * `LAMBDA` — the `Content` names a function, invoked synchronously with the candidate
//!   configuration; a function that answers an error has found the content non-compliant,
//!   which is the verdict that refuses the call. The invocation goes through the one Lambda
//!   seam this crate has ([`super::extension_actions::invoke_function`]), over the
//!   in-process callback, so the function invoked is the one the emulator's own Lambda
//!   service holds and no socket leaves the machine. It is bounded by
//!   [`LAMBDA_CHECK_BUDGET`], which is what a gated call waits on a function that never
//!   answers, since the client the invocation is made with disables its own timeouts.
//! * `JSON_SCHEMA` — the `Content` is a JSON Schema document, and this emulator produces NO
//!   verdict for one: evaluating a caller-supplied schema means running an in-process
//!   evaluator over caller-supplied input, and the evaluator crates available cannot be
//!   bounded by a step budget or interrupted once started, which this project requires of
//!   anything it evaluates in process. So nothing is claimed about such a validator here.
//!
//! A validator that yields NO verdict — a `JSON_SCHEMA` validator, or a `LAMBDA` validator
//! whose invocation does not happen at all as opposed to happening and answering an error —
//! does not refuse the call it gates, for the reason
//! [`super::extension_actions::Failure::refuses_the_call`] gives: nothing is known about
//! what it would have reported, and a refusal reported for it would be a verdict no
//! validator gave.

use std::time::Duration;

use ::serde_json::{Map, Value};

use crate::ops::types::Validator;

use super::extension_actions::{encoded, invoke_function, Failure, LAMBDA_ARN_NAMESPACE};

/// The `ValidatorType` values the model lists (`VALIDATOR_TYPE_VALUES` in
/// `crate::ops::types`), each spelled once here because the evaluation keys off it.
pub const JSON_SCHEMA: &str = "JSON_SCHEMA";
/// See [`JSON_SCHEMA`].
pub const LAMBDA: &str = "LAMBDA";

/// How long one `LAMBDA` check may wait for the function's answer.
///
/// The client the invocation is made with disables its own timeouts — there is no network
/// behind it to time out — so this is what bounds the gated call's wait on a function that
/// never answers. EMULATOR-chosen: no published AppConfig limit covers it.
const LAMBDA_CHECK_BUDGET: Duration = Duration::from_secs(5);

/// The longest validator report a refusal carries, in characters.
///
/// What a validator reports grows with the configuration it was run against rather than
/// with what a client needs to read; the `Message` member carries the head of it.
const MAX_DETAIL_CHARS: usize = 1024;

/// The configuration version a profile's validators are run against: what each validator
/// is handed, and what the refusal of a non-compliant one names.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// The application the configuration profile belongs to.
    pub application_id: &'a str,
    /// The configuration profile whose validators are being run.
    pub configuration_profile_id: &'a str,
    /// The version under validation, as the caller named it — a version number or a
    /// version label, the two spellings `ConfigurationVersion` documents.
    pub configuration_version: &'a str,
    /// The profile's `LocationUri`: "information about the source location of the
    /// configuration", which the event a `LAMBDA` validator is handed carries.
    pub location_uri: &'a str,
    /// The configuration bytes a consumer of this profile would be served for that
    /// version, read through the same projection the data plane serves
    /// ([`super::ConfigurationProfile::served_content`]) — a validator checks what would
    /// go into service, not a different document.
    pub content: &'a [u8],
}

/// The first non-compliant verdict `validators` give for `candidate`, as the `Message` of
/// the `BadRequestException` the gated call is refused with; `None` when no validator
/// reported the content non-compliant.
///
/// The validators are run in the order the profile carries them and the first verdict is
/// answered, so a call is refused for the first check it failed rather than for the last.
pub async fn first_rejection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    validators: &[Validator],
    candidate: &Candidate<'_>,
    request_region: &str,
) -> Option<String> {
    for validator in validators {
        let rejection = match validator.r#type.as_str() {
            LAMBDA => lambda_rejection(ctx, &validator.content, candidate, request_region).await,
            // A `JSON_SCHEMA` validator yields no verdict here; see this module's own
            // documentation. A `Type` outside the model's `ValidatorType` enum is refused
            // by the boundary validator before any handler stores it, so there is no check
            // to run for one either.
            _ => None,
        };
        if rejection.is_some() {
            return rejection;
        }
    }
    None
}

/// The verdict a `LAMBDA` validator gives: whether the function the validator's `Content`
/// names, invoked with the candidate configuration, reports an error.
async fn lambda_rejection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    function: &str,
    candidate: &Candidate<'_>,
    request_region: &str,
) -> Option<String> {
    // The function is reached in the Region its own ARN names, falling back to the Region
    // this request was signed for — the same resolution an extension action's target gets.
    let region = ::service_common::arn::region_of(function, LAMBDA_ARN_NAMESPACE)
        .unwrap_or(request_region);
    let event = validator_event(candidate).to_string();
    match ::tokio::time::timeout(
        LAMBDA_CHECK_BUDGET,
        invoke_function(ctx, function, region, event),
    )
    .await
    {
        // The function ran and reported no error: it found the content compliant. What it
        // RETURNED is not read — a validator reports a verdict, it does not rewrite the
        // configuration.
        Ok(Ok(_)) => None,
        Ok(Err(failure)) if failure.refuses_the_call() => {
            Some(rejection_message(candidate, LAMBDA, &lambda_detail(function, &failure)))
        }
        // The invocation did not happen at all; see this module's own documentation.
        Ok(Err(_)) => None,
        // The invocation happened and did not answer in time, which is an invocation that
        // failed rather than one that was never made: the configuration was not shown
        // valid, so the call it gates is refused.
        Err(_) => Some(rejection_message(
            candidate,
            LAMBDA,
            &format!("the function {function} did not answer the invocation in time"),
        )),
    }
}

/// The event a `LAMBDA` validator's function is handed: the resources the configuration
/// belongs to, the version under validation, its content base64-encoded and the location
/// the configuration is held at, keyed as the AppConfig User Guide's validator page
/// documents them — the page the model's `Validator` shape links to.
fn validator_event(candidate: &Candidate<'_>) -> Value {
    Value::Object(Map::from_iter([
        (
            "applicationId".to_string(),
            Value::from(candidate.application_id),
        ),
        (
            "configurationProfileId".to_string(),
            Value::from(candidate.configuration_profile_id),
        ),
        (
            "configurationVersion".to_string(),
            Value::from(candidate.configuration_version),
        ),
        ("content".to_string(), Value::from(encoded(candidate.content))),
        ("uri".to_string(), Value::from(candidate.location_uri)),
    ]))
}

/// What a failed validator invocation contributes to the refusal: the function that was
/// invoked, the error it answered and, when its answer carried one, the Lambda request id
/// of the invocation — the same three facts this service reports for a failed extension
/// action (`super::extension_actions::ActionTarget::refusal_message`).
fn lambda_detail(function: &str, failure: &Failure) -> String {
    let mut detail = format!("error invoking {function}: {}", summarised(&failure.detail));
    if let Some(request_id) = &failure.request_id {
        detail.push_str(&super::extension_actions::lambda_request_id_clause(request_id));
    }
    detail
}

/// How AppConfig refuses a call whose candidate configuration a validator reported
/// non-compliant: derived from the request — the version and profile the validator ran
/// against, the type of validator that gave the verdict, and the detail it gave.
fn rejection_message(candidate: &Candidate<'_>, validator_type: &str, detail: &str) -> String {
    format!(
        "Configuration version '{}' of configuration profile '{}' did not pass the \
         {validator_type} validator: {detail}",
        candidate.configuration_version, candidate.configuration_profile_id
    )
}

/// `detail` as a refusal carries it: on one line, because the `Message` member carries one
/// text, and no longer than [`MAX_DETAIL_CHARS`].
fn summarised(detail: &str) -> String {
    let flattened = detail.split_whitespace().collect::<Vec<&str>>().join(" ");
    match flattened.char_indices().nth(MAX_DETAIL_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", &flattened[..end]),
        None => flattened,
    }
}
