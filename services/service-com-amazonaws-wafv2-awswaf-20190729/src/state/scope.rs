// SPDX-License-Identifier: CC0-1.0

//! The `Scope` this service namespaces every resource by: the two values the model
//! declares, and the refusal for a value WAF will not accept.
//!
//! Whether a request may NAME a scope is a question about the request rather than
//! about the store, so it lives with this service's other request-level policy
//! (`crate::service::requests::validate_scope`); what a scope IS lives here, where
//! the resources it namespaces are.

use super::*;

/// The `Scope` values the model declares
/// (`smithy_query(query="shape",shape="Scope")` → `CLOUDFRONT`, `REGIONAL`).
pub const SCOPE_CLOUDFRONT: &str = "CLOUDFRONT";
/// See [`SCOPE_CLOUDFRONT`].
pub const SCOPE_REGIONAL: &str = "REGIONAL";

/// The region an ARN carries when the request names none: the emulator's default
/// endpoint region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `true` when `scope` is one of the two values the `Scope` enum declares.
pub fn is_known_scope(scope: &str) -> bool {
    matches!(scope, SCOPE_CLOUDFRONT | SCOPE_REGIONAL)
}

/// The `ErrorReason` WAF carries for a `Scope` it will not accept, as the recorded
/// WAF responses show it.
///
/// One sentence for the whole class — a value the enum does not declare, and a
/// value it does declare that this endpoint does not serve — because the
/// `Parameter` member carries the rejected scope, so the reason does not have to
/// distinguish them.
pub const SCOPE_NOT_VALID_REASON: &str = "The scope is not valid.";

/// [`invalid_parameter_exception`] for a scope WAF will not accept: `Field`
/// [`FIELD_SCOPE_VALUE`], `Parameter` the rejected scope as the request wrote it
/// and `Reason` [`SCOPE_NOT_VALID_REASON`].
pub fn invalid_scope_exception(scope: &str) -> WAFInvalidParameterException {
    invalid_parameter_exception(FIELD_SCOPE_VALUE, scope, SCOPE_NOT_VALID_REASON)
}
