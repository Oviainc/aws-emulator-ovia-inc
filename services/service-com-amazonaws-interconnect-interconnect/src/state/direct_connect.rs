// SPDX-License-Identifier: CC0-1.0

//! The account's Direct Connect gateways: facts this service RESOLVES rather than holds.
//!
//! `AttachPointType`'s only modelled value is `DirectConnectGateway`, and
//! `AttachPoint$directConnectGateway` identifies an attach point "by
//! DirectConnectGatewayID" — so the candidate attach points of an account are the Direct
//! Connect gateways it holds. Those are resources of Direct Connect, not of this service,
//! so they are read the way any emulated service reaches another: the AWS SDK driven over
//! the in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so
//! this crate names no other service's crate.
//!
//! A candidate is only an attach point `ListAttachPoints` reports once it is one "that
//! [is] valid for the specified Environment", so every reader here is scoped by the
//! Environment the request named ([`crate::state::environment_offers_attach_points`]) and
//! not by the account alone.
//!
//! Nothing is assumed from silence and nothing is invented: an account that holds no
//! gateway — and an emulator with no Direct Connect mounted, which knows of none — has no
//! attach point, which is an EMPTY listing rather than a fabricated descriptor.

use crate::ops::types::{AttachPoint, AttachPointDescriptor, Environment};

/// The one `DirectConnectGatewayState` a Connection can be attached to.
///
/// Direct Connect documents the value as "available: The Direct Connect gateway is ready
/// for use"; a gateway that is still being created (`pending`) or being torn down
/// (`deleting`) is not ready for use, so it is not yet — or no longer — a valid attach
/// point.
const GATEWAY_READY_STATE: &str = "available";

/// The page size each `DescribeDirectConnectGateways` call asks for.
///
/// Direct Connect caps a page at 100 ("if MaxResults is given a value larger than 100,
/// only 100 results are returned"), so this asks for the largest page it serves and the
/// walk below makes as few calls as it can.
const GATEWAY_PAGE_SIZE: i32 = 100;

/// How many pages the walk reads before giving up.
///
/// The cursor comes from Direct Connect rather than from this service, so the loop is
/// bounded here: a peer that kept handing out a token can never spin this handler.
const MAX_GATEWAY_PAGES: usize = 64;

/// The Direct Connect client this service resolves the account's gateways through.
///
/// `region` is the request's own signing region, so the hop stays in the Region the
/// caller addressed. The client is given the callback's own HTTP client, so the endpoint
/// URL below is only the address that transport routes on IN PROCESS: no socket is
/// opened, no name is resolved and nothing leaves this process. The credentials are the
/// callback's, which sign this in-process hop and never appear in a response.
fn direct_connect(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_directconnect::Client {
    let config = ::aws_sdk_directconnect::Config::builder()
        .behavior_version(::aws_sdk_directconnect::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_directconnect::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://directconnect.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_directconnect::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_directconnect::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_directconnect::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_directconnect::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_directconnect::Client::from_conf(config)
}

/// One gateway Direct Connect reported, as the descriptor this service publishes for it —
/// or `None` when it is not a valid attach point.
fn descriptor_of(
    gateway: &::aws_sdk_directconnect::types::DirectConnectGateway,
) -> Option<AttachPointDescriptor> {
    let identifier = gateway.direct_connect_gateway_id()?;
    if gateway
        .direct_connect_gateway_state()
        .is_none_or(|state| state.as_str() != GATEWAY_READY_STATE)
    {
        return None;
    }
    Some(AttachPointDescriptor {
        r#type: crate::state::ATTACH_POINT_TYPE_DIRECT_CONNECT_GATEWAY.to_string(),
        // `AttachPointDescriptor$name` is required and is "the descriptive name of the
        // identifier attach point": a gateway carries its own name, and one that reports
        // none is named by its identifier rather than reported unnamed.
        name: gateway
            .direct_connect_gateway_name()
            .unwrap_or(identifier)
            .to_string(),
        identifier: identifier.to_string(),
    })
}

/// What resolving ONE attach-point identifier against the account's gateways found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachPointResolution {
    /// The identifier names a gateway of the account that is a valid attach point.
    Valid,
    /// The account's gateways were read, and none of them is that identifier.
    NotAnAttachPoint,
    /// The account's gateways could not be read to the end, so whether the identifier
    /// names one is UNKNOWN. That is this emulator failing to resolve the account's own
    /// state, not a caller naming something that does not exist, so the two are never
    /// answered the same way.
    Unresolvable,
}

/// Resolve `identifier` against the attach points valid for `environment`.
///
/// Answered from the same reader [`attach_points`] reports, so what a Connection may be
/// attached to is exactly what `ListAttachPoints` offers for that Environment — the two
/// cannot drift apart. A gateway the account does not hold, one that is not ready for
/// use, and any identifier at all on an Environment that carries no Connection
/// ([`crate::state::environment_offers_attach_points`]) are all
/// [`AttachPointResolution::NotAnAttachPoint`]; a walk that could not be completed is
/// [`AttachPointResolution::Unresolvable`], unless the pages it did read already hold
/// the identifier, which settles the question on its own.
pub async fn resolve_attach_point(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    environment: Option<&Environment>,
    identifier: &str,
) -> AttachPointResolution {
    if !crate::state::environment_offers_attach_points(environment) {
        return AttachPointResolution::NotAnAttachPoint;
    }
    let (descriptors, complete) = walk_attach_points(ctx, region).await;
    if descriptors
        .iter()
        .any(|descriptor| descriptor.identifier == identifier)
    {
        AttachPointResolution::Valid
    } else if complete {
        AttachPointResolution::NotAnAttachPoint
    } else {
        AttachPointResolution::Unresolvable
    }
}

/// How a request whose attach point could not be judged AT ALL is worded.
///
/// This is the emulator failing to resolve the account's own gateways rather than the
/// caller naming something that does not exist, so the sentence carries no identifier:
/// nothing is known about the one the request sent, not even that it is absent.
const ATTACH_POINTS_UNRESOLVABLE_MESSAGE: &str =
    "the attach points of this account could not be resolved, so the requested attach point \
     could not be validated";

/// The refusals an attach-point judgement can call for, in the error channel of the
/// operation that asked for it.
///
/// Every operation of this service carries its own modelled error enum, so the rules
/// deciding WHICH refusal a verdict calls for and what it says live once — in
/// [`judge_attach_point`] for the gateway resource and in
/// [`crate::state::judge_attach_point_in_store`] for the member's own constraints and the
/// ARN's resource — and an operation that resolves an attach point supplies only this:
/// which variant of its own channel each refusal travels in. A handler therefore cannot
/// classify or word one of these refusals differently from its neighbour.
pub trait AttachPointRefusal: Sized {
    /// The value the union carries is not one the member it arrived in may hold, so the
    /// request itself is at fault and nothing is resolved for it: the modelled
    /// `InterconnectValidationException` (HTTP 400), the same class a malformed ARN gets
    /// from every other operation addressed by one.
    fn invalid_attach_point(message: String) -> Self;

    /// The account's attach points were read and the identifier is not one of them: the
    /// modelled `ResourceNotFoundException` (HTTP 404).
    fn not_an_attach_point(message: String) -> Self;

    /// The account's attach points could not be read, so the identifier could not be
    /// judged either way: the modelled `InterconnectServerException` (HTTP 500).
    fn attach_points_unresolvable(message: String) -> Self;
}

/// Judge the `attach_point` a request selected for a connection on `environment_id`,
/// answering `Ok(())` when the connection may be made onto it.
///
/// The ONE rule both writers of a connection's attach point (the create that allocates
/// the first one and the acceptance that associates one) are held to, so what a
/// Connection may be attached to does not depend on how it came to exist. The value the
/// union carries is held to its member's OWN modelled constraints FIRST
/// ([`crate::state::attach_point_violation`]), so a value that is not an identifier of
/// its kind is invalid input and nothing is resolved for it — the account's gateways are
/// not walked for it at all. Then the identifier is resolved:
///
/// * [`AttachPointResolution::Valid`] — the identifier names an attach point of the
///   account, and the request continues;
/// * [`AttachPointResolution::NotAnAttachPoint`] — it names none of the attach points
///   valid for that Environment, which is the not-found refusal worded by
///   [`crate::state::attach_point_not_valid_message`] from the identifier the request
///   carried and the Environment it was offered for;
/// * [`AttachPointResolution::Unresolvable`] — the account's attach points could not be
///   read, which is the server refusal carrying the sentence above, a DIFFERENT answer
///   because nothing is known about the identifier either way.
///
/// Only `AttachPoint$directConnectGateway` is RESOLVED here: that member "identifies an
/// DirectConnect Gateway attach point by DirectConnectGatewayID", a resource of the
/// service that OWNS those gateways, while `AttachPoint$arn` names a Connection of the
/// account, which this service's own store resolves under its own lock
/// ([`crate::state::judge_attach_point_in_store`]).
/// `environment` is the catalog record the request resolved, and `environment_id` the
/// identifier it resolved it by — taken separately, the way
/// [`crate::state::bandwidth_availability_violation`] takes them, so the refusal names
/// the Environment the request was for whether or not the catalog still holds it.
pub async fn judge_attach_point<E: AttachPointRefusal>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    attach_point: &AttachPoint,
    environment: Option<&Environment>,
    environment_id: &str,
) -> Result<(), E> {
    if let Some(message) = crate::state::attach_point_violation(attach_point) {
        return Err(E::invalid_attach_point(message));
    }
    let AttachPoint::directConnectGateway(gateway) = attach_point else {
        return Ok(());
    };
    match resolve_attach_point(ctx, region, environment, gateway).await {
        AttachPointResolution::Valid => Ok(()),
        AttachPointResolution::NotAnAttachPoint => Err(E::not_an_attach_point(
            crate::state::attach_point_not_valid_message(gateway, environment_id),
        )),
        AttachPointResolution::Unresolvable => Err(E::attach_points_unresolvable(
            ATTACH_POINTS_UNRESOLVABLE_MESSAGE.to_string(),
        )),
    }
}

/// The attach points valid for `environment`: every Direct Connect gateway of the
/// account that is ready for use, as the descriptors `ListAttachPoints` reports — and
/// NONE at all for an Environment that carries no Connection
/// ([`crate::state::environment_offers_attach_points`]), because an attach point is
/// where a Connection on that Environment would logically connect, so an Environment
/// nothing can be placed upon has nothing valid to offer. The account's gateways are
/// not read at all in that case: no answer about them could change it.
///
/// Empty, too, when the account holds no gateway, and when Direct Connect cannot be
/// asked at all — in both cases nothing is known to attach to, and a descriptor is
/// never synthesised to fill the gap. The last case is not silent: it is logged at
/// `warn`, so an empty listing is not read as a positive answer when the peer simply
/// did not answer.
pub async fn attach_points(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    environment: Option<&Environment>,
) -> Vec<AttachPointDescriptor> {
    if !crate::state::environment_offers_attach_points(environment) {
        return Vec::new();
    }
    walk_attach_points(ctx, region).await.0
}

/// The walk both readers above share: the descriptors it gathered, and whether it
/// reached the END of the account's gateways — `false` when Direct Connect could not be
/// asked, and when the page bound was reached with a cursor still outstanding, so the
/// descriptors are not a complete account of what the account holds.
async fn walk_attach_points(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> (Vec<AttachPointDescriptor>, bool) {
    let client = direct_connect(ctx, region);
    let mut descriptors: Vec<AttachPointDescriptor> = Vec::new();
    let mut token: Option<String> = None;
    for page_number in 0..MAX_GATEWAY_PAGES {
        let answer = client
            .describe_direct_connect_gateways()
            .max_results(GATEWAY_PAGE_SIZE)
            .set_next_token(token)
            .send()
            .await;
        let page = match answer {
            Ok(page) => page,
            Err(error) => {
                // Visible at default verbosity: an empty listing because the peer could
                // not be asked reads exactly like one because the account holds no
                // gateway, so the degraded path says so rather than passing quietly.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the account's Direct Connect gateways could not be resolved"
                );
                return (descriptors, false);
            }
        };
        descriptors.extend(page.direct_connect_gateways().iter().filter_map(descriptor_of));
        token = page.next_token().map(str::to_string);
        if token.is_none() {
            return (descriptors, true);
        }
        // The page bound is reached with a cursor still outstanding, so this listing is
        // SHORT of what the peer holds. Said out loud: a short answer a caller cannot
        // tell from a complete one is worth a line in the log.
        if page_number + 1 == MAX_GATEWAY_PAGES {
            ::tracing::warn!(
                pages = MAX_GATEWAY_PAGES,
                "stopped reading Direct Connect gateways at the page bound, so the attach-point \
                 listing is truncated"
            );
        }
    }
    (descriptors, false)
}
