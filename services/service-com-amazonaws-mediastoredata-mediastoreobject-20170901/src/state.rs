// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediastoredata-mediastoreobject-20170901 service.
//!
//! The service has exactly one primary resource — the MediaStore object — so the
//! state is one map from the normalized object path to the bytes and metadata
//! `PutObject` persisted. Every other operation computes its answer from this map.
//!
//! The CONTAINER dimension is deliberately not modelled here: a MediaStore Data request
//! names its container only in the endpoint it is sent to, and a container is the
//! control-plane service's resource, not this crate's. So the modelled
//! `ContainerNotFoundException` every operation declares is unreachable in this
//! emulator — there is no state in which a container can be absent — and a request is
//! served as if its container exists. Container-level behaviour is filed as its own gap.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// The storage class an object is persisted into when the request names none.
///
/// `PutObject`'s modelled `StorageClass` enum admits exactly one value
/// (`TEMPORAL`), and the model's own documentation says a Put "Defaults to
/// high-performance temporal storage class", so an absent header persists this.
pub const DEFAULT_STORAGE_CLASS: &str = "TEMPORAL";

/// One stored MediaStore object: the bytes `PutObject` received plus the metadata
/// `DescribeObject` / `GetObject` / `ListItems` report back.
#[derive(Debug, Clone)]
pub struct StoredObject {
    /// The exact bytes of the `@httpPayload` body.
    pub body: Vec<u8>,
    /// The request's `Content-Type`, if it carried one.
    pub content_type: Option<String>,
    /// The request's `Cache-Control`, if it carried one.
    pub cache_control: Option<String>,
    /// The effective storage class — a value of the modelled `StorageClass` enum.
    pub storage_class: String,
    /// The request's `x-amz-upload-availability`, if it carried one.
    pub upload_availability: Option<String>,
    /// Hex content digest; `ETag` is modelled as `^[0-9A-Fa-f]+$`, len 1..64.
    pub etag: String,
    /// Lowercase hex SHA-256 of [`body`](Self::body); 64 hex characters.
    pub content_sha256: String,
    /// Byte length of [`body`](Self::body).
    pub content_length: i64,
    /// When this version of the object was stored.
    pub last_modified: SystemTime,
}

impl StoredObject {
    /// Persist `body` with the metadata of the request that carried it.
    ///
    /// `ETag` and `ContentSHA256` are both computed from the stored bytes, so
    /// identical content always reports an identical digest and any change of
    /// content changes both.
    pub fn new(
        body: Vec<u8>,
        content_type: Option<String>,
        cache_control: Option<String>,
        storage_class: String,
        upload_availability: Option<String>,
    ) -> Self {
        let digest = content_digest(&body);
        Self {
            content_length: body.len() as i64,
            etag: digest.clone(),
            content_sha256: digest,
            last_modified: whole_second_now(),
            body,
            content_type,
            cache_control,
            storage_class,
            upload_availability,
        }
    }
}

/// The lowercase hex SHA-256 of `bytes` — 64 hex characters, which is exactly
/// what the modelled `SHA256Hash` (`^[0-9A-Fa-f]{64}$`, len 64..64) admits.
pub fn content_digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// `SystemTime::now()` truncated to whole seconds.
///
/// `LastModified` is bound to the `Last-Modified` HTTP header, whose `HTTP-date`
/// rendering carries second precision, so the stored instant is truncated rather
/// than reported with sub-second digits a real HTTP date never has.
fn whole_second_now() -> SystemTime {
    let now = SystemTime::now();
    match now.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => SystemTime::UNIX_EPOCH + Duration::from_secs(since_epoch.as_secs()),
        Err(_) => now,
    }
}

/// The state key for an object path: the modelled `Path` with any leading `/`
/// removed, so `/folder/file.jpg` and `folder/file.jpg` name one object.
pub fn normalize_object_path(path: &str) -> String {
    path.trim_start_matches('/').to_string()
}

/// The key prefix a `ListItems` `Path` selects: empty for the container root,
/// otherwise the folder path with exactly one trailing `/`
/// (`/premium/`, `premium`, and `premium/` all select `premium/`).
pub fn folder_prefix(path: Option<&str>) -> String {
    let trimmed = path.unwrap_or_default().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// What a handler says when [`service_state`] cannot reach this service's store, kept
/// in one place so all five operations answer the same `InternalServerError` message.
pub const STORE_UNAVAILABLE: &str = "the MediaStore Data object store is unavailable";

// ---------------------------------------------------------------------------
// Store bounds and retention
//
// An object is created by a caller and reclaimed only by an explicit `DeleteObject` or
// by the process exiting (the store is in-memory and lives exactly as long as the
// engine), so every growth site is capped: a per-object byte ceiling, a count ceiling,
// and a ceiling on the total bytes retained. Nothing caller-sized reaches the store
// unchecked, and the product is finite: at most MAX_OBJECTS objects totalling at most
// MAX_STORED_BYTES bytes. The per-object ceilings are MediaStore's own documented
// limits; the two aggregate ceilings are this emulator's resource guards, since the
// service publishes no per-container object count or byte quota.
// ---------------------------------------------------------------------------

/// From `PutObject`'s own documentation: "Object sizes are limited to 25 MB for standard
/// upload availability and 10 MB for streaming upload availability."
pub const MAX_OBJECT_BYTES_STANDARD: usize = 25 * 1024 * 1024;

/// The streaming half of the documented limit; see [`MAX_OBJECT_BYTES_STANDARD`].
pub const MAX_OBJECT_BYTES_STREAMING: usize = 10 * 1024 * 1024;

/// The `UploadAvailability` value the smaller documented object-size limit applies to.
pub const UPLOAD_AVAILABILITY_STREAMING: &str = "STREAMING";

/// Emulator resource guard, NOT an AWS quota: the most objects this emulator stores.
pub const MAX_OBJECTS: usize = 1000;

/// Emulator resource guard, NOT an AWS quota: the most object bytes this emulator
/// retains across every stored object.
pub const MAX_STORED_BYTES: u64 = 512 * 1024 * 1024;

/// The documented byte ceiling for an object stored with this `UploadAvailability`.
pub fn max_object_bytes(upload_availability: Option<&str>) -> usize {
    if upload_availability == Some(UPLOAD_AVAILABILITY_STREAMING) {
        MAX_OBJECT_BYTES_STREAMING
    } else {
        MAX_OBJECT_BYTES_STANDARD
    }
}

/// Per-service state for com-amazonaws-mediastoredata-mediastoreobject-20170901. A handler reaches it through [`service_state`].
///
/// `objects` is a [`BTreeMap`], so `ListItems` enumerates in a deterministic
/// lexicographic order and a pagination cursor stays meaningful across calls.
#[derive(Debug, Default)]
pub struct ComAmazonawsMediastoredataMediastoreobject20170901State {
    /// Normalized object path -> the stored object. Written only through
    /// [`store_object`](Self::store_object) /
    /// [`remove_object`](Self::remove_object), which keep
    /// [`stored_bytes`](Self::stored_bytes) in step with it.
    pub objects: BTreeMap<String, StoredObject>,
    /// The total body bytes `objects` currently retains, so the byte ceiling is checked
    /// without walking the store.
    pub stored_bytes: u64,
}

impl ComAmazonawsMediastoredataMediastoreobject20170901State {
    /// `Ok` when storing `body_len` bytes at `key` keeps the store inside both of its
    /// aggregate ceilings. `Err` carries the message naming the bound it would break,
    /// for the caller to act on.
    ///
    /// Replacing an existing object neither adds a key nor retains the bytes it
    /// overwrites, so a re-put is measured against the new body only.
    pub fn check_capacity(&self, key: &str, body_len: usize) -> Result<(), String> {
        let replaced = self.objects.get(key);
        if replaced.is_none() && self.objects.len() >= MAX_OBJECTS {
            return Err(format!(
                "this emulator stores at most {MAX_OBJECTS} objects; it already holds {}. Delete an object before storing another.",
                self.objects.len()
            ));
        }
        let freed = replaced.map_or(0, |object| object.body.len() as u64);
        let projected = self
            .stored_bytes
            .saturating_sub(freed)
            .saturating_add(body_len as u64);
        if projected > MAX_STORED_BYTES {
            return Err(format!(
                "this emulator retains at most {MAX_STORED_BYTES} object bytes; storing these {body_len} bytes would need {projected}. Delete an object before storing another."
            ));
        }
        Ok(())
    }

    /// Store `object` at `key`, replacing any object already there, and keep
    /// [`stored_bytes`](Self::stored_bytes) in step. Call
    /// [`check_capacity`](Self::check_capacity) first.
    pub fn store_object(&mut self, key: String, object: StoredObject) {
        let added = object.body.len() as u64;
        let replaced = self.objects.insert(key, object);
        if let Some(previous) = replaced {
            self.stored_bytes = self.stored_bytes.saturating_sub(previous.body.len() as u64);
        }
        self.stored_bytes = self.stored_bytes.saturating_add(added);
    }

    /// Remove the object at `key`, reclaiming its bytes; `None` when none is stored.
    pub fn remove_object(&mut self, key: &str) -> Option<StoredObject> {
        let removed = self.objects.remove(key);
        if let Some(object) = &removed {
            self.stored_bytes = self.stored_bytes.saturating_sub(object.body.len() as u64);
        }
        removed
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediastoredataMediastoreobject20170901State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediastoredataMediastoreobject20170901State>)
}

#[cfg(test)]
mod tests {
    use super::{
        ComAmazonawsMediastoredataMediastoreobject20170901State as State, StoredObject,
        DEFAULT_STORAGE_CLASS, MAX_OBJECTS, MAX_STORED_BYTES,
    };

    fn object(len: usize) -> StoredObject {
        StoredObject::new(
            vec![b'x'; len],
            None,
            None,
            DEFAULT_STORAGE_CLASS.to_string(),
            None,
        )
    }

    /// The retained byte total follows every write and every delete, so the byte
    /// ceiling is measured against what the store actually holds.
    #[test]
    fn stored_bytes_tracks_the_objects_in_the_store() {
        let mut state = State::default();
        for (step, (key, len, expected)) in [
            ("a", 10usize, 10u64),
            ("b", 5, 15),
            ("a", 1, 6),
            ("a", 0, 5),
        ]
        .into_iter()
        .enumerate()
        {
            state.store_object(key.to_string(), object(len));
            assert_eq!(
                state.stored_bytes, expected,
                "step {step} (store {key:?}, {len} bytes): expected {expected} retained, got {}",
                state.stored_bytes
            );
        }
        state.remove_object("b");
        assert_eq!(
            state.stored_bytes, 0,
            "a delete must reclaim its bytes, got {}",
            state.stored_bytes
        );
        assert!(
            state.remove_object("b").is_none(),
            "removing an absent key must report nothing removed"
        );
    }

    /// Capacity is refused by count and by total bytes, and a replacement is measured
    /// against the body it overwrites rather than counted twice.
    #[test]
    fn capacity_is_bounded_by_count_and_by_bytes() {
        let mut state = State::default();
        for index in 0..MAX_OBJECTS {
            let key = format!("object-{index}");
            assert!(
                state.check_capacity(&key, 1).is_ok(),
                "case {key:?}: a store holding {index} objects must accept another"
            );
            state.store_object(key, object(1));
        }
        let refused = state.check_capacity("one-too-many", 1);
        assert!(
            refused.is_err(),
            "case one-too-many: a store at {MAX_OBJECTS} objects must refuse a new key, got {refused:?}"
        );
        let replacement = state.check_capacity("object-0", 1);
        assert!(
            replacement.is_ok(),
            "case object-0: replacing a stored object adds no key, got {replacement:?}"
        );

        let mut big = State::default();
        let over = usize::try_from(MAX_STORED_BYTES).unwrap_or(usize::MAX).saturating_add(1);
        let refused = big.check_capacity("huge", over);
        assert!(
            refused.is_err(),
            "case huge: {over} bytes is past the {MAX_STORED_BYTES} byte ceiling, got {refused:?}"
        );
        big.stored_bytes = MAX_STORED_BYTES;
        big.store_object("filler".to_string(), object(0));
        assert!(
            big.check_capacity("another", 1).is_err(),
            "case another: a full store must refuse one more byte"
        );
    }
}

