// SPDX-License-Identifier: CC0-1.0

//! Whether the fine-grained access control a request asks a domain to be reached through
//! is a configuration the service can actually provision — the ONE statement of those
//! rules, so the create path and the update path judge the same request alike.
//!
//! `AdvancedSecurityOptions` is "options for enabling and configuring fine-grained access
//! control" (`smithy_query(query="shape",shape="AdvancedSecurityOptionsInput")`), and the
//! guide it points at states what a domain must already have for that boundary to work:
//! its data encrypted at rest, its node-to-node traffic encrypted and every request
//! arriving over HTTPS. Every member of the container is modelled optional because the
//! whole container is omittable — so what the model cannot say, and this module does, is
//! the CROSS-CONTAINER rule: a request that would leave fine-grained access control
//! switched on without one of those three asks for a domain the service does not
//! provision, so it is refused with that prerequisite's own sentence and nothing is
//! stored.
//!
//! Two of the switches are also ONE-WAY. The service cannot take back a boundary a domain
//! already runs behind, so an update asking for fine-grained access control or
//! node-to-node encryption switched off on a domain that has it switched on is refused and
//! the setting stays enabled. (The model states the same of encryption at rest —
//! `EncryptionAtRestOptions` "can only be used when creating a new domain or enabling
//! encryption at rest for the first time … you can't modify this parameter after it's
//! already been specified" — which this module does not state a sentence for, because the
//! service's wording for that refusal is not established here.)
//!
//! Nothing here reaches the clock or the store: each rule is a pure function of the
//! request and the record it would leave behind, so the same request decides the same way
//! twice.

use ::service_common::arn;

use crate::ops::types::{
    AdvancedSecurityOptionsInput, DomainEndpointOptions, EncryptionAtRestOptions,
    NodeToNodeEncryptionOptions, ValidationException,
};

/// What the service answers a request that switches fine-grained access control on for a
/// domain whose data is not encrypted at rest.
///
/// Fully FIXED text: it names the prerequisite that is missing, never the domain or any
/// other value of the request.
const ENCRYPTION_AT_REST_REQUIRED: &str =
    "You must enable encryption at rest to use advanced security options.";

/// What the service answers the same request for a domain whose node-to-node traffic is
/// not encrypted — the next prerequisite, worded the same way.
const NODE_TO_NODE_ENCRYPTION_REQUIRED: &str =
    "You must enable node-to-node encryption to use advanced security options.";

/// What the service answers the same request for a domain that does not require HTTPS.
///
/// The sentence names the member it is about (`DomainEndpointOptions$EnforceHTTPS`, "true
/// to require that all traffic to the domain arrive over HTTPS") rather than the feature,
/// which is why it reads differently from the two above.
const ENFORCE_HTTPS_REQUIRED: &str =
    "You must enable EnforceHTTPS in the domain endpoint options to use advanced security.";

/// What the service answers a master user ARN that names something no principal can
/// authenticate as.
///
/// Fully FIXED text: it states the types that ARE allowed, and does not echo the value
/// the request carried.
const MASTER_USER_ARN_NOT_A_PRINCIPAL: &str =
    "The master user ARN is not one of the allowed types: user, root user, or role.";

/// What the service answers an update that asks for fine-grained access control switched
/// off on a domain that has it switched on.
const ADVANCED_SECURITY_IS_ONE_WAY: &str =
    "You can't disable advanced security options once you enable it.";

/// What the service answers an update that asks for node-to-node encryption switched off
/// on a domain that has it switched on. The service's sentence for this switch carries no
/// closing full stop.
const NODE_TO_NODE_ENCRYPTION_IS_ONE_WAY: &str =
    "Once enabled, node to node encryption cannot be disabled";

/// Whether `options` switches fine-grained access control ON: `Enabled` is "true to enable
/// fine-grained access control", so a request carrying no container, or one whose
/// container leaves the member out or false, asks for no boundary at all.
#[must_use]
pub fn requested_enabled(options: Option<&AdvancedSecurityOptionsInput>) -> bool {
    options.and_then(|options| options.enabled) == Some(true)
}

/// Whether `options` asks for fine-grained access control switched OFF — the request an
/// update is refused for on a domain that already runs behind it.
///
/// A container that leaves `Enabled` out asks nothing of the switch, which is NOT the same
/// as asking for it off.
#[must_use]
fn requested_disabled(options: Option<&AdvancedSecurityOptionsInput>) -> bool {
    options.and_then(|options| options.enabled) == Some(false)
}

/// Whether a container the service stores or a request carries has its `Enabled` member
/// set — the shape of every one of these switches.
fn switched_on(enabled: Option<bool>) -> bool {
    enabled == Some(true)
}

/// Whether the configuration a request would leave the domain with can carry fine-grained
/// access control, and the refusal naming the ONE prerequisite that is missing otherwise.
///
/// `None` where the configuration does not switch the boundary on: the prerequisites are
/// what fine-grained access control needs, so a domain without it needs none of them. The
/// prerequisites are checked in the order the guide introduces them — encryption at rest,
/// then node-to-node encryption, then HTTPS-only traffic — so a request that leaves out
/// more than one is refused with the same sentence however it was built.
///
/// Each container is the one the configuration would END UP with: the create path passes
/// the containers the request carried, and the update path the merged record's, so an
/// update that enables the boundary on a domain that already encrypts its data is admitted
/// while one that enables it on a domain that does not is refused.
pub fn prerequisites_refusal(
    advanced_security_enabled: bool,
    encryption_at_rest: Option<&EncryptionAtRestOptions>,
    node_to_node_encryption: Option<&NodeToNodeEncryptionOptions>,
    domain_endpoint: Option<&DomainEndpointOptions>,
) -> Option<ValidationException> {
    if !advanced_security_enabled {
        return None;
    }
    let prerequisites = [
        (
            switched_on(encryption_at_rest.and_then(|options| options.enabled)),
            ENCRYPTION_AT_REST_REQUIRED,
        ),
        (
            switched_on(node_to_node_encryption.and_then(|options| options.enabled)),
            NODE_TO_NODE_ENCRYPTION_REQUIRED,
        ),
        (
            switched_on(domain_endpoint.and_then(|options| options.enforce_https)),
            ENFORCE_HTTPS_REQUIRED,
        ),
    ];
    prerequisites
        .into_iter()
        .find(|(met, _)| !met)
        .map(|(_, message)| ValidationException {
            message: Some(message.to_string()),
        })
}

/// Whether the master user `options` names is a principal that can authenticate, and the
/// refusal stating the allowed types otherwise.
///
/// `MasterUserOptions$MasterUserARN` is the "Amazon Resource Name (ARN) for the master
/// user", and the master user is WHO fine-grained access control grants everything to: an
/// ARN naming anything other than an IAM identity names a principal no request can arrive
/// as, so the configuration is refused rather than stored. Which forms those are is
/// [`arn::is_iam_principal_arn`]'s — the one reading of an identity ARN every service of
/// this emulator shares — so a role SESSION counts as a role, speaking for the role it is
/// a session of.
///
/// `ARN`'s pattern is `.*` and the boundary enforces only its length, so a value that is
/// no ARN at all is refused here too: it names no principal either. A request that carries
/// no master user ARN asks for none, so there is nothing to check — the same shape as
/// [`super::instance_types::master_user_password_refusal`].
pub fn master_user_arn_refusal(
    options: Option<&AdvancedSecurityOptionsInput>,
) -> Option<ValidationException> {
    let master_user_arn = options?
        .master_user_options
        .as_ref()?
        .master_user_arn
        .as_deref()?;
    if arn::is_iam_principal_arn(master_user_arn) {
        return None;
    }
    Some(ValidationException {
        message: Some(MASTER_USER_ARN_NOT_A_PRINCIPAL.to_string()),
    })
}

/// The refusal for an update that asks a one-way security switch of the domain to be
/// turned OFF, or `None` where it asks for nothing of the sort.
///
/// Fine-grained access control and node-to-node encryption are boundaries the service
/// cannot take back once a domain runs behind one, so a request that asks for either
/// switched off is refused and the setting stays enabled — whatever else the request
/// carried, and whatever the domain is busy with: the request names a configuration the
/// service never applies, so the answer does not depend on the domain's activity.
///
/// `stored_*` is read from the STORED record, before any merge, and the two `requested_*`
/// containers from the request: a request that leaves a switch out asks nothing of it and
/// is admitted.
pub fn one_way_switch_refusal(
    stored_advanced_security: bool,
    requested_advanced_security: Option<&AdvancedSecurityOptionsInput>,
    stored_node_to_node_encryption: bool,
    requested_node_to_node_encryption: Option<&NodeToNodeEncryptionOptions>,
) -> Option<ValidationException> {
    if stored_advanced_security && requested_disabled(requested_advanced_security) {
        return Some(ValidationException {
            message: Some(ADVANCED_SECURITY_IS_ONE_WAY.to_string()),
        });
    }
    if stored_node_to_node_encryption
        && requested_node_to_node_encryption.is_some_and(|options| options.enabled == Some(false))
    {
        return Some(ValidationException {
            message: Some(NODE_TO_NODE_ENCRYPTION_IS_ONE_WAY.to_string()),
        });
    }
    None
}
