// SPDX-License-Identifier: CC0-1.0

//! How this service answers a request whose SIGNING INSTANT is too far from its own clock.
//!
//! Every assertion drives a real HTTP request through a router with request authentication
//! ON — as the emulator runs it by default — so what is pinned is the wire behaviour a client
//! sees: the status, the `<Code>`, the `<Message>` and the elements S3's own document carries
//! for this refusal.
//!
//! The rule under test: a signature is only as good as the moment it covers, so a request
//! whose `X-Amz-Date` is outside the window S3 accepts around its own clock — "the difference
//! between the request time and the current time cannot be more than 15 minutes" (Amazon S3
//! API Reference, *Signing and authenticating REST requests*) — is refused HTTP 403
//! `RequestTimeTooSkewed`, whatever its signature computes to, and the document states the
//! request's own instant, this service's instant and the window that is allowed. A request
//! stamped INSIDE the window is left to the signature to decide, which is the boundary of the
//! rule: the same request with a fresh stamp is refused `SignatureDoesNotMatch` instead.
//!
//! The rows differ along how far the stamp is from the clock and on which side of it; the
//! window's exact edge is pinned by the unit test beside the rule itself
//! (`service::sigv4`), where no time passes between the stamp and the judgement.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::signature_refusal_wording::{
    authorization, Emulator, SIGNATURE_MISMATCH_MESSAGE, WRONG_SIGNATURE,
};

/// The code S3 answers a request stamped outside that window with, and the sentence it
/// carries (Amazon S3 "Error responses", list of error codes: "RequestTimeTooSkewed — The
/// difference between the request time and the current time is too large", HTTP 403). Spelled
/// out here rather than imported, so a wording regression fails this suite.
const CLOCK_SKEW_CODE: &str = "RequestTimeTooSkewed";
/// See [`CLOCK_SKEW_CODE`].
const CLOCK_SKEW_MESSAGE: &str =
    "The difference between the request time and the current time is too large.";

/// The code a request stamped INSIDE the window is left to be refused by.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// The window S3 accepts around its clock, in the unit the refusal's own element states it
/// in: fifteen minutes of milliseconds.
const MAX_ALLOWED_SKEW_MILLISECONDS: &str = "900000";

/// This instant in epoch seconds, which the rows are offsets from.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_secs() as i64
}

/// `YYYYMMDDTHHMMSSZ` for the instant `seconds`, as SigV4 writes a signing date — rendered by
/// the library's own calendar rather than by any date arithmetic written here.
fn amz_date_of(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("an instant from the clock must render")
        .replace(['-', ':'], "")
}

#[tokio::test]
async fn a_request_signed_outside_the_accepted_window_is_refused_for_its_time() {
    let emulator = Emulator::new();
    let key_id = emulator.access_key_id.clone();
    let window = Duration::from_secs(15 * 60).as_secs() as i64;

    // (case, how far the stamp is from the clock in seconds, the code the refusal carries).
    // Every offset is a clear minute or more either side of the window, so a loaded machine
    // cannot move a row across it while the request is in flight.
    let rows: [(&str, i64, &str); 7] = [
        ("stamped at this instant", 0, SIGNATURE_MISMATCH_CODE),
        (
            "stamped a minute behind the clock",
            -60,
            SIGNATURE_MISMATCH_CODE,
        ),
        (
            "stamped fourteen minutes behind the clock",
            -window + 60,
            SIGNATURE_MISMATCH_CODE,
        ),
        (
            "stamped sixteen minutes behind the clock",
            -window - 60,
            CLOCK_SKEW_CODE,
        ),
        ("stamped 1500 s behind the clock", -1_500, CLOCK_SKEW_CODE),
        (
            "stamped sixteen minutes ahead of the clock",
            window + 60,
            CLOCK_SKEW_CODE,
        ),
        ("stamped an hour ahead of the clock", 3_600, CLOCK_SKEW_CODE),
    ];

    for (case, offset, expected_code) in rows {
        let signed_at = now_seconds() + offset;
        let amz_date = amz_date_of(signed_at);
        let answer = emulator
            .send_body(
                "PUT",
                "/skew-bucket/object.txt",
                &[
                    (
                        "authorization".to_string(),
                        authorization(&key_id, &amz_date, WRONG_SIGNATURE),
                    ),
                    ("x-amz-date".to_string(), amz_date.clone()),
                ],
                b"the bytes a stale request carries",
            )
            .await;

        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            expected_code,
            "case {case:?}: expected the {expected_code:?} code, got {:?} with body {:?}",
            answer.error_code(),
            String::from_utf8_lossy(&answer.body)
        );
        if expected_code == SIGNATURE_MISMATCH_CODE {
            assert_eq!(
                answer.error_message(),
                SIGNATURE_MISMATCH_MESSAGE,
                "case {case:?}: a stamp inside the window is left to the signature, got {:?}",
                answer.error_message()
            );
            continue;
        }

        assert_eq!(
            answer.error_message(),
            CLOCK_SKEW_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            answer.error_message()
        );
        // The document states the request's own instant, this service's, and the window: a
        // caller whose clock has drifted is told by how much.
        let stated = answer
            .element("RequestTime")
            .unwrap_or_else(|| panic!("case {case:?}: the refusal must state the request time"));
        assert_eq!(
            stated,
            amz_date,
            "case {case:?}: the refusal must state the instant the request signed, got \
             {stated:?}"
        );
        let server_time = answer
            .element("ServerTime")
            .unwrap_or_else(|| panic!("case {case:?}: the refusal must state the server's time"));
        let server_seconds = DateTime::from_str(&server_time, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the server time must be an ISO 8601 instant, got {error}")
            })
            .secs();
        assert!(
            (server_seconds - now_seconds()).abs() < 60,
            "case {case:?}: the server time must be this service's own clock, got \
             {server_time:?}"
        );
        assert_eq!(
            answer.element("MaxAllowedSkewMilliseconds").as_deref(),
            Some(MAX_ALLOWED_SKEW_MILLISECONDS),
            "case {case:?}: the refusal must state the window it accepts, got {:?}",
            answer.element("MaxAllowedSkewMilliseconds")
        );
    }
}
