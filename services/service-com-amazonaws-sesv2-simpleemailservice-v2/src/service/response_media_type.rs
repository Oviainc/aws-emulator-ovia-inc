// SPDX-License-Identifier: CC0-1.0

//! Unit cases for the media type this service announces.
//!
//! The rule, the pair it is applied across and the question that decides it are
//! [`super`]'s — this module only pins them, on a response the framework has already
//! rendered, across what a request can declare and the statuses a walk answers with.
//! The end-to-end cases over the wire are in
//! `tests/all/contact_plane_wire_fidelity.rs`.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use service_common::response_media_type::AWS_JSON_1_1;

use super::{announced, CONTENT_TYPE, REST_JSON_MEDIA_TYPE};

/// A request declaring `content_type`, or declaring none at all when it is `None`.
fn request(content_type: Option<&str>) -> RtRequest {
    let mut req = RtRequest::new(SdkBody::from(&b"{}"[..]));
    if let Some(content_type) = content_type {
        req.headers_mut()
            .insert(CONTENT_TYPE, content_type.to_string());
    }
    req
}

/// A response as the framework's renderers stamp one: the protocol-implied media type,
/// under `status`.
fn response(status: u16) -> RtResponse {
    let mut resp = RtResponse::new(
        aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status code"),
        SdkBody::from(&b"{}"[..]),
    );
    resp.headers_mut().insert(CONTENT_TYPE, REST_JSON_MEDIA_TYPE);
    resp
}

/// A response to a request that declared no media type of its own is announced under this
/// service's default, whatever the status; a response to a request that declared one
/// keeps the protocol-implied value.
#[test]
fn only_the_answer_to_a_request_declaring_no_media_type_is_reannounced() {
    // The rows span what a request can declare — nothing (a read, a delete), the media
    // type an SDK declares for a JSON body, the AWS JSON media type a caller may declare
    // instead, and a header present but empty, which declares nothing — against the
    // statuses a walk answers with: a success, the three modelled refusals of this
    // service (400, 404, 429) and the scaffold's 501.
    let requests: [(&str, Option<&str>, &str); 4] = [
        ("a request declaring no media type", None, AWS_JSON_1_1),
        (
            "a request declaring a JSON body",
            Some(REST_JSON_MEDIA_TYPE),
            REST_JSON_MEDIA_TYPE,
        ),
        (
            "a request declaring the AWS JSON media type",
            Some(AWS_JSON_1_1),
            REST_JSON_MEDIA_TYPE,
        ),
        ("a header present but empty", Some(""), AWS_JSON_1_1),
    ];

    for (case, declared, expected) in requests {
        for status in [200u16, 400, 404, 429, 501] {
            let answered = announced(&request(declared), response(status));
            let actual = answered.headers().get(CONTENT_TYPE);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?} (HTTP {status}): expected content-type {expected:?}, got \
                 {actual:?}"
            );
        }
    }
}

/// A response whose own producer chose another media type is left exactly as it was,
/// whatever the request declared: the pair replaces the protocol-implied value and
/// nothing else.
#[test]
fn a_media_type_the_producer_chose_is_left_alone() {
    // An event-stream frame and a binary payload: the two shapes a producer of this
    // service's responses can choose that are not the protocol-implied media type.
    for chosen in ["application/vnd.amazon.eventstream", "application/octet-stream"] {
        for declared in [None, Some(REST_JSON_MEDIA_TYPE)] {
            let mut resp = response(200);
            resp.headers_mut().insert(CONTENT_TYPE, chosen);
            let answered = announced(&request(declared), resp);
            let actual = answered.headers().get(CONTENT_TYPE);
            assert_eq!(
                actual,
                Some(chosen),
                "case {chosen:?} (request declaring {declared:?}): expected the chosen \
                 media type to be left alone, got {actual:?}"
            );
        }
    }
}
