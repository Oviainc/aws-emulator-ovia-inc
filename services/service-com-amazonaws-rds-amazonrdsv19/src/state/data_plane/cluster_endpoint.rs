// SPDX-License-Identifier: CC0-1.0

//! The DB CLUSTER data plane: the engine endpoint `Endpoint`/`Port` publishes.
//!
//! A DB cluster's endpoint is "the connection endpoint for the primary instance of the DB
//! cluster" — AN ADDRESS A CLIENT CONNECTS TO with its ordinary driver and then speaks the
//! engine's own protocol on. So for every cluster whose engine speaks a wire this emulator
//! serves ([`cluster_engine_wire`]) `CreateDBCluster` opens a listener through the CORE and
//! the control plane publishes THAT listener's host and port, exactly as
//! [`super::open_engine_endpoint`] does for a DB instance: an address nothing listens on is
//! not an endpoint. A cluster whose engine speaks neither wire keeps the DNS-form endpoint
//! this service has always reported and gets no listener.
//!
//! Only the WRITER endpoint is served. The reader endpoint load-balances across a cluster's
//! Aurora Replicas, which reject writes; publishing the writer's address under it would let
//! a client write through a connection that AWS answers read-only, so the reader endpoint
//! keeps its DNS name.
//!
//! Everything a session on such a listener needs is the same as on a DB instance's — the
//! two wires, the SQL engine, the server statements are shared — except WHERE the facts of
//! a login come from: a cluster's master user, engine version, status and parameter group
//! live on the CLUSTER record, which is what [`cluster_facts`] reads.

use std::sync::Arc;

use super::sql_engine::InstanceDatabase;
use super::wire_session::{
    facts_of, parameter_group_family, running_engine, EndpointKind, EngineRuntime, InstanceFacts,
    RecordFacts,
};
use super::EngineWire;
use crate::state::{ComAmazonawsRdsAmazonrdsv19State, DbClusterRecord};

/// The wire the engine of a DB CLUSTER speaks, or `None` for a cluster this emulator
/// serves no data plane for.
///
/// A cluster's data plane is addressed through the cluster's own endpoints, whichever of
/// the cluster types it is, so the engines named here are the documented `Engine` values of
/// both: an Aurora cluster (`aurora-postgresql`, `aurora-mysql`, and the `aurora` alias of
/// Aurora MySQL 5.6) and a Multi-AZ DB cluster (`postgres`, `mysql`). Aurora PostgreSQL and
/// RDS for PostgreSQL speak the PostgreSQL frontend/backend protocol and Aurora MySQL and
/// RDS for MySQL the MySQL client/server protocol, which are the two wires this emulator
/// serves.
///
/// Every other engine `CreateDBCluster` accepts is deliberately absent, so it gets no
/// listener: an Amazon DocumentDB cluster speaks the MongoDB wire and an Amazon Neptune
/// cluster the graph protocols, neither of which this emulator runs, and publishing a port
/// for one would be worse than publishing a name. The engine names are matched exactly, as
/// the sibling [`super::engine_wire`] matches an instance's.
#[must_use]
pub fn cluster_engine_wire(engine: &str) -> Option<EngineWire> {
    match engine.to_ascii_lowercase().as_str() {
        "aurora-postgresql" | "postgres" => Some(EngineWire::Postgres),
        "aurora-mysql" | "aurora" | "mysql" => Some(EngineWire::MySql),
        _ => None,
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The DB cluster in `region` whose `DbClusterResourceId` is `resource_id`.
    ///
    /// A session on a cluster's engine endpoint resolves its cluster by this id rather than
    /// by identifier, because `ModifyDBCluster` can rename a cluster under a live
    /// connection while the resource id is "Region-unique, immutable". Reads one region's
    /// ordered range, which the region's own cluster quota bounds.
    #[must_use]
    pub fn cluster_by_resource_id(
        &self,
        region: &str,
        resource_id: &str,
    ) -> Option<&DbClusterRecord> {
        self.db_clusters
            .range((region.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == region)
            .map(|(_, record)| record)
            .find(|record| record.resource_id == resource_id)
    }

    /// The engine endpoint of the DB cluster `key` names, for the clusters that have one.
    #[must_use]
    pub fn cluster_engine_endpoint(
        &self,
        key: &crate::state::RegionalKey,
    ) -> Option<&super::DbEngineEndpoint> {
        self.db_cluster_engine_endpoints.get(key)
    }

    /// Move a cluster's engine endpoint with the cluster when `ModifyDBCluster` renames it,
    /// so the listener a client is connected to stays the endpoint the control plane
    /// publishes for that cluster.
    pub fn move_cluster_engine_endpoint(
        &mut self,
        from: &crate::state::RegionalKey,
        to: &crate::state::RegionalKey,
    ) {
        if from == to {
            return;
        }
        if let Some(endpoint) = self.db_cluster_engine_endpoints.remove(from) {
            self.db_cluster_engine_endpoints.insert(to.clone(), endpoint);
        }
    }

    /// Whether a session on the cluster `record`'s engine endpoint must be encrypted.
    ///
    /// The engines carry this as a PARAMETER, so the answer is the effective value of that
    /// parameter for this cluster — read through the ONE reading of it this module tree has
    /// ([`super::secure_transport_in_force`]), so a cluster's endpoint and an instance's
    /// cannot disagree about a plaintext login: what the cluster's own DB CLUSTER parameter
    /// group sets for it, and the engine's own default where the group sets nothing (a group
    /// this store holds no record of is a default group AWS provides, which sets no
    /// parameter of its own).
    #[must_use]
    pub fn cluster_requires_secure_transport(&self, record: &DbClusterRecord) -> bool {
        let Some(wire) = cluster_engine_wire(&record.engine) else {
            return false;
        };
        let set = record
            .db_cluster_parameter_group_name
            .as_ref()
            .and_then(|name| {
                self.db_cluster_parameter_groups
                    .get(&(record.region.clone(), name.to_ascii_lowercase()))
            })
            .and_then(|group| {
                group
                    .parameters
                    .get(super::secure_transport_parameter(wire))
            });
        super::secure_transport_in_force(
            wire,
            set.map(|setting| setting.value.as_str()),
            record.engine_version.as_deref(),
        )
    }
}

/// The facts a login on the cluster `region`/`resource_id` is admitted against, read under
/// the state lock the caller holds.
///
/// What the CLUSTER record says is read here; everything both kinds of endpoint derive the
/// same way is [`super::wire_session::facts_of`]'s, so the two cannot drift apart.
///
/// `None` when the store holds no such cluster any more, or when its engine speaks no wire
/// this emulator serves — both of which the wire answers as an engine that is not there.
pub(super) fn cluster_facts(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    region: &str,
    resource_id: &str,
    database: &Arc<InstanceDatabase>,
) -> Option<InstanceFacts> {
    let record = store.cluster_by_resource_id(region, resource_id)?;
    let wire = cluster_engine_wire(&record.engine)?;
    Some(facts_of(
        RecordFacts {
            kind: EndpointKind::DbCluster,
            identifier: record.identifier.clone(),
            engine: record.engine.clone(),
            status: record.status.clone(),
            wire,
            engine_version: record.engine_version.clone(),
            database_name: record.database_name.clone(),
            master_username: record.master_username.clone(),
            password: record.master_user_password.clone(),
            requires_secure_transport: store.cluster_requires_secure_transport(record),
            served_host: store
                .cluster_engine_endpoint(&(record.region.clone(), record.identifier.clone()))
                .map(|endpoint| endpoint.endpoint.host.clone()),
            runtime: cluster_runtime_of(store, record),
        },
        database,
    ))
}

/// The engine runtime of the cluster `record`, read under the state lock the caller holds.
///
/// The parameters are the cluster-level ones — a DB CLUSTER parameter group's own catalog
/// ([`crate::state::published_cluster_parameters`]) over the values the cluster's group sets
/// — because those are the parameters a cluster is configured by. A cluster's engine came up
/// when the cluster was created: nothing this service serves restarts it, so the creation
/// time is when the server has been running since.
pub(super) fn cluster_runtime_of(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    record: &DbClusterRecord,
) -> EngineRuntime {
    let group = record
        .db_cluster_parameter_group_name
        .as_ref()
        .and_then(|name| {
            store
                .db_cluster_parameter_groups
                .get(&(record.region.clone(), name.to_ascii_lowercase()))
        });
    let family = parameter_group_family(
        group.map(|group| group.family.as_str()),
        record.db_cluster_parameter_group_name.as_deref(),
    );
    running_engine(
        &family,
        crate::state::published_cluster_parameters(&family),
        group.map(|group| &group.parameters),
        record.cluster_create_time.as_secs_f64(),
        record.master_username.clone(),
    )
}
