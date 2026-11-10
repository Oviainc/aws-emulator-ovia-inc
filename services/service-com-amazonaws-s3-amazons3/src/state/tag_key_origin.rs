// SPDX-License-Identifier: CC0-1.0

//! Who is naming the tag keys one request carries.
//!
//! The namespace [`super::RESERVED_TAG_KEY_PREFIX`] names belongs to AWS: the published tag
//! restrictions say a requester may neither add, edit nor delete a tag in it, and S3 words
//! that refusal on each of its two tagging paths. But the tags in that namespace EXIST —
//! AWS's own services write them, through no public API at all. So the reservation is a rule
//! about WHO states the key, not about the key alone, and the one thing a tag-set write needs
//! to know beside the set itself is which of the two it is.
//!
//! In this emulator one service reaches another ONLY through the in-process callback
//! ([`::smithy_server_runtime::OperationContext::callback`]), so that callback is the inside
//! path, and the signal is the [`::smithy_server_runtime::CallbackRecursionDepth`] request
//! EXTENSION the framework's callback transport stamps on every in-process call. An extension
//! never travels over the wire: nothing a client can send — on any socket, signed with any
//! credential — sets it, so a client's reserved key is refused exactly as before. It is the
//! same framework signal other service crates read to tell an in-process call from a client's
//! request, so nothing about the transport is reconstructed here.

/// Which path a tag-set write arrived on, which is what decides whether a key in the
/// reserved `aws:` namespace may be stored ([`super::validate_tag_set`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKeyOrigin {
    /// A request a CLIENT made, arriving from a socket. A key in the reserved namespace is
    /// AWS's own, so such a request may not write one.
    Caller,
    /// A request another emulated service made in-process, on behalf of a resource it owns —
    /// the path the tags AWS sets from inside itself arrive on, which no client's request can
    /// be. A key in the reserved namespace travels on it.
    Service,
}

/// Where the tag keys this request names came from: see this module's own documentation for
/// why the extension is the signal and why a client cannot forge it.
pub fn tag_key_origin(ctx: &::smithy_server_runtime::OperationContext<'_>) -> TagKeyOrigin {
    match ctx
        .parts()
        .extensions
        .get::<::smithy_server_runtime::CallbackRecursionDepth>()
    {
        Some(_) => TagKeyOrigin::Service,
        None => TagKeyOrigin::Caller,
    }
}
