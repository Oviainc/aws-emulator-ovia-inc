// SPDX-License-Identifier: CC0-1.0

//! The CONNECTOR concern of this service's state: the connectors `CreateConnector`
//! mints and `ListConnectors` reports, and the refusals those operations word.
//!
//! A connector is the service's handle on one SSM-managed instance that runs migration
//! commands. The record holds exactly what the caller supplied plus the minted id and
//! ARN, so a listing reports what the create stored and nothing it computes per read.

use super::*;

/// The most connectors this store holds at a time — per account AND in total.
///
/// The emulator's own bound on a caller-driven growth site, not a claim about any AWS
/// quota: the same reasoning as [`APPLICATION_QUOTA`]. `CreateConnector` checks
/// [`ComAmazonawsMgnApplicationmigrationserviceState::connector_store_is_full`] before
/// it mints anything and refuses past it with the modelled `ValidationException` — the
/// only client refusal the operation models besides the account gate.
pub const CONNECTOR_QUOTA: usize = 1_000;

/// The `connectorID` prefix the model's `^connector-[0-9a-zA-Z]{17}$` pattern requires.
const CONNECTOR_ID_PREFIX: &str = "connector-";

/// How many `[0-9a-zA-Z]` characters follow the prefix of a `ConnectorID`
/// (model pattern `^connector-[0-9a-zA-Z]{17}$`, length 27..27).
const CONNECTOR_ID_BODY_LEN: usize = 17;

/// One stored connector: everything `CreateConnector` and `ListConnectors` report.
#[derive(Debug, Clone, Default)]
pub struct Connector {
    pub connector_id: String,
    pub arn: String,
    pub name: String,
    /// The SSM instance the connector runs commands on, in the model's `SsmInstanceID`
    /// form (`i-` or `mi-` followed by 17 `[0-9a-zA-Z]` characters).
    pub ssm_instance_id: String,
    /// The request's `ssmCommandConfig`, or absent when it named none.
    pub ssm_command_config: Option<ConnectorSsmCommandConfig>,
    pub tags: BTreeMap<String, String>,
    /// The account the connector lives in, so one account's connectors are never
    /// another's to read.
    pub account_id: String,
}

impl Connector {
    /// The `tags` member: the stored tags, or absent when the connector carries none.
    pub fn tags_shape(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            None
        } else {
            Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        }
    }

    /// This connector as the model's `Connector` structure — the output shape
    /// `CreateConnector` returns and `ListConnectors` lists.
    pub fn to_shape(&self) -> CreateConnectorOutput {
        CreateConnectorOutput {
            connector_id: Some(self.connector_id.clone()),
            name: Some(self.name.clone()),
            ssm_instance_id: Some(self.ssm_instance_id.clone()),
            arn: Some(self.arn.clone()),
            tags: self.tags_shape(),
            ssm_command_config: self.ssm_command_config.clone(),
        }
    }

    /// Whether this connector carries exactly what a `CreateConnector` request asked
    /// for: the operation is `@idempotent` and models no idempotency token, so a repeat
    /// of a request every member of which matches a stored connector is a replay of that
    /// connector rather than a second one.
    pub fn matches_request(
        &self,
        name: &str,
        ssm_instance_id: &str,
        ssm_command_config: Option<&ConnectorSsmCommandConfig>,
        tags: &BTreeMap<String, String>,
    ) -> bool {
        self.name == name
            && self.ssm_instance_id == ssm_instance_id
            && self.tags == *tags
            && same_ssm_command_config(self.ssm_command_config.as_ref(), ssm_command_config)
    }
}

/// Whether two `ssmCommandConfig` members are the same configuration — the generated
/// shape carries no equality impl, so the comparison is over its members.
fn same_ssm_command_config(
    stored: Option<&ConnectorSsmCommandConfig>,
    wanted: Option<&ConnectorSsmCommandConfig>,
) -> bool {
    match (stored, wanted) {
        (None, None) => true,
        (Some(stored), Some(wanted)) => {
            stored.s3_output_enabled == wanted.s3_output_enabled
                && stored.output_s3_bucket_name == wanted.output_s3_bucket_name
                && stored.cloud_watch_output_enabled == wanted.cloud_watch_output_enabled
                && stored.cloud_watch_log_group_name == wanted.cloud_watch_log_group_name
        }
        _ => false,
    }
}

impl ComAmazonawsMgnApplicationmigrationserviceState {
    /// Every connector of `account_id`, in creation order.
    pub fn connectors_of(&self, account_id: &str) -> Vec<&Connector> {
        self.connector_order
            .iter()
            .filter_map(|id| self.connectors.get(id))
            .filter(|connector| connector.account_id == account_id)
            .collect()
    }

    /// The connector `connector_id` names within `account_id`, or `None` — a connector
    /// of another account is not visible to this one.
    pub fn connector_of(&self, account_id: &str, connector_id: &str) -> Option<&Connector> {
        self.connectors
            .get(connector_id)
            .filter(|connector| connector.account_id == account_id)
    }

    /// A fresh `connectorID` no stored connector holds.
    pub fn mint_connector_id(&self) -> String {
        loop {
            let candidate = mint_id(CONNECTOR_ID_PREFIX, CONNECTOR_ID_BODY_LEN);
            if !self.connectors.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the connector store is full: either the account already holds
    /// [`CONNECTOR_QUOTA`] connectors, or the whole store does.
    pub fn connector_store_is_full(&self, account_id: &str) -> bool {
        self.connectors.len() >= CONNECTOR_QUOTA
            || self.connectors_of(account_id).len() >= CONNECTOR_QUOTA
    }

    /// Store a newly created connector, remembering its place in creation order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does: a
    /// store already at [`CONNECTOR_QUOTA`] refuses rather than growing past the bound,
    /// and re-storing an id the map already holds keeps its place in the creation order
    /// rather than duplicating it.
    pub fn insert_connector(&mut self, connector: Connector) -> bool {
        let known = self.connectors.contains_key(&connector.connector_id);
        if !known && self.connectors.len() >= CONNECTOR_QUOTA {
            return false;
        }
        if !known {
            self.connector_order.push(connector.connector_id.clone());
        }
        self.connectors
            .insert(connector.connector_id.clone(), connector);
        true
    }
}

/// The ARN of a connector: `arn:aws:mgn:<region>:<account>:connector/<connectorID>`.
pub fn connector_arn(region: &str, connector_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("connector/{connector_id}"))
}

/// The modelled refusal when the emulator's own [`CONNECTOR_QUOTA`] is reached:
/// `CreateConnector` models no `ServiceQuotaExceededException`, so the request is
/// refused with the one client error it does model.
pub fn connector_quota_exceeded(account_id: &str) -> ValidationException {
    ValidationException {
        message: Some(format!(
            "Account {account_id} already holds the maximum of {CONNECTOR_QUOTA} connectors."
        )),
        code: None,
        reason: None,
        field_list: None,
    }
}
