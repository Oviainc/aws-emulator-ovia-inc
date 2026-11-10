// SPDX-License-Identifier: CC0-1.0

//! The REPLICATION CONFIGURATION TEMPLATE concern of this service's state: the
//! templates an account holds, which `DescribeReplicationConfigurationTemplates`
//! reports.
//!
//! A template is addressed by its `replicationConfigurationTemplateID`, and the record
//! holds exactly the replication settings a write supplied plus the id, the ARN and the
//! account it lives in — so a read reports what was stored and computes nothing per
//! read.

use super::*;

/// The most replication configuration templates this store holds at a time, over every
/// account.
///
/// The emulator's own bound on the template growth site, not a claim about any AWS
/// quota: the same reasoning as [`APPLICATION_QUOTA`]. Whatever stores a template goes
/// through
/// [`ComAmazonawsMgnApplicationmigrationserviceState::insert_replication_configuration_template`],
/// which refuses past the bound rather than growing the heap without end. One bound on
/// the whole store bounds every account's count with it, so there is no second,
/// per-account comparison that could never be the one to refuse.
pub const REPLICATION_CONFIGURATION_TEMPLATE_QUOTA: usize = 1_000;

/// One stored replication configuration template: everything the model's
/// `ReplicationConfigurationTemplate` structure reports.
#[derive(Debug, Clone, Default)]
pub struct ReplicationConfigurationTemplate {
    pub replication_configuration_template_id: String,
    pub arn: String,
    /// The account the template lives in, so one account's templates are never
    /// another's to read.
    pub account_id: String,
    pub staging_area_subnet_id: Option<String>,
    pub associate_default_security_group: Option<bool>,
    pub replication_servers_security_groups_ids: Vec<String>,
    pub replication_server_instance_type: Option<String>,
    pub use_dedicated_replication_server: Option<bool>,
    /// One of the model's `ReplicationConfigurationDefaultLargeStagingDiskType` values.
    pub default_large_staging_disk_type: Option<String>,
    /// One of the model's `ReplicationConfigurationEbsEncryption` values.
    pub ebs_encryption: Option<String>,
    pub ebs_encryption_key_arn: Option<String>,
    /// The model's `BandwidthThrottling` (range 0..10000, default 0), which the shape
    /// carries as a required member rather than an optional one. The range is a
    /// modelled constraint, so it is refused at the request boundary of whatever
    /// operation supplies it, ahead of this store.
    pub bandwidth_throttling: i64,
    /// One of the model's `ReplicationConfigurationDataPlaneRouting` values.
    pub data_plane_routing: Option<String>,
    pub create_public_ip: Option<bool>,
    pub staging_area_tags: BTreeMap<String, String>,
    pub use_fips_endpoint: Option<bool>,
    pub tags: BTreeMap<String, String>,
    /// One of the model's `InternetProtocol` values.
    pub internet_protocol: Option<String>,
    pub store_snapshot_on_local_zone: Option<bool>,
    pub storage_configuration: Option<StorageConfiguration>,
}

impl ReplicationConfigurationTemplate {
    /// This template as the model's `ReplicationConfigurationTemplate` structure — the
    /// shape `DescribeReplicationConfigurationTemplates` lists.
    pub fn to_shape(&self) -> CreateReplicationConfigurationTemplateOutput {
        CreateReplicationConfigurationTemplateOutput {
            replication_configuration_template_id: self
                .replication_configuration_template_id
                .clone(),
            arn: Some(self.arn.clone()),
            staging_area_subnet_id: self.staging_area_subnet_id.clone(),
            associate_default_security_group: self.associate_default_security_group,
            replication_servers_security_groups_i_ds: list_shape(
                &self.replication_servers_security_groups_ids,
            ),
            replication_server_instance_type: self.replication_server_instance_type.clone(),
            use_dedicated_replication_server: self.use_dedicated_replication_server,
            default_large_staging_disk_type: self.default_large_staging_disk_type.clone(),
            ebs_encryption: self.ebs_encryption.clone(),
            ebs_encryption_key_arn: self.ebs_encryption_key_arn.clone(),
            bandwidth_throttling: self.bandwidth_throttling,
            data_plane_routing: self.data_plane_routing.clone(),
            create_public_ip: self.create_public_ip,
            staging_area_tags: map_shape(&self.staging_area_tags),
            use_fips_endpoint: self.use_fips_endpoint,
            tags: map_shape(&self.tags),
            internet_protocol: self.internet_protocol.clone(),
            store_snapshot_on_local_zone: self.store_snapshot_on_local_zone,
            storage_configuration: self.storage_configuration.clone(),
        }
    }
}

impl ComAmazonawsMgnApplicationmigrationserviceState {
    /// Every replication configuration template of `account_id`, in creation order.
    pub fn replication_configuration_templates_of(
        &self,
        account_id: &str,
    ) -> Vec<&ReplicationConfigurationTemplate> {
        self.replication_configuration_template_order
            .iter()
            .filter_map(|id| self.replication_configuration_templates.get(id))
            .filter(|template| template.account_id == account_id)
            .collect()
    }

    /// Store a replication configuration template, remembering its place in creation
    /// order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does: a
    /// store already at [`REPLICATION_CONFIGURATION_TEMPLATE_QUOTA`] refuses rather than
    /// growing past the bound, and re-storing an id the map already holds keeps its
    /// place in the creation order rather than duplicating it.
    ///
    /// The account separation is the STORE's property, not each caller's: the map is
    /// keyed by id alone, so an insert under an id another account already holds is
    /// refused rather than replacing that account's record (and its `account_id` with
    /// it).
    pub fn insert_replication_configuration_template(
        &mut self,
        template: ReplicationConfigurationTemplate,
    ) -> bool {
        let id = template.replication_configuration_template_id.clone();
        let held = self.replication_configuration_templates.get(&id);
        if held.is_some_and(|held| held.account_id != template.account_id) {
            return false;
        }
        let known = held.is_some();
        if !known
            && self.replication_configuration_templates.len()
                >= REPLICATION_CONFIGURATION_TEMPLATE_QUOTA
        {
            return false;
        }
        if !known {
            self.replication_configuration_template_order
                .push(id.clone());
        }
        self.replication_configuration_templates.insert(id, template);
        true
    }
}

/// The ARN of a replication configuration template:
/// `arn:aws:mgn:<region>:<account>:replication-configuration-template/<replicationConfigurationTemplateID>`.
pub fn replication_configuration_template_arn(region: &str, template_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("replication-configuration-template/{template_id}"),
    )
}
