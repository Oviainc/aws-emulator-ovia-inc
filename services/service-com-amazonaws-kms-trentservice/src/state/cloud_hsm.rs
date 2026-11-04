// SPDX-License-Identifier: CC0-1.0

//! The CloudHSM cluster behind an `AWS_CLOUDHSM` custom key store, and whether it can
//! back one.
//!
//! A CloudHSM key store names "the cluster ID of any active CloudHSM cluster that is
//! not already associated with a custom key store"
//! (`smithy_query(query="doc",shape="CreateCustomKeyStoreRequest",member="CloudHsmClusterId")`).
//! A CloudHSM cluster is NOT a KMS resource: the clusters an account holds are the
//! CloudHSM service's own records, so the cluster a request names is resolved against
//! THAT service's store, read through its own accessor — the way the identity half of
//! an authorization decision reads IAM's and STS's ([`super::authorization`]). This
//! service keeps no second copy of a cluster, so a cluster created, initialized or
//! deleted through CloudHSM's own operations is the one a key store is judged against.
//!
//! The read is a READ: nothing here writes to another service's store, and a status
//! change that service's clock owes is one it materialises in its own handlers. Every
//! state this can observe before that happens is a state that is not
//! [`CLUSTER_STATE_ACTIVE`] either, so it changes no answer given here.
//!
//! What a cluster's trust anchor certificate is — the `customerCA.crt` content a
//! customer creates when initializing the cluster — is not a fact the CloudHSM store
//! records, so there is nothing here to judge the request's `TrustAnchorCertificate`
//! against, and a request is not refused on evidence this emulator does not hold.
//!
//! This module owns the WORDING of each refusal as well as the decision: a class whose
//! message is fixed text carries it here ([`ClusterRefusal::message`]), so every
//! operation that takes a `CloudHsmClusterId` words the same refusal the same way and
//! no `src/ops/` module keeps a second copy of the text.

use ::service_com_amazonaws_cloudhsmv2_baldrapiservice::state as cloud_hsm_store;

/// The `ClusterState` of a cluster that has finished initializing and is serving, from
/// `smithy_query(query="shape",shape="ClusterState",model_path="services/service-com-amazonaws-cloudhsmv2-baldrapiservice/smithy-model/com-amazonaws-cloudhsmv2-baldrapiservice.json")`.
/// Every other state is one a cluster is on its way through or out of, which is what
/// "initialize and activate the cluster and try the command again"
/// (`CloudHsmClusterNotActiveException`) asks the caller to finish.
const CLUSTER_STATE_ACTIVE: &str = "ACTIVE";

/// The whole `message` KMS answers when the cluster a request names exists but is not
/// serving yet. The sentence is the complete text of this refusal class: no cluster id,
/// key store name or limit is interpolated into it, so every such refusal carries
/// exactly this string.
const CLUSTER_NOT_ACTIVE_MESSAGE: &str = "Cluster is not active.";

/// The fewest Availability Zones a cluster's private subnets have to span before it can
/// back a CloudHSM key store: "the CloudHSM cluster must be configured with private
/// subnets in at least two different Availability Zones in the Region"
/// (`CloudHsmClusterInvalidConfigurationException`).
pub const MIN_CLUSTER_AVAILABILITY_ZONES: usize = 2;

/// Why the CloudHSM cluster a request names cannot back a CloudHSM key store. Each
/// variant is one modelled refusal of the operations that take a `CloudHsmClusterId`,
/// so the decision is made once here and each handler words it on its own error
/// channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterRefusal {
    /// No cluster of this account has that id: "KMS cannot find the CloudHSM cluster
    /// with the specified cluster ID" (`CloudHsmClusterNotFoundException`). A cluster
    /// that is being deleted or is deleted is no longer one an operation can act on,
    /// so its id names none either.
    NotFound,
    /// The cluster exists but is not serving yet:
    /// `CloudHsmClusterNotActiveException`.
    NotActive,
    /// The cluster is serving but its network configuration does not meet the
    /// requirements for a key store:
    /// `CloudHsmClusterInvalidConfigurationException`.
    InvalidConfiguration,
}

impl ClusterRefusal {
    /// The `message` member a handler fills when it words this refusal on its own error
    /// channel, or `None` for a class that carries the error code alone.
    ///
    /// Only the not-active class has a message that is fixed text, which is why it can
    /// be stated once here for every operation that takes a `CloudHsmClusterId`; the
    /// other two classes send no message, as the recorded refusal of a cluster this
    /// account does not hold does.
    pub const fn message(self) -> Option<&'static str> {
        match self {
            Self::NotActive => Some(CLUSTER_NOT_ACTIVE_MESSAGE),
            Self::NotFound | Self::InvalidConfiguration => None,
        }
    }
}

/// Whether the cluster `cloud_hsm_cluster_id` names can back a CloudHSM key store, and
/// the refusal when it cannot.
///
/// Reached with NO store of this service locked: it locks the CloudHSM service's store
/// instead, so a handler resolves the cluster before taking its own guard.
pub fn usable_cluster<Eng: Send + Sync + 'static>(
    engine: &Eng,
    cloud_hsm_cluster_id: &str,
) -> Result<(), ClusterRefusal> {
    // No CloudHSM service mounted beside this one: this deployment holds no cluster at
    // all, so the id names none — the same answer as an id no cluster has, because the
    // caller asked about a cluster and there is none.
    let Some(clusters) = cloud_hsm_store::service_state(engine) else {
        return Err(ClusterRefusal::NotFound);
    };
    let clusters = clusters.lock();
    // A cluster id is global to the CloudHSM store, which holds one record per cluster
    // whatever endpoint created it, so the id alone names the cluster.
    let Some(cluster) = clusters
        .cluster(cloud_hsm_cluster_id)
        .filter(|cluster| !cluster.is_gone())
    else {
        return Err(ClusterRefusal::NotFound);
    };
    if cluster.state != CLUSTER_STATE_ACTIVE {
        return Err(ClusterRefusal::NotActive);
    }
    if cluster.subnet_mapping.len() < MIN_CLUSTER_AVAILABILITY_ZONES {
        return Err(ClusterRefusal::InvalidConfiguration);
    }
    Ok(())
}
