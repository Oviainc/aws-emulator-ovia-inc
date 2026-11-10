// SPDX-License-Identifier: CC0-1.0

//! The access entry Amazon EKS materialises for the principal that CREATES a
//! cluster, and the resolution of WHICH principal that is.
//!
//! `CreateAccessConfigRequest$bootstrapClusterCreatorAdminPermissions` "specifies
//! whether or not the cluster creator IAM principal was set as a cluster admin
//! access entry during cluster creation time. The default value is `true`" — so the
//! flag does not merely record an intention: when it holds, the create writes a real
//! [`AccessEntryRecord`] for the calling IAM principal, carrying one cluster-scoped
//! association of the `cluster-admin` access policy. That entry is an access entry
//! like any other, so the readers of the slice
//! (`ListAccessEntries`, `DescribeAccessEntry`, `ListAssociatedAccessPolicies`) see
//! it, `CreateAccessEntry` for the same principal is the conflict it is, and
//! `DeleteAccessEntry` removes it.
//!
//! Nothing here is keyed to the authentication mode: the member's documentation ties
//! the entry to the FLAG alone, and this service's `CreateAccessEntry` likewise
//! writes an entry whatever `accessConfig.authenticationMode` the cluster carries.
//!
//! Who the caller is comes from the same place `sts:GetCallerIdentity` answers from
//! — the session the request's access key id was minted for, else the IAM user that
//! holds that long-term key — so the entry names the principal the client reads back
//! for the credentials it signed with rather than a fixed identity. A session is a
//! temporary principal that cannot hold an access entry
//! (`CreateAccessEntryRequest$principalArn`: "You can't use the STS session
//! principal type with access entries"), so a role session resolves to the ROLE it
//! is a session of, which is the permanent principal behind it.

use super::*;

use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

/// The access policy the bootstrap entry is granted: `cluster-admin`, one of the
/// policies [`ACCESS_POLICY_NAMES`] publishes, so the association the creator reads
/// back names a policy `ListAccessPolicies` reports and `AssociateAccessPolicy`
/// would accept.
pub const CLUSTER_ADMIN_ACCESS_POLICY_NAME: &str = "AmazonEKSClusterAdminPolicy";

/// The IAM principal the request this operation is serving speaks for, as
/// `sts:GetCallerIdentity` reports it — and for a role session, the role behind it.
///
/// The session registry is read through STS's own resolution, which materialises the
/// expiries the clock has already made due before it answers; a key that names no
/// session speaks for the IAM user that holds it, and one no store holds at all is
/// the account's own published credential. A credential whose session has already
/// expired speaks for that same long-term identity here: the layer that
/// AUTHENTICATES a request is what refuses such a credential, before any handler of
/// this service runs.
pub fn request_caller_principal_arn<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    now: SystemTime,
) -> String {
    let access_key_id = ::service_common::request_access_key_id(ctx.parts());
    // IAM answers which user holds a long-term key, and its lock is given up before
    // the session registry's is taken, so no two of this emulator's stores are held
    // at once.
    let long_term = match sts_store::iam_state(engine) {
        Some(iam) => sts_store::long_term_identity(Some(&iam.lock()), &access_key_id),
        None => sts_store::long_term_identity(None, &access_key_id),
    };
    let fallback = long_term.arn.clone();
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            now,
            ctx.credential_store(),
            long_term,
        )
        .ok(),
        // No session registry on this engine: the credential names no session, so it
        // speaks for the long-term identity it was resolved to.
        None => None,
    };
    match resolved {
        // "The temporary security credentials … have the permissions of the role":
        // the permanent principal behind a role session is that role, and the role
        // is what an access entry can be filed under.
        Some(caller) => caller.role_arn.unwrap_or(caller.identity.arn),
        None => fallback,
    }
}

/// The access entry a create whose effective
/// `bootstrapClusterCreatorAdminPermissions` is true materialises for
/// `principal_arn`, with its one `cluster-admin` association.
///
/// `None` when the calling credential speaks for something that is not an IAM user
/// or role — an access entry is keyed by such a principal and nothing else
/// ([`parse_iam_principal_arn`]), so there is no entry to file for it.
///
/// Everything the entry reports is computed here and stored, exactly as
/// `CreateAccessEntry` stores what it wrote: `type` is `STANDARD` (the type every
/// IAM principal that is not a node gets), `username` is the principal itself
/// ([`default_username`]), the entry is given no Kubernetes groups — Amazon EKS
/// grants it the cluster-admin policy instead of a group binding — and the ARN and
/// the two instants are the server-generated parts.
pub fn cluster_creator_access_entry(
    region: &str,
    cluster_name: &str,
    principal_arn: &str,
    now: SystemTime,
) -> Option<AccessEntryRecord> {
    let principal = parse_iam_principal_arn(principal_arn)?;
    Some(AccessEntryRecord {
        cluster_name: cluster_name.to_string(),
        principal_arn: principal_arn.to_string(),
        arn: access_entry_arn(region, cluster_name, &principal, principal_arn),
        created_at: now,
        // Nothing has modified the entry: the create that wrote it is its last
        // change.
        modified_at: now,
        kubernetes_groups: Vec::new(),
        username: default_username(ACCESS_ENTRY_TYPE_STANDARD, principal_arn),
        entry_type: ACCESS_ENTRY_TYPE_STANDARD.to_string(),
        tags: BTreeMap::new(),
        // No call carried an idempotency token for this entry: Amazon EKS created
        // it, not a `CreateAccessEntry` the caller retried.
        client_request_token: None,
        associated_policies: [(
            access_policy_arn(CLUSTER_ADMIN_ACCESS_POLICY_NAME),
            AssociatedPolicyRecord {
                policy_arn: access_policy_arn(CLUSTER_ADMIN_ACCESS_POLICY_NAME),
                // The creator administers the WHOLE cluster, so the association is
                // cluster-scoped and names no namespace.
                scope_type: ACCESS_SCOPE_TYPE_CLUSTER.to_string(),
                namespaces: Vec::new(),
                associated_at: now,
                modified_at: now,
            },
        )]
        .into_iter()
        .collect(),
    })
}
