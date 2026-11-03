// SPDX-License-Identifier: CC0-1.0

//! The bucket the request this thread is serving ADDRESSES, for the one thing that needs it
//! after a handler has already returned: the `BucketName` a bucket-scoped error document
//! names the bucket by when the refusal itself did not name one
//! ([`crate::scaffolded::Scaffolded::naming_bucket`] is how a handler names it, and is what
//! a refusal about a bucket OTHER than the one addressed must use).
//!
//! The same note carries the OBJECT the request addresses — its key, and the version id the
//! request named — for the same reason: an object-scoped not-found document names the key
//! (and the version) that was not there, and no error shape of this model declares a member
//! to carry either. And the multipart UPLOAD it addresses, for the `UploadId` an
//! upload-scoped document names the upload that was not there by
//! ([`crate::scaffolded::UPLOAD_SCOPED_ERROR_CODES`]).
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
    /// The OBJECT that same request addresses, for the length of that one request.
    static ADDRESSED_OBJECT: RefCell<Option<AddressedObject>> = const { RefCell::new(None) };
    /// The multipart UPLOAD that same request addresses, for the length of that one request.
    static ADDRESSED_UPLOAD: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The object a request addresses: the key, and the version id the request named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddressedObject {
    /// The key, as the request spelled it once its path was decoded.
    pub(crate) key: String,
    /// The `versionId` the request named, for a request that named one.
    pub(crate) version_id: Option<String>,
}

/// Record the OBJECT the request this thread is serving addresses, `None` for a request that
/// addresses a bucket or the service root rather than an object.
///
/// Called for EVERY request, under the same discipline as [`remember`].
pub(crate) fn remember_object(object: Option<AddressedObject>) {
    ADDRESSED_OBJECT.with(|note| {
        *note.borrow_mut() = object;
    });
}

/// The object recorded for the request now being answered, TAKEN rather than read — see
/// [`addressed`].
pub(crate) fn addressed_object() -> Option<AddressedObject> {
    ADDRESSED_OBJECT.with(|note| note.borrow_mut().take())
}

/// Record the multipart UPLOAD the request this thread is serving addresses — the `uploadId`
/// it named — `None` for a request that addresses none.
///
/// Called for EVERY request, under the same discipline as [`remember`].
pub(crate) fn remember_upload(upload_id: Option<&str>) {
    let recorded = upload_id.map(str::to_string);
    ADDRESSED_UPLOAD.with(|note| {
        *note.borrow_mut() = recorded;
    });
}

/// The upload recorded for the request now being answered, TAKEN rather than read — see
/// [`addressed`].
pub(crate) fn addressed_upload() -> Option<String> {
    ADDRESSED_UPLOAD.with(|note| note.borrow_mut().take())
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

    #[test]
    fn a_recorded_object_is_answered_once_and_then_gone() {
        remember_object(Some(AddressedObject {
            key: "note/key.txt".to_string(),
            version_id: Some("note-version".to_string()),
        }));
        assert_eq!(
            addressed_object(),
            Some(AddressedObject {
                key: "note/key.txt".to_string(),
                version_id: Some("note-version".to_string()),
            }),
            "the recorded object must be the one answered"
        );
        assert_eq!(
            addressed_object(),
            None,
            "a note is spent by the response it was recorded for, got {:?}",
            addressed_object()
        );
    }

    #[test]
    fn a_request_that_addresses_no_object_clears_the_slot() {
        remember_object(Some(AddressedObject {
            key: "note/previous.txt".to_string(),
            version_id: None,
        }));
        remember_object(None);
        assert_eq!(
            addressed_object(),
            None,
            "a request addressing no object must not read the previous one's note, got {:?}",
            addressed_object()
        );
    }

    #[test]
    fn a_recorded_upload_is_answered_once_and_then_gone() {
        remember_upload(Some("note-upload-id"));
        assert_eq!(
            addressed_upload().as_deref(),
            Some("note-upload-id"),
            "the recorded upload must be the one answered"
        );
        assert_eq!(
            addressed_upload(),
            None,
            "a note is spent by the response it was recorded for, got {:?}",
            addressed_upload()
        );
    }

    #[test]
    fn a_request_that_addresses_no_upload_clears_the_slot() {
        remember_upload(Some("note-previous-upload"));
        remember_upload(None);
        assert_eq!(
            addressed_upload(),
            None,
            "a request addressing no upload must not read the previous one's note, got {:?}",
            addressed_upload()
        );
    }
}
