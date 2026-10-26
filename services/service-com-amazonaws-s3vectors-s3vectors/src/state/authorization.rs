// SPDX-License-Identifier: CC0-1.0

//! Whether the vector bucket's own resource policy refuses the request.
//!
//! A vector bucket policy is a RESOURCE-BASED policy (`PutVectorBucketPolicy$policy` is
//! "the JSON that defines the policy"), so it is not a document this service only stores
//! and reports back: it is evaluated on every call that acts on that bucket or on
//! something inside it, at the instant of the call. Nothing is cached, so a policy just
//! put refuses immediately and one just deleted stops refusing immediately — the document
//! is read out of the bucket record the write left behind.
//!
//! What is decided here is the one decision a resource policy takes that nothing else can
//! override: an EXPLICIT `Deny` about the request refuses it, whatever any grant says.
//! A policy that merely states no grant for the caller refuses nothing — within one
//! account the permission a caller is missing is its own, which is the identity-based
//! half of authorization and is not read here.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the
//!    one the caller reads back from STS for the credentials it signed with. Resolving it
//!    materialises the session expiries the clock has already made due, a write owed at
//!    that instant whether or not anybody called, which is why a REPORTING operation may
//!    make this decision: what a later caller sees is unchanged by the read itself.
//! 2. **the document** — the bucket record's stored policy ([`super::VectorBucketRecord`]),
//!    collected with the resources the request acts on by
//!    [`super::ComAmazonawsS3vectorsS3vectorsState::authorization_target`].
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against `(action, resource, principal)`, so this
//!    service and every other one handed the same document decide alike.
//!
//! ## The operations a bucket policy is not evaluated for
//!
//! The policy operations themselves — PutVectorBucketPolicy, GetVectorBucketPolicy and
//! DeleteVectorBucketPolicy — are the bucket owner's, and this emulator serves them for
//! the account that owns the bucket. That is AWS's own escape hatch for a bucket policy
//! that locks everyone out ("if you accidentally deny everyone access to your bucket …
//! sign in as the root user … and delete the policy"), and it is what keeps a `Deny` on
//! the policy actions from making a bucket's policy unrecoverable. CreateVectorBucket has
//! no bucket to carry a policy yet, and ListVectorBuckets is an account-level action
//! rather than one on a bucket, so neither has a document to be decided against.

use std::time::SystemTime;

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `s3vectors:<OperationName>`, which is
/// also this service's `arnNamespace` and signing name ([`super::ARN_SERVICE`]).
pub const ACTION_PREFIX: &str = super::ARN_SERVICE;

/// The `s3vectors:GetVectors` permission ListVectors ALSO requires when the caller asks
/// for vector data or metadata: "with only s3vectors:ListVectors permission, you can list
/// vector keys when returnData and returnMetadata are both set to false or not specified.
/// If you set returnData or returnMetadata to true, you must have both s3vectors:ListVectors
/// and s3vectors:GetVectors permissions" (ListVectors' own documentation).
pub const GET_VECTORS_OPERATION: &str = "GetVectors";

/// What an authorization decision about one request is made from: the bucket's stored
/// resource policy, and the ARNs of the resources the request acts on.
///
/// The FIRST resource is the one the action is taken on — the index for a request that
/// addresses an index, the bucket otherwise — which is the resource a refusal names. The
/// rest are the ancestors a bucket-policy statement may name instead: a request that
/// addresses an index acts on the bucket that holds it too, so a statement naming the
/// bucket is about the vector data inside it — which is also why a statement carrying no
/// `Resource` at all covers the request (the shared evaluator's reading of a resource
/// policy).
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// The bucket's policy document, or `None` while the bucket carries none — in which
    /// case there is nothing to decide and no principal to resolve.
    pub policy: Option<String>,
    /// The ARNs the decision is taken against, the resource the action is taken on first.
    pub resources: Vec<String>,
}

impl Target {
    /// The `AccessDeniedException` message (HTTP 403) when the bucket's resource policy
    /// explicitly denies one of `operations` on this target, or `None` when it does not.
    ///
    /// `caller` is who the request speaks for, resolved by [`caller`] BEFORE this
    /// service's own store is locked, so only one store is ever held at a time; the
    /// decision itself reads nothing but this target's own document, which
    /// PutVectorBucketPolicy already bounded.
    ///
    /// `operations` are this service's operation names — `Self::NAME` at the call site —
    /// which an IAM action names as `s3vectors:<OperationName>`. Several are passed where
    /// one call requires several permissions (see [`GET_VECTORS_OPERATION`]), and a deny
    /// of ANY of them refuses the call. The refusal names the resource the action is
    /// taken on, whichever of this target's ARNs the denying statement named.
    pub fn refusal(&self, caller: &Caller, operations: &[&str]) -> Option<String> {
        let policy = self.policy.as_deref()?;
        let acted_on = self.resources.first()?;
        for operation in operations {
            let action = format!("{ACTION_PREFIX}:{operation}");
            for resource in &self.resources {
                let request = iam::Request::new(&action, resource, &caller.principals);
                if iam::resource_decision(policy, &request) == Decision::Denied {
                    return Some(not_authorized(&caller.arn, &action, acted_on));
                }
            }
        }
        None
    }
}

/// The refusal in the sentence AWS words an authorization failure in: who was refused,
/// which action, on which resource, and what refused it. Every value comes from the
/// request, the caller's credential and the stored document.
fn not_authorized(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in a resource-based policy"
    )
}

/// The principal a request speaks for: the ARN a refusal blames, and every spelling a
/// resource-policy statement may name it by.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it.
    arn: String,
    /// The spellings a statement's `Principal` may name this caller by: its own ARN, and
    /// the account ones — a statement that denies the account denies each of its
    /// principals.
    principals: Vec<String>,
}

/// Who this request speaks for, resolved from the access key id it signed with.
///
/// Called BEFORE this service's own store is locked, so the STS and IAM stores this reads
/// are never reached while this service's state guard is held.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let Some(sts) = sts_store::service_state(engine) else {
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which is this account's and holds no role.
        let identity = sts_store::ambient_identity();
        return Caller {
            principals: vec![
                identity.arn.clone(),
                ::engine::emulator_arn("iam", "", "root"),
                identity.account,
            ],
            arn: identity.arn,
        };
    };
    // A long-term access key speaks for the IAM user that holds it, which is the identity
    // STS itself reports for it; a key no store holds is the account's own published
    // credential. Both stores are read through STS's own accessors, so this service sees
    // exactly what `sts:GetCallerIdentity` would answer.
    let iam = sts_store::iam_state(engine);
    let long_term = match &iam {
        Some(iam) => sts_store::long_term_identity(Some(&iam.lock()), &access_key_id),
        None => sts_store::ambient_identity(),
    };
    let resolved = sts_store::resolve_request_caller(
        &mut sts.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term,
    );
    match resolved {
        // Every spelling a policy statement may name this caller by is STS's own list:
        // its ARN, its account's root user and the bare account id that stands for it —
        // a statement that denies the account denies each of its principals — and, for an
        // assumed-role session, the ARN of the role it is a session of.
        Ok(resolved) => Caller {
            principals: resolved.trust_policy_principals(),
            arn: resolved.identity.arn,
        },
        // An expired session credential speaks for nobody: no statement names it, so only
        // a statement naming every principal is about such a request. It is still blamed
        // by the credential it signed with.
        Err(expired) => Caller {
            arn: format!("an expired session credential ({})", expired.access_key_id),
            principals: Vec::new(),
        },
    }
}
