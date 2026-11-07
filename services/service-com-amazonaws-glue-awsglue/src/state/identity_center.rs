// SPDX-License-Identifier: CC0-1.0

//! The account's Glue Identity Center configuration: the Identity Center instance it is
//! registered against, the application Glue mints for it, and the reading that says whether
//! the instance a request names is one this account holds.

use super::*;

/// The IAM Identity Center store the account's instances live in.
///
/// It is the Identity Center service's OWN state struct, reached through the SAME shared
/// engine this service's store lives in, so Glue resolves the very instances
/// `CreateInstance` wrote and `ListInstances` reports rather than a second copy of them.
pub type IdentityCenterState =
    ::service_com_amazonaws_ssoadmin_swbexternalservice::state::ComAmazonawsSsoadminSwbexternalserviceState;

/// Whether the account's Identity Center ANSWERED that `instance_arn` names no instance of
/// this account.
///
/// The instance an application is registered with has to be one the account holds, so a
/// create that names another is refused
/// ([`messages::IDENTITY_CENTER_INSTANCE_NOT_IN_ACCOUNT`]) and registers nothing. WHICH
/// instances the account holds is the owning service's own rule, asked here through its
/// read-only reading of it
/// ([`::service_com_amazonaws_ssoadmin_swbexternalservice::state::ComAmazonawsSsoadminSwbexternalserviceState::holds_instance`]),
/// so nothing about that rule is restated in this crate: it covers the instances that store
/// holds and the account's AMBIENT instance, the one that service hands an account whose
/// enablement nothing has decided yet. It is a READ, so a Glue create decides none of that.
///
/// `false` — do not refuse — when nothing is KNOWN about the account's instances: an
/// emulator with no Identity Center mounted to ask. A refusal has to be grounded in what the
/// owning service answered, never assumed.
pub fn identity_center_instance_is_absent<Eng: Send + Sync + 'static>(
    engine: &Eng,
    instance_arn: &str,
) -> bool {
    use ::service_com_amazonaws_ssoadmin_swbexternalservice::state as identity_center;

    let Some(state) = identity_center::service_state(engine) else {
        return false;
    };
    let held = state.lock().holds_instance(instance_arn);
    !held
}

/// The Glue Identity Center configuration of one account: the Identity Center instance the
/// account's Glue services authenticate against, plus the application Glue registered with
/// that instance.
#[derive(Debug, Clone)]
pub struct IdentityCenterConfigurationEntry {
    /// "The Amazon Resource Name (ARN) of the Identity Center application associated with
    /// the Glue configuration" — minted by the create
    /// ([`new_identity_center_application_arn`]) and stored, so every later read reports
    /// the same application.
    pub application_arn: String,
    /// "The Amazon Resource Name (ARN) of the Identity Center instance", as the create
    /// named it.
    pub instance_arn: String,
    /// "A list of Identity Center scopes that define the permissions and access levels for
    /// the Glue configuration", as the last write set them.
    pub scopes: Option<Vec<String>>,
    /// "Indicates whether users can run background sessions when using Identity Center
    /// authentication with Glue services", as the last write set it.
    pub user_background_sessions_enabled: Option<bool>,
}

impl IdentityCenterConfigurationEntry {
    /// Register the configuration a `CreateGlueIdentityCenterConfiguration` asks for,
    /// against the Identity Center instance `instance_arn` names.
    pub fn new(
        instance_arn: &str,
        scopes: Option<&Vec<String>>,
        user_background_sessions_enabled: Option<bool>,
    ) -> Self {
        Self {
            application_arn: new_identity_center_application_arn(instance_arn),
            instance_arn: instance_arn.to_string(),
            scopes: scopes.cloned(),
            user_background_sessions_enabled,
        }
    }
}

/// How many Identity Center application ids this process has minted. A nonce source for
/// [`new_identity_center_application_arn`], exactly as [`MINTED_DATABASE_IDS`] is for a
/// database id.
pub(super) static MINTED_IDENTITY_CENTER_APPLICATIONS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words an Identity Center application id carries: one, i.e. 64 bits
/// rendered as 16 lowercase hexadecimal characters, which is the width Identity Center
/// spells the id part of an application id (`apl-<16 hex>`) in.
pub(super) const IDENTITY_CENTER_APPLICATION_ID_WORDS: usize = 1;
/// The prefix Identity Center gives an application id, as it appears in the resource part
/// of an application ARN (`application/<instance id>/apl-<id>`).
pub(super) const IDENTITY_CENTER_APPLICATION_PREFIX: &str = "apl-";
/// The service namespace an Identity Center ARN is spelled under, which is the namespace of
/// both the instance ARN a create names and the application ARN it mints.
pub(super) const IDENTITY_CENTER_SERVICE: &str = "sso";
/// The resource segment an Identity Center INSTANCE ARN carries
/// (`arn:aws:sso:::instance/<instance id>`), which is what makes the ARN a create names an
/// instance rather than some other Identity Center resource.
pub const IDENTITY_CENTER_INSTANCE_SEGMENT: &str = "instance/";

/// The `ApplicationArn` of the Glue application registered with the Identity Center
/// instance `instance_arn` names.
///
/// An Identity Center application is a resource OF its instance, so its ARN carries the
/// instance's own id before the application id
/// (`arn:aws:sso::<account>:application/<instance id>/apl-<id>`) — the instance id read off
/// the ARN the create named, and the application id minted here from [`mint_hex_id`], so
/// two configurations created in the same nanosecond cannot be given the same application.
/// Minted once, at create, and stored on the entry, so every later read reports the same
/// ARN.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_identity_center_application_arn(instance_arn: &str) -> String {
    let instance_id = identity_center_instance_id(instance_arn).unwrap_or_default();
    let application_id = mint_hex_id(
        &MINTED_IDENTITY_CENTER_APPLICATIONS,
        IDENTITY_CENTER_APPLICATION_ID_WORDS,
    );
    ::engine::emulator_arn(
        IDENTITY_CENTER_SERVICE,
        "",
        &format!(
            "application/{instance_id}/{IDENTITY_CENTER_APPLICATION_PREFIX}{application_id}"
        ),
    )
}

/// The instance id inside an Identity Center INSTANCE ARN
/// (`arn:aws:sso:::instance/ssoins-…`), or `None` when the value names no such instance.
///
/// That is what a create's `InstanceArn` has to be: the ARN of the instance Glue registers
/// its application with, so a value carrying no instance resource segment names no instance
/// to register against.
pub fn identity_center_instance_id(instance_arn: &str) -> Option<&str> {
    let (prefix, instance_id) = instance_arn.split_once(IDENTITY_CENTER_INSTANCE_SEGMENT)?;
    if !prefix.starts_with("arn:") || instance_id.is_empty() || instance_id.contains('/') {
        return None;
    }
    Some(instance_id)
}
