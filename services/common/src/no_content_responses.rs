// SPDX-License-Identifier: CC0-1.0

//! The framing of a response that carries NO CONTENT, for every service whose success
//! answers HTTP 204.
//!
//! # Why a service needs this
//!
//! An operation whose modelled output structure declares no member leaves its caller
//! nothing to parse, and AWS answers such a success `204 No Content`. The status is the
//! service's own behaviour — the protocol leaves the code defaulted — so a service states it
//! on the operation (`Operation::SUCCESS_STATUS`); what it cannot state there is the BODY.
//! The protocol's serializer renders a members-less output structure as the empty JSON
//! object `{}` (it is a `structure`, not `smithy.api#Unit`, which is the one shape the
//! renderers treat as bodyless), and RFC 9110 section 15.3.5 gives a 204 no room for a body
//! at all: a client, a proxy or a contract check reading one finds content on a response
//! that declares it has none.
//!
//! [`without_content`] is that correction, written once: a response at the no-content status
//! carries no body and states the length it has. A response that carries content — every
//! reader's, every create's, every modelled failure's — is returned untouched, status,
//! headers and every body byte exactly as its renderer wrote them.
//!
//! # Where a service takes it
//!
//! At `Service::override_protocol`, the one seam every response a modelled operation
//! produces passes through: the service serves a protocol that is the model's own in every
//! respect except that each serialized response is handed through this function. The
//! delegation itself stays with the service, because the protocol it delegates to is owned by
//! its generated definition and reached by reference.
//!
//! An answer served over HTTP/1.1 reaches the client without the framing header — the HTTP
//! writer the vendored transport uses drops `content-length` from a 204 — which is below
//! anything a service crate decides.
//!
//! # Services still carrying their own copy
//!
//! This correction predates this module and is inlined in the service crates that needed it
//! before there was anywhere shared to put it. Search for the SPAN rather than for a list
//! that would drift: the body is a status comparison against 204 followed by an empty
//! `SdkBody` and a `content-length` of `0`, and it appears both as a function of its own
//! (spelled `frame_empty_body` in some crates and `frame_empty_answer` in others, each beside
//! a `NO_CONTENT_STATUS` of its own) and inline inside a response wrapper, where no symbol
//! name finds it. Those copies are to be MIGRATED onto this module and are left in place only
//! because this change does not own those services' issues, each being covered by its own
//! suite. Nothing new should inline the rule again — call this module.

use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_types::body::SdkBody;

/// The status whose response carries NO content, whatever a renderer wrote for it.
pub const NO_CONTENT_STATUS: u16 = 204;

/// The length such a response states for the body it does not carry.
const NO_CONTENT_LENGTH: &str = "0";

/// `resp` framed as an answer that carries no content: no body at all, and the length it
/// does have.
///
/// A response at any other status is returned untouched. Idempotent — framing an already
/// framed answer changes nothing.
#[must_use]
pub fn without_content(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() != NO_CONTENT_STATUS {
        return resp;
    }
    // The empty JSON object the serializer renders for a members-less output goes no
    // further: this status has no content to carry it.
    *resp.body_mut() = SdkBody::empty();
    // A decimal length is always a valid header value, so this cannot fail today; if it ever
    // did, the header is REMOVED rather than left describing the body that was dropped.
    if resp
        .headers_mut()
        .try_insert("content-length", NO_CONTENT_LENGTH)
        .is_err()
    {
        resp.headers_mut().remove("content-length");
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;

    /// A response at `status` carrying `body`, as a protocol's serializer hands one back.
    fn rendered(status: u16, body: &str) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a status the test states"),
            SdkBody::from(body),
        );
        resp.headers_mut().insert("content-type", "application/json");
        resp
    }

    #[test]
    fn a_no_content_answer_carries_no_body_and_states_its_length() {
        // The rows differ along what a renderer may have written for such an answer: the
        // empty JSON object a members-less output structure renders as, a body with members
        // in it, and no body at all.
        for case in ["{}", "{\"member\":1}", ""] {
            let framed = without_content(rendered(NO_CONTENT_STATUS, case));
            assert_eq!(
                framed.status().as_u16(),
                NO_CONTENT_STATUS,
                "case {case:?}: the status is the protocol's, got {}",
                framed.status().as_u16()
            );
            assert_eq!(
                framed.body().bytes(),
                Some(&b""[..]),
                "case {case:?}: a no-content answer carries no body bytes"
            );
            assert_eq!(
                framed.headers().get("content-length"),
                Some(NO_CONTENT_LENGTH),
                "case {case:?}: it states the length it has, got {:?}",
                framed.headers().get("content-length")
            );
            assert_eq!(
                framed.headers().get("content-type"),
                Some("application/json"),
                "case {case:?}: every other header is the renderer's"
            );
        }
    }

    #[test]
    fn an_answer_that_carries_content_is_untouched() {
        // Every other status keeps its body, whatever it is: the successes whose output has
        // members, and the modelled failures.
        for (status, body) in [
            (200_u16, "{\"member\":1}"),
            (201, "{}"),
            (304, ""),
            (400, "{\"__type\":\"ValidationException\"}"),
            (500, "{\"__type\":\"InternalServerException\"}"),
        ] {
            let framed = without_content(rendered(status, body));
            assert_eq!(
                framed.status().as_u16(),
                status,
                "case {status}: the status is untouched"
            );
            assert_eq!(
                framed.body().bytes(),
                Some(body.as_bytes()),
                "case {status}: the body is the renderer's, got {:?}",
                framed.body().bytes()
            );
            assert_eq!(
                framed.headers().get("content-length"),
                None,
                "case {status}: no framing is added to an answer that carries content"
            );
        }
    }

    #[test]
    fn framing_twice_frames_once() {
        let once = without_content(rendered(NO_CONTENT_STATUS, "{}"));
        let twice = without_content(once);
        assert_eq!(
            twice.body().bytes(),
            Some(&b""[..]),
            "a second framing leaves the answer as it was"
        );
        assert_eq!(
            twice.headers().get("content-length"),
            Some(NO_CONTENT_LENGTH),
            "a second framing states the same length, got {:?}",
            twice.headers().get("content-length")
        );
    }
}
