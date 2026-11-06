// SPDX-License-Identifier: CC0-1.0

//! Pagination of this service's enumerations: the CURSOR SCOPES its tokens are issued
//! for, the token codec and the paging the enumerations share.
//!
//! Split out of `crate::state` so one concern lives in one file; every name here is
//! re-exported from `crate::state`, which is the path handlers and tests use.

use super::*;

/// Cursor scope of `ListInstances`' pagination token. The enumeration takes no
/// filter, so its scope is the operation alone.
pub const CURSOR_INSTANCES: &str = "instances";

/// Cursor scope of `ListApplicationProviders`' pagination token. The enumeration walks
/// the whole AWS-owned catalog and takes no filter, so its scope is the operation alone.
pub const CURSOR_APPLICATION_PROVIDERS: &str = "applicationProviders";

/// Cursor scope of a `ListPermissionSets` enumeration: the operation and the instance
/// whose permission sets it walks.
pub fn cursor_permission_sets(instance_arn: &str) -> String {
    format!("permissionSets|{instance_arn}")
}

/// Cursor scope of a `ListTrustedTokenIssuers` enumeration: the operation and the
/// instance whose trusted token issuers it walks, since the key (a
/// `TrustedTokenIssuerArn`) only distinguishes rows WITHIN one instance.
pub fn cursor_trusted_token_issuers(instance_arn: &str) -> String {
    format!("trustedTokenIssuers|{instance_arn}")
}

/// Cursor scope of a `ListRegions` enumeration: the operation and the instance whose
/// enabled Regions it walks, since the key (a `RegionName`) only distinguishes rows
/// WITHIN one instance.
pub fn cursor_regions(instance_arn: &str) -> String {
    format!("regions|{instance_arn}")
}

/// Cursor scope of a `ListManagedPoliciesInPermissionSet` enumeration: the operation and
/// the permission set whose attached AWS managed policies it walks, qualified by the
/// instance that permission set belongs to (the same pair the read resolves against).
pub fn cursor_managed_policies(instance_arn: &str, permission_set_arn: &str) -> String {
    format!("managedPolicies|{instance_arn}|{permission_set_arn}")
}

/// Cursor scope of a `ListApplications` enumeration: the operation and the whole filter
/// that selected the rows — the instance whose applications it walks and both members of
/// the request's `ListApplicationsFilter` — since the key (an `ApplicationArn`) only
/// distinguishes rows WITHIN one such filter.
///
/// A filter member the caller omitted renders as an EMPTY segment, which is a scope of its
/// own: the three parts are joined by `|`, a character none of them can hold — the model's
/// `InstanceArn`, `AccountId` and `ApplicationProviderArn` patterns all exclude it — so no
/// two filters render the same scope.
pub fn cursor_applications(
    instance_arn: &str,
    application_account: Option<&str>,
    application_provider: Option<&str>,
) -> String {
    format!(
        "applications|{instance_arn}|{}|{}",
        application_account.unwrap_or_default(),
        application_provider.unwrap_or_default()
    )
}

/// Cursor scope of a `ListApplicationAccessScopes` enumeration: the operation and the
/// application whose access scopes it walks (the same application every scope read
/// resolves against).
pub fn cursor_access_scopes(application_arn: &str) -> String {
    format!("accessScopes|{application_arn}")
}

/// Cursor scope of a `ListApplicationGrants` enumeration: the operation and the
/// application whose grants it walks, since the key (the grant's type) only
/// distinguishes rows WITHIN one application.
pub fn cursor_application_grants(application_arn: &str) -> String {
    format!("applicationGrants|{application_arn}")
}

/// Cursor scope of a `ListApplicationAuthenticationMethods` enumeration: the operation
/// and the application whose authentication methods it walks, since the key (the
/// method's type) only distinguishes rows WITHIN one application.
pub fn cursor_application_authentication_methods(application_arn: &str) -> String {
    format!("applicationAuthenticationMethods|{application_arn}")
}

/// Cursor scope of a `ListApplicationAssignments` enumeration: the operation and the
/// application whose direct assignments it walks, since the key (a principal's type and
/// id) only distinguishes rows WITHIN one application.
pub fn cursor_application_assignments(application_arn: &str) -> String {
    format!("applicationAssignments|{application_arn}")
}

/// Cursor scope of a `ListApplicationAssignmentsForPrincipal` enumeration: the operation
/// and the whole filter that selected the rows — the instance whose applications it walks,
/// the principal whose assignments it reports and the `ListApplicationAssignmentsFilter`'s
/// application — since the key (an `ApplicationArn`) only distinguishes rows WITHIN one
/// such filter.
///
/// A filter member the caller omitted renders as an EMPTY segment, which is a scope of its
/// own, exactly as in [`cursor_applications`].
pub fn cursor_application_assignments_for_principal(
    instance_arn: &str,
    principal_type: &str,
    principal_id: &str,
    application_arn: Option<&str>,
) -> String {
    format!(
        "applicationAssignmentsForPrincipal|{instance_arn}|{principal_type}|{principal_id}|{}",
        application_arn.unwrap_or_default()
    )
}

/// Cursor scope of a `ListTagsForResource` enumeration: the operation and the resource
/// whose tags it walks, which is the whole filter that selected the rows.
pub fn cursor_tags(resource_arn: &str) -> String {
    format!("tags|{resource_arn}")
}

/// Cursor scope of a `ListAccountAssignments` enumeration: the operation and the whole
/// filter that selected the rows, since the key only distinguishes rows WITHIN it.
pub fn cursor_account_assignments(
    instance_arn: &str,
    account_id: &str,
    permission_set_arn: &str,
) -> String {
    format!("accountAssignments|{instance_arn}|{account_id}|{permission_set_arn}")
}

/// Cursor scope of a `ListAccountAssignmentCreationStatus` enumeration: the operation and
/// the whole filter that selected the rows — the instance whose creation history it walks
/// and the `StatusValues` it was filtered by — since the key only distinguishes rows
/// WITHIN one such filter.
pub fn cursor_account_assignment_creation_status(
    instance_arn: &str,
    status: Option<&str>,
) -> String {
    format!(
        "accountAssignmentCreationStatus|{instance_arn}|{}",
        status.unwrap_or_default()
    )
}

/// Cursor scope of a `ListAccountAssignmentsForPrincipal` enumeration: the operation and
/// the whole filter that selected the rows — the instance whose assignments it walks, the
/// principal whose assignments it reports and the `ListAccountAssignmentsFilter`'s account
/// — since the key only distinguishes rows WITHIN one such filter.
///
/// A filter member the caller omitted renders as an EMPTY segment, which is a scope of its
/// own, exactly as in [`cursor_applications`].
pub fn cursor_account_assignments_for_principal(
    instance_arn: &str,
    principal_type: &str,
    principal_id: &str,
    account_id: Option<&str>,
) -> String {
    format!(
        "accountAssignmentsForPrincipal|{instance_arn}|{principal_type}|{principal_id}|{}",
        account_id.unwrap_or_default()
    )
}

/// Cursor scope of a `ListAccountAssignmentDeletionStatus` enumeration: the operation and
/// the whole filter that selected the rows — the instance whose deletion history it walks
/// and the `StatusValues` it was filtered by — since the key only distinguishes rows
/// WITHIN one such filter.
pub fn cursor_account_assignment_deletion_status(
    instance_arn: &str,
    status: Option<&str>,
) -> String {
    format!(
        "accountAssignmentDeletionStatus|{instance_arn}|{}",
        status.unwrap_or_default()
    )
}

/// Cursor scope of a `ListAccountsForProvisionedPermissionSet` enumeration: the operation
/// and the whole filter that selected the rows — the instance, the permission set whose
/// provisioned accounts it walks and the `ProvisioningStatus` it was filtered by, since
/// the key (an account id) only distinguishes rows within one such filter.
pub fn cursor_accounts_for_provisioned_permission_set(
    instance_arn: &str,
    permission_set_arn: &str,
    provisioning_status: Option<&str>,
) -> String {
    format!(
        "accountsForProvisionedPermissionSet|{instance_arn}|{permission_set_arn}|{}",
        provisioning_status.unwrap_or_default()
    )
}

/// Cursor scope of a `ListPermissionSetsProvisionedToAccount` enumeration: the operation
/// and the whole filter that selected the rows — the instance, the account whose
/// provisioned permission sets it walks and the `ProvisioningStatus` it was filtered by,
/// since the key (a permission-set ARN) only distinguishes rows within one such filter.
pub fn cursor_permission_sets_provisioned_to_account(
    instance_arn: &str,
    account_id: &str,
    provisioning_status: Option<&str>,
) -> String {
    format!(
        "permissionSetsProvisionedToAccount|{instance_arn}|{account_id}|{}",
        provisioning_status.unwrap_or_default()
    )
}

/// Cursor scope of a `ListPermissionSetProvisioningStatus` enumeration: the operation and
/// the whole filter that selected the rows — the instance whose provisioning history it
/// walks and the `StatusValues` it was filtered by, since the key only distinguishes rows
/// within one such filter.
pub fn cursor_permission_set_provisioning_status(
    instance_arn: &str,
    status: Option<&str>,
) -> String {
    format!(
        "permissionSetProvisioningStatus|{instance_arn}|{}",
        status.unwrap_or_default()
    )
}

/// The refusal a `NextToken` this service did not issue earns, as the message of the
/// caller's own `ValidationException`.
pub const BAD_CURSOR_MESSAGE: &str =
    "NextToken is not a pagination token issued by this service for this enumeration";

/// Encode a pagination cursor: the SCOPE of the enumeration it belongs to (the
/// operation and the filter that selected the rows) and the key of the last item
/// returned on this page.
///
/// The cursor is SELF-IDENTIFYING — it carries a version marker and that scope, so a
/// token this service never issued (including one that happens to be valid hex) and a
/// token issued for a DIFFERENT scope (another operation, another instance, another
/// account or permission-set filter) are both rejected rather than silently resuming
/// inside an enumeration whose keys mean something else. Hex keeps the token inside
/// `Token`'s `^[-a-zA-Z0-9+=/_]*$` pattern even though the scope and the keys (ARNs)
/// contain characters it rejects, and it is reversible, so the next page resumes from
/// the key rather than from a positional offset that a concurrent create would shift.
pub fn encode_page_token(scope: &str, key: &str) -> String {
    ::hex::encode(format!("ssoadmin:v1:{scope}:{key}"))
}

/// Decode a cursor [`encode_page_token`] issued for `scope`; `None` for anything else.
///
/// Accepted ONLY when the token is byte for byte what [`encode_page_token`] mints for the
/// cursor it carries: the decoder's acceptance set is the encoder's output set and nothing
/// wider, which is what makes the cursor self-identifying. `::hex::decode` also reads
/// spellings `::hex::encode` never writes (uppercase and mixed-case digits), and those
/// spell a token this service never handed out, so the decoded cursor is re-minted and
/// compared — keeping the two exact inverses whatever the encoding is.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let key = decoded.strip_prefix(&format!("ssoadmin:v1:{scope}:"))?;
    (encode_page_token(scope, key) == token).then(|| key.to_string())
}

/// The key a paginated read resumes after, from the caller's `NextToken`.
///
/// `Ok(None)` starts at the beginning (no token). `Err` is the refusal the caller's own
/// `ValidationException` carries: answering an empty page for a token this service did
/// not issue would silently end a caller's enumeration early.
pub fn cursor_after(
    scope: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, &'static str> {
    match next_token {
        None => Ok(None),
        Some(token) => decode_page_token(scope, token)
            .map(Some)
            .ok_or(BAD_CURSOR_MESSAGE),
    }
}

/// Where a page RESUMES after the cursor key the previous page ended on, which is the one
/// thing a sequence's order decides about its paging ([`page_from`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// By the ORDER of the keys: this page is the items whose key sorts strictly after the
    /// cursor. For a sequence that is ascending by key, so a create that lands before the
    /// cursor cannot shift the enumeration.
    AfterKeysUpTo,
    /// By the cursor's own ENTRY: this page is the items that FOLLOW it in the sequence.
    /// For a sequence served in the order its source publishes it, where a key says
    /// nothing about position — so a cursor the sequence does not hold cannot be resumed
    /// at all.
    AfterTheEntry,
}

/// One page of `sequence`, resuming after `after` as `resume` decides, at most `limit`
/// items, with the cursor the NEXT page resumes from.
///
/// `None` only for [`Resume::AfterTheEntry`] with a cursor the sequence does not hold:
/// there is no position to resume at, and an empty page would silently end a caller's
/// enumeration early.
fn page_from<T>(
    scope: &str,
    sequence: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    limit: usize,
    resume: Resume,
) -> Option<(Vec<T>, Option<String>)> {
    let mut resumed = after.is_none();
    let mut items = Vec::new();
    let mut last_key: Option<String> = None;
    let mut truncated = false;
    for (key, item) in sequence {
        if !resumed {
            match resume {
                Resume::AfterKeysUpTo => {
                    if after.is_some_and(|cursor| key.as_str() <= cursor) {
                        continue;
                    }
                    resumed = true;
                }
                Resume::AfterTheEntry => {
                    resumed = after == Some(key.as_str());
                    continue;
                }
            }
        }
        if items.len() == limit {
            truncated = true;
            break;
        }
        last_key = Some(key);
        items.push(item);
    }
    if !resumed {
        return None;
    }
    let next_token = if truncated {
        last_key.as_deref().map(|key| encode_page_token(scope, key))
    } else {
        None
    };
    Some((items, next_token))
}

/// One page of a key-ordered sequence.
///
/// `ordered` is ascending by key; `after` is the key the previous page ended on, so
/// this page is the items whose key sorts strictly after it. The returned token is
/// `Some` only when at least one item was left over — the last page carries none.
pub fn page<T>(
    scope: &str,
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<T>, Option<String>) {
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    // Resuming by key order needs no entry to resume AT, so this page always exists; the
    // empty default is unreachable and still answers a page rather than panicking.
    page_from(scope, ordered, after, limit, Resume::AfterKeysUpTo).unwrap_or_default()
}

/// One page of a sequence served in the order its SOURCE publishes it.
///
/// For a FIXED, service-published catalog the order its entries come back in is the
/// service's own data, so this read reports them in the order the source holds them and
/// imposes none of its own: `sequence` need not be key-ordered, and a page resumes at the
/// entry that FOLLOWS the key the previous page ended on rather than at the first key
/// that sorts after it. The page size is the same effective one every other enumeration
/// of this service serves ([`page`]): the caller's `MaxResults` when it named one, clamped
/// to [`MAX_PAGE_SIZE`], and [`MAX_PAGE_SIZE`] itself when it named none — a paginated
/// operation answers a PAGE, and the token it reports is what carries the rest of the
/// sequence to the caller.
///
/// `None` when `after` names a key the sequence does not hold: that cursor cannot be
/// resumed, and an empty page would silently end a caller's enumeration early, so the
/// caller refuses it with [`BAD_CURSOR_MESSAGE`] exactly as it refuses a cursor of
/// another scope.
pub fn page_in_published_order<T>(
    scope: &str,
    sequence: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Option<(Vec<T>, Option<String>)> {
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    page_from(scope, sequence, after, limit, Resume::AfterTheEntry)
}
