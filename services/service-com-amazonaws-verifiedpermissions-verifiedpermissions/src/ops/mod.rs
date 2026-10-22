// SPDX-License-Identifier: CC0-1.0

//! Operation handlers for com-amazonaws-verifiedpermissions-verifiedpermissions. One file per operation.

include!(concat!(env!("OUT_DIR"), "/com_amazonaws_verifiedpermissions_verifiedpermissions_ops_generated.rs"));

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
