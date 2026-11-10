// SPDX-License-Identifier: CC0-1.0

//! WHO a flow's ownership members name: the one resolution of the principal a request
//! speaks for.
//!
//! `createdBy` ("The ARN of the user who created the flow") and `lastUpdatedBy`
//! ("Specifies the user name of the account that performed the most recent update") — the
//! two members a flow carries on both `DescribeFlowResponse` and the `FlowDefinition`
//! `ListFlows` projects — are the audit fields of a flow, so they have to name the identity
//! the request's CREDENTIAL belongs to, the identity the token service answers for that
//! same credential, and never a name this service invents. A client that compares a flow's
//! `createdBy` with its own identity to decide whether it owns the flow can only ever match
//! if the two answers come from one resolution.
//!
//! So the question is asked of the store that owns it: the session registry in the SAME
//! shared engine, through the token service's own resolution of a request's caller. A
//! credential that registry minted speaks for the session it was minted for; a long-term
//! access key speaks for the IAM user the account holds it for; and a key no store holds a
//! record of — the position the credential this emulator publishes at start is in — speaks
//! for the account's own principal.
//!
//! Nothing here opens a socket: both stores live in the one engine this service was
//! constructed from.

use std::time::SystemTime;

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

/// The ARN AppFlow records in `createdBy` / `lastUpdatedBy`: the principal the request's
/// access key id resolves to.
///
/// One resolution of that key id — the one `sts:GetCallerIdentity` answers the same
/// request with — so the principal a flow reports as its author is the principal the
/// caller reads as itself. A session whose expiry the clock has passed resolves to the
/// long-term identity instead: the same call materialises that expiry, which is a write the
/// clock already owed, and the credential then verifies for nobody.
///
/// An engine carrying no session registry at all (a router that mounts this service
/// without the token service) can hold no session, so the only identity a request can
/// present there is the one its long-term key resolves to.
pub fn caller_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let access_key_id = ::emulator_server::extract_akid(&ctx.parts().headers);
    let long_term = long_term_identity(engine, &access_key_id);
    let Some(registry) = sts_state::service_state(engine) else {
        return long_term.arn;
    };
    let resolved = sts_state::resolve_request_caller(
        &mut registry.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term.clone(),
    )
    .map(|caller| caller.identity);
    resolved.unwrap_or(long_term).arn
}

/// The identity a LONG-TERM access key speaks for: the token service's OWN resolution of
/// such a key against the account's IAM records — the user the account holds the key for,
/// and, for a key no IAM record names, the account's own principal.
///
/// Called rather than reproduced: a caller reading its identity through
/// `sts:GetCallerIdentity` and a caller reading the ARN this service recorded for the same
/// credential have to see one principal, which they cannot if an identity is spelled here
/// out of the access key id's text. Two callers that have to be two principals are two IAM
/// users of the account, each holding its own access key — the records this resolution
/// reads.
fn long_term_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
) -> sts_state::Identity {
    let iam = sts_state::iam_state(engine);
    let guard = iam.as_ref().map(|iam| iam.lock());
    sts_state::long_term_identity(guard.as_deref(), access_key_id)
}
