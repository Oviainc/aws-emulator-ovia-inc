// SPDX-License-Identifier: CC0-1.0

//! Operation handlers for com-amazonaws-verifiedpermissions-verifiedpermissions. One file per operation.

include!(concat!(env!("OUT_DIR"), "/com_amazonaws_verifiedpermissions_verifiedpermissions_ops_generated.rs"));

/// The modelled `ResourceNotFoundException` for a reference that names no live
/// resource: the constant [`crate::state::RESOURCE_NOT_FOUND_MESSAGE`], with the
/// identity of what was not found in the members that model it.
///
/// The exception models that identity in its own required members — `resourceId`
/// is "The unique ID of the resource referenced in the failed request" and
/// `resourceType` is "The resource type of the resource referenced in the failed
/// request" (`smithy_query(query="shape", shape="ResourceNotFoundException")`) —
/// so the identifier has a modelled home and the message stays a fixed sentence.
/// A message formatted from the request would both duplicate those members in
/// unparseable prose and leak the looked-up value (an id, a name, a whole ARN
/// including the account id) into logs. This is the ONE place that builds the
/// shape, so every not-found path of this service answers the same text.
///
/// `resource_id` is the id of the missing resource (for a request that named it
/// by ARN, [`crate::state::resource_id_from_arn`] of that ARN) and
/// `resource_type` one of `ResourceType`'s values
/// ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
pub fn resource_not_found(
    resource_id: &str,
    resource_type: &str,
) -> self::types::ResourceNotFoundException {
    self::types::ResourceNotFoundException {
        message: crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The modelled `InvalidStateException` for a policy store whose deletion
/// protection is enabled: the constant
/// [`crate::state::DELETION_PROTECTION_MESSAGE`], which names no resource.
pub fn deletion_protection_enabled() -> self::types::InvalidStateException {
    self::types::InvalidStateException {
        message: crate::state::DELETION_PROTECTION_MESSAGE.to_string(),
    }
}

/// Emit the structured audit record for a state-changing or destructive
/// operation: when it happened, what operation ran, what it touched and how it
/// came out.
///
/// The framework's own per-request telemetry is gated on recording being active,
/// so a state change otherwise leaves no trace; this record is always emitted.
///
/// It carries SERVICE METADATA ONLY. Nothing is copied out of the request's
/// headers — not the `Authorization` header, not the access-key id inside it, not
/// the user agent: a header is credential-bearing content whether or not any one
/// field of it looks like a secret, and in a normal developer setup the signing
/// key is the developer's real one. Caller attribution, if it is ever wanted, has
/// to arrive as an off-by-default emulator setting with a non-reversible digest
/// and a stated record lifetime, which is not this change.
pub fn audit(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource_id: &str,
    outcome: &str,
) {
    let at_epoch_millis = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    ::tracing::info!(
        target: "audit",
        at_epoch_millis = %at_epoch_millis,
        service = %ctx.identity().signing_name(),
        operation = %operation,
        resource_id = %resource_id,
        outcome = %outcome,
        "verifiedpermissions state change"
    );
}
