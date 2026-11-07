// SPDX-License-Identifier: CC0-1.0

//! Whether the caller may reach the directory a request names — the gate every operation
//! of this data plane passes before its handler resolves anything inside that directory.
//!
//! Directory Service Data serves the objects INSIDE a directory and creates no directory:
//! `CreateDirectory` / `CreateMicrosoftAD` / `DeleteDirectory` belong to the Directory
//! Service control plane, and so does the switch that makes this API usable at all
//! (`EnableDirectoryDataAccess` / `DisableDirectoryDataAccess`). So a `DirectoryId` the
//! request carries is reachable here only on two facts only that plane can state, both read
//! through the record it writes ([`crate::state::ad_directory_inventory`]):
//!
//! * the account HOLDS the directory — otherwise the caller cannot reach it at all, which
//!   every operation here answers the same way: `AccessDeniedException`, HTTP 403, `Reason`
//!   `IAM_AUTH`, naming the calling principal and NOTHING about the directory;
//! * the directory's DS Data feature is on — otherwise this whole API is out of service for
//!   it, which every operation here answers with the same `AccessDeniedException`, HTTP 403,
//!   carrying `Reason` [`DATA_DISABLED`] and the sentence AWS words for a directory whose
//!   feature has not been enabled. The feature is on from the instant the control plane's
//!   enable settles until the instant its disable does, so the data plane answers for
//!   exactly the directories whose `DataAccessStatus` that plane reports as `Enabled`.
//!
//! Both refusals are DIRECTORY-scoped rather than operation-scoped: an object read, a write
//! and an inventory listing carry the identical error code and the identical message,
//! because what is refused is the directory and not the call.
//!
//! That is the modelled shape's own documentation: "You don't have permission to perform
//! the request or access the directory. It can also occur when the `DirectoryId` doesn't
//! exist or the user, member, or group might be outside of your organizational unit (OU)"
//! (`smithy_query(query="shape", shape="AccessDeniedException")`, `@httpError` 403, member
//! `Reason` targeting `AccessDeniedReason`, whose values are `IAM_AUTH`, `DIRECTORY_AUTH`
//! and `DATA_DISABLED`). A directory the caller cannot reach is therefore never reported as
//! `ResourceNotFoundException` — the eleven operations that declare that error state it for
//! an object MISSING INSIDE a directory the caller does reach — and never served as an empty
//! answer: either would tell a caller whether a directory it may not see exists.
//!
//! WHERE the gate sits decides what a refused call is told, so the order is part of the
//! rule:
//!
//! 1. the modelled constraints (`crate::service::validation`), because a value outside a
//!    declared bound is refused whatever directory it was sent to;
//! 2. the semantic attribute layer ([`super::attribute_values`]);
//! 3. THIS gate — ahead of every handler, so it precedes pagination-token parsing,
//!    `ClientToken` idempotency and every object lookup: a fabricated `NextToken` or a
//!    well-formed `ClientToken` on a directory the caller cannot reach is still this
//!    refusal, and nothing of the request is persisted — a write refused here creates
//!    nothing, so the object inventory a later enable exposes does not carry it.
//!
//! The principal a refusal names is the one this emulator's credential speaks for
//! (`service_common::caller_principal`), and the sentence is the tree's one statement of
//! that denial (`service_common::iam_authorization::not_authorized_to_access_resource`), so
//! no two services word it differently.

use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::ServerHttpProtocol;

/// The `AccessDeniedReason` a refusal of an unreachable directory carries: one of the
/// modelled values (`AccessDeniedReason`: `IAM_AUTH`, `DIRECTORY_AUTH`, `DATA_DISABLED` —
/// `crate::ops::types::ACCESS_DENIED_REASON_VALUES`), the one that names the authorization
/// layer rather than the directory's own state.
pub const IAM_AUTH: &str = "IAM_AUTH";

/// The `AccessDeniedReason` a refusal of a directory whose DS Data feature is off carries:
/// the modelled value that names the directory's own data-access state rather than the
/// authorization layer.
pub const DATA_DISABLED: &str = "DATA_DISABLED";

/// The `Message` that refusal states: what AWS says for a directory the DS Data feature has
/// not been enabled on, naming the control-plane call that enables it. Fixed text in full —
/// nothing of the request appears in it, so a read, a write and a listing of any directory
/// are refused with one identical sentence.
pub const DATA_DISABLED_MESSAGE: &str =
    "DS Data feature is not enabled on this directory. This feature must first be enabled \
     by calling the EnableDirectoryDataAccess API.";

/// The error code this service names for a caller that may not reach the directory: the
/// shape `com.amazonaws.directoryservicedata#AccessDeniedException`, which every one of the
/// 17 operations declares.
const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// The HTTP status that shape's `@httpError` declares.
const ACCESS_DENIED_EXCEPTION_STATUS: u16 = 403;

/// The query parameter every operation binds its `DirectoryId` to (`@httpQuery` +
/// `@required`, on all 17 — `smithy_query(query="operation")`), which is where the
/// directory a request names is read from.
const DIRECTORY_ID_PARAMETER: &str = "DirectoryId";

/// The refusal `req` gets when the account does not hold the directory it names or the
/// control plane has that directory's DS Data feature off, or `None` when the request may go
/// on to its handler.
///
/// `None` for a request that names no directory at all: an absent `@required` member is the
/// constraint layer's refusal, which runs before this gate, and an engine carrying no
/// inventory is an engine no control plane has written to, which cannot be read as the
/// account holding the directory.
pub fn refusing_request<Eng: Send + Sync + 'static>(
    protocol: &dyn ServerHttpProtocol,
    engine: &Eng,
    req: &RtRequest,
) -> Option<RtResponse> {
    let named = directory_named(req.uri())?;
    let now = SystemTime::now();
    let (held, serving) = crate::state::ad_directory_inventory(engine)
        // The inventory is read and RELEASED here, before any handler takes this
        // service's own store: see the lock order on the accessor.
        .map(|inventory| {
            let recorded = inventory.lock();
            (
                recorded.holds(named),
                recorded.data_access_is_in_service(named, now),
            )
        })
        .unwrap_or((false, false));
    if !held {
        return Some(access_denied(
            protocol,
            // The principal and nothing else: a denial must not tell a caller whether a
            // directory it may not reach exists.
            ::service_common::iam_authorization::not_authorized_to_access_resource(
                &::service_common::caller_principal::ambient_user_arn(),
            ),
            IAM_AUTH,
        ));
    }
    if !serving {
        return Some(access_denied(
            protocol,
            DATA_DISABLED_MESSAGE.to_string(),
            DATA_DISABLED,
        ));
    }
    None
}

/// The directory `uri` names, percent-decoding nothing: the value is compared against the
/// inventory exactly as the control plane recorded it, and a `DirectoryId` is `^d-[0-9a-f]
/// {10}$`, which carries no character a query string escapes.
///
/// The query string is read through the tree's one reader of a raw one
/// (`service_common::query_parameter`). `None` when the query binds no `DirectoryId` and
/// when it binds it with no value — neither is a directory this account could hold, and
/// both are the constraint layer's to report.
fn directory_named(uri: &str) -> Option<&str> {
    let (_, query) = uri.split_once('?')?;
    ::service_common::query_parameter(query, DIRECTORY_ID_PARAMETER)
        .filter(|value| !value.is_empty())
}

/// This service's answer to a caller whose request the gate refused: 403
/// `AccessDeniedException` with `message` in the `Message` member the model declares and
/// `reason` in its `Reason` member, serialized through that shape's own schema.
///
/// An authorization decision never degrades into success, so this answers a response in
/// every case: where the error cannot be serialized through its schema, the same status,
/// error code and members are written directly, rather than letting the request fall
/// through to a handler the gate decided not to reach.
fn access_denied(protocol: &dyn ServerHttpProtocol, message: String, reason: &str) -> RtResponse {
    let refused = crate::ops::types::shared_types::AccessDeniedException {
        message: Some(message),
        reason: Some(reason.to_string()),
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::shared_types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION_STATUS,
        )
        .unwrap_or_else(|_| {
            // The same status, error code and members, written without the schema through
            // the one fallback every refusal path of this service shares.
            super::modelled_error_response(
                ACCESS_DENIED_EXCEPTION,
                ACCESS_DENIED_EXCEPTION_STATUS,
                refused.message.as_deref(),
                refused.reason.as_deref(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::directory_named;

    /// RULE: the directory a request names is the `DirectoryId` its query string binds,
    /// whatever else the URI carries; a request binding none names none.
    #[test]
    fn the_directory_named_is_the_bound_query_parameter() {
        // (case, uri, the directory named)
        let rows: [(&str, &str, Option<&str>); 7] = [
            (
                "the one parameter",
                "/Users/ListUsers?DirectoryId=d-1234567890",
                Some("d-1234567890"),
            ),
            (
                "among other parameters",
                "/Users/ListUsers?MaxResults=1&DirectoryId=d-00abcdef99&NextToken=x",
                Some("d-00abcdef99"),
            ),
            (
                "an absolute URI",
                "https://ds-data.us-east-1.amazonaws.com/Users/ListUsers?DirectoryId=d-aaaaaaaaaa",
                Some("d-aaaaaaaaaa"),
            ),
            ("no query string at all", "/Users/ListUsers", None),
            (
                "another parameter only",
                "/Users/ListUsers?MaxResults=1",
                None,
            ),
            ("bound with no value", "/Users/ListUsers?DirectoryId=", None),
            ("bound as a bare key", "/Users/ListUsers?DirectoryId", None),
        ];
        for (case, uri, expected) in rows {
            let named = directory_named(uri);
            assert_eq!(
                named, expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }
}
