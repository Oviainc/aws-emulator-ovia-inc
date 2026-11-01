// SPDX-License-Identifier: CC0-1.0

//! The bucket the request this thread is serving ADDRESSES, for the one thing that needs it
//! after a handler has already returned: the `BucketName` a bucket-scoped error document
//! names the bucket by when the refusal itself did not name one
//! ([`crate::scaffolded::Scaffolded::naming_bucket`] is how a handler names it, and is what
//! a refusal about a bucket OTHER than the one addressed must use).
//!
//! Not state: a request-scoped note, needed because a refusal and the REQUEST that provoked
//! it are handed to two different seams. The serializer that writes the error document is
//! handed the error value, the error code and the status — and S3's error shapes declare no
//! members at all, so a refusal that did not carry the name has nowhere else to get it from.
//!
//! The note lives in a THREAD-LOCAL for the same reason [`crate::state::request_ids`]'s
//! does, and under the same discipline:
//!
//! * EVERY request this service reads records one as it is deserialized
//!   (`service::wire::UnwrappedXmlRestXml::deserialize_input`), including a request that
//!   addresses no bucket at all, which records `None` — so the slot never holds what an
//!   earlier request left in it.
//! * The error document TAKES it ([`addressed`]) rather than reading it, so a note is spent
//!   by the one response it was recorded for.
//!
//! The two sides are one poll apart: the framework deserializes the request, runs the
//! handler and serializes its answer in one stretch of the one dispatch
//! (`vendor/smithy-server-runtime`'s request walk), so the note an answer reads is the one
//! that answer's own request recorded. A note that is missing leaves the element off, which
//! is what an envelope about no bucket carries anyway.
//!
//! That holds for a handler that returns within the one poll, which is every handler of
//! this service but one: a handler that AWAITS something that can pend may be resumed on
//! another worker thread, and must therefore name its own buckets
//! ([`crate::scaffolded::Scaffolded::naming_bucket`]) instead of relying on this note —
//! `ops/put_bucket_notification_configuration.rs`, which resolves a configuration's
//! destinations against the services that own them, does exactly that at both of its
//! bucket-scoped refusals, and a new handler that awaits must do the same. Taking the note
//! unconditionally is what bounds the cost of getting that wrong to a MISSING element on
//! another in-flight request rather than a wrong one.

use std::cell::RefCell;

thread_local! {
    /// The bucket the request this thread is serving addresses, for the length of that one
    /// request.
    static ADDRESSED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Record `bucket` as the bucket the request this thread is serving addresses, `None` for a
/// request that addresses none.
///
/// Called for EVERY request, so the value is always this request's: a `None` overwrites
/// whatever the previous request on this thread recorded rather than leaving it to be read
/// by the next refusal.
pub(crate) fn remember(bucket: Option<&str>) {
    let recorded = bucket.map(str::to_string);
    ADDRESSED.with(|note| {
        *note.borrow_mut() = recorded;
    });
}

/// The bucket recorded for the request now being answered, TAKEN rather than read: the note
/// is request-scoped, so spending it is what keeps one request's bucket out of a later
/// request's error document.
pub(crate) fn addressed() -> Option<String> {
    ADDRESSED.with(|note| note.borrow_mut().take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_bucket_is_answered_once_and_then_gone() {
        remember(Some("note-bucket"));
        assert_eq!(
            addressed().as_deref(),
            Some("note-bucket"),
            "the recorded bucket must be the one answered"
        );
        assert_eq!(
            addressed(),
            None,
            "a note is spent by the response it was recorded for, got {:?}",
            addressed()
        );
    }

    #[test]
    fn a_request_that_addresses_no_bucket_clears_the_slot() {
        remember(Some("note-previous"));
        remember(None);
        assert_eq!(
            addressed(),
            None,
            "a request addressing no bucket must not read the previous one's note, got {:?}",
            addressed()
        );
    }
}
