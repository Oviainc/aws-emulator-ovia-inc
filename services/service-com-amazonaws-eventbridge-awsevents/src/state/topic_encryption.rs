// SPDX-License-Identifier: CC0-1.0

//! The customer managed key an Amazon SNS TOPIC target is encrypted with:
//! whether this service may have a data key generated under it, which is what
//! publishing one event to such a topic needs.
//!
//! The rule the services follow: a topic carrying `KmsMasterKeyId` has every
//! message published to it encrypted under that key, and Amazon SNS performs
//! that encryption by calling `kms:GenerateDataKey` for the publisher — so the
//! KEY's own policy is a second document in force over a delivery to such a
//! topic, beside the topic's own access policy, and a key policy that does not
//! admit this service's principal is a publish that does not happen however
//! freely the topic's policy grants `sns:Publish`.
//!
//! The decision is KMS's OWN ([`kms_authorization::authorize_use`]) over the
//! key record the KMS service's own handlers wrote, so `PutKeyPolicy` moves it
//! with nothing else changed and the two services cannot disagree about what a
//! key policy says.

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::state::authorization as kms_authorization;

/// The topic attribute Amazon SNS reports an encrypted topic's key under: "the
/// ID of an Amazon Web Services managed customer master key (CMK) for Amazon SNS
/// or a custom CMK" (`GetTopicAttributes`'s `KmsMasterKeyId`).
pub const KMS_MASTER_KEY_ATTRIBUTE: &str = "KmsMasterKeyId";

/// The KMS operation encrypting ONE published message performs: "GenerateDataKey
/// returns a unique symmetric data key for use outside of KMS", which is how a
/// service encrypts a payload under a customer managed key.
const GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// The KMS store in the SAME shared engine, which is where the key a topic is
/// encrypted with lives. `None` means the engine carries no such store — a
/// deployment with no KMS to ask, which is evidence of nothing.
pub fn kms_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<KmsState>> {
    kms_store::service_state(engine)
}

/// The KMS store, as its own service's state struct — the one `CreateKey` and
/// `PutKeyPolicy` write.
pub type KmsState = ::service_com_amazonaws_kms_trentservice::ComAmazonawsKmsTrentserviceState;

/// Whether the key `identifier` names withholds `kms:GenerateDataKey` from this
/// service, so a message EventBridge publishes to the topic `topic_arn` names
/// cannot be encrypted at all.
///
/// The use is one Amazon SNS makes on the publisher's behalf rather than one this
/// account asked KMS for directly, so it is judged as arriving THROUGH that
/// service ([`kms_authorization::RequestedUse::through_service`]), which is what a
/// key policy scoped by `kms:ViaService` is written against; and the principal it
/// is judged FOR is this service's own ([`super::SERVICE_PRINCIPAL`]), acting for
/// the topic, because EventBridge publishes as itself and not as any identity of
/// the account. Such a principal holds no identity-based policies, so the KEY's
/// own policy is the only authority over the use: a key carrying only the policy
/// `CreateKey` generates for the account root admits it nothing until a statement
/// names it.
///
/// `false` wherever nothing of the kind can be shown, so a delivery is never
/// withheld on a guess: an engine carrying no KMS store, an identifier naming no
/// key this Region holds, an Amazon Web Services managed key (whose policy AWS
/// manages for the service that owns it), and a key that cannot be used in a
/// cryptographic operation right now at all — that is a different failure from a
/// permission the key withholds, and this reports only the permission.
///
/// Call it with no store of this service locked: the KMS guard is taken here and
/// released before it answers.
pub fn generate_data_key_denied(
    kms: Option<&::engine::ServiceState<KmsState>>,
    region: &str,
    identifier: &str,
    topic_arn: &str,
) -> bool {
    let Some(kms) = kms else {
        // Visible to an operator: a lookup nothing answered lets the publish
        // through, so without this line it would be indistinguishable from a key
        // whose policy KMS said admits this service.
        ::tracing::warn!(
            "no KMS service is mounted beside EventBridge; an encrypted topic target's key \
             is accepted"
        );
        return false;
    };
    let caller = kms_authorization::Caller::service_principal(super::SERVICE_PRINCIPAL, topic_arn);
    let via_service = format!("sns.{region}.amazonaws.com");
    let store = kms.lock();
    let kms_store::KeyRef::Found(key) = store.resolve(region, identifier) else {
        return false;
    };
    if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER || !key.is_usable() {
        return false;
    }
    kms_authorization::authorize_use(
        &caller,
        &store,
        region,
        key,
        kms_authorization::RequestedUse::through_service(
            GENERATE_DATA_KEY_OPERATION,
            &via_service,
        ),
        // No encryption context is presented, so a grant constrained to one does
        // not permit this use: the authority it carries is not claimed rather
        // than assumed.
        None,
        None,
    )
    .is_err()
}
