// SPDX-License-Identifier: CC0-1.0

//! Which Certificate Manager certificates a LOAD BALANCER of this service presents, as
//! the service that HOLDS those certificates has to see it.
//!
//! A TLS-terminating listener serves a certificate this service does not own: the
//! certificate is Certificate Manager's resource, and that service reports the resources
//! using one and protects it while any of them does. It cannot see this store, so the
//! fact has to be written where it can read it — the ONE shared directory
//! ([`::service_common::acm_certificate_associations::CertificateAssociationDirectory`]),
//! which the engine keys by TYPE, so a copy of its own in this service's state would be
//! invisible to the reader.
//!
//! THE CONSUMER IS THE LOAD BALANCER, not the listener: a certificate is presented by the
//! endpoint that terminates the connection, and that endpoint is the load balancer, whose
//! ARN is the one a certificate's report names. So the set recorded against a load
//! balancer is the union of every certificate its listeners present — each listener's
//! default certificate and every entry of its certificate list
//! ([`super::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State::certificates_presented_by`])
//! — and one call rewrites that whole set ([`record_certificate_consumers`]), so a
//! listener created, modified, emptied or deleted leaves exactly the references the store
//! still holds.
//!
//! Only Certificate Manager certificate ARNs are recorded. A listener may equally serve a
//! server certificate uploaded to IAM ([`super::names_a_certificate`]), which is a
//! resource of that service and not of the one this directory is read by.
//!
//! LOCK ORDER: the caller holds this service's own store and this module takes the
//! directory under it, never the other way round, so two services cannot invert the order
//! between them.

use std::time::SystemTime;

use super::{ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState, ListenerRecord, SETTLE_AFTER};

/// The SHARED certificate-association directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// Answer `None` by leaving the directory unwritten: the record is an aid to the service
/// that holds the certificate, never part of this operation's own answer, so an engine
/// carrying no directory still serves the call.
pub fn certificate_association_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::acm_certificate_associations::CertificateAssociationDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::acm_certificate_associations::CertificateAssociationDirectory,
            >,
        )
}

impl ElbState {
    /// Every Certificate Manager certificate the load balancer `load_balancer_arn` names
    /// presents right now: the default certificate of each of its listeners and every
    /// entry of their certificate lists, as the store holds them.
    ///
    /// Derived from the listeners that EXIST rather than accumulated, exactly as the
    /// target-group associations a load balancer's listeners make are
    /// ([`ElbState::refresh_target_group_associations`]), so a listener that is gone
    /// contributes nothing and a load balancer this store no longer holds presents
    /// nothing at all.
    pub fn certificates_presented_by(&self, load_balancer_arn: &str) -> Vec<String> {
        let mut arns: Vec<String> = self
            .listeners
            .values()
            .filter(|listener| listener.load_balancer_arn == load_balancer_arn)
            .flat_map(ListenerRecord::presented_certificate_arns)
            .filter(|arn| ::service_common::arn::ACM_CERTIFICATE.admits(arn))
            .collect();
        arns.sort();
        arns.dedup();
        arns
    }
}

impl ListenerRecord {
    /// Every certificate ARN this listener presents to a client: its default certificate
    /// and every entry of its certificate list.
    ///
    /// Both are certificates the listener serves — the default one for a client that
    /// names no host, a list entry for one whose SNI selects it — so both make the load
    /// balancer a consumer of the certificate.
    pub fn presented_certificate_arns(&self) -> Vec<String> {
        self.certificate_arns
            .iter()
            .chain(self.list_certificate_arns.iter())
            .cloned()
            .collect()
    }
}

/// Record in the shared directory exactly which certificates the load balancer
/// `load_balancer_arn` presents, as `store` now holds it.
///
/// Called by every operation that changes a load balancer's certificates — a listener
/// created, modified, given or taken a list certificate, deleted, or the whole load
/// balancer removed — with this service's own store already locked and holding the result
/// of the change. The directory entry is REPLACED, so a reference the store no longer
/// holds cannot survive the call, and a load balancer this store holds nothing for ends
/// up presenting nothing.
///
/// A reference the load balancer gives up stays accounted for for [`SETTLE_AFTER`]: the
/// endpoints that were serving the certificate have to stop before it is really free, and
/// this service accepts that work rather than completing it inside the call — the same
/// span its other clock-owed moves take. Nothing sleeps for it.
pub fn record_certificate_consumers<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &ElbState,
    load_balancer_arn: &str,
    now: SystemTime,
) {
    let Some(directory) = certificate_association_directory(engine) else {
        return;
    };
    directory.lock().record_resource(
        load_balancer_arn,
        store.certificates_presented_by(load_balancer_arn),
        now,
        SETTLE_AFTER,
    );
}
