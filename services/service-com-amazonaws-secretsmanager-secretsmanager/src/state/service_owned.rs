// SPDX-License-Identifier: CC0-1.0

//! The secrets ANOTHER Amazon Web Services service creates and MANAGES for one of its own
//! resources, rather than a caller creating them for itself.
//!
//! "Several Amazon Web Services services create and manage secrets in Secrets Manager" —
//! the master user password an RDS DB instance has RDS generate is the shape — and such a
//! secret is not an ordinary secret of the account:
//!
//! * it reports the service that created it as `OwningService` ("The ID of the service that
//!   created this secret", `DescribeSecretResponse$OwningService`), which is how a client
//!   tells a credential a service manages from one a caller created;
//! * it is NAMED under the prefix AWS reserves for that service, `<service>!…` — a form no
//!   caller can mint, because the character joining the two is not one a secret name may
//!   carry ([`super::SECRET_NAME_EXTRA_CHARS`]);
//! * it carries the tags the OWNING service applies, which are in the `aws:` namespace
//!   ([`super::RESERVED_TAG_PREFIX`]) and so are likewise not a caller's to write.
//!
//! WHICH credential the create presents is what says the call is the owning service's. The
//! calling service mints a credential in the emulator's own credential store for the one
//! call, signs that call with it, and records under that credential's access key id the
//! service principal it is calling as and the ARN of the resource it is calling for
//! ([`::service_common::service_principal_calls`], the one directory in the shared engine
//! both sides read). This module resolves that record for the request being served — from
//! the access key id the request PRESENTS, the one thing about a request a client cannot
//! choose for itself — and so answers one question for the handler: is this create the
//! owning service's own, and which service is it?
//!
//! A request no record answers for is an ordinary caller: the reserved name is refused as
//! the malformed name it is, and a reserved tag key is refused as a caller's to write.

use super::RESERVED_TAG_PREFIX;

/// The character AWS joins a service's id to the rest of a service-owned secret's name
/// with: `rds!db-…`, `events!connection/…`.
///
/// It is deliberately NOT one of the characters a secret name may carry
/// ([`super::SECRET_NAME_EXTRA_CHARS`]), which is what reserves the whole namespace: a
/// caller's `CreateSecret` cannot name a secret in this form at all.
pub const SERVICE_OWNED_NAME_MARK: char = '!';

/// The service id the name `name` claims to be owned by, for a name in the reserved
/// `<service>!<rest>` form; `None` for every other name.
///
/// The claim is read from the name only to decide whether the name is in that form at all:
/// what a create is ALLOWED to carry is decided by the calling service principal
/// ([`owning_service_of`]), never by the text the request chose.
#[must_use]
pub fn claimed_owner(name: &str) -> Option<&str> {
    let (owner, rest) = name.split_once(SERVICE_OWNED_NAME_MARK)?;
    let service_id = !owner.is_empty()
        && owner
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (service_id && super::is_valid_secret_name(rest)).then_some(owner)
}

/// The service whose own principal is making this request, for a request another emulated
/// service is making as its service principal on behalf of one of its resources.
///
/// The id is the first label of that principal (`rds.amazonaws.com` → `rds`), which is the
/// id `OwningService` reports and the prefix such a service's secret names carry.
#[must_use]
pub fn owning_service_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<String> {
    let directory = service_principal_calls(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let account = ::engine::emulator_account_id();
    let now = ::std::time::SystemTime::now();
    let held = directory.lock();
    let call = held.call(&access_key_id, account, now)?;
    let service_id = call.principal.split('.').next().unwrap_or_default();
    (!service_id.is_empty()).then(|| service_id.to_string())
}

/// What a create may carry beyond what a caller's create may: the service that owns the
/// secret it creates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwningService {
    /// The id this secret reports as `OwningService`.
    pub service_id: String,
}

impl OwningService {
    /// Whether `name` is a name this owner may create a secret under: its own reserved
    /// prefix, or an ordinary name (a service may create a secret of its own under either,
    /// and only the reserved form is closed to everybody else).
    #[must_use]
    pub fn may_name(&self, name: &str) -> bool {
        match claimed_owner(name) {
            Some(claimed) => claimed == self.service_id,
            None => super::is_valid_secret_name(name),
        }
    }

    /// Whether `key` is a tag key this owner may apply: AWS's own tag namespace is where
    /// the tags a service applies to the secret it manages live.
    #[must_use]
    pub fn may_tag(&self, key: &str) -> bool {
        key.strip_prefix(RESERVED_TAG_PREFIX)
            .is_some_and(|rest| rest.starts_with(&format!("{}:", self.service_id)))
    }
}

/// The owning service of the request being served, for a request that is one — the one
/// call a handler makes to decide both of the questions above.
#[must_use]
pub fn requesting_owner<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<OwningService> {
    owning_service_of(engine, ctx).map(|service_id| OwningService { service_id })
}

/// The shared directory of the calls another service is making as its own service
/// principal, in the one shared engine: written by that service, read here.
fn service_principal_calls<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::service_principal_calls::ServicePrincipalCalls>>
{
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::service_principal_calls::ServicePrincipalCalls,
            >,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a name in the `<service>!<rest>` form claims an owner, and the rest of the name
    /// is still a name this service accepts: the rows differ along the mark's presence, the
    /// shape of the claimed id and the characters after the mark.
    #[test]
    fn a_reserved_name_claims_the_service_before_the_mark() {
        for (case, name, expected) in [
            ("an instance secret", "rds!db-db-ABCDEF", Some("rds")),
            ("a cluster secret", "rds!cluster-cluster-ABC", Some("rds")),
            ("another service", "events!connection/a/b", Some("events")),
            ("an ordinary name", "rds-db-ABCDEF", None),
            ("a name with no owner", "!db-ABCDEF", None),
            ("an owner with a capital", "RDS!db-ABCDEF", None),
            ("nothing after the mark", "rds!", None),
            ("a space after the mark", "rds!db ABCDEF", None),
            ("two marks", "rds!db!ABCDEF", None),
        ] {
            let found = claimed_owner(name);
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected:?} for {name:?}, got {found:?}"
            );
        }
    }

    /// An owner may name its own reserved prefix and an ordinary name, and may apply only
    /// the `aws:` tags of its own namespace.
    #[test]
    fn an_owner_names_its_own_prefix_and_tags_its_own_namespace() {
        let owner = OwningService {
            service_id: "rds".to_string(),
        };
        for (case, name, expected) in [
            ("its own prefix", "rds!db-ABCDEF", true),
            ("another service's prefix", "events!connection/a/b", false),
            ("an ordinary name", "rds-db-ABCDEF", true),
            ("a name no secret may carry", "db ABCDEF", false),
        ] {
            assert_eq!(
                owner.may_name(name),
                expected,
                "case {case:?}: expected may_name({name:?}) == {expected}"
            );
        }
        for (case, key, expected) in [
            ("its own namespace", "aws:rds:primaryDBInstanceArn", true),
            ("another service's", "aws:cloudformation:stack-name", false),
            ("a caller's own key", "owner", false),
            ("the bare prefix", "aws:", false),
        ] {
            assert_eq!(
                owner.may_tag(key),
                expected,
                "case {case:?}: expected may_tag({key:?}) == {expected}"
            );
        }
    }
}
