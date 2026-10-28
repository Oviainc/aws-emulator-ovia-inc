// SPDX-License-Identifier: CC0-1.0

//! The two request-correlation ids ONE response carries, and the seam that lets the
//! operation that served a request and the serializer that answers it name the same
//! request.
//!
//! Every S3 response carries them: "`x-amz-request-id`: A value created by Amazon S3 that
//! uniquely identifies the request" and "`x-amz-id-2`: A special token that helps Amazon
//! Web Services troubleshoot problems" (Amazon S3 API Reference, *Common Response
//! Headers*). A client reads them back as `ResponseMetadata.RequestId` and
//! `ResponseMetadata.HostId`.
//!
//! They are not only a response header. An EVENT NOTIFICATION names the call it came of by
//! the SAME request id — the classic record's `responseElements` carries both headers and
//! the EventBridge detail carries `request-id` — so a consumer can tell which API call
//! produced which event. That is why the pair is minted ONCE per served request, here,
//! rather than in the serializer: the handler publishes its event under the pair and then
//! records it ([`RequestCorrelation::answer`]) as the pair its response carries, which the
//! serializer takes ([`answering`]) when it writes the headers.
//!
//! The record lives in a THREAD-LOCAL because the two sides of it are one poll apart: a
//! handler records the pair as the LAST thing it does, with nothing awaited after it, and
//! the request walk serializes its output immediately afterwards, so nothing can run on
//! this thread between the record and the read. Two things keep that from degrading
//! quietly: the serializer TAKES the pair before anything that can fail, and every request
//! CLEARS this thread's slot as it is deserialized ([`forget_any_unanswered`]) — so the
//! slot a handler records into is always empty, and a response is named either by its own
//! request's pair or by a freshly minted one, never by another request's. A response
//! nothing recorded a pair for — every operation that publishes no event — is answered
//! with a freshly minted pair, so no response ever goes out without them.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many correlation pairs this process has minted.
///
/// A request id "uniquely identifies the request", so two responses may never carry the
/// same one: the ids are derived from this monotonic counter rather than from anything
/// about the request, so two identical requests are still told apart. They are correlation
/// values and not security tokens, which is why a counter-derived digest is the right
/// shape for them.
static MINTED_PAIRS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The pair the operation serving this thread's request recorded for its response.
    static ANSWERING: RefCell<Option<RequestCorrelation>> = const { RefCell::new(None) };
}

/// The two ids one served request is named by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestCorrelation {
    /// `x-amz-request-id`: 16 upper-case hex digits, the shape that header carries.
    pub request_id: String,
    /// `x-amz-id-2`: 64 lower-case hex digits, the shape that header carries.
    pub extended_request_id: String,
}

impl RequestCorrelation {
    /// A fresh pair, naming a request no other response names.
    ///
    /// Both ids come from ONE draw on the counter, so the `x-amz-request-id` and the
    /// `x-amz-id-2` of a response name the same request.
    pub fn mint() -> Self {
        use ::sha2::Digest;

        let drawn = MINTED_PAIRS.fetch_add(1, Ordering::Relaxed);
        let mut digest = ::sha2::Sha256::new();
        digest.update(b"s3-request-id:");
        digest.update(drawn.to_be_bytes());
        let bytes = digest.finalize();
        Self {
            request_id: ::hex::encode_upper(&bytes[..8]),
            extended_request_id: ::hex::encode(bytes),
        }
    }

    /// Record this pair as the one the response this thread is about to answer carries.
    ///
    /// Called by a handler as the LAST thing it does, with nothing awaited after it,
    /// because the request walk serializes the output in the same poll: see the module
    /// docs. The slot is empty by then — every request clears it as it is deserialized
    /// ([`forget_any_unanswered`]) — so a pair found here means a handler recorded twice
    /// and is replaced and reported rather than silently reused.
    pub fn answer(&self) {
        let recorded = self.clone();
        let left_behind = ANSWERING.with(|slot| slot.borrow_mut().replace(recorded));
        if let Some(left_behind) = left_behind {
            ::tracing::warn!(
                request_id = %left_behind.request_id,
                "this request recorded a second request-correlation pair; the first is \
                 discarded rather than named on another request's response"
            );
        }
    }
}

/// The pair recorded for the response now being serialized, TAKEN rather than read, so a
/// pair one response left behind can never name a later one.
pub(crate) fn answering() -> Option<RequestCorrelation> {
    ANSWERING.with(|slot| slot.borrow_mut().take())
}

/// Drop whatever pair this thread may still hold, called at the START of every request
/// this service deserializes.
///
/// The seam above is correct because a handler records its pair as the last thing it does
/// and the request walk serializes the output in the same poll. This closes the one way
/// that could stop being true: a pair recorded for a response that was never serialized
/// (a serializer that errored on its way to one) would otherwise sit in this thread's slot
/// until something took it, and the next response serialized on this thread would be named
/// by another request's id. Clearing it here means the slot a handler records into is
/// always empty, so [`answering`] either returns THIS request's pair or none at all.
pub(crate) fn forget_any_unanswered() {
    if let Some(left_behind) = answering() {
        ::tracing::warn!(
            request_id = %left_behind.request_id,
            "a request-correlation pair was left unanswered on this thread; it is \
             discarded rather than named on another request's response"
        );
    }
}
