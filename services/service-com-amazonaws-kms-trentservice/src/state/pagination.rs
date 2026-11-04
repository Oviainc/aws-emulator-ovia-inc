// SPDX-License-Identifier: CC0-1.0

//! List pagination: the `NextMarker` each listing mints, and the refusal of one it did
//! not.
//!
//! One concern of [`super`]'s store, in its own file: the helpers here are reached as
//! `crate::state::…` exactly as they were before, because the parent re-exports them.

/// Tags a `NextMarker` as one this service minted, and for which list.
const MARKER_PREFIX: &str = "keys/1/";

/// [`MARKER_PREFIX`], for the alias list. A token is tagged per LIST so a marker
/// from one list is not accepted by another, which would silently resume a page at
/// an item of the wrong kind instead of answering `InvalidMarkerException`.
const ALIAS_MARKER_PREFIX: &str = "aliases/1/";

/// [`MARKER_PREFIX`], for one key's tag list.
const RESOURCE_TAG_MARKER_PREFIX: &str = "resource-tags/1/";

/// [`MARKER_PREFIX`], for the grant list.
const GRANT_MARKER_PREFIX: &str = "grants/1/";

/// [`MARKER_PREFIX`], for the retirable-grant list, which pages over a different
/// selection of grants than the per-key list and so mints its own tokens.
const RETIRABLE_GRANT_MARKER_PREFIX: &str = "retirable-grants/1/";

/// The `NextMarker` that resumes a list after `key_id`.
///
/// Hex-encoded so the token is opaque to a caller (who must pass it back
/// verbatim) while still satisfying `MarkerType`'s `^[\u0020-\u00FF]*$` pattern.
pub fn encode_marker(key_id: &str) -> String {
    encode_marker_for(MARKER_PREFIX, key_id)
}

/// The key id a `Marker` resumes after, or `None` when the token is not one this
/// service minted.
pub fn decode_marker(marker: &str) -> Option<String> {
    decode_marker_for(MARKER_PREFIX, marker)
}

/// [`encode_marker`], for the alias list: the token resumes after the alias whose
/// ARN is `alias_arn`, since the ARN is what identifies an alias.
pub fn encode_alias_marker(alias_arn: &str) -> String {
    encode_marker_for(ALIAS_MARKER_PREFIX, alias_arn)
}

/// [`decode_marker`], for the alias list.
pub fn decode_alias_marker(marker: &str) -> Option<String> {
    decode_marker_for(ALIAS_MARKER_PREFIX, marker)
}

/// [`encode_marker`], for one key's tag list: the token resumes after the tag whose
/// key is `tag_key`, since the tag key is what identifies a tag on a key.
pub fn encode_resource_tag_marker(tag_key: &str) -> String {
    encode_marker_for(RESOURCE_TAG_MARKER_PREFIX, tag_key)
}

/// [`decode_marker`], for one key's tag list.
pub fn decode_resource_tag_marker(marker: &str) -> Option<String> {
    decode_marker_for(RESOURCE_TAG_MARKER_PREFIX, marker)
}

/// [`encode_marker`], for the grant list: the token resumes after the grant whose
/// id is `grant_id`.
pub fn encode_grant_marker(grant_id: &str) -> String {
    encode_marker_for(GRANT_MARKER_PREFIX, grant_id)
}

/// [`decode_marker`], for the grant list.
pub fn decode_grant_marker(marker: &str) -> Option<String> {
    decode_marker_for(GRANT_MARKER_PREFIX, marker)
}

/// [`encode_marker`], for the retirable-grant list.
pub fn encode_retirable_grant_marker(grant_id: &str) -> String {
    encode_marker_for(RETIRABLE_GRANT_MARKER_PREFIX, grant_id)
}

/// [`decode_marker`], for the retirable-grant list.
pub fn decode_retirable_grant_marker(marker: &str) -> Option<String> {
    decode_marker_for(RETIRABLE_GRANT_MARKER_PREFIX, marker)
}

/// How KMS words `InvalidMarkerException`, "the marker that specifies where pagination
/// should next begin is not valid", on the listings whose refusal of one is recorded
/// carrying no message.
///
/// KMS refuses a marker it never issued with the error CODE alone — the refusal is
/// recorded on the wire with no `Message` member at all — so no explanatory text is
/// manufactured for it. What was wrong with the marker is stated in a comment at the
/// check that refuses it, not on the wire.
///
/// One function for every paginated operation that answers it that way, so no two of them
/// answer the same refusal with different members — which is why it is a function
/// returning `None` rather than `None` written out at each call site. The TAG listing is
/// recorded naming the token instead ([`invalid_next_marker_message`]).
#[must_use]
pub fn invalid_marker_message() -> Option<String> {
    None
}

/// How the TAG listing refuses a `Marker` it did not issue: by naming the token it
/// rejected, `Invalid NextMarker: <marker>`.
///
/// A `Marker` "specifies where pagination should next begin"
/// (`smithy_query(query="doc",shape="ListResourceTagsRequest",member="Marker")`), and a
/// token this reader never minted — or one naming a tag the key no longer carries —
/// resumes nowhere; the token is the whole of what the request got wrong, so it is the
/// refusal's one variable part and it travels back exactly as the request supplied it.
///
/// It is the tagging surface's own wording: the recorded refusal of a marker by the key,
/// alias and grant listings carries no message at all ([`invalid_marker_message`]), so
/// each surface keeps the wording its own refusal is recorded with rather than borrowing
/// the other's.
#[must_use]
pub fn invalid_next_marker_message(marker: &str) -> String {
    format!("Invalid NextMarker: {marker}")
}

/// [`MARKER_PREFIX`], for the custom key store list, which pages over custom key
/// stores rather than keys and so mints its own tokens.
const CUSTOM_KEY_STORE_MARKER_PREFIX: &str = "custom-key-stores/1/";

/// [`encode_marker`], for the custom key store list: the token resumes after the
/// key store whose id is `custom_key_store_id`.
pub fn encode_custom_key_store_marker(custom_key_store_id: &str) -> String {
    encode_marker_for(CUSTOM_KEY_STORE_MARKER_PREFIX, custom_key_store_id)
}

/// [`decode_marker`], for the custom key store list.
pub fn decode_custom_key_store_marker(marker: &str) -> Option<String> {
    decode_marker_for(CUSTOM_KEY_STORE_MARKER_PREFIX, marker)
}

/// [`MARKER_PREFIX`], for the key-material rotation list, which pages over the
/// versions of one key's material and so mints its own tokens.
const ROTATION_MARKER_PREFIX: &str = "key-rotations/1/";

/// [`encode_marker`], for the rotation list: the token resumes after the key material
/// whose `KeyMaterialId` is `key_material_id`.
pub fn encode_rotation_marker(key_material_id: &str) -> String {
    encode_marker_for(ROTATION_MARKER_PREFIX, key_material_id)
}

/// [`decode_marker`], for the rotation list.
pub fn decode_rotation_marker(marker: &str) -> Option<String> {
    decode_marker_for(ROTATION_MARKER_PREFIX, marker)
}

/// One encoding for every list, so no two pagers mint tokens a reader must handle
/// differently.
fn encode_marker_for(prefix: &str, id: &str) -> String {
    hex::encode(format!("{prefix}{id}"))
}

/// The inverse of [`encode_marker_for`], for the list `prefix` tags.
fn decode_marker_for(prefix: &str, marker: &str) -> Option<String> {
    let decoded = hex::decode(marker).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let id = text.strip_prefix(prefix)?;
    (!id.is_empty()).then(|| id.to_string())
}
