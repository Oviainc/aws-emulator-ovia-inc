// SPDX-License-Identifier: CC0-1.0

//! The HTTP STATUS this service answers its modelled errors with, shape by shape.
//!
//! The rule itself is [`crate::scaffolded::wire_status`], which every modelled error of
//! every operation is rendered through; this is its TABLE. The framework's `awsJson1_1`
//! renderers would otherwise take the status from each exception shape's `@httpError`
//! annotation, and several of this model's client errors annotate one this service does not
//! answer on the wire:
//!
//! ```text
//! smithy_query(query="shape",shape="ResourceNotFoundException,NotAuthorizedException,ResourceConflictException,InvalidParameterException,TooManyRequestsException,InternalErrorException")
//! ```
//!
//! reports `httpError` 404, 403, 409, 400, 429 and 500 for them, while every recorded
//! failure of this service that names one of the first three in its `awsJson1_1` body
//! answers HTTP 400 — the error class travels in the body's `__type` and in the
//! `x-amzn-errortype` header ([`super::error_type_header`]), not in the status.
//!
//! The wire behaviour — that a refused call of every operation carries that status — is
//! asserted through the router in `tests/all/wire_error_envelope.rs`; the rows below are the
//! rule itself, one per status the model annotates.

use crate::scaffolded::{wire_status, CLIENT_ERROR_STATUS};

/// Every `@httpError` status this model annotates an error shape with, and the status this
/// service answers it with.
///
/// The three client errors whose annotation is not 400 collapse onto it; a throttle and a
/// server fault keep their own, so a retry classifier still tells a server fault from a
/// client one and a throttled caller still sees a throttle.
#[test]
fn every_modelled_status_is_answered_with_the_one_this_service_answers() {
    // (the shape the status is annotated on, its `@httpError`, the status answered)
    let rows = [
        ("ResourceNotFoundException", 404u16, CLIENT_ERROR_STATUS),
        ("NotAuthorizedException", 403, CLIENT_ERROR_STATUS),
        ("ResourceConflictException", 409, CLIENT_ERROR_STATUS),
        ("InvalidParameterException", 400, CLIENT_ERROR_STATUS),
        ("ExternalServiceException", 400, CLIENT_ERROR_STATUS),
        ("LimitExceededException", 400, CLIENT_ERROR_STATUS),
        (
            "InvalidIdentityPoolConfigurationException",
            400,
            CLIENT_ERROR_STATUS,
        ),
        (
            "DeveloperUserAlreadyRegisteredException",
            400,
            CLIENT_ERROR_STATUS,
        ),
        ("ConcurrentModificationException", 400, CLIENT_ERROR_STATUS),
        ("TooManyRequestsException", 429, 429),
        ("InternalErrorException", 500, 500),
    ];

    for (shape, annotated, answered) in rows {
        let actual = wire_status(annotated);
        assert_eq!(
            actual, answered,
            "case {shape:?}: a shape annotated @httpError({annotated}) is answered \
             {answered}, got {actual}"
        );
    }
}
