// SPDX-License-Identifier: CC0-1.0

//! The REST endpoint a `ProductRestEndpointPage` documents, as the service that OWNS REST
//! APIs reports it.
//!
//! A REST API is not a resource of this service: an `IdentifierParts` names one by its
//! `RestApiId` and one of its stages, and both live in the API Gateway (v1) service, which
//! is where a client created them (`apigateway:CreateRestApi`, `apigateway:CreateStage`).
//! A page's content is GENERATED from that endpoint — "By default, this is the
//! documentation of your REST API from API Gateway"
//! (`DisplayContentOverrides$Body`) — so whether the generation can be carried out at all
//! is a question only the owner of REST APIs can answer, and it is resolved there rather
//! than guessed here.
//!
//! The owner is reached through its own `state` accessor on the SHARED engine, the way
//! [`crate::service::authorization`] reads the IAM and STS stores, and under the same lock
//! order: the other service's lock is taken and released BEFORE this service's own store
//! lock, so every caller resolves first and reaches its own store after. The v1 service
//! reads nothing of this one's state.
//!
//! Nothing is cached: a page's generation is resolved at the WRITE that starts it (see
//! [`ProductRestEndpointPage::generation_failure`]), so a REST API created after that
//! write does not retroactively change what an already-generated page reports, and a read
//! of the page asks nothing of anybody.

use super::*;

/// The sentence this service states for a `RestEndpointIdentifier` that names no REST API,
/// or no stage of the API it names: the identifier identifies no endpoint to document.
///
/// Held here once, so the generation failure a page reports and any other refusal of the
/// same fact cannot word it differently.
pub const UNRESOLVED_REST_ENDPOINT_MESSAGE: &str =
    "Invalid restEndpointIdentifier stage or restApiId";

/// The `StatusException$Exception` a page's failed generation names: the class of error the
/// service reports that fact under, which is the modelled `BadRequestException` ("The
/// request is not valid, for example, the input is incomplete or incorrect") — the page's
/// own identifier is what the generation could not resolve.
pub const UNRESOLVED_REST_ENDPOINT_EXCEPTION: &str = "BadRequestException";

/// Why the generation of a page documenting the REST endpoint `parts` identify cannot
/// succeed, or `None` when that endpoint resolves.
///
/// `Some` carries the modelled `StatusException` the page then reports beside its `FAILED`
/// status — "the status exception information" — built from the fact that the account holds
/// no such REST API, or no such stage of it.
///
/// A HOP THAT CANNOT BE MADE IS EVIDENCE OF NOTHING: an engine carrying no API Gateway (v1)
/// store cannot be asked which REST APIs the account has, so nothing is reported as failed
/// there. That is the same reading [`super::ec2_subnets`] takes of silence from the owner
/// of subnets.
///
/// Call it BEFORE this service's own store lock is taken — see this module's documentation.
pub fn rest_endpoint_generation_failure<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    parts: &wire::IdentifierParts,
) -> Option<wire::StatusException> {
    let owner = ::service_com_amazonaws_apigateway_backplanecontrolservice::state::service_state(
        engine,
    )?;
    let resolved = {
        let rest_apis = owner.lock();
        rest_apis
            .rest_apis
            .get(&parts.rest_api_id)
            // A page documents ONE endpoint of ONE stage, and it is the stage that is
            // deployed and invocable, so an API the account has under a name it has no such
            // stage for identifies nothing either.
            .is_some_and(|rest_api| rest_api.stages.contains_key(&parts.stage))
    };
    if resolved {
        return None;
    }
    Some(wire::StatusException {
        exception: Some(UNRESOLVED_REST_ENDPOINT_EXCEPTION.to_string()),
        message: Some(UNRESOLVED_REST_ENDPOINT_MESSAGE.to_string()),
    })
}
